//! A race: one task text to several agents, each in its own copy; the user picks one result.
//!
//! Only practice agents run here. They are told apart by a word on their
//! command line, so a test can count which of them are still running.
#![cfg(unix)]

mod common;

use ae_core::agents::{AgentDef, Registry};
use ae_core::server::{AppState, dispatch};
use ae_core::tasks::{Manager, TaskState};
use common::*;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const DEMO: &str = "src/middleware/rateLimit.ts";
/// A question: the practice agent answers and changes nothing.
const ASK: &str = "What does the limiter do?";
/// A request for an edit: the practice agent asks before it changes the demo file.
const EDIT: &str = "Fix the login limit";

/// A core with three practice agents that can race ("red", "green", "blue"), one
/// whose program ends at once ("broken") and one that is not installed ("missing").
struct Track {
    sandbox: Sandbox,
    repo: PathBuf,
    state: AppState,
    manager: Arc<Manager>,
    project_id: String,
    /// The word on the command line of every agent this core starts.
    marker: String,
}

impl Track {
    fn new(name: &str) -> Self {
        let sandbox = Sandbox::new(name);
        let repo = git_project(&sandbox);
        Self::in_folder(sandbox, repo, name)
    }

    fn in_folder(sandbox: Sandbox, repo: PathBuf, name: &str) -> Self {
        let marker = format!("ae-race-{name}-{}", std::process::id());
        let practice = |id: &str, title: &str| AgentDef {
            id: id.into(),
            name: title.into(),
            short: title[..2].into(),
            vendor: "Agent Editor".into(),
            program: mock_agent().to_string_lossy().into_owned(),
            // The practice agent ignores words it does not know.
            args: vec![marker.clone()],
            env: vec![("AE_MOCK_DELAY_MS".into(), "0".into())],
            available: true,
            note: None,
            spends_tokens: false,
            speaks: Default::default(),
        };
        let mut broken = practice("broken", "Broken agent");
        broken.program = "false".into();
        let mut missing = practice("missing", "Missing agent");
        missing.available = false;
        missing.note = Some("Install it first".into());
        let registry = Registry::with(vec![
            practice("red", "Red agent"),
            practice("green", "Green agent"),
            practice("blue", "Blue agent"),
            practice("grey", "Grey agent"),
            practice("pink", "Pink agent"),
            broken,
            missing,
        ]);
        let manager = Manager::with_hosts(registry, sandbox.store(), sandbox.data(), no_services());
        let project_id = manager.add_project(&repo.to_string_lossy(), Some("Shop".into())).unwrap().id;
        Self { state: AppState::new(manager.clone()), sandbox, repo, manager, project_id, marker }
    }

    async fn ask(&self, method: &str, params: Value) -> anyhow::Result<Value> {
        dispatch(&self.state, method, &params).await
    }

    /// Starts a race and returns what the core answered.
    async fn start(&self, agents: &[&str], text: &str) -> Value {
        self.ask("race.start", json!({ "projectId": self.project_id, "agentIds": agents, "text": text }))
            .await
            .expect("the race starts")
    }

    async fn get(&self, race_id: &str) -> Value {
        self.ask("race.get", json!({ "raceId": race_id })).await.expect("the race is read")
    }

    /// Waits until a task is in `state`.
    async fn until_state(&self, task_id: &str, state: TaskState) {
        for _ in 0..2000 {
            if self.manager.task(task_id).is_ok_and(|task| task.state == state) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("task {task_id} never became {state:?}: {:#?}", self.manager.history(task_id));
    }

    /// Answers the question an entrant asked before its edit, and waits for the end of its turn.
    async fn answer(&self, task_id: &str, option: &str) {
        self.until_state(task_id, TaskState::WaitingPermission).await;
        let history = self.manager.history(task_id).unwrap();
        let request = history.iter().rev().find(|event| is(event, "permission_request")).expect("it asked");
        self.manager
            .answer_permission(task_id, request["requestId"].as_str().unwrap(), Some(option.to_string()))
            .unwrap();
        self.until_state(task_id, TaskState::Idle).await;
    }

    /// How many agent processes of this core are running.
    fn agents_running(&self) -> usize {
        let found = std::process::Command::new("pgrep").args(["-f", &self.marker]).output().expect("pgrep runs");
        String::from_utf8_lossy(&found.stdout).split_whitespace().count()
    }

    /// Waits until exactly `count` agent processes are left.
    async fn until_running(&self, count: usize) {
        for _ in 0..300 {
            if self.agents_running() == count {
                return;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        panic!("{} agents are running, not {count}", self.agents_running());
    }

    /// The working copies the core has made and not removed.
    fn copies(&self) -> Vec<PathBuf> {
        walk(&self.sandbox.data().join("worktrees")).into_iter().filter(|path| path.join(".git").exists()).collect()
    }

    /// The task branches the project's repository has.
    fn branches(&self) -> Vec<String> {
        git(&self.repo, &["branch", "--list", "task/*", "--format=%(refname:short)"])
            .lines()
            .map(str::to_string)
            .collect()
    }
}

/// Every folder under `dir`, at any depth.
fn walk(dir: &Path) -> Vec<PathBuf> {
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

fn ids(value: &Value) -> Vec<String> {
    value.as_array().unwrap().iter().map(|id| id.as_str().unwrap().to_string()).collect()
}

fn entrant<'a>(race: &'a Value, task_id: &str) -> &'a Value {
    race["entrants"].as_array().unwrap().iter().find(|one| one["taskId"] == task_id).expect("the entrant is listed")
}

fn refusal(outcome: anyhow::Result<Value>) -> String {
    format!("{:#}", outcome.expect_err("it is refused"))
}

#[tokio::test]
async fn two_agents_get_the_same_task_each_in_its_own_copy() {
    let track = Track::new("two");
    let mut windows = track.manager.subscribe();
    let started = track.start(&["red", "green"], ASK).await;

    let race_id = started["race"]["id"].as_str().unwrap().to_string();
    let tasks = ids(&started["race"]["tasks"]);
    assert_eq!(tasks.len(), 2);
    assert_eq!(started["failed"], json!([]));
    let listed = started["tasks"].as_array().unwrap();
    assert_eq!((listed[0]["agentId"].as_str(), listed[1]["agentId"].as_str()), (Some("red"), Some("green")));
    for task in listed {
        assert_eq!(task["title"], ASK, "every entrant is an ordinary task with the same text");
        assert_eq!(task["race"], json!({ "id": race_id, "tasks": tasks }), "each knows its race");
        assert!(Path::new(task["cwd"].as_str().unwrap()).starts_with(track.sandbox.data().join("worktrees")));
        assert!(Path::new(task["cwd"].as_str().unwrap()).join(DEMO).is_file(), "the copy has the project's files");
    }
    assert_ne!(listed[0]["cwd"], listed[1]["cwd"], "two entrants never share a folder");
    assert_ne!(listed[0]["branch"], listed[1]["branch"], "nor a branch");
    assert_eq!((track.copies().len(), track.branches().len()), (2, 2));

    // Every window is told, and the list of tasks says the same.
    let mut told = false;
    while let Ok(message) = windows.try_recv() {
        told |= message["event"] == "race.changed"
            && message["data"] == json!({ "raceId": race_id, "tasks": tasks, "decided": false, "picked": null });
    }
    assert!(told, "the start of a race is announced");
    for task in track.manager.list().unwrap() {
        assert_eq!(task.race.as_ref().map(|race| race.id.as_str()), Some(race_id.as_str()));
        assert_eq!(track.manager.history(&task.row.id).unwrap()[1]["text"], ASK, "the same first message");
    }

    for id in &tasks {
        track.until_state(id, TaskState::Idle).await;
    }
    let race = track.get(&race_id).await;
    assert_eq!((race["text"].as_str(), race["projectId"].as_str()), (Some(ASK), Some(track.project_id.as_str())));
    assert_eq!(race["agentIds"], json!(["red", "green"]));
    assert_eq!(race["tasks"], json!(tasks));
    assert!(race["startedAt"].as_u64().unwrap() > 0);
    assert_eq!(
        (race["over"].as_bool(), race["picked"].is_null(), race["discarded"].as_bool()),
        (Some(true), true, Some(false))
    );
    let names: Vec<&str> =
        race["entrants"].as_array().unwrap().iter().map(|one| one["agentName"].as_str().unwrap()).collect();
    assert_eq!(names, ["Red agent", "Green agent"]);
    // The same race is found from any of its tasks.
    let by_task = track.ask("race.get", json!({ "taskId": tasks[1] })).await.unwrap();
    assert_eq!(by_task["id"], race_id.as_str());
    track.manager.stop_all().await;
}

#[tokio::test]
async fn a_race_is_read_while_it_runs_and_when_it_has_finished() {
    let track = Track::new("read");
    // The project brings a lint of its own, so the entrants have a check.
    std::fs::write(track.repo.join("package.json"), json!({ "scripts": { "lint": "echo 'no problems'" } }).to_string())
        .unwrap();
    git(&track.repo, &["add", "."]);
    git(&track.repo, &["-c", "user.name=T", "-c", "user.email=t@example.com", "commit", "-qm", "Scripts"]);

    let started = track.start(&["red", "green", "blue"], EDIT).await;
    let race_id = started["race"]["id"].as_str().unwrap().to_string();
    let tasks = ids(&started["race"]["tasks"]);
    assert_eq!(tasks.len(), 3, "three agents, three tasks");
    for id in &tasks {
        track.until_state(id, TaskState::WaitingPermission).await;
    }

    // Everyone waits for an answer: the race is not over, nobody has a time or a change yet.
    let running = track.get(&race_id).await;
    assert_eq!((running["over"].as_bool(), running["finishedAt"].is_null()), (Some(false), true));
    for id in &tasks {
        let one = entrant(&running, id);
        assert_eq!(one["state"], "waiting_permission");
        assert!(one["ms"].is_null() && one["endedAt"].is_null() && one["startedAt"].as_u64().is_some());
        assert_eq!((one["files"].as_array().unwrap().len(), one["added"].as_u64()), (0, Some(0)));
        assert_eq!(one["checks"], json!({ "done": 0, "total": 1, "failed": 0, "running": 0 }));
    }

    // Red and green make the change, blue is not allowed to.
    track.answer(&tasks[0], "allow-once").await;
    let half = track.get(&race_id).await;
    assert_eq!(half["over"], false, "two are still waiting");
    assert!(entrant(&half, &tasks[0])["ms"].as_u64().is_some(), "the one that finished has its time already");
    track.answer(&tasks[1], "allow-once").await;
    track.answer(&tasks[2], "reject-once").await;
    // Red's lint is run; the others' is not.
    track.manager.checks_run(&tasks[0], None).unwrap();
    let mut race = track.get(&race_id).await;
    for _ in 0..400 {
        if entrant(&race, &tasks[0])["checks"]["running"] == 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
        race = track.get(&race_id).await;
    }

    assert_eq!(race["over"], true);
    let red = entrant(&race, &tasks[0]);
    assert_eq!(
        (red["agentId"].as_str(), red["short"].as_str(), red["state"].as_str()),
        (Some("red"), Some("Re"), Some("idle"))
    );
    assert_eq!(red["said"], "Done. The limit now comes from config instead of a fixed 5. One file changed.");
    assert_eq!(red["files"].as_array().unwrap().len(), 1);
    assert_eq!((red["files"][0]["path"].as_str(), red["files"][0]["kind"].as_str()), (Some(DEMO), Some("modified")));
    assert_eq!((red["added"].as_u64(), red["removed"].as_u64()), (Some(1), Some(1)));
    assert_eq!(red["checks"], json!({ "done": 1, "total": 1, "failed": 0, "running": 0 }));
    assert_eq!(red["ms"].as_u64(), Some(red["endedAt"].as_u64().unwrap() - red["startedAt"].as_u64().unwrap()));
    assert!(red["problem"].is_null());
    let blue = entrant(&race, &tasks[2]);
    assert_eq!(blue["said"], "I left the file as it was, because the edit was not allowed.");
    assert_eq!(
        (blue["files"].as_array().unwrap().len(), blue["added"].as_u64(), blue["removed"].as_u64()),
        (0, Some(0), Some(0))
    );
    assert_eq!(blue["checks"]["done"], 0);
    // The race finished when its last entrant did.
    let last = race["entrants"].as_array().unwrap().iter().filter_map(|one| one["endedAt"].as_u64()).max();
    assert_eq!(race["finishedAt"].as_u64(), last);

    // The change is in the entrant's own copy only: the project and the other copies are as they were.
    let cwd = |id: &str| PathBuf::from(track.manager.task(id).unwrap().row.cwd);
    assert!(std::fs::read_to_string(cwd(&tasks[0]).join(DEMO)).unwrap().contains("limits.login"));
    assert!(std::fs::read_to_string(cwd(&tasks[2]).join(DEMO)).unwrap().contains("limit: 5,"));
    assert!(std::fs::read_to_string(track.repo.join(DEMO)).unwrap().contains("limit: 5,"));
    track.manager.stop_all().await;
}

#[tokio::test]
async fn what_cannot_race_is_refused_with_a_reason() {
    let track = Track::new("refused");
    let start = |agents: Value, text: &str| {
        track.ask("race.start", json!({ "projectId": track.project_id, "agentIds": agents, "text": text }))
    };
    assert!(refusal(start(json!(["red"]), ASK).await).contains("A race needs two to four agents, and 1 were picked"));
    let five = json!(["red", "green", "blue", "grey", "pink"]);
    assert!(refusal(start(five, ASK).await).contains("two to four agents, and 5 were picked"));
    assert!(refusal(start(json!(["red", "red"]), ASK).await).contains("Red agent is in the race twice"));
    assert!(refusal(start(json!(["red", "nobody"]), ASK).await).contains("Unknown agent: nobody"));
    let missing = refusal(start(json!(["red", "missing"]), ASK).await);
    assert!(missing.contains("Missing agent is not available: Install it first"), "{missing}");
    assert!(refusal(start(json!(["red", "green"]), "   ").await).contains("Describe the task first"));
    let nowhere = track.ask("race.start", json!({ "projectId": "no-such", "agentIds": ["red", "green"], "text": ASK }));
    assert!(refusal(nowhere.await).contains("No such project"));
    assert!(
        refusal(track.ask("race.start", json!({ "projectId": track.project_id, "text": ASK })).await)
            .contains("Missing parameter: agentIds")
    );

    assert!(refusal(track.ask("race.get", json!({ "raceId": "no-such-race" })).await).contains("No such race"));
    assert!(refusal(track.ask("race.pick", json!({ "raceId": "p:1", "taskId": "t" })).await).contains("No such race"));
    assert!(refusal(track.ask("race.discard", json!({ "raceId": "p:1" })).await).contains("No such race"));
    assert!(refusal(track.ask("race.again", json!({ "raceId": "p:1" })).await).contains("No such race"));

    // An ordinary task is in no race.
    let alone = track.manager.create(&track.project_id, "red", ASK).await.unwrap();
    assert!(alone.race.is_none());
    let error = refusal(track.ask("race.get", json!({ "taskId": alone.row.id })).await);
    assert!(error.contains("This task is not part of a race"), "{error}");

    // None of the refused races left a task, a copy or an agent behind.
    assert_eq!(track.manager.list().unwrap().len(), 1);
    assert_eq!((track.copies().len(), track.branches().len()), (1, 1));
    track.until_running(1).await;
    track.manager.stop_all().await;
}

#[tokio::test]
async fn a_folder_without_git_cannot_hold_a_race() {
    let sandbox = Sandbox::new("nogit");
    let folder = sandbox.project();
    std::fs::write(folder.join("README.md"), "# Shop\n").unwrap();
    let track = Track::in_folder(sandbox, folder, "nogit");

    let error = refusal(
        track
            .ask("race.start", json!({ "projectId": track.project_id, "agentIds": ["red", "green"], "text": ASK }))
            .await,
    );
    assert!(
        error.contains(
            "A race needs git: each agent works in its own copy of the code, and Shop is a folder without git"
        ),
        "{error}"
    );
    assert!(track.manager.list().unwrap().is_empty(), "no task was made");
    assert_eq!(track.agents_running(), 0, "and no agent was started");
    // One agent alone still works there, in the folder itself.
    let alone = track.manager.create(&track.project_id, "red", ASK).await.unwrap();
    assert_eq!(alone.row.branch, None);
    track.manager.stop_all().await;
}

#[tokio::test]
async fn an_agent_that_does_not_start_is_named_and_the_others_race_on() {
    let track = Track::new("nostart");
    let started = track.start(&["red", "broken", "green"], ASK).await;
    let tasks = ids(&started["race"]["tasks"]);
    assert_eq!(tasks.len(), 2, "the two that started are the race");
    let failed = started["failed"].as_array().unwrap();
    assert_eq!(failed.len(), 1);
    assert_eq!(
        (failed[0]["agentId"].as_str(), failed[0]["agentName"].as_str()),
        (Some("broken"), Some("Broken agent"))
    );
    assert!(failed[0]["message"].as_str().unwrap().contains("Broken agent did not start"), "{failed:?}");
    assert_eq!((track.copies().len(), track.branches().len()), (2, 2), "the one that failed left no copy behind");
    let race = track.get(started["race"]["id"].as_str().unwrap()).await;
    assert_eq!(race["agentIds"], json!(["red", "broken", "green"]), "the race remembers who was asked");
    assert_eq!(race["entrants"].as_array().unwrap().len(), 2);

    // With one agent left there is nothing to compare: its task is kept, as an ordinary one.
    let alone = track.start(&["broken", "blue"], ASK).await;
    assert!(alone["race"].is_null());
    assert_eq!(alone["failed"].as_array().unwrap().len(), 1);
    let kept = alone["tasks"].as_array().unwrap();
    assert_eq!((kept.len(), kept[0]["agentId"].as_str(), kept[0]["race"].is_null()), (1, Some("blue"), true));
    assert!(track.manager.task(kept[0]["id"].as_str().unwrap()).unwrap().race.is_none());
    assert_eq!(track.manager.list().unwrap().len(), 3);
    track.manager.stop_all().await;
}

#[tokio::test]
async fn picking_a_result_removes_every_other_entrant() {
    let track = Track::new("pick");
    let started = track.start(&["red", "green", "blue"], EDIT).await;
    let race_id = started["race"]["id"].as_str().unwrap().to_string();
    let tasks = ids(&started["race"]["tasks"]);
    // Red and green finish; blue is still waiting for an answer when the user picks.
    track.answer(&tasks[0], "allow-once").await;
    track.answer(&tasks[1], "allow-once").await;
    track.until_state(&tasks[2], TaskState::WaitingPermission).await;
    assert_eq!((track.copies().len(), track.branches().len(), track.agents_running()), (3, 3, 3));
    let losers: Vec<(String, String)> = tasks[1..]
        .iter()
        .map(|id| track.manager.task(id).unwrap().row)
        .map(|row| (row.cwd, row.branch.unwrap()))
        .collect();
    let winner = track.manager.task(&tasks[0]).unwrap().row;

    // What is not an entrant cannot be picked.
    let other = track.manager.create(&track.project_id, "grey", ASK).await.unwrap();
    let error = refusal(track.ask("race.pick", json!({ "raceId": race_id, "taskId": other.row.id })).await);
    assert!(error.contains("That task is not part of this race"), "{error}");

    let mut windows = track.manager.subscribe();
    let race = track.ask("race.pick", json!({ "raceId": race_id, "taskId": tasks[0] })).await.unwrap();
    assert_eq!(race["picked"], tasks[0].as_str());
    assert!(race["decidedAt"].as_u64().is_some() && race["discarded"] == false);
    let left: Vec<&str> =
        race["entrants"].as_array().unwrap().iter().map(|one| one["taskId"].as_str().unwrap()).collect();
    assert_eq!(left, [tasks[0].as_str()], "only the picked one is still there");

    // The others are archived, their copies and branches are gone, their agents are stopped.
    for (id, (cwd, branch)) in tasks[1..].iter().zip(&losers) {
        assert!(track.manager.task(id).is_err(), "a loser is off the lists");
        assert!(!Path::new(cwd).exists(), "its working copy is removed: {cwd}");
        assert!(!track.branches().contains(branch), "its branch is removed: {branch}");
    }
    assert!(
        git(&track.repo, &["for-each-ref", "refs/ae/checkpoints"])
            .lines()
            .all(|line| !tasks[1..].iter().any(|id| line.contains(id.as_str()))),
        "their checkpoints are gone"
    );
    // The picked one and the task that never raced.
    track.until_running(2).await;
    assert_eq!(track.copies().len(), 2);

    // The picked one stays as an ordinary task, with its change and its conversation.
    let kept = track.manager.task(&tasks[0]).unwrap();
    assert!(kept.race.is_none(), "it is no entrant any more");
    assert_eq!((kept.state, kept.row.cwd.as_str()), (TaskState::Idle, winner.cwd.as_str()));
    assert!(std::fs::read_to_string(Path::new(&winner.cwd).join(DEMO)).unwrap().contains("limits.login"));
    assert!(track.branches().contains(winner.branch.as_ref().unwrap()));
    track.manager.prompt(&tasks[0], ASK.into()).await.unwrap();
    track.until_state(&tasks[0], TaskState::Idle).await;

    // Every window learns that the race is decided and which tasks left.
    let (mut decided, mut archived) = (false, 0);
    while let Ok(message) = windows.try_recv() {
        decided |= message["event"] == "race.changed"
            && message["data"]["decided"] == true
            && message["data"]["picked"] == tasks[0].as_str();
        archived += usize::from(message["event"] == "task.archived");
    }
    assert!(decided && archived == 2, "decided: {decided}, archived: {archived}");

    // A decided race stays decided.
    let again = refusal(track.ask("race.pick", json!({ "raceId": race_id, "taskId": tasks[0] })).await);
    assert!(again.contains("This race is already decided"), "{again}");
    let discard = refusal(track.ask("race.discard", json!({ "raceId": race_id })).await);
    assert!(discard.contains("This race is already decided"), "{discard}");
    assert!(track.manager.task(&tasks[0]).is_ok(), "the picked task is not touched by the refused discard");
    // It is still found from its task, and says what was picked.
    assert_eq!(track.ask("race.get", json!({ "taskId": tasks[0] })).await.unwrap()["picked"], tasks[0].as_str());
    track.manager.stop_all().await;
}

#[tokio::test]
async fn discarding_a_race_leaves_nothing_behind() {
    let track = Track::new("discard");
    let started = track.start(&["red", "green"], EDIT).await;
    let race_id = started["race"]["id"].as_str().unwrap().to_string();
    let tasks = ids(&started["race"]["tasks"]);
    track.answer(&tasks[0], "allow-once").await;
    track.until_state(&tasks[1], TaskState::WaitingPermission).await;
    assert_eq!((track.copies().len(), track.branches().len(), track.agents_running()), (2, 2, 2));

    let race = track.ask("race.discard", json!({ "raceId": race_id })).await.unwrap();
    assert_eq!(
        (race["discarded"].as_bool(), race["picked"].is_null(), race["over"].as_bool()),
        (Some(true), true, Some(true))
    );
    assert_eq!(race["entrants"], json!([]));
    assert!(track.manager.list().unwrap().is_empty(), "every entrant is archived");
    assert_eq!((track.copies().len(), track.branches().len()), (0, 0), "no copy and no branch is left");
    assert_eq!(git(&track.repo, &["for-each-ref", "refs/ae/checkpoints"]), "", "nor a checkpoint");
    track.until_running(0).await;
    // The project itself is as it was.
    assert_eq!(git(&track.repo, &["status", "--porcelain"]), "");
    assert_eq!(git(&track.repo, &["rev-parse", "--abbrev-ref", "HEAD"]), "main");

    // With its tasks archived the project can be removed, and the race's record goes with it.
    let key = format!("project.{}.race.{}", track.project_id, race_id.split_once(':').unwrap().1);
    assert!(track.sandbox.store().setting(&key).unwrap().is_some(), "the record is kept with the project");
    track.manager.remove_project(&track.project_id).unwrap();
    assert!(track.sandbox.store().setting(&key).unwrap().is_none());
}

#[tokio::test]
async fn a_race_is_run_again_with_the_same_text_and_agents() {
    let track = Track::new("again");
    let first = track.start(&["red", "green"], ASK).await;
    let first_id = first["race"]["id"].as_str().unwrap().to_string();
    let first_tasks = ids(&first["race"]["tasks"]);
    for id in &first_tasks {
        track.until_state(id, TaskState::Idle).await;
    }

    let second = track.ask("race.again", json!({ "raceId": first_id })).await.unwrap();
    let second_id = second["race"]["id"].as_str().unwrap().to_string();
    let second_tasks = ids(&second["race"]["tasks"]);
    assert_ne!(second_id, first_id);
    assert!(second_tasks.iter().all(|id| !first_tasks.contains(id)), "a new race has tasks of its own");
    let agents: Vec<&str> =
        second["tasks"].as_array().unwrap().iter().map(|task| task["agentId"].as_str().unwrap()).collect();
    assert_eq!(agents, ["red", "green"]);
    assert_eq!(track.get(&second_id).await["text"], ASK);
    // The first race is as it was: its entrants are still there to pick from.
    assert_eq!(track.get(&first_id).await["entrants"].as_array().unwrap().len(), 2);
    assert_eq!(track.manager.task(&first_tasks[0]).unwrap().race.unwrap().id, first_id);
    assert_eq!(track.manager.list().unwrap().len(), 4);

    // A race that was thrown away can be run again too.
    track.ask("race.discard", json!({ "raceId": first_id })).await.unwrap();
    let third = track.ask("race.again", json!({ "raceId": first_id })).await.unwrap();
    assert_eq!(ids(&third["race"]["tasks"]).len(), 2);
    track.manager.stop_all().await;
}

#[tokio::test]
async fn one_entrant_reviews_the_change_of_another() {
    let track = Track::new("review");
    let started = track.start(&["red", "green", "blue"], EDIT).await;
    let race_id = started["race"]["id"].as_str().unwrap().to_string();
    let tasks = ids(&started["race"]["tasks"]);
    track.answer(&tasks[0], "allow-once").await;
    track.answer(&tasks[2], "reject-once").await;
    let review =
        |of: &str, by: &str| track.ask("race.review", json!({ "raceId": race_id, "taskId": of, "reviewerTaskId": by }));

    // What cannot be: a review of itself, by an agent that is busy, of a result that changed nothing.
    assert!(refusal(review(&tasks[0], &tasks[0]).await).contains("An agent cannot review its own result"));
    let busy = refusal(review(&tasks[0], &tasks[1]).await);
    assert!(busy.contains("Green agent is still working. Wait for it or stop its turn"), "{busy}");
    let nothing = refusal(review(&tasks[2], &tasks[0]).await);
    assert!(nothing.contains("There is nothing to review: this result changes no file"), "{nothing}");
    track.answer(&tasks[1], "allow-once").await;

    let before = track.get(&race_id).await;
    let red_events = track.manager.history(&tasks[0]).unwrap().len();
    review(&tasks[0], &tasks[1]).await.unwrap();
    track.until_state(&tasks[1], TaskState::Idle).await;

    // The reviewer got a message of its own with the task and the change as text.
    let history = track.manager.history(&tasks[1]).unwrap();
    let asked = history.iter().rev().find(|event| is(event, "user_message")).unwrap()["text"].as_str().unwrap();
    assert!(asked.starts_with("Another agent, Red agent, worked on the same task as you"), "{asked}");
    assert!(asked.contains(&format!("The task was:\n\n{EDIT}\n")), "{asked}");
    assert!(asked.contains(&format!("--- {DEMO} (modified, +1 -1)\n@@ line 3 @@\n")), "{asked}");
    assert!(asked.contains("\n-  limit: 5,\n+  limit: limits.login,\n"), "{asked}");
    assert!(asked.contains("\n   windowMs: 60 * 1000,\n"), "the lines around the change are there too: {asked}");
    let marks: Vec<&Value> = history.iter().filter(|event| is(event, "race_review")).collect();
    assert_eq!(marks.len(), 1);
    assert_eq!((marks[0]["of"].as_str(), marks[0]["ofName"].as_str()), (Some(tasks[0].as_str()), Some("Red agent")));
    // It answered, and changed nothing: the practice agent says so itself.
    assert!(answer_text(&history).ends_with("Nothing was changed."));

    // Nothing else changed: the reviewed entrant heard nothing, and the race reads as before.
    assert_eq!(track.manager.history(&tasks[0]).unwrap().len(), red_events);
    let after = track.get(&race_id).await;
    for id in &tasks {
        let (was, is_now) = (entrant(&before, id), entrant(&after, id));
        for field in ["said", "ms", "endedAt", "added", "removed", "files", "state"] {
            assert_eq!(was[field], is_now[field], "{field} of {id}");
        }
    }
    assert_eq!(
        entrant(&after, &tasks[1])["said"],
        "Done. The limit now comes from config instead of a fixed 5. One file changed.",
        "the reviewer's card still says what it did itself"
    );
    assert_eq!(after["finishedAt"], before["finishedAt"]);
    assert!(after["picked"].is_null());
    track.manager.stop_all().await;
}

#[tokio::test]
async fn an_entrant_archived_or_cleaned_up_by_hand_does_not_break_the_race() {
    let track = Track::new("byhand");
    let started = track.start(&["red", "green", "blue", "grey"], EDIT).await;
    let race_id = started["race"]["id"].as_str().unwrap().to_string();
    let tasks = ids(&started["race"]["tasks"]);
    assert_eq!(tasks.len(), 4, "four agents is the most a race takes");
    for id in &tasks {
        track.answer(id, "allow-once").await;
    }
    let archived = track.manager.task(&tasks[1]).unwrap().row;

    // Green is archived by hand: its copy stays, as for any archived task. Blue is cleaned up by hand.
    track.ask("tasks.archive", json!({ "taskId": tasks[1] })).await.unwrap();
    track.ask("tasks.cleanUp", json!({ "taskId": tasks[2] })).await.unwrap();
    assert!(Path::new(&archived.cwd).exists());

    let race = track.get(&race_id).await;
    let left: Vec<&str> =
        race["entrants"].as_array().unwrap().iter().map(|one| one["taskId"].as_str().unwrap()).collect();
    assert_eq!(left, [tasks[0].as_str(), tasks[3].as_str()], "the two that are still there");
    assert_eq!(race["tasks"], json!(tasks), "the race remembers everyone it started with");
    assert_eq!(race["over"], true);
    assert_eq!(track.manager.task(&tasks[0]).unwrap().race.unwrap().tasks, tasks);

    // A result that is gone cannot be picked or reviewed.
    let gone = refusal(track.ask("race.pick", json!({ "raceId": race_id, "taskId": tasks[1] })).await);
    assert!(gone.contains("That result is gone: its task was archived"), "{gone}");
    let review = track.ask("race.review", json!({ "raceId": race_id, "taskId": tasks[1], "reviewerTaskId": tasks[0] }));
    assert!(refusal(review.await).contains("That result is gone"));

    // Picking one of the rest ends the race, and takes the archived entrant's leftovers with it.
    track.ask("race.pick", json!({ "raceId": race_id, "taskId": tasks[3] })).await.unwrap();
    assert!(!Path::new(&archived.cwd).exists(), "the copy of the entrant archived by hand is removed too");
    assert_eq!((track.copies().len(), track.branches().len()), (1, 1), "only the picked one is left");
    assert_eq!(track.manager.list().unwrap().len(), 1);
    track.until_running(1).await;
    track.manager.stop_all().await;
}
