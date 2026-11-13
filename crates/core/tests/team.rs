//! A team: planner, builder, reviewer and the checks, on one task.
//!
//! Three practice agents take the roles. Nothing is spent.

mod common;

use ae_core::tasks::Manager;
use common::*;
use serde_json::Value;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const FILE: &str = "src/middleware/rateLimit.ts";

async fn started(sandbox: &Sandbox, text: &str) -> (Arc<Manager>, String, PathBuf) {
    let project_dir = git_project(sandbox);
    let manager = sandbox.manager();
    let project = manager.add_project(&project_dir.to_string_lossy(), None).unwrap();
    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock", text).await.unwrap();
    until(&mut events, &task.row.id, |e| state_is(e, "idle")).await;
    (manager, task.row.id.clone(), PathBuf::from(&task.row.cwd))
}

fn states(manager: &Manager, id: &str) -> Vec<(String, String)> {
    manager.team(id).unwrap().steps.iter().map(|step| (step.role.clone(), step.state.clone())).collect()
}

/// Waits until the team of a task is at rest: done, paused, asking or failed.
async fn at_rest(manager: &Manager, id: &str) {
    for _ in 0..800 {
        if !manager.team(id).unwrap().running {
            return;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the team did not come to rest: {:?}", states(manager, id));
}

/// Allows every edit the task's agents ask for, until the task is dropped.
fn allow_edits(manager: &Arc<Manager>, id: &str) -> tokio::task::JoinHandle<()> {
    let (manager, id) = (manager.clone(), id.to_string());
    let mut events = manager.subscribe();
    tokio::spawn(async move {
        while let Ok(message) = events.recv().await {
            let event = &message["data"];
            if message["event"] == "task.event" && event["taskId"] == id.as_str() && is(event, "permission_request") {
                let _ = manager.answer_permission(&id, event["requestId"].as_str().unwrap(), Some("allow-once".into()));
            }
        }
    })
}

fn pairs(list: &[(&str, &str)]) -> Vec<(String, String)> {
    list.iter().map(|(a, b)| (a.to_string(), b.to_string())).collect()
}

#[tokio::test]
async fn a_team_plans_builds_reviews_and_runs_the_checks() {
    let sandbox = Sandbox::new("team");
    let (manager, id, root) = started(&sandbox, "Take the login limit from config").await;
    manager.check_add(&id, "Always fine", "true").unwrap();
    let allowing = allow_edits(&manager, &id);

    let team = manager.team_start(&id, "mock-load", "mock", Some("mock-modes"), None).await.unwrap();
    assert_eq!(team.text, "Take the login limit from config", "the task is what the user first asked for");
    assert!(team.running);
    at_rest(&manager, &id).await;
    allowing.abort();

    assert_eq!(
        states(&manager, &id),
        pairs(&[("plan", "done"), ("build", "done"), ("review", "done"), ("checks", "done")])
    );
    let team = manager.team(&id).unwrap();
    assert!(team.stopped.is_none());
    assert_eq!(team.rounds, 1);
    assert_eq!(
        team.steps[2].note.as_deref(),
        Some("The change does what the plan says."),
        "the verdict line is not handed on"
    );
    assert_eq!(team.steps[3].note.as_deref(), Some("1 of 1 lines of “Done when” are done."));
    // The builder really built.
    assert!(std::fs::read_to_string(root.join(FILE)).unwrap().contains("limit: limits.login,"));

    let history = manager.history(&id).unwrap();
    // Each step went to its own agent, in order.
    let handed: Vec<&str> =
        history.iter().filter(|e| is(e, "agent_switched")).map(|e| e["to"].as_str().unwrap()).collect();
    assert_eq!(handed, ["mock-load", "mock", "mock-modes"]);
    // What went from one agent to the next is in the conversation as a note, not as something the user said.
    let notes: Vec<(&str, &str, &str)> = history
        .iter()
        .filter(|e| is(e, "team_note"))
        .map(|e| (e["role"].as_str().unwrap(), e["from"].as_str().unwrap(), e["to"].as_str().unwrap()))
        .collect();
    assert_eq!(
        notes,
        [
            ("plan", "You", "Practice agent"),
            ("build", "Practice agent", "Practice agent"),
            ("review", "Practice agent", "Practice agent")
        ]
    );
    assert_eq!(history.iter().filter(|e| is(e, "user_message")).count(), 1, "only the user's own first message");
    // The builder was told the plan, the reviewer what the builder said.
    let told: Vec<String> = history
        .iter()
        .filter(|e| e["update"]["sessionUpdate"] == "agent_thought_chunk")
        .filter_map(|e| e["update"]["content"]["text"].as_str().map(str::to_string))
        .collect();
    assert!(told.iter().any(|t| t.contains("You are the planner") && t.contains("Take the login limit from config")));
    assert!(told.iter().any(|t| t.contains("You are the builder") && t.contains("The plan, from Practice agent:")));
    assert!(told.iter().any(|t| t.contains("You are the reviewer") && t.contains("What Practice agent said it did:")));

    assert!(manager.team_continue(&id).unwrap_err().to_string().contains("has finished"));
}

#[tokio::test]
async fn a_reviewer_who_wants_changes_asks_the_user_and_the_builder_builds_again() {
    let sandbox = Sandbox::new("team-redo");
    let (manager, id, _root) = started(&sandbox, "Be strict about the login limit").await;
    let allowing = allow_edits(&manager, &id);
    manager.team_start(&id, "mock", "mock", Some("mock-modes"), None).await.unwrap();
    at_rest(&manager, &id).await;

    // The reviewer found something: the team waits for the user.
    assert_eq!(
        states(&manager, &id),
        pairs(&[("plan", "done"), ("build", "done"), ("review", "asks"), ("checks", "waiting")])
    );
    let team = manager.team(&id).unwrap();
    assert_eq!(team.steps[2].note.as_deref(), Some("Line 3 still holds a number that should come from config."));
    assert!(manager.team_continue(&id).unwrap_err().to_string().contains("waiting for your answer"));

    // Sent back: the builder is told what the reviewer found, then the reviewer looks again and is content.
    manager.team_answer(&id, true).unwrap();
    at_rest(&manager, &id).await;
    allowing.abort();
    let team = manager.team(&id).unwrap();
    assert_eq!(team.rounds, 2);
    assert_eq!(team.steps[2].state, "done");
    assert_eq!(team.steps[3].state, "skipped", "this project has no checks");
    let history = manager.history(&id).unwrap();
    let back = history.iter().filter(|e| is(e, "team_note") && e["role"] == "build").nth(1).unwrap();
    assert_eq!(back["text"], "Line 3 still holds a number that should come from config.");
    assert!(manager.team_answer(&id, true).is_err(), "nobody is asking any more");
}

#[tokio::test]
async fn a_team_can_be_paused_stopped_and_gone_on_with() {
    let sandbox = Sandbox::new("team-pause");
    let (manager, id, _root) = started(&sandbox, "Take the login limit from config").await;
    // Nobody allows the builder's edit: the build waits for the user.
    let mut events = manager.subscribe();
    manager.team_start(&id, "mock", "mock", None, None).await.unwrap();
    until(&mut events, &id, |e| is(e, "permission_request")).await;
    assert_eq!(states(&manager, &id)[1], ("build".to_string(), "running".to_string()));
    assert!(manager.team_start(&id, "mock", "mock", None, None).await.unwrap_err().to_string().contains("already"));

    // Stopping the team stops the turn that is running.
    manager.team_stop(&id).unwrap();
    until(&mut events, &id, |e| state_is(e, "idle")).await;
    at_rest(&manager, &id).await;
    let team = manager.team(&id).unwrap();
    assert_eq!(team.steps[1].state, "stopped");
    assert_eq!(team.stopped.as_deref(), Some("You stopped the team."));
    assert_eq!(team.steps.len(), 3, "without a reviewer the checks follow the build");

    // Going on builds again; with "pause after this step" the team stops before the checks.
    let allowing = allow_edits(&manager, &id);
    manager.team_continue(&id).unwrap();
    manager.team_pause(&id, true).unwrap();
    at_rest(&manager, &id).await;
    allowing.abort();
    let team = manager.team(&id).unwrap();
    assert_eq!((team.steps[1].state.as_str(), team.steps[2].state.as_str()), ("done", "waiting"));
    assert_eq!(team.stopped.as_deref(), Some("Paused after this step, as you asked."));
    manager.team_continue(&id).unwrap();
    at_rest(&manager, &id).await;
    assert_eq!(manager.team(&id).unwrap().steps[2].state, "skipped");
}

#[tokio::test]
async fn a_team_gone_on_with_right_after_a_stop_builds_again() {
    let sandbox = Sandbox::new("team-stop-go");
    let (manager, id, _root) = started(&sandbox, "Take the login limit from config").await;
    let mut events = manager.subscribe();
    manager.team_start(&id, "mock", "mock", None, None).await.unwrap();
    until(&mut events, &id, |e| is(e, "permission_request")).await;

    // The user does not wait for the stopped turn to be over: the agent is still coming to a stop,
    // and the task still waits for the answer nobody gave.
    manager.team_stop(&id).unwrap();
    let allowing = allow_edits(&manager, &id);
    manager.team_continue(&id).unwrap();
    assert!(manager.team(&id).unwrap().running);
    at_rest(&manager, &id).await;
    allowing.abort();

    let team = manager.team(&id).unwrap();
    assert_eq!(states(&manager, &id), pairs(&[("plan", "done"), ("build", "done"), ("checks", "skipped")]));
    assert_eq!(team.stopped, None, "the end of the stopped turn is not the end of the new one");
}

#[tokio::test]
async fn a_team_that_cannot_be_is_refused() {
    let sandbox = Sandbox::new("team-refused");
    let (manager, id, _root) = started(&sandbox, "Take the login limit from config").await;
    assert!(manager.team(&id).is_none());
    let refused = |plan: &'static str, build: &'static str| {
        let (manager, id) = (manager.clone(), id.clone());
        async move { manager.team_start(&id, plan, build, None, None).await.unwrap_err().to_string() }
    };
    assert!(refused("nobody", "mock").await.contains("No such agent"));
    assert!(refused("mock", "mock-missing").await.contains("not available on this computer"));
    // A program in a terminal answers where the editor cannot read: its plan could not be handed on,
    // and its verdict would never be seen.
    assert_eq!(
        refused("mock-terminal", "mock").await,
        "Practice agent in a terminal runs in a terminal, where the editor cannot read its answer, so it cannot plan for a team. It can build: pick another agent to plan."
    );
    let as_reviewer =
        manager.team_start(&id, "mock", "mock", Some("mock-terminal"), None).await.unwrap_err().to_string();
    assert!(as_reviewer.contains("so it cannot review for a team"), "{as_reviewer}");
    assert!(manager.team_pause(&id, true).is_err());
    assert!(manager.team_stop(&id).is_err());
    assert_eq!(Value::Null, serde_json::to_value(manager.team(&id)).unwrap());
}

/// A program in a terminal can build: its work is in the files, whatever it writes on its screen.
#[cfg(unix)]
#[tokio::test]
async fn a_program_in_a_terminal_builds_for_a_team_between_two_agents_that_speak_the_protocol() {
    let sandbox = Sandbox::new("team-terminal");
    let (manager, id, _root) = started(&sandbox, "Take the login limit from config").await;
    // The terminals of the core, as the server has them, and a short quiet so the builder's turn ends soon.
    let state = ae_core::server::AppState::new(manager.clone());
    manager.set_terminal_quiet(Duration::from_millis(300));

    manager.team_start(&id, "mock", "mock-terminal", Some("mock-modes"), None).await.unwrap();
    at_rest(&manager, &id).await;
    assert_eq!(
        states(&manager, &id),
        pairs(&[("plan", "done"), ("build", "done"), ("review", "done"), ("checks", "skipped")])
    );
    let history = manager.history(&id).unwrap();
    let handed: Vec<&str> =
        history.iter().filter(|e| is(e, "agent_switched")).map(|e| e["to"].as_str().unwrap()).collect();
    assert_eq!(handed, ["mock-terminal", "mock-modes"], "the planner had the task already");

    // The builder was a program in a terminal of the task, and was told the plan there.
    let started: Vec<bool> = history.iter().filter(|e| is(e, "agent_started")).map(|e| e["terminal"] == true).collect();
    assert_eq!(started.iter().filter(|in_terminal| **in_terminal).count(), 1, "{started:?}");
    // It said nothing the editor could read, and the reviewer is told so instead of being handed nothing.
    let told: Vec<String> = history
        .iter()
        .filter(|e| e["update"]["sessionUpdate"] == "agent_thought_chunk")
        .filter_map(|e| e["update"]["content"]["text"].as_str().map(str::to_string))
        .collect();
    let review = told.iter().find(|text| text.contains("You are the reviewer")).expect("the reviewer was asked");
    assert!(review.contains("What Practice agent in a terminal said it did:\n(it said nothing)"), "{review}");
    // The terminal of the builder went with it when the reviewer took the task.
    assert!(state.terminals.find(&id, ae_core::terminals::AGENT).is_none());
    manager.stop_all().await;
}

#[tokio::test]
async fn a_team_cut_short_by_the_core_stopping_is_at_rest_and_can_go_on() {
    let sandbox = Sandbox::new("team-crash");
    let (manager, id, _root) = started(&sandbox, "Take the login limit from config").await;
    // Nobody allows the builder's edit, so the build is still running when the core goes away.
    let mut events = manager.subscribe();
    manager.team_start(&id, "mock", "mock", None, None).await.unwrap();
    until(&mut events, &id, |e| is(e, "permission_request")).await;
    let running = sandbox.store().setting(&format!("task.{id}.team")).unwrap().unwrap();
    assert!(running.contains("\"running\":true"), "{running}");

    // The core is killed outright: its agent dies with it, and nothing of it writes anything more.
    // What is on disk still says the team was at work — that is what the next core finds.
    manager.stop_all().await;
    until(&mut events, &id, |e| state_is(e, "asleep")).await;
    sandbox.store().set_setting(&format!("task.{id}.team"), Some(&running)).unwrap();

    // A second core over the same data, as after a crash.
    let next = sandbox.manager();
    next.recover().unwrap();
    let team = next.team(&id).unwrap();
    assert!(!team.running);
    assert_eq!(team.steps[1].state, "stopped");
    assert_eq!(team.stopped.as_deref(), Some("The editor was closed while the team worked."));
    // Recovering again leaves it as it is.
    next.recover().unwrap();
    assert_eq!(next.team(&id).unwrap().stopped, team.stopped);

    // Going on builds again from the step that was cut short.
    let allowing = allow_edits(&next, &id);
    next.team_continue(&id).unwrap();
    at_rest(&next, &id).await;
    allowing.abort();
    let team = next.team(&id).unwrap();
    if team.steps[1].state != "done" {
        panic!("{:?} {:?}", states(&next, &id), team.stopped);
    }
    next.stop_all().await;
}
