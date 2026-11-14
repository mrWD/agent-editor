//! The background jobs of a project: found in its files, started with arguments, watched to their end.
//!
//! Nothing of Celery or Django is installed here: the jobs are found by reading, and they are
//! "started" with command lines of the test's own, the way a project replaces the usual ones.
#![cfg(unix)]

mod common;

use ae_core::server::{AppState, dispatch};
use common::*;
use serde_json::{Value, json};
use std::time::Duration;

async fn asked(state: &AppState, method: &str, params: Value) -> anyhow::Result<Value> {
    dispatch(state, method, &params).await
}

async fn idle_task(state: &AppState, project_id: &str) -> String {
    let mut events = state.manager.subscribe();
    let task = state.manager.create(project_id, "mock", "What is here?").await.unwrap();
    until(&mut events, &task.row.id, |e| state_is(e, "idle")).await;
    task.row.id
}

/// Asks how a job stands until `done` is satisfied, and gives that answer.
async fn state_until(state: &AppState, task_id: &str, id: &str, done: impl Fn(&Value) -> bool) -> Value {
    let mut last = Value::Null;
    for _ in 0..400 {
        last = asked(state, "jobs.state", json!({ "taskId": task_id, "id": id })).await.unwrap();
        if done(&last) {
            return last;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the job never got there; it stands as {last}");
}

const TASKS: &str =
    "from celery import shared_task\n\n@shared_task\ndef send_receipt(order_id, copy_to=None):\n    ...\n";

#[tokio::test]
async fn the_jobs_of_a_project_are_listed_with_the_ones_the_task_touched_first() {
    let sandbox = Sandbox::new("jobs-list");
    let project = git_project(&sandbox);
    std::fs::create_dir_all(project.join("shop")).unwrap();
    std::fs::write(project.join("shop/__init__.py"), "").unwrap();
    std::fs::write(project.join("shop/celery.py"), "from celery import Celery\napp = Celery('shop')\n").unwrap();
    std::fs::write(project.join("shop/tasks.py"), TASKS).unwrap();
    std::fs::write(
        project.join("shop/mail.py"),
        "from celery import shared_task\n\n@shared_task\ndef digest():\n    ...\n",
    )
    .unwrap();
    git(&project, &["add", "."]);
    git(&project, &["commit", "-q", "-m", "Tasks"]);
    let state = AppState::new(sandbox.manager());
    let added = state.manager.add_project(&project.to_string_lossy(), None).unwrap();
    let task_id = idle_task(&state, &added.id).await;
    let folder = state.manager.cwd(&task_id).unwrap();

    let before = asked(&state, "jobs.get", json!({ "taskId": task_id })).await.unwrap();
    let names = |found: &Value| -> Vec<(String, bool)> {
        found["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .map(|job| (job["name"].as_str().unwrap().to_string(), job["changed"].as_bool().unwrap()))
            .collect()
    };
    assert_eq!(
        names(&before),
        [("shop.mail.digest".to_string(), false), ("shop.tasks.send_receipt".to_string(), false)]
    );
    assert_eq!(before["celeryApp"], "shop.celery:app");
    assert_eq!(before["jobs"][1]["takes"], json!(["order_id", "copy_to=None"]));
    assert!(before["templates"]["celery"].as_str().unwrap().starts_with("id=$(celery -A shop.celery:app call {name}"));

    // The task writes a job and touches the file of another: both come first.
    std::fs::write(folder.join("shop/tasks.py"), format!("{TASKS}\n@shared_task\ndef refund(order_id):\n    ...\n"))
        .unwrap();
    let after = asked(&state, "jobs.get", json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(
        names(&after),
        [
            ("shop.tasks.refund".to_string(), true),
            ("shop.tasks.send_receipt".to_string(), true),
            ("shop.mail.digest".to_string(), false)
        ]
    );
}

#[tokio::test]
async fn a_job_is_started_with_its_arguments_and_what_it_printed_is_kept_after_it_ends() {
    let sandbox = Sandbox::new("jobs-run");
    let project = sandbox.project();
    std::fs::write(project.join("tasks.py"), TASKS).unwrap();
    std::fs::write(project.join(".env"), "SHOP_KEY=sk-secret-value-123\n").unwrap();
    let state = AppState::new(sandbox.manager());
    let added = state.manager.add_project(&project.to_string_lossy(), None).unwrap();
    let task_id = idle_task(&state, &added.id).await;
    let id = "celery:tasks.send_receipt";
    let run = |args: &str, kwargs: &str| json!({ "taskId": task_id, "id": id, "args": args, "kwargs": kwargs });

    // The project's own line stands in for Celery: it says what it was given, three ways.
    let own = "echo called {name} with {args} and {kwargs}; echo \"from the environment: $AE_ARGS $AE_KWARGS\"; echo \"key $SHOP_KEY\"; exit 4";
    asked(&state, "jobs.set", json!({ "taskId": task_id, "kind": "celery", "template": own })).await.unwrap();
    assert!(asked(&state, "jobs.set", json!({ "taskId": task_id, "kind": "cron", "template": "x" })).await.is_err());
    let kept = asked(&state, "jobs.get", json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(kept["settings"]["celery"], own);

    let never = asked(&state, "jobs.state", json!({ "taskId": task_id, "id": id })).await.unwrap();
    assert_eq!(never, json!({ "running": false, "termId": null, "exit": null, "output": null }));

    let started = asked(&state, "jobs.run", run("[42, \"tea\"]", "{\"copy_to\": \"a@b.c\"}")).await.unwrap();
    assert!(started["termId"].is_u64());
    assert!(
        started["command"]
            .as_str()
            .unwrap()
            .starts_with("echo called tasks.send_receipt with '[42,\"tea\"]' and '{\"copy_to\":\"a@b.c\"}'")
    );
    let ended = state_until(&state, &task_id, id, |now| now["running"] == false && now["exit"].is_u64()).await;
    assert_eq!(ended["exit"], 4, "how it ended is kept");
    let printed = ended["output"].as_str().unwrap();
    assert!(printed.contains("called tasks.send_receipt with [42,\"tea\"] and {\"copy_to\":\"a@b.c\"}"), "{printed}");
    assert!(printed.contains("from the environment: [42,\"tea\"] {\"copy_to\":\"a@b.c\"}"), "{printed}");
    assert!(
        printed.contains("key ") && !printed.contains("sk-secret-value-123"),
        "a secret of the task is never shown: {printed}"
    );

    // What is not JSON never reaches a shell, and a job that is not there is not started.
    assert!(
        asked(&state, "jobs.run", run("[42", ""))
            .await
            .unwrap_err()
            .to_string()
            .starts_with("The arguments are not JSON")
    );
    let unknown =
        asked(&state, "jobs.run", json!({ "taskId": task_id, "id": "celery:nowhere" })).await.unwrap_err().to_string();
    assert_eq!(unknown, "This project has no job celery:nowhere");

    // One run of a job at a time; a run can be stopped.
    asked(&state, "jobs.set", json!({ "taskId": task_id, "kind": "celery", "template": "echo started; sleep 30" }))
        .await
        .unwrap();
    asked(&state, "jobs.run", run("", "")).await.unwrap();
    let going =
        state_until(&state, &task_id, id, |now| now["output"].as_str().is_some_and(|text| text.contains("started")))
            .await;
    assert_eq!(going["running"], true);
    let twice = asked(&state, "jobs.run", run("", "")).await.unwrap_err().to_string();
    assert_eq!(twice, "tasks.send_receipt is running already. Stop it first, or wait for it to end.");
    asked(&state, "jobs.stop", json!({ "taskId": task_id, "id": id })).await.unwrap();
    state_until(&state, &task_id, id, |now| now["running"] == false).await;
    state.terminals.close_all();
}
