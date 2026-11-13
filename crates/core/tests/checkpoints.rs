//! Checkpoints: looking back, rewinding, and "since you left".
//!
//! Only the practice agent runs here, so nothing is spent on anyone's account.

mod common;

use common::*;
use serde_json::Value;
use std::path::{Path, PathBuf};

const FILE: &str = "src/middleware/rateLimit.ts";
const BEFORE: &str = "export const loginLimiter = rateLimit({\n  windowMs: 60 * 1000,\n  limit: 5,\n});\n";

/// A task in a git project whose agent has answered its first message.
async fn started(sandbox: &Sandbox, text: &str) -> (std::sync::Arc<ae_core::tasks::Manager>, String, PathBuf) {
    let project_dir = git_project(sandbox);
    let manager = sandbox.manager();
    let project = manager.add_project(&project_dir.to_string_lossy(), None).unwrap();
    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock", text).await.unwrap();
    until(&mut events, &task.row.id, |e| state_is(e, "idle")).await;
    (manager, task.row.id.clone(), PathBuf::from(&task.row.cwd))
}

/// Sends a message that asks for an edit, allows it and waits for the end of the turn.
async fn let_agent_edit(manager: &std::sync::Arc<ae_core::tasks::Manager>, id: &str, text: &str) {
    let mut events = manager.subscribe();
    manager.prompt(id, text.into()).await.unwrap();
    let seen = until(&mut events, id, |e| is(e, "permission_request")).await;
    let request = seen.last().unwrap()["requestId"].as_str().unwrap().to_string();
    manager.answer_permission(id, &request, Some("allow-once".into())).unwrap();
    until(&mut events, id, |e| state_is(e, "idle")).await;
}

fn listed(manager: &ae_core::tasks::Manager, id: &str) -> Vec<Value> {
    manager.checkpoints(id).unwrap()["checkpoints"].as_array().unwrap().clone()
}

fn read(root: &Path, file: &str) -> String {
    std::fs::read_to_string(root.join(file)).unwrap()
}

#[tokio::test]
async fn a_checkpoint_is_taken_before_every_message_and_touches_nothing() {
    let sandbox = Sandbox::new("checkpoints");
    let (manager, id, root) = started(&sandbox, "What does the limiter do?").await;
    let head = git(&root, &["rev-parse", "HEAD"]);
    let_agent_edit(&manager, &id, "Fix the limit").await;

    let points = listed(&manager, &id);
    assert_eq!(points.len(), 2);
    assert_eq!(points[0]["kind"], "start");
    assert_eq!(points[0]["label"], "Start of the task");
    assert_eq!(points[0]["files"], 0, "the first turn changed nothing");
    assert_eq!(points[1]["kind"], "message");
    assert_eq!(points[1]["label"], "Fix the limit");
    // After the second checkpoint the agent changed one line of one file.
    assert_eq!(points[1]["who"], "agent");
    assert_eq!(
        (points[1]["files"].as_u64(), points[1]["added"].as_u64(), points[1]["removed"].as_u64()),
        (Some(1), Some(1), Some(1))
    );
    assert_eq!(points[1]["paths"][0], FILE);
    assert_eq!(points[1]["sameAsNow"], false);

    // Taking checkpoints leaves the branch, the index and the list of changes as they were.
    assert_eq!(git(&root, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&root, &["status", "--porcelain"]), format!("M {FILE}"));
    assert_eq!(git(&root, &["stash", "list"]), "");
    // They are kept outside the branches, so they are never pushed.
    let kept = git(&root, &["for-each-ref", "--format=%(refname)", "refs/ae/"]);
    assert_eq!(kept.lines().count(), 2, "{kept}");
    assert_eq!(
        git(&root, &["for-each-ref", "--format=%(refname)", "refs/heads/"])
            .lines()
            .filter(|l| l.contains("ae/"))
            .count(),
        0
    );
}

#[tokio::test]
async fn rewinding_puts_the_files_back_and_can_be_undone() {
    let sandbox = Sandbox::new("rewind");
    let (manager, id, root) = started(&sandbox, "What does the limiter do?").await;
    let_agent_edit(&manager, &id, "Fix the limit").await;
    let after = read(&root, FILE);
    assert!(after.contains("limit: limits.login,"));
    // The agent also made a new file, and the user one more.
    std::fs::write(root.join("NEW.md"), "made later\n").unwrap();
    let point = listed(&manager, &id)[1]["n"].as_i64().unwrap();

    // What rewinding would change, before anything is changed.
    let files = manager.files_between(&id, &format!("checkpoint:{point}")).unwrap();
    let paths: Vec<&str> = files["files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap()).collect();
    assert_eq!(paths, ["NEW.md", FILE]);
    let diff = manager.diff_between(&id, FILE, &format!("checkpoint:{point}")).unwrap();
    assert_eq!((diff["added"].as_u64(), diff["removed"].as_u64()), (Some(1), Some(1)));
    assert_eq!(read(&root, FILE), after, "looking changes nothing");

    let done = manager.rewind(&id, point, false, false).unwrap();
    assert_eq!(done["files"], 2);
    assert_eq!(read(&root, FILE), BEFORE);
    assert!(!root.join("NEW.md").exists(), "a file that was not there then is gone");
    let history = manager.history(&id).unwrap();
    let told = history.iter().rev().find(|e| is(e, "rewound")).unwrap();
    assert_eq!(told["label"], "Fix the limit");
    assert_eq!(told["droppedMessages"], false);
    assert!(history.iter().any(|e| is(e, "user_message") && e["text"] == "Fix the limit"), "the conversation stays");

    // The rewind left its own checkpoint: going back to it undoes the rewind.
    let undo = done["undo"].as_i64().unwrap();
    assert_eq!(listed(&manager, &id).last().unwrap()["kind"], "rewind");
    manager.rewind(&id, undo, false, false).unwrap();
    assert_eq!(read(&root, FILE), after);
    assert_eq!(read(&root, "NEW.md"), "made later\n");
}

#[tokio::test]
async fn a_rewind_keeps_the_lines_the_user_wrote() {
    let sandbox = Sandbox::new("keep-mine");
    let (manager, id, root) = started(&sandbox, "What does the limiter do?").await;
    let_agent_edit(&manager, &id, "Fix the limit").await;
    // The user changes another line of the same file by hand, the way the editor saves it.
    let mine = read(&root, FILE).replace("windowMs: 60 * 1000,", "windowMs: 30 * 1000,");
    manager.checkpoint_before_edit(&id);
    manager.observe(&id, FILE, ae_core::marks::Author::Agent, None).unwrap();
    std::fs::write(root.join(FILE), &mine).unwrap();
    manager.observe(&id, FILE, ae_core::marks::Author::You, None).unwrap();

    let points = listed(&manager, &id);
    assert_eq!(points.last().unwrap()["kind"], "edit");
    assert_eq!(points.last().unwrap()["who"], "you");
    let point = points[1]["n"].as_i64().unwrap();
    let files = manager.files_between(&id, &format!("checkpoint:{point}")).unwrap();
    assert_eq!(files["files"][0]["yours"], true);

    let done = manager.rewind(&id, point, true, false).unwrap();
    assert_eq!(done["kept"][0], FILE);
    let text = read(&root, FILE);
    assert!(text.contains("limit: 5,"), "the agent's line went back: {text}");
    assert!(text.contains("windowMs: 30 * 1000,"), "the user's line stayed: {text}");

    // Without keeping, the file is exactly as it was then.
    let undo = done["undo"].as_i64().unwrap();
    manager.rewind(&id, undo, false, false).unwrap();
    assert_eq!(read(&root, FILE), mine);
    manager.rewind(&id, point, false, false).unwrap();
    assert_eq!(read(&root, FILE), BEFORE);
}

#[tokio::test]
async fn a_rewind_can_take_the_later_messages_out_and_undoing_it_brings_them_back() {
    let sandbox = Sandbox::new("drop-messages");
    let (manager, id, root) = started(&sandbox, "What does the limiter do?").await;
    let_agent_edit(&manager, &id, "Fix the limit").await;
    let point = listed(&manager, &id)[1]["n"].as_i64().unwrap();
    let mut events = manager.subscribe();
    manager.stop_agent(&id).await.unwrap();
    until(&mut events, &id, |e| state_is(e, "asleep")).await;

    let mut windows = manager.subscribe();
    let done = manager.rewind(&id, point, false, true).unwrap();
    assert_eq!(read(&root, FILE), BEFORE);
    let history = manager.history(&id).unwrap();
    assert!(
        !history.iter().any(|e| is(e, "user_message") && e["text"] == "Fix the limit"),
        "the later message is gone"
    );
    assert!(history.iter().any(|e| is(e, "user_message") && e["text"] == "What does the limiter do?"));
    assert_eq!(history.last().unwrap()["type"], "rewound");
    assert_eq!(history.last().unwrap()["droppedMessages"], true);
    // What is left ends with the state the task really has, not with an older one.
    let state = history.iter().rev().find(|e| is(e, "state")).unwrap();
    assert_eq!(state["state"], "asleep");
    // Every window is told to read the conversation again.
    let mut reload = false;
    while let Ok(message) = windows.try_recv() {
        reload |= message["event"] == "task.reload" && message["data"]["taskId"] == id.as_str();
    }
    assert!(reload);

    // The next agent is told only what is left of the conversation.
    manager.prompt(&id, "And who calls it?".into()).await.unwrap();
    let seen = until(&mut events, &id, |e| state_is(e, "idle")).await;
    let told: String = seen
        .iter()
        .filter(|e| e["update"]["sessionUpdate"] == "agent_thought_chunk")
        .filter_map(|e| e["update"]["content"]["text"].as_str())
        .collect();
    assert!(told.contains("User: What does the limiter do?"), "{told}");
    assert!(!told.contains("Fix the limit"), "{told}");

    // Undoing the rewind brings the code and the messages back.
    manager.stop_agent(&id).await.unwrap();
    until(&mut events, &id, |e| state_is(e, "asleep")).await;
    manager.rewind(&id, done["undo"].as_i64().unwrap(), false, false).unwrap();
    assert!(read(&root, FILE).contains("limit: limits.login,"));
    let history = manager.history(&id).unwrap();
    assert!(history.iter().any(|e| is(e, "user_message") && e["text"] == "Fix the limit"));
}

#[tokio::test]
async fn a_rewind_is_refused_while_the_agent_works_and_without_git() {
    let sandbox = Sandbox::new("rewind-refused");
    let (manager, id, _root) = started(&sandbox, "What does the limiter do?").await;
    let point = listed(&manager, &id)[0]["n"].as_i64().unwrap();
    let mut events = manager.subscribe();
    manager.prompt(&id, "Fix the limit".into()).await.unwrap();
    until(&mut events, &id, |e| is(e, "permission_request")).await;
    let error = manager.rewind(&id, point, true, false).unwrap_err().to_string();
    assert!(error.contains("Stop its turn first"), "{error}");
    assert!(manager.rewind(&id, 999, true, false).is_err(), "no such checkpoint");
    manager.cancel(&id).unwrap();
    until(&mut events, &id, |e| state_is(e, "idle")).await;

    // A folder without git has no checkpoints, and says so.
    let plain = Sandbox::new("rewind-plain");
    let manager = plain.manager();
    let project = manager.add_project(&plain.project().to_string_lossy(), None).unwrap();
    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock", "What does the limiter do?").await.unwrap();
    until(&mut events, &task.row.id, |e| state_is(e, "idle")).await;
    let list = manager.checkpoints(&task.row.id).unwrap();
    assert_eq!(list["git"], false);
    assert!(list["checkpoints"].as_array().unwrap().is_empty());
    let error = manager.rewind(&task.row.id, 1, true, false).unwrap_err().to_string();
    assert!(error.contains("without git"), "{error}");
    // Leaving and coming back still works there, without files to compare.
    manager.leave(&task.row.id).unwrap();
    let since = manager.since(&task.row.id).unwrap();
    assert_eq!(since["quiet"], true);
    assert_eq!(since["canCompare"], false);
}

#[tokio::test]
async fn coming_back_says_where_the_user_stopped_and_what_happened_since() {
    let sandbox = Sandbox::new("since");
    let (manager, id, root) = started(&sandbox, "What does the limiter do?").await;
    assert_eq!(manager.since(&id).unwrap(), Value::Null, "a task that was never left has nothing to say");

    // The user sends a message and leaves while the agent is waiting for an answer.
    let mut events = manager.subscribe();
    manager.prompt(&id, "Fix the limit".into()).await.unwrap();
    let seen = until(&mut events, &id, |e| is(e, "permission_request")).await;
    let request = seen.last().unwrap()["requestId"].as_str().unwrap().to_string();
    manager.leave(&id).unwrap();
    let just_left = manager.since(&id).unwrap();
    assert_eq!(
        just_left["stopped"],
        "You started this task with “What does the limiter do?” and told Practice agent “Fix the limit”, and left while Practice agent was waiting for your answer."
    );
    assert_eq!(just_left["quiet"], true, "the question was asked before the user left: nothing is new yet");

    // While the user is away the answer is given (from another window) and the agent finishes.
    manager.answer_permission(&id, &request, Some("allow-once".into())).unwrap();
    until(&mut events, &id, |e| state_is(e, "idle")).await;
    let since = manager.since(&id).unwrap();
    let texts: Vec<&str> = since["happened"].as_array().unwrap().iter().map(|l| l["text"].as_str().unwrap()).collect();
    assert_eq!(texts, ["Practice agent changed rateLimit.ts", "Practice agent finished its turn"]);
    assert_eq!(since["lastWords"], "Done. The limit now comes from config instead of a fixed 5. One file changed.");
    assert_eq!(since["files"][0]["path"], FILE);
    assert_eq!(since["quiet"], false);
    assert_eq!(since["canCompare"], true);
    assert!(since["awayMs"].as_u64().is_some());

    // The same change, line by line, counted from the moment of leaving.
    let files = manager.files_between(&id, "visit").unwrap();
    assert_eq!(files["files"].as_array().unwrap().len(), 1);
    let diff = manager.diff_between(&id, FILE, "visit").unwrap();
    assert_eq!((diff["added"].as_u64(), diff["removed"].as_u64()), (Some(1), Some(1)));

    // Leaving again starts the count from here: nothing is new any more.
    manager.leave(&id).unwrap();
    let since = manager.since(&id).unwrap();
    assert_eq!(since["quiet"], true);
    assert!(since["files"].as_array().unwrap().is_empty());
    assert!(manager.files_between(&id, "visit").unwrap()["files"].as_array().unwrap().is_empty());

    // A commit the agent made itself in the terminal is told too.
    git(
        &root,
        &[
            "-c",
            "user.name=Test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-q",
            "-am",
            "Move the limit into config",
        ],
    );
    let since = manager.since(&id).unwrap();
    assert_eq!(since["happened"][0]["text"], "Practice agent committed “Move the limit into config”");

    // The whole branch against where it started.
    let branch = manager.files_between(&id, "branch").unwrap();
    assert_eq!(branch["files"][0]["path"], FILE);
}
