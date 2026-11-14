//! Tool servers of the user's own: kept in the settings and named to every agent that starts.
//!
//! The practice agent says back which servers its session was given. None is started or reached.

mod common;

use ae_core::server::{AppState, dispatch};
use common::*;
use serde_json::{Value, json};

async fn asked(state: &AppState, method: &str, params: Value) -> anyhow::Result<Value> {
    dispatch(state, method, &params).await
}

/// What the agent of a task answers to a message.
async fn answer(state: &AppState, task_id: &str, text: &str) -> String {
    let mut events = state.manager.subscribe();
    state.manager.prompt(task_id, text.into()).await.unwrap();
    answer_text(&until(&mut events, task_id, |event| state_is(event, "idle")).await)
}

#[tokio::test]
async fn servers_are_kept_changed_and_removed_and_a_wrong_one_is_refused() {
    let sandbox = Sandbox::new("toolservers-kept");
    let state = AppState::new(sandbox.manager());
    let mut windows = state.manager.subscribe();
    assert_eq!(asked(&state, "tools.list", json!({})).await.unwrap()["servers"], json!([]));

    let sentry = json!({ "name": "Sentry", "kind": "http", "url": "https://mcp.sentry.dev/mcp" });
    let saved = asked(&state, "tools.save", sentry.clone()).await.unwrap();
    let id = saved["saved"].as_str().expect("the new server has an id").to_string();
    assert_eq!(
        saved["servers"],
        json!([{ "id": id, "name": "Sentry", "kind": "http", "program": "", "args": [], "env": [],
                 "url": "https://mcp.sentry.dev/mcp", "headers": [], "on": true }])
    );
    assert_eq!(windows.try_recv().unwrap()["event"], "tools.changed", "every window is told");

    // A second one of the same name, in any case, is refused; so is one that is not on the list.
    let twice = asked(&state, "tools.save", json!({ "name": "sentry", "kind": "stdio", "program": "npx" })).await;
    assert_eq!(twice.unwrap_err().to_string(), "There is a server called sentry already");
    let unknown =
        asked(&state, "tools.save", json!({ "id": "nope", "name": "Other", "kind": "stdio", "program": "x" })).await;
    assert_eq!(unknown.unwrap_err().to_string(), "No such server: nope");

    // Changed under its id: turned off, with a header. Its own name is not "taken" by itself.
    let mut changed = sentry.clone();
    changed["id"] = json!(id);
    changed["on"] = json!(false);
    changed["headers"] = json!([["Authorization", "Bearer abc"]]);
    let after = asked(&state, "tools.save", changed).await.unwrap();
    assert_eq!((after["servers"][0]["on"].as_bool(), after["servers"].as_array().unwrap().len()), (Some(false), 1));
    assert_eq!(after["servers"][0]["headers"], json!([["Authorization", "Bearer abc"]]));

    // It is there after the core is started again, and gone once it is removed.
    let again = AppState::new(sandbox.manager());
    assert_eq!(asked(&again, "tools.list", json!({})).await.unwrap()["servers"][0]["name"], "Sentry");
    assert_eq!(asked(&again, "tools.remove", json!({ "id": id })).await.unwrap()["servers"], json!([]));
    assert!(asked(&again, "tools.remove", json!({ "id": id })).await.is_err());
}

#[tokio::test]
async fn every_agent_that_starts_is_named_the_servers_it_can_reach() {
    let sandbox = Sandbox::new("toolservers-given");
    let state = AppState::new(sandbox.manager());
    let manager = &state.manager;
    let project = manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    for server in [
        json!({ "name": "Notion", "kind": "http", "url": "https://mcp.notion.com/mcp", "headers": [["Authorization", "Bearer abc"]] }),
        json!({ "name": "Db", "kind": "stdio", "program": "ae-no-such-program", "args": ["--read-only"], "env": [["DATABASE_URL", "postgres://secret"]] }),
        // The practice agent takes none over SSE: this one is left out, and said to be.
        json!({ "name": "Old", "kind": "sse", "url": "http://localhost:1/sse" }),
        json!({ "name": "Off", "kind": "http", "url": "https://off.example/mcp", "on": false }),
    ] {
        asked(&state, "tools.save", server).await.unwrap();
    }

    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock", "What does the limiter do?").await.unwrap();
    let id = task.row.id.clone();
    let seen = until(&mut events, &id, |event| state_is(event, "idle")).await;
    let started = seen.iter().find(|event| is(event, "agent_started")).expect("the agent started");
    assert_eq!(
        started["toolServers"],
        json!({ "given": ["Notion", "Db"],
                "leftOut": [{ "name": "Old", "why": "this agent takes no tool servers over SSE" }] })
    );
    // The agent says them back as it was handed them: how each is reached, and the names of what it is
    // handed. The one that is off is not among them, and the editor's own tools are not one of the user's.
    let said = answer(&state, &id, "Which tool servers do you have?").await;
    assert_eq!(
        said,
        "This session was given these tool servers: Notion (http https://mcp.notion.com/mcp; headers: Authorization); \
         Db (runs ae-no-such-program --read-only; env: DATABASE_URL)."
    );

    // A change holds for the sessions that open from then on: this one, woken again, has the new list.
    let listed = asked(&state, "tools.list", json!({})).await.unwrap();
    let notion = listed["servers"][0]["id"].as_str().unwrap().to_string();
    asked(&state, "tools.remove", json!({ "id": notion })).await.unwrap();
    assert!(
        answer(&state, &id, "Which tool servers do you have?").await.contains("Notion"),
        "the running one goes on as it was"
    );
    manager.stop_agent(&id).await.unwrap();
    let said = answer(&state, &id, "Which tool servers do you have?").await;
    assert_eq!(
        said,
        "This session was given these tool servers: Db (runs ae-no-such-program --read-only; env: DATABASE_URL)."
    );
    manager.stop_all().await;
}
