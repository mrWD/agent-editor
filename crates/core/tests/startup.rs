//! Starting the whole core the way `ae-core` and the desktop app do, and stopping it again.
//!
//! Only the practice agent runs here, and no test listens on the port a real core uses.

mod common;

use ae_core::server::{AppState, dispatch};
use ae_core::{Core, Desktop, Options, Port};
use common::*;
use serde_json::{Value, json};
use std::path::PathBuf;
use std::sync::Arc;

/// Options for a core on any free port, so a test never meets a real core or another test.
fn on_a_free_port() -> Options {
    Options { port: Port::Free { prefer: None }, ..Options::default() }
}

/// What a running core answered: the text, and whether the browser was told not to keep it.
struct Page {
    text: String,
    keep: Option<String>,
}

/// Asks a running core for a page. `Err` when nothing answers.
async fn get(core_url: &str, path: &str) -> Result<Page, String> {
    let url = format!("{core_url}{path}");
    tokio::task::spawn_blocking(move || {
        let mut answer = ureq::get(&url).call().map_err(|error| error.to_string())?;
        let keep = answer.headers().get("cache-control").and_then(|value| value.to_str().ok()).map(str::to_string);
        let text = answer.body_mut().read_to_string().map_err(|error| error.to_string())?;
        Ok(Page { text, keep })
    })
    .await
    .unwrap()
}

async fn health(core: &Core) -> Value {
    serde_json::from_str(&get(&core.url(), "/api/health").await.expect("the core answers").text).unwrap()
}

#[tokio::test]
async fn the_core_starts_on_a_free_port_serves_the_interface_and_stops() {
    let sandbox = Sandbox::new("start");
    // A stand-in for the built interface: a page and one file the page would load.
    let ui = sandbox.0.join("ui");
    std::fs::create_dir_all(ui.join("assets")).unwrap();
    std::fs::write(ui.join("index.html"), "<title>Agent Editor</title>").unwrap();
    std::fs::write(ui.join("assets").join("app-1a2b3c.js"), "start()").unwrap();
    // The data folder does not exist yet: starting makes it.
    let data = sandbox.0.join("fresh").join("data");

    let core = ae_core::start(Options { ui_dir: Some(ui), data_dir: Some(data.clone()), ..on_a_free_port() })
        .await
        .expect("the core starts");

    assert!(core.addr().ip().is_loopback(), "only this computer can connect");
    assert_ne!(core.addr().port(), 0, "the real port is known");
    assert_ne!(core.addr().port(), ae_core::DEFAULT_PORT, "a free port is not the one a real core sits on");
    assert_eq!(core.url(), format!("http://127.0.0.1:{}", core.addr().port()));
    assert_eq!(core.data_dir(), ae_core::paths::real_path(&data).unwrap());
    assert!(data.join("agent-editor.db").is_file(), "the database is in the data folder");

    assert_eq!(health(&core).await["ok"], true);
    let home = get(&core.url(), "/").await.unwrap();
    assert_eq!(home.text, "<title>Agent Editor</title>");
    // A link inside the interface survives a reload: unknown paths give the interface, not an error.
    let deep = get(&core.url(), "/task/1/code").await.unwrap();
    assert_eq!(deep.text, "<title>Agent Editor</title>");
    // The page is never kept: after an update an old copy would ask for script files that are gone.
    assert_eq!(home.keep.as_deref(), Some("no-store"));
    assert_eq!(deep.keep.as_deref(), Some("no-store"));
    // The script files have a new name with every build, so those may be kept.
    let script = get(&core.url(), "/assets/app-1a2b3c.js").await.unwrap();
    assert_eq!((script.text.as_str(), script.keep), ("start()", None));

    let url = core.url();
    core.stop().await.expect("the core stops");
    assert!(get(&url, "/api/health").await.is_err(), "nothing listens after the stop");
}

#[tokio::test]
async fn a_taken_port_is_passed_over_only_when_any_free_one_will_do() {
    let first_box = Sandbox::new("port-first");
    let second_box = Sandbox::new("port-second");
    let first = ae_core::startup::serve(first_box.manager(), &on_a_free_port()).await.unwrap();
    let taken = first.addr().port();

    // "This port if you can": the port is taken, so another one is used and both cores answer.
    let prefer_taken = Options { port: Port::Free { prefer: Some(taken) }, ..Options::default() };
    let second = ae_core::startup::serve(second_box.manager(), &prefer_taken).await.unwrap();
    assert_ne!(second.addr().port(), taken);
    assert_eq!(health(&first).await["ok"], true);
    assert_eq!(health(&second).await["ok"], true);
    second.stop().await.unwrap();

    // "This port and no other": starting fails and says which port.
    let exactly = Options { port: Port::Exactly(taken), ..Options::default() };
    let refused = ae_core::startup::serve(second_box.manager(), &exactly).await.err().expect("the port is taken");
    assert!(format!("{refused:#}").contains(&format!("Cannot listen on 127.0.0.1:{taken}")), "{refused:#}");
    first.stop().await.unwrap();
}

#[tokio::test]
async fn the_port_from_last_time_is_used_again_when_it_is_free() {
    let sandbox = Sandbox::new("port-again");
    // A port that was just given up can be handed to the next program that asks, and on this machine
    // many other tests are asking. When that happens the port is no longer free, which is not what this
    // test is about, so it starts over with another one.
    let mut last = 0;
    for _ in 0..5 {
        let first = ae_core::startup::serve(sandbox.manager(), &on_a_free_port()).await.unwrap();
        let port = first.addr().port();
        first.stop().await.unwrap();

        let again = Options { port: Port::Free { prefer: Some(port) }, ..Options::default() };
        let second = ae_core::startup::serve(sandbox.manager(), &again).await.unwrap();
        last = second.addr().port();
        second.stop().await.unwrap();
        if last == port {
            return;
        }
    }
    panic!("the interface never found what it remembered under the same address; it got {last}");
}

/// Quitting the desktop app ends in `Core::stop`. Nothing the core started may outlive it:
/// an agent left behind would go on working, and spending, with no window to see it in.
#[cfg(unix)]
#[tokio::test]
async fn stopping_the_core_stops_every_agent_and_every_terminal() {
    // Named here, not at the top: this test is not built on Windows, and an unused name there is an error.
    use ae_core::terminals::TermEvent;
    use std::time::Duration;

    let running = || {
        let found = std::process::Command::new("pgrep").args(["-f", &wrapped_marker()]).output().expect("pgrep runs");
        String::from_utf8_lossy(&found.stdout).split_whitespace().count()
    };
    let sandbox = Sandbox::new("stop-all");
    let core = ae_core::startup::serve(sandbox.manager(), &on_a_free_port()).await.unwrap();
    let state = core.state().clone();
    let manager = state.manager.clone();
    let project = manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();

    // An agent started through a launcher, as real agents are, and a terminal with a shell in it.
    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock-wrapped", "What does the limiter do?").await.unwrap();
    let id = task.row.id.clone();
    until(&mut events, &id, |e| state_is(e, "idle")).await;
    assert_eq!(running(), 3, "the launcher, the agent it started and the agent's helper");
    let mut term_events = state.terminals.subscribe();
    let terminal = state.terminals.open(&id, &manager.cwd(&id).unwrap(), 80, 24).unwrap();

    core.stop().await.unwrap();

    // The task says why its agent is gone: found asleep later, it would otherwise say nothing.
    let history = manager.history(&id).unwrap();
    let why = history.iter().rev().find(|event| event["type"] == "tracker").expect("the task was told");
    assert!(why["text"].as_str().unwrap().starts_with("The editor was closed, so this task’s agent was stopped."));

    for _ in 0..100 {
        if running() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(running(), 0, "nothing of the agent is left running");
    let ended = async {
        loop {
            if let TermEvent::Exit { term_id, .. } = term_events.recv().await.expect("terminal events stay open")
                && term_id == terminal.term_id
            {
                break;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), ended).await.expect("the terminal's shell ended");
}

#[tokio::test]
async fn the_folder_dialog_is_shown_only_to_the_window_of_the_desktop_app() {
    let sandbox = Sandbox::new("dialog");

    // A core on its own has no dialog to show: the interface is told to take a typed path.
    let alone = AppState::new(sandbox.manager());
    let refused = dispatch(&alone, "dialogs.pickFolder", &json!({ "key": "anything" })).await.unwrap_err();
    assert_eq!(refused.to_string(), "Only the desktop app can show a folder dialog. Type the path instead.");

    // Inside the desktop app the dialog is the app's; here a stand-in that "chooses" the project folder.
    let chosen = sandbox.project();
    let answer = chosen.clone();
    let mut inside = AppState::new(sandbox.manager());
    inside.desktop = Some(Desktop {
        key: "the-window".into(),
        pick_folder: Arc::new(move || Some(answer.clone())),
        notify: Arc::new(|_| {}),
    });
    let picked = dispatch(&inside, "dialogs.pickFolder", &json!({ "key": "the-window" })).await.unwrap();
    assert_eq!(picked, json!({ "path": chosen.to_string_lossy() }));

    // A page that is not the app's window, a browser tab for one, does not know the key.
    for params in [json!({}), json!({ "key": "a-guess" }), json!({ "key": "" })] {
        let refused = dispatch(&inside, "dialogs.pickFolder", &params).await.unwrap_err();
        assert!(refused.to_string().starts_with("Only the desktop app"), "{refused}");
    }

    // The user closed the dialog without choosing: that is an answer, not an error.
    let mut closed = AppState::new(sandbox.manager());
    closed.desktop =
        Some(Desktop { key: "the-window".into(), pick_folder: Arc::new(|| None::<PathBuf>), notify: Arc::new(|_| {}) });
    let nothing = dispatch(&closed, "dialogs.pickFolder", &json!({ "key": "the-window" })).await.unwrap();
    assert_eq!(nothing, json!({ "path": null }));
}
