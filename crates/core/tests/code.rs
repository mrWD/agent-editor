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

/// Starts a task that asks for nothing and waits until its first turn is over.
async fn quiet_task(state: &AppState, project_id: &str, text: &str) -> String {
    let mut events = state.manager.subscribe();
    let task = state.manager.create(project_id, "mock", text).await.unwrap();
    until(&mut events, &task.row.id, |e| state_is(e, "idle")).await;
    task.row.id
}

#[tokio::test]
async fn search_finds_commits_of_the_branch_and_of_the_project_and_shows_one() {
    let sandbox = Sandbox::new("commits");
    let state = AppState::new(sandbox.manager());
    let manager = &state.manager;
    let repo = git_project(&sandbox);
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    let project = manager.add_project(&repo.to_string_lossy(), None).unwrap();
    let id = quiet_task(&state, &project.id, "What does the limiter do?").await;
    let other = quiet_task(&state, &project.id, "And the README?").await;

    dispatch(&state, "files.write", &json!({ "taskId": id, "path": "README.md", "text": "# Shop\nby hand\n" }))
        .await
        .unwrap();
    let made = dispatch(
        &state,
        "git.commit",
        &json!({ "taskId": id, "message": "Say who wrote the README", "picks": [{ "path": "README.md" }] }),
    )
    .await
    .unwrap();
    let hash = made["commits"][0]["hash"].as_str().unwrap().to_string();

    // The task's own commit, in any letter case, with who made it and when.
    let found = dispatch(&state, "search.commits", &json!({ "taskId": id, "query": "the readme" })).await.unwrap();
    let hits = found["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(
        (&hits[0]["hash"], &hits[0]["subject"], &hits[0]["author"], &hits[0]["onBranch"]),
        (&json!(hash), &json!("Say who wrote the README"), &json!("Test"), &json!(true))
    );
    assert_eq!(hits[0]["short"].as_str().unwrap(), &hash[..hits[0]["short"].as_str().unwrap().len()]);
    assert!(hits[0]["at"].as_u64().unwrap() > 1_600_000_000_000);
    // The commit the project started with is the project's, not the branch's.
    let start = dispatch(&state, "search.commits", &json!({ "taskId": id, "query": "start" })).await.unwrap();
    assert_eq!((&start["hits"][0]["subject"], &start["hits"][0]["onBranch"]), (&json!("Start"), &json!(false)));
    // Another task of the project does not have this task's commit; it has the project's.
    let elsewhere = dispatch(&state, "search.commits", &json!({ "taskId": other, "query": "readme" })).await.unwrap();
    assert_eq!(elsewhere["hits"], json!([]));
    let shared = dispatch(&state, "search.commits", &json!({ "taskId": other, "query": "Start" })).await.unwrap();
    assert_eq!(shared["hits"].as_array().unwrap().len(), 1);

    // The commit itself: its files, then one file line by line.
    let shown = dispatch(&state, "git.show", &json!({ "taskId": id, "hash": hash })).await.unwrap();
    assert_eq!((&shown["subject"], &shown["onBranch"]), (&json!("Say who wrote the README"), &json!(true)));
    assert_eq!(
        shown["files"],
        json!([{ "path": "README.md", "kind": "modified", "added": 1, "removed": 0, "binary": false, "changes": 1 }])
    );
    let diff = dispatch(&state, "git.show", &json!({ "taskId": id, "hash": hash, "path": "README.md" })).await.unwrap();
    let added: Vec<_> = diff["hunks"][0]["lines"].as_array().unwrap().iter().filter(|l| l["kind"] == "add").collect();
    assert_eq!(added.len(), 1);
    assert_eq!(added[0]["text"], "by hand");
    let refused = dispatch(&state, "git.show", &json!({ "taskId": id, "hash": "HEAD" })).await.unwrap_err();
    assert!(refused.to_string().contains("not the id of a commit"), "{refused}");
}

#[tokio::test]
async fn a_folder_without_git_has_no_commits_to_find() {
    let sandbox = Sandbox::new("nocommits");
    let state = AppState::new(sandbox.manager());
    let project = state.manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    let id = quiet_task(&state, &project.id, "What is here?").await;
    let found = dispatch(&state, "search.commits", &json!({ "taskId": id, "query": "start" })).await.unwrap();
    assert_eq!(found["hits"], json!([]));
    let refused = dispatch(&state, "git.show", &json!({ "taskId": id, "hash": "abcd1234" })).await.unwrap_err();
    assert!(refused.to_string().contains("without git"), "{refused}");
}

#[tokio::test]
async fn search_looks_in_every_task_and_lists_a_file_once_for_its_copies() {
    let sandbox = Sandbox::new("everywhere");
    let state = AppState::new(sandbox.manager());
    let manager = &state.manager;
    let shop = manager.add_project(&git_project(&sandbox).to_string_lossy(), None).unwrap();
    // A second project with a file of the same name and one of its own.
    let blog_dir = sandbox.0.join("blog");
    std::fs::create_dir_all(blog_dir.join("src/middleware")).unwrap();
    std::fs::write(blog_dir.join("src/middleware/rateLimit.ts"), "export const windowMs = 1000;\n").unwrap();
    std::fs::write(blog_dir.join("src/posts.ts"), "export const posts = [];\n").unwrap();
    git(&blog_dir, &["init", "-q", "-b", "main"]);
    git(&blog_dir, &["add", "."]);
    git(&blog_dir, &["-c", "user.name=Test", "-c", "user.email=test@example.com", "commit", "-q", "-m", "Start"]);
    let blog = manager.add_project(&blog_dir.to_string_lossy(), None).unwrap();

    let here = quiet_task(&state, &shop.id, "What does the limiter do?").await;
    let sibling = quiet_task(&state, &shop.id, "And the README?").await;
    let far = quiet_task(&state, &blog.id, "What is in the blog?").await;

    let found = |hits: &Value| -> Vec<(String, String, String)> {
        hits["hits"]
            .as_array()
            .unwrap()
            .iter()
            .map(|hit| {
                let text = |name: &str| hit[name].as_str().unwrap().to_string();
                (text("taskId"), text("projectId"), text("path"))
            })
            .collect()
    };
    let by_name = json!({ "taskId": here, "query": "ratelim" });

    // The other project's file is found and says whose it is. The sibling task's
    // copy of the file on screen is the same file: it is not listed again.
    let files = dispatch(&state, "everywhere.files", &by_name).await.unwrap();
    assert_eq!(found(&files), vec![(far.clone(), blog.id.clone(), DEMO.to_string())]);
    assert_eq!(files["hits"][0]["positions"], json!([15, 16, 17, 18, 19, 20, 21]));

    // Once the sibling's copy differs, it is a file of its own.
    let sibling_dir = manager.cwd(&sibling).unwrap();
    std::fs::write(sibling_dir.join(DEMO), "export const loginLimiter = rateLimit({ windowMs: 5 });\n").unwrap();
    let files = dispatch(&state, "everywhere.files", &by_name).await.unwrap();
    let mut listed = found(&files);
    listed.sort();
    let mut wanted =
        vec![(far.clone(), blog.id.clone(), DEMO.to_string()), (sibling.clone(), shop.id.clone(), DEMO.to_string())];
    wanted.sort();
    assert_eq!(listed, wanted);

    // Text: the line of the other project and the changed line of the sibling; the unchanged README is left out.
    let text = dispatch(&state, "everywhere.text", &json!({ "taskId": here, "query": "WINDOWMS" })).await.unwrap();
    let lines: Vec<_> = text["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|hit| (hit["taskId"].as_str().unwrap(), hit["line"].as_u64().unwrap(), hit["text"].as_str().unwrap()))
        .collect();
    assert_eq!(
        lines,
        vec![
            (far.as_str(), 1, "export const windowMs = 1000;"),
            (sibling.as_str(), 1, "export const loginLimiter = rateLimit({ windowMs: 5 });"),
        ],
        "newest task first"
    );
    assert_eq!((&text["hits"][0]["column"], &text["hits"][0]["length"]), (&json!(13), &json!(8)));
    let readme = dispatch(&state, "everywhere.text", &json!({ "taskId": here, "query": "# Shop" })).await.unwrap();
    assert_eq!(readme["hits"], json!([]));
    let only_one = json!({ "taskId": here, "query": "windowms", "limit": 1 });
    let one = dispatch(&state, "everywhere.text", &only_one).await.unwrap();
    assert_eq!(one["hits"].as_array().unwrap().len(), 1, "the limit is respected");

    // With no task on screen every task is looked at, and a project's file is still listed once per copy.
    let all = dispatch(&state, "everywhere.files", &json!({ "query": "README" })).await.unwrap();
    assert_eq!(found(&all), vec![(sibling.clone(), shop.id.clone(), "README.md".to_string())]);
    let all = dispatch(&state, "everywhere.files", &json!({ "query": "ratelim" })).await.unwrap();
    assert_eq!(found(&all).len(), 3, "the changed copy, the first copy and the other project's file");
    assert_eq!(dispatch(&state, "everywhere.files", &json!({ "query": " " })).await.unwrap()["hits"], json!([]));
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

/// True while some process on this machine has exactly this command line.
#[cfg(unix)]
fn is_running(command_line: &str) -> bool {
    let listed = std::process::Command::new("ps").args(["-ax", "-ww", "-o", "args="]).output().expect("ps runs");
    String::from_utf8_lossy(&listed.stdout).lines().any(|line| line.trim() == command_line)
}

#[cfg(unix)]
#[tokio::test]
async fn a_task_has_several_shells_each_with_a_number_and_the_command_that_runs_in_it() {
    let sandbox = Sandbox::new("shells");
    let terminals = Arc::new(Terminals::new());
    let mut events = terminals.subscribe();
    let folder = sandbox.project();

    let first = terminals.open("task-1", &folder, 100, 30).unwrap();
    let second = terminals.open("task-1", &folder, 100, 30).unwrap();
    let elsewhere = terminals.open("task-2", &folder, 100, 30).unwrap();
    // Numbers count inside a task; a program of the Run tab has none.
    assert_eq!((first.number, second.number, elsewhere.number), (Some(1), Some(2), Some(1)));
    let dev = terminals.run("task-1", &folder, "sleep 20", "run:dev", &[]).unwrap();
    assert_eq!(dev.number, None);
    let listed = terminals.list("task-1");
    let shells: Vec<_> = listed.iter().filter(|info| info.purpose == "shell").map(|info| info.term_id).collect();
    assert_eq!(shells, vec![first.term_id, second.term_id]);

    // Two shells, not two views of one: what is set in the first is unknown to the second.
    terminals.input(first.term_id, b"mark=one; echo \"first-[$mark]\"\n").unwrap();
    output_until(&mut events, first.term_id, "first-[one]").await;
    terminals.input(second.term_id, b"echo \"second-[$mark]\"\n").unwrap();
    output_until(&mut events, second.term_id, "second-[]").await;
    // At its prompt a shell has no command to be named after, and nothing to ask about before closing.
    assert_eq!(terminals.running(first.term_id).unwrap(), None);

    // A command that stays in front of the second shell. Its length is this test's own, so it can be found later.
    let sleep = format!("sleep 3{:04}", std::process::id() % 10_000);
    terminals.input(second.term_id, format!("{sleep}\n").as_bytes()).unwrap();
    let named = async {
        loop {
            let running = terminals.list("task-1").into_iter().find(|info| info.term_id == second.term_id);
            if running.and_then(|info| info.running).as_deref() == Some(sleep.as_str()) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(20), named).await.expect("the shell is named after its command");
    assert_eq!(terminals.running(second.term_id).unwrap().as_deref(), Some(sleep.as_str()));
    let first_now = terminals.list("task-1").into_iter().find(|info| info.term_id == first.term_id).unwrap();
    assert_eq!(first_now.running, None, "the other shell is still at its prompt");
    // A program of the Run tab is not a shell: there is no "in front" to speak of.
    assert_eq!(terminals.running(dev.term_id).unwrap(), None);
    assert!(is_running(&sleep));

    // Closing the shell ends what ran in it, and the event says it was closed on purpose.
    terminals.stop(second.term_id).unwrap();
    let left: Vec<_> = terminals.list("task-1").into_iter().filter_map(|info| info.number).collect();
    assert_eq!(left, vec![1], "a closed shell is off the list at once, before its program is gone");
    let ended = async {
        loop {
            if let Ok(TermEvent::Exit { term_id, closed, .. }) = events.recv().await
                && term_id == second.term_id
            {
                assert!(closed);
                break;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(20), ended).await.expect("the shell ends when closed");
    let gone = async {
        while is_running(&sleep) {
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    };
    tokio::time::timeout(Duration::from_secs(20), gone).await.expect("the command ends with its shell");

    // The next shell gets the number after the highest one still open: 2 again, then 3.
    let third = terminals.open("task-1", &folder, 100, 30).unwrap();
    assert_eq!(third.number, Some(2));
    assert_eq!(terminals.open("task-1", &folder, 100, 30).unwrap().number, Some(3));

    // A shell that ends by itself was not closed by anyone.
    terminals.input(first.term_id, b"exit\n").unwrap();
    let left = async {
        loop {
            if let Ok(TermEvent::Exit { term_id, closed, .. }) = events.recv().await
                && term_id == first.term_id
            {
                assert!(!closed);
                break;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(20), left).await.expect("the shell ends on exit");
    terminals.close_all();
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
