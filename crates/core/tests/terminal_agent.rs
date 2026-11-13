//! An agent that does not speak the protocol: a program in a terminal of the
//! task, which the editor types into. Tried with the practice agent started
//! as such a program; it reads lines and answers "You said: …".
#![cfg(unix)]

mod common;

use ae_core::server::{AppState, dispatch};
use ae_core::terminals::AGENT;
use common::{Sandbox, git, git_project, is, state_is, until};
use serde_json::{Value, json};
use std::time::Duration;

/// A core whose agents in terminals are taken to be done after a short quiet.
fn core(sandbox: &Sandbox) -> AppState {
    let state = AppState::new(sandbox.manager());
    state.manager.set_terminal_quiet(Duration::from_millis(300));
    state
}

/// What the task's agent has on its terminal, without colour codes.
fn screen(state: &AppState, task_id: &str) -> String {
    let info = state.terminals.find(task_id, AGENT).expect("the agent has a terminal");
    let (kept, _) = state.terminals.scrollback(info.term_id).unwrap();
    ae_core::terminals::strip_codes(&String::from_utf8_lossy(&kept))
}

async fn asked(state: &AppState, method: &str, params: Value) -> Value {
    dispatch(state, method, &params).await.unwrap_or_else(|error| panic!("{method} failed: {error:#}"))
}

#[tokio::test]
async fn a_message_is_typed_into_the_program_and_its_quiet_ends_the_turn() {
    let sandbox = Sandbox::new("terminal-agent");
    let state = core(&sandbox);
    let manager = &state.manager;
    let mut events = manager.subscribe();
    let project = manager.add_project(sandbox.project().to_str().unwrap(), None).unwrap();

    let task = manager.create(&project.id, "mock-terminal", "Describe the limiter").await.unwrap();
    let id = task.row.id.clone();
    // The task says which terminal its agent is in, so the interface can show it.
    let term_id = task.config.as_ref().unwrap()["terminal"].as_u64().expect("the terminal is named");
    assert_eq!(state.terminals.find(&id, AGENT).unwrap().term_id as u64, term_id);
    assert_eq!(task.row.agent_session, None);

    let seen = until(&mut events, &id, |event| state_is(event, "idle")).await;
    let kinds: Vec<&str> = seen.iter().filter_map(|event| event["type"].as_str()).collect();
    assert_eq!(kinds, ["agent_started", "user_message", "state", "turn_end", "state"], "{seen:#?}");
    assert_eq!(seen[0]["terminal"], true);
    assert_eq!(seen[2]["state"], "running");
    // The end of the turn is a guess, and says so.
    assert_eq!(seen[3]["stopReason"], "end_turn");
    assert_eq!(seen[3]["guessed"], true);
    assert!(screen(&state, &id).contains("You said: Describe the limiter"), "{}", screen(&state, &id));

    // The next message goes to the same program.
    manager.prompt(&id, "And again".into()).await.unwrap();
    until(&mut events, &id, |event| state_is(event, "idle")).await;
    assert!(screen(&state, &id).contains("You said: And again"));
    assert_eq!(state.terminals.find(&id, AGENT).unwrap().term_id as u64, term_id);

    // While it has the turn, nothing more can be sent.
    manager.prompt(&id, "Third".into()).await.unwrap();
    let refused = manager.prompt(&id, "Too soon".into()).await.unwrap_err();
    assert!(refused.to_string().contains("still working"), "{refused}");
    until(&mut events, &id, |event| state_is(event, "idle")).await;
}

#[tokio::test]
async fn what_the_program_changes_is_the_agents_and_a_message_of_many_lines_is_one_line() {
    let sandbox = Sandbox::new("terminal-agent-writes");
    let state = core(&sandbox);
    let manager = &state.manager;
    let mut events = manager.subscribe();
    let project = manager.add_project(git_project(&sandbox).to_str().unwrap(), None).unwrap();

    let said = "Please do this.\nWRITE notes/plan.md first then second";
    let task = manager.create(&project.id, "mock-terminal", said).await.unwrap();
    let id = task.row.id.clone();
    until(&mut events, &id, |event| state_is(event, "idle")).await;

    // The program reads lines, so the two lines went in as one, and it did what the second asked.
    assert!(screen(&state, &id).contains("You said: Please do this. WRITE notes/plan.md first then second"));
    let written = std::path::Path::new(&task.row.cwd).join("notes/plan.md");
    assert_eq!(std::fs::read_to_string(&written).unwrap(), "first then second\n");
    // The task works in a copy of its own, and what changed there is marked as the agent's.
    assert!(git(std::path::Path::new(&task.row.cwd), &["status", "--short"]).contains("notes/"));
    let review = asked(&state, "review.list", json!({ "taskId": id })).await;
    let file = review["files"]
        .as_array()
        .and_then(|files| files.iter().find(|file| file["path"] == "notes/plan.md"))
        .unwrap_or_else(|| panic!("the new file is listed: {review:#}"));
    assert_eq!(file["agentLines"], 1, "{file:#}");
}

#[tokio::test]
async fn a_program_that_marks_pasted_text_gets_every_line_of_the_message() {
    let sandbox = Sandbox::new("terminal-agent-paste");
    let state = core(&sandbox);
    let manager = &state.manager;
    let mut events = manager.subscribe();
    let project = manager.add_project(sandbox.project().to_str().unwrap(), None).unwrap();

    let task = manager.create(&project.id, "mock-terminal-paste", "First line.\nSecond line.").await.unwrap();
    until(&mut events, &task.row.id, |event| state_is(event, "idle")).await;
    // It prints the lines of one message with a stroke between them.
    let screen = screen(&state, &task.row.id);
    assert!(screen.contains("You said: First line. / Second line."), "{screen}");
}

#[tokio::test]
async fn a_program_that_ends_leaves_the_task_asleep_and_the_next_message_starts_it_again_with_what_was_said() {
    let sandbox = Sandbox::new("terminal-agent-wakes");
    let state = core(&sandbox);
    let manager = &state.manager;
    let mut events = manager.subscribe();
    let project = manager.add_project(sandbox.project().to_str().unwrap(), None).unwrap();

    let task = manager.create(&project.id, "mock-terminal", "Remember the word heron").await.unwrap();
    let id = task.row.id.clone();
    until(&mut events, &id, |event| state_is(event, "idle")).await;
    let first = state.terminals.find(&id, AGENT).unwrap().term_id;

    // Stopping the agent ends the program and its terminal.
    manager.stop_agent_and_wait(&id).await.unwrap();
    until(&mut events, &id, |event| state_is(event, "asleep")).await;
    assert!(state.terminals.find(&id, AGENT).is_none());
    assert_eq!(manager.task(&id).unwrap().config, None);

    manager.prompt(&id, "What was the word?".into()).await.unwrap();
    let seen = until(&mut events, &id, |event| state_is(event, "idle")).await;
    let started = seen.iter().find(|event| is(event, "agent_started")).expect("a new program was started");
    assert_eq!((started["resumed"].clone(), started["loaded"].clone()), (json!(true), json!(false)));
    let second = state.terminals.find(&id, AGENT).unwrap().term_id;
    assert_ne!(first, second);
    // The new program knows nothing, so it was told what the user said before, then the new message.
    let screen = screen(&state, &id);
    assert!(screen.contains("Remember the word heron"), "{screen}");
    assert!(screen.contains("Now the user says: What was the word?"), "{screen}");
}

#[tokio::test]
async fn a_line_the_user_types_into_the_terminal_is_a_turn_too_and_exit_puts_the_task_to_sleep() {
    let sandbox = Sandbox::new("terminal-agent-typed");
    let state = core(&sandbox);
    let manager = &state.manager;
    let mut events = manager.subscribe();
    let project = manager.add_project(sandbox.project().to_str().unwrap(), None).unwrap();

    let task = manager.create(&project.id, "mock-terminal", "Hello").await.unwrap();
    let id = task.row.id.clone();
    until(&mut events, &id, |event| state_is(event, "idle")).await;
    let term_id = state.terminals.find(&id, AGENT).unwrap().term_id;

    // Letters alone are not a message.
    state.terminals.input(term_id, b"typed by hand").unwrap();
    manager.typed_into(term_id, b"typed by hand");
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(serde_json::to_value(manager.task(&id).unwrap().state).unwrap(), "idle");

    // The line end is: the agent has the turn, and its quiet ends it.
    state.terminals.input(term_id, b"\r").unwrap();
    manager.typed_into(term_id, b"\r");
    let seen = until(&mut events, &id, |event| state_is(event, "idle")).await;
    assert!(seen.iter().any(|event| state_is(event, "running")), "{seen:#?}");
    assert!(seen.iter().any(|event| is(event, "turn_end")), "{seen:#?}");
    // Nothing is put in the conversation in the user's name: they did not say it there.
    assert!(!seen.iter().any(|event| is(event, "user_message")));
    assert!(screen(&state, &id).contains("You said: typed by hand"));

    // The program's own way out ends it; the task is asleep, with no complaint about a clean end.
    state.terminals.input(term_id, b"/exit\r").unwrap();
    manager.typed_into(term_id, b"/exit\r");
    let seen = until(&mut events, &id, |event| state_is(event, "asleep")).await;
    assert!(!seen.iter().any(|event| is(event, "error")), "{seen:#?}");
}

#[tokio::test]
async fn stop_this_turn_presses_the_key_that_stops_a_program() {
    let sandbox = Sandbox::new("terminal-agent-stop");
    let state = core(&sandbox);
    let manager = &state.manager;
    let mut events = manager.subscribe();
    let project = manager.add_project(sandbox.project().to_str().unwrap(), None).unwrap();

    let task = manager.create(&project.id, "mock-terminal", "Hello").await.unwrap();
    let id = task.row.id.clone();
    until(&mut events, &id, |event| state_is(event, "idle")).await;

    manager.prompt(&id, "One more".into()).await.unwrap();
    manager.cancel(&id).unwrap();
    // The practice program ends at that key, as any program that reads lines does.
    // It was stopped on purpose, so nothing is said about how it ended.
    let seen = until(&mut events, &id, |event| state_is(event, "asleep")).await;
    assert!(!seen.iter().any(|event| is(event, "error") && event["message"].as_str().unwrap().contains("by itself")));
}

#[tokio::test]
async fn a_program_that_is_not_there_makes_no_task_and_settings_say_what_such_an_agent_is() {
    let sandbox = Sandbox::new("terminal-agent-settings");
    let state = core(&sandbox);
    let manager = &state.manager;
    let project = manager.add_project(git_project(&sandbox).to_str().unwrap(), None).unwrap();

    // An agent of the user's own that runs in a terminal.
    let program = common::mock_agent().to_string_lossy().into_owned();
    let saved = asked(
        &state,
        "agents.save",
        json!({ "name": "Liner", "short": "Li", "program": program, "args": ["--terminal"], "terminal": true }),
    )
    .await;
    let id = saved["saved"].as_str().expect("the new agent has an id").to_string();
    let listed = asked(&state, "agents.list", json!({})).await;
    let mine = listed.as_array().unwrap().iter().find(|agent| agent["id"] == id.as_str()).unwrap();
    assert_eq!((mine["speaks"].clone(), mine["available"].clone()), (json!("terminal"), json!(true)));
    // "Try it" starts nothing: it says where the program is and how it will be used.
    let tried = asked(&state, "agents.try", json!({ "id": id })).await;
    assert_eq!(tried["ok"], true, "{tried:#}");
    assert!(tried["says"].as_str().unwrap().contains("runs in a terminal"));

    // It works in a task like the one the tests come with.
    let mut events = manager.subscribe();
    let task = manager.create(&project.id, &id, "Hello there").await.unwrap();
    until(&mut events, &task.row.id, |event| state_is(event, "idle")).await;
    assert!(screen(&state, &task.row.id).contains("You said: Hello there"));

    // With a program that does not exist, the task is not made and no working copy is left.
    asked(
        &state,
        "agents.save",
        json!({ "id": id, "name": "Liner", "short": "Li", "program": "/nonexistent/ae-test/liner", "terminal": true }),
    )
    .await;
    let before = manager.list().unwrap().len();
    let refused = manager.create(&project.id, &id, "Hello").await.unwrap_err();
    assert!(refused.to_string().contains("Liner"), "{refused}");
    assert_eq!(manager.list().unwrap().len(), before);
    let tried = asked(&state, "agents.try", json!({ "id": id })).await;
    assert_eq!(tried["ok"], false);
}
