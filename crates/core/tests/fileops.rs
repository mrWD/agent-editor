//! Making, renaming and deleting files by hand.

mod common;

use ae_core::marks::Author;
use common::*;
use serde_json::Value;
use std::path::PathBuf;

const FILE: &str = "src/middleware/rateLimit.ts";

async fn started(sandbox: &Sandbox) -> (std::sync::Arc<ae_core::tasks::Manager>, String, PathBuf) {
    let project_dir = git_project(sandbox);
    let manager = sandbox.manager();
    let project = manager.add_project(&project_dir.to_string_lossy(), None).unwrap();
    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock", "What does the limiter do?").await.unwrap();
    until(&mut events, &task.row.id, |e| state_is(e, "idle")).await;
    (manager, task.row.id.clone(), PathBuf::from(&task.row.cwd))
}

fn file_events(manager: &ae_core::tasks::Manager, id: &str) -> Vec<Value> {
    manager.history(id).unwrap().into_iter().filter(|e| is(e, "user_file")).collect()
}

#[tokio::test]
async fn a_new_file_is_made_with_its_folders_and_counts_as_the_users() {
    let sandbox = Sandbox::new("file-create");
    let (manager, id, root) = started(&sandbox).await;

    let made = manager.file_create(&id, "src/limits/config.ts", false).unwrap();
    assert_eq!(made["path"], "src/limits/config.ts");
    assert_eq!(std::fs::read_to_string(root.join("src/limits/config.ts")).unwrap(), "");
    manager.file_create(&id, "docs", true).unwrap();
    assert!(root.join("docs").is_dir());

    // It is waiting for review as a file the user made, and the agent will be told.
    let changed = manager.changed_files(&id).unwrap();
    let review = changed.iter().find(|file| file.path == "src/limits/config.ts").expect("the new file is listed");
    assert!(review.exists && !review.existed);
    let told = file_events(&manager, &id);
    assert_eq!(told[0]["action"], "created");
    // A point to rewind to was saved before the first change by hand.
    let points = manager.checkpoints(&id).unwrap();
    assert_eq!(points["checkpoints"].as_array().unwrap().last().unwrap()["kind"], "edit");

    // What cannot be made says why, and makes nothing.
    for (path, why) in [
        ("src/limits/config.ts", "already something called"),
        ("../outside.ts", "\"..\""),
        ("", "Give a name"),
        ("a//b.ts", "empty parts"),
    ] {
        let error = manager.file_create(&id, path, false).unwrap_err().to_string();
        assert!(error.contains(why), "{path}: {error}");
    }
    assert!(!root.parent().unwrap().join("outside.ts").exists());
}

#[tokio::test]
async fn a_renamed_file_keeps_what_is_known_about_its_lines() {
    let sandbox = Sandbox::new("file-rename");
    let (manager, id, root) = started(&sandbox).await;
    // The agent changes one line of the file first.
    let mut events = manager.subscribe();
    manager.prompt(&id, "Fix the limit".into()).await.unwrap();
    let seen = until(&mut events, &id, |e| is(e, "permission_request")).await;
    let request = seen.last().unwrap()["requestId"].as_str().unwrap().to_string();
    manager.answer_permission(&id, &request, Some("allow-once".into())).unwrap();
    until(&mut events, &id, |e| state_is(e, "idle")).await;

    let done = manager.file_rename(&id, FILE, "src/middleware/limiter.ts").unwrap();
    assert_eq!(done["moved"][0]["from"], FILE);
    assert_eq!(done["moved"][0]["to"], "src/middleware/limiter.ts");
    assert!(!root.join(FILE).exists());
    assert!(root.join("src/middleware/limiter.ts").is_file());
    // The agent's line is still the agent's under the new name.
    let review = manager.review_of(&id, "src/middleware/limiter.ts").unwrap();
    assert_eq!(review.agent_lines, 1);
    assert_eq!(review.your_lines, 0);
    assert_eq!(file_events(&manager, &id).last().unwrap()["to"], "src/middleware/limiter.ts");

    // A whole folder moves with everything in it.
    let done = manager.file_rename(&id, "src/middleware", "src/guards").unwrap();
    assert_eq!(done["moved"][0]["to"], "src/guards/limiter.ts");
    assert_eq!(done["to"], "src/guards");
    assert!(root.join("src/guards/limiter.ts").is_file());
    assert_eq!(manager.review_of(&id, "src/guards/limiter.ts").unwrap().agent_lines, 1);

    // A name that is taken, or a file that is not there, is refused.
    let error = manager.file_rename(&id, "README.md", "src/guards/limiter.ts").unwrap_err().to_string();
    assert!(error.contains("already something called"), "{error}");
    assert!(manager.file_rename(&id, "nothing.ts", "x.ts").is_err());
    assert!(root.join("README.md").is_file());
}

#[tokio::test]
async fn a_deleted_file_is_kept_and_can_be_put_back() {
    let sandbox = Sandbox::new("file-delete");
    let (manager, id, root) = started(&sandbox).await;
    // A file git does not know is the one that could be lost for good.
    std::fs::write(root.join(".env"), "SECRET=1\n").unwrap();
    manager.observe(&id, ".env", Author::You, None).unwrap();

    let gone = manager.file_delete(&id, ".env").unwrap();
    assert_eq!(gone["files"][0], ".env");
    assert!(!root.join(".env").exists());
    let kept = gone["keptAs"].as_str().unwrap().to_string();
    assert!(sandbox.data().join("trash").join(&id).join(&kept).join(".env").is_file(), "it is kept, not destroyed");

    let back = manager.file_restore(&id, &kept).unwrap();
    assert_eq!(back["path"], ".env");
    assert_eq!(std::fs::read_to_string(root.join(".env")).unwrap(), "SECRET=1\n");
    assert!(manager.file_restore(&id, &kept).is_err(), "it can be put back once");
    let actions: Vec<String> =
        file_events(&manager, &id).iter().map(|e| e["action"].as_str().unwrap().to_string()).collect();
    assert_eq!(actions, ["deleted", "restored"]);

    // A folder goes and comes back whole; a file known to git shows as deleted in between.
    let gone = manager.file_delete(&id, "src").unwrap();
    assert_eq!(gone["files"][0], FILE);
    assert!(!root.join("src").exists());
    let deleted = manager.changed_files(&id).unwrap();
    assert!(deleted.iter().any(|file| file.path == FILE && !file.exists && file.existed));
    manager.file_restore(&id, gone["keptAs"].as_str().unwrap()).unwrap();
    assert!(root.join(FILE).is_file());

    // What must never go: the folder itself, what belongs to git, anything outside.
    assert!(manager.file_delete(&id, "").is_err());
    assert!(manager.file_delete(&id, ".git").unwrap_err().to_string().contains("belongs to git"));
    assert!(manager.file_delete(&id, "../project").is_err());
    assert!(manager.file_restore(&id, "../../etc").is_err());
    // Putting back onto a name that is taken again is refused.
    let gone = manager.file_delete(&id, "README.md").unwrap();
    std::fs::write(root.join("README.md"), "new\n").unwrap();
    let error = manager.file_restore(&id, gone["keptAs"].as_str().unwrap()).unwrap_err().to_string();
    assert!(error.contains("again"), "{error}");
    assert_eq!(std::fs::read_to_string(root.join("README.md")).unwrap(), "new\n");
}
