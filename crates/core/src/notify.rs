//! Telling the user what happened while they were not looking.
//!
//! Four things are worth it: an agent asks a question, an agent finished its
//! turn, the checks of a merge request failed, a merge request became ready
//! to merge. Each is told once, at the moment it happens, and only when no
//! window of the editor is in front: a user who is looking sees it anyway.
//!
//! The core decides, not the windows. Every connected window says whether it
//! is in front (`notifications.attention`), so two windows never tell the same
//! thing twice and a window in the background cannot tell what the one in
//! front already shows. A notice then goes out one of two ways:
//!
//! - inside the desktop app, through the "notify" the app lends the core. That
//!   works with the app's window closed, when no page is there to show anything;
//! - otherwise as a `notification` event, which a page in a browser shows with
//!   the browser's own notifications.
//!
//! Nothing is told until the user turns notifications on in Settings, and each
//! of the four can be turned off there.

use crate::gitops;
use crate::hosts::{Request, remote_of};
use crate::tasks::{Manager, now_ms};
use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// Where the user's choices are kept, as one JSON object.
const CHOICES: &str = "notifications";

/// How much of a question or an error goes into a notice, in characters.
const DETAIL_LIMIT: usize = 200;

/// What is worth telling.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Question,
    TurnEnd,
    ChecksFailed,
    ReadyToMerge,
}

impl Kind {
    /// The name the interface knows it by; also the name of its switch in Settings.
    pub fn name(self) -> &'static str {
        match self {
            Kind::Question => "question",
            Kind::TurnEnd => "turnEnd",
            Kind::ChecksFailed => "checksFailed",
            Kind::ReadyToMerge => "readyToMerge",
        }
    }

    /// The view of the task a click on the notice opens: the conversation, or the merge request.
    fn view(self) -> &'static str {
        match self {
            Kind::Question | Kind::TurnEnd => "code",
            Kind::ChecksFailed | Kind::ReadyToMerge => "changes",
        }
    }
}

/// One thing told to the user.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Notice {
    /// The same in every window that shows it, so two windows of one browser show it once.
    pub id: String,
    pub kind: &'static str,
    pub task_id: String,
    /// The view of the task a click opens.
    pub view: &'static str,
    /// Who, and what happened.
    pub title: String,
    /// Where: the task and its project, then the question or the error itself.
    pub body: String,
}

/// What the user wants to be told about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct Choices {
    /// Off until the user turns notifications on once. With this off nothing is told.
    pub on: bool,
    pub question: bool,
    pub turn_end: bool,
    pub checks_failed: bool,
    pub ready_to_merge: bool,
}

impl Default for Choices {
    fn default() -> Self {
        Self { on: false, question: true, turn_end: true, checks_failed: true, ready_to_merge: true }
    }
}

impl Choices {
    fn wants(self, kind: Kind) -> bool {
        self.on
            && match kind {
                Kind::Question => self.question,
                Kind::TurnEnd => self.turn_end,
                Kind::ChecksFailed => self.checks_failed,
                Kind::ReadyToMerge => self.ready_to_merge,
            }
    }
}

/// How the desktop app shows a notice. It must come back at once: it is called
/// in the middle of handling an event.
pub type Show = Arc<dyn Fn(&Notice) + Send + Sync>;

/// Who is looking, and the desktop app's way to tell.
#[derive(Default)]
pub struct Notifier {
    /// Every connected window, with whether it is in front right now.
    windows: Mutex<HashMap<u64, bool>>,
    next_window: AtomicU64,
    by_app: Mutex<Option<Show>>,
}

impl Notifier {
    fn someone_is_looking(&self) -> bool {
        self.windows.lock().unwrap().values().any(|front| *front)
    }
}

/// One connected window, for as long as its connection lasts.
pub struct Window {
    id: u64,
    manager: Arc<Manager>,
}

impl Window {
    pub fn id(&self) -> u64 {
        self.id
    }
}

impl Drop for Window {
    fn drop(&mut self) {
        // A window that is gone is not looking.
        self.manager.notifier.windows.lock().unwrap().remove(&self.id);
    }
}

/// The stage of a merge request that is worth telling: its checks failed, or it can merge.
fn stage_of(request: &Request) -> Option<Kind> {
    if request.state == "open" && request.checks.state == "failed" {
        Some(Kind::ChecksFailed)
    } else if request.ready() {
        Some(Kind::ReadyToMerge)
    } else {
        None
    }
}

/// Where it is kept in which stage a task's request was last seen. A stage is news
/// once, when the request gets there, whether or not anybody had to be told then.
fn told_key(task_id: &str) -> String {
    format!("task.{task_id}.requestTold")
}

fn cut(text: &str) -> String {
    let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if text.chars().count() <= DETAIL_LIMIT {
        return text;
    }
    let mut short: String = text.chars().take(DETAIL_LIMIT - 1).collect();
    short.push('…');
    short
}

fn capitalized(text: &str) -> String {
    let mut letters = text.chars();
    match letters.next() {
        Some(first) => first.to_uppercase().chain(letters).collect(),
        None => String::new(),
    }
}

/// How a turn ended, as the rest of "Claude …".
fn ending(stop_reason: &str) -> String {
    match stop_reason {
        "end_turn" => "finished its turn".into(),
        "max_tokens" => "stopped: it ran out of room in its context".into(),
        "max_turn_requests" => "stopped: it reached its limit of steps for one turn".into(),
        "refusal" => "refused to continue".into(),
        other => format!("stopped: {other}"),
    }
}

impl Manager {
    // ----- who is looking -----

    /// Registers a connected window. It counts as "not in front" until it says otherwise.
    pub fn window_opened(self: &Arc<Self>) -> Window {
        // Zero is "no window": a command that came in some other way than from an interface.
        let id = self.notifier.next_window.fetch_add(1, Ordering::SeqCst) + 1;
        self.notifier.windows.lock().unwrap().insert(id, false);
        Window { id, manager: self.clone() }
    }

    /// Takes note of whether a window is in front: on screen and the one the user types into.
    pub fn window_attention(&self, window: u64, front: bool) {
        if let Some(known) = self.notifier.windows.lock().unwrap().get_mut(&window) {
            *known = front;
        }
    }

    /// The desktop app lends its way of showing a notice.
    pub fn notify_through(&self, show: Show) {
        *self.notifier.by_app.lock().unwrap() = Some(show);
    }

    // ----- what the user wants to be told -----

    pub fn notification_choices(&self) -> Choices {
        self.store()
            .setting(CHOICES)
            .ok()
            .flatten()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// The choices as Settings shows them. `byApp` says the desktop app shows
    /// the notices itself, so a page need not ask its browser for permission.
    pub(crate) fn notifications_as_set(&self) -> Value {
        let mut choices = json!(self.notification_choices());
        choices["byApp"] = json!(self.notifier.by_app.lock().unwrap().is_some());
        choices
    }

    /// Changes the choices that are named and leaves the others as they are.
    pub(crate) fn set_notifications(&self, change: &Value) -> Result<()> {
        let Some(change) = change.as_object() else { bail!("Say which notifications to change") };
        let mut choices = json!(self.notification_choices());
        for (name, value) in change {
            if choices.get(name).is_none() {
                bail!("There is no notification called {name}");
            }
            choices[name] = json!(value.as_bool().ok_or_else(|| anyhow!("{name} is on or off"))?);
        }
        self.store().set_setting(CHOICES, Some(&choices.to_string()))
    }

    // ----- telling -----

    /// Looks at an event of a task as it is written down: some are worth
    /// telling, and a change of state says whether an agent is at work.
    pub(crate) fn after_event(&self, task_id: &str, event: &Value) {
        let text = |name: &str| event.get(name).and_then(Value::as_str).unwrap_or_default();
        let id = || format!("{task_id}:{}", event.get("seq").and_then(Value::as_u64).unwrap_or(0));
        match text("type") {
            "state" => self.agent_works(task_id, text("state") == "running"),
            "permission_request" => {
                let mut asked =
                    event.pointer("/toolCall/title").and_then(Value::as_str).unwrap_or_default().to_string();
                // Agents name files by their full path; inside a task the path from its folder reads better.
                if let Ok(cwd) = self.cwd(task_id) {
                    asked = asked.replace(&format!("{}/", cwd.to_string_lossy()), "");
                }
                self.tell(Kind::Question, task_id, id(), |agent| format!("{agent} asks a question"), &asked);
            }
            // A turn the user stopped is no news to the user.
            "turn_end" if text("stopReason") != "cancelled" => {
                let how = ending(text("stopReason"));
                self.tell(Kind::TurnEnd, task_id, id(), |agent| format!("{agent} {how}"), "");
            }
            // An agent that is gone (stopped by the user, or with the core) answers nothing: that is
            // code -32000, and it is no end of a turn. Anything else is the agent saying why it failed.
            "error" if event.get("code").and_then(Value::as_i64) != Some(-32000) => {
                self.tell(
                    Kind::TurnEnd,
                    task_id,
                    id(),
                    |agent| format!("{agent} stopped with an error"),
                    text("message"),
                );
            }
            _ => {}
        }
    }

    /// Tells the user one thing, unless they did not ask for it or are looking at the editor.
    fn tell(&self, kind: Kind, task_id: &str, id: String, title: impl FnOnce(&str) -> String, detail: &str) {
        if !self.notification_choices().wants(kind) || self.notifier.someone_is_looking() {
            return;
        }
        // A task that was archived meanwhile has nothing left to open.
        let Ok(task) = self.store().task(task_id) else { return };
        let agent =
            self.registry().get(&task.agent_id).map_or_else(|| task.agent_id.clone(), |agent| agent.name.clone());
        let mut body = format!("{} · {}", task.title, self.project_name(&task.project_id));
        let detail = cut(detail);
        if !detail.is_empty() {
            body = format!("{body}\n{detail}");
        }
        let notice = Notice { id, kind: kind.name(), task_id: task.id, view: kind.view(), title: title(&agent), body };
        let by_app = self.notifier.by_app.lock().unwrap().clone();
        if let Some(show) = &by_app {
            show(&notice);
        }
        let mut told = json!(notice);
        // A page must not show what the app has shown already.
        told["byApp"] = json!(by_app.is_some());
        self.broadcast("notification", told);
    }

    // ----- merge requests -----

    /// Called whenever a host said how a task's request stands. Tells the user
    /// when the checks have just failed or the request has just become ready
    /// to merge: once for each time it gets there, however often it is looked at.
    pub(crate) fn request_learned(&self, task_id: &str, request: Option<&Request>) {
        // No request to speak of may be a host that did not answer: that changes nothing.
        let Some(request) = request else { return };
        let stage = stage_of(request);
        let told = self.store().setting(&told_key(task_id)).ok().flatten();
        if told.as_deref() == stage.map(Kind::name) {
            return;
        }
        let _ = self.store().set_setting(&told_key(task_id), stage.map(Kind::name));
        let Some(kind) = stage else { return };
        // Nothing more is looked up for a notice nobody will see: this runs for every request any screen reads.
        if !self.notification_choices().wants(kind) || self.notifier.someone_is_looking() {
            return;
        }
        // A request that is about to merge by itself needs nobody.
        if kind == Kind::ReadyToMerge && self.auto_merge(task_id).is_some() {
            return;
        }
        let number = request.number;
        let id = format!("{task_id}:request:{number}:{}:{}", kind.name(), now_ms());
        let title = |_: &str| {
            // What the host calls a request is asked of git, and only here: with nobody to
            // tell there is nothing to word, and a request is looked at far more often than told.
            let word = self
                .cwd(task_id)
                .ok()
                .and_then(|root| remote_of(&root))
                .map_or("merge request", |remote| remote.kind.request_word());
            match kind {
                Kind::ChecksFailed => format!("The checks of {word} {number} failed"),
                _ => format!("{} {number} is ready to merge", capitalized(word)),
            }
        };
        self.tell(kind, task_id, id, title, "");
    }

    /// Asks the hosts how every pushed task's request stands, so a change is
    /// noticed with no window open. Only when the user wants to be told about
    /// requests: otherwise nobody is asked. Blocks and uses the network.
    pub fn look_at_requests(&self) {
        let choices = self.notification_choices();
        if !(choices.wants(Kind::ChecksFailed) || choices.wants(Kind::ReadyToMerge)) {
            return;
        }
        for task in self.store().tasks().unwrap_or_default() {
            let Some(branch) = task.branch.clone() else { continue };
            let root = PathBuf::from(&task.cwd);
            // A branch that was never pushed cannot have a request; do not ask the host about it.
            if !gitops::is_pushed(&root, &branch) {
                continue;
            }
            let Some(remote) = remote_of(&root) else { continue };
            let asked = Instant::now();
            // A host that cannot be reached now is asked again in the next round.
            let Ok(request) = self.hosts.request(&remote, &branch) else { continue };
            let stood = self.store().setting(&told_key(&task.id)).ok().flatten();
            self.remember_request(&task.id, request, asked);
            // No window asked for this look, so none knows what it found: the open ones are told to
            // read the request again. Home then shows it where it stands, as the notice says.
            if self.store().setting(&told_key(&task.id)).ok().flatten() != stood {
                self.broadcast("mr.changed", json!({ "taskId": task.id }));
            }
        }
    }

    /// `look_at_requests`, off the event loop.
    pub async fn requests_round(self: &Arc<Self>) {
        let manager = self.clone();
        let _ = tokio::task::spawn_blocking(move || manager.look_at_requests()).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hosts::Checks;

    fn request(state: &'static str, checks: &'static str) -> Request {
        Request {
            number: 128,
            title: "T".into(),
            url: "https://github.com/x/shop/pull/128".into(),
            state,
            draft: false,
            mergeable: Some(true),
            checks: Checks { state: checks, ..Checks::default() },
            review: "none",
        }
    }

    #[test]
    fn a_request_is_worth_telling_when_its_checks_failed_or_it_can_merge() {
        assert_eq!(stage_of(&request("open", "failed")), Some(Kind::ChecksFailed));
        assert_eq!(stage_of(&request("open", "passed")), Some(Kind::ReadyToMerge));
        assert_eq!(stage_of(&request("open", "none")), Some(Kind::ReadyToMerge));
        assert_eq!(stage_of(&request("open", "pending")), None);
        // Merged or closed: there is nothing left for the user to do about its checks.
        assert_eq!(stage_of(&request("merged", "failed")), None);
        assert_eq!(stage_of(&request("closed", "passed")), None);
        let mut waits = request("open", "passed");
        waits.review = "changes_requested";
        assert_eq!(stage_of(&waits), None);
    }

    #[test]
    fn nothing_is_wanted_until_notifications_are_turned_on() {
        let fresh = Choices::default();
        for kind in [Kind::Question, Kind::TurnEnd, Kind::ChecksFailed, Kind::ReadyToMerge] {
            assert!(!fresh.wants(kind));
            assert!(Choices { on: true, ..fresh }.wants(kind));
        }
        let quiet_ends = Choices { on: true, turn_end: false, ..fresh };
        assert!(!quiet_ends.wants(Kind::TurnEnd));
        assert!(quiet_ends.wants(Kind::Question));
        // What was kept by an older version, with a switch missing, reads as that switch being on.
        let kept: Choices = serde_json::from_str(r#"{"on":true,"question":false}"#).unwrap();
        assert!(!kept.wants(Kind::Question) && kept.wants(Kind::ReadyToMerge));
    }

    #[test]
    fn the_words_of_a_notice() {
        assert_eq!(ending("end_turn"), "finished its turn");
        assert_eq!(ending("max_tokens"), "stopped: it ran out of room in its context");
        assert_eq!(ending("something_new"), "stopped: something_new");
        assert_eq!(capitalized("pull request"), "Pull request");
        assert_eq!(cut("  two\n lines  "), "two lines");
        let long = cut(&"word ".repeat(100));
        assert_eq!(long.chars().count(), DETAIL_LIMIT);
        assert!(long.ends_with('…'));
    }
}
