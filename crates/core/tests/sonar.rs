//! What SonarQube found, on Health.
//!
//! "SonarQube" is a small server of the test's own that takes one token. The
//! real one is never reached.

mod common;

use ae_core::server::{AppState, dispatch};
use axum::Router;
use axum::extract::Query;
use axum::http::{HeaderMap, StatusCode};
use axum::routing::get;
use base64::Engine;
use common::*;
use serde_json::{Value, json};
use std::collections::HashMap;
use tokio::net::TcpListener;

/// Whether the request shows the token the way SonarQube takes one: as a user's name, with no password.
fn allowed(headers: &HeaderMap) -> bool {
    let wanted = format!("Basic {}", base64::engine::general_purpose::STANDARD.encode("squ_good:"));
    headers.get("authorization").and_then(|value| value.to_str().ok()) == Some(wanted.as_str())
}

type Asked = Query<HashMap<String, String>>;

/// Starts the stand-in and gives its address. It knows one project, "acme:shop".
async fn sonar() -> String {
    let known = |asked: &HashMap<String, String>, name: &str| asked.get(name).map(String::as_str) == Some("acme:shop");
    let app = Router::new()
        .route("/api/authentication/validate", get(|headers: HeaderMap| async move { axum::Json(json!({ "valid": allowed(&headers) })) }))
        .route("/api/qualitygates/project_status", get(move |Query(asked): Asked, headers: HeaderMap| async move {
            if !allowed(&headers) {
                return (StatusCode::UNAUTHORIZED, axum::Json(json!({})));
            }
            if !known(&asked, "projectKey") {
                return (StatusCode::NOT_FOUND, axum::Json(json!({ "errors": [{ "msg": "Project not found" }] })));
            }
            (StatusCode::OK, axum::Json(json!({ "projectStatus": { "status": "ERROR", "conditions": [
                { "status": "ERROR", "metricKey": "new_coverage", "comparator": "LT", "actualValue": "61.2", "errorThreshold": "80" }
            ] } })))
        }))
        .route("/api/issues/search", get(move |Query(asked): Asked| async move {
            // The worst first, and only what is still open, as it was asked for.
            assert_eq!(asked.get("issueStatuses").map(String::as_str), Some("OPEN,CONFIRMED"));
            assert_eq!((asked.get("s").map(String::as_str), asked.get("asc").map(String::as_str)), (Some("SEVERITY"), Some("false")));
            assert!(known(&asked, "components"));
            axum::Json(json!({ "paging": { "total": 41 }, "issues": [
                { "severity": "CRITICAL", "type": "BUG", "rule": "typescript:S2259", "component": "acme:shop:src/checkout/submit.ts", "line": 42, "message": "cart.user can be undefined here." },
                { "severity": "MAJOR", "type": "CODE_SMELL", "rule": "typescript:S1128", "component": "acme:shop:src/app.ts", "line": 3, "message": "Remove this unused import." }
            ] }))
        }))
        .route("/api/hotspots/search", get(|| async { axum::Json(json!({ "paging": { "total": 2 }, "hotspots": [] })) }));
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = format!("http://{}", listener.local_addr().unwrap());
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    address
}

async fn asked(state: &AppState, method: &str, params: Value) -> anyhow::Result<Value> {
    dispatch(state, method, &params).await
}

#[tokio::test]
async fn what_sonarqube_found_is_read_on_check_now_and_shown_as_findings_to_fix() {
    let sandbox = Sandbox::new("sonar");
    let state = AppState::new(sandbox.manager());
    let url = sonar().await;
    let project_dir = git_project(&sandbox);
    std::fs::write(project_dir.join("sonar-project.properties"), "sonar.projectKey=acme:shop\nsonar.sources=src\n")
        .unwrap();
    let project = state.manager.add_project(&project_dir.to_string_lossy(), None).unwrap();
    let of_sonar = |health: &Value| -> Vec<String> {
        let list = health["findings"].as_array().unwrap().iter();
        list.filter(|one| one["id"].as_str().unwrap().contains(":sonar-"))
            .map(|one| one["title"].as_str().unwrap().to_string())
            .collect()
    };

    // Not connected: Health says so, the key of the project is known from its own files, and nothing is asked.
    let before = asked(&state, "health.get", json!({})).await.unwrap();
    assert_eq!(
        before["sonar"],
        json!({ "connected": false, "url": "", "checkedAt": null, "projects": [{ "projectId": project.id, "key": "acme:shop", "from": "file" }] })
    );
    assert!(of_sonar(&before).is_empty());

    // A token it does not take is not kept; an address that is none is refused before anything is asked.
    let wrong = asked(&state, "sonar.connect", json!({ "url": url, "token": "squ_bad" })).await;
    assert_eq!(wrong.unwrap_err().to_string(), "SonarQube did not accept the token.");
    let nowhere = asked(&state, "sonar.connect", json!({ "url": "sonar.example.com", "token": "squ_good" })).await;
    assert!(nowhere.unwrap_err().to_string().starts_with("Give the address of your SonarQube"));
    let connected =
        asked(&state, "sonar.connect", json!({ "url": format!("{url}/"), "token": "squ_good" })).await.unwrap();
    assert_eq!((connected["connected"].as_bool(), connected["url"].as_str()), (Some(true), Some(url.as_str())));
    // Connecting alone asks for nothing yet: the findings come with "Check now".
    assert!(of_sonar(&asked(&state, "health.get", json!({})).await.unwrap()).is_empty());

    let checked = asked(&state, "health.audit", json!({})).await.unwrap();
    assert_eq!(
        of_sonar(&checked),
        [
            "2 places wait for a security review in SonarQube",
            "The quality gate of SonarQube fails",
            "SonarQube has 41 open findings",
        ],
        "what is about security comes first"
    );
    assert!(checked["sonar"]["checkedAt"].as_u64().is_some());
    let findings = checked["findings"].as_array().unwrap();
    let open = findings.iter().find(|one| one["title"] == "SonarQube has 41 open findings").unwrap();
    assert_eq!(open["category"], "Quality");
    assert_eq!(
        open["detail"],
        "src/checkout/submit.ts:42: cart.user can be undefined here. (typescript:S2259, critical) src/app.ts:3: Remove this unused import. (typescript:S1128, major) And 39 more."
    );
    let fix = open["fix"].as_str().unwrap();
    assert!(
        fix.contains("- src/checkout/submit.ts:42: cart.user can be undefined here. (typescript:S2259, critical)"),
        "{fix}"
    );
    assert!(fix.contains("39 more are open in SonarQube; these are the worst."), "{fix}");

    // A key the user gives stands before the one in the files. One SonarQube does not know says so, on Health.
    let set = asked(&state, "sonar.key", json!({ "projectId": project.id, "key": "acme:other" })).await.unwrap();
    assert_eq!(set["projects"][0], json!({ "projectId": project.id, "key": "acme:other", "from": "set" }));
    assert!(asked(&state, "sonar.key", json!({ "projectId": project.id, "key": "a b" })).await.is_err());
    let unread = asked(&state, "health.audit", json!({})).await.unwrap();
    assert_eq!(of_sonar(&unread), ["SonarQube could not be read for this project"]);
    let said = unread["findings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|one| one["id"].as_str().unwrap().ends_with(":sonar-unread"))
        .unwrap();
    assert_eq!(said["detail"], "SonarQube knows no project by that key.");
    // An empty key goes back to the project's own.
    let back = asked(&state, "sonar.key", json!({ "projectId": project.id, "key": "" })).await.unwrap();
    assert_eq!(back["projects"][0]["from"], "file");

    // A finding can be ignored like any other, and forgetting the token takes all of SonarQube's away.
    let id = open["id"].as_str().unwrap();
    asked(&state, "health.audit", json!({})).await.unwrap();
    asked(&state, "health.ignore", json!({ "id": id })).await.unwrap();
    assert_eq!(of_sonar(&asked(&state, "health.get", json!({})).await.unwrap()).len(), 2);
    let gone = asked(&state, "sonar.connect", json!({ "token": "" })).await.unwrap();
    assert_eq!((gone["connected"].as_bool(), gone["checkedAt"].is_null()), (Some(false), true));
    assert!(of_sonar(&asked(&state, "health.get", json!({})).await.unwrap()).is_empty());
}
