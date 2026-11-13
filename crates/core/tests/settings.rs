//! Settings and notifications, from the commands the interface sends down to what is kept and done.
//!
//! Only the practice agent and stand-ins run here: "Try it" starts the practice
//! agents, a shell script or a program that does not exist; the helper that
//! keeps the computer awake is a `sleep`; GitHub is a shell script.
#![cfg(unix)]

mod common;

use ae_core::hosts::Hosts;
use ae_core::server::{AppState, dispatch};
use ae_core::settings::try_agent;
use ae_core::tasks::Manager;
use ae_core::{Desktop, Notice, Options, Port};
use common::*;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::broadcast::Receiver;

/// What the user was told since the last look, read off the stream of events.
fn told(events: &mut Receiver<Value>) -> Vec<Value> {
    let mut notices = Vec::new();
    while let Ok(message) = events.try_recv() {
        if message["event"] == "notification" {
            notices.push(message["data"].clone());
        }
    }
    notices
}

async fn set(state: &AppState, change: Value) -> Value {
    dispatch(state, "settings.set", &change).await.unwrap()
}

/// One agent of the list Settings shows.
fn listed<'a>(settings: &'a Value, id: &str) -> &'a Value {
    settings["agents"]
        .as_array()
        .unwrap()
        .iter()
        .find(|agent| agent["id"] == id)
        .unwrap_or_else(|| panic!("no agent {id} in {settings:#}"))
}

fn process_is_there(pid: &str) -> bool {
    std::process::Command::new("/bin/kill").args(["-0", pid]).output().is_ok_and(|done| done.status.success())
}

// ----- agents -----

#[tokio::test]
async fn an_agents_command_is_shown_changed_and_put_back() {
    let sandbox = Sandbox::new("agent-command");
    let state = AppState::new(sandbox.manager());
    let project = state.manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();

    let settings = dispatch(&state, "settings.get", &json!({})).await.unwrap();
    let practice = listed(&settings, "mock");
    assert_eq!(practice["program"], mock_agent().to_string_lossy().as_ref());
    assert_eq!(practice["env"], json!([["AE_MOCK_DELAY_MS", "0"]]));
    assert_eq!((practice["own"].as_bool(), practice["changed"].as_bool()), (Some(false), Some(false)));
    assert_eq!(listed(&settings, "mock-missing")["note"], "Install it first");

    // A command that is not there: the agent is "not installed" from this moment, and no task starts with it.
    let changed = dispatch(
        &state,
        "agents.save",
        &json!({ "id": "mock", "program": "/nonexistent/ae-test/agent", "args": ["--acp"], "env": [["KEY", "v"]] }),
    )
    .await
    .unwrap();
    let practice = listed(&changed, "mock");
    assert_eq!(practice["program"], "/nonexistent/ae-test/agent");
    assert_eq!(practice["args"], json!(["--acp"]));
    assert_eq!((practice["changed"].as_bool(), practice["available"].as_bool()), (Some(true), Some(false)));
    assert_eq!(practice["note"], "Its command was not found: /nonexistent/ae-test/agent");
    assert_eq!(practice["name"], "Practice agent", "only the command is the user's to change");
    let refused = state.manager.create(&project.id, "mock", "What does the limiter do?").await.unwrap_err();
    assert!(refused.to_string().contains("Practice agent is not available"), "{refused}");
    let everywhere = dispatch(&state, "agents.list", &json!({})).await.unwrap();
    assert_eq!(everywhere.as_array().unwrap().iter().find(|a| a["id"] == "mock").unwrap()["available"], false);

    // It is kept: a core started again on the same data has the changed command.
    let again = AppState::new(sandbox.manager());
    let kept = dispatch(&again, "settings.get", &json!({})).await.unwrap();
    assert_eq!(listed(&kept, "mock")["program"], "/nonexistent/ae-test/agent");

    // Back to the command it comes with, and it starts again.
    let back = dispatch(&state, "agents.reset", &json!({ "id": "mock" })).await.unwrap();
    assert_eq!(
        (listed(&back, "mock")["changed"].as_bool(), listed(&back, "mock")["available"].as_bool()),
        (Some(false), Some(true))
    );
    assert_eq!(listed(&back, "mock")["env"], json!([["AE_MOCK_DELAY_MS", "0"]]));
    let mut events = state.manager.subscribe();
    let task = state.manager.create(&project.id, "mock", "What does the limiter do?").await.unwrap();
    until(&mut events, &task.row.id, |e| state_is(e, "idle")).await;

    // Saving the very command it comes with is the same as putting it back.
    let own_command = listed(&back, "mock").clone();
    let same = dispatch(
        &state,
        "agents.save",
        &json!({ "id": "mock", "program": own_command["program"], "env": own_command["env"] }),
    )
    .await
    .unwrap();
    assert_eq!(listed(&same, "mock")["changed"], false);

    // What cannot be: no program, an agent that is not there, putting back one that has nothing to go back to.
    let wrong = |method: &'static str, params: Value| {
        let state = state.clone();
        async move { dispatch(&state, method, &params).await.unwrap_err().to_string() }
    };
    assert!(wrong("agents.save", json!({ "id": "mock", "program": " " })).await.contains("Give the program"));
    assert!(
        wrong("agents.save", json!({ "id": "nobody", "name": "N", "short": "N", "program": "x" }))
            .await
            .contains("No such agent")
    );
    assert!(wrong("agents.reset", json!({ "id": "nobody" })).await.contains("comes with"));
    assert!(wrong("agents.remove", json!({ "id": "mock" })).await.contains("added yourself"));
}

#[tokio::test]
async fn an_agent_of_the_users_own_is_added_works_in_a_task_and_is_removed() {
    let sandbox = Sandbox::new("own-agent");
    let state = AppState::new(sandbox.manager());
    let manager = &state.manager;
    let project = manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    let mut changes = manager.subscribe();

    let own = json!({
        "name": "My agent",
        "short": "My",
        "program": mock_agent().to_string_lossy(),
        "args": [],
        "env": [["AE_MOCK_DELAY_MS", "0"], ["AE_MOCK_LOAD", "1"]]
    });
    let added = dispatch(&state, "agents.save", &own).await.unwrap();
    assert_eq!(added["saved"], "own-my-agent");
    let mine = listed(&added, "own-my-agent");
    assert_eq!(
        (mine["own"].as_bool(), mine["available"].as_bool(), mine["tasks"].as_u64()),
        (Some(true), Some(true), Some(0))
    );
    assert_eq!(mine["vendor"], "Yours");
    // Every window is told, so the pickers everywhere have the new agent at once.
    let told_windows = std::iter::from_fn(|| changes.try_recv().ok())
        .any(|m| m["event"] == "settings.changed" && m["data"]["agents"] == true);
    assert!(told_windows);

    // A second one of the same name is refused; another name gets an id of its own.
    let twice = dispatch(&state, "agents.save", &own).await.unwrap_err().to_string();
    assert_eq!(twice, "There is an agent called My agent already");
    for (params, why) in [
        (json!({ "name": "", "short": "X", "program": "x" }), "Give the agent a name"),
        (json!({ "name": "Other", "short": "", "program": "x" }), "one or two characters"),
        (json!({ "name": "Other", "short": "Long", "program": "x" }), "one or two characters"),
        (json!({ "name": "Practice agent", "short": "P", "program": "x" }), "already"),
    ] {
        let refused = dispatch(&state, "agents.save", &params).await.unwrap_err().to_string();
        assert!(refused.contains(why), "{refused}");
    }

    // It is an agent like any other: a task starts with it, and it is the default when the user says so.
    let settings = set(&state, json!({ "defaultAgent": "own-my-agent" })).await;
    assert_eq!(settings["defaultAgent"], "own-my-agent");
    let everywhere = dispatch(&state, "agents.list", &json!({})).await.unwrap();
    let defaults: Vec<&str> = everywhere
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["isDefault"] == true)
        .filter_map(|a| a["id"].as_str())
        .collect();
    assert_eq!(defaults, vec!["own-my-agent"]);
    assert!(dispatch(&state, "settings.set", &json!({ "defaultAgent": "nobody" })).await.is_err());

    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "own-my-agent", "What does the limiter do?").await.unwrap();
    let id = task.row.id.clone();
    let seen = until(&mut events, &id, |e| state_is(e, "idle")).await;
    assert!(answer_text(&seen).contains("five attempts a minute"));

    // Its name and command can be changed; the id, which tasks carry, stays.
    let renamed = dispatch(
        &state,
        "agents.save",
        &json!({ "id": "own-my-agent", "name": "Mine", "short": "Mi", "program": mock_agent().to_string_lossy(), "env": [] }),
    )
    .await
    .unwrap();
    assert_eq!(listed(&renamed, "own-my-agent")["name"], "Mine");
    assert_eq!(listed(&renamed, "own-my-agent")["tasks"], 1);

    // While a task has it, it stays. Once the task is archived it can go, and it is no default any more.
    let held = dispatch(&state, "agents.remove", &json!({ "id": "own-my-agent" })).await.unwrap_err().to_string();
    assert_eq!(held, "Mine has one open task. Hand it to another agent or archive it first.");
    manager.archive(&id).await.unwrap();
    let removed = dispatch(&state, "agents.remove", &json!({ "id": "own-my-agent" })).await.unwrap();
    assert!(removed["agents"].as_array().unwrap().iter().all(|agent| agent["id"] != "own-my-agent"));
    assert_eq!(removed["defaultAgent"], Value::Null);
    assert!(manager.registry().get("own-my-agent").is_none());

    // No default picked is a setting too: the first agent that is installed.
    set(&state, json!({ "defaultAgent": "mock-load" })).await;
    assert_eq!(set(&state, json!({ "defaultAgent": null })).await["defaultAgent"], Value::Null);
}

#[tokio::test]
async fn try_it_reads_how_an_agent_introduces_itself_and_says_what_went_wrong() {
    let sandbox = Sandbox::new("try");
    let state = AppState::new(sandbox.manager());

    let practice = dispatch(&state, "agents.try", &json!({ "id": "mock" })).await.unwrap();
    assert_eq!((practice["ok"].as_bool(), practice["name"].as_str()), (Some(true), Some("Practice agent")));
    assert_eq!(
        practice["says"],
        format!(
            "It started and calls itself Practice agent, version {}. It is told the conversation again after a \
             restart, takes pictures and can use the editor's own tools.",
            env!("CARGO_PKG_VERSION")
        )
    );
    // The practice agents that differ say so themselves.
    let loads = dispatch(&state, "agents.try", &json!({ "id": "mock-load" })).await.unwrap();
    assert_eq!((loads["continues"].as_bool(), loads["pictures"].as_bool()), (Some(true), Some(true)));
    let older = dispatch(&state, "agents.try", &json!({ "id": "mock-modes" })).await.unwrap();
    assert_eq!(older["pictures"], false);
    assert!(dispatch(&state, "agents.try", &json!({ "id": "nobody" })).await.is_err());

    // A program that does not exist.
    dispatch(
        &state,
        "agents.save",
        &json!({ "name": "Ghost", "short": "Gh", "program": "/nonexistent/ae-test/ghost" }),
    )
    .await
    .unwrap();
    let ghost = dispatch(&state, "agents.try", &json!({ "id": "own-ghost" })).await.unwrap();
    assert_eq!(ghost["ok"], false);
    assert_eq!(
        ghost["problem"],
        "The program \"/nonexistent/ae-test/ghost\" was not found. Check how it is spelled, or give its full path."
    );

    // A program that ends at once, one that refuses, and one that never answers.
    let script = |name: &str, script: &str| {
        let mut agent = state.manager.registry().get("mock").unwrap().clone();
        agent.name = name.to_string();
        agent.program = "/bin/sh".into();
        agent.args = vec!["-c".into(), script.into()];
        agent
    };
    let cwd = sandbox.data();
    let ends =
        try_agent(&script("Quitter", "echo 'no licence found' >&2; exit 3"), &cwd, Duration::from_secs(20)).await;
    assert_eq!(
        ends["problem"],
        "Quitter started and stopped again before it said anything. It wrote: no licence found."
    );
    let refuses = try_agent(
        &script(
            "Refuser",
            r#"read line; echo '{"jsonrpc":"2.0","id":1,"error":{"code":-32603,"message":"Sign in first"}}'; sleep 5"#,
        ),
        &cwd,
        Duration::from_secs(20),
    )
    .await;
    assert_eq!(refuses["problem"], "Refuser started, but refused the first message: Sign in first");
    let silent = try_agent(&script("Silent", "exec sleep 30"), &cwd, Duration::from_millis(300)).await;
    let problem = silent["problem"].as_str().unwrap();
    assert!(problem.starts_with("Silent did not answer within a second."), "{problem}");
    assert!(problem.ends_with("check that the command speaks the Agent Client Protocol."), "{problem}");
}

#[tokio::test]
async fn trying_an_agent_that_comes_through_a_launcher_leaves_nothing_running() {
    let running = || {
        let found = std::process::Command::new("pgrep").args(["-f", &wrapped_marker()]).output().expect("pgrep runs");
        String::from_utf8_lossy(&found.stdout).split_whitespace().count()
    };
    let sandbox = Sandbox::new("try-wrapped");
    let state = AppState::new(sandbox.manager());
    let tried = dispatch(&state, "agents.try", &json!({ "id": "mock-wrapped" })).await.unwrap();
    assert_eq!(tried["ok"], true, "{tried}");
    for _ in 0..100 {
        if running() == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert_eq!(running(), 0, "the launcher, the agent and the agent's helper are all gone");
    // Trying is not working: no task was made and nothing was said to anybody.
    assert!(state.manager.list().unwrap().is_empty());
}

// ----- the editor -----

#[tokio::test]
async fn a_new_project_starts_with_formatting_as_settings_say_and_an_older_one_keeps_what_it_has() {
    let sandbox = Sandbox::new("new-project");
    let state = AppState::new(sandbox.manager());
    let manager = &state.manager;
    let folder = |name: &str| {
        let folder = sandbox.0.join(name);
        std::fs::create_dir_all(&folder).unwrap();
        folder.to_string_lossy().into_owned()
    };
    let fresh = dispatch(&state, "settings.get", &json!({})).await.unwrap();
    assert_eq!((fresh["formatOnSave"].as_bool(), fresh["ruleOffers"].as_bool()), (Some(true), Some(true)));

    let older = manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    let settings = set(&state, json!({ "formatOnSave": false })).await;
    assert_eq!(settings["formatOnSave"], false);
    let newer = manager.add_project(&folder("second"), None).unwrap();
    assert!(!state.lsp.format_on_save(&newer.id));
    assert!(state.lsp.format_on_save(&older.id), "an older project is not touched");

    // It is a start, not a rule: the project's own switch still works, and adding the folder again changes nothing.
    state.lsp.set_format_on_save(&newer.id, true).unwrap();
    manager.add_project(&folder("second"), None).unwrap();
    assert!(state.lsp.format_on_save(&newer.id));

    // Turned back on, the next project starts with it again.
    set(&state, json!({ "formatOnSave": true })).await;
    let newest = manager.add_project(&folder("third"), None).unwrap();
    assert!(state.lsp.format_on_save(&newest.id));

    for (change, why) in [
        (json!({ "formatOnSave": "yes" }), "formatOnSave is on or off"),
        (json!({ "volume": 11 }), "There is no setting called volume"),
        (json!({ "notifications": { "sound": true } }), "There is no notification called sound"),
        (json!({ "notifications": { "on": 1 } }), "on is on or off"),
    ] {
        assert_eq!(dispatch(&state, "settings.set", &change).await.unwrap_err().to_string(), why);
    }
}

#[tokio::test]
async fn the_offer_of_a_rule_is_stopped_everywhere_at_once_and_a_project_can_still_stop_it_for_itself() {
    let sandbox = Sandbox::new("rule-offers");
    let state = AppState::new(sandbox.manager());
    let manager = &state.manager;
    let one = manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    let second = sandbox.0.join("second");
    std::fs::create_dir_all(&second).unwrap();
    let two = manager.add_project(&second.to_string_lossy(), None).unwrap();
    let offers = |project: &str| manager.rule_offers(project);
    assert!(offers(&one.id) && offers(&two.id));

    // Off in Settings: no project is asked from this moment, the ones that are there included.
    assert_eq!(set(&state, json!({ "ruleOffers": false })).await["ruleOffers"], false);
    assert!(!offers(&one.id) && !offers(&two.id));
    assert_eq!(
        dispatch(&state, "rules.offers", &json!({ "projectId": one.id })).await.unwrap(),
        json!({ "on": false })
    );

    // A project stops the question for itself meanwhile: that is its own, and it stays.
    manager.set_rule_offers(&two.id, false).unwrap();
    set(&state, json!({ "ruleOffers": true })).await;
    assert!(offers(&one.id), "on again everywhere");
    assert!(!offers(&two.id), "but not where the project said no");
}

// ----- notifications -----

#[tokio::test]
async fn nothing_is_told_until_notifications_are_on_and_then_each_thing_once() {
    let sandbox = Sandbox::new("notify");
    let state = AppState::new(sandbox.manager());
    let manager = &state.manager;
    let project = manager.add_project(&sandbox.project().to_string_lossy(), Some("shop".into())).unwrap();
    let (mut events, mut notices) = (manager.subscribe(), manager.subscribe());

    let fresh = dispatch(&state, "settings.get", &json!({})).await.unwrap();
    assert_eq!(
        fresh["notifications"],
        json!({ "on": false, "question": true, "turnEnd": true, "checksFailed": true, "readyToMerge": true, "byApp": false })
    );
    let task = manager.create(&project.id, "mock", "What does the limiter do?").await.unwrap();
    let id = task.row.id.clone();
    until(&mut events, &id, |e| state_is(e, "idle")).await;
    assert_eq!(told(&mut notices), Vec::<Value>::new(), "off until the user turns notifications on");

    let on = set(&state, json!({ "notifications": { "on": true } })).await;
    assert_eq!(on["notifications"]["on"], true);

    // A question: who asks, in which task, and what.
    manager.prompt(&id, "Please edit the limit".into()).await.unwrap();
    let asked = until(&mut events, &id, |e| is(e, "permission_request")).await;
    let question = asked.last().unwrap();
    let said = told(&mut notices);
    assert_eq!(
        said,
        vec![json!({
            "id": format!("{id}:{}", question["seq"]),
            "kind": "question",
            "taskId": id,
            "view": "code",
            "title": "Practice agent asks a question",
            "body": "What does the limiter do? · shop\nEdit NOTES.md",
            "byApp": false
        })]
    );

    // The end of the turn. The answer to the question and the agent's words in between are told to nobody.
    manager.answer_permission(&id, question["requestId"].as_str().unwrap(), Some("allow-once".into())).unwrap();
    until(&mut events, &id, |e| state_is(e, "idle")).await;
    let said = told(&mut notices);
    assert_eq!(said.len(), 1, "{said:#?}");
    assert_eq!(
        (said[0]["kind"].as_str(), said[0]["title"].as_str()),
        (Some("turnEnd"), Some("Practice agent finished its turn"))
    );
    assert_eq!(said[0]["body"], "What does the limiter do? · shop");

    // Each kind has its own switch.
    set(&state, json!({ "notifications": { "turnEnd": false } })).await;
    manager.prompt(&id, "Change it once more".into()).await.unwrap();
    let asked = until(&mut events, &id, |e| is(e, "permission_request")).await;
    manager
        .answer_permission(&id, asked.last().unwrap()["requestId"].as_str().unwrap(), Some("allow-once".into()))
        .unwrap();
    until(&mut events, &id, |e| state_is(e, "idle")).await;
    let kinds: Vec<Value> = told(&mut notices).into_iter().map(|notice| notice["kind"].clone()).collect();
    assert_eq!(kinds, vec![json!("question")]);
    set(&state, json!({ "notifications": { "turnEnd": true, "question": false } })).await;

    // A turn the user stopped is no news to the user, and neither is an agent the user stopped.
    manager.prompt(&id, "Change it a third time".into()).await.unwrap();
    until(&mut events, &id, |e| is(e, "permission_request")).await;
    manager.cancel(&id).unwrap();
    until(&mut events, &id, |e| state_is(e, "idle")).await;
    manager.stop_agent(&id).await.unwrap();
    until(&mut events, &id, |e| state_is(e, "asleep")).await;
    assert_eq!(told(&mut notices), Vec::<Value>::new());

    // Turned off again, nothing is told.
    set(&state, json!({ "notifications": { "on": false, "question": true } })).await;
    manager.prompt(&id, "And what calls it?".into()).await.unwrap();
    until(&mut events, &id, |e| state_is(e, "idle")).await;
    assert_eq!(told(&mut notices), Vec::<Value>::new());
}

#[tokio::test]
async fn a_window_in_front_keeps_it_quiet_and_one_that_is_gone_does_not() {
    let sandbox = Sandbox::new("notify-front");
    let state = AppState::new(sandbox.manager());
    let manager = &state.manager;
    let project = manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    set(&state, json!({ "notifications": { "on": true } })).await;
    let (mut events, mut notices) = (manager.subscribe(), manager.subscribe());
    let turn = |text: &'static str, id: Option<String>| {
        let (manager, project) = (manager.clone(), project.id.clone());
        async move {
            match id {
                Some(id) => {
                    manager.prompt(&id, text.into()).await.unwrap();
                    id
                }
                None => manager.create(&project, "mock", text).await.unwrap().row.id,
            }
        }
    };

    // Two windows are connected. One of them is in front: the user sees what happens, nothing is told.
    let (front, behind) = (manager.window_opened(), manager.window_opened());
    assert_ne!(front.id(), behind.id());
    dispatch(&state, "notifications.attention", &json!({ "window": front.id(), "front": true })).await.unwrap();
    let id = turn("What does the limiter do?", None).await;
    until(&mut events, &id, |e| state_is(e, "idle")).await;
    assert_eq!(told(&mut notices), Vec::<Value>::new(), "the window behind must not tell what the one in front shows");

    // The user goes elsewhere: neither window is in front.
    dispatch(&state, "notifications.attention", &json!({ "window": front.id(), "front": false })).await.unwrap();
    turn("And who calls it?", Some(id.clone())).await;
    until(&mut events, &id, |e| state_is(e, "idle")).await;
    assert_eq!(told(&mut notices).len(), 1);

    // Back in front, then the window is closed without a word: a window that is gone is not looking.
    dispatch(&state, "notifications.attention", &json!({ "window": front.id(), "front": true })).await.unwrap();
    drop(front);
    turn("And since when?", Some(id.clone())).await;
    until(&mut events, &id, |e| state_is(e, "idle")).await;
    assert_eq!(told(&mut notices).len(), 1);

    // What is said about a window that is not connected changes nothing.
    dispatch(&state, "notifications.attention", &json!({ "window": 9999, "front": true })).await.unwrap();
    dispatch(&state, "notifications.attention", &json!({ "front": true })).await.unwrap();
    turn("And why?", Some(id.clone())).await;
    until(&mut events, &id, |e| state_is(e, "idle")).await;
    assert_eq!(told(&mut notices).len(), 1);
    drop(behind);
}

#[tokio::test]
async fn inside_the_desktop_app_the_app_shows_the_notice_with_or_without_a_window() {
    let sandbox = Sandbox::new("notify-app");
    let shown: Arc<Mutex<Vec<Notice>>> = Arc::default();
    let show = shown.clone();
    let desktop = Desktop {
        key: "the-window".into(),
        pick_folder: Arc::new(|| None::<PathBuf>),
        notify: Arc::new(move |notice| show.lock().unwrap().push(notice.clone())),
    };
    let options = Options { port: Port::Free { prefer: None }, desktop: Some(desktop), ..Options::default() };
    let core = ae_core::startup::serve(sandbox.manager(), &options).await.unwrap();
    let state = core.state().clone();
    let manager = &state.manager;
    let project = manager.add_project(&sandbox.project().to_string_lossy(), Some("shop".into())).unwrap();

    // The page is told that the app shows notices, so it does not ask a browser for permission.
    let settings = set(&state, json!({ "notifications": { "on": true } })).await;
    assert_eq!(settings["notifications"]["byApp"], true);

    // No window at all, as on a Mac after the window was closed: the app still tells.
    let (mut events, mut notices) = (manager.subscribe(), manager.subscribe());
    let task = manager.create(&project.id, "mock", "What does the limiter do?").await.unwrap();
    let id = task.row.id.clone();
    until(&mut events, &id, |e| state_is(e, "idle")).await;
    let by_app = shown.lock().unwrap().clone();
    assert_eq!(by_app.len(), 1);
    assert_eq!(
        (by_app[0].kind, by_app[0].title.as_str(), by_app[0].body.as_str(), by_app[0].task_id.as_str()),
        ("turnEnd", "Practice agent finished its turn", "What does the limiter do? · shop", id.as_str())
    );
    // A page that is connected too hears that it was shown already.
    let said = told(&mut notices);
    assert_eq!(
        (said.len(), said[0]["byApp"].as_bool(), said[0]["id"].as_str()),
        (1, Some(true), Some(by_app[0].id.as_str()))
    );

    // With the app's window in front the app shows nothing either.
    let window = manager.window_opened();
    manager.window_attention(window.id(), true);
    manager.prompt(&id, "And who calls it?".into()).await.unwrap();
    until(&mut events, &id, |e| state_is(e, "idle")).await;
    assert_eq!(shown.lock().unwrap().len(), 1);
    drop(window);
    core.stop().await.unwrap();
}

// ----- merge requests -----

const PULL_PENDING: &str = r#"{"number":128,"title":"TITLE","url":"https://github.com/x/shop/pull/128","state":"OPEN","isDraft":false,
 "mergeable":"MERGEABLE","reviewDecision":"","statusCheckRollup":[{"__typename":"CheckRun","status":"IN_PROGRESS","conclusion":""}]}"#;

/// A stand-in for `gh` that knows one pull request, kept in `pull.json` next to it, and notes every call.
fn fake_gh(dir: &Path) -> Hosts {
    let gh = fake_tool(
        dir,
        "gh",
        r#"here="$(dirname "$0")"
echo "$@" >> "$here/calls.log"
case "$1 $2" in
  "auth status") exit 0 ;;
  "pr view") if [ -f "$here/pull.json" ]; then cat "$here/pull.json"; else echo "no pull requests found" >&2; exit 1; fi ;;
  *) echo "unexpected call: $*" >&2; exit 2 ;;
esac
"#,
    );
    let missing = PathBuf::from("/nonexistent/ae-test/tool");
    Hosts::with(gh, missing.clone(), missing, "http://127.0.0.1:9/graphql".into())
}

/// What `gh` says about the pull request from now on.
fn pull_is(tools: &Path, checks: &str, state: &str) {
    let run = match checks {
        "failed" => r#""status":"COMPLETED","conclusion":"FAILURE""#,
        "passed" => r#""status":"COMPLETED","conclusion":"SUCCESS""#,
        _ => r#""status":"IN_PROGRESS","conclusion":"""#,
    };
    let pull = PULL_PENDING
        .replace(r#""status":"IN_PROGRESS","conclusion":"""#, run)
        .replace("\"OPEN\"", &format!("\"{state}\""));
    std::fs::write(tools.join("pull.json"), pull).unwrap();
}

fn views(tools: &Path) -> usize {
    std::fs::read_to_string(tools.join("calls.log"))
        .unwrap_or_default()
        .lines()
        .filter(|call| call.starts_with("pr view"))
        .count()
}

/// A task in a project that reads as GitHub, with its branch pushed to a folder next to it.
async fn pushed_task(sandbox: &Sandbox, manager: &Arc<Manager>) -> String {
    let repo = git_project(sandbox);
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    let bare = sandbox.0.join("remote.git");
    git(&repo, &["init", "-q", "--bare", &bare.to_string_lossy()]);
    git(&repo, &["remote", "add", "origin", "git@github.com:x/shop.git"]);
    git(&repo, &["config", &format!("url.{}.insteadOf", bare.to_string_lossy()), "git@github.com:x/shop.git"]);
    let project = manager.add_project(&repo.to_string_lossy(), Some("shop".into())).unwrap();
    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock", "Take the limit from config").await.unwrap();
    until(&mut events, &task.row.id, |e| state_is(e, "idle")).await;
    let cwd = PathBuf::from(&task.row.cwd);
    std::fs::write(cwd.join("NOTES.md"), "A note.\n").unwrap();
    git(&cwd, &["add", "."]);
    git(&cwd, &["commit", "-q", "-m", "A note"]);
    git(&cwd, &["push", "-q", "-u", "origin", task.row.branch.as_deref().unwrap()]);
    task.row.id
}

#[tokio::test]
async fn failed_checks_and_a_request_that_can_merge_are_each_told_once() {
    let sandbox = Sandbox::new("notify-request");
    let tools = sandbox.0.join("tools");
    let state = AppState::new(sandbox.manager_with(fake_gh(&tools)));
    let manager = &state.manager;
    let id = pushed_task(&sandbox, manager).await;
    let mut notices = manager.subscribe();
    let titles = |notices: &mut Receiver<Value>| -> Vec<String> {
        told(notices)
            .iter()
            .map(|notice| format!("{}: {}", notice["kind"].as_str().unwrap(), notice["title"].as_str().unwrap()))
            .collect()
    };

    // Notifications are off: nobody is asked anything.
    pull_is(&tools, "pending", "OPEN");
    manager.requests_round().await;
    assert_eq!(views(&tools), 0, "no host is asked while the user does not want to be told");

    set(&state, json!({ "notifications": { "on": true } })).await;
    manager.requests_round().await;
    assert_eq!(views(&tools), 1);
    assert_eq!(titles(&mut notices), Vec::<String>::new(), "checks that run are nothing to tell");

    // The checks fail: told once, however often the request is looked at, by the core or by a window.
    pull_is(&tools, "failed", "OPEN");
    manager.requests_round().await;
    assert_eq!(titles(&mut notices), vec!["checksFailed: The checks of pull request 128 failed"]);
    manager.requests_round().await;
    dispatch(&state, "mr.get", &json!({ "taskId": id, "fresh": true })).await.unwrap();
    assert_eq!(titles(&mut notices), Vec::<String>::new());

    // A host that does not answer for a moment changes nothing: the same failure is not told again.
    std::fs::rename(tools.join("pull.json"), tools.join("pull.away")).unwrap();
    manager.requests_round().await;
    dispatch(&state, "mr.overview", &json!({})).await.unwrap();
    std::fs::rename(tools.join("pull.away"), tools.join("pull.json")).unwrap();
    manager.requests_round().await;
    assert_eq!(titles(&mut notices), Vec::<String>::new());

    // A core started again on the same data remembers what it told.
    let again = AppState::new(sandbox.manager_with(fake_gh(&tools)));
    let mut later = again.manager.subscribe();
    again.manager.requests_round().await;
    assert_eq!(titles(&mut later), Vec::<String>::new());

    // The agent pushes a fix, the checks run again and pass: ready to merge, told once, by a click on "changes".
    pull_is(&tools, "pending", "OPEN");
    manager.requests_round().await;
    pull_is(&tools, "passed", "OPEN");
    manager.requests_round().await;
    let ready = told(&mut notices);
    assert_eq!(ready.len(), 1, "{ready:#?}");
    assert_eq!(
        (ready[0]["kind"].as_str(), ready[0]["title"].as_str(), ready[0]["view"].as_str()),
        (Some("readyToMerge"), Some("Pull request 128 is ready to merge"), Some("changes"))
    );
    assert_eq!(ready[0]["body"], "Take the limit from config · shop");

    // They fail a second time: that is a new thing, and it is told.
    pull_is(&tools, "failed", "OPEN");
    manager.requests_round().await;
    assert_eq!(titles(&mut notices), vec!["checksFailed: The checks of pull request 128 failed"]);

    // With its switch off it is not told, and with both switches off nobody is asked.
    set(&state, json!({ "notifications": { "readyToMerge": false } })).await;
    pull_is(&tools, "passed", "OPEN");
    manager.requests_round().await;
    assert_eq!(titles(&mut notices), Vec::<String>::new());
    set(&state, json!({ "notifications": { "checksFailed": false } })).await;
    let asked = views(&tools);
    manager.requests_round().await;
    assert_eq!(views(&tools), asked);

    // Merged: nothing to tell, and a window in front would have kept all of the above quiet.
    set(&state, json!({ "notifications": { "readyToMerge": true, "checksFailed": true } })).await;
    pull_is(&tools, "passed", "MERGED");
    manager.requests_round().await;
    let window = manager.window_opened();
    manager.window_attention(window.id(), true);
    pull_is(&tools, "failed", "OPEN");
    manager.requests_round().await;
    assert_eq!(titles(&mut notices), Vec::<String>::new());
}

#[tokio::test]
async fn a_request_that_merges_by_itself_is_not_announced_as_ready() {
    let sandbox = Sandbox::new("notify-auto");
    let tools = sandbox.0.join("tools");
    let state = AppState::new(sandbox.manager_with(fake_gh(&tools)));
    let manager = &state.manager;
    let id = pushed_task(&sandbox, manager).await;
    set(&state, json!({ "notifications": { "on": true } })).await;
    pull_is(&tools, "pending", "OPEN");
    manager.requests_round().await;
    dispatch(&state, "mr.auto", &json!({ "taskId": id, "on": true })).await.unwrap();

    let mut notices = manager.subscribe();
    pull_is(&tools, "passed", "OPEN");
    manager.requests_round().await;
    assert_eq!(told(&mut notices), Vec::<Value>::new(), "it is about to merge without the user");
}

// ----- keeping the computer awake -----

#[tokio::test]
async fn the_computer_is_kept_awake_exactly_while_an_agent_works() {
    let sandbox = Sandbox::new("awake");
    let state = AppState::new(sandbox.manager());
    let manager = &state.manager;
    let project = manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    // A stand-in for the system's helper: it notes its process id and then waits, as `caffeinate` does.
    let noted = sandbox.0.join("helper.pid");
    let helper = format!("echo $$ >> '{}'; exec sleep 600", noted.display());
    manager.keep_awake_with(vec!["/bin/sh".into(), "-c".into(), helper]);
    // The helpers started so far, once there are `count` of them: a helper needs a moment to note itself.
    let pids = |count: usize| {
        let noted = noted.clone();
        async move {
            let read = || -> Vec<String> {
                std::fs::read_to_string(&noted).unwrap_or_default().lines().map(str::to_string).collect()
            };
            for _ in 0..400 {
                if read().len() >= count {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            read()
        }
    };
    // A practice agent slow enough to be caught working.
    dispatch(
        &state,
        "agents.save",
        &json!({ "name": "Slow", "short": "Sl", "program": mock_agent().to_string_lossy(), "env": [["AE_MOCK_DELAY_MS", "150"]] }),
    )
    .await
    .unwrap();
    let active = || manager.keep_awake()["active"] == true;
    let wait_for = |wanted: bool| async move {
        for _ in 0..400 {
            if active() == wanted {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the helper never became {}", if wanted { "active" } else { "idle" });
    };

    // Off, as it is at first: an agent works and no helper is started.
    let fresh = dispatch(&state, "settings.get", &json!({})).await.unwrap();
    assert_eq!(
        (fresh["keepAwake"]["on"].as_bool(), fresh["keepAwake"]["available"].as_bool()),
        (Some(false), Some(true))
    );
    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "own-slow", "What does the limiter do?").await.unwrap();
    let id = task.row.id.clone();
    until(&mut events, &id, |e| state_is(e, "idle")).await;
    assert_eq!(pids(0).await, Vec::<String>::new());

    // On, with nobody working: still no helper.
    let on = set(&state, json!({ "keepAwake": true })).await;
    assert_eq!((on["keepAwake"]["on"].as_bool(), on["keepAwake"]["active"].as_bool()), (Some(true), Some(false)));

    // The agent works: the helper runs. The turn ends: the helper is gone.
    manager.prompt(&id, "And who calls it?".into()).await.unwrap();
    wait_for(true).await;
    let first = pids(1).await;
    assert_eq!(first.len(), 1);
    assert!(process_is_there(&first[0]));
    until(&mut events, &id, |e| state_is(e, "idle")).await;
    wait_for(false).await;
    assert!(!process_is_there(&first[0]), "the helper ended with the agent's turn");

    // Turned off in the middle of a turn, it ends at once; turned on in the middle of a turn, it starts at once.
    manager.prompt(&id, "And since when?".into()).await.unwrap();
    wait_for(true).await;
    let second = pids(2).await;
    set(&state, json!({ "keepAwake": false })).await;
    assert!(!active() && !process_is_there(&second[1]));
    set(&state, json!({ "keepAwake": true })).await;
    assert!(active());
    let third = pids(3).await;
    assert_eq!(third.len(), 3);

    // The core stops while the agent works: the helper does not outlive it.
    manager.stop_all().await;
    assert!(!process_is_there(&third[2]));
    assert!(!active());
}
