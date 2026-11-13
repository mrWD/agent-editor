//! End-to-end tests of working with code: saving files, line marks, terminals and search.
//!
//! Like the task tests, these run against the practice agent only.

mod common;

use ae_core::marks::{Author, Run};
use ae_core::server::{AppState, dispatch};
use ae_core::terminals::{TermEvent, Terminals};
use common::*;
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

const DEMO: &str = "src/middleware/rateLimit.ts";

fn read(dir: &str, file: &str) -> String {
    std::fs::read_to_string(Path::new(dir).join(file)).unwrap()
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

#[tokio::test]
async fn lines_are_marked_by_who_wrote_them_and_can_be_kept_or_undone() {
    let sandbox = Sandbox::new("marks");
    let state = AppState::new(sandbox.manager());
    let manager = &state.manager;
    let project = manager.add_project(&git_project(&sandbox).to_string_lossy(), None).unwrap();
    let id = task_with_an_allowed_edit(&state, &project.id, "Change the limit").await;
    let cwd = manager.cwd(&id).unwrap().to_string_lossy().into_owned();

    // The practice agent really changed the file, and its line is marked as the agent's.
    assert!(read(&cwd, DEMO).contains("limit: limits.login,"));
    let opened = dispatch(&state, "files.read", &json!({ "taskId": id, "path": DEMO })).await.unwrap();
    assert_eq!(opened["review"]["runs"], json!([{ "from": 3, "to": 3, "who": "agent" }]));
    assert_eq!(opened["review"]["agentLines"], 1);

    // The user adds a line at the top and saves; that line is the user's.
    let mine = format!("// mine\n{}", opened["file"]["text"].as_str().unwrap());
    let saved = dispatch(
        &state,
        "files.write",
        &json!({ "taskId": id, "path": DEMO, "text": mine, "version": opened["file"]["version"] }),
    )
    .await
    .unwrap();
    assert_eq!(
        saved["review"]["runs"],
        json!([{ "from": 1, "to": 1, "who": "you" }, { "from": 4, "to": 4, "who": "agent" }])
    );
    let listed = dispatch(&state, "review.list", &json!({ "taskId": id })).await.unwrap();
    assert_eq!(listed["files"].as_array().unwrap().len(), 1);
    assert_eq!(listed["files"][0]["path"], DEMO);

    // Saving with a stale version is refused and nothing is written.
    let stale = dispatch(
        &state,
        "files.write",
        &json!({ "taskId": id, "path": DEMO, "text": "lost", "version": opened["file"]["version"] }),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(stale.contains("changed on disk"), "{stale}");
    assert_eq!(read(&cwd, DEMO), mine);

    // Undo takes back the agent's line and leaves the user's.
    let undone = dispatch(&state, "review.undo", &json!({ "taskId": id, "path": DEMO })).await.unwrap();
    assert_eq!(undone["review"]["runs"], json!([{ "from": 1, "to": 1, "who": "you" }]));
    let text = read(&cwd, DEMO);
    assert!(text.starts_with("// mine\n") && text.contains("limit: 5,"), "{text}");
}

#[tokio::test]
async fn keeping_a_file_clears_the_agents_marks_for_good() {
    let sandbox = Sandbox::new("keep");
    let state = AppState::new(sandbox.manager());
    let manager = &state.manager;
    let project = manager.add_project(&git_project(&sandbox).to_string_lossy(), None).unwrap();
    let id = task_with_an_allowed_edit(&state, &project.id, "Change the limit").await;

    let kept = dispatch(&state, "review.keep", &json!({ "taskId": id, "path": DEMO })).await.unwrap();
    assert_eq!(kept["review"]["agentLines"], 0);
    assert_eq!(dispatch(&state, "review.list", &json!({ "taskId": id })).await.unwrap()["files"], json!([]));

    // Another look at the disk does not bring the marks back.
    manager.sweep(&id, Author::You, &[]).unwrap();
    assert_eq!(manager.review_of(&id, DEMO).unwrap().runs, Vec::<Run>::new());

    // A second edit by the agent is marked again, on top of the kept text.
    let mut events = manager.subscribe();
    manager.prompt(&id, "Edit README.md".into()).await.unwrap();
    let seen = until(&mut events, &id, |e| is(e, "permission_request")).await;
    let request = seen.last().unwrap()["requestId"].as_str().unwrap().to_string();
    manager.answer_permission(&id, &request, Some("allow-once".into())).unwrap();
    until(&mut events, &id, |e| state_is(e, "idle")).await;
    let review = manager.review_of(&id, "README.md").unwrap();
    assert_eq!(review.runs, vec![Run { from: 2, to: 2, who: Author::Agent }]);
}

#[tokio::test]
async fn a_file_the_agent_made_goes_away_on_undo() {
    let sandbox = Sandbox::new("newfile");
    let state = AppState::new(sandbox.manager());
    let manager = &state.manager;
    let project = sandbox.project();
    std::fs::write(project.join("readme.md"), "hello\n").unwrap();
    git(&project, &["init", "-q", "-b", "main"]);
    git(&project, &["add", "."]);
    git(&project, &["-c", "user.name=Test", "-c", "user.email=test@example.com", "commit", "-q", "-m", "Start"]);
    let added = manager.add_project(&project.to_string_lossy(), None).unwrap();

    // Nothing to edit is named and the demo file is absent, so the agent makes NOTES.md.
    let id = task_with_an_allowed_edit(&state, &added.id, "Write some notes").await;
    let cwd = manager.cwd(&id).unwrap();
    assert!(cwd.join("NOTES.md").is_file());
    let review = manager.review_of(&id, "NOTES.md").unwrap();
    assert!(!review.existed && review.exists && review.agent_lines == 1);

    let after = manager.undo(&id, "NOTES.md").unwrap();
    assert!(!cwd.join("NOTES.md").exists());
    assert!(!after.is_changed());
}

#[tokio::test]
async fn the_agent_is_told_what_the_user_changed_by_hand() {
    let sandbox = Sandbox::new("manual");
    let state = AppState::new(sandbox.manager());
    let manager = &state.manager;
    let project = manager.add_project(&git_project(&sandbox).to_string_lossy(), None).unwrap();

    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock", "What does the limiter do?").await.unwrap();
    let id = task.row.id.clone();
    until(&mut events, &id, |e| state_is(e, "idle")).await;

    // Two saves of one file are one note in the conversation.
    for text in ["# Shop\nby hand\n", "# Shop\nby hand, twice\n"] {
        dispatch(&state, "files.write", &json!({ "taskId": id, "path": "README.md", "text": text })).await.unwrap();
    }
    let history = manager.history(&id).unwrap();
    let notes: Vec<_> = history.iter().filter(|e| is(e, "user_edit")).collect();
    assert_eq!(notes.len(), 1);
    assert_eq!(notes[0]["path"], "README.md");
    assert_eq!(manager.review_of(&id, "README.md").unwrap().your_lines, 1);

    // The next message carries the note; the practice agent repeats what it was told.
    manager.prompt(&id, "And now?".into()).await.unwrap();
    let seen = until(&mut events, &id, |e| state_is(e, "idle")).await;
    let message = seen.iter().find(|e| is(e, "user_message")).unwrap();
    assert_eq!(message["text"], "And now?");
    assert_eq!(message["edited"], json!(["README.md"]));
    let thought: String = seen
        .iter()
        .filter(|e| e["update"]["sessionUpdate"] == "agent_thought_chunk")
        .filter_map(|e| e["update"]["content"]["text"].as_str())
        .collect();
    assert!(thought.contains("changed these files by hand: `README.md`"), "{thought}");

    // The note is given once.
    manager.prompt(&id, "Again".into()).await.unwrap();
    let seen = until(&mut events, &id, |e| state_is(e, "idle")).await;
    assert_eq!(seen.iter().find(|e| is(e, "user_message")).unwrap()["edited"], json!([]));
}

#[tokio::test]
async fn search_finds_files_text_and_things_said() {
    let sandbox = Sandbox::new("search");
    let state = AppState::new(sandbox.manager());
    let manager = &state.manager;
    let project = manager.add_project(&git_project(&sandbox).to_string_lossy(), None).unwrap();
    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock", "What does the limiter do?").await.unwrap();
    let id = task.row.id.clone();
    until(&mut events, &id, |e| state_is(e, "idle")).await;

    let files = dispatch(&state, "search.files", &json!({ "taskId": id, "query": "ratelim" })).await.unwrap();
    assert_eq!(files["hits"][0]["path"], DEMO);

    let text = dispatch(&state, "search.text", &json!({ "taskId": id, "query": "WINDOWMS" })).await.unwrap();
    assert_eq!(
        text["hits"],
        json!([{ "path": DEMO, "line": 2, "column": 2, "length": 8, "text": "  windowMs: 60 * 1000," }])
    );

    // The answer arrived word by word, yet it is found as one sentence.
    let chats = dispatch(&state, "search.chats", &json!({ "query": "five attempts a minute" })).await.unwrap();
    let hits = chats["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["taskId"], id.as_str());
    assert_eq!(hits[0]["who"], "agent");
    let yours = dispatch(&state, "search.chats", &json!({ "query": "limiter do" })).await.unwrap();
    assert_eq!(yours["hits"][0]["who"], "you");
}

/// Collects what a terminal prints until `wanted` shows up.
async fn output_until(events: &mut tokio::sync::broadcast::Receiver<TermEvent>, term_id: u32, wanted: &str) -> String {
    let mut printed = String::new();
    let wait = async {
        loop {
            if let Ok(TermEvent::Data { term_id: from, bytes, .. }) = events.recv().await
                && from == term_id
            {
                printed.push_str(&String::from_utf8_lossy(&bytes));
                if printed.contains(wanted) {
                    break;
                }
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(20), wait)
        .await
        .unwrap_or_else(|_| panic!("the terminal never printed {wanted:?}; it printed: {printed:?}"));
    printed
}

/// Types a command into a terminal and waits for what it should print.
///
/// A shell that has only just started may drop what is typed before it is ready — PowerShell on Windows
/// does, and says nothing about it — so the line is typed again until it takes, as a person would.
async fn type_until(
    terminals: &Terminals,
    events: &mut tokio::sync::broadcast::Receiver<TermEvent>,
    term_id: u32,
    command: &str,
    wanted: &str,
) -> String {
    let mut printed = String::new();
    for _ in 0..6 {
        terminals.input(term_id, command.as_bytes()).unwrap();
        let wait = async {
            loop {
                if let Ok(TermEvent::Data { term_id: from, bytes, .. }) = events.recv().await
                    && from == term_id
                {
                    printed.push_str(&String::from_utf8_lossy(&bytes));
                    if printed.contains(wanted) {
                        break;
                    }
                }
            }
        };
        if tokio::time::timeout(Duration::from_secs(5), wait).await.is_ok() {
            return printed;
        }
    }
    panic!("the terminal never printed {wanted:?}; it printed: {printed:?}")
}

#[tokio::test]
async fn a_terminal_runs_commands_in_the_tasks_folder() {
    let sandbox = Sandbox::new("terminal");
    std::fs::write(sandbox.project().join("marker-file.txt"), "x").unwrap();
    let terminals = Arc::new(Terminals::new());
    let mut events = terminals.subscribe();

    let info = terminals.open("task-1", &sandbox.project(), 100, 30).unwrap();
    assert_eq!(terminals.list("task-1").len(), 1);
    assert!(terminals.list("another-task").is_empty());

    // The answer is built by the shell, so finding it proves the command ran
    // and is not just the typed text echoed back.
    let command = if cfg!(windows) { "echo ae-$(6*7)-ok\r" } else { "echo ae-$((6*7))-ok\n" };
    type_until(&terminals, &mut events, info.term_id, command, "ae-42-ok").await;

    let list = if cfg!(windows) { "dir\r" } else { "ls\n" };
    type_until(&terminals, &mut events, info.term_id, list, "marker-file.txt").await;

    // Someone who joins later still gets what was printed.
    let (kept, total) = terminals.scrollback(info.term_id).unwrap();
    assert!(String::from_utf8_lossy(&kept).contains("ae-42-ok"));
    assert!(total >= kept.len() as u64);
    terminals.resize(info.term_id, 120, 40).unwrap();

    terminals.close(info.term_id).unwrap();
    let ended = async {
        loop {
            if let Ok(TermEvent::Exit { term_id, task_id, purpose, .. }) = events.recv().await {
                assert_eq!((term_id, task_id.as_str(), purpose.as_str()), (info.term_id, "task-1", "shell"));
                break;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(20), ended).await.expect("the shell ends when closed");
    assert!(terminals.list("task-1").is_empty());
    assert!(terminals.input(info.term_id, b"x").is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn the_cores_own_settings_do_not_leak_into_what_it_starts() {
    let sandbox = Sandbox::new("scrub");
    // Cargo sets this for the test process, so it is certain to be in the environment.
    assert!(std::env::var("CARGO_MANIFEST_DIR").is_ok());
    let terminals = Arc::new(Terminals::without(vec!["CARGO_MANIFEST_DIR".into()]));
    let mut events = terminals.subscribe();
    let command = "echo \"kept=[$AE_KEPT] gone=[$CARGO_MANIFEST_DIR]\"";
    let info =
        terminals.run("task-1", &sandbox.project(), command, "run:check", &[("AE_KEPT".into(), "yes".into())]).unwrap();
    let printed = output_until(&mut events, info.term_id, "gone=[").await;
    assert!(printed.contains("kept=[yes] gone=[]"), "{printed}");
}

#[tokio::test]
async fn an_unknown_command_is_named_in_the_error() {
    let sandbox = Sandbox::new("unknown");
    let state = AppState::new(sandbox.manager());
    let error = dispatch(&state, "files.nothing", &Value::Null).await.unwrap_err().to_string();
    assert!(error.contains("files.nothing"), "{error}");
}

#[tokio::test]
async fn a_task_goes_from_an_edit_to_a_pushed_commit() {
    let sandbox = Sandbox::new("commit");
    let state = AppState::new(sandbox.manager());
    let manager = &state.manager;
    let repo = git_project(&sandbox);
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    // A remote that lives next to the project, so the push stays on this machine.
    let remote = sandbox.0.join("remote.git");
    git(&repo, &["init", "-q", "--bare", &remote.to_string_lossy()]);
    git(&repo, &["remote", "add", "origin", &remote.to_string_lossy()]);

    let project = manager.add_project(&repo.to_string_lossy(), None).unwrap();
    let id = task_with_an_allowed_edit(&state, &project.id, "Change the limit").await;
    let cwd = manager.cwd(&id).unwrap().to_string_lossy().into_owned();
    assert_eq!(manager.task(&id).unwrap().row.base_branch.as_deref(), Some("main"));
    dispatch(&state, "files.write", &json!({ "taskId": id, "path": "README.md", "text": "# Shop\nby hand\n" }))
        .await
        .unwrap();

    // The view lists both files; the diff says who wrote the changed line.
    let before = dispatch(&state, "git.state", &json!({ "taskId": id })).await.unwrap();
    assert_eq!(before["base"], "main");
    assert_eq!(before["remote"], "origin");
    let paths: Vec<_> = before["files"].as_array().unwrap().iter().map(|f| f["path"].as_str().unwrap()).collect();
    assert_eq!(paths, vec!["README.md", DEMO]);
    let diff = dispatch(&state, "git.diff", &json!({ "taskId": id, "path": DEMO })).await.unwrap();
    assert_eq!(diff["hunks"][0]["who"], "agent");
    assert_eq!(
        (diff["hunks"][0]["fromLine"].as_u64(), diff["added"].as_u64(), diff["removed"].as_u64()),
        (Some(3), Some(1), Some(1))
    );
    let mine = dispatch(&state, "git.diff", &json!({ "taskId": id, "path": "README.md" })).await.unwrap();
    assert_eq!(mine["hunks"][0]["who"], "you");

    // Only the agent's file is ticked; it is committed and pushed in one go.
    let after = dispatch(
        &state,
        "git.commit",
        &json!({ "taskId": id, "message": "Take the login limit from config", "picks": [{ "path": DEMO }], "push": true }),
    )
    .await
    .unwrap();
    assert_eq!(after["commits"][0]["subject"], "Take the login limit from config");
    assert_eq!(after["commits"][0]["pushed"], true);
    assert_eq!(after["notPushed"], 0);
    assert_eq!(after["files"].as_array().unwrap().len(), 1, "the unticked file is still waiting");
    let on_remote = git(&remote, &["log", "--format=%s", "-1", &manager.task(&id).unwrap().row.branch.unwrap()]);
    assert_eq!(on_remote, "Take the login limit from config");

    // What was committed is no longer marked, and the conversation says what happened.
    assert_eq!(manager.review_of(&id, DEMO).unwrap().agent_lines, 0);
    let history = manager.history(&id).unwrap();
    let actions: Vec<_> = history.iter().filter(|e| is(e, "git")).map(|e| e["action"].as_str().unwrap()).collect();
    assert_eq!(actions, vec!["committed", "pushed"]);

    // The file that was left out is put back as it was.
    dispatch(&state, "git.revert", &json!({ "taskId": id, "path": "README.md" })).await.unwrap();
    assert_eq!(read(&cwd, "README.md"), "# Shop\n");
    assert!(!manager.review_of(&id, "README.md").unwrap().is_changed());

    // A pushed commit is not undone; it is reverted with a new commit.
    let hash = after["commits"][0]["hash"].as_str().unwrap();
    let refused =
        dispatch(&state, "git.undoCommit", &json!({ "taskId": id, "hash": hash })).await.unwrap_err().to_string();
    assert!(refused.contains("already pushed"), "{refused}");
    dispatch(&state, "git.revertCommit", &json!({ "taskId": id, "hash": hash })).await.unwrap();
    assert!(read(&cwd, DEMO).contains("limit: 5,"));
    let last = dispatch(&state, "git.state", &json!({ "taskId": id })).await.unwrap();
    assert_eq!(last["commits"].as_array().unwrap().len(), 2);
    assert_eq!(last["notPushed"], 1);
}

#[tokio::test]
async fn a_folder_without_git_says_so() {
    let sandbox = Sandbox::new("nogit");
    let state = AppState::new(sandbox.manager());
    let manager = &state.manager;
    let project = manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock", "What is here?").await.unwrap();
    until(&mut events, &task.row.id, |e| state_is(e, "idle")).await;
    let error = dispatch(&state, "git.state", &json!({ "taskId": task.row.id })).await.unwrap_err().to_string();
    assert!(error.contains("without git"), "{error}");
}

#[tokio::test]
async fn a_file_with_windows_line_endings_is_marked_line_by_line_like_any_other() {
    let sandbox = Sandbox::new("marks-crlf");
    let state = AppState::new(sandbox.manager());
    let manager = &state.manager;
    let repo = git_project(&sandbox);
    // As git is usually set on Windows: "\n" in the repository, "\r\n" in every working copy.
    git(&repo, &["config", "core.autocrlf", "true"]);
    let project = manager.add_project(&repo.to_string_lossy(), None).unwrap();
    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock", "What does the limiter do?").await.unwrap();
    let id = task.row.id.clone();
    until(&mut events, &id, |e| state_is(e, "idle")).await;
    let cwd = manager.cwd(&id).unwrap().to_string_lossy().into_owned();
    let text = read(&cwd, DEMO);
    assert!(text.contains("\r\n"), "the task's own copy was checked out with Windows endings");

    // One line added outside the editor (in another program, say) is one line marked, not the whole file:
    // the file is compared with its committed text, which git keeps with "\n".
    let opened = dispatch(&state, "files.read", &json!({ "taskId": id, "path": DEMO })).await.unwrap();
    assert_eq!(opened["review"]["runs"], json!([]));
    let mine = format!("// mine\r\n{text}");
    std::fs::write(Path::new(&cwd).join(DEMO), &mine).unwrap();
    let seen = dispatch(&state, "files.read", &json!({ "taskId": id, "path": DEMO })).await.unwrap();
    assert_eq!(seen["review"]["runs"], json!([{ "from": 1, "to": 1, "who": "you" }]));

    // And one more through the editor, at the end.
    let more = format!("{mine}// and this\r\n");
    let saved = dispatch(
        &state,
        "files.write",
        &json!({ "taskId": id, "path": DEMO, "text": more, "version": seen["file"]["version"] }),
    )
    .await
    .unwrap();
    let last = more.matches("\r\n").count();
    assert_eq!(
        saved["review"]["runs"],
        json!([{ "from": 1, "to": 1, "who": "you" }, { "from": last, "to": last, "who": "you" }])
    );
}
