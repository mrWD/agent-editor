//! Errors from Sentry as tasks.
//!
//! "Sentry" is a small server of the test's own that answers as Sentry does
//! and takes one token. The real one is never reached, and the practice agent
//! takes the tasks.

mod common;

use ae_core::server::{AppState, dispatch};
use axum::Router;
use axum::extract::Path;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;
use common::*;
use serde_json::{Value, json};
use tokio::net::TcpListener;

fn allowed(headers: &HeaderMap) -> bool {
    headers.get("authorization").and_then(|value| value.to_str().ok()) == Some("Bearer sntryu_good")
}

fn issue() -> Value {
    json!({
        "id": "4001", "shortId": "SHOP-1A", "title": "TypeError: cart.user is undefined",
        "culprit": "submit(src/checkout/submit)", "level": "error", "count": "1204", "userCount": 87,
        "firstSeen": "2026-11-01T08:00:00Z", "lastSeen": "2026-11-13T17:40:12Z",
        "permalink": "https://acme.sentry.io/issues/4001/"
    })
}

/// Answers `body` to whoever shows the token, and "no" to anybody else.
fn guarded(headers: HeaderMap, body: Value) -> (StatusCode, axum::Json<Value>) {
    match allowed(&headers) {
        true => (StatusCode::OK, axum::Json(body)),
        false => (StatusCode::UNAUTHORIZED, axum::Json(json!({ "detail": "Invalid token" }))),
    }
}

/// Starts the stand-in and gives its address.
async fn sentry() -> String {
    let app = Router::new()
        .route("/api/0/organizations/{org}/", get(|Path(org): Path<String>, headers: HeaderMap| async move {
            if org != "acme" {
                return (StatusCode::NOT_FOUND, axum::Json(json!({ "detail": "The requested resource does not exist" })));
            }
            guarded(headers, json!({ "slug": "acme" }))
        }))
        .route("/api/0/organizations/acme/projects/", get(|headers: HeaderMap| async move {
            guarded(headers, json!([{ "slug": "shop-web", "name": "Shop web" }, { "slug": "shop-api", "name": "Shop API" }]))
        }))
        .route("/api/0/projects/acme/shop-web/issues/", get(|headers: HeaderMap| async move {
            let second = json!({ "id": "4002", "shortId": "SHOP-1B", "title": "Slow page", "level": "warning", "count": 3, "userCount": 0,
                                 "firstSeen": "2026-11-12T08:00:00Z", "lastSeen": "2026-11-12T09:00:00Z", "permalink": "https://acme.sentry.io/issues/4002/" });
            guarded(headers, json!([issue(), second]))
        }))
        .route("/api/0/organizations/acme/issues/4001/", get(|headers: HeaderMap| async move { guarded(headers, issue()) }))
        .route("/api/0/organizations/acme/issues/4001/events/latest/", get(|headers: HeaderMap| async move {
            guarded(headers, json!({
                "tags": [{ "key": "release", "value": "1.4.2" }],
                "entries": [{ "type": "exception", "data": { "values": [{
                    "type": "TypeError", "value": "cart.user is undefined",
                    "stacktrace": { "frames": [
                        { "filename": "node_modules/express/lib/router.js", "function": "handle", "lineNo": 280, "inApp": false },
                        { "filename": "src/checkout/submit.ts", "function": "submit", "lineNo": 42, "colNo": 17, "inApp": true,
                          "context": [[41, "  const cart = await load();"], [42, "  return cart.user.id;"], [43, "}"]] }
                    ] }
                }] } }]
            }))
        }));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    address
}

async fn asked(state: &AppState, method: &str, params: Value) -> anyhow::Result<Value> {
    dispatch(state, method, &params).await
}

#[tokio::test]
async fn sentry_is_connected_with_a_token_that_was_tried_and_a_project_is_joined_to_one_of_its_own() {
    let sandbox = Sandbox::new("sentry-connect");
    let state = AppState::new(sandbox.manager());
    let url = sentry().await;
    let project = state.manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    let before = asked(&state, "sentry.status", json!({})).await.unwrap();
    assert_eq!(before, json!({ "connected": false, "url": "https://sentry.io", "org": "", "projects": {} }));

    // A token Sentry does not take is not kept, and neither is one for an organization it does not know.
    let wrong = asked(&state, "sentry.connect", json!({ "token": "sntryu_bad", "org": "acme", "url": url })).await;
    assert_eq!(
        wrong.unwrap_err().to_string(),
        "Sentry did not accept the token. Make a new one under User settings, Auth tokens."
    );
    let nowhere = asked(&state, "sentry.connect", json!({ "token": "sntryu_good", "org": "nobody", "url": url })).await;
    assert!(nowhere.unwrap_err().to_string().starts_with("Sentry knows nothing by that name"));
    let odd = asked(&state, "sentry.connect", json!({ "token": "sntryu_good", "org": "a/b", "url": url })).await;
    assert!(odd.unwrap_err().to_string().starts_with("Give the organization"));
    assert_eq!(asked(&state, "sentry.status", json!({})).await.unwrap()["connected"], false);

    let connected =
        asked(&state, "sentry.connect", json!({ "token": " sntryu_good ", "org": "acme", "url": format!("{url}/") }))
            .await
            .unwrap();
    assert_eq!(
        connected,
        json!({ "connected": true, "url": url, "org": "acme", "projects": {} }),
        "the token is never said back"
    );

    // Its projects are there to pick from, and one is joined to the editor's project.
    let projects = asked(&state, "sentry.projects", json!({})).await.unwrap();
    assert_eq!(
        projects["projects"],
        json!([{ "slug": "shop-web", "name": "Shop web" }, { "slug": "shop-api", "name": "Shop API" }])
    );
    let unjoined = asked(&state, "sentry.issues", json!({ "projectId": project.id })).await.unwrap();
    assert_eq!(
        unjoined,
        json!({ "slug": null, "issues": [] }),
        "a project that is joined to nothing has nothing to list"
    );
    let joined = asked(&state, "sentry.join", json!({ "projectId": project.id, "slug": "shop-web" })).await.unwrap();
    assert_eq!(joined["projects"], json!({ project.id.clone(): "shop-web" }));
    assert!(asked(&state, "sentry.join", json!({ "projectId": project.id, "slug": "a b" })).await.is_err());

    // Kept across a start of the core; an empty token takes everything about Sentry away but the joins.
    let again = AppState::new(sandbox.manager());
    assert_eq!(asked(&again, "sentry.status", json!({})).await.unwrap()["connected"], true);
    let gone = asked(&again, "sentry.connect", json!({ "token": "" })).await.unwrap();
    assert_eq!((gone["connected"].as_bool(), gone["org"].as_str()), (Some(false), Some("")));
    let listing = asked(&again, "sentry.issues", json!({ "projectId": project.id })).await;
    assert_eq!(listing.unwrap_err().to_string(), "Sentry is not connected yet");
}

#[tokio::test]
async fn an_issue_of_sentry_becomes_a_task_whose_first_message_is_the_issue_with_its_newest_event() {
    let sandbox = Sandbox::new("sentry-start");
    let state = AppState::new(sandbox.manager());
    let manager = &state.manager;
    let url = sentry().await;
    let project = manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    asked(&state, "sentry.connect", json!({ "token": "sntryu_good", "org": "acme", "url": url })).await.unwrap();
    asked(&state, "sentry.join", json!({ "projectId": project.id, "slug": "shop-web" })).await.unwrap();

    // As Sentry orders them, with the numbers as numbers and no task on any yet.
    let listed = asked(&state, "sentry.issues", json!({ "projectId": project.id })).await.unwrap();
    assert_eq!(listed["slug"], "shop-web");
    let rows = listed["issues"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(
        (rows[0]["key"].as_str(), rows[0]["count"].as_u64(), rows[0]["users"].as_u64(), rows[0]["taskId"].is_null()),
        (Some("SHOP-1A"), Some(1204), Some(87), true)
    );

    let mut events = manager.subscribe();
    let started =
        asked(&state, "sentry.start", json!({ "projectId": project.id, "issueId": "4001", "agentId": "mock" }))
            .await
            .unwrap();
    let task_id = started["id"].as_str().unwrap().to_string();
    assert_eq!(started["title"], "TypeError: cart.user is undefined");
    assert_eq!(
        started["issue"],
        json!({ "key": "SHOP-1A", "id": "4001", "url": "https://acme.sentry.io/issues/4001/",
                "title": "TypeError: cart.user is undefined", "source": "sentry" })
    );
    // The message asks for a fix, so the practice agent stops to ask before it edits: that is far enough.
    until(&mut events, &task_id, |event| is(event, "permission_request") || state_is(event, "idle")).await;
    let history = manager.history(&task_id).unwrap();
    let said = history.iter().find(|event| is(event, "user_message")).unwrap()["text"].as_str().unwrap().to_string();
    assert!(said.starts_with("TypeError: cart.user is undefined\n\nThis is Sentry issue SHOP-1A"), "{said}");
    assert!(said.contains("It happened 1204 times to 87 users, first on 2026-11-01 and last on 2026-11-13."), "{said}");
    assert!(said.contains("Release: 1.4.2."), "{said}");
    assert!(said.contains("  at submit (src/checkout/submit.ts:42:17)\n           41 |   const cart = await load();\n      >    42 |   return cart.user.id;"), "{said}");
    assert!(said.contains("(1 more frame of libraries and of the app left out)"), "{said}");

    // The issue is listed with its task now, and is not started twice.
    let listed = asked(&state, "sentry.issues", json!({ "projectId": project.id })).await.unwrap();
    assert_eq!(listed["issues"][0]["taskId"], task_id.as_str());
    assert!(listed["issues"][1]["taskId"].is_null());
    let twice =
        asked(&state, "sentry.start", json!({ "projectId": project.id, "issueId": "4001", "agentId": "mock" })).await;
    assert_eq!(twice.unwrap_err().to_string(), "A task for that issue of Sentry is already open");
    // What is no number of an issue never reaches an address.
    let odd =
        asked(&state, "sentry.start", json!({ "projectId": project.id, "issueId": "../../users", "agentId": "mock" }))
            .await;
    assert_eq!(odd.unwrap_err().to_string(), "That is not the number of an issue of Sentry");
    manager.stop_all().await;
}
