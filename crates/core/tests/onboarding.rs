//! The first run: the switch the core keeps, the programs it looks for, and
//! what it says about a folder that has just become a project.

mod common;

use ae_core::server::{AppState, dispatch};
use common::{Sandbox, git, git_project};
use serde_json::{Value, json};

async fn asked(state: &AppState, method: &str, params: Value) -> Value {
    dispatch(state, method, &params).await.unwrap_or_else(|error| panic!("{method} failed: {error:#}"))
}

#[tokio::test]
async fn the_first_run_is_ahead_until_it_is_done_and_stays_done_after_a_restart() {
    let sandbox = Sandbox::new("onboarding");
    let state = AppState::new(sandbox.manager());
    let mut events = state.manager.subscribe();

    assert_eq!(asked(&state, "app.info", json!({})).await["onboarded"], false);
    let first = asked(&state, "onboarding.get", json!({})).await;
    assert_eq!(first["done"], false);
    // Every program it looks for is named, found or not: the interface never has to guess.
    for name in ["claude", "codex", "gemini", "aider", "ollama", "npx"] {
        let found = &first["programs"][name];
        assert!(found.is_null() || found.is_string(), "{name}: {found}");
    }

    asked(&state, "onboarding.set", json!({ "done": true })).await;
    let told = events.recv().await.unwrap();
    assert_eq!((told["event"].clone(), told["data"]["done"].clone()), (json!("onboarding.changed"), json!(true)));
    // Another core over the same data: the user is not greeted twice.
    let again = AppState::new(sandbox.manager());
    assert_eq!(asked(&again, "app.info", json!({})).await["onboarded"], true);
    assert_eq!(asked(&again, "onboarding.get", json!({})).await["done"], true);

    // It can be brought back, from Settings.
    asked(&again, "onboarding.set", json!({ "done": false })).await;
    assert_eq!(asked(&again, "onboarding.get", json!({})).await["done"], false);
    assert!(dispatch(&again, "onboarding.set", &json!({})).await.is_err());
}

#[tokio::test]
async fn a_new_project_is_described_by_what_its_folder_holds() {
    let sandbox = Sandbox::new("onboarding-project");
    let state = AppState::new(sandbox.manager());
    let folder = git_project(&sandbox);
    std::fs::write(
        folder.join("package.json"),
        r#"{ "name": "shop", "scripts": { "dev": "vite", "test": "vitest run", "lint": "eslint ." } }"#,
    )
    .unwrap();
    std::fs::write(folder.join("compose.yaml"), "services:\n  db:\n    image: postgres:17\n").unwrap();
    git(&folder, &["remote", "add", "origin", "git@github.com:mrx/shop.git"]);
    let project = state.manager.add_project(folder.to_str().unwrap(), None).unwrap();

    let seen = asked(&state, "onboarding.project", json!({ "projectId": project.id })).await;
    assert_eq!(seen["git"], true);
    assert_eq!((seen["host"].clone(), seen["server"].clone()), (json!("GitHub"), json!("github.com")));
    assert_eq!(seen["repository"], "mrx/shop");
    assert_eq!(seen["scripts"], json!(["dev"]), "{seen:#}");
    assert_eq!(seen["compose"], true);
    let checks: Vec<&str> = seen["checks"].as_array().unwrap().iter().filter_map(Value::as_str).collect();
    assert!(checks.len() >= 2, "the project's own lint and tests are found: {checks:?}");

    // A plain folder has none of that, and says so by being empty.
    let plain = Sandbox::new("onboarding-plain");
    let other = state.manager.add_project(plain.project().to_str().unwrap(), None).unwrap();
    let seen = asked(&state, "onboarding.project", json!({ "projectId": other.id })).await;
    assert_eq!((seen["git"].clone(), seen["host"].clone()), (json!(false), Value::Null));
    assert_eq!((seen["scripts"].clone(), seen["compose"].clone()), (json!([]), json!(false)));
}
