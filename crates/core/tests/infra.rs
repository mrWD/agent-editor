//! The infrastructure a task changes: found in the files, read before and after, and checked by hand.
//!
//! Nothing of Terraform or Helm is installed here: the files are read, and the checks are
//! "run" by a stand-in that only says what it was given.
#![cfg(unix)]

mod common;

use ae_core::preview::Previews;
use ae_core::server::{AppState, dispatch};
use ae_core::tools::Tools;
use common::*;
use serde_json::{Value, json};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

async fn asked(state: &AppState, method: &str, params: Value) -> anyhow::Result<Value> {
    dispatch(state, method, &params).await
}

async fn idle_task(state: &AppState, project_id: &str) -> String {
    let mut events = state.manager.subscribe();
    let task = state.manager.create(project_id, "mock", "What is here?").await.unwrap();
    until(&mut events, &task.row.id, |e| state_is(e, "idle")).await;
    task.row.id
}

fn write(folder: &Path, path: &str, text: &str) {
    let file = folder.join(path);
    std::fs::create_dir_all(file.parent().unwrap()).unwrap();
    std::fs::write(file, text).unwrap();
}

/// A state whose Infra view runs these programs and no others.
fn state_with(sandbox: &Sandbox, terraform: PathBuf, helm: PathBuf) -> AppState {
    let mut state = AppState::new(sandbox.manager());
    state.previews =
        Arc::new(Previews::with_tools(Tools { terraform: Some(terraform), helm: Some(helm), ..Tools::default() }));
    state
}

fn missing(name: &str) -> PathBuf {
    PathBuf::from(format!("/nonexistent/ae-test/{name}"))
}

const MAIN_TF: &str = r#"resource "aws_db_instance" "orders" {
  instance_class = "db.t3.micro"
}

resource "aws_s3_bucket" "exports" {
  bucket = "shop-exports"
}
"#;

const WEB_YAML: &str = r#"apiVersion: apps/v1
kind: Deployment
metadata:
  name: web
spec:
  replicas: 2
  template:
    spec:
      containers:
        - name: web
          image: shop/web:1.4
          ports:
            - containerPort: 8080
"#;

/// Each step of a file as its field, its text and whether it is marked.
fn steps_of(found: &Value, path: &str) -> Vec<(String, String, bool)> {
    let file = found["files"]
        .as_array()
        .unwrap()
        .iter()
        .find(|file| file["path"] == path)
        .unwrap_or_else(|| panic!("{path} is not among {}", found["files"]));
    file["steps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|step| {
            (
                step["field"].as_str().unwrap_or_default().to_string(),
                step["text"].as_str().unwrap().to_string(),
                !step["danger"].is_null(),
            )
        })
        .collect()
}

fn step(field: &str, text: &str, marked: bool) -> (String, String, bool) {
    (field.to_string(), text.to_string(), marked)
}

#[tokio::test]
async fn what_a_task_changes_in_the_infrastructure_is_read_tool_by_tool_with_the_dangers_marked() {
    let sandbox = Sandbox::new("infra-read");
    let project = git_project(&sandbox);
    write(&project, "infra/main.tf", MAIN_TF);
    write(&project, "k8s/web.yaml", WEB_YAML);
    write(&project, "Dockerfile", "FROM node:18-alpine\nEXPOSE 3000\n");
    write(&project, ".github/workflows/ci.yml", "on: push\njobs:\n  test:\n    runs-on: ubuntu-latest\n");
    write(&project, "config/app.yaml", "name: shop\n");
    git(&project, &["add", "."]);
    git(&project, &["commit", "-q", "-m", "Infrastructure"]);
    let state = state_with(&sandbox, missing("terraform"), missing("helm"));
    let added = state.manager.add_project(&project.to_string_lossy(), None).unwrap();
    let task_id = idle_task(&state, &added.id).await;
    let folder = state.manager.cwd(&task_id).unwrap();

    // The project has infrastructure, and the task has touched none of it yet.
    assert_eq!(asked(&state, "infra.has", json!({ "taskId": task_id })).await.unwrap(), json!({ "has": true }));
    let before = asked(&state, "infra.get", json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(
        before["tools"],
        json!([
            { "kind": "terraform", "files": 1 },
            { "kind": "kubernetes", "files": 1 },
            { "kind": "docker", "files": 1 },
            { "kind": "ci", "files": 1 }
        ])
    );
    assert_eq!(before["files"], json!([]));
    assert_eq!(before["note"], Value::Null);

    // The task takes a bucket away and makes the database larger, gives the app another image, more
    // of it and a privileged container, moves the Dockerfile to another base, and writes a compose file
    // with a password in it. A file that is not infrastructure is touched too.
    write(
        &folder,
        "infra/main.tf",
        "resource \"aws_db_instance\" \"orders\" {\n  instance_class = \"db.t3.small\"\n}\n\nresource \"aws_sqs_queue\" \"mail\" {\n  name = \"mail\"\n}\n",
    );
    write(
        &folder,
        "k8s/web.yaml",
        &WEB_YAML
            .replace("replicas: 2", "replicas: 4")
            .replace("shop/web:1.4", "shop/web:1.5")
            .replace("          ports:", "          securityContext:\n            privileged: true\n          ports:"),
    );
    write(&folder, "Dockerfile", "FROM node:20-alpine\nEXPOSE 3000 9229\n");
    write(
        &folder,
        "compose.yaml",
        "services:\n  db:\n    image: postgres:16\n    environment:\n      POSTGRES_PASSWORD: hunter2hunter2\n",
    );
    write(&folder, "config/app.yaml", "name: shop\ndebug: true\n");
    std::fs::remove_file(folder.join(".github/workflows/ci.yml")).unwrap();

    let after = asked(&state, "infra.get", json!({ "taskId": task_id })).await.unwrap();
    let listed: Vec<(&str, &str, &str)> = after["files"]
        .as_array()
        .unwrap()
        .iter()
        .map(|file| (file["tool"].as_str().unwrap(), file["path"].as_str().unwrap(), file["change"].as_str().unwrap()))
        .collect();
    assert_eq!(
        listed,
        [
            ("terraform", "infra/main.tf", "modified"),
            ("kubernetes", "k8s/web.yaml", "modified"),
            ("docker", "Dockerfile", "modified"),
            ("compose", "compose.yaml", "added"),
            ("ci", ".github/workflows/ci.yml", "deleted"),
        ],
        "grouped by tool, and only what is infrastructure"
    );
    assert_eq!(
        steps_of(&after, "infra/main.tf"),
        [
            step("", "Changes resource `aws_db_instance.orders`: `instance_class`", true),
            step("", "Removes resource `aws_s3_bucket.exports`", true),
            step("", "Adds resource `aws_sqs_queue.mail`", false),
        ]
    );
    assert_eq!(
        steps_of(&after, "k8s/web.yaml"),
        [
            step("", "Changes Deployment `web`", false),
            step("replicas", "4, was 2", false),
            step("image", "of `web`: `shop/web:1.5`, was `shop/web:1.4`", false),
            step("", "`web` runs privileged", true),
        ]
    );
    assert_eq!(
        steps_of(&after, "Dockerfile"),
        [
            step("image", "base `node:20-alpine`, was `node:18-alpine`", false),
            step("ports", "exposes 3000, 9229, was 3000", false),
        ]
    );
    assert_eq!(
        steps_of(&after, "compose.yaml"),
        [
            step("", "Adds service `db`", false),
            step("image", "of `db`: `postgres:16`", false),
            step("env", "of `db`: `POSTGRES_PASSWORD`", false),
            step("", "`POSTGRES_PASSWORD` is written in plain text", true),
        ]
    );
    assert_eq!(steps_of(&after, ".github/workflows/ci.yml"), [step("", "Removes job `test`", true)]);
    assert!(!after.to_string().contains("hunter2"), "a secret is named and never repeated");
    let why = after["files"][0]["steps"][1]["danger"].as_str().unwrap();
    assert_eq!(why, "Terraform destroys it on the next apply, with whatever it holds.");

    // What the task committed is read the same as what it only wrote.
    git(&folder, &["add", "-A"]);
    git(&folder, &["commit", "-q", "-m", "Infrastructure, changed"]);
    let committed = asked(&state, "infra.get", json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(committed["files"], after["files"]);
    // The compose file is part of the project now.
    assert_eq!(committed["tools"][3], json!({ "kind": "compose", "files": 1 }));
}

#[tokio::test]
async fn all_the_infrastructure_of_a_project_is_read_as_it_stands_with_what_the_task_touched_said_so() {
    let sandbox = Sandbox::new("infra-all");
    let project = git_project(&sandbox);
    write(&project, "infra/main.tf", MAIN_TF);
    write(&project, "k8s/web.yaml", WEB_YAML);
    write(&project, "Dockerfile", "FROM node:18-alpine\nUSER node\nEXPOSE 3000\n");
    write(&project, ".github/workflows/ci.yml", "on: push\njobs:\n  test:\n    runs-on: ubuntu-latest\n");
    git(&project, &["add", "."]);
    git(&project, &["commit", "-q", "-m", "Infrastructure"]);
    let state = state_with(&sandbox, missing("terraform"), missing("helm"));
    let added = state.manager.add_project(&project.to_string_lossy(), None).unwrap();
    let task_id = idle_task(&state, &added.id).await;
    let folder = state.manager.cwd(&task_id).unwrap();

    // Before the task touches anything: every file, tool by tool, with what it holds.
    let all = asked(&state, "infra.all", json!({ "taskId": task_id })).await.unwrap();
    let listed = |all: &Value| -> Vec<(String, String, Value)> {
        all["files"]
            .as_array()
            .unwrap()
            .iter()
            .map(|file| {
                (
                    file["tool"].as_str().unwrap().to_string(),
                    file["path"].as_str().unwrap().to_string(),
                    file["change"].clone(),
                )
            })
            .collect()
    };
    let row = |tool: &str, path: &str, change: Value| (tool.to_string(), path.to_string(), change);
    assert_eq!(
        listed(&all),
        [
            row("terraform", "infra/main.tf", Value::Null),
            row("kubernetes", "k8s/web.yaml", Value::Null),
            row("docker", "Dockerfile", Value::Null),
            row("ci", ".github/workflows/ci.yml", Value::Null),
        ]
    );
    assert_eq!(all["more"], 0);
    assert_eq!(
        steps_of(&all, "infra/main.tf"),
        [step("", "Resource `aws_db_instance.orders`", false), step("", "Resource `aws_s3_bucket.exports`", false)]
    );
    assert_eq!(
        steps_of(&all, "k8s/web.yaml"),
        [
            step("", "Deployment `web`", false),
            step("replicas", "2", false),
            step("image", "of `web`: `shop/web:1.4`", false),
            step("ports", "of `web`: 8080", false),
        ]
    );
    assert_eq!(
        steps_of(&all, "Dockerfile"),
        [
            step("image", "base `node:18-alpine`", false),
            step("ports", "exposes 3000", false),
            step("", "Runs as `node`", false),
        ]
    );
    assert_eq!(
        steps_of(&all, ".github/workflows/ci.yml"),
        [step("", "Job `test`", false), step("", "Runs on push", false)]
    );

    // The task changes one file, writes another and removes a third: the files are read as they
    // stand now, the first two are said to be touched, and the one that is gone is not listed.
    write(&folder, "Dockerfile", "FROM node:20-alpine\nUSER root\nEXPOSE 3000\n");
    write(
        &folder,
        "compose.yaml",
        "services:\n  db:\n    image: postgres:16\n    environment:\n      POSTGRES_PASSWORD: hunter2hunter2\n",
    );
    std::fs::remove_file(folder.join(".github/workflows/ci.yml")).unwrap();
    let all = asked(&state, "infra.all", json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(
        listed(&all),
        [
            row("terraform", "infra/main.tf", Value::Null),
            row("kubernetes", "k8s/web.yaml", Value::Null),
            row("docker", "Dockerfile", json!("modified")),
            row("compose", "compose.yaml", json!("added")),
        ]
    );
    // What deserves a second look in a file as it stands is marked the same as in a change.
    assert_eq!(
        steps_of(&all, "Dockerfile"),
        [
            step("image", "base `node:20-alpine`", false),
            step("ports", "exposes 3000", false),
            step("", "Runs as `root`", true),
        ]
    );
    assert_eq!(
        steps_of(&all, "compose.yaml"),
        [
            step("", "Service `db`", false),
            step("image", "of `db`: `postgres:16`", false),
            step("env", "of `db`: `POSTGRES_PASSWORD`", false),
            step("", "`POSTGRES_PASSWORD` is written in plain text", true),
        ]
    );
    assert!(!all.to_string().contains("hunter2"), "a secret is named and never repeated");
}

#[tokio::test]
async fn what_was_only_moved_is_not_read_as_removed() {
    let sandbox = Sandbox::new("infra-moves");
    let project = git_project(&sandbox);
    write(&project, "infra/main.tf", MAIN_TF);
    write(&project, "k8s/web.yaml", WEB_YAML);
    write(&project, "Dockerfile", "FROM node:18-alpine\n");
    git(&project, &["add", "."]);
    git(&project, &["commit", "-q", "-m", "Infrastructure"]);
    let state = state_with(&sandbox, missing("terraform"), missing("helm"));
    let added = state.manager.add_project(&project.to_string_lossy(), None).unwrap();
    let task_id = idle_task(&state, &added.id).await;
    let folder = state.manager.cwd(&task_id).unwrap();

    // The bucket goes into a file of its own in the same folder; the manifest goes into another
    // folder as it is; the Dockerfile goes into another folder and gets another base on the way.
    let (database, bucket) = MAIN_TF.split_at(MAIN_TF.find("resource \"aws_s3_bucket\"").unwrap());
    write(&folder, "infra/main.tf", database);
    write(&folder, "infra/storage.tf", bucket);
    std::fs::remove_file(folder.join("k8s/web.yaml")).unwrap();
    write(&folder, "deploy/web.yaml", WEB_YAML);
    std::fs::remove_file(folder.join("Dockerfile")).unwrap();
    write(&folder, "docker/Dockerfile", "FROM node:20-alpine\n");

    let found = asked(&state, "infra.get", json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(
        steps_of(&found, "infra/main.tf"),
        [step("", "Moves resource `aws_s3_bucket.exports` to `storage.tf`", false)]
    );
    assert_eq!(
        steps_of(&found, "infra/storage.tf"),
        [step("", "Takes resource `aws_s3_bucket.exports` from `main.tf`", false)]
    );
    assert_eq!(
        steps_of(&found, "k8s/web.yaml"),
        [step("", "Moved to `deploy/web.yaml`, with nothing in it changed", false)]
    );
    assert_eq!(
        steps_of(&found, "deploy/web.yaml"),
        [step("", "Moved from `k8s/web.yaml`, with nothing in it changed", false)]
    );
    // A file that changed on the way is read as one that went and one that came.
    assert_eq!(steps_of(&found, "Dockerfile"), [step("", "Removes the Dockerfile", true)]);
    assert_eq!(steps_of(&found, "docker/Dockerfile"), [step("image", "base `node:20-alpine`", false)]);
}

#[tokio::test]
async fn a_project_without_infrastructure_has_none_and_a_task_without_a_base_says_so() {
    let sandbox = Sandbox::new("infra-none");
    // Not a git repository: there is no base to tell the task's changes from.
    let project = sandbox.project();
    write(&project, "src/app.ts", "export {};\n");
    let state = state_with(&sandbox, missing("terraform"), missing("helm"));
    let added = state.manager.add_project(&project.to_string_lossy(), None).unwrap();
    let task_id = idle_task(&state, &added.id).await;
    assert_eq!(asked(&state, "infra.has", json!({ "taskId": task_id })).await.unwrap(), json!({ "has": false }));
    let none = asked(&state, "infra.get", json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(none["tools"], json!([]));
    assert_eq!(none["checks"], json!([]));

    write(&state.manager.cwd(&task_id).unwrap(), "Dockerfile", "FROM node:20\n");
    assert_eq!(asked(&state, "infra.has", json!({ "taskId": task_id })).await.unwrap(), json!({ "has": true }));
    let some = asked(&state, "infra.get", json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(some["tools"], json!([{ "kind": "docker", "files": 1 }]));
    assert_eq!(some["files"], json!([]));
    assert_eq!(some["note"], "This task has no base branch, so what it changed cannot be told from the rest.");
}

/// Asks how a check stands until `done` is satisfied, and gives that answer.
async fn state_until(state: &AppState, task_id: &str, id: &str, done: impl Fn(&Value) -> bool) -> Value {
    let mut last = Value::Null;
    for _ in 0..400 {
        last = asked(state, "infra.state", json!({ "taskId": task_id, "id": id })).await.unwrap();
        if done(&last) {
            return last;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("the check never got there; it stands as {last}");
}

#[tokio::test]
async fn a_check_runs_only_when_asked_and_only_with_a_tool_that_is_here() {
    let sandbox = Sandbox::new("infra-checks");
    let project = git_project(&sandbox);
    write(&project, "infra/main.tf", MAIN_TF);
    write(&project, "deploy/shop/Chart.yaml", "name: shop\nversion: 1.0.0\n");
    write(&project, ".env", "CLOUD_KEY=sk-secret-value-123\n");
    git(&project, &["add", "."]);
    git(&project, &["commit", "-q", "-m", "Infrastructure"]);
    // The stand-in says where it was started and with what, and keeps a list of its calls.
    let tools = sandbox.0.join("tools");
    let log = tools.join("terraform.log");
    let terraform = fake_tool(
        &tools,
        "terraform",
        &format!(
            "echo \"$@\" >> '{}'\necho \"terraform $* in $(basename \"$PWD\")\"\necho \"key $CLOUD_KEY\"\nif [ \"$1\" = plan ]; then echo 'Plan: 1 to add, 0 to change, 1 to destroy.'; exit 2; fi\necho 'Success! The configuration is valid.'\n",
            log.display()
        ),
    );
    let state = state_with(&sandbox, terraform.clone(), missing("helm"));
    let added = state.manager.add_project(&project.to_string_lossy(), None).unwrap();
    let task_id = idle_task(&state, &added.id).await;
    let run = |id: &str| json!({ "taskId": task_id, "id": id });

    let found = asked(&state, "infra.get", json!({ "taskId": task_id })).await.unwrap();
    let checks: Vec<(&str, &str, bool)> = found["checks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|check| {
            (check["id"].as_str().unwrap(), check["name"].as_str().unwrap(), check["ready"].as_bool().unwrap())
        })
        .collect();
    assert_eq!(
        checks,
        [
            ("terraform-validate:infra", "terraform validate", true),
            ("terraform-plan:infra", "terraform plan", true),
            ("helm-template:deploy/shop", "helm template", false),
        ]
    );
    assert!(!log.exists(), "reading the infrastructure starts no tool");

    let never = asked(&state, "infra.state", run("terraform-validate:infra")).await.unwrap();
    assert_eq!(never, json!({ "running": false, "termId": null, "exit": null, "output": null }));

    let started = asked(&state, "infra.run", run("terraform-validate:infra")).await.unwrap();
    assert!(started["command"].as_str().unwrap().ends_with("/tools/terraform validate -no-color"), "{started}");
    let ended = state_until(&state, &task_id, "terraform-validate:infra", |now| now["exit"].is_u64()).await;
    assert_eq!(ended["exit"], 0);
    let printed = ended["output"].as_str().unwrap();
    assert!(printed.contains("terraform validate -no-color in infra"), "it runs in the folder of the files: {printed}");
    assert!(printed.contains("Success! The configuration is valid."), "{printed}");
    assert!(!printed.contains("sk-secret-value-123"), "a secret of the task is never shown: {printed}");

    // A plan is asked not to lock or refresh the state, and how it ended is kept.
    asked(&state, "infra.run", run("terraform-plan:infra")).await.unwrap();
    let planned = state_until(&state, &task_id, "terraform-plan:infra", |now| now["exit"].is_u64()).await;
    assert_eq!(planned["exit"], 2);
    assert!(planned["output"].as_str().unwrap().contains("Plan: 1 to add, 0 to change, 1 to destroy."));
    let calls = std::fs::read_to_string(&log).unwrap();
    assert_eq!(calls, "validate -no-color\nplan -lock=false -refresh=false -input=false -no-color\n");

    // A tool that is not here is not started, and neither is a check that is not there.
    let helm = asked(&state, "infra.run", run("helm-template:deploy/shop")).await.unwrap_err().to_string();
    assert_eq!(helm, "Install Helm on this computer to run it here.");
    let unknown = asked(&state, "infra.run", run("terraform-apply:infra")).await.unwrap_err().to_string();
    assert_eq!(unknown, "This project has no check terraform-apply:infra");
    state.terminals.close_all();
}

#[tokio::test]
async fn a_chart_is_rendered_by_helm_and_a_check_that_goes_on_can_be_stopped() {
    let sandbox = Sandbox::new("infra-helm");
    let project = git_project(&sandbox);
    write(&project, "deploy/shop/Chart.yaml", "name: shop\nversion: 1.0.0\n");
    write(&project, "deploy/shop/values.yaml", "image:\n  tag: '1.4'\n");
    git(&project, &["add", "."]);
    git(&project, &["commit", "-q", "-m", "A chart"]);
    let helm = fake_tool(&sandbox.0.join("tools"), "helm", "echo \"rendering $*\"\nsleep 30\n");
    let state = state_with(&sandbox, missing("terraform"), helm);
    let added = state.manager.add_project(&project.to_string_lossy(), None).unwrap();
    let task_id = idle_task(&state, &added.id).await;
    write(&state.manager.cwd(&task_id).unwrap(), "deploy/shop/values.yaml", "image:\n  tag: '1.5'\n");
    let id = "helm-template:deploy/shop";
    let run = json!({ "taskId": task_id, "id": id });

    let found = asked(&state, "infra.get", json!({ "taskId": task_id })).await.unwrap();
    assert_eq!(found["checks"][0]["id"], id);
    assert_eq!(found["checks"][0]["changed"], true, "the task touched the chart");
    assert_eq!(found["files"][0]["steps"][0]["text"], "`image.tag`: `1.5`, was `1.4`");

    asked(&state, "infra.run", run.clone()).await.unwrap();
    let going = state_until(&state, &task_id, id, |now| {
        now["output"].as_str().is_some_and(|text| text.contains("rendering template ."))
    })
    .await;
    assert_eq!(going["running"], true);
    let twice = asked(&state, "infra.run", run.clone()).await.unwrap_err().to_string();
    assert_eq!(twice, "helm template is running already. Stop it first, or wait for it to end.");
    asked(&state, "infra.stop", run).await.unwrap();
    state_until(&state, &task_id, id, |now| now["running"] == false).await;
    state.terminals.close_all();
}
