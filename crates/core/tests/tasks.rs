//! End-to-end tests of tasks against the practice agent.
//!
//! No real agent is started here, so nothing is spent on anyone's account.

mod common;

use ae_core::tasks::TaskState;
use common::*;
use std::path::Path;

#[tokio::test]
async fn a_task_streams_an_answer_and_ends_the_turn() {
    let sandbox = Sandbox::new("answer");
    let manager = sandbox.manager();
    let project = manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    assert_eq!(project.name, "project");

    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock", "What does the limiter do?\nSecond line").await.unwrap();
    assert_eq!(task.row.title, "What does the limiter do?");
    assert_eq!(task.row.branch, None, "a folder without git is used as it is");
    assert_eq!(task.row.cwd, sandbox.project().to_string_lossy());

    let seen = until(&mut events, &task.row.id, |e| state_is(e, "idle")).await;
    assert!(seen.iter().any(|e| is(e, "agent_started") && e["resumed"] == false));
    assert!(seen.iter().any(|e| is(e, "user_message")));
    assert!(seen.iter().any(|e| e["update"]["sessionUpdate"] == "plan"));
    assert_eq!(seen.iter().find(|e| is(e, "turn_end")).unwrap()["stopReason"], "end_turn");
    assert_eq!(
        answer_text(&seen),
        "The limiter allows five attempts a minute and answers 429 after that. Nothing was changed."
    );

    // What was stored is what was sent, in order.
    let history = manager.history(&task.row.id).unwrap();
    assert_eq!(history.len(), seen.len());
    let seqs: Vec<u64> = history.iter().map(|e| e["seq"].as_u64().unwrap()).collect();
    assert_eq!(seqs, (0..history.len() as u64).collect::<Vec<_>>());
}

#[tokio::test]
async fn an_edit_waits_for_permission_and_runs_when_allowed() {
    let sandbox = Sandbox::new("allow");
    let manager = sandbox.manager();
    let project = manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock", "Please edit the limit").await.unwrap();
    let id = task.row.id;

    let before = until(&mut events, &id, |e| is(e, "permission_request")).await;
    let request = before.last().unwrap();
    assert_eq!(request["toolCall"]["kind"], "edit");
    assert_eq!(request["options"].as_array().unwrap().len(), 3);
    assert_eq!(manager.task(&id).unwrap().state, TaskState::WaitingPermission);

    let request_id = request["requestId"].as_str().unwrap();
    manager.answer_permission(&id, request_id, Some("allow-once".into())).unwrap();
    let after = until(&mut events, &id, |e| state_is(e, "idle")).await;
    assert!(after.iter().any(|e| is(e, "permission_resolved")));
    assert!(
        after
            .iter()
            .any(|e| e["update"]["sessionUpdate"] == "tool_call_update" && e["update"]["status"] == "completed")
    );
    assert!(answer_text(&after).starts_with("Done."));
    // The same question cannot be answered twice.
    assert!(manager.answer_permission(&id, request_id, Some("allow-once".into())).is_err());
}

#[tokio::test]
async fn a_refused_edit_is_not_made_and_cancel_stops_the_turn() {
    let sandbox = Sandbox::new("refuse");
    let manager = sandbox.manager();
    let project = manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock", "fix it").await.unwrap();
    let id = task.row.id;

    let before = until(&mut events, &id, |e| is(e, "permission_request")).await;
    let request_id = before.last().unwrap()["requestId"].as_str().unwrap().to_string();
    manager.answer_permission(&id, &request_id, Some("reject-once".into())).unwrap();
    let after = until(&mut events, &id, |e| state_is(e, "idle")).await;
    assert!(answer_text(&after).starts_with("I left the file as it was"));

    // A second turn in the same task, cancelled while the agent waits for permission.
    manager.prompt(&id, "change the limit".into()).await.unwrap();
    until(&mut events, &id, |e| is(e, "permission_request")).await;
    let busy = manager.prompt(&id, "and another".into()).await.unwrap_err().to_string();
    assert!(busy.contains("still working"), "{busy}");
    manager.cancel(&id).unwrap();
    let cancelled = until(&mut events, &id, |e| state_is(e, "idle")).await;
    assert_eq!(cancelled.iter().find(|e| is(e, "turn_end")).unwrap()["stopReason"], "cancelled");
    assert!(cancelled.iter().any(|e| is(e, "permission_resolved") && e["optionId"].is_null()));
}

#[tokio::test]
async fn a_task_survives_a_restart_and_wakes_with_its_history() {
    let sandbox = Sandbox::new("restart");
    let task_id;
    {
        let manager = sandbox.manager();
        let project = manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
        let mut events = manager.subscribe();
        let task = manager.create(&project.id, "mock", "What does the limiter do?").await.unwrap();
        task_id = task.row.id.clone();
        until(&mut events, &task_id, |e| state_is(e, "idle")).await;
        // The core goes away: every agent is stopped with it.
        manager.stop_all().await;
        until(&mut events, &task_id, |e| state_is(e, "asleep")).await;
    }

    // A new core over the same data.
    let manager = sandbox.manager();
    assert_eq!(manager.projects().unwrap().len(), 1);
    let tasks = manager.list().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].state, TaskState::Asleep);
    let before = manager.history(&task_id).unwrap();
    assert!(before.iter().any(|e| is(e, "user_message") && e["text"] == "What does the limiter do?"));

    // The next message wakes the task with a fresh agent that is told what was said.
    let mut events = manager.subscribe();
    manager.prompt(&task_id, "And who calls it?".into()).await.unwrap();
    let seen = until(&mut events, &task_id, |e| state_is(e, "idle")).await;
    assert!(seen.iter().any(|e| is(e, "agent_started") && e["resumed"] == true));
    let thought = seen
        .iter()
        .find(|e| e["update"]["sessionUpdate"] == "agent_thought_chunk")
        .and_then(|e| e["update"]["content"]["text"].as_str())
        .unwrap()
        .to_string();
    assert!(thought.contains("continues an earlier conversation"), "{thought}");
    assert!(thought.contains("User: What does the limiter do?"), "{thought}");
    assert!(thought.contains("Agent: The limiter allows five attempts"), "{thought}");
    assert!(thought.contains("And who calls it?"), "{thought}");

    // The user sees only what they typed, and numbering goes on where it stopped.
    let user = seen.iter().find(|e| is(e, "user_message")).unwrap();
    assert_eq!(user["text"], "And who calls it?");
    let after = manager.history(&task_id).unwrap();
    assert_eq!(after.len(), before.len() + seen.len());
    assert_eq!(after.last().unwrap()["seq"].as_u64().unwrap(), after.len() as u64 - 1);
}

#[tokio::test]
async fn history_is_put_right_after_the_core_stopped_without_warning() {
    let sandbox = Sandbox::new("crash");
    let first = sandbox.manager();
    let project = first.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    let mut events = first.subscribe();
    let task = first.create(&project.id, "mock", "edit the limit").await.unwrap();
    let id = task.row.id;
    until(&mut events, &id, |e| is(e, "permission_request")).await;

    // A second core over the same data, as after a crash: the first never said goodbye.
    let second = sandbox.manager();
    second.recover().unwrap();
    let history = second.history(&id).unwrap();
    let last = history.last().unwrap();
    assert!(state_is(last, "asleep"), "{last}");
    assert!(history.iter().any(|e| is(e, "permission_resolved") && e["optionId"].is_null()));
    assert_eq!(second.list().unwrap()[0].state, TaskState::Asleep);

    // Recovering again changes nothing.
    second.recover().unwrap();
    assert_eq!(second.history(&id).unwrap().len(), history.len());
    first.stop_all().await;
}

#[tokio::test]
async fn a_git_project_gives_each_task_its_own_working_copy() {
    let sandbox = Sandbox::new("worktree");
    let repo = sandbox.project();
    git(&repo, &["init", "-q", "-b", "main"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    git(&repo, &["config", "user.name", "Test"]);
    std::fs::write(repo.join("readme.md"), "hello\n").unwrap();
    git(&repo, &["add", "."]);
    git(&repo, &["commit", "-q", "-m", "first"]);

    let manager = sandbox.manager();
    let project = manager.add_project(&repo.to_string_lossy(), Some("Shop".into())).unwrap();
    let mut events = manager.subscribe();
    let first = manager.create(&project.id, "mock", "Add rate limiting to /login").await.unwrap();
    let second = manager.create(&project.id, "mock", "Add rate limiting to /login").await.unwrap();
    // "Add" makes the practice agent ask before it edits: that is as far as this test needs to go.
    until(&mut events, &second.row.id, |e| is(e, "permission_request")).await;

    let branch = first.row.branch.clone().expect("a branch was made");
    assert!(branch.starts_with("task/add-rate-limiting-to-login-"), "{branch}");
    assert_ne!(first.row.branch, second.row.branch, "two tasks never share a branch");
    assert_ne!(first.row.cwd, second.row.cwd, "two tasks never share a folder");
    assert!(Path::new(&first.row.cwd).starts_with(sandbox.data().join("worktrees")));
    assert!(Path::new(&first.row.cwd).join("readme.md").is_file(), "the working copy has the project's files");
    assert!(git(&repo, &["branch", "--list", &branch]).contains(&branch));
    // The project's own folder is untouched: still on main.
    assert_eq!(git(&repo, &["rev-parse", "--abbrev-ref", "HEAD"]), "main");
}

#[tokio::test]
async fn archiving_takes_a_task_off_the_lists_and_stops_its_agent() {
    let sandbox = Sandbox::new("archive");
    let manager = sandbox.manager();
    let project = manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock", "write something").await.unwrap();
    until(&mut events, &task.row.id, |e| is(e, "permission_request")).await;

    assert!(manager.remove_project(&project.id).is_err(), "a project with open tasks stays");
    manager.archive(&task.row.id).await.unwrap();
    assert!(manager.list().unwrap().is_empty());
    assert!(manager.prompt(&task.row.id, "hello".into()).await.is_err());
    manager.remove_project(&project.id).unwrap();
    assert!(manager.projects().unwrap().is_empty());
}

#[tokio::test]
async fn bad_requests_are_refused_with_a_reason() {
    let sandbox = Sandbox::new("bad");
    let manager = sandbox.manager();
    let missing = sandbox.0.join("no-such-folder");
    assert!(manager.add_project(&missing.to_string_lossy(), None).unwrap_err().to_string().contains("No such folder"));
    assert!(manager.add_project("  ", None).is_err());

    let project = manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    let again = manager.add_project(&sandbox.project().to_string_lossy(), None).unwrap();
    assert_eq!(project.id, again.id, "the same folder is one project");

    assert!(manager.create(&project.id, "nobody", "hi").await.unwrap_err().to_string().contains("Unknown agent"));
    assert!(manager.create(&project.id, "mock", "   ").await.unwrap_err().to_string().contains("Describe the task"));
    assert!(manager.create("no-such-project", "mock", "hi").await.is_err());
    assert!(manager.prompt("no-such-task", "hi".into()).await.is_err());
    assert!(manager.list().unwrap().is_empty(), "nothing was left behind by the failed attempts");
}

#[tokio::test]
async fn an_agent_that_does_not_start_leaves_nothing_behind() {
    let sandbox = Sandbox::new("nostart");
    let repo = git_project(&sandbox);
    // An agent whose program ends at once without saying a word of the protocol.
    let broken = ae_core::agents::AgentDef {
        id: "broken".into(),
        name: "Broken agent".into(),
        short: "Br".into(),
        vendor: "Test".into(),
        program: if cfg!(windows) { "cmd".into() } else { "false".into() },
        args: if cfg!(windows) { vec!["/C".into(), "exit 1".into()] } else { vec![] },
        env: vec![],
        available: true,
        note: None,
        spends_tokens: false,
    };
    let store = ae_core::store::Store::open(&sandbox.data().join("test.db")).unwrap();
    let manager = ae_core::tasks::Manager::with_hosts(
        ae_core::agents::Registry::with(vec![broken]),
        store,
        sandbox.data(),
        no_services(),
    );
    let project = manager.add_project(&repo.to_string_lossy(), None).unwrap();

    let error = manager.create(&project.id, "broken", "Do something").await.unwrap_err().to_string();
    assert!(error.contains("Broken agent did not start"), "{error}");
    assert!(manager.list().unwrap().is_empty(), "no task was made");
    // The working copy and the branch made for it are gone again.
    let copies = sandbox.data().join("worktrees");
    let left: Vec<_> = walk(&copies).into_iter().filter(|path| path.join(".git").exists()).collect();
    assert!(left.is_empty(), "left behind: {left:?}");
    assert!(!git(&repo, &["branch", "--list"]).contains("task/"), "the branch is gone too");
}

/// Every folder under `dir`, at any depth.
fn walk(dir: &Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(walk(&path));
            found.push(path);
        }
    }
    found
}
