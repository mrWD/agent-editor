//! What a session offers to set, and picking a session up again after its agent stopped.
//!
//! Only the practice agent runs here, so nothing is spent on anyone's account.

mod common;

use ae_core::tasks::TaskInfo;
use common::*;
use serde_json::Value;

/// The current value of one setting of a task's running agent.
fn current(task: &TaskInfo, option: &str) -> Option<String> {
    let config = task.config.as_ref()?;
    let found = config["options"].as_array()?.iter().find(|one| one["id"] == option)?;
    found["currentValue"].as_str().map(str::to_string)
}

fn thought(events: &[Value]) -> String {
    events
        .iter()
        .filter(|e| e["update"]["sessionUpdate"] == "agent_thought_chunk")
        .filter_map(|e| e["update"]["content"]["text"].as_str())
        .collect()
}

#[tokio::test]
async fn an_agent_that_can_load_picks_its_session_up_without_a_recap() {
    let sandbox = Sandbox::new("load");
    let manager = sandbox.manager();
    let project = manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock-load", "What does the limiter do?").await.unwrap();
    let id = task.row.id.clone();
    until(&mut events, &id, |e| state_is(e, "idle")).await;

    manager.stop_agent(&id).await.unwrap();
    until(&mut events, &id, |e| state_is(e, "asleep")).await;
    assert!(manager.task(&id).unwrap().config.is_none(), "a sleeping task offers nothing to set");

    manager.prompt(&id, "And who calls it?".into()).await.unwrap();
    let seen = until(&mut events, &id, |e| state_is(e, "idle")).await;
    let started = seen.iter().find(|e| is(e, "agent_started")).unwrap();
    assert_eq!(started["resumed"], true);
    assert_eq!(started["loaded"], true, "the agent took its own session back");
    // The agent remembers the conversation by itself, so it is told only the new message.
    assert_eq!(thought(&seen), "The user asked: And who calls it?");
    // What the agent sent again while loading is not written down a second time.
    let history = manager.history(&id).unwrap();
    assert!(!history.iter().any(|e| e["update"]["content"]["text"] == "An earlier answer, sent again."));
    assert_eq!(current(&manager.task(&id).unwrap(), "mode").as_deref(), Some("ask"));
}

#[tokio::test]
async fn a_session_the_agent_lost_is_opened_anew_and_told_what_was_said() {
    let sandbox = Sandbox::new("lost");
    let manager = sandbox.manager();
    let project = manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock-load", "What does the limiter do?").await.unwrap();
    let id = task.row.id.clone();
    until(&mut events, &id, |e| state_is(e, "idle")).await;
    manager.stop_agent(&id).await.unwrap();
    until(&mut events, &id, |e| state_is(e, "asleep")).await;

    // The agent no longer has a session by this name.
    sandbox.store().set_agent_session(&id, "gone").unwrap();

    manager.prompt(&id, "And who calls it?".into()).await.unwrap();
    let seen = until(&mut events, &id, |e| state_is(e, "idle")).await;
    assert_eq!(seen.iter().find(|e| is(e, "agent_started")).unwrap()["loaded"], false);
    let told = thought(&seen);
    assert!(told.contains("continues an earlier conversation"), "{told}");
    assert!(told.contains("User: What does the limiter do?"), "{told}");

    // The new session is the one remembered from now on.
    let kept = sandbox.store().task(&id).unwrap().agent_session.unwrap();
    assert!(kept.starts_with("mock-"), "{kept}");
}

#[tokio::test]
async fn settings_are_offered_changed_and_remembered() {
    let sandbox = Sandbox::new("settings");
    let project_dir = git_project(&sandbox);
    let manager = sandbox.manager();
    let project = manager.add_project(&project_dir.to_string_lossy(), None).unwrap();
    let mut events = manager.subscribe();
    let first = manager.create(&project.id, "mock", "What does the limiter do?").await.unwrap();
    let id = first.row.id.clone();
    assert_eq!(current(&first, "mode").as_deref(), Some("ask"));
    assert_eq!(current(&first, "model").as_deref(), Some("practice"));
    until(&mut events, &id, |e| state_is(e, "idle")).await;

    // A change is answered with the settings as they stand and is told to every window.
    let mut windows = manager.subscribe();
    let config = manager.set_option(&id, "model", "practice-fast").await.unwrap();
    assert_eq!(config["options"][1]["currentValue"], "practice-fast");
    let told = loop {
        let message = windows.recv().await.unwrap();
        if message["event"] == "task.config" {
            break message["data"].clone();
        }
    };
    assert_eq!(told["taskId"], id.as_str());
    assert_eq!(told["config"]["options"][1]["currentValue"], "practice-fast");

    // In the mode that edits freely no question is asked.
    manager.set_option(&id, "mode", "auto").await.unwrap();
    manager.prompt(&id, "Fix the limit".into()).await.unwrap();
    let seen = until(&mut events, &id, |e| state_is(e, "idle")).await;
    assert!(!seen.iter().any(|e| is(e, "permission_request")), "no question in the auto mode");
    let text =
        std::fs::read_to_string(std::path::Path::new(&first.row.cwd).join("src/middleware/rateLimit.ts")).unwrap();
    assert!(text.contains("limit: limits.login,"), "{text}");

    // The model goes with the agent to the next task; the mode does not.
    let second = manager.create(&project.id, "mock", "What does the limiter do?").await.unwrap();
    assert_eq!(current(&second, "model").as_deref(), Some("practice-fast"));
    assert_eq!(current(&second, "mode").as_deref(), Some("ask"), "a new task asks again");
    until(&mut events, &second.row.id, |e| state_is(e, "idle")).await;

    // A task keeps its own mode when its agent is started again.
    manager.stop_agent(&id).await.unwrap();
    until(&mut events, &id, |e| state_is(e, "asleep")).await;
    manager.prompt(&id, "What does the limiter do?".into()).await.unwrap();
    until(&mut events, &id, |e| state_is(e, "idle")).await;
    let woken = manager.task(&id).unwrap();
    assert_eq!(current(&woken, "mode").as_deref(), Some("auto"));
    assert_eq!(current(&woken, "model").as_deref(), Some("practice-fast"));
}

#[tokio::test]
async fn a_setting_that_cannot_be_made_is_refused_with_a_reason() {
    let sandbox = Sandbox::new("refused");
    let manager = sandbox.manager();
    let project = manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock", "What does the limiter do?").await.unwrap();
    let id = task.row.id.clone();
    until(&mut events, &id, |e| state_is(e, "idle")).await;

    let error = manager.set_option(&id, "model", "no-such-model").await.unwrap_err().to_string();
    assert!(error.contains("model cannot be no-such-model"), "{error}");
    assert_eq!(current(&manager.task(&id).unwrap(), "model").as_deref(), Some("practice"));

    // Nothing was remembered: the next task starts with the agent's own choice.
    let next = manager.create(&project.id, "mock", "What does the limiter do?").await.unwrap();
    assert_eq!(current(&next, "model").as_deref(), Some("practice"));

    manager.stop_agent(&id).await.unwrap();
    until(&mut events, &id, |e| state_is(e, "asleep")).await;
    let error = manager.set_option(&id, "model", "practice-fast").await.unwrap_err().to_string();
    assert!(error.contains("No agent is running in this task"), "{error}");
}

#[tokio::test]
async fn an_agent_that_only_knows_modes_offers_them_as_one_setting() {
    let sandbox = Sandbox::new("modes");
    let project_dir = git_project(&sandbox);
    let manager = sandbox.manager();
    let project = manager.add_project(&project_dir.to_string_lossy(), None).unwrap();
    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock-modes", "What does the limiter do?").await.unwrap();
    let id = task.row.id.clone();
    let options = task.config.as_ref().unwrap()["options"].as_array().unwrap().clone();
    assert_eq!(options.len(), 1);
    assert_eq!(options[0]["id"], "mode");
    assert_eq!(options[0]["currentValue"], "ask");
    assert_eq!(options[0]["options"][1]["value"], "auto");
    assert_eq!(options[0]["options"][1]["name"], "Edit freely");
    until(&mut events, &id, |e| state_is(e, "idle")).await;

    manager.set_option(&id, "mode", "auto").await.unwrap();
    assert_eq!(current(&manager.task(&id).unwrap(), "mode").as_deref(), Some("auto"));
    manager.prompt(&id, "Fix the limit".into()).await.unwrap();
    let seen = until(&mut events, &id, |e| state_is(e, "idle")).await;
    assert!(!seen.iter().any(|e| is(e, "permission_request")));
    // What the agent says about its mode is not part of the conversation.
    assert!(!seen.iter().any(|e| e["update"]["sessionUpdate"] == "current_mode_update"));

    let error = manager.set_option(&id, "model", "anything").await.unwrap_err().to_string();
    assert!(error.contains("no setting called model"), "{error}");
}

#[tokio::test]
async fn a_turn_ends_with_a_note_on_how_full_the_context_is() {
    let sandbox = Sandbox::new("usage");
    let manager = sandbox.manager();
    let project = manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock", "What does the limiter do?").await.unwrap();
    let seen = until(&mut events, &task.row.id, |e| state_is(e, "idle")).await;
    let usage = seen.iter().find(|e| e["update"]["sessionUpdate"] == "usage_update").expect("a usage note");
    assert!(usage["update"]["used"].as_u64().unwrap() > 0);
    assert_eq!(usage["update"]["size"], 200_000);
    assert_eq!(usage["update"]["cost"]["currency"], "USD");
}

/// Real agents are started through a launcher. Stopping the agent must stop the
/// agent itself, not only the launcher: a leftover would keep running, and
/// keep its output open, for as long as the computer is on.
#[cfg(unix)]
#[tokio::test]
async fn stopping_an_agent_started_through_a_launcher_leaves_nothing_running() {
    let running = || {
        let found = std::process::Command::new("pgrep").args(["-f", &wrapped_marker()]).output().expect("pgrep runs");
        String::from_utf8_lossy(&found.stdout).split_whitespace().count()
    };
    let sandbox = Sandbox::new("launcher");
    let manager = sandbox.manager();
    let project = manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock-wrapped", "What does the limiter do?").await.unwrap();
    let id = task.row.id.clone();
    until(&mut events, &id, |e| state_is(e, "idle")).await;
    assert_eq!(running(), 3, "the launcher, the agent it started and the agent's helper");

    manager.stop_agent(&id).await.unwrap();
    until(&mut events, &id, |e| state_is(e, "asleep")).await;
    for _ in 0..50 {
        if running() == 0 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(running(), 0, "nothing of the agent is left running");
}
