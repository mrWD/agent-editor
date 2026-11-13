//! Review comments, merging by itself, and lines two tasks both change.
//!
//! GitHub is stood in for by a shell script with the name of its tool. No real account is touched.
#![cfg(unix)]

mod common;

use ae_core::hosts::Hosts;
use ae_core::server::{AppState, dispatch};
use common::*;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};

const FILE: &str = "src/middleware/rateLimit.ts";

fn pull(checks: &str) -> String {
    format!(
        r#"{{"number":128,"title":"TITLE","url":"https://github.com/x/shop/pull/128","state":"OPEN","isDraft":false,
 "mergeable":"MERGEABLE","reviewDecision":"","statusCheckRollup":[{checks}]}}"#
    )
}
const GREEN: &str = r#"{"__typename":"CheckRun","status":"COMPLETED","conclusion":"SUCCESS"}"#;
const RUNNING: &str = r#"{"__typename":"CheckRun","status":"IN_PROGRESS","conclusion":""}"#;
const RED: &str = r#"{"__typename":"CheckRun","status":"COMPLETED","conclusion":"FAILURE"}"#;

const THREADS: &str = r#"{"data":{"repository":{"pullRequest":{"reviewThreads":{"nodes":[
 {"isResolved":false,"comments":{"nodes":[{"databaseId":55,"author":{"login":"anna"},"path":"src/middleware/rateLimit.ts","line":3,
   "body":"Should the limit come from config, not a fixed 5?","url":"https://github.com/x/shop/pull/128#discussion_r55"}]}},
 {"isResolved":true,"comments":{"nodes":[{"databaseId":56,"author":{"login":"anna"},"path":"README.md","line":1,"body":"Settled.","url":""}]}}
]}}}}}"#;

/// A stand-in for `gh` that also answers what is asked about review comments.
fn fake_gh(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    fake_tool(
        dir,
        "gh",
        r#"here="$(dirname "$0")"
echo "$@" >> "$here/calls.log"
case "$1 $2" in
  "auth status") exit 0 ;;
  "pr view")
    if [ -f "$here/pull.json" ]; then cat "$here/pull.json"; else echo "no pull requests found for branch \"$3\"" >&2; exit 1; fi ;;
  "pr create") cp "$here/pull-template.json" "$here/pull.json"; echo "https://github.com/x/shop/pull/128" ;;
  "pr merge") sed 's/"OPEN"/"MERGED"/' "$here/pull.json" > "$here/pull.tmp" && mv "$here/pull.tmp" "$here/pull.json" ;;
  "api graphql") cat "$here/threads.json" ;;
  "api --hostname") echo '{}' ;;
  *) echo "unexpected call: $*" >&2; exit 2 ;;
esac
"#,
    )
}

fn calls(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("calls.log")).unwrap_or_default()
}

/// A core over a project on "GitHub", with one task whose agent changed a line,
/// the change committed and a pull request opened for it.
async fn with_request(sandbox: &Sandbox, checks: &str) -> (AppState, PathBuf, String) {
    let tools = sandbox.0.join("tools");
    let missing = PathBuf::from("/nonexistent/ae-test/tool");
    let gh = fake_gh(&tools);
    std::fs::write(tools.join("pull-template.json"), pull(checks)).unwrap();
    std::fs::write(tools.join("threads.json"), THREADS).unwrap();
    let hosts = Hosts::with(gh, missing.clone(), missing, "http://127.0.0.1:9/graphql".into());
    let state = AppState::new(sandbox.manager_with(hosts));
    let manager = &state.manager;

    let repo = git_project(sandbox);
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    let bare = sandbox.0.join("remote.git");
    git(&repo, &["init", "-q", "--bare", &bare.to_string_lossy()]);
    git(&repo, &["remote", "add", "origin", "git@github.com:x/shop.git"]);
    git(&repo, &["config", &format!("url.{}.insteadOf", bare.to_string_lossy()), "git@github.com:x/shop.git"]);
    let project = manager.add_project(&repo.to_string_lossy(), None).unwrap();

    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock", "What does the limiter do?").await.unwrap();
    let id = task.row.id.clone();
    until(&mut events, &id, |e| state_is(e, "idle")).await;
    manager.prompt(&id, "Change the limit".into()).await.unwrap();
    let seen = until(&mut events, &id, |e| is(e, "permission_request")).await;
    let request = seen.last().unwrap()["requestId"].as_str().unwrap().to_string();
    manager.answer_permission(&id, &request, Some("allow-once".into())).unwrap();
    until(&mut events, &id, |e| state_is(e, "idle")).await;
    dispatch(
        &state,
        "git.commit",
        &json!({ "taskId": id, "message": "Take the limit from config", "picks": [{ "path": FILE }] }),
    )
    .await
    .unwrap();
    dispatch(&state, "mr.create", &json!({ "taskId": id, "title": "Take the limit from config", "body": "" }))
        .await
        .unwrap();
    (state, tools, id)
}

#[tokio::test]
async fn review_comments_are_listed_and_answered_in_their_thread() {
    let sandbox = Sandbox::new("comments");
    let (state, tools, id) = with_request(&sandbox, GREEN).await;

    let listed = dispatch(&state, "mr.comments", &json!({ "taskId": id })).await.unwrap();
    let comments = listed["comments"].as_array().unwrap();
    assert_eq!(comments.len(), 1, "the settled thread is left out");
    assert_eq!(
        comments[0],
        json!({ "id": "55", "author": "anna", "path": FILE, "line": 3,
            "body": "Should the limit come from config, not a fixed 5?",
            "url": "https://github.com/x/shop/pull/128#discussion_r55" })
    );
    let log = calls(&tools);
    assert!(log.contains("api graphql --hostname github.com -f query=query("), "{log}");
    assert!(log.contains("-F owner=x -F name=shop -F number=128"), "{log}");

    dispatch(&state, "mr.reply", &json!({ "taskId": id, "commentId": "55", "body": " It does now. " })).await.unwrap();
    let log = calls(&tools);
    assert!(
        log.contains(
            "api --hostname github.com -X POST repos/x/shop/pulls/128/comments/55/replies -f body=It does now."
        ),
        "{log}"
    );

    // An empty reply, or an id that is not one, never reaches the host.
    let before = calls(&tools).lines().count();
    assert!(dispatch(&state, "mr.reply", &json!({ "taskId": id, "commentId": "55", "body": "  " })).await.is_err());
    assert!(dispatch(&state, "mr.reply", &json!({ "taskId": id, "commentId": "55/../1", "body": "x" })).await.is_err());
    assert!(calls(&tools).lines().filter(|line| line.contains("replies")).count() == 1);
    assert!(calls(&tools).lines().count() >= before);
}

#[tokio::test]
async fn a_request_merges_by_itself_once_its_checks_are_green() {
    let sandbox = Sandbox::new("auto-merge");
    let (state, tools, id) = with_request(&sandbox, RUNNING).await;
    let manager = &state.manager;
    let info = dispatch(&state, "mr.get", &json!({ "taskId": id, "fresh": true })).await.unwrap();
    assert_eq!((info["ready"].as_bool(), info["auto"].clone()), (Some(false), Value::Null));

    dispatch(&state, "mr.auto", &json!({ "taskId": id, "on": true, "squash": false, "archive": true })).await.unwrap();
    let info = dispatch(&state, "mr.get", &json!({ "taskId": id })).await.unwrap();
    assert_eq!(info["auto"], json!({ "squash": false, "deleteBranch": true, "archive": true }));
    let told = manager.history(&id).unwrap();
    assert!(
        told.iter()
            .any(|e| is(e, "tracker") && e["text"] == "The pull request will merge by itself when everything is green")
    );

    // While the checks run, a round changes nothing.
    state.auto_merge_round().await;
    assert!(!calls(&tools).contains("pr merge"));
    assert_eq!(manager.list().unwrap().len(), 1);

    // The checks turn green: the next round merges it the way the user asked, and finishes the task.
    std::fs::write(tools.join("pull.json"), pull(GREEN)).unwrap();
    state.auto_merge_round().await;
    assert!(calls(&tools).contains("pr merge 128 -R x/shop --merge --delete-branch"), "{}", calls(&tools));
    assert!(manager.list().unwrap().is_empty(), "the task was archived, as asked");
    // It is not merged a second time.
    state.auto_merge_round().await;
    assert_eq!(calls(&tools).matches("pr merge").count(), 1);
}

#[tokio::test]
async fn failed_checks_stop_the_wait_and_the_task_is_told() {
    let sandbox = Sandbox::new("auto-stop");
    let (state, tools, id) = with_request(&sandbox, RUNNING).await;
    let manager = &state.manager;
    dispatch(&state, "mr.auto", &json!({ "taskId": id })).await.unwrap();

    std::fs::write(tools.join("pull.json"), pull(RED)).unwrap();
    state.auto_merge_round().await;
    assert!(!calls(&tools).contains("pr merge"));
    let info = dispatch(&state, "mr.get", &json!({ "taskId": id })).await.unwrap();
    assert_eq!(info["auto"], Value::Null);
    let last = manager.history(&id).unwrap().into_iter().rev().find(|e| is(e, "tracker")).unwrap();
    assert_eq!(last["text"], "The checks of the pull request failed, so it will not merge by itself");
    assert_eq!(last["failed"], true);

    // Turning it on and off by hand is told too, once each.
    dispatch(&state, "mr.auto", &json!({ "taskId": id, "on": true })).await.unwrap();
    dispatch(&state, "mr.auto", &json!({ "taskId": id, "on": false })).await.unwrap();
    dispatch(&state, "mr.auto", &json!({ "taskId": id, "on": false })).await.unwrap();
    let texts: Vec<String> = manager
        .history(&id)
        .unwrap()
        .iter()
        .filter(|e| is(e, "tracker"))
        .map(|e| e["text"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(texts.iter().filter(|text| text.contains("will not merge by itself any more")).count(), 1);
}

#[tokio::test]
async fn lines_that_two_open_tasks_both_change_are_found() {
    let sandbox = Sandbox::new("overlaps");
    let project_dir = git_project(&sandbox);
    let manager = sandbox.manager();
    let project = manager.add_project(&project_dir.to_string_lossy(), None).unwrap();
    let mut events = manager.subscribe();
    let first = manager.create(&project.id, "mock", "What does the limiter do?").await.unwrap();
    until(&mut events, &first.row.id, |e| state_is(e, "idle")).await;
    let second = manager.create(&project.id, "mock", "What does the limiter do, again?").await.unwrap();
    until(&mut events, &second.row.id, |e| state_is(e, "idle")).await;
    let (a, b) = (PathBuf::from(&first.row.cwd), PathBuf::from(&second.row.cwd));
    let original = std::fs::read_to_string(a.join(FILE)).unwrap();
    let overlaps = |id: &str| manager.overlaps(id).unwrap()["overlaps"].as_array().unwrap().clone();
    assert!(overlaps(&first.row.id).is_empty(), "nothing is changed yet");

    // Both tasks change the same line, each its own way.
    std::fs::write(a.join(FILE), original.replace("limit: 5,", "limit: limits.login,")).unwrap();
    std::fs::write(b.join(FILE), original.replace("limit: 5,", "limit: 10,")).unwrap();
    let found = overlaps(&first.row.id);
    assert_eq!(found.len(), 1);
    assert_eq!((found[0]["path"].as_str(), found[0]["line"].as_u64()), (Some(FILE), Some(3)));
    assert_eq!(found[0]["mine"], json!(["  limit: limits.login,"]));
    assert_eq!(found[0]["theirs"], json!(["  limit: 10,"]));
    assert_eq!(found[0]["before"], json!(["  limit: 5,"]));
    assert_eq!(found[0]["task"]["id"], second.row.id.as_str());
    assert_eq!(found[0]["task"]["title"], "What does the limiter do, again?");
    // The other task sees the same, from its side.
    assert_eq!(overlaps(&second.row.id)[0]["theirs"], json!(["  limit: limits.login,"]));

    // "Not a problem" is remembered for as long as the overlap stays the same.
    manager.dismiss_overlap(&first.row.id, found[0]["mark"].as_str().unwrap()).unwrap();
    assert!(overlaps(&first.row.id).is_empty());
    assert_eq!(overlaps(&second.row.id).len(), 1, "the other task was not asked");
    std::fs::write(b.join(FILE), original.replace("limit: 5,", "limit: 20,")).unwrap();
    assert_eq!(overlaps(&first.row.id).len(), 1, "it changed, so it is told again");

    // The very same change in both is no conflict; nor are changes with a line between them.
    std::fs::write(b.join(FILE), original.replace("limit: 5,", "limit: limits.login,")).unwrap();
    assert!(overlaps(&first.row.id).is_empty());
    std::fs::write(b.join(FILE), original.replace("export const", "// The limiter.\nexport const")).unwrap();
    assert!(overlaps(&first.row.id).is_empty(), "line 1 and line 3 have line 2 between them");
    // A line next to the changed one does conflict.
    std::fs::write(b.join(FILE), original.replace("windowMs: 60 * 1000,", "windowMs: 30 * 1000,")).unwrap();
    assert_eq!(overlaps(&first.row.id).len(), 1, "lines 2 and 3 touch");

    // A commit in one of them changes nothing about it: the working copies are what is compared.
    git(&b, &["-c", "user.name=T", "-c", "user.email=t@example.com", "commit", "-qam", "Half a minute"]);
    assert_eq!(overlaps(&first.row.id).len(), 1);
    // A task in a folder without git has nothing to compare.
    let plain = Sandbox::new("overlaps-plain");
    let manager = plain.manager();
    let project = manager.add_project(&plain.project().to_string_lossy(), None).unwrap();
    let mut events = manager.subscribe();
    let task = manager.create(&project.id, "mock", "What does the limiter do?").await.unwrap();
    until(&mut events, &task.row.id, |e| state_is(e, "idle")).await;
    assert!(manager.overlaps(&task.row.id).unwrap()["overlaps"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn a_request_merged_on_the_host_is_cleaned_up_only_when_asked_and_safe() {
    use ae_core::automations::{Then, When};
    let sandbox = Sandbox::new("merged-elsewhere");
    let (state, tools, id) = with_request(&sandbox, GREEN).await;
    let manager = &state.manager;
    let task = manager.task(&id).unwrap();
    let cwd = PathBuf::from(&task.row.cwd);
    // Somebody merges the request on GitHub.
    std::fs::write(tools.join("pull.json"), pull(GREEN).replace("\"OPEN\"", "\"MERGED\"")).unwrap();

    // The automation is off: nothing is asked and nothing is touched.
    assert!(manager.merged_elsewhere().is_empty());
    assert!(cwd.is_dir());

    // On, but the working copy holds work that is nowhere else: it holds back and says so.
    manager.automation_turn("clean-up", true).unwrap();
    std::fs::write(cwd.join("NOTES.md"), "not committed\n").unwrap();
    assert!(manager.merged_elsewhere().is_empty());
    let held = manager.automations().remove(0);
    assert!(held.last_note.unwrap().contains("1 files are not committed"));
    let told = manager.history(&id).unwrap().into_iter().rev().find(|e| is(e, "tracker")).unwrap();
    assert_eq!(told["text"], "Not cleaned up by itself: 1 files are not committed");
    // One merge is handled once: it does not nag on every look.
    assert!(manager.merged_elsewhere().is_empty());
    assert_eq!(manager.history(&id).unwrap().iter().filter(|e| is(e, "tracker") && e["failed"] == true).count(), 1);

    // An automation of the user's own for the same moment runs the checks.
    manager.check_add(&id, "Always fine", "true").unwrap();
    manager
        .automation_save(None, "Check on merge", When::Merged, Then::RunChecks { which: "all".into() }, vec![])
        .unwrap();
    assert!(manager.on_request_event(&id, true).is_empty(), "the copy is still not clean");
    for _ in 0..200 {
        if manager.checks_state(&id).unwrap()["done"] == 1 {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert_eq!(manager.checks_state(&id).unwrap()["done"], 1);

    // With nothing to lose the task is handed back for cleaning up.
    std::fs::remove_file(cwd.join("NOTES.md")).unwrap();
    assert_eq!(manager.on_request_event(&id, true), vec![id.clone()]);
    let done = manager.automations().remove(0);
    assert!(done.last_note.unwrap().starts_with("Cleaned up “"));
    // It ran three times today: held back twice, then done.
    assert_eq!(done.runs_that_day, 3);
}
