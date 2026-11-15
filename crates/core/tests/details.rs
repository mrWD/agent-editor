//! An issue and a merge request in full, and the pictures in them.
//!
//! `gh` and `glab` are stand-ins that answer from files; Linear is a small server of the test's own
//! that takes one key. No real tracker is reached.
#![cfg(unix)]

mod common;

use ae_core::hosts::Hosts;
use ae_core::server::{AppState, dispatch};
use axum::Router;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{get, post};
use common::*;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use tokio::net::TcpListener;

/// The smallest thing that begins as a PNG does.
const PNG: [u8; 8] = [0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];

async fn asked(state: &AppState, method: &str, params: Value) -> anyhow::Result<Value> {
    dispatch(state, method, &params).await
}

fn missing() -> PathBuf {
    PathBuf::from("/nonexistent/ae-test/tool")
}

fn calls(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("calls.log")).unwrap_or_default()
}

#[tokio::test]
async fn an_issue_of_github_is_read_whole_and_a_pull_request_with_what_reviewers_said() {
    let sandbox = Sandbox::new("details-github");
    let tools = sandbox.0.join("tools");
    std::fs::create_dir_all(&tools).unwrap();
    std::fs::write(
        tools.join("issue.json"),
        r#"{"number":12,"title":"Login allows unlimited attempts","body":"Five a minute is enough.","state":"OPEN",
            "author":{"login":"ann"},"assignees":[],"labels":[{"name":"auth"}],"createdAt":"2026-11-10T08:00:00Z",
            "updatedAt":"2026-11-12T19:05:30Z","url":"https://github.com/x/shop/issues/12",
            "comments":[{"author":{"login":"bob"},"body":"Agreed.","createdAt":"2026-11-11T10:00:00Z"}]}"#,
    )
    .unwrap();
    // One answer serves both questions about the pull request: the short one and the whole one.
    std::fs::write(
        tools.join("pull.json"),
        r#"{"number":128,"title":"Limit logins","body":"Closes #12.","url":"https://github.com/x/shop/pull/128","state":"OPEN",
            "isDraft":false,"mergeable":"MERGEABLE","reviewDecision":"","statusCheckRollup":[],"author":{"login":"ann"},
            "assignees":[],"reviewRequests":[{"login":"cy"}],"labels":[],"createdAt":"2026-11-10T08:00:00Z",
            "updatedAt":"2026-11-12T19:05:30Z","baseRefName":"main","headRefName":"task/limit",
            "comments":[],"reviews":[{"author":{"login":"cy"},"state":"APPROVED","body":"Good.","submittedAt":"2026-11-12T09:00:00Z"}]}"#,
    )
    .unwrap();
    let gh = fake_tool(
        &tools,
        "gh",
        r#"here="$(dirname "$0")"
echo "$@" >> "$here/calls.log"
case "$1 $2" in
  "issue view") if [ "$3" = "12" ]; then cat "$here/issue.json"; else echo "GraphQL: Could not resolve to an Issue with the number of $3." >&2; exit 1; fi ;;
  "pr view") cat "$here/pull.json" ;;
  *) echo "unexpected call: $*" >&2; exit 2 ;;
esac
"#,
    );
    let hosts = Hosts::with(gh, missing(), missing(), "http://127.0.0.1:9/graphql".into());
    let state = AppState::new(sandbox.manager_with(hosts));
    let repo = git_project(&sandbox);
    git(&repo, &["remote", "add", "origin", "https://github.com/x/shop.git"]);
    let project = state.manager.add_project(&repo.to_string_lossy(), None).unwrap();

    let issue = asked(&state, "issues.detail", json!({ "projectId": project.id, "key": "#12" })).await.unwrap();
    assert_eq!(
        (issue["source"].as_str(), issue["key"].as_str(), issue["author"].as_str()),
        (Some("github"), Some("#12"), Some("ann"))
    );
    assert_eq!(issue["body"], "Five a minute is enough.");
    assert_eq!(issue["notes"][0]["body"], "Agreed.");
    assert!(calls(&tools).contains("issue view 12 -R x/shop --json number,title,body"));
    // What is not there is said in the tracker's own words, and what is not a number is not asked for.
    let gone = asked(&state, "issues.detail", json!({ "projectId": project.id, "key": "#99" })).await.unwrap_err();
    assert!(gone.to_string().contains("Could not resolve to an Issue"), "{gone}");
    let odd = asked(&state, "issues.detail", json!({ "projectId": project.id, "key": "APP-1" })).await.unwrap_err();
    assert_eq!(odd.to_string(), "APP-1 is not the number of an issue");

    // The request of a task: found by its branch, then read whole by its number.
    let mut events = state.manager.subscribe();
    let task = state.manager.create(&project.id, "mock", "What is here?").await.unwrap();
    until(&mut events, &task.row.id, |e| state_is(e, "idle")).await;
    let request = asked(&state, "mr.detail", json!({ "taskId": task.row.id })).await.unwrap();
    assert_eq!(
        (request["key"].as_str(), request["branch"].as_str(), request["base"].as_str()),
        (Some("#128"), Some("task/limit"), Some("main"))
    );
    assert_eq!(request["reviewers"], json!(["cy"]));
    assert_eq!(
        request["notes"][0],
        json!({ "author": "cy", "at": 1794474000000u64, "body": "Good.", "kind": "review", "about": "approved" })
    );
    assert!(calls(&tools).contains("pr view 128 -R x/shop --json number,title,body,state,isDraft"));
}

#[tokio::test]
async fn an_issue_of_gitlab_is_read_through_its_api_and_its_picture_comes_through_the_core() {
    let sandbox = Sandbox::new("details-gitlab");
    let tools = sandbox.0.join("tools");
    std::fs::create_dir_all(&tools).unwrap();
    std::fs::write(
        tools.join("issue.json"),
        r#"{"iid":7,"title":"Wrong total","description":"See ![shot](/uploads/0123456789abcdef0123456789abcdef/shot.png)","state":"opened",
            "author":{"name":"Ann Lee"},"assignees":[{"name":"Bob"}],"labels":["bug"],"created_at":"2026-11-10T08:00:00.000Z",
            "updated_at":"2026-11-12T19:05:30.000Z","web_url":"https://gitlab.example.com/g/shop/-/issues/7"}"#,
    )
    .unwrap();
    std::fs::write(
        tools.join("notes.json"),
        r#"[{"body":"changed the description","system":true,"author":{"name":"Ann Lee"},"created_at":"2026-11-10T09:00:00.000Z"},
            {"body":"Seen on staging too.","system":false,"author":{"name":"Cy"},"created_at":"2026-11-11T09:00:00.000Z"}]"#,
    )
    .unwrap();
    std::fs::write(tools.join("shot.png"), PNG).unwrap();
    std::fs::write(tools.join("page.html"), "<!DOCTYPE html><script>alert(1)</script>").unwrap();
    let glab = fake_tool(
        &tools,
        "glab",
        r#"here="$(dirname "$0")"
echo "[$GITLAB_HOST] $*" >> "$here/calls.log"
case "$4" in
  projects/g%2Fshop/issues/7) cat "$here/issue.json" ;;
  projects/g%2Fshop/issues/7/notes\?*) cat "$here/notes.json" ;;
  projects/g%2Fshop/uploads/0123456789abcdef0123456789abcdef/shot.png) cat "$here/shot.png" ;;
  projects/g%2Fshop/uploads/0123456789abcdef0123456789abcdef/page.png) cat "$here/page.html" ;;
  *) echo '{"message":"404 Not found"}' >&2; exit 1 ;;
esac
"#,
    );
    let hosts = Hosts::with(missing(), glab, missing(), "http://127.0.0.1:9/graphql".into());
    let state = AppState::new(sandbox.manager_with(hosts));
    let repo = git_project(&sandbox);
    git(&repo, &["remote", "add", "origin", "git@gitlab.example.com:g/shop.git"]);
    let project = state.manager.add_project(&repo.to_string_lossy(), None).unwrap();

    let issue = asked(&state, "issues.detail", json!({ "projectId": project.id, "key": "#7" })).await.unwrap();
    assert_eq!(
        (issue["source"].as_str(), issue["key"].as_str(), issue["state"].as_str()),
        (Some("gitlab"), Some("#7"), Some("opened"))
    );
    assert_eq!(issue["assignees"], json!(["Bob"]));
    assert_eq!(issue["notes"].as_array().unwrap().len(), 1, "what GitLab notes by itself is not a comment");
    assert_eq!(issue["notes"][0]["body"], "Seen on staging too.");
    assert!(
        calls(&tools).contains(
            "[gitlab.example.com] api --hostname gitlab.example.com projects/g%2Fshop/issues/7/notes?sort=asc"
        )
    );

    // The picture of the description is asked of GitLab's API and handed on as what it is.
    let manager = state.manager.clone();
    let (bytes, kind) =
        manager.tracker_picture(&project.id, "/uploads/0123456789abcdef0123456789abcdef/shot.png").unwrap();
    assert_eq!((bytes, kind), (PNG.to_vec(), "image/png"));
    // What is not a picture is not handed on, whatever it is called; another address is not asked at all.
    let page = manager.tracker_picture(&project.id, "/uploads/0123456789abcdef0123456789abcdef/page.png").unwrap_err();
    assert_eq!(page.to_string(), "What the tracker gave is not a picture that can be shown");
    let before = calls(&tools);
    let far = manager.tracker_picture(&project.id, "https://evil.example.com/g/shop/uploads/abc/shot.png").unwrap_err();
    assert_eq!(far.to_string(), "That picture is not kept by the project's tracker, so it is not fetched");
    assert_eq!(calls(&tools), before, "nothing was asked");
}

#[tokio::test]
async fn an_issue_of_linear_is_read_with_its_comments_and_its_picture_needs_the_key() {
    let sandbox = Sandbox::new("details-linear");
    let app = Router::new()
        .route(
            "/graphql",
            post(|headers: HeaderMap, body: String| async move {
                if headers.get("authorization").and_then(|value| value.to_str().ok()) != Some("lin_test_key") {
                    return (StatusCode::UNAUTHORIZED, axum::Json(json!({})));
                }
                let question: Value = serde_json::from_str(&body).unwrap();
                if !question["query"].as_str().unwrap().contains("comments(first: 100)") {
                    return (StatusCode::OK, axum::Json(json!({ "data": { "viewer": { "name": "Ann" } } })));
                }
                let found = question["variables"]["id"] == "APP-214";
                let issue = json!({
                    "identifier": "APP-214", "title": "Unreachable UI", "url": "https://linear.app/x/issue/APP-214",
                    "description": "Remove it.", "createdAt": "2026-11-10T08:00:00.000Z", "updatedAt": "2026-11-12T19:05:30.000Z",
                    "state": { "name": "Todo" }, "creator": { "name": "Ann" }, "assignee": null, "labels": { "nodes": [] },
                    "attachments": { "nodes": [{ "title": "Design", "url": "https://figma.com/file/1" }] },
                    "comments": { "nodes": [{ "body": "Agreed", "createdAt": "2026-11-11T08:00:00.000Z", "user": { "name": "Cy" } }] }
                });
                let answer = if found { json!({ "data": { "issue": issue } }) } else { json!({ "errors": [{ "message": "Entity not found: Issue" }] }) };
                (StatusCode::OK, axum::Json(answer))
            }),
        )
        .route(
            "/uploads/a/shot.png",
            get(|headers: HeaderMap| async move {
                match headers.get("authorization").and_then(|value| value.to_str().ok()) == Some("lin_test_key") {
                    true => (StatusCode::OK, PNG.to_vec()),
                    false => (StatusCode::UNAUTHORIZED, Vec::new()),
                }
            }),
        );
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let origin = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

    let hosts = Hosts::with(missing(), missing(), missing(), format!("{origin}/graphql"));
    let state = AppState::new(sandbox.manager_with(hosts));
    let project = state.manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    let ask = json!({ "projectId": project.id, "key": "APP-214", "source": "linear" });

    // Without a key nothing is asked.
    let not_yet = asked(&state, "issues.detail", ask.clone()).await.unwrap_err();
    assert_eq!(not_yet.to_string(), "Linear is not connected yet. Paste a key in Issues.");
    state.manager.set_linear_key(Some("lin_test_key")).unwrap();

    let issue = asked(&state, "issues.detail", ask).await.unwrap();
    assert_eq!(
        (issue["source"].as_str(), issue["author"].as_str(), issue["state"].as_str()),
        (Some("linear"), Some("Ann"), Some("Todo"))
    );
    assert_eq!(issue["links"], json!([{ "title": "Design", "url": "https://figma.com/file/1" }]));
    assert_eq!(issue["notes"][0]["author"], "Cy");
    let gone =
        asked(&state, "issues.detail", json!({ "projectId": project.id, "key": "APP-999", "source": "linear" })).await;
    assert_eq!(gone.unwrap_err().to_string(), "Linear refused: Entity not found: Issue");

    // A picture kept by Linear is fetched with the key; one kept anywhere else is not fetched.
    let manager = state.manager.clone();
    let src = format!("{origin}/uploads/a/shot.png");
    let fetched =
        tokio::task::spawn_blocking(move || manager.tracker_picture(&project.id, &src)).await.unwrap().unwrap();
    assert_eq!(fetched, (PNG.to_vec(), "image/png"));
}
