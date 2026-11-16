//! End-to-end tests of issues and merge requests.
//!
//! GitHub and GitLab are stood in for by shell scripts with the names of their
//! tools, Linear by a small server on this machine. No real account is touched.
#![cfg(unix)]

mod common;

use ae_core::hosts::Hosts;
use ae_core::server::{AppState, dispatch};
use common::*;
use serde_json::{Value, json};
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

const ISSUES: &str = r##"[
 {"number":12,"title":"Login allows unlimited attempts","url":"https://github.com/x/shop/issues/12","state":"OPEN",
  "body":"Five a minute is enough.","labels":[{"name":"auth"}],"assignees":[],"updatedAt":"2026-11-12T19:05:30Z"},
 {"number":13,"title":"Add pagination to /orders","url":"https://github.com/x/shop/issues/13","state":"OPEN",
  "body":"","labels":[],"assignees":[{"login":"ann"}],"updatedAt":"2026-11-11T10:00:00Z"}]"##;

const PULL: &str = r#"{"number":128,"title":"TITLE","url":"https://github.com/x/shop/pull/128","state":"OPEN","isDraft":false,
 "mergeable":"MERGEABLE","reviewDecision":"","statusCheckRollup":[{"__typename":"CheckRun","status":"COMPLETED","conclusion":"SUCCESS"}]}"#;

/// A stand-in for `gh`. It writes every call to `calls.log` next to itself and keeps
/// the pull request, once "created", in `pull.json`.
fn fake_gh(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    std::fs::write(dir.join("issues.json"), ISSUES).unwrap();
    std::fs::write(dir.join("pull-template.json"), PULL).unwrap();
    fake_tool(
        dir,
        "gh",
        r#"here="$(dirname "$0")"
echo "$@" >> "$here/calls.log"
case "$1 $2" in
  "auth status") exit 0 ;;
  "issue list") cat "$here/issues.json" ;;
  "pr view")
    if [ -f "$here/pull.json" ]; then cat "$here/pull.json"; else echo "no pull requests found for branch \"$3\"" >&2; exit 1; fi ;;
  "pr create") cp "$here/pull-template.json" "$here/pull.json"; echo "https://github.com/x/shop/pull/128" ;;
  "pr merge") sed 's/"OPEN"/"MERGED"/' "$here/pull.json" > "$here/pull.tmp" && mv "$here/pull.tmp" "$here/pull.json" ;;
  *) echo "unexpected call: $*" >&2; exit 2 ;;
esac
"#,
    )
}

fn calls(dir: &Path) -> String {
    std::fs::read_to_string(dir.join("calls.log")).unwrap_or_default()
}

/// A project whose remote reads as GitHub but whose pushes land in a folder next to it.
fn github_project(sandbox: &Sandbox) -> (PathBuf, PathBuf) {
    let repo = git_project(sandbox);
    git(&repo, &["config", "user.name", "Test"]);
    git(&repo, &["config", "user.email", "test@example.com"]);
    let bare = sandbox.0.join("remote.git");
    git(&repo, &["init", "-q", "--bare", &bare.to_string_lossy()]);
    git(&repo, &["remote", "add", "origin", "git@github.com:x/shop.git"]);
    git(&repo, &["config", &format!("url.{}.insteadOf", bare.to_string_lossy()), "git@github.com:x/shop.git"]);
    (repo, bare)
}

fn hosts_with_gh(dir: &Path) -> Hosts {
    let missing = PathBuf::from("/nonexistent/ae-test/tool");
    Hosts::with(fake_gh(dir), missing.clone(), missing, "http://127.0.0.1:9/graphql".into())
}

#[tokio::test]
async fn issues_are_listed_from_the_code_host_and_a_task_is_started_from_one() {
    let sandbox = Sandbox::new("issues");
    let tools = sandbox.0.join("tools");
    let state = AppState::new(sandbox.manager_with(hosts_with_gh(&tools)));
    let manager = &state.manager;
    let (repo, _) = github_project(&sandbox);
    let project = manager.add_project(&repo.to_string_lossy(), None).unwrap();

    let status = dispatch(&state, "integrations.status", &json!({})).await.unwrap();
    assert_eq!(status["github"], json!({ "installed": true, "signedIn": true, "hint": null }));
    assert_eq!(status["gitlab"]["installed"], false);
    assert!(status["gitlab"]["hint"].as_str().unwrap().contains("glab auth login"));
    assert_eq!(status["linear"]["hasKey"], false);

    let listed = dispatch(&state, "issues.list", &json!({})).await.unwrap();
    let mine = &listed["projects"][0];
    assert_eq!((mine["source"].as_str(), mine["label"].as_str()), (Some("github"), Some("GitHub x/shop")));
    assert_eq!(mine["problem"], Value::Null);
    let keys: Vec<_> =
        mine["issues"].as_array().unwrap().iter().map(|i| (i["key"].as_str().unwrap(), i["taskId"].clone())).collect();
    assert_eq!(keys, vec![("#12", Value::Null), ("#13", Value::Null)]);
    assert!(calls(&tools).contains("issue list -R x/shop --state open"));

    // Starting from an issue: the task carries the issue, and the agent is told what it says.
    let mut events = manager.subscribe();
    let task = dispatch(&state, "issues.start", &json!({ "projectId": project.id, "key": "#12", "agentId": "mock" }))
        .await
        .unwrap();
    let id = task["id"].as_str().unwrap().to_string();
    assert_eq!(task["title"], "Login allows unlimited attempts");
    assert_eq!(
        task["issue"],
        json!({ "key": "#12", "id": "12", "url": "https://github.com/x/shop/issues/12",
        "title": "Login allows unlimited attempts", "source": "github" })
    );
    let seen = until(&mut events, &id, |e| state_is(e, "idle")).await;
    let said = seen.iter().find(|e| is(e, "user_message")).unwrap()["text"].as_str().unwrap().to_string();
    assert!(said.contains("Five a minute is enough.") && said.contains("This is issue #12"), "{said}");

    // The issue now shows its task, and cannot be started twice.
    let again = dispatch(&state, "issues.list", &json!({})).await.unwrap();
    assert_eq!(again["projects"][0]["issues"][0]["taskId"], id.as_str());
    assert_eq!(again["projects"][0]["issues"][1]["taskId"], Value::Null);
    let twice =
        dispatch(&state, "issues.start", &json!({ "projectId": project.id, "key": "#12", "agentId": "mock" })).await;
    assert!(twice.unwrap_err().to_string().contains("already open"));
    let unknown =
        dispatch(&state, "issues.start", &json!({ "projectId": project.id, "key": "#99", "agentId": "mock" })).await;
    assert!(unknown.unwrap_err().to_string().contains("no open issue #99"));
}

#[tokio::test]
async fn a_project_without_a_known_host_says_so_and_a_missing_tool_explains_itself() {
    let sandbox = Sandbox::new("nohost");
    let state = AppState::new(sandbox.manager());
    let manager = &state.manager;
    let repo = git_project(&sandbox);
    manager.add_project(&repo.to_string_lossy(), None).unwrap();
    let listed = dispatch(&state, "issues.list", &json!({})).await.unwrap();
    assert_eq!(listed["projects"][0]["source"], "none");
    assert_eq!(listed["projects"][0]["issues"], json!([]));

    // A GitHub remote, but no `gh` on the machine: the reason is given in plain words.
    git(&repo, &["remote", "add", "origin", "https://github.com/x/shop.git"]);
    let listed = dispatch(&state, "issues.list", &json!({})).await.unwrap();
    let problem = listed["projects"][0]["problem"].as_str().unwrap();
    assert!(problem.contains("`gh` command was not found"), "{problem}");
}

#[tokio::test]
async fn a_merge_request_is_opened_merged_and_the_task_cleaned_up() {
    let sandbox = Sandbox::new("merge");
    let tools = sandbox.0.join("tools");
    let state = AppState::new(sandbox.manager_with(hosts_with_gh(&tools)));
    let manager = &state.manager;
    let (repo, bare) = github_project(&sandbox);
    let project = manager.add_project(&repo.to_string_lossy(), None).unwrap();

    // A task started from issue #12, with one allowed edit, committed.
    dispatch(&state, "issues.list", &json!({})).await.unwrap();
    let mut events = manager.subscribe();
    let mut everything = manager.subscribe();
    let task = manager.start_issue(&project.id, "#12", "mock", None).await.unwrap();
    let id = task.row.id.clone();
    until(&mut events, &id, |e| state_is(e, "idle")).await;
    manager.prompt(&id, "Change the limit".into()).await.unwrap();
    let seen = until(&mut events, &id, |e| is(e, "permission_request")).await;
    let request = seen.last().unwrap()["requestId"].as_str().unwrap().to_string();
    manager.answer_permission(&id, &request, Some("allow-once".into())).unwrap();
    until(&mut events, &id, |e| state_is(e, "idle")).await;
    let demo = "src/middleware/rateLimit.ts";
    dispatch(
        &state,
        "git.commit",
        &json!({ "taskId": id, "message": "Take the login limit from config", "picks": [{ "path": demo }] }),
    )
    .await
    .unwrap();

    // Before: no request, and nothing is asked of the host for a branch that was never pushed.
    let before = dispatch(&state, "mr.get", &json!({ "taskId": id })).await.unwrap();
    assert_eq!(before["request"], Value::Null);
    assert_eq!(before["host"]["word"], "pull request");
    assert_eq!(
        before["defaults"],
        json!({ "title": "Take the login limit from config", "body": "Closes #12.\n", "base": "main" })
    );
    assert_eq!(dispatch(&state, "mr.overview", &json!({})).await.unwrap()["requests"], json!([]));

    // Opening it pushes the branch first.
    let opened = dispatch(
        &state,
        "mr.create",
        &json!({ "taskId": id, "title": "Take the login limit from config", "body": "Closes #12.\n" }),
    )
    .await
    .unwrap();
    assert_eq!((opened["request"]["number"].as_u64(), opened["request"]["state"].as_str()), (Some(128), Some("open")));
    let branch = task.row.branch.clone().unwrap();
    assert_eq!(git(&bare, &["log", "--format=%s", "-1", &branch]), "Take the login limit from config");
    let log = calls(&tools);
    assert!(
        log.contains(&format!(
            "pr create -R x/shop --head {branch} --base main --title Take the login limit from config"
        )),
        "{log}"
    );
    assert!(dispatch(&state, "mr.create", &json!({ "taskId": id, "title": " " })).await.is_err(), "a title is needed");

    let overview = dispatch(&state, "mr.overview", &json!({})).await.unwrap();
    assert_eq!(overview["requests"][0]["taskId"], id.as_str());
    assert_eq!(overview["requests"][0]["ready"], true);

    // Merging: squash, delete the branch on the host, remove the working copy, archive the task.
    let cwd = PathBuf::from(&task.row.cwd);
    assert!(cwd.is_dir());
    let merged = dispatch(&state, "mr.merge", &json!({ "taskId": id })).await.unwrap();
    assert_eq!(merged["request"]["state"], "merged");
    assert!(calls(&tools).contains("pr merge 128 -R x/shop --squash --delete-branch"));
    assert!(!cwd.exists(), "the working copy is removed");
    assert!(!git(&repo, &["branch", "--list"]).contains(&branch), "the local branch is removed");
    assert!(manager.list().unwrap().is_empty(), "the task has left the lists");

    // What happened was said in the task's conversation, in order.
    let mut actions = Vec::new();
    while let Ok(message) = everything.try_recv() {
        let event = &message["data"];
        if message["event"] == "task.event" && is(event, "git") {
            actions.push(format!("{} {}", event["action"].as_str().unwrap(), event["subject"].as_str().unwrap()));
        }
    }
    assert_eq!(
        actions,
        vec!["committed Take the login limit from config", "opened pull request 128", "merged pull request 128"]
    );
}

/// A stand-in for Linear's API: answers every request on one port and keeps what it was asked.
struct FakeLinear {
    url: String,
    asked: Arc<Mutex<Vec<(String, String)>>>,
}

fn fake_linear() -> FakeLinear {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = format!("http://{}/graphql", listener.local_addr().unwrap());
    let asked = Arc::new(Mutex::new(Vec::new()));
    let seen = asked.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut raw = Vec::new();
            let mut buffer = [0u8; 4096];
            // Read the head, then as much body as the head says there is.
            let (head_end, length) = loop {
                let n = stream.read(&mut buffer).unwrap_or(0);
                if n == 0 {
                    break (raw.len(), 0);
                }
                raw.extend_from_slice(&buffer[..n]);
                if let Some(at) = raw.windows(4).position(|w| w == b"\r\n\r\n") {
                    let head = String::from_utf8_lossy(&raw[..at]).to_lowercase();
                    let length = head
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    break (at + 4, length);
                }
            };
            while raw.len() < head_end + length {
                let n = stream.read(&mut buffer).unwrap_or(0);
                if n == 0 {
                    break;
                }
                raw.extend_from_slice(&buffer[..n]);
            }
            let head = String::from_utf8_lossy(&raw[..head_end]).to_string();
            let body = String::from_utf8_lossy(&raw[head_end..]).to_string();
            let key = head
                .lines()
                .find_map(|line| {
                    line.to_lowercase().starts_with("authorization:").then(|| line[14..].trim().to_string())
                })
                .unwrap_or_default();
            seen.lock().unwrap().push((key.clone(), body.clone()));

            let (status, answer) = if key != "lin_test_key" && key != "lin_read_only" {
                ("401 Unauthorized", json!({ "errors": [{ "message": "Authentication required" }] }))
            } else if body.contains("issueUpdate") && key == "lin_read_only" {
                // What Linear says to a key that was made without write access.
                (
                    "200 OK",
                    json!({ "errors": [{ "message": "Invalid scope: `write` or `issues:create` required" }], "data": null }),
                )
            } else if body.contains("issue(id") && body.contains("APP-215") {
                // An issue somebody works on already: its requests are attached, one of them in another repository.
                (
                    "200 OK",
                    json!({ "data": { "issue": {
                    "id": "uuid-215", "identifier": "APP-215", "title": "Orders need pagination",
                    "url": "https://linear.app/x/issue/APP-215", "description": "Fifty a page.",
                    "updatedAt": "2026-11-12T19:05:30.000Z", "state": { "name": "In Progress", "type": "started" },
                    "assignee": null, "labels": { "nodes": [] },
                    "attachments": { "nodes": [
                        { "title": "Deployment Link", "url": "https://128.preview.example.com" },
                        { "title": "feat(orders): pages", "url": "https://github.com/x/shop/pull/128" },
                        { "title": "the same, seen in its changes", "url": "https://github.com/X/Shop/pull/128/files" },
                        { "title": "fix(web): pages", "url": "https://github.com/x/web/pull/7" },
                        { "title": "an older try", "url": "https://github.com/x/shop/pull/129" }
                    ] }
                } } }),
                )
            } else if body.contains("issue(id") && body.contains("APP-214") {
                // The issue by itself comes with what is attached to it; the lists do not.
                (
                    "200 OK",
                    json!({ "data": { "issue": {
                    "id": "uuid-214", "identifier": "APP-214", "title": "Login allows unlimited attempts",
                    "url": "https://linear.app/x/issue/APP-214", "description": "Limit it to five a minute.",
                    "updatedAt": "2026-11-12T19:05:30.000Z", "state": { "name": "Todo", "type": "unstarted" },
                    "assignee": null, "labels": { "nodes": [] },
                    "attachments": { "nodes": [
                        { "title": "Error: too many logins", "url": "https://sentry.io/organizations/x/issues/7/" }
                    ] }
                } } }),
                )
            } else if body.contains("workflowStates") {
                (
                    "200 OK",
                    json!({ "data": { "workflowStates": { "nodes": [
                    { "id": "s-review", "name": "In Review", "type": "started", "position": 3.0 },
                    { "id": "s-progress", "name": "In Progress", "type": "started", "position": 2.0 },
                    { "id": "s-done", "name": "Done", "type": "completed", "position": 4.0 }
                ] } } }),
                )
            } else if body.contains("issueUpdate") {
                ("200 OK", json!({ "data": { "issueUpdate": { "success": true } } }))
            } else if body.contains("organization") {
                (
                    "200 OK",
                    json!({ "data": { "viewer": { "name": "Viktor", "email": "v@example.com" },
                        "organization": { "name": "Shop Inc" },
                        "teams": { "nodes": [{ "key": "APP", "name": "App" }, { "key": "OPS", "name": "Operations" }] } } }),
                )
            } else if body.contains("assignedIssues") {
                (
                    "200 OK",
                    json!({ "data": { "viewer": { "assignedIssues": { "nodes": [{
                    "id": "uuid-12590", "identifier": "OPS-12590", "title": "Skills with empty names",
                    "url": "https://linear.app/x/issue/OPS-12590", "description": "Hide them from the filter.",
                    "updatedAt": "2026-11-13T09:00:00.000Z", "state": { "name": "Todo", "type": "unstarted" },
                    "assignee": { "name": "Viktor" }, "labels": { "nodes": [] }
                }] } } } }),
                )
            } else if body.contains("issue(id") {
                (
                    "200 OK",
                    json!({ "data": { "issue": {
                    "id": "uuid-12590", "identifier": "OPS-12590", "title": "Skills with empty names",
                    "url": "https://linear.app/x/issue/OPS-12590", "description": "Hide them from the filter.",
                    "updatedAt": "2026-11-13T09:00:00.000Z", "state": { "name": "Todo", "type": "unstarted" },
                    "assignee": { "name": "Viktor" }, "labels": { "nodes": [] }
                } } }),
                )
            } else {
                (
                    "200 OK",
                    json!({ "data": { "issues": { "nodes": [{
                    "id": "uuid-214", "identifier": "APP-214", "title": "Login allows unlimited attempts",
                    "url": "https://linear.app/x/issue/APP-214", "description": "Limit it to five a minute.",
                    "updatedAt": "2026-11-12T19:05:30.000Z", "state": { "name": "Todo", "type": "unstarted" },
                    "assignee": null, "labels": { "nodes": [] }
                }] } } }),
                )
            };
            let text = answer.to_string();
            let _ = write!(
                stream,
                "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{text}",
                text.len()
            );
        }
    });
    FakeLinear { url, asked }
}

#[tokio::test]
async fn linear_issues_are_listed_and_moved_when_a_task_starts() {
    let sandbox = Sandbox::new("linear");
    let linear = fake_linear();
    let missing = PathBuf::from("/nonexistent/ae-test/tool");
    let hosts = Hosts::with(missing.clone(), missing.clone(), missing, linear.url.clone());
    let state = AppState::new(sandbox.manager_with(hosts));
    let manager = &state.manager;
    let project = manager.add_project(&git_project(&sandbox).to_string_lossy(), None).unwrap();

    // A team is set but no key yet: the screen is told what is missing.
    dispatch(&state, "integrations.setLinearTeam", &json!({ "projectId": project.id, "team": " app " })).await.unwrap();
    let listed = dispatch(&state, "issues.list", &json!({})).await.unwrap();
    assert_eq!(
        (listed["projects"][0]["source"].as_str(), listed["projects"][0]["label"].as_str()),
        (Some("linear"), Some("Linear team APP"))
    );
    assert!(listed["projects"][0]["problem"].as_str().unwrap().contains("not connected"));

    // A wrong key is refused by Linear, and that is said as it is.
    dispatch(&state, "integrations.setLinearKey", &json!({ "key": "wrong" })).await.unwrap();
    let listed = dispatch(&state, "issues.list", &json!({})).await.unwrap();
    assert!(listed["projects"][0]["problem"].as_str().unwrap().contains("did not accept the key"));

    dispatch(&state, "integrations.setLinearKey", &json!({ "key": " lin_test_key " })).await.unwrap();
    assert_eq!(dispatch(&state, "integrations.status", &json!({})).await.unwrap()["linear"]["hasKey"], true);
    let listed = dispatch(&state, "issues.list", &json!({})).await.unwrap();
    assert_eq!(listed["projects"][0]["issues"][0]["key"], "APP-214");
    assert_eq!(listed["projects"][0]["issues"][0]["state"], "Todo");

    // Starting the task moves the issue to the team's first "started" state.
    let mut events = manager.subscribe();
    let task =
        dispatch(&state, "issues.start", &json!({ "projectId": project.id, "key": "APP-214", "agentId": "mock" }))
            .await
            .unwrap();
    let id = task["id"].as_str().unwrap().to_string();
    assert_eq!(task["issue"]["source"], "linear");
    let seen = until(&mut events, &id, |e| is(e, "tracker")).await;
    assert_eq!(seen.last().unwrap()["text"], "The issue is now “In Progress” in Linear.");
    // What is attached to the issue reached the agent with its text: it may be all there is to go on.
    let said = seen.iter().find(|e| is(e, "user_message")).expect("the first message")["text"].as_str().unwrap();
    assert!(said.contains("Limit it to five a minute."), "{said}");
    assert!(
        said.contains("Attached to the issue:\n- Error: too many logins: https://sentry.io/organizations/x/issues/7/"),
        "{said}"
    );
    let asked = linear.asked.lock().unwrap().clone();
    let update = asked.iter().find(|(_, body)| body.contains("issueUpdate")).expect("the issue was moved");
    assert!(update.1.contains("uuid-214") && update.1.contains("s-progress"), "{}", update.1);
    assert!(asked.iter().filter(|(key, _)| key == "lin_test_key").count() >= 3);

    // Forgetting the team goes back to the code host (here: none).
    dispatch(&state, "integrations.setLinearTeam", &json!({ "projectId": project.id, "team": null })).await.unwrap();
    assert_eq!(dispatch(&state, "issues.list", &json!({})).await.unwrap()["projects"][0]["source"], "none");
}

#[tokio::test]
async fn a_new_issue_with_the_label_starts_a_task_once() {
    use ae_core::automations::{Then, When};
    const MINUTE: u64 = 60 * 1000;
    let sandbox = Sandbox::new("issue-label");
    let tools = sandbox.0.join("tools");
    let manager = sandbox.manager_with(hosts_with_gh(&tools));
    let (repo, _) = github_project(&sandbox);
    manager.add_project(&repo.to_string_lossy(), None).unwrap();
    let id = manager
        .automation_save(
            None,
            "Bugs get a failing test",
            When::IssueLabeled { label: "Bug".into() },
            Then::StartTask { agent_id: "mock".into(), text: "Describe what is wrong first.".into() },
            vec![],
        )
        .unwrap();

    // Issues that are open when the automation is made are not news.
    manager.automations_tick(MINUTE).await;
    assert!(manager.list().unwrap().is_empty());
    assert!(calls(&tools).contains("issue list"));

    // A new one with the label comes in. The host is not asked again right away.
    let more = ISSUES.replace(
        "[\n",
        "[\n {\"number\":14,\"title\":\"Checkout fails on empty carts\",\"url\":\"https://github.com/x/shop/issues/14\",\"state\":\"OPEN\",\n  \"body\":\"It answers 500.\",\"labels\":[{\"name\":\"bug\"}],\"assignees\":[],\"updatedAt\":\"2026-11-13T08:00:00Z\"},\n",
    );
    assert!(more.contains("\"number\":14"), "the new issue is in the list");
    std::fs::write(tools.join("issues.json"), more).unwrap();
    manager.automations_tick(2 * MINUTE).await;
    assert!(manager.list().unwrap().is_empty(), "too soon to ask the host again");

    let mut events = manager.subscribe();
    manager.automations_tick(7 * MINUTE).await;
    let tasks = manager.list().unwrap();
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].row.title, "Checkout fails on empty carts");
    assert_eq!(tasks[0].row.issue.as_ref().unwrap().key, "#14");
    // First what the issue says, then what the automation is for.
    let task_id = tasks[0].row.id.clone();
    let seen =
        until(&mut events, &task_id, |e| is(e, "user_message") && e["text"] == "Describe what is wrong first.").await;
    assert!(seen.iter().any(|e| is(e, "user_message") && e["text"].as_str().unwrap().contains("It answers 500.")));
    until(&mut events, &task_id, |e| state_is(e, "idle")).await;
    let own = manager.automations().into_iter().find(|one| one.id == id).unwrap();
    assert_eq!(own.last_note.as_deref(), Some("Started a task for #14 in project"));

    // The same issue never starts a second task.
    manager.automations_tick(20 * MINUTE).await;
    assert_eq!(manager.list().unwrap().len(), 1);
}

#[tokio::test]
async fn a_company_gitlab_is_asked_about_itself_and_not_about_gitlab_com() {
    let sandbox = Sandbox::new("gitlab-own-server");
    let tools = sandbox.0.join("tools");
    // Signed in to the company's server only. Like the real `glab`, a plain `auth status` fails because
    // gitlab.com is not signed in, and every call without GITLAB_HOST goes to gitlab.com, which does not
    // know the project.
    let glab = fake_tool(
        &tools,
        "glab",
        r#"here="$(dirname "$0")"
echo "GITLAB_HOST=$GITLAB_HOST $@" >> "$here/calls.log"
case "$1 $2" in
  "auth status")
    if [ "$3 $4" = "--hostname gitlab.company.com" ]; then
      echo "gitlab.company.com: Logged in to gitlab.company.com as viktor" >&2; exit 0
    fi
    echo "gitlab.company.com: Logged in to gitlab.company.com as viktor" >&2
    echo "gitlab.com: No token found" >&2; exit 1 ;;
  "issue list")
    if [ "$GITLAB_HOST" != "gitlab.company.com" ]; then echo "ERROR 404 Not Found." >&2; exit 1; fi
    echo '[{"iid":7,"title":"Skills with empty names","web_url":"https://gitlab.company.com/application-development/webapp/backend/-/issues/7","state":"opened","description":"","labels":[],"assignees":[],"updated_at":"2026-11-12T19:05:30Z"}]' ;;
  *) echo "unexpected call: $*" >&2; exit 2 ;;
esac
"#,
    );
    let missing = PathBuf::from("/nonexistent/ae-test/tool");
    let hosts = Hosts::with(missing.clone(), glab, missing, "http://127.0.0.1:9/graphql".into());
    let state = AppState::new(sandbox.manager_with(hosts));
    let manager = &state.manager;
    let repo = git_project(&sandbox);
    // A group inside a group inside a group, on the company's own server.
    git(&repo, &["remote", "add", "origin", "git@gitlab.company.com:application-development/webapp/backend.git"]);
    manager.add_project(&repo.to_string_lossy(), None).unwrap();

    let status = dispatch(&state, "integrations.status", &json!({})).await.unwrap();
    assert_eq!(status["gitlab"], json!({ "installed": true, "signedIn": true, "hint": null }));

    let listed = dispatch(&state, "issues.list", &json!({})).await.unwrap();
    let mine = &listed["projects"][0];
    assert_eq!(mine["problem"], Value::Null, "{mine}");
    assert_eq!(mine["label"], "GitLab application-development/webapp/backend");
    assert_eq!(mine["issues"][0]["key"], "#7");
    assert!(
        calls(&tools).contains("GITLAB_HOST=gitlab.company.com issue list -R application-development/webapp/backend")
    );
}

#[tokio::test]
async fn a_server_that_is_not_signed_in_is_named_with_the_command_for_it() {
    let sandbox = Sandbox::new("gitlab-signed-out");
    let tools = sandbox.0.join("tools");
    let glab = fake_tool(&tools, "glab", "echo \"$3 $4: not logged in\" >&2; exit 1\n");
    let missing = PathBuf::from("/nonexistent/ae-test/tool");
    let hosts = Hosts::with(missing.clone(), glab, missing, "http://127.0.0.1:9/graphql".into());
    let state = AppState::new(sandbox.manager_with(hosts));
    let repo = git_project(&sandbox);
    git(&repo, &["remote", "add", "origin", "https://gitlab.company.com/group/shop.git"]);
    state.manager.add_project(&repo.to_string_lossy(), None).unwrap();
    let status = dispatch(&state, "integrations.status", &json!({})).await.unwrap();
    assert_eq!(status["gitlab"]["signedIn"], false);
    assert_eq!(
        status["gitlab"]["hint"],
        "Run `glab auth login --hostname gitlab.company.com` in a terminal to sign in to gitlab.company.com."
    );
}

#[tokio::test]
async fn issues_assigned_to_the_user_in_linear_show_without_a_team_and_start_in_any_project() {
    let sandbox = Sandbox::new("linear-mine");
    let linear = fake_linear();
    let missing = PathBuf::from("/nonexistent/ae-test/tool");
    let hosts = Hosts::with(missing.clone(), missing.clone(), missing, linear.url.clone());
    let state = AppState::new(sandbox.manager_with(hosts));
    let manager = &state.manager;
    // A project on a code host the core cannot reach, and no Linear team set for it.
    let project = manager.add_project(&git_project(&sandbox).to_string_lossy(), None).unwrap();

    // Without a key nothing is asked of Linear.
    let listed = dispatch(&state, "issues.list", &json!({})).await.unwrap();
    assert_eq!(listed["linear"], Value::Null);

    dispatch(&state, "integrations.setLinearKey", &json!({ "key": "lin_test_key" })).await.unwrap();
    // Who the key is, and the teams to choose from.
    let account = dispatch(&state, "integrations.linearAccount", &json!({})).await.unwrap();
    assert_eq!((account["name"].as_str(), account["workspace"].as_str()), (Some("Viktor"), Some("Shop Inc")));
    assert_eq!(account["teams"], json!([{ "key": "APP", "name": "App" }, { "key": "OPS", "name": "Operations" }]));

    // The user's own issues are there at once, though no project uses Linear.
    let listed = dispatch(&state, "issues.list", &json!({})).await.unwrap();
    assert_eq!(listed["linear"]["problem"], Value::Null);
    let mine = &listed["linear"]["issues"][0];
    assert_eq!((mine["key"].as_str(), mine["taskId"].clone()), (Some("OPS-12590"), Value::Null));

    // One of them becomes a task in the project; the issue moves along in its own team.
    let mut events = manager.subscribe();
    let task = dispatch(
        &state,
        "issues.start",
        &json!({ "projectId": project.id, "key": "OPS-12590", "agentId": "mock", "source": "linear" }),
    )
    .await
    .unwrap();
    let id = task["id"].as_str().unwrap().to_string();
    assert_eq!(task["issue"]["source"], "linear");
    assert_eq!(task["title"], "Skills with empty names");
    let seen = until(&mut events, &id, |e| is(e, "tracker")).await;
    assert_eq!(seen.last().unwrap()["text"], "The issue is now “In Progress” in Linear.");
    assert!(
        linear.asked.lock().unwrap().iter().any(|(_, body)| body.contains("workflowStates") && body.contains("OPS"))
    );

    // Listed again, the issue names its task; a second task for it is refused.
    let listed = dispatch(&state, "issues.list", &json!({})).await.unwrap();
    assert_eq!(listed["linear"]["issues"][0]["taskId"], id.as_str());
    assert_eq!(listed["linear"]["issues"][0]["projectId"], project.id.as_str());
    let again = dispatch(
        &state,
        "issues.start",
        &json!({ "projectId": project.id, "key": "OPS-12590", "agentId": "mock", "source": "linear" }),
    )
    .await
    .unwrap_err()
    .to_string();
    assert!(again.contains("already open"), "{again}");
}

#[tokio::test]
async fn an_agent_sets_the_editor_up_from_what_stands_and_what_the_user_said() {
    let sandbox = Sandbox::new("setup");
    let tools = sandbox.0.join("tools");
    // glab is signed in to the company's own server only.
    let glab = fake_tool(
        &tools,
        "glab",
        "case \"$*\" in *gitlab.company.com*) echo 'Logged in to gitlab.company.com' >&2; exit 0;; esac\nexit 1\n",
    );
    let linear = fake_linear();
    let missing = PathBuf::from("/nonexistent/ae-test/tool");
    let hosts = Hosts::with(missing.clone(), glab, missing, linear.url.clone());
    let state = AppState::new(sandbox.manager_with(hosts));
    let manager = &state.manager;

    // Nothing yet: the report says what to do first.
    let report = manager.setup_report();
    assert!(report.contains("No projects yet. Add the user's repositories with `add_project`."), "{report}");
    assert!(report.contains("Not connected: there is no key."), "{report}");
    assert!(report.contains("- `gh` (GitHub): not installed."), "{report}");

    let repo = git_project(&sandbox);
    git(&repo, &["remote", "add", "origin", "git@gitlab.company.com:group/shop.git"]);
    let project = manager.add_project(&repo.to_string_lossy(), Some("shop".into())).unwrap();
    let report = manager.setup_report();
    assert!(
        report.contains("code on GitLab at gitlab.company.com, group/shop; issues come from the code host."),
        "{report}"
    );
    assert!(report.contains("- `glab` (GitLab): ready."), "{report}");
    assert!(report.contains("GitLab servers of the projects: gitlab.company.com."), "{report}");

    // A team needs Linear connected, and a team the workspace has.
    let refused = manager.setup_linear_team("shop", "APP").unwrap_err().to_string();
    assert!(refused.contains("paste a personal API key in the Issues screen"), "{refused}");
    manager.set_linear_key(Some("lin_test_key")).unwrap();
    let report = manager.setup_report();
    assert!(report.contains("Connected as Viktor in the workspace Shop Inc."), "{report}");
    assert!(report.contains("Teams: APP (App), OPS (Operations)."), "{report}");
    let wrong = manager.setup_linear_team("shop", "WEB").unwrap_err().to_string();
    assert_eq!(wrong, "The workspace has no team \"WEB\". Its teams are: APP, OPS.");
    let unknown = manager.setup_linear_team("api", "APP").unwrap_err().to_string();
    assert_eq!(unknown, "There is no project \"api\". The projects are: shop.");
    assert_eq!(manager.setup_linear_team("SHOP", "app").unwrap(), "shop: issues now come from Linear team APP.");
    assert!(manager.setup_report().contains("issues come from Linear team APP."));
    assert_eq!(manager.setup_linear_team(&project.id, "none").unwrap(), "shop: issues come from its code host again.");

    // The task the setup button starts: the guide's instruction first, then what the user said.
    let mut events = manager.subscribe();
    let task = dispatch(
        &state,
        "setup.start",
        &json!({ "projectId": project.id, "agentId": "mock", "text": "Linear team APP, our own GitLab" }),
    )
    .await
    .unwrap();
    let id = task["id"].as_str().unwrap().to_string();
    let seen = until(&mut events, &id, |e| state_is(e, "idle")).await;
    let first = seen.iter().find(|e| is(e, "user_message")).expect("the first message is sent");
    let text = first["text"].as_str().unwrap();
    assert!(text.starts_with(ae_core::setup::SETUP_PROMPT), "{text}");
    assert!(text.ends_with("What my team uses: Linear team APP, our own GitLab"), "{text}");
}

#[tokio::test]
async fn a_linear_key_that_may_only_read_is_said_in_words_once_and_not_tried_again() {
    let sandbox = Sandbox::new("linear-read-only");
    let linear = fake_linear();
    let missing = PathBuf::from("/nonexistent/ae-test/tool");
    let hosts = Hosts::with(missing.clone(), missing.clone(), missing, linear.url.clone());
    let state = AppState::new(sandbox.manager_with(hosts));
    let manager = &state.manager;
    let project = manager.add_project(&git_project(&sandbox).to_string_lossy(), None).unwrap();
    dispatch(&state, "integrations.setLinearTeam", &json!({ "projectId": project.id, "team": "APP" })).await.unwrap();
    dispatch(&state, "integrations.setLinearKey", &json!({ "key": "lin_read_only" })).await.unwrap();
    let status = dispatch(&state, "integrations.status", &json!({})).await.unwrap();
    assert_eq!((status["linear"]["hasKey"].clone(), status["linear"]["readOnly"].clone()), (json!(true), json!(false)));

    // The task starts all the same; that the issue was not moved is said with the way to mend it.
    let mut events = manager.subscribe();
    let start = json!({ "projectId": project.id, "key": "APP-214", "agentId": "mock" });
    let task = dispatch(&state, "issues.start", &start).await.unwrap();
    let id = task["id"].as_str().unwrap().to_string();
    let seen = until(&mut events, &id, |e| is(e, "tracker")).await;
    let told = seen.last().unwrap();
    let text = told["text"].as_str().unwrap();
    assert!(text.starts_with("The issue was not moved in Linear: the key kept here may only read."), "{text}");
    assert!(text.contains("write access"), "{text}");
    assert!(!text.contains("Invalid scope"), "Linear's own words are not the user's: {text}");
    assert_eq!(told["failed"], true);
    // From now on the screens can say so before anything is started.
    assert_eq!(dispatch(&state, "integrations.status", &json!({})).await.unwrap()["linear"]["readOnly"], true);

    // The next task does not ask Linear to move anything, and says so in a line that is no failure.
    dispatch(&state, "tasks.archive", &json!({ "taskId": id })).await.unwrap();
    let tries = |linear: &FakeLinear| {
        linear.asked.lock().unwrap().iter().filter(|(_, body)| body.contains("issueUpdate")).count()
    };
    let before = tries(&linear);
    let mut events = manager.subscribe();
    let again = dispatch(&state, "issues.start", &start).await.unwrap();
    let again_id = again["id"].as_str().unwrap().to_string();
    let seen = until(&mut events, &again_id, |e| is(e, "tracker")).await;
    assert_eq!(seen.last().unwrap()["text"], "The issue stays as it is in Linear: the key kept here may only read.");
    assert!(seen.last().unwrap().get("failed").is_none());
    assert_eq!(tries(&linear), before);

    // Another key is another story: what was learned about the old one is forgotten.
    dispatch(&state, "integrations.setLinearKey", &json!({ "key": "lin_test_key" })).await.unwrap();
    assert_eq!(dispatch(&state, "integrations.status", &json!({})).await.unwrap()["linear"]["readOnly"], false);
}

/// A stand-in for `gh` that knows two pull requests by their numbers: 128 is open on the branch
/// `feat/order-pages`, 129 was merged long ago.
fn gh_with_requests(dir: &Path) -> PathBuf {
    std::fs::create_dir_all(dir).unwrap();
    let pull = |number: u64, state: &str, branch: &str| {
        json!({ "number": number, "title": "feat(orders): pages", "url": format!("https://github.com/x/shop/pull/{number}"),
            "state": state, "isDraft": true, "headRefName": branch, "baseRefName": "main", "isCrossRepository": false })
        .to_string()
    };
    std::fs::write(dir.join("pull-128.json"), pull(128, "OPEN", "feat/order-pages")).unwrap();
    std::fs::write(dir.join("pull-129.json"), pull(129, "MERGED", "feat/old-pages")).unwrap();
    fake_tool(
        dir,
        "gh",
        r#"here="$(dirname "$0")"
echo "$@" >> "$here/calls.log"
case "$1 $2" in
  "auth status") exit 0 ;;
  "issue list") echo "[]" ;;
  "pr view")
    if [ -f "$here/pull-$3.json" ]; then cat "$here/pull-$3.json"; else echo "no pull requests found for branch \"$3\"" >&2; exit 1; fi ;;
  *) echo "unexpected call: $*" >&2; exit 2 ;;
esac
"#,
    )
}

#[tokio::test]
async fn a_task_goes_on_with_the_open_request_of_its_issue_on_that_request_s_branch() {
    let sandbox = Sandbox::new("on-request");
    let tools = sandbox.0.join("tools");
    let linear = fake_linear();
    let missing = PathBuf::from("/nonexistent/ae-test/tool");
    let hosts = Hosts::with(gh_with_requests(&tools), missing.clone(), missing, linear.url.clone());
    let state = AppState::new(sandbox.manager_with(hosts));
    let manager = &state.manager;
    let (repo, _) = github_project(&sandbox);
    // Somebody pushed work for the issue: a branch with a commit, which this folder no longer has.
    let trunk = git(&repo, &["symbolic-ref", "--short", "HEAD"]).trim().to_string();
    git(&repo, &["checkout", "-q", "-b", "feat/order-pages"]);
    std::fs::write(repo.join("pages.txt"), "fifty a page\n").unwrap();
    git(&repo, &["add", "pages.txt"]);
    git(&repo, &["commit", "-q", "-m", "Pages of fifty"]);
    git(&repo, &["push", "-q", "origin", "feat/order-pages"]);
    git(&repo, &["checkout", "-q", &trunk]);
    git(&repo, &["branch", "-q", "-D", "feat/order-pages"]);
    git(&repo, &["update-ref", "-d", "refs/remotes/origin/feat/order-pages"]);
    let project = manager.add_project(&repo.to_string_lossy(), None).unwrap();
    dispatch(&state, "integrations.setLinearKey", &json!({ "key": "lin_test_key" })).await.unwrap();

    // What is attached to the issue is looked through for requests of this very repository. The
    // one that is open is offered once; the merged one, the one of another repository and what
    // is no request at all are not.
    let asked = json!({ "projectId": project.id, "key": "APP-215", "source": "linear" });
    let found = dispatch(&state, "issues.requests", &asked).await.unwrap();
    assert_eq!(
        found["requests"],
        json!([{ "number": 128, "name": "PR #128", "title": "feat(orders): pages",
            "url": "https://github.com/x/shop/pull/128", "state": "open", "draft": true,
            "branch": "feat/order-pages", "base": "main", "own": true }])
    );
    let viewed: Vec<String> =
        calls(&tools).lines().filter(|line| line.starts_with("pr view")).map(|line| line[..11].to_string()).collect();
    assert_eq!(viewed, ["pr view 128", "pr view 129"]);

    // A request that is not open is nothing to go on with, and no task is made for it.
    let merged = dispatch(
        &state,
        "issues.start",
        &json!({ "projectId": project.id, "key": "APP-215", "agentId": "mock", "source": "linear", "request": 129 }),
    )
    .await;
    assert!(merged.unwrap_err().to_string().contains("PR #129 is merged"));
    assert!(manager.list().unwrap().is_empty());

    // Started on the open one, the task is on its branch, with what was pushed there, and is told so.
    let mut events = manager.subscribe();
    let task = dispatch(
        &state,
        "issues.start",
        &json!({ "projectId": project.id, "key": "APP-215", "agentId": "mock", "source": "linear", "request": 128 }),
    )
    .await
    .unwrap();
    let id = task["id"].as_str().unwrap().to_string();
    assert_eq!((task["branch"].as_str(), task["baseBranch"].as_str()), (Some("feat/order-pages"), Some("main")));
    let cwd = PathBuf::from(task["cwd"].as_str().unwrap());
    assert_eq!(git(&cwd, &["symbolic-ref", "--short", "HEAD"]).trim(), "feat/order-pages");
    assert_eq!(git(&cwd, &["rev-parse", "--abbrev-ref", "@{upstream}"]).trim(), "origin/feat/order-pages");
    assert_eq!(std::fs::read_to_string(cwd.join("pages.txt")).unwrap(), "fifty a page\n");
    let seen = until(&mut events, &id, |e| state_is(e, "idle")).await;
    let said = seen.iter().find(|e| is(e, "user_message")).unwrap()["text"].as_str().unwrap().to_string();
    assert!(said.contains("Orders need pagination") && said.contains("This is issue APP-215"), "{said}");
    assert!(
        said.contains(
            "The work on this goes on in PR #128 (https://github.com/x/shop/pull/128), on its branch `feat/order-pages`"
        ),
        "{said}"
    );
    // No branch of the task's own was made beside it.
    assert!(!git(&repo, &["branch", "--list", "task/*"]).contains("task/"));
}

#[tokio::test]
async fn a_working_copy_on_a_branch_that_is_there_takes_what_was_pushed_and_never_a_branch_that_is_open_elsewhere() {
    let sandbox = Sandbox::new("on-branch");
    let (repo, _) = github_project(&sandbox);
    let trunk = git(&repo, &["symbolic-ref", "--short", "HEAD"]).trim().to_string();
    git(&repo, &["checkout", "-q", "-b", "feat/pages"]);
    std::fs::write(repo.join("pages.txt"), "one\n").unwrap();
    git(&repo, &["add", "pages.txt"]);
    git(&repo, &["commit", "-q", "-m", "One"]);
    git(&repo, &["push", "-q", "origin", "feat/pages"]);
    // The remote goes one commit further than this folder knows: pushed from a second copy.
    let second = sandbox.0.join("second");
    git(&repo, &["worktree", "add", "-q", "--detach", &second.to_string_lossy(), "feat/pages"]);
    std::fs::write(second.join("pages.txt"), "one\ntwo\n").unwrap();
    git(&second, &["commit", "-q", "-am", "Two"]);
    git(&second, &["push", "-q", "origin", "HEAD:feat/pages"]);
    git(&repo, &["worktree", "remove", "--force", &second.to_string_lossy()]);

    // While the project's own folder has the branch open, it cannot be opened a second time: said in words.
    let dir = sandbox.0.join("copy");
    let refused = ae_core::git::add_worktree_on(&repo, &dir, "feat/pages").await.unwrap_err().to_string();
    assert!(refused.contains("The branch feat/pages is open in another folder of this project"), "{refused}");
    assert!(!dir.exists());

    // Once the folder is on another branch, the copy is on the branch as it was here, brought up to what was pushed.
    git(&repo, &["checkout", "-q", &trunk]);
    ae_core::git::add_worktree_on(&repo, &dir, "feat/pages").await.unwrap();
    assert_eq!(git(&dir, &["symbolic-ref", "--short", "HEAD"]).trim(), "feat/pages");
    assert_eq!(std::fs::read_to_string(dir.join("pages.txt")).unwrap(), "one\ntwo\n");

    // A branch nobody has is said to be nowhere.
    let nowhere = ae_core::git::add_worktree_on(&repo, &sandbox.0.join("none"), "feat/none").await.unwrap_err();
    assert!(nowhere.to_string().contains("is neither in this project's folder nor on its remote"), "{nowhere}");

    // Taken down again, the copy leaves its branch where it was.
    ae_core::git::remove_worktree_only(&repo, &dir).await;
    assert!(!dir.exists());
    assert!(git(&repo, &["branch", "--list", "feat/pages"]).contains("feat/pages"));
}
