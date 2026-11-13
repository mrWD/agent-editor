//! A race: one task text goes to several agents at once, each in its own copy
//! of the code. The user compares what they did and picks one result; the
//! others are removed.
//!
//! Every entrant is an ordinary task. The race itself is a small record that
//! names them. It is kept with its project's settings, under
//! `project.<project id>.race.<n>`, so it goes when the project is removed;
//! the id of a race is `<project id>:<n>` for that reason. Each entrant points
//! back at its race under `task.<task id>.race`.
//!
//! Nothing here asks a model for anything: what a race says about an entrant
//! is read from that task's events, its branch and its checks.

use crate::checkpoints::happened_since;
use crate::server::AppState;
use crate::tasks::{Manager, TaskInfo, TaskState, now_ms};
use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeSet;
use std::path::Path;
use std::sync::{Arc, Mutex};

/// One agent alone is no race, and more than four results are too many to compare.
const MIN_AGENTS: usize = 2;
const MAX_AGENTS: usize = 4;

/// How much of a change travels to the reviewer, in characters. A longer one is cut and says so.
const REVIEW_LIMIT: usize = 24_000;

/// The races that are being decided right now. Picking and discarding remove
/// working copies, so a race is decided by one press at a time: a second one
/// that comes meanwhile (from another window, say) is refused, not queued.
static DECIDING: Mutex<BTreeSet<String>> = Mutex::new(BTreeSet::new());

/// Holds a race while it is being decided, and lets go of it when dropped.
#[derive(Debug)]
struct Deciding(String);

impl Deciding {
    fn begin(race_id: &str) -> Result<Self> {
        if !DECIDING.lock().unwrap().insert(race_id.to_string()) {
            bail!("This race is being decided right now");
        }
        Ok(Self(race_id.to_string()))
    }
}

impl Drop for Deciding {
    fn drop(&mut self) {
        DECIDING.lock().unwrap().remove(&self.0);
    }
}

/// One agent's task in a race.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Entrant {
    task_id: String,
    /// The agent the task was started with.
    agent_id: String,
    /// Where its copy of the code is and on which branch. Kept here too, so the
    /// copy can still be removed after the task itself was archived by hand.
    cwd: String,
    branch: Option<String>,
}

/// A race as it is stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct Record {
    id: String,
    project_id: String,
    text: String,
    /// The agents the race was asked for. "Run the race again" starts these.
    agent_ids: Vec<String>,
    entrants: Vec<Entrant>,
    started_at: u64,
    /// The task whose result the user took.
    picked: Option<String>,
    /// True when the user threw every result away.
    discarded: bool,
    decided_at: Option<u64>,
}

impl Record {
    fn decided(&self) -> bool {
        self.picked.is_some() || self.discarded
    }

    fn task_ids(&self) -> Vec<String> {
        self.entrants.iter().map(|entrant| entrant.task_id.clone()).collect()
    }

    fn has(&self, task_id: &str) -> bool {
        self.entrants.iter().any(|entrant| entrant.task_id == task_id)
    }
}

/// What a task says about the race it is in: enough for the interface to find the other entrants.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RaceRef {
    pub id: String,
    /// Every task the race started with, in the order of its agents.
    pub tasks: Vec<String>,
}

/// Where a race is kept. `None` for an id that was never a race's.
fn record_key(race_id: &str) -> Option<String> {
    let (project_id, n) = race_id.split_once(':')?;
    (!project_id.is_empty() && !n.is_empty()).then(|| format!("project.{project_id}.race.{n}"))
}

fn pointer_key(task_id: &str) -> String {
    format!("task.{task_id}.race")
}

/// The events of an entrant that are about its own result. A turn in which it
/// reviewed another entrant's change is left out: what it said there is about
/// someone else's work, and the time it took is not time spent on the task.
fn own_events(events: Vec<Value>) -> Vec<Value> {
    // The mark of a review comes right after the message that asks for it.
    let mut reviews = Vec::new();
    let mut last_message = None;
    for (index, event) in events.iter().enumerate() {
        match event["type"].as_str() {
            Some("user_message") => last_message = Some(index),
            Some("race_review") => reviews.extend(last_message),
            _ => {}
        }
    }
    let mut in_review = false;
    events
        .into_iter()
        .enumerate()
        .filter(|(index, event)| {
            if event["type"] == "user_message" {
                in_review = reviews.contains(index);
            }
            !in_review && event["type"] != "race_review"
        })
        .map(|(_, event)| event)
        .collect()
}

/// When an entrant got its first message and when its last turn ended.
fn timing(events: &[Value]) -> (Option<u64>, Option<u64>) {
    let at = |event: &Value| event["at"].as_u64();
    let started = events.iter().find(|event| event["type"] == "user_message").and_then(at);
    // A turn that failed has ended too.
    let ended = events.iter().rev().find(|event| event["type"] == "turn_end" || event["type"] == "error").and_then(at);
    (started, ended.filter(|ended| started.is_some_and(|started| *ended >= started)))
}

impl Manager {
    fn race_record(&self, race_id: &str) -> Result<Record> {
        record_key(race_id)
            .and_then(|key| self.store().setting(&key).ok().flatten())
            .and_then(|text| serde_json::from_str(&text).ok())
            .ok_or_else(|| anyhow!("No such race"))
    }

    fn save_race(&self, record: &Record) -> Result<()> {
        let key = record_key(&record.id).ok_or_else(|| anyhow!("No such race"))?;
        self.store().set_setting(&key, Some(&serde_json::to_string(record)?))
    }

    /// The race a task is an entrant of, while that race waits for the user's choice.
    /// Once a result is picked the winner is an ordinary task again.
    pub(crate) fn race_ref(&self, task_id: &str) -> Option<RaceRef> {
        let race_id = self.store().setting(&pointer_key(task_id)).ok().flatten()?;
        let record = self.race_record(&race_id).ok()?;
        (!record.decided()).then(|| RaceRef { tasks: record.task_ids(), id: record.id })
    }

    /// The id of the race a task was started in, decided or not.
    pub fn race_of_task(&self, task_id: &str) -> Result<String> {
        self.store().setting(&pointer_key(task_id))?.ok_or_else(|| anyhow!("This task is not part of a race"))
    }

    /// Tells every interface that a race started or was decided, so its tasks can be shown as what they are now.
    fn race_announce(&self, record: &Record) {
        self.broadcast(
            "race.changed",
            json!({ "raceId": record.id, "tasks": record.task_ids(), "decided": record.decided(), "picked": record.picked }),
        );
    }

    /// Sends one task text to several agents at once. Each gets an ordinary
    /// task with its own working copy; the race is the record that names them.
    ///
    /// An agent that fails to start does not stop the others: they are kept,
    /// and the answer says who failed and why. With fewer than two started
    /// there is nothing to compare, so no race is recorded.
    pub async fn race_start(self: &Arc<Self>, project_id: &str, agent_ids: &[String], text: &str) -> Result<Value> {
        if text.trim().is_empty() {
            bail!("Describe the task first");
        }
        if !(MIN_AGENTS..=MAX_AGENTS).contains(&agent_ids.len()) {
            bail!("A race needs two to four agents, and {} were picked", agent_ids.len());
        }
        for (index, agent_id) in agent_ids.iter().enumerate() {
            let Some(agent) = self.registry().get(agent_id).cloned() else { bail!("Unknown agent: {agent_id}") };
            if agent_ids[..index].contains(agent_id) {
                bail!("{} is in the race twice. Pick each agent once.", agent.name);
            }
            if !agent.available {
                bail!(
                    "{} is not available: {}",
                    agent.name,
                    agent.note.as_deref().unwrap_or("its command was not found")
                );
            }
        }
        let project = self.store().project(project_id)?;
        if !Path::new(&project.path).is_dir() {
            bail!("The project's folder is gone: {}", project.path);
        }
        // Without git every agent would write into the same folder, over each other's work.
        if !crate::git::is_repo_with_commits(Path::new(&project.path)).await {
            bail!(
                "A race needs git: each agent works in its own copy of the code, and {} is a folder without git or without a first commit",
                project.name
            );
        }

        // The entrants are started one after another, not side by side. Git cannot add two
        // working copies to one repository at the same moment: the second reads the notes
        // of the first while they are half written and gives up. Each entrant's time is
        // counted from its own first message, so the later ones lose nothing by it.
        let mut started: Vec<TaskInfo> = Vec::new();
        let mut failed = Vec::new();
        for agent_id in agent_ids {
            match self.create(project_id, agent_id, text).await {
                Ok(task) => started.push(task),
                Err(error) => failed.push(json!({
                    "agentId": agent_id,
                    "agentName": self.agent_name(agent_id),
                    "message": format!("{error:#}"),
                })),
            }
        }
        if started.is_empty() {
            let reasons: Vec<&str> = failed.iter().filter_map(|one| one["message"].as_str()).collect();
            bail!("No agent started. {}", reasons.join(" "));
        }
        if started.len() < MIN_AGENTS {
            // One agent alone is no race: its task stays as the ordinary task it is.
            return Ok(json!({ "race": null, "tasks": started, "failed": failed }));
        }

        let record = Record {
            id: format!("{}:{}", project.id, uuid::Uuid::new_v4().simple()),
            project_id: project.id.clone(),
            text: text.to_string(),
            agent_ids: agent_ids.to_vec(),
            entrants: started
                .iter()
                .map(|task| Entrant {
                    task_id: task.row.id.clone(),
                    agent_id: task.row.agent_id.clone(),
                    cwd: task.row.cwd.clone(),
                    branch: task.row.branch.clone(),
                })
                .collect(),
            started_at: now_ms(),
            picked: None,
            discarded: false,
            decided_at: None,
        };
        self.save_race(&record)?;
        for entrant in &record.entrants {
            self.store().set_setting(&pointer_key(&entrant.task_id), Some(&record.id))?;
        }
        self.race_announce(&record);
        // The tasks as they are now: each knows its race.
        let tasks: Vec<TaskInfo> = started.iter().filter_map(|task| self.task(&task.row.id).ok()).collect();
        Ok(json!({ "race": RaceRef { tasks: record.task_ids(), id: record.id }, "tasks": tasks, "failed": failed }))
    }

    /// Starts a new race with the text and the agents of an earlier one. The earlier one stays as it is.
    pub async fn race_again(self: &Arc<Self>, race_id: &str) -> Result<Value> {
        let record = self.race_record(race_id)?;
        self.race_start(&record.project_id, &record.agent_ids, &record.text).await
    }

    /// How one entrant stands: what it says about itself, what it changed and
    /// how its checks ended. `None` for a task that is gone (archived or cleaned
    /// up by hand): it is simply no longer in the race.
    fn entrant_state(&self, entrant: &Entrant) -> Option<Value> {
        let task = self.store().task(&entrant.task_id).ok()?;
        let agent = self.registry().get(&task.agent_id).cloned();
        let name = agent.as_ref().map_or_else(|| task.agent_id.clone(), |agent| agent.name.clone());
        let events = own_events(self.store().events(&task.id).unwrap_or_default());
        let (started, ended) = timing(&events);
        let (_, said) = happened_since(&[], &events, &name, &task.cwd);
        // The same comparison the Changes view makes for "Branch": commits and what is not committed, together.
        let (files, problem) = match self.files_between(&task.id, "branch") {
            Ok(listed) => (listed["files"].as_array().cloned().unwrap_or_default(), None),
            Err(error) => (Vec::new(), Some(format!("{error:#}"))),
        };
        let sum = |name: &str| files.iter().filter_map(|file| file[name].as_u64()).sum::<u64>();
        let checks = self.checks_state(&task.id).ok().map(|state| {
            json!({ "done": state["done"], "total": state["total"], "failed": state["failed"], "running": state["running"] })
        });
        Some(json!({
            "taskId": task.id,
            "agentId": task.agent_id,
            "agentName": name,
            "short": agent.map(|agent| agent.short.clone()),
            "state": self.state_of(&task.id),
            "startedAt": started,
            "endedAt": ended,
            // From its first message to the end of its last turn.
            "ms": started.zip(ended).map(|(started, ended)| ended - started),
            "said": said,
            "added": sum("added"),
            "removed": sum("removed"),
            "files": files,
            "checks": checks,
            "problem": problem,
        }))
    }

    /// A race as the interface shows it: every entrant that is still there,
    /// whether the race is over (nobody is working or waiting for an answer),
    /// when it finished and what was picked. Runs git; call it from `spawn_blocking`.
    pub fn race_get(&self, race_id: &str) -> Result<Value> {
        let record = self.race_record(race_id)?;
        // Each entrant is its own working copy, so they are read side by side.
        let entrants: Vec<Value> = std::thread::scope(|scope| {
            let reading: Vec<_> =
                record.entrants.iter().map(|entrant| scope.spawn(move || self.entrant_state(entrant))).collect();
            reading.into_iter().filter_map(|one| one.join().ok().flatten()).collect()
        });
        let over = !entrants.iter().any(|entrant| {
            entrant["state"] == json!(TaskState::Running) || entrant["state"] == json!(TaskState::WaitingPermission)
        });
        let finished_at =
            if over { entrants.iter().filter_map(|entrant| entrant["endedAt"].as_u64()).max() } else { None };
        Ok(json!({
            "id": record.id,
            "projectId": record.project_id,
            "text": record.text,
            "agentIds": record.agent_ids,
            "tasks": record.task_ids(),
            "startedAt": record.started_at,
            "entrants": entrants,
            "over": over,
            "finishedAt": finished_at,
            "picked": record.picked,
            "discarded": record.discarded,
            "decidedAt": record.decided_at,
        }))
    }

    /// The change an entrant's branch holds, as text another agent can read. Runs git.
    fn race_change_text(&self, task_id: &str) -> Result<String> {
        let listed = self.files_between(task_id, "branch")?;
        let files = listed["files"].as_array().cloned().unwrap_or_default();
        if files.is_empty() {
            bail!("There is nothing to review: this result changes no file");
        }
        let mut text = String::new();
        let mut left_out = 0;
        for file in &files {
            if text.len() >= REVIEW_LIMIT {
                left_out += 1;
                continue;
            }
            let path = file["path"].as_str().unwrap_or_default();
            let (kind, added, removed) =
                (file["kind"].as_str().unwrap_or("modified"), file["added"].as_u64(), file["removed"].as_u64());
            text.push_str(&format!("--- {path} ({kind}, +{} -{})\n", added.unwrap_or(0), removed.unwrap_or(0)));
            if file["binary"] == true {
                text.push_str("(not text, or too large to show line by line)\n");
                continue;
            }
            let diff = self.diff_between(task_id, path, "branch")?;
            'file: for hunk in diff["hunks"].as_array().into_iter().flatten() {
                text.push_str(&format!("@@ line {} @@\n", hunk["fromLine"]));
                for line in hunk["lines"].as_array().into_iter().flatten() {
                    if text.len() >= REVIEW_LIMIT {
                        text.push_str("(the rest of this file is left out: the change is too long for one message)\n");
                        break 'file;
                    }
                    let sign = match line["kind"].as_str() {
                        Some("add") => '+',
                        Some("remove") => '-',
                        _ => ' ',
                    };
                    text.push(sign);
                    text.push_str(line["text"].as_str().unwrap_or_default());
                    text.push('\n');
                }
            }
        }
        if left_out > 0 {
            let files =
                if left_out == 1 { "one more file is".to_string() } else { format!("{left_out} more files are") };
            text.push_str(&format!("({files} left out: the change is too long for one message)\n"));
        }
        Ok(text)
    }

    /// Asks one entrant's agent to review another entrant's result. The
    /// reviewer gets the change as text, in its own task; nothing else changes.
    pub async fn race_review(self: &Arc<Self>, race_id: &str, task_id: &str, reviewer_id: &str) -> Result<()> {
        let record = self.race_record(race_id)?;
        if record.decided() {
            bail!("This race is already decided");
        }
        if !record.has(task_id) || !record.has(reviewer_id) {
            bail!("Both tasks must be part of this race");
        }
        if task_id == reviewer_id {
            bail!("An agent cannot review its own result. Ask another agent of the race.");
        }
        let task = self.store().task(task_id).map_err(|_| anyhow!("That result is gone: its task was archived"))?;
        let reviewer =
            self.store().task(reviewer_id).map_err(|_| anyhow!("The reviewer's task is gone: it was archived"))?;
        if matches!(self.state_of(reviewer_id), TaskState::Running | TaskState::WaitingPermission) {
            bail!(
                "{} is still working. Wait for it or stop its turn, then ask again.",
                self.agent_name(&reviewer.agent_id)
            );
        }
        let author = self.agent_name(&task.agent_id);
        let (manager, id) = (self.clone(), task_id.to_string());
        let change = tokio::task::spawn_blocking(move || manager.race_change_text(&id)).await??;
        let message = format!(
            "Another agent, {author}, worked on the same task as you, in its own copy of the code. \
             Review its change. Do not change any file.\n\n\
             The task was:\n\n{}\n\n\
             {author}'s change:\n\n```diff\n{change}```\n\n\
             Say whether it does what the task asks, what is wrong or risky in it, and whether you would use it.",
            record.text.trim()
        );
        self.prompt(reviewer_id, message).await?;
        // The mark that says the message above asked for a review. See `own_events`.
        self.emit(reviewer_id, json!({ "type": "race_review", "of": task_id, "ofName": author }));
        Ok(())
    }

    /// Removes what an entrant left on disk after its task was archived by
    /// hand: archiving keeps the working copy and the branch, a decided race does not.
    async fn race_remove_copy(&self, project_id: &str, entrant: &Entrant) {
        let Ok(project) = self.store().project(project_id) else { return };
        let project_root = Path::new(&project.path);
        // Only a working copy the core made itself is removed, never the project's own folder.
        let inside = Path::new(&entrant.cwd).starts_with(self.data_root().join("worktrees"));
        if let Some(branch) = entrant.branch.as_deref().filter(|_| inside) {
            crate::git::remove_worktree(project_root, Path::new(&entrant.cwd), branch).await;
        }
        self.drop_checkpoints(project_root, &entrant.task_id);
    }
}

impl AppState {
    /// Takes one entrant out for good: stops what it runs, removes its working
    /// copy and branch and archives it. The same steps as "Clean up".
    async fn race_remove(&self, project_id: &str, entrant: &Entrant) {
        if self.manager.task(&entrant.task_id).is_err() {
            return self.manager.race_remove_copy(project_id, entrant).await;
        }
        self.stop_everything(&entrant.task_id).await;
        if let Err(error) = self.manager.clean_up(&entrant.task_id, true, true).await {
            tracing::debug!("Could not clean up {} of a race: {error:#}", entrant.task_id);
        }
    }

    /// Ends a race. With `keep`, that entrant stays as an ordinary task and the
    /// others are removed; without it, all of them are.
    async fn race_decide(&self, race_id: &str, keep: Option<&str>) -> Result<Value> {
        let _one_at_a_time = Deciding::begin(race_id)?;
        let manager = &self.manager;
        let mut record = manager.race_record(race_id)?;
        if record.decided() {
            bail!("This race is already decided");
        }
        if let Some(task_id) = keep {
            if !record.has(task_id) {
                bail!("That task is not part of this race");
            }
            if manager.task(task_id).is_err() {
                bail!("That result is gone: its task was archived. Pick another one.");
            }
        }
        // The copies go first and the race is marked last: if this is cut short,
        // the race is still open and the same press finishes the work.
        for entrant in record.entrants.iter().filter(|entrant| Some(entrant.task_id.as_str()) != keep) {
            self.race_remove(&record.project_id, entrant).await;
        }
        record.picked = keep.map(str::to_string);
        record.discarded = keep.is_none();
        record.decided_at = Some(now_ms());
        manager.save_race(&record)?;
        manager.race_announce(&record);
        let (manager, id) = (manager.clone(), record.id);
        tokio::task::spawn_blocking(move || manager.race_get(&id)).await?
    }

    /// Takes one entrant's result. Every other entrant is stopped and cleaned up.
    pub async fn race_pick(&self, race_id: &str, task_id: &str) -> Result<Value> {
        self.race_decide(race_id, Some(task_id)).await
    }

    /// Throws every result of a race away.
    pub async fn race_discard(&self, race_id: &str) -> Result<Value> {
        self.race_decide(race_id, None).await
    }
}

/// Every command of a race, as the interface sends it.
pub async fn command(state: &AppState, method: &str, params: &Value) -> Result<Value> {
    let manager = &state.manager;
    let text =
        |name: &str| params.get(name).and_then(Value::as_str).ok_or_else(|| anyhow!("Missing parameter: {name}"));
    match method {
        "race.start" => {
            let agents: Vec<String> = params
                .get("agentIds")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow!("Missing parameter: agentIds"))?
                .iter()
                .filter_map(|id| id.as_str().map(str::to_string))
                .collect();
            manager.race_start(text("projectId")?, &agents, text("text")?).await
        }
        "race.get" => {
            // By the race's own id, or by the id of one of its tasks.
            let race_id = match params.get("raceId").and_then(Value::as_str) {
                Some(id) => id.to_string(),
                None => manager.race_of_task(text("taskId")?)?,
            };
            let manager = manager.clone();
            tokio::task::spawn_blocking(move || manager.race_get(&race_id)).await?
        }
        "race.pick" => state.race_pick(text("raceId")?, text("taskId")?).await,
        "race.discard" => state.race_discard(text("raceId")?).await,
        "race.again" => manager.race_again(text("raceId")?).await,
        "race.review" => {
            manager.race_review(text("raceId")?, text("taskId")?, text("reviewerTaskId")?).await?;
            Ok(json!({}))
        }
        other => bail!("Unknown method: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(text: &str, at: u64) -> Value {
        json!({ "type": "user_message", "text": text, "at": at })
    }
    fn chunk(text: &str, at: u64) -> Value {
        json!({ "type": "update", "at": at, "update": { "sessionUpdate": "agent_message_chunk", "content": { "text": text } } })
    }
    fn turn_end(at: u64) -> Value {
        json!({ "type": "turn_end", "at": at, "stopReason": "end_turn" })
    }

    #[test]
    fn a_race_is_kept_with_its_project() {
        assert_eq!(record_key("p1:abc").as_deref(), Some("project.p1.race.abc"));
        assert_eq!(record_key("abc"), None, "an id that was never a race's finds nothing");
        assert_eq!(record_key(":abc"), None);
        assert_eq!(record_key("p1:"), None);
    }

    #[test]
    fn a_race_is_decided_by_one_press_at_a_time() {
        let first = Deciding::begin("p9:one").unwrap();
        assert!(Deciding::begin("p9:one").unwrap_err().to_string().contains("being decided right now"));
        // Another race is not held up by it.
        drop(Deciding::begin("p9:two").unwrap());
        drop(first);
        drop(Deciding::begin("p9:one").unwrap());
    }

    #[test]
    fn time_runs_from_the_first_message_to_the_end_of_the_last_turn() {
        let events = vec![
            said("Fix the test", 100),
            chunk("Done.", 150),
            turn_end(160),
            said("Shorter, please", 400),
            chunk("Shorter now.", 450),
            turn_end(460),
        ];
        assert_eq!(timing(&events), (Some(100), Some(460)));
        // A turn that is still going has not ended: the last end stands.
        assert_eq!(timing(&events[..5]), (Some(100), Some(160)));
        assert_eq!(timing(&events[..2]), (Some(100), None));
        assert_eq!(timing(&[]), (None, None));
        // A turn that failed has ended too.
        let failed = vec![said("Fix the test", 100), json!({ "type": "error", "at": 130, "message": "no" })];
        assert_eq!(timing(&failed), (Some(100), Some(130)));
    }

    #[test]
    fn a_turn_spent_reviewing_another_result_is_not_the_entrants_own() {
        let events = vec![
            said("Fix the test", 100),
            chunk("I wait for the frame.", 150),
            turn_end(160),
            said("Review its change", 300),
            json!({ "type": "race_review", "at": 301, "of": "t2", "ofName": "Codex" }),
            chunk("It looks right.", 350),
            turn_end(360),
        ];
        let own = own_events(events.clone());
        assert_eq!(own.len(), 3);
        assert_eq!(timing(&own), (Some(100), Some(160)));
        assert_eq!(happened_since(&[], &own, "Claude", "/work").1.as_deref(), Some("I wait for the frame."));
        // What the user says after the review is about the entrant's own result again.
        let mut later = events;
        later.extend([said("Use a shorter wait", 500), chunk("Shorter now.", 550), turn_end(560)]);
        let own = own_events(later);
        assert_eq!(timing(&own), (Some(100), Some(560)));
        assert_eq!(happened_since(&[], &own, "Claude", "/work").1.as_deref(), Some("Shorter now."));
    }
}
