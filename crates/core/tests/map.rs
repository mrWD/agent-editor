//! End-to-end tests of the Map view: the `map.get` command on small projects,
//! and the practice agent's answers to the two messages the view sends.
//!
//! Like the other tests, these run against the practice agent only.

mod common;

use ae_core::server::{AppState, dispatch};
use common::*;
use serde_json::{Value, json};
use std::path::Path;

const DEMO: &str = "src/middleware/rateLimit.ts";

/// Adds two files around the demo file and commits them: one that imports it, and one that one imports.
fn with_neighbours(project: &Path) {
    std::fs::create_dir_all(project.join("src/limits")).unwrap();
    std::fs::write(
        project.join("src/app.ts"),
        "import { loginLimiter } from './middleware/rateLimit';\nimport { limits } from './limits/config';\n\nexport const app = { loginLimiter, limits };\n",
    )
    .unwrap();
    std::fs::write(project.join("src/limits/config.ts"), "export const limits = { login: 5 };\n").unwrap();
    git(project, &["add", "."]);
    git(project, &["-c", "user.name=Test", "-c", "user.email=test@example.com", "commit", "-q", "-m", "Neighbours"]);
}

/// Starts a task that asks for an edit, allows it and waits for the turn to end.
async fn task_with_an_allowed_edit(state: &AppState, project_id: &str, text: &str) -> String {
    let manager = &state.manager;
    let mut events = manager.subscribe();
    let task = manager.create(project_id, "mock", text).await.unwrap();
    let id = task.row.id.clone();
    let seen = until(&mut events, &id, |e| is(e, "permission_request")).await;
    let request = seen.last().unwrap()["requestId"].as_str().unwrap().to_string();
    manager.answer_permission(&id, &request, Some("allow-once".into())).unwrap();
    until(&mut events, &id, |e| state_is(e, "idle")).await;
    id
}

/// The labels of the boxes of one layer of a picture.
fn layer(graph: &Value, layer: i64) -> Vec<String> {
    let nodes = graph["nodes"].as_array().unwrap();
    nodes.iter().filter(|node| node["layer"] == layer).map(|node| node["label"].as_str().unwrap().to_string()).collect()
}

fn tree_paths(map: &Value) -> Vec<(String, bool)> {
    let files = map["tree"].as_array().unwrap();
    files.iter().map(|file| (file["path"].as_str().unwrap().to_string(), file["changed"].as_bool().unwrap())).collect()
}

#[tokio::test]
async fn the_map_shows_what_the_agent_changed_and_the_files_around_it() {
    let sandbox = Sandbox::new("map");
    let state = AppState::new(sandbox.manager());
    let manager = &state.manager;
    let folder = git_project(&sandbox);
    with_neighbours(&folder);
    let project = manager.add_project(&folder.to_string_lossy(), None).unwrap();

    // Before anything changed there is nothing to draw, and that is not an error.
    let mut events = manager.subscribe();
    let quiet = manager.create(&project.id, "mock", "What does the limiter do?").await.unwrap();
    until(&mut events, &quiet.row.id, |e| state_is(e, "idle")).await;
    let empty = dispatch(&state, "map.get", &json!({ "taskId": quiet.row.id })).await.unwrap();
    assert_eq!(empty["center"], Value::Null);
    assert_eq!(empty["tree"], json!([]));

    let id = task_with_an_allowed_edit(&state, &project.id, "Change the limit").await;
    let map = dispatch(&state, "map.get", &json!({ "taskId": id })).await.unwrap();

    // The centre is the constant the agent changed.
    assert_eq!(map["center"], json!({ "path": DEMO, "symbol": "loginLimiter", "label": "loginLimiter", "line": 1 }));
    let centers: Vec<&str> = map["centers"].as_array().unwrap().iter().map(|c| c["label"].as_str().unwrap()).collect();
    assert_eq!(centers, vec!["rateLimit.ts", "loginLimiter"]);

    // The tree: the changed file and the file that imports it. What that one imports besides is not a neighbour.
    assert_eq!(tree_paths(&map), vec![("src/app.ts".to_string(), false), (DEMO.to_string(), true)]);
    assert_eq!(
        map["tree"][1],
        json!({
            "path": DEMO,
            "changed": true,
            "language": "TypeScript",
            "defs": [{ "name": "loginLimiter", "kind": "const", "line": 1, "changed": true }],
            "tests": 0
        })
    );
    assert_eq!(map["tree"][0]["defs"], json!([{ "name": "app", "kind": "const", "line": 4, "changed": false }]));

    // Dependencies: who needs the changed file.
    assert_eq!(
        (layer(&map["deps"], -1), layer(&map["deps"], 0)),
        (vec!["app.ts".to_string()], vec!["rateLimit.ts".to_string()])
    );
    assert_eq!(map["deps"]["edges"], json!([{ "from": "file:src/app.ts", "to": format!("file:{DEMO}") }]));
    assert_eq!(map["deps"]["nodes"][0]["changed"], true);

    // Calls: `app` uses the limiter; the limiter calls `rateLimit`, which no import explains.
    assert_eq!(layer(&map["calls"], -1), vec!["app"]);
    assert_eq!(layer(&map["calls"], 0), vec!["loginLimiter"]);
    assert_eq!(layer(&map["calls"], 1), vec!["rateLimit()"]);
    let unknown =
        &map["calls"]["nodes"].as_array().unwrap().iter().find(|n| n["label"] == "rateLimit()").unwrap().clone();
    assert_eq!(
        (unknown["kind"].as_str(), unknown["path"].as_str(), unknown["line"].as_u64()),
        (Some("other"), Some(DEMO), Some(1))
    );
    assert_eq!(map["notes"], json!([]));

    // The whole file can be the centre. A centre that does not exist falls back to the first one.
    let whole = dispatch(&state, "map.get", &json!({ "taskId": id, "center": { "path": DEMO } })).await.unwrap();
    assert_eq!(whole["center"]["symbol"], Value::Null);
    assert_eq!(whole["center"]["label"], "rateLimit.ts");
    let gone = dispatch(&state, "map.get", &json!({ "taskId": id, "center": { "path": "nowhere.ts" } })).await.unwrap();
    assert_eq!(gone["center"]["symbol"], "loginLimiter");
}

#[tokio::test]
async fn commits_of_the_branch_new_files_and_unknown_languages_all_count() {
    let sandbox = Sandbox::new("map-branch");
    let state = AppState::new(sandbox.manager());
    let manager = &state.manager;
    let folder = git_project(&sandbox);
    with_neighbours(&folder);
    let project = manager.add_project(&folder.to_string_lossy(), None).unwrap();
    let id = task_with_an_allowed_edit(&state, &project.id, "Change the limit").await;

    // Committed on the task's branch: still what the task changed.
    dispatch(
        &state,
        "git.commit",
        &json!({ "taskId": id, "message": "Take the limit from config", "picks": [{ "path": DEMO }] }),
    )
    .await
    .unwrap();
    // A new file git does not know yet, in a language the map reads, and one in a language it does not.
    for (path, text) in [
        ("tools/report.py", "from .format import line\n\ndef report(rows):\n    return [line(row) for row in rows]\n"),
        ("tools/format.py", "def line(row):\n    return str(row)\n"),
        ("README.md", "# Shop\n\nNow with limits.\n"),
    ] {
        let full = manager.cwd(&id).unwrap().join(path);
        std::fs::create_dir_all(full.parent().unwrap()).unwrap();
        std::fs::write(full, text).unwrap();
    }

    let map = dispatch(&state, "map.get", &json!({ "taskId": id })).await.unwrap();
    assert_eq!(
        tree_paths(&map),
        vec![
            ("README.md".to_string(), true),
            ("src/app.ts".to_string(), false),
            (DEMO.to_string(), true),
            ("tools/format.py".to_string(), true),
            ("tools/report.py".to_string(), true),
        ]
    );
    // Markdown is listed without details; nothing is made up about it.
    assert_eq!(
        map["tree"][0],
        json!({ "path": "README.md", "changed": true, "language": null, "defs": [], "tests": 0 })
    );
    assert_eq!(map["tree"][4]["defs"], json!([{ "name": "report", "kind": "function", "line": 3, "changed": true }]));
    // Only files the map can read can be its centre.
    let centers: Vec<&str> = map["centers"].as_array().unwrap().iter().map(|c| c["label"].as_str().unwrap()).collect();
    assert_eq!(centers, vec!["rateLimit.ts", "loginLimiter", "format.py", "line", "report.py", "report"]);

    let python = dispatch(
        &state,
        "map.get",
        &json!({ "taskId": id, "center": { "path": "tools/format.py", "symbol": "line" } }),
    )
    .await
    .unwrap();
    assert_eq!(layer(&python["deps"], -1), vec!["report.py"]);
    assert_eq!(layer(&python["calls"], -1), vec!["report()"]);

    // A task that does not exist is refused in plain words.
    let refused = dispatch(&state, "map.get", &json!({ "taskId": "nope" })).await.unwrap_err().to_string();
    assert!(!refused.is_empty());
}

#[tokio::test]
async fn in_a_folder_without_git_the_review_marks_say_what_changed() {
    let sandbox = Sandbox::new("map-plain");
    let state = AppState::new(sandbox.manager());
    let manager = &state.manager;
    let folder = sandbox.project();
    std::fs::create_dir_all(folder.join("src/middleware")).unwrap();
    std::fs::write(
        folder.join(DEMO),
        "export const before = 1;\n\nexport const loginLimiter = rateLimit({\n  limit: 5,\n});\n\nexport const after = 2;\n",
    )
    .unwrap();
    let project = manager.add_project(&folder.to_string_lossy(), None).unwrap();
    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock", "What does the limiter do?").await.unwrap();
    let id = task.row.id.clone();
    until(&mut events, &id, |e| state_is(e, "idle")).await;
    let nothing = dispatch(&state, "map.get", &json!({ "taskId": id })).await.unwrap();
    assert_eq!(nothing["center"], Value::Null);

    // Without git the core knows a change only from a save it saw: here, one line saved in the editor.
    let text = std::fs::read_to_string(folder.join(DEMO)).unwrap().replace("limit: 5,", "limit: 6,");
    dispatch(&state, "files.write", &json!({ "taskId": id, "path": DEMO, "text": text })).await.unwrap();

    let map = dispatch(&state, "map.get", &json!({ "taskId": id })).await.unwrap();
    assert_eq!(map["center"]["symbol"], "loginLimiter");
    // Only the constant the changed line is in counts as changed.
    let changed: Vec<bool> =
        map["tree"][0]["defs"].as_array().unwrap().iter().map(|d| d["changed"].as_bool().unwrap()).collect();
    assert_eq!(changed, vec![false, true, false]);
}

#[tokio::test]
async fn the_practice_agent_draws_a_flow_and_explains_without_editing() {
    let sandbox = Sandbox::new("map-flow");
    let state = AppState::new(sandbox.manager());
    let manager = &state.manager;
    let folder = git_project(&sandbox);
    let project = manager.add_project(&folder.to_string_lossy(), None).unwrap();
    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock", "What does the limiter do?").await.unwrap();
    let id = task.row.id.clone();
    until(&mut events, &id, |e| state_is(e, "idle")).await;
    let before = std::fs::read_to_string(manager.cwd(&id).unwrap().join(DEMO)).unwrap();

    // The request shows the format; its words "changed" and "write" must not start an edit.
    let request = "Draw the flow of `loginLimiter` in `src/middleware/rateLimit.ts`.\n\nAnswer with one block in exactly this format:\n\n```flow\nstep a: First thing\n```\n\nPut * after the word for what this task changed. Write nothing else.";
    manager.prompt(&id, request.into()).await.unwrap();
    let seen = until(&mut events, &id, |e| state_is(e, "idle")).await;
    assert!(!seen.iter().any(|e| is(e, "permission_request")), "a flow is drawn without touching a file");
    let answer = answer_text(&seen);
    assert!(
        answer.contains(
            "```flow\nstep a: Request to POST /login\nask b: Over the limit? @ src/middleware/rateLimit.ts:3\nstep* c: Respond 429 @ src/middleware/rateLimit.ts:3\nstep d: Check the password\na -> b\nb -> c: yes\nb -> d: no\n```"
        ),
        "{answer}"
    );

    manager
        .prompt(&id, "Map, `loginLimiter` in `src/middleware/rateLimit.ts`\n\nExplain what changed here.".into())
        .await
        .unwrap();
    let seen = until(&mut events, &id, |e| state_is(e, "idle")).await;
    assert!(!seen.iter().any(|e| is(e, "permission_request")), "a question about a change is not a request for one");
    assert!(answer_text(&seen).contains("Nothing was changed."));
    assert_eq!(std::fs::read_to_string(manager.cwd(&id).unwrap().join(DEMO)).unwrap(), before);
}
