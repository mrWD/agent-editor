//! The editor's own tools, as agents reach them.
//!
//! A core is served on a free port of this machine, and the practice agent
//! calls the tools the way a real agent would. Nothing leaves the machine.

mod common;

use common::*;
use serde_json::{Value, json};

async fn served(sandbox: &Sandbox) -> (ae_core::startup::Core, std::sync::Arc<ae_core::tasks::Manager>) {
    let options = ae_core::Options { port: ae_core::Port::Free { prefer: None }, ..Default::default() };
    let core = ae_core::startup::serve(sandbox.manager(), &options).await.unwrap();
    let manager = core.state().manager.clone();
    (core, manager)
}

fn call(url: &str, message: Value) -> (u16, Option<Value>) {
    let sent = ureq::post(url).config().http_status_as_error(false).build().send_json(&message);
    let mut response = sent.expect("the core answers");
    let status = response.status().as_u16();
    (status, response.body_mut().read_json::<Value>().ok())
}

#[tokio::test]
async fn an_agent_leaves_a_note_and_the_next_agent_is_told() {
    let sandbox = Sandbox::new("mcp-note");
    let (core, manager) = served(&sandbox).await;
    let project = manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock", "What does the limiter do?").await.unwrap();
    let id = task.row.id.clone();
    until(&mut events, &id, |e| state_is(e, "idle")).await;

    manager.prompt(&id, "Please leave a note for whoever comes next".into()).await.unwrap();
    let seen = until(&mut events, &id, |e| state_is(e, "idle")).await;
    let note = seen.iter().find(|e| is(e, "agent_note")).expect("the note is in the conversation");
    assert_eq!((note["from"].as_str(), note["agentId"].as_str()), (Some("Practice agent"), Some("mock")));
    assert_eq!(note["text"], "The limit lives in config/limits.ts. Do not put the number back.");
    assert_eq!(answer_text(&seen), "The note is kept with the task. Whoever works on it next is told.");

    // It can be read back, and another agent that takes the task over is told it.
    manager.prompt(&id, "Now read the notes".into()).await.unwrap();
    let seen = until(&mut events, &id, |e| state_is(e, "idle")).await;
    assert_eq!(
        answer_text(&seen),
        "- Practice agent: The limit lives in config/limits.ts. Do not put the number back."
    );
    manager.switch_agent(&id, "mock-load").await.unwrap();
    manager.prompt(&id, "And who calls it?".into()).await.unwrap();
    let seen = until(&mut events, &id, |e| state_is(e, "idle")).await;
    let told: String = seen
        .iter()
        .filter(|e| e["update"]["sessionUpdate"] == "agent_thought_chunk")
        .filter_map(|e| e["update"]["content"]["text"].as_str())
        .collect();
    assert!(told.contains("Note left by Practice agent: The limit lives in config/limits.ts."), "{told}");
    core.stop().await.unwrap();
}

#[tokio::test]
async fn an_agent_sees_the_other_tasks_and_reports_what_it_noticed() {
    let sandbox = Sandbox::new("mcp-board");
    let (core, manager) = served(&sandbox).await;
    let project = manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    let mut events = manager.subscribe();
    let first = manager.create(&project.id, "mock", "What does the limiter do?").await.unwrap();
    until(&mut events, &first.row.id, |e| state_is(e, "idle")).await;
    let second = manager.create(&project.id, "mock-load", "Look at the orders").await.unwrap();
    let id = second.row.id.clone();
    until(&mut events, &id, |e| state_is(e, "idle")).await;

    manager.prompt(&id, "First list the tasks".into()).await.unwrap();
    let said = answer_text(&until(&mut events, &id, |e| state_is(e, "idle")).await);
    assert!(said.contains("- What does the limiter do?: Practice agent; waits for the user's next message"), "{said}");
    assert!(said.contains("- Look at the orders (this task): Practice agent; its agent is working"), "{said}");

    // Something outside the task goes to the Health screen, once, however often it is said.
    for _ in 0..2 {
        manager.prompt(&id, "Please report a problem you noticed".into()).await.unwrap();
        let said = answer_text(&until(&mut events, &id, |e| state_is(e, "idle")).await);
        assert_eq!(said, "The user is told on the Health screen. Do not fix it as part of this task.");
    }
    let health = manager.health().unwrap();
    let noticed: Vec<&Value> =
        health["findings"].as_array().unwrap().iter().filter(|f| f["category"] == "Noticed").collect();
    assert_eq!(noticed.len(), 1);
    assert_eq!(noticed[0]["title"], "POST /register has no rate limit");
    assert_eq!(
        noticed[0]["detail"],
        "Only /login is limited, in src/middleware/rateLimit.ts. Noticed by Practice agent while it worked on “Look at the orders”."
    );
    assert!(noticed[0]["fix"].as_str().unwrap().contains("An agent noticed this while working on another task"));
    // It can be ignored like any other finding.
    manager.health_ignore(noticed[0]["id"].as_str()).unwrap();
    assert!(!manager.health().unwrap()["findings"].as_array().unwrap().iter().any(|f| f["category"] == "Noticed"));
    core.stop().await.unwrap();
}

#[tokio::test]
async fn the_tools_speak_the_protocol_and_only_to_a_known_key() {
    let sandbox = Sandbox::new("mcp-protocol");
    let (core, manager) = served(&sandbox).await;
    let project = manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock", "What does the limiter do?").await.unwrap();
    until(&mut events, &task.row.id, |e| state_is(e, "idle")).await;
    let url = manager.mcp_address_of(&task.row.id).expect("the agent was given an address");
    assert!(url.starts_with(&format!("{}/mcp/", core.url())), "{url}");

    let asked = url.clone();
    let answers = tokio::task::spawn_blocking(move || {
        let hello = call(&asked, json!({ "jsonrpc": "2.0", "id": 1, "method": "initialize", "params": { "protocolVersion": "2025-06-18" } }));
        let noted = call(&asked, json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        let listed = call(&asked, json!({ "jsonrpc": "2.0", "id": 2, "method": "tools/list" }));
        let unknown = call(&asked, json!({ "jsonrpc": "2.0", "id": 3, "method": "tools/call", "params": { "name": "delete_everything" } }));
        let empty = call(&asked, json!({ "jsonrpc": "2.0", "id": 4, "method": "tools/call", "params": { "name": "leave_note", "arguments": { "text": " " } } }));
        let odd = call(&asked, json!({ "jsonrpc": "2.0", "id": 5, "method": "resources/list" }));
        let stranger = call(&format!("{}x", asked), json!({ "jsonrpc": "2.0", "id": 6, "method": "tools/list" }));
        (hello, noted, listed, unknown, empty, odd, stranger)
    })
    .await
    .unwrap();
    let (hello, noted, listed, unknown, empty, odd, stranger) = answers;

    let hello = hello.1.unwrap();
    assert_eq!(hello["result"]["protocolVersion"], "2025-06-18");
    assert_eq!(hello["result"]["serverInfo"]["name"], "agent-editor");
    assert_eq!(noted.0, 202, "a message that wants no answer gets none");
    let names: Vec<&str> = listed.1.as_ref().unwrap()["result"]["tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["list_tasks", "leave_note", "read_notes", "report_problem"]);
    let unknown = unknown.1.unwrap();
    assert_eq!(unknown["result"]["isError"], true);
    assert_eq!(unknown["result"]["content"][0]["text"], "No such tool: delete_everything");
    assert_eq!(empty.1.unwrap()["result"]["content"][0]["text"], "The note is empty");
    assert_eq!(odd.1.unwrap()["error"]["code"], -32601);
    assert_eq!(stranger.0, 404, "a key nobody was given opens nothing");

    // A new session of the agent gets a new key; the old one stops working.
    manager.stop_agent(&task.row.id).await.unwrap();
    until(&mut events, &task.row.id, |e| state_is(e, "asleep")).await;
    manager.prompt(&task.row.id, "And who calls it?".into()).await.unwrap();
    until(&mut events, &task.row.id, |e| state_is(e, "idle")).await;
    let fresh = manager.mcp_address_of(&task.row.id).unwrap();
    assert_ne!(fresh, url);
    let old =
        tokio::task::spawn_blocking(move || call(&url, json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/list" })))
            .await
            .unwrap();
    assert_eq!(old.0, 404);
    core.stop().await.unwrap();
}
