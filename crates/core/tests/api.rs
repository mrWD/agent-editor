//! The API of a task's service: its description, a request to try, and what the task changed.
//!
//! "The service" is a small server of the test's own. It describes itself at
//! `/openapi.json` the way FastAPI does, and answers a few requests.

mod common;

use ae_core::server::{AppState, dispatch};
use axum::Router;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use common::*;
use serde_json::{Value, json};
use tokio::net::TcpListener;

/// The description of the shop's API. `with_gift` is how the task's branch has it: a field more
/// is needed to place an order, an order no longer says its total, and there is a new operation.
fn description(with_gift: bool) -> Value {
    let mut order = json!({
        "type": "object", "required": ["id", "total"],
        "properties": { "id": { "type": "integer" }, "total": { "type": "number" } }
    });
    let mut placing = json!({
        "type": "object", "required": ["sku"],
        "properties": { "sku": { "type": "string", "example": "tea-1" } }
    });
    let mut paths = json!({
        "/health": { "get": { "summary": "Whether it is up", "responses": { "200": {} } } },
        "/orders": { "post": {
            "summary": "Place an order",
            "requestBody": { "required": true, "content": { "application/json": { "schema": placing.clone() } } },
            "responses": { "201": { "content": { "application/json": { "schema": order.clone() } } } }
        } }
    });
    if with_gift {
        order["required"] = json!(["id"]);
        order["properties"].as_object_mut().unwrap().remove("total");
        placing["required"] = json!(["sku", "count"]);
        placing["properties"]["count"] = json!({ "type": "integer", "minimum": 1 });
        paths["/orders"]["post"]["requestBody"]["content"]["application/json"]["schema"] = placing;
        paths["/orders"]["post"]["responses"]["201"]["content"]["application/json"]["schema"] = order;
        paths["/gifts"] = json!({ "get": { "summary": "What can be given", "responses": { "200": {} } } });
    }
    json!({ "openapi": "3.1.0", "info": { "title": "Shop", "version": "0.3" }, "paths": paths })
}

/// Starts the stand-in and gives its address, without the scheme.
async fn service(with_gift: bool) -> String {
    let app = Router::new()
        .route("/openapi.json", get(move || async move { axum::Json(description(with_gift)) }))
        .route("/health", get(|| async { axum::Json(json!({ "up": true })) }))
        .route(
            "/orders",
            post(|headers: HeaderMap, body: String| async move {
                if headers.get("authorization").and_then(|value| value.to_str().ok()) != Some("Bearer shop-key") {
                    return (StatusCode::UNAUTHORIZED, axum::Json(json!({ "detail": "Not signed in" })));
                }
                let asked: Value = serde_json::from_str(&body).unwrap_or(Value::Null);
                (StatusCode::CREATED, axum::Json(json!({ "id": 7, "sku": asked["sku"], "trace": headers.get("x-trace").and_then(|v| v.to_str().ok()) })))
            }),
        )
        .route("/moved", get(|| async { (StatusCode::FOUND, [("location", "/health")]) }))
        .route("/logo", get(|| async { ([("content-type", "image/png")], vec![0x89u8, b'P', b'N', b'G', 0, 1, 2]) }));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    address
}

async fn asked(state: &AppState, method: &str, params: Value) -> anyhow::Result<Value> {
    dispatch(state, method, &params).await
}

async fn idle_task(state: &AppState, project_id: &str) -> String {
    let mut events = state.manager.subscribe();
    let task = state.manager.create(project_id, "mock", "What is here?").await.unwrap();
    until(&mut events, &task.row.id, |e| state_is(e, "idle")).await;
    task.row.id
}

async fn state_with_task(sandbox: &Sandbox) -> (AppState, String) {
    let state = AppState::new(sandbox.manager());
    let project = state.manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    let task_id = idle_task(&state, &project.id).await;
    (state, task_id)
}

fn ids(found: &Value) -> Vec<&str> {
    found["operations"].as_array().unwrap().iter().map(|one| one["id"].as_str().unwrap()).collect()
}

#[tokio::test]
async fn the_running_service_of_a_task_is_asked_for_its_description() {
    let sandbox = Sandbox::new("api-service");
    let (state, task_id) = state_with_task(&sandbox).await;

    // Nothing runs and no file describes it: that is said, and nothing is made up.
    let nothing = asked(&state, "api.get", json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(nothing["operations"], json!([]));
    assert_eq!(
        nothing["note"],
        "No description of the API was found among the files, and no service of this task is running to ask."
    );

    // Of the services that run, the one that describes itself is the one requests go to.
    let address = service(false).await;
    let silent = "127.0.0.1:9";
    let found = asked(&state, "api.get", json!({ "taskId": task_id, "guess": [silent, address] })).await.unwrap();
    assert_eq!(found["address"], address);
    assert_eq!(found["source"], json!({ "kind": "service", "where": "/openapi.json" }));
    assert_eq!((found["title"].as_str(), found["version"].as_str()), (Some("Shop"), Some("0.3")));
    assert_eq!(ids(&found), ["GET /health", "POST /orders"]);
    let placing = &found["operations"][1];
    assert_eq!(placing["body"]["kind"], "application/json");
    assert_eq!(
        serde_json::from_str::<Value>(placing["body"]["example"].as_str().unwrap()).unwrap(),
        json!({ "sku": "tea-1" })
    );
    assert_eq!(found["contract"], Value::Null);
    assert!(found["contractNote"].as_str().unwrap().starts_with("To see what this task changes in the contract"));

    // An address the user gives is kept for the task, and only one on this computer is taken.
    let far = asked(&state, "api.set", json!({ "taskId": task_id, "address": "example.com:80" })).await;
    assert!(far.unwrap_err().to_string().contains("this computer"), "requests never leave this computer");
    asked(&state, "api.set", json!({ "taskId": task_id, "address": silent })).await.unwrap();
    let unreachable = asked(&state, "api.get", json!({ "taskId": task_id, "guess": [address] })).await.unwrap();
    assert_eq!(unreachable["address"], silent, "what the user gave is not guessed over");
    assert_eq!(
        unreachable["note"],
        "Nothing answers at 127.0.0.1:9. Start the service in Run, or give another address."
    );
    asked(&state, "api.set", json!({ "taskId": task_id, "address": "" })).await.unwrap();
    let again = asked(&state, "api.get", json!({ "taskId": task_id, "guess": [address] })).await.unwrap();
    assert_eq!(again["address"], address);
}

#[tokio::test]
async fn a_request_is_sent_to_the_service_and_the_answer_comes_back_whole() {
    let sandbox = Sandbox::new("api-send");
    let (state, task_id) = state_with_task(&sandbox).await;
    let address = service(false).await;
    let send = |method: &str, path: &str, extra: Value| {
        let mut params = json!({ "taskId": task_id, "address": address, "method": method, "path": path });
        params.as_object_mut().unwrap().extend(extra.as_object().cloned().unwrap_or_default());
        params
    };

    let up = asked(&state, "api.send", send("get", "/health", json!({}))).await.unwrap();
    assert_eq!(
        (up["status"].as_u64(), up["reason"].as_str(), up["kind"].as_str()),
        (Some(200), Some("OK"), Some("json"))
    );
    assert_eq!(serde_json::from_str::<Value>(up["body"].as_str().unwrap()).unwrap(), json!({ "up": true }));
    assert_eq!(up["sent"]["url"], format!("http://{address}/health"));
    assert!(up["took"].is_u64() && up["size"].as_u64() == Some(11));

    // An answer that says "no" is an answer, not a failure.
    let body = json!({ "body": "{\"sku\":\"tea-1\"}", "headers": [["Content-Type", "application/json"]] });
    let refused = asked(&state, "api.send", send("POST", "/orders", body.clone())).await.unwrap();
    assert_eq!(refused["status"], 401);
    assert_eq!(serde_json::from_str::<Value>(refused["body"].as_str().unwrap()).unwrap()["detail"], "Not signed in");

    // The headers kept for the project go with every request; one of the request's own says otherwise.
    asked(
        &state,
        "api.set",
        json!({ "taskId": task_id, "headers": [["Authorization", "Bearer shop-key"], ["X-Trace", "kept"], ["", ""]] }),
    )
    .await
    .unwrap();
    let kept = asked(&state, "api.get", json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(kept["settings"]["headers"], json!([["Authorization", "Bearer shop-key"], ["X-Trace", "kept"]]));
    let mut with_trace = body.clone();
    with_trace["headers"] = json!([["Content-Type", "application/json"], ["x-trace", "mine"]]);
    let placed = asked(&state, "api.send", send("POST", "/orders", with_trace)).await.unwrap();
    assert_eq!(placed["status"], 201);
    let answer: Value = serde_json::from_str(placed["body"].as_str().unwrap()).unwrap();
    assert_eq!(answer, json!({ "id": 7, "sku": "tea-1", "trace": "mine" }));

    // A redirect is shown, not followed; bytes that are not text are not shown as text.
    let moved = asked(&state, "api.send", send("GET", "/moved", json!({}))).await.unwrap();
    assert_eq!(moved["status"], 302);
    assert!(moved["headers"].as_array().unwrap().contains(&json!(["location", "/health"])));
    let logo = asked(&state, "api.send", send("GET", "/logo", json!({}))).await.unwrap();
    assert_eq!(
        (logo["kind"].as_str(), logo["body"].as_str(), logo["size"].as_u64()),
        (Some("binary"), Some(""), Some(7))
    );

    // What cannot be sent is said before anything is sent.
    for (method, path, address, words) in [
        ("GET", "health", address.as_str(), "starts with /"),
        ("GET", "/a b", address.as_str(), "starts with /"),
        ("TRACE", "/health", address.as_str(), "not a method"),
        ("GET", "/health", "example.com", "this computer"),
        ("GET", "/health", "127.0.0.1:9", "Nothing answers at 127.0.0.1:9"),
    ] {
        let params = json!({ "taskId": task_id, "address": address, "method": method, "path": path });
        let error = asked(&state, "api.send", params).await.unwrap_err().to_string();
        assert!(error.contains(words), "{method} {path} at {address}: {error}");
    }
}

#[tokio::test]
async fn the_contract_is_compared_with_a_service_that_runs_the_base() {
    let sandbox = Sandbox::new("api-base-service");
    let (state, task_id) = state_with_task(&sandbox).await;
    let (base, branch) = (service(false).await, service(true).await);

    asked(&state, "api.set", json!({ "taskId": task_id, "address": branch, "base": format!("http://{base}/") }))
        .await
        .unwrap();
    let found = asked(&state, "api.get", json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(ids(&found), ["GET /gifts", "POST /orders", "GET /health"], "what the task touched comes first");
    let marks: Vec<_> = found["operations"].as_array().unwrap().iter().map(|one| one["change"].clone()).collect();
    assert_eq!(marks, [json!("new"), json!("changed"), Value::Null]);
    assert_eq!(found["contract"]["against"], format!("http://{base}/"));
    assert_eq!(
        found["contract"]["changes"],
        json!([
            { "operation": "POST /orders", "level": "breaking", "text": "The body now needs `count`" },
            { "operation": "POST /orders", "level": "breaking", "text": "The 201 answer no longer has `total`" },
            { "operation": "GET /gifts", "level": "safe", "text": "It is new" }
        ])
    );

    // A base that does not answer leaves the list as it is and says why there is nothing to compare.
    asked(&state, "api.set", json!({ "taskId": task_id, "base": "127.0.0.1:9" })).await.unwrap();
    let alone = asked(&state, "api.get", json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(alone["contract"], Value::Null);
    assert!(
        alone["contractNote"]
            .as_str()
            .unwrap()
            .starts_with("Nothing answers at 127.0.0.1:9, where the base should run")
    );
    assert_eq!(ids(&alone), ["GET /gifts", "GET /health", "POST /orders"]);
    assert!(asked(&state, "api.set", json!({ "taskId": task_id, "base": "not an address" })).await.is_err());
}

#[tokio::test]
async fn a_description_kept_as_a_file_is_compared_with_the_file_where_the_branch_started() {
    let sandbox = Sandbox::new("api-file");
    let project = git_project(&sandbox);
    std::fs::create_dir_all(project.join("docs")).unwrap();
    std::fs::write(project.join("docs/openapi.json"), description(false).to_string()).unwrap();
    git(&project, &["add", "."]);
    git(&project, &["commit", "-q", "-m", "Describe the API"]);
    let (state, task_id) = state_with_task(&sandbox).await;
    let folder = state.manager.cwd(&task_id).unwrap();
    assert_ne!(folder, project, "the task works in a folder of its own");

    // Untouched, the file differs from its base in nothing.
    let same = asked(&state, "api.get", json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(same["source"], json!({ "kind": "file", "where": "docs/openapi.json" }));
    assert_eq!(same["contract"], json!({ "against": "main", "changes": [] }));
    assert_eq!(same["address"], Value::Null);
    assert!(same["note"].as_str().unwrap().starts_with("No service of this task is running"));

    // The task rewrites it, in YAML's place or not: what it changed is said without committing anything.
    std::fs::write(folder.join("docs/openapi.json"), description(true).to_string()).unwrap();
    let changed = asked(&state, "api.get", json!({ "taskId": task_id, "guess": ["127.0.0.1:9"] })).await.unwrap();
    assert_eq!(ids(&changed)[..2], ["GET /gifts", "POST /orders"]);
    assert_eq!(changed["contract"]["changes"].as_array().unwrap().len(), 3);
    assert_eq!(changed["address"], "127.0.0.1:9", "requests still have somewhere to go");

    // A file of the task's own, named by hand, is new as a whole; one that leads out of the folder is refused.
    std::fs::write(folder.join("fresh.yaml"), "openapi: 3.0.0\npaths:\n  /ping:\n    get:\n      responses: {}\n")
        .unwrap();
    asked(&state, "api.set", json!({ "taskId": task_id, "spec": "fresh.yaml" })).await.unwrap();
    let fresh = asked(&state, "api.get", json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(fresh["operations"][0]["change"], "new");
    assert!(asked(&state, "api.set", json!({ "taskId": task_id, "spec": "../other.yaml" })).await.is_err());
    asked(&state, "api.set", json!({ "taskId": task_id, "spec": "missing.yaml" })).await.unwrap();
    let missing = asked(&state, "api.get", json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(missing["note"], "There is no file missing.yaml in the task's folder");
}
