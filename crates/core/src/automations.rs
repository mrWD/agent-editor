//! Automations: work that starts without the user.
//!
//! An automation is "when this happens, do that", for all projects or some:
//! - when a request is merged: clean up after the task;
//! - when a request is opened: run the task's checks;
//! - every day at a time: start a task with an agent and a text;
//! - when a new issue has a label: start a task from that issue.
//!
//! Two are built in and can only be turned on and off; the rest are the
//! user's. Every run that starts an agent shows up as a normal task, so
//! nothing an automation does is hidden. An automation never does what the
//! user could not do by hand, and the one that deletes things holds back
//! when there is work that would be lost.

use crate::gitops;
use crate::tasks::{Manager, now_ms};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashSet;
use std::path::Path;
use std::sync::Arc;

/// What sets an automation off.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum When {
    /// A task's request was merged, here or on the host.
    Merged,
    /// A request was opened for a task.
    RequestOpened,
    /// Every day at this time of the user's clock, as "HH:MM".
    Daily { at: String },
    /// A new open issue of a project carries this label.
    IssueLabeled { label: String },
}

/// What an automation does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum Then {
    /// Archive the task and delete its working copy and branch.
    CleanUp,
    /// Run the task's checks: "all" that are on, or the end-to-end ones.
    RunChecks { which: String },
    /// Start a task with this agent and this text.
    StartTask {
        #[serde(rename = "agentId")]
        agent_id: String,
        text: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Automation {
    pub id: String,
    pub name: String,
    pub on: bool,
    pub when: When,
    pub then: Then,
    /// The projects it is for. Empty means all.
    #[serde(default)]
    pub projects: Vec<String>,
    #[serde(default)]
    pub built_in: bool,
    /// When it last ran, and what came of it.
    #[serde(default)]
    pub last_run: Option<u64>,
    #[serde(default)]
    pub last_note: Option<String>,
    /// How many times it ran on the day of `last_run` (the user's day).
    #[serde(default)]
    pub runs_that_day: u32,
    /// For "every day": the day it last ran on, counted in days since 1970 on the user's clock.
    #[serde(default)]
    pub ran_on_day: Option<i64>,
    /// For "new issue with a label": the issues it has already seen, so each starts one task.
    #[serde(default)]
    pub seen: Vec<String>,
}

const KEY: &str = "automations";
const CLOCK_KEY: &str = "automations.utcOffsetMinutes";
/// When the hosts were last asked for new issues, and how long to leave them alone after.
const ISSUES_KEY: &str = "automations.issuesLookedAt";
const ISSUES_EVERY_MS: u64 = 5 * 60 * 1000;

fn built_in() -> Vec<Automation> {
    let make = |id: &str, name: &str, when: When, then: Then| Automation {
        id: id.into(),
        name: name.into(),
        // Both are off until the user turns them on: the first deletes things, the second takes machine time.
        on: false,
        when,
        then,
        projects: vec![],
        built_in: true,
        last_run: None,
        last_note: None,
        runs_that_day: 0,
        ran_on_day: None,
        seen: vec![],
    };
    vec![
        make("clean-up", "Archive the task and delete its working copy and branch", When::Merged, Then::CleanUp),
        make("e2e", "Start the end-to-end tests", When::RequestOpened, Then::RunChecks { which: "e2e".into() }),
    ]
}

/// "HH:MM" as minutes of the day.
fn minutes_of(at: &str) -> Option<i64> {
    let (hours, minutes) = at.split_once(':')?;
    let (hours, minutes) = (hours.trim().parse::<i64>().ok()?, minutes.trim().parse::<i64>().ok()?);
    ((0..24).contains(&hours) && (0..60).contains(&minutes)).then_some(hours * 60 + minutes)
}

impl Manager {
    /// Every automation: the built-in ones first, then the user's.
    pub fn automations(&self) -> Vec<Automation> {
        let stored: Vec<Automation> = self
            .store()
            .setting(KEY)
            .ok()
            .flatten()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        let mut all = built_in();
        for one in &mut all {
            if let Some(kept) = stored.iter().find(|kept| kept.id == one.id) {
                // A built-in one keeps what it does; only its switch, its projects and its record are the user's.
                let (when, then, name) = (one.when.clone(), one.then.clone(), one.name.clone());
                *one = Automation { when, then, name, built_in: true, ..kept.clone() };
            }
        }
        all.extend(stored.into_iter().filter(|one| !one.built_in));
        all
    }

    fn save_automations(&self, all: &[Automation]) -> Result<()> {
        self.store().set_setting(KEY, Some(&serde_json::to_string(all)?))?;
        self.broadcast("automations.changed", json!({}));
        Ok(())
    }

    /// Turns an automation on or off.
    pub fn automation_turn(&self, id: &str, on: bool) -> Result<()> {
        let mut all = self.automations();
        let Some(one) = all.iter_mut().find(|one| one.id == id) else { bail!("No such automation") };
        one.on = on;
        if on && matches!(one.when, When::IssueLabeled { .. }) {
            // Issues that are open already are not news: only ones that come from now on start a task.
            one.seen = vec!["*".into()];
        }
        self.save_automations(&all)
    }

    /// Adds an automation of the user's own, or changes one. Returns its id.
    pub fn automation_save(
        &self,
        id: Option<&str>,
        name: &str,
        when: When,
        then: Then,
        projects: Vec<String>,
    ) -> Result<String> {
        let name = name.trim();
        if name.is_empty() {
            bail!("Give the automation a name");
        }
        match &when {
            When::Daily { at } if minutes_of(at).is_none() => {
                bail!("Give the time as hours and minutes, such as 03:00")
            }
            When::IssueLabeled { label } if label.trim().is_empty() => bail!("Give the label to look for"),
            _ => {}
        }
        if let Then::StartTask { agent_id, text } = &then {
            if text.trim().is_empty() {
                bail!("Say what the agent should do");
            }
            if self.registry().get(agent_id).is_none() {
                bail!("No such agent: {agent_id}");
            }
        }
        match (&when, &then) {
            (When::Daily { .. } | When::IssueLabeled { .. }, Then::StartTask { .. }) => {}
            (When::Merged, Then::CleanUp) | (When::RequestOpened | When::Merged, Then::RunChecks { .. }) => {}
            _ => bail!("These two do not go together: nothing here tells which task is meant"),
        }
        let known: HashSet<String> = self.store().projects()?.into_iter().map(|project| project.id).collect();
        if let Some(missing) = projects.iter().find(|id| !known.contains(*id)) {
            bail!("No such project: {missing}");
        }
        let mut all = self.automations();
        let id = match id {
            Some(id) => {
                let Some(one) = all.iter_mut().find(|one| one.id == id && !one.built_in) else {
                    bail!("Only an automation you made can be changed")
                };
                (one.name, one.when, one.then, one.projects) = (name.into(), when, then, projects);
                id.to_string()
            }
            None => {
                let id = format!("own-{}", uuid::Uuid::new_v4().simple().to_string().split_at(8).0);
                // Open issues at the time it is made are not news to it.
                let seen = if matches!(when, When::IssueLabeled { .. }) { vec!["*".into()] } else { vec![] };
                all.push(Automation {
                    id: id.clone(),
                    name: name.into(),
                    on: true,
                    when,
                    then,
                    projects,
                    built_in: false,
                    last_run: None,
                    last_note: None,
                    runs_that_day: 0,
                    ran_on_day: None,
                    seen,
                });
                id
            }
        };
        self.save_automations(&all)?;
        Ok(id)
    }

    pub fn automation_remove(&self, id: &str) -> Result<()> {
        let mut all = self.automations();
        let before = all.len();
        all.retain(|one| one.id != id || one.built_in);
        if all.len() == before {
            bail!("Only an automation you made can be removed. A built-in one can be turned off.");
        }
        self.save_automations(&all)
    }

    /// The user's clock, as minutes ahead of UTC. The interface tells it: the
    /// core cannot know where the user's day begins.
    pub fn set_clock(&self, utc_offset_minutes: i64) -> Result<()> {
        if !(-14 * 60..=14 * 60).contains(&utc_offset_minutes) {
            bail!("That is not an offset a clock can have");
        }
        self.store().set_setting(CLOCK_KEY, Some(&utc_offset_minutes.to_string()))
    }

    /// The minute of the user's day and the user's day number, for a moment.
    fn clock(&self, at_ms: u64) -> (i64, i64) {
        let offset =
            self.store().setting(CLOCK_KEY).ok().flatten().and_then(|text| text.parse::<i64>().ok()).unwrap_or(0);
        let local_minutes = (at_ms / 60_000) as i64 + offset;
        (local_minutes.rem_euclid(24 * 60), local_minutes.div_euclid(24 * 60))
    }

    /// Writes down that an automation ran, with what came of it.
    fn note_run(&self, id: &str, note: &str, change: impl FnOnce(&mut Automation)) {
        let mut all = self.automations();
        let now = now_ms();
        let (_, today) = self.clock(now);
        if let Some(one) = all.iter_mut().find(|one| one.id == id) {
            let same_day = one.last_run.is_some_and(|at| self.clock(at).1 == today);
            one.runs_that_day = if same_day { one.runs_that_day + 1 } else { 1 };
            one.last_run = Some(now);
            one.last_note = Some(note.to_string());
            change(one);
        }
        if let Err(error) = self.save_automations(&all) {
            tracing::debug!("Could not write down a run of {id}: {error:#}");
        }
    }

    fn is_for(one: &Automation, project_id: &str) -> bool {
        one.projects.is_empty() || one.projects.iter().any(|id| id == project_id)
    }

    /// Something happened to a task's request: the automations that wait for it run.
    /// Returns the tasks to clean up; the caller does that, because it also
    /// stops what the task runs.
    pub fn on_request_event(self: &Arc<Self>, task_id: &str, merged: bool) -> Vec<String> {
        let Ok(task) = self.store().task(task_id) else { return vec![] };
        let mut clean = Vec::new();
        for one in self.automations().into_iter().filter(|one| one.on && Self::is_for(one, &task.project_id)) {
            let fits = matches!((&one.when, merged), (When::Merged, true) | (When::RequestOpened, false));
            if !fits {
                continue;
            }
            match &one.then {
                Then::CleanUp => {
                    // Work that is nowhere else must not be deleted behind the user's back.
                    let root = Path::new(&task.cwd);
                    let dirty = gitops::changes(root).map(|files| files.len()).unwrap_or(0);
                    if dirty > 0 {
                        let note = format!("Held back for “{}”: {dirty} files are not committed", task.title);
                        self.note_run(&one.id, &note, |_| {});
                        self.emit(task_id, json!({ "type": "tracker", "text": format!("Not cleaned up by itself: {dirty} files are not committed"), "failed": true }));
                        continue;
                    }
                    self.note_run(&one.id, &format!("Cleaned up “{}”", task.title), |_| {});
                    clean.push(task_id.to_string());
                }
                Then::RunChecks { which } => {
                    let only = (which == "e2e").then_some("e2e");
                    let note = match self.checks_run(task_id, only) {
                        Ok(()) => format!("Started the checks of “{}”", task.title),
                        Err(error) => format!("Could not start the checks of “{}”: {error:#}", task.title),
                    };
                    self.note_run(&one.id, &note, |_| {});
                }
                Then::StartTask { .. } => {}
            }
        }
        clean
    }

    /// Looked at now and then: starts what is due by the clock or by a new issue.
    /// `now_ms` is passed in so that tests can set the time.
    pub async fn automations_tick(self: &Arc<Self>, now: u64) {
        let (minute, day) = self.clock(now);
        // The hosts are asked for new issues now and then, not on every look.
        let polled = self.store().setting(ISSUES_KEY).ok().flatten().and_then(|text| text.parse::<u64>().ok());
        let look_at_issues = polled.is_none_or(|at| now.saturating_sub(at) >= ISSUES_EVERY_MS);
        let mut looked = false;
        for one in self.automations().into_iter().filter(|one| one.on) {
            let Then::StartTask { agent_id, text } = &one.then else { continue };
            let projects: Vec<_> = self
                .store()
                .projects()
                .unwrap_or_default()
                .into_iter()
                .filter(|project| Self::is_for(&one, &project.id))
                .collect();
            match &one.when {
                When::Daily { at } => {
                    let Some(due) = minutes_of(at) else { continue };
                    // Once a day, at or after its time: a core that was off at 03:00 runs it when it is next on that day.
                    if minute < due || one.ran_on_day == Some(day) {
                        continue;
                    }
                    let mut started = Vec::new();
                    for project in &projects {
                        match self.create(&project.id, agent_id, text).await {
                            Ok(task) => started.push(format!("“{}” in {}", task.row.title, project.name)),
                            Err(error) => started.push(format!("could not start in {}: {error:#}", project.name)),
                        }
                    }
                    let note = if started.is_empty() {
                        "No project to run in".to_string()
                    } else {
                        format!("Started {}", started.join(", "))
                    };
                    self.note_run(&one.id, &note, |one| one.ran_on_day = Some(day));
                }
                When::IssueLabeled { label } => {
                    if !look_at_issues {
                        continue;
                    }
                    looked = true;
                    let wanted = label.trim().to_lowercase();
                    let mut seen: HashSet<String> = one.seen.iter().cloned().collect();
                    // "*" marks an automation that has not looked yet: what is open now is not news.
                    let first_look = seen.remove("*");
                    let mut started = Vec::new();
                    for project in &projects {
                        let manager = self.clone();
                        let id = project.id.clone();
                        let Ok(Ok(listed)) = tokio::task::spawn_blocking(move || manager.project_issues(&id)).await
                        else {
                            continue;
                        };
                        for row in &listed.issues {
                            let issue = &row.issue;
                            let mark = format!("{}:{}", project.id, issue.key);
                            let labeled = issue.labels.iter().any(|one| one.to_lowercase() == wanted);
                            if !labeled || !seen.insert(mark) || first_look || row.task_id.is_some() {
                                continue;
                            }
                            match self.start_issue(&project.id, &issue.key, agent_id).await {
                                Ok(task) => {
                                    // What the automation is for goes after what the issue says.
                                    let _ = self.prompt_when_idle(&task.row.id, text.clone());
                                    started.push(format!("{} in {}", issue.key, project.name));
                                }
                                Err(error) => started.push(format!("could not start {}: {error:#}", issue.key)),
                            }
                        }
                    }
                    let seen: Vec<String> = seen.into_iter().collect();
                    if !started.is_empty() {
                        self.note_run(&one.id, &format!("Started a task for {}", started.join(", ")), |one| {
                            one.seen = seen
                        });
                    } else if first_look || seen.len() != one.seen.len() {
                        let mut all = self.automations();
                        if let Some(kept) = all.iter_mut().find(|kept| kept.id == one.id) {
                            kept.seen = seen;
                        }
                        let _ = self.save_automations(&all);
                    }
                }
                _ => {}
            }
        }
        if looked {
            let _ = self.store().set_setting(ISSUES_KEY, Some(&now.to_string()));
        }
    }

    /// Finds the requests that were merged on the host, not from here, and lets
    /// the automations that wait for a merge run, once for each. Returns the
    /// tasks to clean up. Asks the hosts only when such an automation is on.
    pub fn merged_elsewhere(self: &Arc<Self>) -> Vec<String> {
        let waits = self.automations().into_iter().any(|one| one.on && one.when == When::Merged);
        if !waits {
            return vec![];
        }
        let mut clean = Vec::new();
        for task_id in self.merged_since_last_look() {
            let key = format!("task.{task_id}.mergeHandled");
            if self.store().setting(&key).ok().flatten().is_some() {
                continue;
            }
            let _ = self.store().set_setting(&key, Some("1"));
            clean.extend(self.on_request_event(&task_id, true));
        }
        clean
    }

    /// Sends a message to a task as soon as its agent has finished the turn it is in.
    fn prompt_when_idle(self: &Arc<Self>, task_id: &str, text: String) -> Result<()> {
        let (manager, id) = (self.clone(), task_id.to_string());
        tokio::spawn(async move {
            for _ in 0..600 {
                match manager.state_of(&id) {
                    crate::tasks::TaskState::Idle => {
                        let _ = manager.prompt(&id, text).await;
                        return;
                    }
                    crate::tasks::TaskState::Asleep => return,
                    _ => tokio::time::sleep(std::time::Duration::from_millis(500)).await,
                }
            }
        });
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_time_of_day_is_read_as_minutes() {
        assert_eq!(minutes_of("03:00"), Some(180));
        assert_eq!(minutes_of(" 23:59 "), Some(23 * 60 + 59));
        assert_eq!(minutes_of("24:00"), None);
        assert_eq!(minutes_of("3"), None);
        assert_eq!(minutes_of("ab:cd"), None);
    }

    #[test]
    fn what_sets_it_off_and_what_it_does_are_stored_by_name() {
        let when: When = serde_json::from_value(json!({ "kind": "daily", "at": "03:00" })).unwrap();
        assert_eq!(when, When::Daily { at: "03:00".into() });
        let then = Then::StartTask { agent_id: "claude".into(), text: "Update the dependencies".into() };
        assert_eq!(
            serde_json::to_value(&then).unwrap(),
            json!({ "kind": "startTask", "agentId": "claude", "text": "Update the dependencies" })
        );
        assert_eq!(serde_json::to_value(When::IssueLabeled { label: "bug".into() }).unwrap()["kind"], "issueLabeled");
    }
}
