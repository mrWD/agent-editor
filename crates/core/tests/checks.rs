//! The checks of a task: found in the project, run, and read.
#![cfg(unix)]

mod common;

use common::*;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

const FILE: &str = "src/middleware/rateLimit.ts";

/// A task in a project whose `package.json` brings a lint that passes and tests that fail.
async fn started(sandbox: &Sandbox, test_script: &str) -> (Arc<ae_core::tasks::Manager>, String, PathBuf) {
    let project_dir = git_project(sandbox);
    let package = json!({ "scripts": {
        "lint": "echo 'no problems'",
        "test": test_script,
        "e2e": "echo slow",
        "dev": "sleep 60"
    }});
    std::fs::write(project_dir.join("package.json"), package.to_string()).unwrap();
    git(&project_dir, &["add", "."]);
    git(&project_dir, &["-c", "user.name=T", "-c", "user.email=t@example.com", "commit", "-qm", "Scripts"]);
    let manager = sandbox.manager();
    let project = manager.add_project(&project_dir.to_string_lossy(), None).unwrap();
    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock", "What does the limiter do?").await.unwrap();
    until(&mut events, &task.row.id, |e| state_is(e, "idle")).await;
    (manager, task.row.id.clone(), PathBuf::from(&task.row.cwd))
}

fn check<'a>(state: &'a Value, id: &str) -> &'a Value {
    state["checks"].as_array().unwrap().iter().find(|check| check["id"] == id).expect("the check is listed")
}

/// Waits until no check of the task is running, and returns the state then.
async fn settled(manager: &ae_core::tasks::Manager, id: &str) -> Value {
    for _ in 0..600 {
        let state = manager.checks_state(id).unwrap();
        if state["running"] == 0 {
            return state;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the checks did not end");
}

#[tokio::test]
async fn checks_are_found_run_and_their_problems_read() {
    let sandbox = Sandbox::new("checks");
    let failing = format!("printf '{FILE}(3,10): error TS2322: Type mismatch.\\n'; echo 'Tests: 1 failed' >&2; exit 1");
    let (manager, id, _root) = started(&sandbox, &failing).await;

    let state = manager.checks_state(&id).unwrap();
    let ids: Vec<(&str, bool, &str)> = state["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["id"].as_str().unwrap(), c["on"].as_bool().unwrap(), c["state"].as_str().unwrap()))
        .collect();
    assert_eq!(ids, [("lint", true, "none"), ("test", true, "none"), ("e2e", false, "none")], "the slow one is off");
    assert_eq!(check(&state, "lint")["command"], "npm run lint");
    assert_eq!((state["done"].as_u64(), state["total"].as_u64()), (Some(0), Some(2)));

    // "Run all" starts what is on, and every window is told when each ends.
    let mut windows = manager.subscribe();
    manager.checks_run(&id, None).unwrap();
    let state = settled(&manager, &id).await;
    let lint = check(&state, "lint");
    assert_eq!((lint["state"].as_str(), lint["exitCode"].as_i64()), (Some("passed"), Some(0)));
    assert!(lint["tail"].as_str().unwrap().contains("no problems"));
    assert!(lint["problems"].as_array().unwrap().is_empty());
    let test = check(&state, "test");
    assert_eq!((test["state"].as_str(), test["exitCode"].as_i64()), (Some("failed"), Some(1)));
    assert!(test["tail"].as_str().unwrap().contains("Tests: 1 failed"), "what went to stderr is kept too");
    assert_eq!(
        test["problems"],
        json!([{ "path": FILE, "line": 3, "column": 10, "severity": "error", "message": "error TS2322: Type mismatch." }])
    );
    assert_eq!(check(&state, "e2e")["state"], "none", "what is off was not run");
    assert_eq!(
        (state["done"].as_u64(), state["failed"].as_u64(), state["total"].as_u64()),
        (Some(1), Some(1), Some(2))
    );
    let mut told = 0;
    while let Ok(message) = windows.try_recv() {
        told += usize::from(message["event"] == "checks.changed");
    }
    assert!(told >= 3, "once at the start and once as each ends: {told}");

    // A change to the files makes the verdicts old: they count for nothing until run again.
    manager.file_create(&id, "src/new.ts", false).unwrap();
    let state = manager.checks_state(&id).unwrap();
    assert_eq!(check(&state, "lint")["stale"], true);
    assert_eq!((state["done"].as_u64(), state["failed"].as_u64()), (Some(0), Some(0)));
    manager.checks_run(&id, Some("lint")).unwrap();
    let state = settled(&manager, &id).await;
    assert_eq!(check(&state, "lint")["stale"], false);
    assert_eq!(state["done"], 1);

    assert!(manager.checks_run(&id, Some("no-such")).is_err());
}

#[tokio::test]
async fn checks_are_turned_on_and_off_and_the_users_own_are_added() {
    let sandbox = Sandbox::new("checks-own");
    let (manager, id, root) = started(&sandbox, "exit 0").await;

    manager.check_turn(&id, "test", false).unwrap();
    manager.check_turn(&id, "e2e", true).unwrap();
    manager.check_add(&id, "The build is not broken", "test -f package.json").unwrap();
    manager.check_add(&id, "", "echo plain").unwrap();
    let state = manager.checks_state(&id).unwrap();
    let on: Vec<(String, bool)> = state["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| (c["title"].as_str().unwrap().to_string(), c["on"].as_bool().unwrap()))
        .collect();
    assert_eq!(
        on,
        [
            ("Lint is clean".to_string(), true),
            ("Unit tests pass".to_string(), false),
            ("End-to-end tests pass".to_string(), true),
            ("The build is not broken".to_string(), true),
            ("echo plain".to_string(), true),
        ]
    );
    let own = state["checks"][3]["id"].as_str().unwrap().to_string();
    assert_eq!(state["checks"][3]["from"], "you");

    // The choice belongs to the project: another task of it has the same checks.
    let project = manager.projects().unwrap().remove(0);
    let mut events = manager.subscribe();
    let other = manager.create(&project.id, "mock", "What does the limiter do?").await.unwrap();
    until(&mut events, &other.row.id, |e| state_is(e, "idle")).await;
    assert_eq!(manager.checks_state(&other.row.id).unwrap()["checks"].as_array().unwrap().len(), 5);

    manager.checks_run(&id, None).unwrap();
    let state = settled(&manager, &id).await;
    assert_eq!((state["done"].as_u64(), state["total"].as_u64()), (Some(4), Some(4)));
    assert_eq!(check(&state, "test")["state"], "none");
    // A command runs in the task's own folder.
    assert!(root.join("package.json").is_file());

    manager.check_remove(&id, &own).unwrap();
    assert!(manager.check_remove(&id, "lint").is_err(), "one from the project can only be turned off");
    assert!(manager.check_add(&id, "Nothing", "  ").is_err());
    assert_eq!(manager.checks_state(&id).unwrap()["checks"].as_array().unwrap().len(), 4);
}

#[tokio::test]
async fn a_running_check_can_be_stopped_and_leaves_nothing_behind() {
    let sandbox = Sandbox::new("checks-stop");
    let marker = format!("ae-check-{}", std::process::id());
    // The check starts a helper of its own, the way a test runner starts workers.
    let (manager, id, _root) = started(&sandbox, &format!("sh -c 'sleep 30; : {marker}' & sleep 30; : {marker}")).await;
    let running = || {
        let found = std::process::Command::new("pgrep").args(["-f", &marker]).output().unwrap();
        String::from_utf8_lossy(&found.stdout).split_whitespace().count()
    };

    manager.checks_run(&id, Some("test")).unwrap();
    // A shell and its helper can take a while to start on a busy machine.
    for _ in 0..500 {
        if running() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(check(&manager.checks_state(&id).unwrap(), "test")["state"], "running");
    assert!(running() >= 2, "the command and its helper are running");

    manager.check_stop(&id, "test").unwrap();
    for _ in 0..100 {
        if running() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(running(), 0, "nothing of the check is left running");
    let state = manager.checks_state(&id).unwrap();
    assert_eq!(check(&state, "test")["state"], "none", "a stopped check has no verdict");
    assert_eq!(state["running"], 0);
}

#[tokio::test]
async fn lines_a_person_confirms_are_added_ticked_and_removed() {
    let sandbox = Sandbox::new("checks-lines");
    let (manager, id, _root) = started(&sandbox, "exit 0").await;
    assert!(manager.checks_state(&id).unwrap()["lines"].as_array().unwrap().is_empty());

    manager.line_add(&id, " The sixth attempt in a minute gets 429 ").unwrap();
    assert!(manager.line_add(&id, "  ").is_err());
    let state = manager.checks_state(&id).unwrap();
    let line = state["lines"][0].clone();
    assert_eq!(
        (line["text"].as_str(), line["done"].as_bool(), line["from"].as_str()),
        (Some("The sixth attempt in a minute gets 429"), Some(false), Some("you"))
    );
    assert_eq!((state["done"].as_u64(), state["total"].as_u64()), (Some(0), Some(3)));

    let line_id = line["id"].as_str().unwrap();
    manager.line_set(&id, line_id, true).unwrap();
    assert_eq!(manager.checks_state(&id).unwrap()["done"], 1);
    manager.line_set(&id, line_id, false).unwrap();
    assert_eq!(manager.checks_state(&id).unwrap()["done"], 0);
    assert!(manager.line_set(&id, "nope", true).is_err());
    manager.line_remove(&id, line_id).unwrap();
    assert_eq!(manager.checks_state(&id).unwrap()["total"], 2);
}
