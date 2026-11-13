//! A team: several agents on one task, one after another, each in its role.
//!
//! The planner says how to do the task and changes nothing. The builder does
//! it. The reviewer reads the change and ends with a verdict. Then the
//! task's checks run. Between two agents the editor carries a note: what the
//! one before said. A new agent has none of the old one's memory, so it is
//! also told the conversation so far, as with any hand-over.
//!
//! The user stays in charge: a question from an agent is answered as always,
//! a turn can be stopped, the team can be paused after a step, and when the
//! reviewer wants something put right the user decides whether it goes back
//! to the builder.

use crate::tasks::{Manager, TaskState, now_ms};
use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::sync::Arc;
use std::time::Duration;

/// How many times a change can go back from the reviewer to the builder before the team stops asking.
const MAX_ROUNDS: u32 = 3;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Step {
    /// "plan", "build", "review" or "checks".
    pub role: String,
    /// The agent that has this role. `None` for the checks, which no agent runs.
    pub agent_id: Option<String>,
    /// "waiting", "running", "done", "asks", "skipped", "failed" or "stopped".
    pub state: String,
    pub started_at: Option<u64>,
    pub ended_at: Option<u64>,
    /// What the agent said at the end of its step: what the next one is handed.
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Team {
    /// The task as the user put it.
    pub text: String,
    pub steps: Vec<Step>,
    /// True while a step is running or about to.
    pub running: bool,
    pub pause_after: bool,
    /// How many times the builder has built so far.
    pub rounds: u32,
    /// Why the team stopped, when it stopped before the end.
    pub stopped: Option<String>,
}

fn key(task_id: &str) -> String {
    format!("task.{task_id}.team")
}

/// The verdict a reviewer ends with: `Some(true)` for fine, `Some(false)` for redo.
pub fn verdict_of(answer: &str) -> Option<bool> {
    let line = answer.lines().rev().find(|line| line.to_uppercase().contains("VERDICT:"))?;
    let said = line.to_lowercase();
    if said.contains("redo") {
        Some(false)
    } else if said.contains("fine") {
        Some(true)
    } else {
        None
    }
}

/// An answer without its verdict line, for showing and for handing on.
fn without_verdict(answer: &str) -> String {
    answer
        .lines()
        .filter(|line| !line.to_uppercase().contains("VERDICT:"))
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

impl Manager {
    pub fn team(&self, task_id: &str) -> Option<Team> {
        let text = self.store().setting(&key(task_id)).ok().flatten()?;
        serde_json::from_str(&text).ok()
    }

    fn save_team(&self, task_id: &str, team: &Team) {
        if let Ok(text) = serde_json::to_string(team) {
            let _ = self.store().set_setting(&key(task_id), Some(&text));
        }
        self.broadcast("team.changed", json!({ "taskId": task_id, "team": team }));
    }

    /// What an agent is called, or its id when it is no longer in the list.
    pub(crate) fn agent_name(&self, agent_id: &str) -> String {
        self.registry().get(agent_id).map_or_else(|| agent_id.to_string(), |agent| agent.name.clone())
    }

    /// Puts a team on a task and starts its first step.
    /// `review` may be left out: then nobody reviews and the checks follow the build.
    pub async fn team_start(
        self: &Arc<Self>,
        task_id: &str,
        plan: &str,
        build: &str,
        review: Option<&str>,
        text: Option<&str>,
    ) -> Result<Team> {
        self.store().task(task_id)?;
        if self.team(task_id).is_some_and(|team| team.running) {
            bail!("A team is working on this task already");
        }
        if matches!(self.state_of(task_id), TaskState::Running | TaskState::WaitingPermission) {
            bail!("The agent is working. Wait for its turn to end, then start the team.");
        }
        for id in [Some(plan), Some(build), review].into_iter().flatten() {
            match self.registry().get(id) {
                None => bail!("No such agent: {id}"),
                Some(agent) if !agent.available => bail!("{} is not available on this computer", agent.name),
                Some(_) => {}
            }
        }
        // The task is what the user first asked for, unless said anew.
        let text = match text.map(str::trim).filter(|text| !text.is_empty()) {
            Some(text) => text.to_string(),
            None => self
                .store()
                .events(task_id)?
                .iter()
                .find(|event| event["type"] == "user_message")
                .and_then(|event| event["text"].as_str())
                .ok_or_else(|| anyhow!("Say what the team should do"))?
                .to_string(),
        };
        let step = |role: &str, agent: Option<&str>| Step {
            role: role.into(),
            agent_id: agent.map(str::to_string),
            state: "waiting".into(),
            started_at: None,
            ended_at: None,
            note: None,
        };
        let mut steps = vec![step("plan", Some(plan)), step("build", Some(build))];
        steps.extend(review.map(|agent| step("review", Some(agent))));
        steps.push(step("checks", None));
        let team = Team { text, steps, running: true, pause_after: false, rounds: 0, stopped: None };
        self.save_team(task_id, &team);
        self.drive(task_id);
        Ok(team)
    }

    /// Asks the team to stop after the step it is in, or takes that back.
    pub fn team_pause(&self, task_id: &str, on: bool) -> Result<Team> {
        let mut team = self.team(task_id).ok_or_else(|| anyhow!("No team works on this task"))?;
        team.pause_after = on;
        self.save_team(task_id, &team);
        Ok(team)
    }

    /// Goes on with a team that was paused or stopped, from the first step that is not done.
    pub fn team_continue(self: &Arc<Self>, task_id: &str) -> Result<Team> {
        let mut team = self.team(task_id).ok_or_else(|| anyhow!("No team works on this task"))?;
        if team.running {
            bail!("The team is working already");
        }
        if team.steps.iter().any(|step| step.state == "asks") {
            bail!("The reviewer is waiting for your answer first");
        }
        if team.steps.iter().all(|step| matches!(step.state.as_str(), "done" | "skipped")) {
            bail!("The team has finished");
        }
        for step in &mut team.steps {
            if matches!(step.state.as_str(), "stopped" | "failed" | "running") {
                step.state = "waiting".into();
            }
        }
        team.running = true;
        team.pause_after = false;
        team.stopped = None;
        self.save_team(task_id, &team);
        self.drive(task_id);
        Ok(team)
    }

    /// The user's answer to a reviewer who wants something put right:
    /// send it back to the builder, or leave it and go on.
    pub fn team_answer(self: &Arc<Self>, task_id: &str, back: bool) -> Result<Team> {
        let mut team = self.team(task_id).ok_or_else(|| anyhow!("No team works on this task"))?;
        let Some(review) = team.steps.iter().position(|step| step.state == "asks") else {
            bail!("Nobody in the team is waiting for an answer");
        };
        if back {
            // The builder builds again with what the reviewer found, and the reviewer looks again.
            team.steps[review].state = "waiting".into();
            if let Some(build) = team.steps.iter_mut().find(|step| step.role == "build") {
                build.state = "waiting".into();
            }
        } else {
            team.steps[review].state = "done".into();
        }
        team.running = true;
        team.stopped = None;
        self.save_team(task_id, &team);
        self.drive(task_id);
        Ok(team)
    }

    /// Stops the team where it is. The turn that is running is stopped too.
    pub fn team_stop(&self, task_id: &str) -> Result<Team> {
        let mut team = self.team(task_id).ok_or_else(|| anyhow!("No team works on this task"))?;
        team.running = false;
        team.stopped = Some("You stopped the team.".into());
        for step in &mut team.steps {
            if step.state == "running" {
                step.state = "stopped".into();
            }
        }
        self.save_team(task_id, &team);
        let _ = self.cancel(task_id);
        Ok(team)
    }

    /// A team that was running when the core stopped is not running any more.
    pub(crate) fn settle_teams(&self) {
        for task in self.store().tasks().unwrap_or_default() {
            if let Some(mut team) = self.team(&task.id).filter(|team| team.running) {
                team.running = false;
                team.stopped = Some("The editor was closed while the team worked.".into());
                for step in &mut team.steps {
                    if step.state == "running" {
                        step.state = "stopped".into();
                    }
                }
                self.save_team(&task.id, &team);
            }
        }
    }

    /// Runs the steps that are waiting, one after another, until the team is
    /// done, pauses, asks the user or fails.
    fn drive(self: &Arc<Self>, task_id: &str) {
        let (manager, task_id) = (self.clone(), task_id.to_string());
        tokio::spawn(async move {
            loop {
                let Some(mut team) = manager.team(&task_id).filter(|team| team.running) else { return };
                let Some(index) = team.steps.iter().position(|step| step.state == "waiting") else {
                    team.running = false;
                    manager.save_team(&task_id, &team);
                    return;
                };
                team.steps[index].state = "running".into();
                team.steps[index].started_at = Some(now_ms());
                team.steps[index].ended_at = None;
                manager.save_team(&task_id, &team);

                let outcome = manager.run_step(&task_id, &team, index).await;
                // The user may have stopped or paused the team meanwhile: what is stored now is what counts.
                let Some(mut team) = manager.team(&task_id) else { return };
                let step = &mut team.steps[index];
                step.ended_at = Some(now_ms());
                let mut go_on = team.running;
                match outcome {
                    Ok((state, note)) => {
                        if step.state == "running" {
                            step.state = state.to_string();
                        }
                        step.note = note;
                        if matches!(state, "asks" | "failed") {
                            go_on = false;
                        }
                        if state == "failed" {
                            team.stopped = Some("The checks failed. The team stops here.".into());
                        }
                    }
                    Err(error) => {
                        if step.state == "running" {
                            step.state = "failed".into();
                        }
                        team.stopped.get_or_insert(format!("{error:#}"));
                        go_on = false;
                    }
                }
                if go_on && team.pause_after && team.steps.iter().any(|step| step.state == "waiting") {
                    team.pause_after = false;
                    team.stopped = Some("Paused after this step, as you asked.".into());
                    go_on = false;
                }
                team.running = go_on;
                manager.save_team(&task_id, &team);
                if !go_on {
                    return;
                }
            }
        });
    }

    /// Does one step and says how it ended: the new state and the note to hand on.
    async fn run_step(
        self: &Arc<Self>,
        task_id: &str,
        team: &Team,
        index: usize,
    ) -> Result<(&'static str, Option<String>)> {
        let step = &team.steps[index];
        let note_of = |role: &str| team.steps.iter().find(|one| one.role == role).and_then(|one| one.note.clone());
        let name_of = |role: &str| {
            let id = team.steps.iter().find(|one| one.role == role).and_then(|one| one.agent_id.clone());
            id.map(|id| self.agent_name(&id)).unwrap_or_else(|| "nobody".into())
        };

        if step.role == "checks" {
            if self.checks_run(task_id, None).is_err() {
                return Ok(("skipped", Some("This project has no checks that are on.".into())));
            }
            loop {
                tokio::time::sleep(Duration::from_millis(150)).await;
                if !self.team(task_id).is_some_and(|team| team.running) {
                    bail!("The team was stopped while the checks ran");
                }
                let state = self.checks_state(task_id)?;
                if state["running"] == 0 {
                    let (done, total, failed) =
                        (&state["done"], &state["total"], state["failed"].as_u64().unwrap_or(0));
                    let note = format!("{done} of {total} lines of “Done when” are done.");
                    return Ok((if failed > 0 { "failed" } else { "done" }, Some(note)));
                }
            }
        }

        let agent_id = step.agent_id.clone().ok_or_else(|| anyhow!("This step has no agent"))?;
        let me = self.agent_name(&agent_id);
        let (prompt, from, to) = match step.role.as_str() {
            "plan" => (
                format!(
                    "You are the planner in a team of agents working on one task. Another agent builds after you.\n\nThe task:\n{}\n\nPlan how to do it: the steps in order, the files that matter, and what the builder must keep to.\n\nAnswer with the plan only, as short steps, and leave every file as it is.",
                    team.text
                ),
                "You".to_string(),
                me.clone(),
            ),
            "build" => {
                let review = note_of("review").filter(|_| team.rounds > 0);
                let body = match &review {
                    Some(found) => format!(
                        "The reviewer, {}, asks for this to be put right:\n{found}\n\nNow make the change the reviewer asks for.",
                        name_of("review")
                    ),
                    None => format!(
                        "The plan, from {}:\n{}\n\nWhen you are done, say in a few lines what you did, and what you left out and why.\n\nNow make the change the plan describes.",
                        name_of("plan"),
                        note_of("plan").unwrap_or_else(|| "(no plan was written)".into())
                    ),
                };
                let from = if review.is_some() { name_of("review") } else { name_of("plan") };
                (
                    format!(
                        "You are the builder in a team of agents working on one task.\n\nThe task:\n{}\n\n{body}",
                        team.text
                    ),
                    from,
                    me.clone(),
                )
            }
            "review" => {
                // A second look is told what the first one asked for, so it checks that and no more.
                let before = match step.note.as_deref().filter(|_| team.rounds > 1) {
                    Some(asked) => {
                        format!("You looked at this change before and asked for this to be put right:\n{asked}\n\n")
                    }
                    None => String::new(),
                };
                (
                    format!(
                        "You are the reviewer in a team of agents working on one task. {builder} built it; its changes are in the working copy and are not committed.\n\nThe task:\n{}\n\n{before}What {builder} said it did:\n{}\n\nRead the changed files and list what must be put right, if anything. Leave every file as it is.\n\nEnd your answer with a line that says exactly VERDICT: fine or VERDICT: redo.",
                        team.text,
                        note_of("build").unwrap_or_else(|| "(it said nothing)".into()),
                        builder = name_of("build"),
                    ),
                    name_of("build"),
                    me.clone(),
                )
            }
            other => bail!("Unknown step: {other}"),
        };

        if step.role == "build" {
            let mut counted = self.team(task_id).ok_or_else(|| anyhow!("The team is gone"))?;
            counted.rounds += 1;
            self.save_team(task_id, &counted);
        }
        // The step's agent takes the task. It is told the conversation so far, as with any hand-over.
        self.switch_agent(task_id, &agent_id).await?;
        let mut events = self.subscribe();
        // What goes from one agent to the next is shown as a note, not as something the user said.
        let handed = match step.role.as_str() {
            "plan" => team.text.clone(),
            "build" if team.rounds > 0 => note_of("review").unwrap_or_default(),
            "build" => note_of("plan").unwrap_or_default(),
            _ => note_of("build").unwrap_or_default(),
        };
        let shown = json!({ "type": "team_note", "role": step.role, "from": from, "to": to, "text": handed });
        // The step before this one may have been stopped a moment ago, and an agent takes a moment to
        // come to a stop. Waiting here is what the user means by "go on": they should not be told to wait.
        for _ in 0..200 {
            if self.state_of(task_id) != crate::tasks::TaskState::Running {
                break;
            }
            if !self.team(task_id).is_some_and(|team| team.running) {
                bail!("The team was stopped");
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
        self.prompt_as(task_id, prompt, Some(shown)).await?;

        // The step lasts until the agent's turn is over. A question to the user is part of the turn.
        let mut said = String::new();
        let mut ended_well = false;
        loop {
            let message = match tokio::time::timeout(Duration::from_secs(3600), events.recv()).await {
                Ok(Ok(message)) => message,
                Ok(Err(tokio::sync::broadcast::error::RecvError::Lagged(_))) => continue,
                _ => bail!("{me} did not finish its step"),
            };
            if message["event"] != "task.event" || message["data"]["taskId"] != task_id {
                continue;
            }
            let event = &message["data"];
            match event["type"].as_str().unwrap_or("") {
                "update" if event["update"]["sessionUpdate"] == "agent_message_chunk" => {
                    said.push_str(event["update"]["content"]["text"].as_str().unwrap_or(""));
                }
                "turn_end" => ended_well = event["stopReason"] == "end_turn",
                "error" => bail!("{me} failed: {}", event["message"].as_str().unwrap_or("no reason given")),
                "state" if event["state"] == "idle" => break,
                "state" if event["state"] == "asleep" => bail!("{me} stopped before its step was over"),
                _ => {}
            }
        }
        if !ended_well {
            bail!("The turn of {me} was stopped before its end");
        }
        let said = said.trim().to_string();
        if step.role != "review" {
            return Ok(("done", Some(said)));
        }
        let note = without_verdict(&said);
        match verdict_of(&said) {
            Some(true) => Ok(("done", Some(note))),
            // Past a few rounds the team stops sending it back by itself and asks all the same.
            Some(false) if team.rounds >= MAX_ROUNDS => {
                Ok(("asks", Some(format!("{note}\n\n(This change has gone back {MAX_ROUNDS} times already.)"))))
            }
            Some(false) => Ok(("asks", Some(note))),
            // A reviewer that gave no verdict is read as having found nothing it insists on.
            None => Ok(("done", Some(note))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_verdict_is_read_from_the_last_line_that_holds_one() {
        assert_eq!(verdict_of("Looks right.\n\nVERDICT: fine"), Some(true));
        assert_eq!(verdict_of("Line 3 is wrong.\nverdict: REDO\n"), Some(false));
        assert_eq!(verdict_of("VERDICT: fine\nOn second thought:\nVERDICT: redo"), Some(false));
        assert_eq!(verdict_of("No verdict here."), None);
        assert_eq!(verdict_of("VERDICT: maybe"), None);
        assert_eq!(without_verdict("Line 3 is wrong.\n\nVERDICT: redo\n"), "Line 3 is wrong.");
    }
}
