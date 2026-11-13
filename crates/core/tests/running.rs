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

/// A stand-in for `docker`. Its "running" containers are lines in `running-<compose project>.json`
/// next to it; what it answers about memory and disk is whatever the test wrote into
/// `ps.json`, `stats.json`, `df.json` and `df-v.json`.
fn fake_docker(dir: &Path, wanted_port: u16) -> PathBuf {
    fake_docker_with(
        dir,
        json!({ "name": "shop", "services": {
            "db": { "image": "postgres:16", "ports": [{ "target": 5432, "published": wanted_port.to_string(), "protocol": "tcp" }] },
            "worker": { "build": { "context": "." } }
        } }),
    )
}

/// The same stand-in with a compose file of the test's own. `__PROJECT__` in it
/// becomes the compose project Docker is asked about, as in Docker's own answer.
fn fake_docker_with(dir: &Path, config: Value) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("config.json"), config.to_string()).unwrap();
    fake_tool(
        dir,
        "docker",
        r#"here="$(dirname "$0")"
echo "$@" >> "$here/calls.log"
if [ "$1" = "info" ]; then echo "29.0.0"; exit 0; fi
project=""; file=""; prev=""; last=""
for word in "$@"; do
  [ "$prev" = "-p" ] && project="$word"
  [ "$prev" = "-f" ] && file="$word"
  prev="$word"; last="$word"
done
running="$here/running-$project.json"
off() { [ -f "$here/engine-off" ] && { echo "Cannot connect to the Docker daemon" >&2; exit 1; }; return 0; }
case "$*" in
  *" exec "*) echo "inside the container of $project"; exec cat ;;
  *" config --format json")
    # What of the task's environment reached Docker.
    env | grep '^SHOP_' | sort > "$here/env.log"
    sed "s/__PROJECT__/$project/g" "$here/config.json" ;;
  *" ps --all --format json") [ -f "$running" ] && cat "$running"; exit 0 ;;
  "ps --format json") [ -f "$here/ps.json" ] && cat "$here/ps.json"; exit 0 ;;
  "stats --no-stream --format json") [ -f "$here/stats.json" ] && cat "$here/stats.json"; exit 0 ;;
  "system df --format json") cat "$here/df.json" ;;
  "system df -v --format json") cat "$here/df-v.json" ;;
  "volume rm "*)
    if [ -f "$here/no-volume" ]; then echo "Error response from daemon: get $3: no such volume" >&2; exit 1; fi ;;
  *" up -d"*)
    if [ -f "$here/fail-up" ]; then cat "$here/fail-up" >&2; exit 1; fi
    # The task's own file is the one after -f; the port in it is the one "published".
    port=$(grep -o '"published": *"[0-9]*"' "$file" | head -1 | grep -o '[0-9]*')
    echo "Container db  Started"
    printf '{"Service":"db","State":"running","ExitCode":0,"Publishers":[{"PublishedPort":%s,"TargetPort":5432}]}\n' "${port:-0}" > "$running"
    printf '{"Service":"worker","State":"exited","ExitCode":3,"Publishers":[]}\n' >> "$running" ;;
  *" rm --stop --force --volumes "*) rm -f "$running" ;;
  *" down"*|*" stop "*) off; rm -f "$running" ;;
  *) echo "unexpected call: $*" >&2; exit 2 ;;
esac
"#,
    )
}

/// Every call the stand-in for `docker` got so far.
fn calls(tools: &Path) -> String {
    std::fs::read_to_string(tools.join("calls.log")).unwrap_or_default()
}

/// A port nothing listens on right now, so a service that asks for it keeps it.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0").unwrap().local_addr().unwrap().port()
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
        "address": null, "note": null, "termId": null, "web": true, "inProfile": true }])
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

/// A project with a `dev` script and the stand-in's two Docker services, and an idle task in it.
async fn shop_with_docker(sandbox: &Sandbox, config: Option<Value>) -> (AppState, PathBuf, String, String) {
    let tools = sandbox.0.join("tools");
    let docker = match config {
        Some(config) => fake_docker_with(&tools, config),
        None => fake_docker(&tools, free_port()),
    };
    let state = AppState::new(sandbox.manager_with(hosts_with_docker(docker)));
    let project_dir = sandbox.project();
    std::fs::write(project_dir.join("package.json"), r#"{"scripts":{"dev":"sleep 60"}}"#).unwrap();
    std::fs::write(project_dir.join("compose.yaml"), "services:\n  db:\n    image: postgres:16\n").unwrap();
    let project = state.manager.add_project(&project_dir.to_string_lossy(), None).unwrap();
    let id = idle_task(&state, &project.id).await;
    (state, tools, project.id, id)
}

/// The refusal a command gives, as the user reads it.
async fn refusal(state: &AppState, method: &str, params: Value) -> String {
    dispatch(state, method, &params).await.expect_err("the command is refused").to_string()
}

#[tokio::test]
async fn a_profile_names_what_starts_together_and_is_kept_for_the_project() {
    let sandbox = Sandbox::new("profiles");
    let (state, tools, project_id, id) = shop_with_docker(&sandbox, None).await;

    let before = dispatch(&state, "run.state", &json!({ "taskId": id })).await.unwrap();
    assert_eq!(before["profile"], "Full stack");
    assert_eq!(
        before["profiles"],
        json!([{ "name": "Full stack", "services": ["dev", "db", "worker"], "builtIn": true }])
    );

    for (params, says) in [
        (json!({ "taskId": id, "name": "  ", "services": ["db"] }), "Give the profile a name"),
        (json!({ "taskId": id, "name": "full stack", "services": ["db"] }), "starts everything"),
        (json!({ "taskId": id, "name": "Data", "services": [] }), "Tick at least one"),
        (json!({ "taskId": id, "name": "Data", "services": ["mail"] }), "mail is not something this project runs"),
    ] {
        let refused = refusal(&state, "run.saveProfile", params).await;
        assert!(refused.contains(says), "{refused}");
    }

    let kept = dispatch(&state, "run.saveProfile", &json!({ "taskId": id, "name": "Data only", "services": ["db"] }))
        .await
        .unwrap();
    assert_eq!(kept["profile"], "Data only", "a new profile is the chosen one");
    assert_eq!(kept["profiles"][1], json!({ "name": "Data only", "services": ["db"], "builtIn": false }));
    assert_eq!(
        (service(&kept, "dev")["inProfile"].clone(), service(&kept, "db")["inProfile"].clone()),
        (json!(false), json!(true))
    );
    let twice =
        refusal(&state, "run.saveProfile", json!({ "taskId": id, "name": "data ONLY", "services": ["db"] })).await;
    assert!(twice.contains("already"), "{twice}");

    // "Start all" starts what the profile names, and nothing else.
    dispatch(&state, "run.start", &json!({ "taskId": id })).await.unwrap();
    let up = run_state_until(&state, &id, |s| service(s, "db")["state"] == "running").await;
    assert_eq!(service(&up, "dev")["state"], "stopped");
    assert!(state.terminals.find(&id, "run:dev").is_none(), "the script outside the profile was left alone");
    let asked = calls(&tools);
    assert!(asked.lines().any(|line| line.ends_with(" up -d --remove-orphans db")), "{asked}");

    // Another task of the same project has the same profiles and the same choice.
    let other = idle_task(&state, &project_id).await;
    let elsewhere = dispatch(&state, "run.state", &json!({ "taskId": other })).await.unwrap();
    assert_eq!(elsewhere["profile"], "Data only");
    assert_eq!(elsewhere["profiles"].as_array().unwrap().len(), 2);

    // A profile is changed under its old name; it stays the chosen one.
    let renamed = dispatch(
        &state,
        "run.saveProfile",
        &json!({ "taskId": id, "name": "Data", "services": ["db", "worker", "db"], "was": "Data only" }),
    )
    .await
    .unwrap();
    assert_eq!(renamed["profiles"][1], json!({ "name": "Data", "services": ["db", "worker"], "builtIn": false }));
    assert_eq!((renamed["profiles"].as_array().unwrap().len(), renamed["profile"].as_str()), (2, Some("Data")));

    let full = dispatch(&state, "run.profile", &json!({ "taskId": id, "name": "Full stack" })).await.unwrap();
    assert_eq!(full["profile"], "Full stack");
    assert_eq!(service(&full, "dev")["inProfile"], true);
    let unknown = refusal(&state, "run.profile", json!({ "taskId": id, "name": "Nope" })).await;
    assert!(unknown.contains("no profile called Nope"), "{unknown}");

    dispatch(&state, "run.profile", &json!({ "taskId": id, "name": "Data" })).await.unwrap();
    let removed = dispatch(&state, "run.removeProfile", &json!({ "taskId": id, "name": "Data" })).await.unwrap();
    assert_eq!(removed["profiles"].as_array().unwrap().len(), 1);
    assert_eq!(removed["profile"], "Full stack", "a profile that is gone is not the chosen one");
}

#[tokio::test]
async fn the_tasks_environment_is_the_projects_with_its_own_values_and_no_secret_leaves_whole() {
    let sandbox = Sandbox::new("taskenv");
    let tools = sandbox.0.join("tools");
    let state = AppState::new(sandbox.manager_with(hosts_with_docker(fake_docker(&tools, free_port()))));
    let project_dir = git_project(&sandbox);
    std::fs::write(project_dir.join("compose.yaml"), "services:\n  db:\n    image: postgres:16\n").unwrap();
    std::fs::write(project_dir.join(".env.example"), "SHOP_PORT=8080\nSHOP_API_KEY=\nSHOP_MAIL=shop@example.com\n")
        .unwrap();
    std::fs::write(project_dir.join(".gitignore"), ".env\n").unwrap();
    git(&project_dir, &["add", "."]);
    git(&project_dir, &["-c", "user.name=Test", "-c", "user.email=test@example.com", "commit", "-q", "-m", "Env"]);
    // The real values are not in git, so the task's working copy never gets them as a file.
    std::fs::write(
        project_dir.join(".env"),
        "SHOP_PORT=3000\nSHOP_API_KEY=sk_live_0123456789abcdef\nSHOP_DB=postgres://shop:hunter2pass@localhost/shop\n",
    )
    .unwrap();
    let project = state.manager.add_project(&project_dir.to_string_lossy(), None).unwrap();
    let id = idle_task(&state, &project.id).await;
    let copy = state.manager.cwd(&id).unwrap();
    assert_ne!(copy, project_dir, "the task works in a copy of its own");

    let shown = dispatch(&state, "env.get", &json!({ "taskId": id })).await.unwrap();
    assert_eq!(shown["files"], json!([".env", ".env.example"]));
    assert_eq!(
        shown["values"],
        json!([
            { "name": "SHOP_PORT", "value": "3000", "secret": false, "own": false,
              "project": { "value": "3000", "from": ".env" } },
            { "name": "SHOP_API_KEY", "value": "••••••••cdef", "secret": true, "own": false,
              "project": { "value": "••••••••cdef", "from": ".env" } },
            { "name": "SHOP_DB", "value": "postgres://shop:••••••••@localhost/shop", "secret": true, "own": false,
              "project": { "value": "postgres://shop:••••••••@localhost/shop", "from": ".env" } },
            { "name": "SHOP_MAIL", "value": "shop@example.com", "secret": false, "own": false,
              "project": { "value": "shop@example.com", "from": ".env.example" } },
        ])
    );

    let set =
        dispatch(&state, "env.set", &json!({ "taskId": id, "name": "SHOP_PORT", "value": "3012" })).await.unwrap();
    assert_eq!(
        set["values"][0],
        json!({ "name": "SHOP_PORT", "value": "3012", "secret": false, "own": true,
                "project": { "value": "3000", "from": ".env" } })
    );
    let set = dispatch(&state, "env.set", &json!({ "taskId": id, "name": "SHOP_TOKEN", "value": "tok_abcdefgh" }))
        .await
        .unwrap();
    assert_eq!(set["own"], 2);
    assert_eq!(
        set["values"][4],
        json!({ "name": "SHOP_TOKEN", "value": "••••••••", "secret": true, "own": true, "project": null })
    );
    for answer in [&shown, &set] {
        let text = answer.to_string();
        assert!(!text.contains("hunter2pass") && !text.contains("sk_live") && !text.contains("tok_abc"), "{text}");
    }
    let refused = refusal(&state, "env.set", json!({ "taskId": id, "name": "2 many", "value": "x" })).await;
    assert!(refused.contains("letters, digits and underscores"), "{refused}");

    // The task's own values are the core's to keep: nothing is written where the agent reads.
    assert!(sandbox.data().join("run").join(&id).join("env.json").is_file());
    assert!(!copy.join(".env").exists());

    // What the Run tab starts is given all of it: Docker reads the compose file with these values.
    dispatch(&state, "run.start", &json!({ "taskId": id, "service": "db" })).await.unwrap();
    run_state_until(&state, &id, |s| service(s, "db")["state"] == "running").await;
    let given = std::fs::read_to_string(tools.join("env.log")).unwrap();
    assert_eq!(
        given.lines().collect::<Vec<_>>(),
        vec![
            "SHOP_API_KEY=sk_live_0123456789abcdef",
            "SHOP_DB=postgres://shop:hunter2pass@localhost/shop",
            "SHOP_MAIL=shop@example.com",
            "SHOP_PORT=3012",
            "SHOP_TOKEN=tok_abcdefgh"
        ]
    );

    // Docker's own words about a failure are shown, and may be handed to an agent: a secret in them is not.
    dispatch(&state, "run.stop", &json!({ "taskId": id })).await.unwrap();
    std::fs::write(tools.join("fail-up"), "Error response from daemon: service db: password hunter2pass refused\n")
        .unwrap();
    dispatch(&state, "run.start", &json!({ "taskId": id, "service": "db" })).await.unwrap();
    let failed = run_state_until(&state, &id, |s| {
        service(s, "db")["note"].as_str().is_some_and(|note| note.contains("refused"))
    })
    .await;
    assert_eq!(service(&failed, "db")["note"], "service db: password •••••••• refused");
    assert!(!failed.to_string().contains("hunter2pass"));

    // Taking the task's own value away gives the project's back.
    let back = dispatch(&state, "env.set", &json!({ "taskId": id, "name": "SHOP_PORT", "value": null })).await.unwrap();
    assert_eq!((back["values"][0]["value"].as_str(), back["values"][0]["own"].as_bool()), (Some("3000"), Some(false)));

    // The task's own values go when the task does.
    dispatch(&state, "tasks.archive", &json!({ "taskId": id })).await.unwrap();
    assert!(!sandbox.data().join("run").join(&id).exists());
}

#[tokio::test]
async fn packages_are_missing_then_installed_and_stale_once_the_lock_file_moves_on() {
    let sandbox = Sandbox::new("deps");
    let state = AppState::new(sandbox.manager());
    let project_dir = sandbox.project();
    let project = state.manager.add_project(&project_dir.to_string_lossy(), None).unwrap();
    let id = idle_task(&state, &project.id).await;
    let deps = |state: &Value| state["dependencies"].clone();

    let nothing = dispatch(&state, "run.state", &json!({ "taskId": id })).await.unwrap();
    assert_eq!(deps(&nothing), Value::Null, "no package.json, nothing to install");
    let refused = refusal(&state, "deps.install", json!({ "taskId": id })).await;
    assert!(refused.contains("no package.json"), "{refused}");

    std::fs::write(project_dir.join("package.json"), r#"{"scripts":{"build":"true"}}"#).unwrap();
    let none = dispatch(&state, "run.state", &json!({ "taskId": id })).await.unwrap();
    assert_eq!(
        deps(&none),
        json!({ "command": "npm install", "lockFile": null, "state": "none", "installedAt": null, "note": null,
                "termId": null })
    );

    std::fs::write(project_dir.join("package.json"), r#"{"dependencies":{"left":"1.0.0"}}"#).unwrap();
    std::fs::write(project_dir.join("package-lock.json"), "{}").unwrap();
    let missing = dispatch(&state, "run.state", &json!({ "taskId": id })).await.unwrap();
    assert_eq!(
        (deps(&missing)["state"].as_str(), deps(&missing)["command"].as_str(), deps(&missing)["lockFile"].as_str()),
        (Some("missing"), Some("npm ci"), Some("package-lock.json"))
    );

    // An install is a program in a terminal. This one stands in for the package manager:
    // it waits for the test, then leaves what npm leaves.
    let install =
        "while [ ! -f go ]; do sleep 0.05; done; mkdir -p node_modules && touch node_modules/.package-lock.json";
    let started = state.terminals.run(&id, &project_dir, install, "deps:install", &[]).unwrap();
    let installing = dispatch(&state, "run.state", &json!({ "taskId": id })).await.unwrap();
    assert_eq!(
        (deps(&installing)["state"].as_str(), deps(&installing)["termId"].as_u64()),
        (Some("installing"), Some(u64::from(started.term_id)))
    );
    // Asking for an install while one runs gives the same one.
    dispatch(&state, "deps.install", &json!({ "taskId": id })).await.unwrap();
    assert_eq!(state.terminals.list(&id).iter().filter(|info| info.purpose == "deps:install").count(), 1);

    std::fs::write(project_dir.join("go"), "").unwrap();
    let installed = run_state_until(&state, &id, |s| deps(s)["state"] == "installed").await;
    assert!(deps(&installed)["installedAt"].as_u64().is_some_and(|at| at > 1_700_000_000_000));

    // The lock file changed after the install: what is installed is no longer what it says.
    let lock = std::fs::File::options().write(true).open(project_dir.join("package-lock.json")).unwrap();
    lock.set_modified(std::time::SystemTime::now() + Duration::from_secs(60)).unwrap();
    let stale = dispatch(&state, "run.state", &json!({ "taskId": id })).await.unwrap();
    assert_eq!(deps(&stale)["state"], "stale");

    // An install that fails says how, in its own last words.
    state
        .terminals
        .run(&id, &project_dir, "echo 'npm error the registry said no'; echo; exit 3", "deps:install", &[])
        .unwrap();
    let failed =
        run_state_until(&state, &id, |s| deps(s)["note"].as_str().is_some_and(|note| note.contains("registry"))).await;
    assert_eq!(deps(&failed)["state"], "failed");
    assert_eq!(deps(&failed)["note"], "It stopped with code 3. Its last words: npm error the registry said no");
}

#[tokio::test]
async fn a_database_keeps_its_data_in_a_volume_of_the_task_and_is_reset_there() {
    let sandbox = Sandbox::new("database");
    let config = json!({ "name": "__PROJECT__", "services": {
        "db": { "image": "postgres:16",
                "ports": [{ "target": 5432, "published": free_port().to_string(), "protocol": "tcp" }],
                "volumes": [
                    { "type": "volume", "source": "dbdata", "target": "/var/lib/postgresql/data" },
                    { "type": "bind", "source": "/shop/seed", "target": "/docker-entrypoint-initdb.d" }
                ] },
        "legacy": { "image": "mysql:8", "volumes": [{ "type": "volume", "source": "kept", "target": "/var/lib/mysql" }] },
        "worker": { "build": { "context": "." } }
    }, "volumes": { "dbdata": { "name": "__PROJECT___dbdata" }, "kept": { "name": "shop-data" } } });
    let (state, tools, _, id) = shop_with_docker(&sandbox, Some(config)).await;
    let project = format!("ae-project-{}", &id[..4]);
    let volume = format!("{project}_dbdata");

    // Docker is asked to read the compose file as the task's own project: that is what names the volume after the task.
    let before = dispatch(&state, "run.state", &json!({ "taskId": id })).await.unwrap();
    assert!(
        calls(&tools)
            .lines()
            .any(|line| line.starts_with(&format!("compose -p {project} -f "))
                && line.ends_with(" config --format json"))
    );
    assert_eq!(
        before["databases"],
        json!([
            { "name": "db", "image": "postgres:16", "volumes": [{ "name": volume, "own": true }], "seed": true },
            { "name": "legacy", "image": "mysql:8", "volumes": [{ "name": "shop-data", "own": false }], "seed": false },
        ])
    );
    // A database's address is not a page to open.
    assert_eq!(
        (service(&before, "db")["web"].clone(), service(&before, "worker")["web"].clone()),
        (json!(false), json!(true))
    );

    // A shell needs a running container.
    let early = refusal(&state, "run.shell", json!({ "taskId": id, "service": "db" })).await;
    assert!(early.contains("not running"), "{early}");
    let local = refusal(&state, "run.shell", json!({ "taskId": id, "service": "dev" })).await;
    assert!(local.contains("runs on this machine"), "{local}");

    dispatch(&state, "run.start", &json!({ "taskId": id, "service": "db" })).await.unwrap();
    run_state_until(&state, &id, |s| service(s, "db")["state"] == "running").await;

    let shell = dispatch(&state, "run.shell", &json!({ "taskId": id, "service": "db" })).await.unwrap();
    let term_id = shell["termId"].as_u64().unwrap() as u32;
    let again = dispatch(&state, "run.shell", &json!({ "taskId": id, "service": "db" })).await.unwrap();
    assert_eq!(again["termId"], shell["termId"], "one shell per service");
    let mut printed = String::new();
    for _ in 0..200 {
        printed = String::from_utf8_lossy(&state.terminals.scrollback(term_id).unwrap().0).into_owned();
        if printed.contains("inside the container") {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(printed.contains(&format!("inside the container of {project}")), "{printed}");
    assert!(calls(&tools).lines().any(|line| line.contains(" exec db sh -c ")), "{}", calls(&tools));

    // Restarting a container makes it anew.
    dispatch(&state, "run.restart", &json!({ "taskId": id, "service": "db" })).await.unwrap();
    run_state_until(&state, &id, |s| {
        service(s, "db")["state"] == "running" && state.terminals.find(&id, "docker:up").is_none()
    })
    .await;
    assert!(calls(&tools).lines().any(|line| line.ends_with(" up -d --remove-orphans --force-recreate db")));

    // Reset: the container goes, then its volume, then the database is started again on an empty one.
    let asked_before = calls(&tools).lines().count();
    dispatch(&state, "db.reset", &json!({ "taskId": id, "service": "db" })).await.unwrap();
    run_state_until(&state, &id, |s| {
        service(s, "db")["state"] == "running" && state.terminals.find(&id, "docker:up").is_none()
    })
    .await;
    let asked = calls(&tools);
    let steps: Vec<&str> = asked
        .lines()
        .skip(asked_before)
        .filter(|line| line.contains(" rm ") || line.starts_with("volume ") || line.contains(" up -d"))
        .collect();
    assert_eq!(steps.len(), 3, "{steps:?}");
    assert!(steps[0].ends_with(" rm --stop --force --volumes db"), "{steps:?}");
    assert_eq!(steps[1], format!("volume rm {volume}"));
    assert!(steps[2].ends_with(" up -d --remove-orphans db"), "{steps:?}");
    assert!(state.terminals.find(&id, "shell:db").is_none(), "the shell went with the container");

    // A volume that was never made is already empty; a database that was not running is not started.
    dispatch(&state, "run.stop", &json!({ "taskId": id })).await.unwrap();
    std::fs::write(tools.join("no-volume"), "").unwrap();
    let asked_before = calls(&tools).lines().count();
    dispatch(&state, "db.reset", &json!({ "taskId": id, "service": "db" })).await.unwrap();
    assert!(!calls(&tools).lines().skip(asked_before).any(|line| line.contains(" up -d")));

    // A volume the compose file names itself may be another task's too: it is left alone.
    let shared = refusal(&state, "db.reset", json!({ "taskId": id, "service": "legacy" })).await;
    assert!(shared.contains("shop-data") && shared.contains("not reset from here"), "{shared}");
    let not_one = refusal(&state, "db.reset", json!({ "taskId": id, "service": "worker" })).await;
    assert!(not_one.contains("not a database"), "{not_one}");
}

#[tokio::test]
async fn what_runs_in_other_tasks_is_listed_with_what_it_uses_and_can_be_stopped() {
    let sandbox = Sandbox::new("others");
    let (state, tools, project_id, here) = shop_with_docker(&sandbox, None).await;
    let project_dir = sandbox.project();
    let idle = idle_task(&state, &project_id).await;
    // A task whose agent waits for an answer is in use.
    let mut events = state.manager.subscribe();
    let busy = state.manager.create(&project_id, "mock", "Fix the limit").await.unwrap().row.id;
    until(&mut events, &busy, |e| is(e, "permission_request")).await;

    let nothing = dispatch(&state, "resources.get", &json!({ "taskId": here, "disk": true })).await.unwrap();
    assert_eq!(nothing["others"], json!([]));
    assert_eq!((nothing["othersMemory"].clone(), nothing["leftovers"].clone()), (Value::Null, json!(0)));

    // The idle task runs a container and a command; the busy one runs a command.
    dispatch(&state, "run.start", &json!({ "taskId": idle, "service": "db" })).await.unwrap();
    run_state_until(&state, &idle, |s| service(s, "db")["state"] == "running").await;
    state.terminals.run(&idle, &project_dir, "sleep 60", "run:dev", &[]).unwrap();
    state.terminals.run(&busy, &project_dir, "sleep 60", "run:dev", &[]).unwrap();
    let project_of = |task: &str| format!("ae-project-{}", &task[..4]);
    let labels = |task: &str| format!("com.docker.compose.project={},com.docker.compose.service=db", project_of(task));
    std::fs::write(
        tools.join("ps.json"),
        format!(
            "{}\n{}\n",
            json!({ "ID": "aaa111", "Labels": labels(&idle), "State": "running" }),
            json!({ "ID": "zzz999", "Labels": "io.tags=sonar,code", "State": "running" })
        ),
    )
    .unwrap();
    std::fs::write(
        tools.join("stats.json"),
        format!(
            "{}\n{}\n",
            json!({ "ID": "aaa111", "MemUsage": "120MiB / 7.651GiB" }),
            json!({ "ID": "zzz999", "MemUsage": "2.376GiB / 7.651GiB" })
        ),
    )
    .unwrap();
    std::fs::write(
        tools.join("df.json"),
        format!(
            "{}\n{}\n",
            json!({ "Type": "Images", "Size": "2.576GB" }),
            json!({ "Type": "Local Volumes", "Size": "758.4MB" })
        ),
    )
    .unwrap();
    let volume = |name: String, labels: String, size: &str| json!({ "Name": name, "Labels": labels, "Size": size });
    std::fs::write(
        tools.join("df-v.json"),
        json!({ "Images": [], "Containers": [], "Volumes": [
            volume(format!("{}_dbdata", project_of(&idle)), format!("com.docker.compose.project={}", project_of(&idle)), "84.2MB"),
            volume("sonar_data".into(), "com.docker.compose.project=sonar".into(), "600MB"),
            volume("abc".into(), "com.docker.volume.anonymous=".into(), "1kB")
        ] })
        .to_string(),
    )
    .unwrap();

    let seen = dispatch(&state, "resources.get", &json!({ "taskId": here, "disk": true })).await.unwrap();
    let others = seen["others"].as_array().unwrap();
    assert_eq!(others.len(), 2, "{seen:#}");
    // The heavier one first.
    let (first, second) = (&others[0], &others[1]);
    assert_eq!((first["taskId"].as_str(), first["title"].as_str()), (Some(idle.as_str()), Some("What is here?")));
    assert_eq!(first["inUse"], false);
    assert!(first["idleSince"].as_u64().is_some_and(|at| at > 1_700_000_000_000), "{first:#}");
    assert_eq!(first["running"], json!(["dev", "db"]));
    // The container's 120 MiB as Docker said, and whatever the command's processes hold.
    let container = 120 * 1024 * 1024;
    assert!(first["memory"].as_u64().unwrap() > container, "{first:#}");
    assert_eq!((second["taskId"].as_str(), second["inUse"].as_bool()), (Some(busy.as_str()), Some(true)));
    assert_eq!(second["idleSince"], Value::Null);
    assert!(second["memory"].as_u64().unwrap() > 0, "{second:#}");
    assert_eq!(
        seen["othersMemory"].as_u64().unwrap(),
        first["memory"].as_u64().unwrap() + second["memory"].as_u64().unwrap()
    );
    // Docker's own totals: every image, and the volumes of this core's tasks alone.
    assert_eq!(seen["disk"], json!({ "images": 2_576_000_000u64, "volumes": 84_200_000, "tasks": 1 }));
    assert_eq!((seen["memory"].clone(), seen["volumes"].clone()), (json!({}), json!({})), "this task runs nothing");

    // Seen from the task that runs them, the same numbers are its own.
    let own = dispatch(&state, "resources.get", &json!({ "taskId": idle, "disk": true })).await.unwrap();
    assert_eq!(own["memory"]["db"], container);
    assert!(own["memory"]["dev"].as_u64().unwrap() > 0);
    assert_eq!(own["volumes"], json!({ format!("{}_dbdata", project_of(&idle)): 84_200_000 }));
    assert_eq!(own["others"].as_array().unwrap().len(), 1);
    // Without `disk`, Docker is not asked about its disk.
    let quick = dispatch(&state, "resources.get", &json!({ "taskId": idle })).await.unwrap();
    assert!(quick.get("disk").is_none() && quick.get("volumes").is_none());

    // Cleaning up stops what idle tasks run and leaves the task that is in use alone.
    let cleaned = dispatch(&state, "resources.cleanUp", &json!({ "taskId": here })).await.unwrap();
    assert_eq!(cleaned, json!({ "stopped": 1, "removed": 0 }));
    for _ in 0..100 {
        if state.terminals.find(&idle, "run:dev").is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(state.terminals.find(&idle, "run:dev").is_none());
    assert!(state.terminals.find(&busy, "run:dev").is_some());
    assert!(
        calls(&tools)
            .lines()
            .any(|line| line.starts_with(&format!("compose -p {} -f ", project_of(&idle))) && line.ends_with(" down"))
    );

    // One task is stopped from another with the ordinary stop.
    dispatch(&state, "run.stop", &json!({ "taskId": busy })).await.unwrap();
    for _ in 0..100 {
        if state.terminals.find(&busy, "run:dev").is_none() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    std::fs::remove_file(tools.join("ps.json")).unwrap();
    let after = dispatch(&state, "resources.get", &json!({ "taskId": here })).await.unwrap();
    assert_eq!(after["others"], json!([]));
}

#[tokio::test]
async fn what_a_finished_task_left_in_docker_is_removed_later() {
    let sandbox = Sandbox::new("leftovers");
    let (state, tools, project_id, here) = shop_with_docker(&sandbox, None).await;
    let gone = idle_task(&state, &project_id).await;
    dispatch(&state, "run.start", &json!({ "taskId": gone, "service": "db" })).await.unwrap();
    run_state_until(&state, &gone, |s| service(s, "db")["state"] == "running").await;
    dispatch(&state, "env.set", &json!({ "taskId": gone, "name": "SHOP_TOKEN", "value": "tok_abcdefgh" }))
        .await
        .unwrap();

    // Docker cannot be asked while the task is archived: its containers and volumes stay.
    std::fs::write(tools.join("engine-off"), "").unwrap();
    dispatch(&state, "tasks.archive", &json!({ "taskId": gone })).await.unwrap();
    let folder = sandbox.data().join("run").join(&gone);
    assert!(folder.join("compose.json").is_file(), "what it takes to remove them later is kept");
    assert!(!folder.join("env.json").exists(), "the task's own values are not");
    let seen = dispatch(&state, "resources.get", &json!({ "taskId": here })).await.unwrap();
    assert_eq!(seen["leftovers"], 1);

    // Still not there: nothing is forgotten, and the user is told.
    let refused = refusal(&state, "resources.cleanUp", json!({ "taskId": here })).await;
    assert!(refused.contains("could not be removed"), "{refused}");
    assert!(folder.is_dir());

    std::fs::remove_file(tools.join("engine-off")).unwrap();
    let cleaned = dispatch(&state, "resources.cleanUp", &json!({ "taskId": here })).await.unwrap();
    assert_eq!(cleaned, json!({ "stopped": 0, "removed": 1 }));
    assert!(!folder.exists());
    let project = format!("ae-project-{}", &gone[..4]);
    assert!(calls(&tools).lines().any(|line| line.starts_with(&format!("compose -p {project} -f "))
        && line.ends_with(" down --volumes --remove-orphans")));
}

#[tokio::test]
async fn a_script_waits_for_the_install_and_is_started_only_when_it_ended_well() {
    let sandbox = Sandbox::new("install-first");
    let state = AppState::new(sandbox.manager());
    let project_dir = sandbox.project();
    std::fs::write(
        project_dir.join("package.json"),
        r#"{"scripts":{"dev":"sleep 60","api":"sleep 60","worker":"sleep 60"},"dependencies":{"left":"1.0.0"}}"#,
    )
    .unwrap();
    std::fs::write(project_dir.join("package-lock.json"), "{}").unwrap();
    let project = state.manager.add_project(&project_dir.to_string_lossy(), None).unwrap();
    let id = idle_task(&state, &project.id).await;
    // A stand-in for the package manager, held until the test lets it go; then it ends as told.
    let install = |gate: &str, then: &str| {
        let line = format!("while [ ! -f {gate} ]; do sleep 0.05; done; {then}");
        state.terminals.run(&id, &project_dir, &line, "deps:install", &[]).unwrap()
    };
    let leaves_packages = "mkdir -p node_modules && touch node_modules/.package-lock.json";
    let state_of = |shown: &Value, name: &str| service(shown, name)["state"].as_str().unwrap_or_default().to_string();

    // While the packages are being installed, a script that is asked for waits.
    install("go-1", leaves_packages);
    let waits = dispatch(&state, "run.start", &json!({ "taskId": id, "service": "dev" })).await.unwrap();
    assert_eq!(state_of(&waits, "dev"), "starting");
    assert_eq!(service(&waits, "dev")["note"], "Waiting for npm ci to finish.");
    assert!(state.terminals.find(&id, "run:dev").is_none());
    assert_eq!(state.terminals.list(&id).iter().filter(|info| info.purpose == "deps:install").count(), 1);
    // The install ends well: the script is started. (How it fares depends on npm being on this machine.)
    let mut told = state.manager.subscribe();
    std::fs::write(project_dir.join("go-1"), "").unwrap();
    let after = run_state_until(&state, &id, |s| !matches!(state_of(s, "dev").as_str(), "starting")).await;
    assert_ne!(state_of(&after, "dev"), "stopped", "the script was started after the install");
    assert_eq!(after["dependencies"]["state"], "installed");
    // Nobody asked for that start just now, so the interfaces are told of it.
    let announced = async {
        loop {
            let event = told.recv().await.expect("the events go on");
            if event["event"] == "run.changed" && event["data"]["taskId"] == id.as_str() {
                break;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), announced).await.expect("the start after the install is announced");
    dispatch(&state, "run.stop", &json!({ "taskId": id })).await.unwrap();

    // The install fails: nothing is started after it.
    std::fs::remove_dir_all(project_dir.join("node_modules")).unwrap();
    install("go-2", "exit 3");
    let waits = dispatch(&state, "run.start", &json!({ "taskId": id, "service": "api" })).await.unwrap();
    assert_eq!(state_of(&waits, "api"), "starting");
    std::fs::write(project_dir.join("go-2"), "").unwrap();
    let failed =
        run_state_until(&state, &id, |s| s["dependencies"]["state"] == "failed" && state_of(s, "api") != "starting")
            .await;
    assert_eq!(state_of(&failed, "api"), "stopped");
    assert!(state.terminals.last_exit(&id, "run:api").is_none(), "it never ran");

    // A script that is stopped while it waits is not started when the install ends.
    install("go-3", leaves_packages);
    let waits = dispatch(&state, "run.start", &json!({ "taskId": id, "service": "worker" })).await.unwrap();
    assert_eq!(state_of(&waits, "worker"), "starting");
    let stopped = dispatch(&state, "run.stop", &json!({ "taskId": id, "service": "worker" })).await.unwrap();
    assert_eq!(state_of(&stopped, "worker"), "stopped");
    std::fs::write(project_dir.join("go-3"), "").unwrap();
    let done = run_state_until(&state, &id, |s| s["dependencies"]["state"] == "installed").await;
    assert_eq!(state_of(&done, "worker"), "stopped");
    assert!(
        state.terminals.find(&id, "run:worker").is_none() && state.terminals.last_exit(&id, "run:worker").is_none()
    );
}
