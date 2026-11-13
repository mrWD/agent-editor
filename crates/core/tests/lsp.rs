//! Language servers: starting one for a task, carrying messages both ways, and stopping it with nothing left behind.
//!
//! Only the practice language server runs here (the practice agent started with
//! `--lsp`). No real language server is started, and none is installed.
#![cfg(unix)]

mod common;

use ae_core::lsp::{Lsp, ServerDef, known_servers, practice_server};
use ae_core::server::{AppState, dispatch};
use common::*;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;
use tokio::sync::broadcast::Receiver;

/// A word on the command line of everything one test's server starts, to find its processes by.
fn marker() -> String {
    static COUNTER: AtomicU32 = AtomicU32::new(0);
    format!("ae-practice-lsp-{}-{}", std::process::id(), COUNTER.fetch_add(1, Ordering::SeqCst))
}

/// How many processes carry the marker.
fn running(marker: &str) -> usize {
    let found = std::process::Command::new("pgrep").args(["-f", marker]).output().expect("pgrep runs");
    String::from_utf8_lossy(&found.stdout).split_whitespace().count()
}

async fn none_left(marker: &str) {
    for _ in 0..150 {
        if running(marker) == 0 {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("{} processes of the language server are still running", running(marker));
}

/// The practice server behind a launcher that also starts a helper, the way a
/// real server starts a compiler next to itself. The helper keeps the server's
/// output open and would outlive it, if only the server itself were stopped.
fn practice_with_helper(marker: &str) -> ServerDef {
    let script = format!("sh -c 'sleep 30; true' \"$0\" & exec '{}' --lsp \"$0\"", mock_agent().display());
    let mut def = practice_server(Path::new("/bin/sh"));
    def.launches[0].args = vec!["-c".into(), script, marker.into()];
    def
}

/// A core's state whose language servers are the given ones, and a task with a file of the practice language.
async fn task_with(sandbox: &Sandbox, table: Vec<ServerDef>, idle: Duration) -> (AppState, String, String) {
    let manager = sandbox.manager();
    let mut state = AppState::new(manager.clone());
    state.lsp = Arc::new(Lsp::with(manager.clone(), table, idle));
    std::fs::write(sandbox.project().join("notes.practice"), "one\n").unwrap();
    let project = manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock", "What does the limiter do?").await.unwrap();
    until(&mut events, &task.row.id, |e| state_is(e, "idle")).await;
    (state, task.row.id, project.id)
}

/// Waits for an event of one kind that satisfies `wanted`.
async fn event(events: &mut Receiver<Value>, name: &str, wanted: impl Fn(&Value) -> bool) -> Value {
    let wait = async {
        loop {
            let message = events.recv().await.expect("event stream stays open");
            if message["event"] == name && wanted(&message["data"]) {
                return message["data"].clone();
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(20), wait).await.unwrap_or_else(|_| panic!("no {name} event in time"))
}

/// Sends one message to the practice server of a task, as a window would.
async fn send(state: &AppState, task_id: &str, window: u64, message: Value) -> anyhow::Result<Value> {
    let params = json!({ "taskId": task_id, "language": "practice", "window": window, "message": message });
    dispatch(state, "lsp.send", &params).await
}

fn opened(uri: &str, text: &str) -> Value {
    json!({
        "method": "textDocument/didOpen",
        "params": { "textDocument": { "uri": uri, "languageId": "practice", "version": 1, "text": text } }
    })
}

fn changed(uri: &str, text: &str) -> Value {
    json!({
        "method": "textDocument/didChange",
        "params": { "textDocument": { "uri": uri, "version": 7 }, "contentChanges": [{ "text": text }] }
    })
}

fn closed(uri: &str) -> Value {
    json!({ "method": "textDocument/didClose", "params": { "textDocument": { "uri": uri } } })
}

fn hover(id: &str, uri: &str, line: u64, character: u64) -> Value {
    json!({
        "id": id,
        "method": "textDocument/hover",
        "params": { "textDocument": { "uri": uri }, "position": { "line": line, "character": character } }
    })
}

/// The answer to the request with this id.
async fn answer(events: &mut Receiver<Value>, id: &str) -> Value {
    event(events, "lsp.message", |data| data["message"]["id"] == id).await["message"].clone()
}

/// What the server says is wrong in a file, the next time it says so.
async fn diagnostics(events: &mut Receiver<Value>, uri: &str) -> Vec<Value> {
    let said = event(events, "lsp.message", |data| {
        data["message"]["method"] == "textDocument/publishDiagnostics" && data["message"]["params"]["uri"] == uri
    })
    .await;
    said["message"]["params"]["diagnostics"].as_array().unwrap().clone()
}

#[tokio::test]
async fn a_server_starts_for_a_file_answers_and_is_stopped_with_its_task() {
    let sandbox = Sandbox::new("lsp-start");
    let marker = marker();
    let (state, task_id, _) = task_with(&sandbox, vec![practice_with_helper(&marker)], Duration::from_secs(60)).await;
    let mut events = state.manager.subscribe();

    let started = dispatch(&state, "lsp.start", &json!({ "taskId": task_id, "path": "notes.practice" })).await.unwrap();
    assert_eq!(started["state"], "running", "{started}");
    assert_eq!(
        (&started["language"], &started["name"], &started["languageId"]),
        (&json!("practice"), &json!("Practice"), &json!("practice"))
    );
    assert_eq!(started["server"]["name"], "Practice language server");
    assert_eq!(started["capabilities"]["documentFormattingProvider"], true);
    assert_eq!(started["formatOnSave"], true, "files are formatted on save unless the user says no");
    // The server works in the task's folder.
    let root = started["root"].as_str().unwrap().to_string();
    assert_eq!(root, ae_core::lsp::file_uri(&state.manager.cwd(&task_id).unwrap()));
    assert_eq!(running(&marker), 2, "the server and the helper it started");
    assert_eq!(state.lsp.running(Some(&task_id)), 1);

    // The server asked its client for settings right after the start; the core answered for every window.
    let configured = event(&mut events, "lsp.message", |data| data["message"]["method"] == "practice/configured").await;
    assert_eq!(configured["message"]["params"]["answer"], json!([null]));
    assert_eq!((&configured["taskId"], &configured["language"]), (&json!(task_id), &json!("practice")));

    // A file is opened: the server says what is wrong in it.
    let uri = format!("{root}/notes.practice");
    send(&state, &task_id, 1, opened(&uri, "limits\n  FIXME here\n")).await.unwrap();
    let found = diagnostics(&mut events, &uri).await;
    assert_eq!(found.len(), 1);
    assert_eq!(found[0]["range"]["start"], json!({ "line": 1, "character": 2 }));

    // A request goes to the server as it is; its answer comes back as an event with the same id.
    send(&state, &task_id, 1, hover("w1:1", &uri, 0, 2)).await.unwrap();
    let hovered = answer(&mut events, "w1:1").await;
    assert!(hovered["result"]["contents"]["value"].as_str().unwrap().contains("practice language server"), "{hovered}");

    // The same file a second time is the same server, not a second one.
    let again = dispatch(&state, "lsp.start", &json!({ "taskId": task_id, "path": "other.practice" })).await.unwrap();
    assert_eq!(again["run"], started["run"]);
    assert_eq!(running(&marker), 2);

    // Archiving the task stops its server and everything the server started.
    dispatch(&state, "tasks.archive", &json!({ "taskId": task_id })).await.unwrap();
    let stopped = event(&mut events, "lsp.stopped", |data| data["taskId"] == task_id.as_str()).await;
    assert_eq!((&stopped["reason"], &stopped["language"]), (&json!("task"), &json!("practice")));
    none_left(&marker).await;
    assert_eq!(state.lsp.running(None), 0);
    let refused = send(&state, &task_id, 1, hover("w1:2", &uri, 0, 2)).await.unwrap_err();
    assert_eq!(refused.to_string(), "The language server is not running");
}

#[tokio::test]
async fn a_server_that_is_not_installed_is_reported_with_how_to_install_it() {
    let sandbox = Sandbox::new("lsp-missing");
    // The known servers under names nothing on this machine has, so no real one is ever started here.
    let mut table = known_servers();
    for def in &mut table {
        for launch in &mut def.launches {
            launch.program = format!("ae-test-no-such-{}", launch.program);
        }
    }
    let (state, task_id, _) = task_with(&sandbox, table, Duration::from_secs(60)).await;
    let start = |path: &'static str| {
        let (state, task_id) = (state.clone(), task_id.clone());
        async move { dispatch(&state, "lsp.start", &json!({ "taskId": task_id, "path": path })).await.unwrap() }
    };

    let missing = start("src/app.ts").await;
    assert_eq!(
        missing,
        json!({
            "state": "missing",
            "language": "typescript",
            "name": "TypeScript and JavaScript",
            "command": "ae-test-no-such-typescript-language-server",
            "install": "npm install -g typescript-language-server typescript"
        })
    );
    assert_eq!(start("main.go").await["install"], "go install golang.org/x/tools/gopls@latest");
    assert_eq!(start("src/lib.rs").await["install"], "rustup component add rust-analyzer");
    assert_eq!(start("tool.PY").await["language"], "python", "the ending counts whatever its case");
    assert_eq!(start("package.json").await["install"], "npm install -g vscode-langservers-extracted");

    // A file of no known language is nobody's: the editor works as it always did.
    assert_eq!(start("README.md").await, json!({ "state": "none" }));
    assert_eq!(start("Makefile").await, json!({ "state": "none" }));
    assert_eq!(start("notes.practice").await, json!({ "state": "none" }), "this table has no practice server");
    assert_eq!(state.lsp.running(None), 0, "nothing was started");

    // There is nothing to send a message to.
    let refused = send(&state, &task_id, 1, json!({ "method": "initialized", "params": {} })).await.unwrap_err();
    assert_eq!(refused.to_string(), "The language server is not running");
    // A task that does not exist has no server either.
    assert!(dispatch(&state, "lsp.start", &json!({ "taskId": "nobody", "path": "a.ts" })).await.is_err());
}

/// A server in the task's own `node_modules/.bin` is the one the project wants: it is found before PATH is looked at.
#[tokio::test]
async fn a_server_in_the_tasks_node_modules_is_used_and_core_stop_ends_it() {
    let sandbox = Sandbox::new("lsp-node-modules");
    let marker = marker();
    // The real table, as a running core has it. The "TypeScript server" of this
    // project is a script that starts the practice server and a helper.
    let script = format!("sh -c 'sleep 30; true' {marker} &\nexec '{}' --lsp {marker}", mock_agent().display());
    fake_tool(&sandbox.project().join("node_modules").join(".bin"), "typescript-language-server", &script);
    std::fs::write(sandbox.project().join("app.ts"), "const limits = 1; // FIXME\n").unwrap();

    let options = ae_core::Options { port: ae_core::Port::Free { prefer: None }, ..Default::default() };
    let core = ae_core::startup::serve(sandbox.manager(), &options).await.unwrap();
    let state = core.state().clone();
    let manager = state.manager.clone();
    let project = manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock", "What does the limiter do?").await.unwrap();
    let task_id = task.row.id.clone();
    until(&mut events, &task_id, |e| state_is(e, "idle")).await;

    let started = dispatch(&state, "lsp.start", &json!({ "taskId": task_id, "path": "app.ts" })).await.unwrap();
    assert_eq!(started["state"], "running", "{started}");
    assert_eq!((&started["language"], &started["languageId"]), (&json!("typescript"), &json!("typescript")));
    assert_eq!(started["server"]["name"], "Practice language server");
    assert_eq!(running(&marker), 2, "the server and its helper");

    // Quitting the app ends in `Core::stop`: no language server may outlive it.
    core.stop().await.unwrap();
    none_left(&marker).await;
}

#[tokio::test]
async fn two_windows_share_a_server_and_a_file_is_closed_when_the_last_one_lets_go() {
    let sandbox = Sandbox::new("lsp-windows");
    let marker = marker();
    let (state, task_id, _) =
        task_with(&sandbox, vec![practice_with_helper(&marker)], Duration::from_millis(300)).await;
    let mut events = state.manager.subscribe();
    let started = dispatch(&state, "lsp.start", &json!({ "taskId": task_id, "path": "notes.practice" })).await.unwrap();
    let uri = format!("{}/notes.practice", started["root"].as_str().unwrap());

    // Two windows open the same file. The second "open" reaches the server as new text, not as a second opening.
    let (first, second) = (state.lsp.window(), state.lsp.window());
    assert_ne!(first.id(), second.id());
    send(&state, &task_id, first.id(), opened(&uri, "limits\n")).await.unwrap();
    assert!(diagnostics(&mut events, &uri).await.is_empty());
    send(&state, &task_id, second.id(), opened(&uri, "limits FIXME\n")).await.unwrap();
    assert_eq!(diagnostics(&mut events, &uri).await.len(), 1);

    // The first window closes the file; the second still has it, so the server keeps it and still answers.
    send(&state, &task_id, first.id(), closed(&uri)).await.unwrap();
    send(&state, &task_id, second.id(), changed(&uri, "x limits\n")).await.unwrap();
    assert!(diagnostics(&mut events, &uri).await.is_empty());
    send(&state, &task_id, second.id(), hover("w2:1", &uri, 0, 3)).await.unwrap();
    assert!(
        answer(&mut events, "w2:1").await["result"]["contents"].is_object(),
        "the file is still open in the server"
    );

    // While a file is open the server stays, however long nothing is said.
    tokio::time::sleep(Duration::from_millis(700)).await;
    assert_eq!(state.lsp.running(Some(&task_id)), 1);

    // The second window goes away without a word, as a closed tab does. Its file is closed for it,
    drop(second);
    assert!(diagnostics(&mut events, &uri).await.is_empty(), "the server was told the file is closed");
    // and after a while with no open file the server is stopped.
    let stopped = event(&mut events, "lsp.stopped", |data| data["taskId"] == task_id.as_str()).await;
    assert_eq!(stopped["reason"], "idle");
    none_left(&marker).await;
    drop(first);

    // The next file that is opened starts it again.
    let again = dispatch(&state, "lsp.start", &json!({ "taskId": task_id, "path": "notes.practice" })).await.unwrap();
    assert_eq!(again["state"], "running");
    assert_ne!(again["run"], started["run"], "a new start is told apart from the old one");
    state.lsp.stop_all().await;
    none_left(&marker).await;
    // A core that is stopping starts nothing.
    assert!(dispatch(&state, "lsp.start", &json!({ "taskId": task_id, "path": "notes.practice" })).await.is_err());
}

#[tokio::test]
async fn a_server_that_crashes_is_reported_once_and_starts_again_with_the_next_file() {
    let sandbox = Sandbox::new("lsp-crash");
    let marker = marker();
    let (state, task_id, _) = task_with(&sandbox, vec![practice_with_helper(&marker)], Duration::from_secs(60)).await;
    let mut events = state.manager.subscribe();
    let started = dispatch(&state, "lsp.start", &json!({ "taskId": task_id, "path": "notes.practice" })).await.unwrap();
    let uri = format!("{}/notes.practice", started["root"].as_str().unwrap());
    send(&state, &task_id, 1, opened(&uri, "fine\n")).await.unwrap();
    diagnostics(&mut events, &uri).await;

    // The practice server ends at once when it reads this word, as a crash would.
    send(&state, &task_id, 1, changed(&uri, "CRASH-NOW\n")).await.unwrap();
    let stopped = event(&mut events, "lsp.stopped", |data| data["taskId"] == task_id.as_str()).await;
    assert_eq!(stopped["reason"], "crashed");
    assert_eq!(stopped["name"], "Practice");
    assert!(
        stopped["message"].as_str().unwrap().contains("asked to crash"),
        "what it wrote last is passed on: {stopped}"
    );
    // The helper it had started is not left behind, although nobody asked for the server to stop.
    none_left(&marker).await;
    assert_eq!(state.lsp.running(None), 0);
    assert!(send(&state, &task_id, 1, changed(&uri, "fine\n")).await.is_err());

    // Nothing restarts it by itself.
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert_eq!(running(&marker), 0);
    // The next opened file does.
    let again = dispatch(&state, "lsp.start", &json!({ "taskId": task_id, "path": "notes.practice" })).await.unwrap();
    assert_eq!(again["state"], "running");
    assert_ne!(again["run"], started["run"]);
    // It knows nothing of the files the old one had open: a change to one of them is not passed on.
    send(&state, &task_id, 1, changed(&uri, "FIXME\n")).await.unwrap();
    send(&state, &task_id, 1, opened(&uri, "FIXME\nFIXME\n")).await.unwrap();
    assert_eq!(
        diagnostics(&mut events, &uri).await.len(),
        2,
        "the first thing the new server hears of the file is its opening"
    );
    state.lsp.stop_all().await;
    none_left(&marker).await;
}

#[tokio::test]
async fn a_server_that_will_not_start_says_why_and_leaves_nothing_running() {
    let sandbox = Sandbox::new("lsp-failed");
    let marker = marker();
    // It writes a line, leaves a helper behind and ends before it has said hello.
    let broken = fake_tool(
        &sandbox.0.join("tools"),
        "broken-server",
        &format!("sh -c 'sleep 30; true' {marker} &\necho 'broken-server: no such toolchain' >&2\nexit 1"),
    );
    let mut def = practice_server(&broken);
    def.launches[0].args.clear();
    let (state, task_id, _) = task_with(&sandbox, vec![def], Duration::from_secs(60)).await;
    let mut events = state.manager.subscribe();

    let failed = dispatch(&state, "lsp.start", &json!({ "taskId": task_id, "path": "notes.practice" })).await.unwrap();
    assert_eq!(failed["state"], "failed", "{failed}");
    assert!(failed["message"].as_str().unwrap().contains("broken-server: no such toolchain"), "{failed}");
    none_left(&marker).await;
    assert_eq!(state.lsp.running(None), 0);
    // A server that never started is not announced as stopped.
    tokio::time::sleep(Duration::from_millis(100)).await;
    while let Ok(message) = events.try_recv() {
        assert_ne!(message["event"], "lsp.stopped", "{message}");
    }
}

#[tokio::test]
async fn formatting_on_save_is_a_choice_kept_for_the_project() {
    let sandbox = Sandbox::new("lsp-format");
    let marker = marker();
    let (state, task_id, project_id) =
        task_with(&sandbox, vec![practice_with_helper(&marker)], Duration::from_secs(60)).await;
    let mut events = state.manager.subscribe();
    let start = json!({ "taskId": task_id, "path": "notes.practice" });
    assert_eq!(dispatch(&state, "lsp.start", &start).await.unwrap()["formatOnSave"], true);

    let off = dispatch(&state, "lsp.setFormatOnSave", &json!({ "projectId": project_id, "on": false })).await.unwrap();
    assert_eq!(off, json!({ "on": false }));
    // Every window hears of it.
    let told = event(&mut events, "lsp.settings", |_| true).await;
    assert_eq!(told, json!({ "projectId": project_id, "formatOnSave": false }));
    assert_eq!(dispatch(&state, "lsp.start", &start).await.unwrap()["formatOnSave"], false);
    // The choice is in the core's settings: another look at the same data finds it.
    assert!(!Lsp::new(sandbox.manager()).format_on_save(&project_id));

    dispatch(&state, "lsp.setFormatOnSave", &json!({ "projectId": project_id, "on": true })).await.unwrap();
    assert_eq!(dispatch(&state, "lsp.start", &start).await.unwrap()["formatOnSave"], true);
    // A project that does not exist has no choice to keep.
    assert!(dispatch(&state, "lsp.setFormatOnSave", &json!({ "projectId": "nobody", "on": false })).await.is_err());
    assert!(dispatch(&state, "lsp.setFormatOnSave", &json!({ "projectId": project_id })).await.is_err());
    state.lsp.stop_all().await;
    none_left(&marker).await;
}
