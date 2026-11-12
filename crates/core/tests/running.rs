//! End-to-end tests of running a project: its dev command and its Docker services.
//!
//! Docker is stood in for by a shell script, so no container is started here.
#![cfg(unix)]

mod common;

use ae_core::hosts::Hosts;
use ae_core::server::{AppState, dispatch};
use common::*;
use serde_json::{Value, json};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// A stand-in for `docker`. "Running" containers are whatever is in `running.json` next to it.
fn fake_docker(dir: &Path, wanted_port: u16) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let config = json!({ "name": "shop", "services": {
        "db": { "image": "postgres:16", "ports": [{ "target": 5432, "published": wanted_port.to_string(), "protocol": "tcp" }] },
        "worker": { "build": { "context": "." } }
    } });
    std::fs::write(dir.join("config.json"), config.to_string()).unwrap();
    fake_tool(
        dir,
        "docker",
        r#"here="$(dirname "$0")"
echo "$@" >> "$here/calls.log"
if [ "$1" = "info" ]; then echo "29.0.0"; exit 0; fi
case "$*" in
  *" config --format json") cat "$here/config.json" ;;
  *" ps --all --format json") [ -f "$here/running.json" ] && cat "$here/running.json"; exit 0 ;;
  *" up -d"*)
    # The task's own file is the one after -f; the port in it is the one "published".
    file=""; prev=""
    for word in "$@"; do [ "$prev" = "-f" ] && file="$word"; prev="$word"; done
    port=$(grep -o '"published": *"[0-9]*"' "$file" | head -1 | grep -o '[0-9]*')
    echo "Container db  Started"
    printf '{"Service":"db","State":"running","ExitCode":0,"Publishers":[{"PublishedPort":%s,"TargetPort":5432}]}\n' "$port" > "$here/running.json"
    printf '{"Service":"worker","State":"exited","ExitCode":3,"Publishers":[]}\n' >> "$here/running.json" ;;
  *" down"*|*" stop "*) rm -f "$here/running.json" ;;
  *) echo "unexpected call: $*" >&2; exit 2 ;;
esac
"#,
    )
}

fn hosts_with_docker(docker: PathBuf) -> Hosts {
    let missing = PathBuf::from("/nonexistent/ae-test/tool");
    Hosts::with(missing.clone(), missing, docker, "http://127.0.0.1:9/graphql".into())
}

/// Asks for the run state until `done` is satisfied, and returns that state.
async fn run_state_until(state: &AppState, task_id: &str, done: impl Fn(&Value) -> bool) -> Value {
    let mut last = Value::Null;
    for _ in 0..200 {
        last = dispatch(state, "run.state", &json!({ "taskId": task_id })).await.unwrap();
        if done(&last) {
            return last;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    panic!("the run state never got there; last: {last:#}");
}

fn service<'a>(state: &'a Value, name: &str) -> &'a Value {
    state["services"].as_array().unwrap().iter().find(|s| s["name"] == name).unwrap_or(&Value::Null)
}

async fn idle_task(state: &AppState, project_id: &str) -> String {
    let mut events = state.manager.subscribe();
    let task = state.manager.create(project_id, "mock", "What is here?").await.unwrap();
    until(&mut events, &task.row.id, |e| state_is(e, "idle")).await;
    task.row.id
}

#[tokio::test]
async fn a_dev_command_is_started_its_address_is_found_and_it_is_stopped() {
    let sandbox = Sandbox::new("devserver");
    let state = AppState::new(sandbox.manager());
    let project_dir = sandbox.project();
    // A stand-in for a dev server: it says where it listens, in colour as real ones do, and stays up.
    std::fs::write(
        project_dir.join("package.json"),
        r#"{"scripts":{"dev":"printf 'ready\\n  Local: http://localhost:\\033[1m4567\\033[22m/\\n'; sleep 60","build":"true","crash":"exit 0"}}"#,
    )
    .unwrap();
    let project = state.manager.add_project(&project_dir.to_string_lossy(), None).unwrap();
    let id = idle_task(&state, &project.id).await;

    let before = dispatch(&state, "run.state", &json!({ "taskId": id })).await.unwrap();
    assert_eq!(
        before["services"],
        json!([{ "name": "dev", "kind": "command", "runs": "npm run dev", "state": "stopped",
        "address": null, "note": null, "termId": null }])
    );
    assert_eq!(before["composeFile"], Value::Null);

    // Start the command the way the core would, without needing npm on the machine that runs the tests.
    let started = state
        .terminals
        .run(
            &id,
            &project_dir,
            "printf 'ready\\n  Local: http://localhost:\\033[1m4567\\033[22m/\\n'; sleep 60",
            "run:dev",
            &[],
        )
        .unwrap();

    let running = run_state_until(&state, &id, |s| service(s, "dev")["address"] == "localhost:4567").await;
    assert_eq!(service(&running, "dev")["state"], "running");
    assert_eq!(service(&running, "dev")["termId"], started.term_id);
    let logs = dispatch(&state, "run.logs", &json!({ "taskId": id, "service": "dev" })).await.unwrap();
    assert_eq!(logs["termId"], started.term_id);

    // Stopped on purpose is "stopped", not "failed".
    dispatch(&state, "run.stop", &json!({ "taskId": id, "service": "dev" })).await.unwrap();
    let stopped = run_state_until(&state, &id, |s| service(s, "dev")["termId"].is_null()).await;
    assert_eq!(
        (service(&stopped, "dev")["state"].as_str(), service(&stopped, "dev")["note"].clone()),
        (Some("stopped"), Value::Null)
    );

    // One that ends by itself with an error is "failed", and says with which code.
    state.terminals.run(&id, &project_dir, "exit 7", "run:dev", &[]).unwrap();
    let failed = run_state_until(&state, &id, |s| service(s, "dev")["state"] == "failed").await;
    assert!(service(&failed, "dev")["note"].as_str().unwrap().contains("code 7"));
    let no_logs =
        dispatch(&state, "run.logs", &json!({ "taskId": id, "service": "dev" })).await.unwrap_err().to_string();
    assert!(no_logs.contains("not running"), "{no_logs}");
}

#[tokio::test]
async fn docker_services_get_a_project_of_their_own_and_a_free_port() {
    let sandbox = Sandbox::new("docker");
    // Hold a port, so the task has to be given another one.
    let holder = TcpListener::bind("0.0.0.0:0").unwrap();
    let taken = holder.local_addr().unwrap().port();
    let tools = sandbox.0.join("tools");
    let state = AppState::new(sandbox.manager_with(hosts_with_docker(fake_docker(&tools, taken))));
    let project_dir = sandbox.project();
    std::fs::write(project_dir.join("compose.yaml"), "services:\n  db:\n    image: postgres:16\n").unwrap();
    let project = state.manager.add_project(&project_dir.to_string_lossy(), None).unwrap();
    let id = idle_task(&state, &project.id).await;

    let before = dispatch(&state, "run.state", &json!({ "taskId": id })).await.unwrap();
    assert_eq!(before["composeFile"], "compose.yaml");
    assert_eq!(before["composeProject"], format!("ae-project-{}", &id[..4]));
    assert_eq!(before["dockerReady"], true);
    assert_eq!(
        (
            service(&before, "db")["kind"].as_str(),
            service(&before, "db")["runs"].as_str(),
            service(&before, "db")["state"].as_str()
        ),
        (Some("docker"), Some("postgres:16"), Some("stopped"))
    );
    assert_eq!(service(&before, "worker")["runs"], "built from a Dockerfile");

    dispatch(&state, "run.start", &json!({ "taskId": id })).await.unwrap();
    let up = run_state_until(&state, &id, |s| service(s, "db")["state"] == "running").await;
    let address = service(&up, "db")["address"].as_str().unwrap().to_string();
    assert_ne!(address, format!("localhost:{taken}"), "the taken port was not used");
    let note = service(&up, "db")["note"].as_str().unwrap();
    assert!(note.contains(&format!("Port {taken} was taken")), "{note}");
    // A container that came up and died is "failed", with its exit code.
    assert_eq!(service(&up, "worker")["state"], "failed");
    assert!(service(&up, "worker")["note"].as_str().unwrap().contains("code 3"));

    let log = std::fs::read_to_string(tools.join("calls.log")).unwrap();
    let project_name = format!("ae-project-{}", &id[..4]);
    assert!(log.contains(&format!("compose -p {project_name} -f")) && log.contains("up -d --remove-orphans"), "{log}");

    dispatch(&state, "run.stop", &json!({ "taskId": id })).await.unwrap();
    let down = run_state_until(&state, &id, |s| service(s, "db")["state"] == "stopped").await;
    assert_eq!(service(&down, "db")["address"], Value::Null);
    assert_eq!(service(&down, "db")["note"], Value::Null, "a stopped service says nothing about ports");
    assert!(std::fs::read_to_string(tools.join("calls.log")).unwrap().contains(" down"));

    // Archiving the task takes its containers and volumes with it.
    dispatch(&state, "tasks.archive", &json!({ "taskId": id })).await.unwrap();
    assert!(!sandbox.data().join("run").join(&id).exists());
}

#[tokio::test]
async fn docker_that_is_not_there_is_said_plainly() {
    let sandbox = Sandbox::new("nodocker");
    let state = AppState::new(sandbox.manager());
    let project_dir = sandbox.project();
    std::fs::write(project_dir.join("docker-compose.yml"), "services:\n  db:\n    image: postgres:16\n").unwrap();
    let project = state.manager.add_project(&project_dir.to_string_lossy(), None).unwrap();
    let id = idle_task(&state, &project.id).await;

    let shown = dispatch(&state, "run.state", &json!({ "taskId": id })).await.unwrap();
    assert_eq!(shown["dockerReady"], false);
    assert!(shown["dockerHint"].as_str().unwrap().contains("not installed"));
    assert!(shown["problem"].as_str().unwrap().contains("compose file could not be read"));
    let refused = dispatch(&state, "run.start", &json!({ "taskId": id })).await.unwrap_err().to_string();
    assert!(refused.contains("not installed"), "{refused}");

    // A project with nothing to run says that too.
    std::fs::remove_file(project_dir.join("docker-compose.yml")).unwrap();
    let nothing = dispatch(&state, "run.start", &json!({ "taskId": id })).await.unwrap_err().to_string();
    assert!(nothing.contains("nothing to start"), "{nothing}");
}
