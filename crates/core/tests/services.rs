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
    let task = manager.start_issue(&project.id, "#12", "mock").await.unwrap();
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

            let (status, answer) = if key != "lin_test_key" {
                ("401 Unauthorized", json!({ "errors": [{ "message": "Authentication required" }] }))
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
