//! Conversations of Claude Code brought in as tasks.
//!
//! The "files of Claude Code" are written here, into a folder of the test:
//! nothing is read from the real `~/.claude`, and no agent with a model is started.

mod common;

use ae_core::agents::{AgentDef, Registry};
use ae_core::tasks::Manager;
use common::*;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

/// One line of a conversation, as Claude Code writes them.
fn line(session: &str, cwd: &Path, branch: &str, at: &str, kind: &str, content: Value) -> String {
    json!({
        "type": kind, "sessionId": session, "cwd": cwd, "gitBranch": branch, "timestamp": at,
        "message": { "role": kind, "content": content }
    })
    .to_string()
}

/// Writes a conversation where Claude Code would keep it for the folder it ran in.
fn keep(claude: &Path, folder: &str, session: &str, lines: &[String]) -> PathBuf {
    let dir = claude.join("projects").join(folder);
    std::fs::create_dir_all(&dir).unwrap();
    let file = dir.join(format!("{session}.jsonl"));
    std::fs::write(&file, lines.join("\n") + "\n").unwrap();
    file
}

const FIRST: &str = "11111111-aaaa-4bbb-8ccc-000000000001";
const SECOND: &str = "22222222-aaaa-4bbb-8ccc-000000000002";
const ELSEWHERE: &str = "33333333-aaaa-4bbb-8ccc-000000000003";
const SILENT: &str = "44444444-aaaa-4bbb-8ccc-000000000004";
const GONE: &str = "55555555-aaaa-4bbb-8ccc-000000000005";

/// A project with two conversations of its own (one in the project's folder, one in a folder inside
/// it), one where nobody said anything, and a conversation of a neighbour whose name begins the same.
fn kept(sandbox: &Sandbox) -> (PathBuf, PathBuf) {
    let project = git_project(sandbox);
    let claude = sandbox.0.join("claude");
    let user = |text: &str| json!([{ "type": "text", "text": text }]);
    keep(
        &claude,
        "-shop",
        FIRST,
        &[
            // What the program put before the user's words is not what they said.
            line(FIRST, &project, "main", "2026-11-10T09:00:00.000Z", "user", json!("<command-name>/clear</command-name>")),
            line(
                FIRST,
                &project,
                "main",
                "2026-11-10T09:00:05.000Z",
                "user",
                user("<system-reminder>Today is Tuesday.</system-reminder>\nTake the login limit from config\nand keep the tests green"),
            ),
            line(
                FIRST,
                &project,
                "main",
                "2026-11-10T09:00:09.000Z",
                "assistant",
                json!([
                    { "type": "thinking", "thinking": "Where is the limit?" },
                    { "type": "text", "text": "I will read the limiter first." },
                    { "type": "tool_use", "id": "t1", "name": "Read", "input": { "file_path": project.join("src/middleware/rateLimit.ts") } }
                ]),
            ),
            line(
                FIRST,
                &project,
                "main",
                "2026-11-10T09:00:10.000Z",
                "user",
                json!([{ "type": "tool_result", "tool_use_id": "t1", "content": "export const loginLimiter = …" }]),
            ),
            line(
                FIRST,
                &project,
                "fix/login-limit",
                "2026-11-10T09:01:00.000Z",
                "assistant",
                json!([
                    { "type": "tool_use", "id": "t2", "name": "Bash", "input": { "command": "npm test" } }
                ]),
            ),
            line(
                FIRST,
                &project,
                "fix/login-limit",
                "2026-11-10T09:01:30.000Z",
                "user",
                json!([{ "type": "tool_result", "tool_use_id": "t2", "is_error": true, "content": [{ "type": "text", "text": "1 failed" }] }]),
            ),
            line(FIRST, &project, "fix/login-limit", "2026-11-10T09:02:00.000Z", "assistant", json!([{ "type": "text", "text": "One test fails; the limit now comes from config." }])),
            // A helper agent's own conversation is not this one's.
            json!({ "type": "assistant", "isSidechain": true, "sessionId": FIRST, "cwd": project, "timestamp": "2026-11-10T09:02:10.000Z",
                    "message": { "role": "assistant", "content": [{ "type": "text", "text": "from a helper" }] } })
            .to_string(),
            line(FIRST, &project, "fix/login-limit", "2026-11-10T09:05:00.000Z", "user", user("Fix that test too")),
            line(FIRST, &project, "fix/login-limit", "2026-11-10T09:06:00.000Z", "assistant", json!([{ "type": "text", "text": "Done: all green." }])),
            "this line is not JSON and is passed over".to_string(),
        ],
    );
    let inside = project.join("packages/web");
    std::fs::create_dir_all(&inside).unwrap();
    keep(
        &claude,
        "-shop-packages-web",
        SECOND,
        &[
            line(SECOND, &inside, "HEAD", "2026-11-12T10:00:00.000Z", "user", json!("Why is the cart slow?")),
            line(
                SECOND,
                &inside,
                "HEAD",
                "2026-11-12T10:00:20.000Z",
                "assistant",
                json!([{ "type": "text", "text": "It asks for every price twice." }]),
            ),
        ],
    );
    keep(
        &claude,
        "-shop",
        SILENT,
        &[line(
            SILENT,
            &project,
            "main",
            "2026-11-13T08:00:00.000Z",
            "user",
            json!("<local-command-stdout>ok</local-command-stdout>"),
        )],
    );
    // A neighbour: Claude Code's name for its folder begins like the project's, the folder is another.
    let neighbour = sandbox.0.join("project-two");
    std::fs::create_dir_all(&neighbour).unwrap();
    keep(
        &claude,
        "-shop-two",
        ELSEWHERE,
        &[line(ELSEWHERE, &neighbour, "main", "2026-11-13T09:00:00.000Z", "user", json!("Not of this project"))],
    );
    (project, claude)
}

#[tokio::test]
async fn the_conversations_of_a_project_are_listed_newest_first_and_only_its_own() {
    let sandbox = Sandbox::new("imports-list");
    let (project_dir, claude) = kept(&sandbox);
    let manager = sandbox.manager();
    let project = manager.add_project(&project_dir.to_string_lossy(), None).unwrap();

    let found = manager.claude_sessions(&project.id, &claude).unwrap();
    let told: Vec<(&str, &str, Option<&str>)> =
        found.iter().map(|one| (one.id.as_str(), one.title.as_str(), one.branch.as_deref())).collect();
    assert_eq!(
        told,
        [
            // The one in a folder inside the project; "HEAD" is Claude Code's word for no branch.
            (SECOND, "Why is the cart slow?", None),
            // Its title is the first thing the user said, and its branch the one it ended on.
            (FIRST, "Take the login limit from config", Some("fix/login-limit")),
        ],
        "the neighbour's conversation and the one nobody spoke in are left out"
    );
    assert_eq!(found[1].started_at, 1_794_301_200_000);
    assert_eq!(found[1].last_at, 1_794_301_560_000);
    assert!(found.iter().all(|one| one.folder_there && one.task_id.is_none()));
    assert_eq!(found[0].cwd, project_dir.join("packages/web").to_string_lossy());

    // A computer without Claude Code has nothing to list, and that is not a failure.
    assert_eq!(manager.claude_sessions(&project.id, &sandbox.0.join("no-claude")).unwrap(), vec![]);
    assert!(manager.claude_sessions("no-such-project", &claude).is_err());
}

#[tokio::test]
async fn a_conversation_becomes_a_sleeping_task_with_its_words_and_tool_calls() {
    let sandbox = Sandbox::new("imports-take");
    let (project_dir, claude) = kept(&sandbox);
    let manager = sandbox.manager();
    let project = manager.add_project(&project_dir.to_string_lossy(), None).unwrap();
    let mut windows = manager.subscribe();

    let task = manager.import_claude_session(&project.id, FIRST, &claude).await.unwrap();
    assert_eq!(task.row.title, "Take the login limit from config");
    assert_eq!(task.row.cwd, project_dir.to_string_lossy(), "it works where the conversation worked");
    assert_eq!(task.row.branch.as_deref(), Some("main"), "on the branch that folder is on now");
    assert_eq!(task.row.created_at, 1_794_301_200_000);
    assert_eq!(serde_json::to_value(task.state).unwrap(), "asleep");
    // These tests know no Claude: the task goes to an agent that can start, and has no session to pick up.
    assert_eq!((task.row.agent_id.as_str(), task.row.agent_session.as_deref()), ("mock-paid", None));
    let told = windows.try_recv().expect("every window is told");
    assert_eq!(
        (told["event"].as_str(), told["data"]["id"].as_str()),
        (Some("task.created"), Some(task.row.id.as_str()))
    );

    let history = manager.history(&task.row.id).unwrap();
    let kinds: Vec<String> = history
        .iter()
        .map(|event| match event["type"].as_str().unwrap() {
            "update" => event["update"]["sessionUpdate"].as_str().unwrap().to_string(),
            "state" => format!("state:{}", event["state"].as_str().unwrap()),
            other => other.to_string(),
        })
        .collect();
    assert_eq!(
        kinds,
        [
            "agent_started",
            "user_message",
            "state:running",
            "agent_message_chunk",
            "tool_call",
            "tool_call_update",
            "tool_call",
            "tool_call_update",
            "agent_message_chunk",
            "turn_end",
            "state:idle",
            "user_message",
            "state:running",
            "agent_message_chunk",
            "turn_end",
            "state:idle",
            "tracker",
            "state:asleep",
        ],
        "the helper's words, the model's thinking and the program's own lines are not in it"
    );
    // Numbered from nought without a gap, each with the time it was said, and all of it read already.
    assert!(history.iter().enumerate().all(|(n, event)| event["seq"] == n && event["taskId"] == task.row.id.as_str()));
    assert_eq!(history[1]["at"], 1_794_301_205_000_u64);
    assert_eq!(task.row.seen_seq, history.len() as i64 - 1);
    assert_eq!(history[1]["text"], "Take the login limit from config\nand keep the tests green");
    assert_eq!(history[4]["update"]["title"], "Read rateLimit.ts");
    assert_eq!(history[6]["update"]["title"], "`npm test`");
    assert_eq!(
        (history[7]["update"]["status"].as_str(), history[7]["update"]["toolCallId"].as_str()),
        (Some("failed"), Some("t2"))
    );
    assert_eq!(history[7]["update"]["content"][0]["content"]["text"], "1 failed");
    assert_eq!(history[7]["update"]["_meta"]["claudeCode"]["toolName"], "Bash");
    assert!(history[16]["text"].as_str().unwrap().starts_with("This conversation was brought in from Claude Code."));

    // Nothing of Claude Code's was changed.
    let file = claude.join("projects/-shop").join(format!("{FIRST}.jsonl"));
    assert!(std::fs::read_to_string(file).unwrap().contains("this line is not JSON"));

    // What cannot be brought in says why.
    let refused = |session: &'static str| {
        let (manager, project, claude) = (manager.clone(), project.id.clone(), claude.clone());
        async move { manager.import_claude_session(&project, session, &claude).await.unwrap_err().to_string() }
    };
    assert_eq!(refused("../../etc/passwd").await, "That is not the name of a conversation");
    assert_eq!(refused(ELSEWHERE).await, "That conversation is not among the ones of this project any more");
    assert_eq!(refused(SILENT).await, "Nobody said anything in that conversation, so there is nothing to bring in");

    // The next message wakes an agent in it, which is told what was said.
    let mut events = manager.subscribe();
    manager.switch_agent(&task.row.id, "mock").await.unwrap();
    manager.prompt(&task.row.id, "And the docs?".into()).await.unwrap();
    let seen = until(&mut events, &task.row.id, |event| state_is(event, "idle")).await;
    let thought = seen
        .iter()
        .find(|event| event["update"]["sessionUpdate"] == "agent_thought_chunk")
        .and_then(|event| event["update"]["content"]["text"].as_str())
        .unwrap_or_default()
        .to_string();
    assert!(thought.contains("User: Take the login limit from config"), "{thought}");
    assert!(thought.contains("Agent: Done: all green."), "{thought}");
    manager.stop_all().await;
}

#[tokio::test]
async fn a_conversation_whose_folder_is_gone_gets_a_working_copy_of_its_own() {
    let sandbox = Sandbox::new("imports-gone");
    let (project_dir, claude) = kept(&sandbox);
    let manager = sandbox.manager();
    let project = manager.add_project(&project_dir.to_string_lossy(), None).unwrap();
    // It worked in a working copy inside the project that was removed since.
    let gone = project_dir.join(".claude/worktrees/old");
    std::fs::create_dir_all(&gone).unwrap();
    keep(
        &claude,
        "-shop--claude-worktrees-old",
        GONE,
        &[
            line(GONE, &gone, "task/old", "2026-11-09T10:00:00.000Z", "user", json!("Rename the limiter")),
            line(
                GONE,
                &gone,
                "task/old",
                "2026-11-09T10:00:20.000Z",
                "assistant",
                json!([{ "type": "text", "text": "Renamed." }]),
            ),
        ],
    );
    // Claude Code's folder is told from its conversations while the folder they worked in is there…
    assert!(
        manager.claude_sessions(&project.id, &claude).unwrap().iter().any(|one| one.id == GONE && one.folder_there)
    );
    std::fs::remove_dir_all(&gone).unwrap();
    // …and when it is gone, the conversation is still the project's: it began inside it.
    let listed = manager.claude_sessions(&project.id, &claude).unwrap();
    let one = listed.iter().find(|one| one.id == GONE).expect("it is still listed");
    assert!(!one.folder_there);

    let task = manager.import_claude_session(&project.id, GONE, &claude).await.unwrap();
    assert!(Path::new(&task.row.cwd).starts_with(sandbox.data().join("worktrees")), "{}", task.row.cwd);
    assert!(Path::new(&task.row.cwd).join("README.md").is_file());
    assert!(task.row.branch.as_deref().is_some_and(|branch| branch.starts_with("task/rename-the-limiter-")));
    assert_eq!(task.row.agent_session, None, "a session cannot be picked up in another folder");
    let history = manager.history(&task.row.id).unwrap();
    let note = history.iter().find(|event| is(event, "tracker")).unwrap();
    assert!(note["text"].as_str().unwrap().contains("The folder it worked in is gone"), "{note}");
    manager.stop_all().await;
}

/// On a computer that knows Claude, the task is Claude's and holds the session, so Claude picks it up.
#[tokio::test]
async fn claude_takes_the_task_with_its_session_and_a_conversation_is_not_brought_in_twice() {
    let sandbox = Sandbox::new("imports-claude");
    let (project_dir, claude) = kept(&sandbox);
    // "Claude" here is the practice agent that can pick an earlier session up. Nothing is spent.
    let stand_in = AgentDef {
        id: "claude".into(),
        name: "Claude".into(),
        short: "Cl".into(),
        vendor: "Agent Editor".into(),
        program: mock_agent().to_string_lossy().into_owned(),
        args: vec![],
        env: vec![("AE_MOCK_DELAY_MS".into(), "0".into()), ("AE_MOCK_LOAD".into(), "1".into())],
        available: true,
        note: None,
        spends_tokens: false,
        speaks: Default::default(),
    };
    let mut agents = vec![stand_in];
    agents.extend(registry().list().iter().cloned());
    let manager = Manager::with_hosts(Registry::with(agents), sandbox.store(), sandbox.data(), no_services());
    let project = manager.add_project(&project_dir.to_string_lossy(), None).unwrap();

    let task = manager.import_claude_session(&project.id, SECOND, &claude).await.unwrap();
    assert_eq!((task.row.agent_id.as_str(), task.row.agent_session.as_deref()), ("claude", Some(SECOND)));
    assert_eq!(task.row.cwd, project_dir.join("packages/web").to_string_lossy());

    // It is listed as the task it is now, and is not brought in twice.
    let found = manager.claude_sessions(&project.id, &claude).unwrap();
    let by_id = |id: &str| found.iter().find(|one| one.id == id).unwrap().task_id.clone();
    assert_eq!((by_id(SECOND), by_id(FIRST)), (Some(task.row.id.clone()), None));
    let again = manager.import_claude_session(&project.id, SECOND, &claude).await.unwrap_err().to_string();
    assert_eq!(again, "That conversation is a task here already: Why is the cart slow?");

    // The next message wakes Claude with the session the conversation had.
    let mut events = manager.subscribe();
    manager.prompt(&task.row.id, "And who calls it?".into()).await.unwrap();
    let seen = until(&mut events, &task.row.id, |event| state_is(event, "idle")).await;
    let started = seen.iter().find(|event| is(event, "agent_started")).expect("the agent started");
    assert_eq!(started["resumed"], true, "{started}");
    // This stand-in never had that session, so it began a new one and was told what was said. The
    // task is still that conversation: it stays marked in the list.
    assert_ne!(manager.task(&task.row.id).unwrap().row.agent_session.as_deref(), Some(SECOND));
    let found = manager.claude_sessions(&project.id, &claude).unwrap();
    assert_eq!(found.iter().find(|one| one.id == SECOND).unwrap().task_id.as_deref(), Some(task.row.id.as_str()));
    manager.stop_all().await;
    // Once the task is archived, the conversation can be brought in again.
    manager.archive(&task.row.id).await.unwrap();
    let found = manager.claude_sessions(&project.id, &claude).unwrap();
    assert_eq!(found.iter().find(|one| one.id == SECOND).unwrap().task_id, None);
    manager.import_claude_session(&project.id, SECOND, &claude).await.unwrap();
}
