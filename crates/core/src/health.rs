//! Health: what each project is missing.
//!
//! Every finding here is read from the project's own files, or from its
//! package manager's own audit when the user asks for one: a project with no
//! test command, a pipeline that never runs the lint, a file with secrets in
//! git, dependencies with known vulnerabilities. No model looks at anything,
//! so nothing is spent and nothing is guessed. "Fix" starts a task with an
//! agent and a plain description of what is missing.

use crate::checks;
use crate::hosts::remote_of;
use crate::store::Project;
use crate::tasks::{Manager, now_ms};
use anyhow::Result;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::Path;
use std::process::{Command, Stdio};

const IGNORED_KEY: &str = "health.ignored";
const AUDIT_KEY: &str = "health.audit";

struct Finding {
    kind: &'static str,
    category: &'static str,
    title: String,
    detail: String,
    /// What an agent is told when the user presses "Fix".
    fix: String,
}

fn finding(kind: &'static str, category: &'static str, title: &str, detail: &str, fix: &str) -> Finding {
    Finding { kind, category, title: title.into(), detail: detail.into(), fix: fix.into() }
}

/// The text of every pipeline file of a project, joined. Empty when it has none.
fn pipeline_text(root: &Path) -> Option<String> {
    let mut text = String::new();
    if let Ok(entries) = std::fs::read_dir(root.join(".github").join("workflows")) {
        for entry in entries.flatten() {
            let path = entry.path();
            if matches!(path.extension().and_then(|e| e.to_str()), Some("yml" | "yaml")) {
                text.push_str(&std::fs::read_to_string(path).unwrap_or_default());
            }
        }
    }
    for name in
        [".gitlab-ci.yml", ".circleci/config.yml", "azure-pipelines.yml", "Jenkinsfile", "bitbucket-pipelines.yml"]
    {
        text.push_str(&std::fs::read_to_string(root.join(name)).unwrap_or_default());
    }
    (!text.trim().is_empty()).then_some(text)
}

/// Files in git that look like they hold secrets.
fn secrets_in_git(root: &Path) -> Vec<String> {
    let Ok(output) = Command::new("git")
        .arg("-C")
        .arg(root)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .args(["ls-files", "-z"])
        .stdin(Stdio::null())
        .output()
    else {
        return vec![];
    };
    String::from_utf8_lossy(&output.stdout)
        .split('\0')
        .filter(|path| {
            let name = path.rsplit('/').next().unwrap_or(path).to_lowercase();
            let example = ["example", "sample", "template", "dist"].iter().any(|word| name.contains(word));
            let env = name == ".env" || (name.starts_with(".env.") && !example);
            env || name.ends_with(".pem") || name == "id_rsa" || name == "id_ed25519"
        })
        .take(5)
        .map(str::to_string)
        .collect()
}

/// What can be seen missing in a project by looking at its files.
fn look(root: &Path) -> Vec<Finding> {
    let mut found = Vec::new();
    let checks = checks::detect(root);
    let has = |kind: &str| checks.iter().any(|check| check.kind == kind);
    let has_code = ["package.json", "Cargo.toml", "go.mod", "pyproject.toml", "Makefile"]
        .iter()
        .any(|name| root.join(name).is_file());

    let secrets = secrets_in_git(root);
    if !secrets.is_empty() {
        found.push(finding(
            "secrets",
            "Security",
            &format!("A file that may hold secrets is in git: {}", secrets.join(", ")),
            "Everyone with the repository can read it, and it stays in the history.",
            &format!(
                "These files are tracked by git and may hold secrets: {}. Take them out of git without deleting them from disk, add them to .gitignore, and add an example file with the names of the settings but no values. Say which secrets should be changed because they were in the history.",
                secrets.join(", ")
            ),
        ));
    }
    if has_code && !has("test") {
        found.push(finding(
            "no-tests",
            "Tests",
            "There is no command that runs tests",
            "Nothing in the project's own files says how to test it, so no task can prove it broke nothing.",
            "This project has no command that runs tests. Set up a test runner that fits the project, add one small real test, and add the command to the project's own files so the tests run with one command. Say what you chose and why.",
        ));
    }
    if has_code && !has("lint") {
        found.push(finding(
            "no-lint",
            "Tests",
            "There is no lint command",
            "Mistakes a linter would catch are left for the review.",
            "This project has no lint command. Set up the linter that is usual for its language with sensible rules, add the command to the project's own files, and fix what it reports only where the fix is obvious. Say what you chose.",
        ));
    }
    let on_host = remote_of(root).is_some();
    match pipeline_text(root) {
        None if on_host && has_code => found.push(finding(
            "no-pipeline",
            "Pipeline",
            "Nothing runs before a merge",
            "The project lives on a code host but has no pipeline, so a merge request is never checked by a machine.",
            "This project has no pipeline. Add one for its code host that runs the project's own lint and test commands on every merge request. Keep it small. Say what it runs.",
        )),
        Some(text) => {
            let text = text.to_lowercase();
            let mentions = |words: &[&str]| words.iter().any(|word| text.contains(word));
            if has("lint") && !mentions(&["lint", "clippy", "ruff", "vet", "golangci"]) {
                found.push(finding(
                    "pipeline-no-lint",
                    "Pipeline",
                    "Lint does not run before merging",
                    "The project has a lint command, but its pipeline never runs it.",
                    "The pipeline of this project does not run its lint command. Add a step that runs it on every merge request. Change nothing else in the pipeline.",
                ));
            }
            if has("test") && !mentions(&["test", "vitest", "jest"]) {
                found.push(finding(
                    "pipeline-no-tests",
                    "Pipeline",
                    "Tests do not run before merging",
                    "The project has a test command, but its pipeline never runs it.",
                    "The pipeline of this project does not run its tests. Add a step that runs them on every merge request. Change nothing else in the pipeline.",
                ));
            }
        }
        None => {}
    }
    if root.join(".git").exists() && !root.join(".gitignore").is_file() {
        found.push(finding(
            "no-gitignore",
            "Housekeeping",
            "There is no .gitignore",
            "Build output and local settings can end up in commits.",
            "This project has no .gitignore. Add one that fits its language and tools. Do not remove any file from git; only list what should not be added from now on.",
        ));
    }
    if let Ok(text) = std::fs::read_to_string(root.join("package.json"))
        && (text.contains("\"dependencies\"") || text.contains("\"devDependencies\""))
        && !["package-lock.json", "pnpm-lock.yaml", "yarn.lock", "bun.lockb", "bun.lock"]
            .iter()
            .any(|name| root.join(name).is_file())
    {
        found.push(finding(
            "no-lockfile",
            "Housekeeping",
            "Dependencies have no lock file",
            "Two installs can bring different versions, so a bug may show on one machine only.",
            "This project has dependencies but no lock file. Create the lock file with the package manager the project uses and say which one that is.",
        ));
    }
    found
}

/// What `npm audit --json` says: how many dependencies have known vulnerabilities, by how bad.
pub fn read_npm_audit(json: &str) -> Option<(u64, u64, u64)> {
    let value: Value = serde_json::from_str(json).ok()?;
    let counts = value.pointer("/metadata/vulnerabilities")?;
    let of = |name: &str| counts.get(name).and_then(Value::as_u64).unwrap_or(0);
    Some((of("total"), of("high"), of("critical")))
}

/// Asks npm for its audit of a project. `None` when the project is not one npm can audit, or npm cannot be asked.
fn npm_audit(root: &Path) -> Option<String> {
    if !root.join("package-lock.json").is_file() {
        return None;
    }
    let output = Command::new("npm")
        .args(["audit", "--json"])
        .current_dir(root)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()?;
    // npm ends with a code other than zero when it finds something; its answer is still the report.
    Some(String::from_utf8_lossy(&output.stdout).into_owned())
}

impl Manager {
    fn ignored_findings(&self) -> HashSet<String> {
        self.store()
            .setting(IGNORED_KEY)
            .ok()
            .flatten()
            .and_then(|text| serde_json::from_str::<Vec<String>>(&text).ok())
            .map(|list| list.into_iter().collect())
            .unwrap_or_default()
    }

    /// Everything the projects are missing, the user's ignored findings left
    /// out, and for each project its checks with whether each is on.
    pub fn health(&self) -> Result<Value> {
        let ignored = self.ignored_findings();
        let audit: Value =
            self.store().setting(AUDIT_KEY)?.and_then(|text| serde_json::from_str(&text).ok()).unwrap_or(Value::Null);
        let mut findings = Vec::new();
        let mut left_out = 0;
        let mut projects = Vec::new();
        for project in self.store().projects()? {
            let root = Path::new(&project.path);
            let mut found = look(root);
            if let Some(counts) = audit.pointer(&format!("/projects/{}", project.id))
                && let (Some(total), Some(high), Some(critical)) =
                    (counts["total"].as_u64(), counts["high"].as_u64(), counts["critical"].as_u64())
                && total > 0
            {
                let bad = match (critical, high) {
                    (0, 0) => String::new(),
                    (0, high) => format!(", {high} of them high"),
                    (critical, 0) => format!(", {critical} of them critical"),
                    (critical, high) => format!(", {critical} critical and {high} high"),
                };
                found.push(finding(
                    "audit",
                    "Security",
                    &format!("{total} {} known vulnerabilities{bad}", if total == 1 { "dependency has" } else { "dependencies have" }),
                    "Found by the package manager's own audit the last time it was asked.",
                    "The package manager's audit reports dependencies with known vulnerabilities. Run the audit, update what can be updated without breaking the project, run the tests, and say what could not be updated and why.",
                ));
            }
            // What matters most comes first.
            found.sort_by_key(|one| {
                ["Security", "Tests", "Pipeline", "Housekeeping"].iter().position(|c| *c == one.category)
            });
            for one in found {
                let id = format!("{}:{}", project.id, one.kind);
                if ignored.contains(&id) {
                    left_out += 1;
                    continue;
                }
                findings.push(json!({
                    "id": id,
                    "projectId": project.id,
                    "category": one.category,
                    "title": one.title,
                    "detail": one.detail,
                    "fix": one.fix,
                }));
            }
            projects.push(json!({ "projectId": project.id, "checks": self.project_checks_list(&project)? }));
        }
        Ok(json!({
            "findings": findings,
            "ignored": left_out,
            "auditedAt": audit.get("at").cloned().unwrap_or(Value::Null),
            "projects": projects,
        }))
    }

    /// Ignores a finding, or brings every ignored one back (`id` is `None`).
    pub fn health_ignore(&self, id: Option<&str>) -> Result<()> {
        let mut ignored = self.ignored_findings();
        match id {
            Some(id) => {
                ignored.insert(id.to_string());
            }
            None => ignored.clear(),
        }
        let list: Vec<&String> = ignored.iter().collect();
        self.store().set_setting(IGNORED_KEY, Some(&serde_json::to_string(&list)?))
    }

    /// Asks each project's package manager for its audit and keeps the answers.
    /// This reaches the network; it runs only when the user asks.
    pub fn health_audit(&self) -> Result<()> {
        self.health_audit_with(&npm_audit)
    }

    /// The same with the asking given from outside, so it can be tried without a network.
    pub fn health_audit_with(&self, ask: &dyn Fn(&Path) -> Option<String>) -> Result<()> {
        let mut projects = serde_json::Map::new();
        for project in self.store().projects()? {
            let Some((total, high, critical)) = ask(Path::new(&project.path)).as_deref().and_then(read_npm_audit)
            else {
                continue;
            };
            projects.insert(project.id, json!({ "total": total, "high": high, "critical": critical }));
        }
        let audit = json!({ "at": now_ms(), "projects": projects });
        self.store().set_setting(AUDIT_KEY, Some(&audit.to_string()))
    }

    /// The checks of a project with whether each is on. See `checks`.
    fn project_checks_list(&self, project: &Project) -> Result<Value> {
        Ok(json!(self.project_check_defs(&project.id, Path::new(&project.path))))
    }
}
