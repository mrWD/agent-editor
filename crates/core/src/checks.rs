//! The checks of a task: what must hold before the task is done.
//!
//! Two kinds of lines make up "Done when":
//! - checks a machine can run: the project's own lint, types, format and test
//!   commands, found in its files (`package.json`, `Cargo.toml`, `go.mod`,
//!   `pyproject.toml`, a `Makefile`), plus commands the user adds;
//! - lines only a person can confirm, taken from the task's issue or added by hand.
//!
//! A check is a command that passes when it ends with code 0. Its output is
//! kept, and the places it names ("src/a.ts:12:5: …") become problems that
//! open the file at the line. No model is asked for any of this.

use crate::tasks::{Manager, now_ms};
use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

/// How much of a check's output is kept: its end, where the verdict is.
const KEPT_BYTES: usize = 200 * 1024;
/// How many lines of the output travel to the interface.
const TAIL_LINES: usize = 80;
/// How many problems are read out of one check's output, at most.
const MAX_PROBLEMS: usize = 200;

/// A command that checks something about the project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckDef {
    pub id: String,
    pub title: String,
    pub command: String,
    /// "lint", "types", "format", "test", "e2e" or "custom".
    pub kind: String,
    /// Where it was found: a file's name, or "you".
    pub from: String,
}

fn def(id: &str, title: &str, command: String, kind: &str, from: &str) -> CheckDef {
    CheckDef { id: id.into(), title: title.into(), command, kind: kind.into(), from: from.into() }
}

fn package_manager(root: &Path) -> &'static str {
    if root.join("pnpm-lock.yaml").is_file() {
        "pnpm"
    } else if root.join("yarn.lock").is_file() {
        "yarn"
    } else if root.join("bun.lockb").is_file() || root.join("bun.lock").is_file() {
        "bun"
    } else {
        "npm"
    }
}

/// The checks a project brings with it, read from its own files.
pub fn detect(root: &Path) -> Vec<CheckDef> {
    let mut found: Vec<CheckDef> = Vec::new();
    let has = |found: &[CheckDef], kind: &str| found.iter().any(|check| check.kind == kind);

    if let Ok(text) = std::fs::read_to_string(root.join("package.json"))
        && let Ok(package) = serde_json::from_str::<Value>(&text)
        && let Some(scripts) = package.get("scripts").and_then(Value::as_object)
    {
        let manager = package_manager(root);
        let wanted: [(&str, &[&str], &str); 5] = [
            ("lint", &["lint", "lint:check", "eslint"], "Lint is clean"),
            ("types", &["typecheck", "type-check", "check-types", "types", "tsc"], "Types are right"),
            ("format", &["format:check", "fmt:check", "prettier:check", "check-format"], "The code is formatted"),
            ("test", &["test", "test:unit", "unit"], "Unit tests pass"),
            ("e2e", &["e2e", "test:e2e"], "End-to-end tests pass"),
        ];
        for (kind, names, title) in wanted {
            let Some(name) = names.iter().find(|name| scripts.get(**name).and_then(Value::as_str).is_some()) else {
                continue;
            };
            let script = scripts[*name].as_str().unwrap_or_default();
            // What `npm init` writes for "test" is a reminder, not a test.
            if script.contains("no test specified") {
                continue;
            }
            found.push(def(kind, title, format!("{manager} run {name}"), kind, "package.json"));
        }
    }
    if root.join("Cargo.toml").is_file() {
        if !has(&found, "format") {
            found.push(def(
                "format",
                "The code is formatted",
                "cargo fmt --all -- --check".into(),
                "format",
                "Cargo.toml",
            ));
        }
        if !has(&found, "lint") {
            let command = "cargo clippy --all-targets -- -D warnings".to_string();
            found.push(def("lint", "Lint is clean", command, "lint", "Cargo.toml"));
        }
        if !has(&found, "test") {
            found.push(def("test", "Unit tests pass", "cargo test".into(), "test", "Cargo.toml"));
        }
    }
    if root.join("go.mod").is_file() {
        if !has(&found, "lint") {
            found.push(def("lint", "Lint is clean", "go vet ./...".into(), "lint", "go.mod"));
        }
        if !has(&found, "test") {
            found.push(def("test", "Unit tests pass", "go test ./...".into(), "test", "go.mod"));
        }
    }
    if let Ok(text) = std::fs::read_to_string(root.join("pyproject.toml")) {
        if text.contains("[tool.ruff") && !has(&found, "lint") {
            found.push(def("lint", "Lint is clean", "ruff check .".into(), "lint", "pyproject.toml"));
        }
        if (text.contains("[tool.pytest") || text.contains("pytest")) && !has(&found, "test") {
            found.push(def("test", "Unit tests pass", "pytest".into(), "test", "pyproject.toml"));
        }
    }
    if let Ok(text) = std::fs::read_to_string(root.join("Makefile")) {
        for (kind, title) in [("lint", "Lint is clean"), ("test", "Unit tests pass")] {
            if !has(&found, kind) && text.lines().any(|line| line.starts_with(&format!("{kind}:"))) {
                found.push(def(kind, title, format!("make {kind}"), kind, "Makefile"));
            }
        }
    }
    found
}

/// One thing a check's output names as wrong.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Problem {
    pub path: String,
    pub line: usize,
    pub column: Option<usize>,
    /// "error" or "warning".
    pub severity: &'static str,
    pub message: String,
}

fn severity_of(text: &str) -> &'static str {
    let lower = text.to_lowercase();
    if lower.contains("warning") && !lower.contains("error") { "warning" } else { "error" }
}

/// A path the output names, as the task sees it: relative, with forward
/// slashes, and only when such a file is really there.
fn known_file(root: &Path, named: &str) -> Option<String> {
    let named = named.trim().trim_start_matches("./");
    if named.is_empty() || named.contains(char::is_whitespace) {
        return None;
    }
    let path = Path::new(named);
    let relative: PathBuf = if path.is_absolute() { path.strip_prefix(root).ok()?.to_path_buf() } else { path.into() };
    root.join(&relative).is_file().then(|| relative.to_string_lossy().replace('\\', "/"))
}

/// Reads "file, line, what is wrong" out of what a check printed. Understood:
/// `path:line:col: message` (most tools), `path(line,col): message` (tsc),
/// a message followed by `--> path:line:col` (rustc), and a path on its own
/// line followed by `line:col  error  message` rows (ESLint).
pub fn problems_in(output: &str, root: &Path) -> Vec<Problem> {
    let mut problems: Vec<Problem> = Vec::new();
    let mut eslint_file: Option<String> = None;
    let mut rust_message: Option<String> = None;
    let number = |text: &str| text.trim().parse::<usize>().ok().filter(|n| *n > 0);

    for raw in output.lines() {
        if problems.len() >= MAX_PROBLEMS {
            break;
        }
        let line = raw.trim_end();
        let trimmed = line.trim_start();

        // rustc: "error[E0308]: mismatched types" then "  --> src/main.rs:12:5".
        if let Some(place) = trimmed.strip_prefix("--> ") {
            let mut parts = place.rsplitn(3, ':');
            let (column, row, file) = (parts.next(), parts.next(), parts.next());
            if let (Some(file), Some(row)) = (file.and_then(|f| known_file(root, f)), row.and_then(number)) {
                let message = rust_message.take().unwrap_or_else(|| "See the output".into());
                problems.push(Problem {
                    path: file,
                    line: row,
                    column: column.and_then(number),
                    severity: severity_of(&message),
                    message,
                });
            }
            continue;
        }
        if trimmed.starts_with("error") || trimmed.starts_with("warning") {
            rust_message = Some(trimmed.to_string());
        }

        // ESLint: a file on a line of its own, then "  12:5  error  message  rule".
        if !line.starts_with(' ') && known_file(root, line).is_some() {
            eslint_file = known_file(root, line);
            continue;
        }
        if let Some(file) = &eslint_file
            && line.starts_with(' ')
            && let Some((place, rest)) = trimmed.split_once(char::is_whitespace)
            && let Some((row, column)) = place.split_once(':')
            && let (Some(row), Some(column)) = (number(row), number(column))
        {
            let rest = rest.trim();
            problems.push(Problem {
                path: file.clone(),
                line: row,
                column: Some(column),
                severity: if rest.starts_with("warn") { "warning" } else { "error" },
                message: rest.trim_start_matches("error").trim_start_matches("warning").trim().to_string(),
            });
            continue;
        }
        if trimmed.is_empty() {
            eslint_file = None;
        }

        // tsc: "src/a.ts(12,5): error TS2322: message".
        if let Some((before, message)) = line.split_once("): ")
            && let Some((file, place)) = before.rsplit_once('(')
            && let Some((row, column)) = place.split_once(',')
            && let (Some(file), Some(row)) = (known_file(root, file), number(row))
        {
            problems.push(Problem {
                path: file,
                line: row,
                column: number(column),
                severity: severity_of(message),
                message: message.trim().to_string(),
            });
            continue;
        }

        // Most tools: "path:line:col: message" or "path:line: message".
        let mut parts = line.splitn(4, ':');
        if let (Some(file), Some(row), Some(third)) = (parts.next(), parts.next(), parts.next())
            && let (Some(file), Some(row)) = (known_file(root, file), number(row))
        {
            let rest = parts.next();
            let (column, message) = match (number(third), rest) {
                (Some(column), Some(rest)) => (Some(column), rest.to_string()),
                _ => (None, [third, rest.unwrap_or("")].join(":").trim_end_matches(':').to_string()),
            };
            let message = message.trim().to_string();
            if !message.is_empty() {
                problems.push(Problem { path: file, line: row, column, severity: severity_of(&message), message });
            }
        }
    }
    problems
}

/// One run of a check: what it printed and how it ended.
struct Run {
    started_at: u64,
    /// The end of what the command printed, both streams together.
    output: Mutex<Vec<u8>>,
    /// The exit code and how long it took, once it has ended. A stopped run has no verdict.
    ended: Mutex<Option<(i32, u64)>>,
    pid: Mutex<Option<u32>>,
    stopped: std::sync::atomic::AtomicBool,
}

/// The runs of every task's checks, the newest of each.
#[derive(Default)]
pub struct Checks {
    runs: Mutex<HashMap<(String, String), Arc<Run>>>,
    /// The core's own settings, which the commands it starts must not inherit.
    scrub: Mutex<Vec<String>>,
    /// When the files of each task last changed, so an older verdict can be told as stale.
    changed_at: Mutex<HashMap<String, u64>>,
}

impl Checks {
    pub fn set_scrub(&self, names: Vec<String>) {
        *self.scrub.lock().unwrap() = names;
    }

    /// Notes that files of a task changed just now.
    pub fn note_change(&self, task_id: &str) {
        self.changed_at.lock().unwrap().insert(task_id.to_string(), now_ms());
    }
}

/// Where a task's checks run, whose they are, and each check with whether it is on.
type Place = (PathBuf, String, Vec<(CheckDef, bool)>);

/// What the user set for a project's checks.
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
struct ProjectChecks {
    /// Checks that are found in the project but turned off.
    #[serde(default)]
    off: Vec<String>,
    /// Commands the user added.
    #[serde(default)]
    custom: Vec<CheckDef>,
}

/// A line only a person can confirm.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Line {
    id: String,
    text: String,
    done: bool,
    /// "issue" or "you".
    from: String,
}

fn project_key(project_id: &str) -> String {
    format!("project.{project_id}.checks")
}

fn lines_key(task_id: &str) -> String {
    format!("task.{task_id}.lines")
}

fn kill_family(pid: u32) {
    #[cfg(unix)]
    // SAFETY: only sends a signal to the group the check was started as the head of.
    unsafe {
        libc::killpg(pid as libc::pid_t, libc::SIGKILL);
    }
    #[cfg(windows)]
    {
        let _ = Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// Lines of an issue written as a task list ("- [ ] …") are what "done" means for it.
fn lines_from_issue(text: &str) -> Vec<String> {
    text.lines()
        .map(str::trim)
        .filter_map(|line| line.strip_prefix("- [ ]").or_else(|| line.strip_prefix("* [ ]")))
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty())
        .take(20)
        .collect()
}

impl Manager {
    fn project_checks(&self, project_id: &str) -> ProjectChecks {
        self.store()
            .setting(&project_key(project_id))
            .ok()
            .flatten()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// The lines a person confirms. The first time they are asked for, the ones the issue lists are added.
    fn lines(&self, task_id: &str) -> Result<Vec<Line>> {
        if let Some(text) = self.store().setting(&lines_key(task_id))? {
            return Ok(serde_json::from_str(&text).unwrap_or_default());
        }
        let task = self.store().task(task_id)?;
        let mut lines = Vec::new();
        if task.issue.is_some() {
            // The issue's text is the first thing that was said in the task.
            let first = self.store().events(task_id)?.into_iter().find(|event| event["type"] == "user_message");
            let text = first.as_ref().and_then(|event| event["text"].as_str()).unwrap_or_default();
            for (index, text) in lines_from_issue(text).into_iter().enumerate() {
                lines.push(Line { id: format!("issue-{}", index + 1), text, done: false, from: "issue".into() });
            }
        }
        self.store().set_setting(&lines_key(task_id), Some(&serde_json::to_string(&lines)?))?;
        Ok(lines)
    }

    fn save_lines(&self, task_id: &str, lines: &[Line]) -> Result<()> {
        self.store().set_setting(&lines_key(task_id), Some(&serde_json::to_string(lines)?))?;
        self.broadcast("checks.changed", json!({ "taskId": task_id }));
        Ok(())
    }

    /// Every check of a project, as found in `root`, each with whether it is on:
    /// the ones found in its files, then the ones the user added.
    pub(crate) fn project_check_defs(&self, project_id: &str, root: &Path) -> Vec<Value> {
        self.checks_in(project_id, root)
            .into_iter()
            .map(|(check, on)| {
                let mut entry = json!(check);
                entry["on"] = json!(on);
                entry
            })
            .collect()
    }

    fn checks_in(&self, project_id: &str, root: &Path) -> Vec<(CheckDef, bool)> {
        let config = self.project_checks(project_id);
        let mut all: Vec<(CheckDef, bool)> = detect(root)
            .into_iter()
            // End-to-end tests are slow: they are there, and off until the user turns them on.
            .map(|check| {
                let on = if check.kind == "e2e" {
                    config.off.contains(&"-e2e".to_string())
                } else {
                    !config.off.contains(&check.id)
                };
                (check, on)
            })
            .collect();
        all.extend(config.custom.into_iter().map(|check| {
            let on = !config.off.contains(&check.id);
            (check, on)
        }));
        all
    }

    /// Every check of a task's project, as found in the task's own working copy.
    fn check_defs(&self, task_id: &str) -> Result<Place> {
        let task = self.store().task(task_id)?;
        let root = PathBuf::from(&task.cwd);
        let all = self.checks_in(&task.project_id, &root);
        Ok((root, task.project_id, all))
    }

    /// "Done when" for a task: every line, how it stands, and how many are done.
    pub fn checks_state(&self, task_id: &str) -> Result<Value> {
        let (root, _, defs) = self.check_defs(task_id)?;
        let changed_at = self.checks.changed_at.lock().unwrap().get(task_id).copied().unwrap_or(0);
        let mut checks = Vec::new();
        let (mut done, mut total, mut failed, mut running) = (0, 0, 0, 0);
        for (check, on) in defs {
            let run = self.checks.runs.lock().unwrap().get(&(task_id.to_string(), check.id.clone())).cloned();
            let mut entry = json!(check);
            entry["on"] = json!(on);
            entry["state"] = json!("none");
            if let Some(run) = run {
                let ended = *run.ended.lock().unwrap();
                let output = String::from_utf8_lossy(&run.output.lock().unwrap()).into_owned();
                let plain = crate::terminals::strip_codes(&output);
                let lines: Vec<&str> = plain.lines().collect();
                entry["tail"] = json!(lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n"));
                entry["at"] = json!(run.started_at);
                match ended {
                    None if run.stopped.load(std::sync::atomic::Ordering::SeqCst) => {}
                    None => entry["state"] = json!("running"),
                    Some((code, ms)) => {
                        entry["state"] = json!(if code == 0 { "passed" } else { "failed" });
                        entry["exitCode"] = json!(code);
                        entry["ms"] = json!(ms);
                        // A verdict from before the files last changed says nothing about them now.
                        entry["stale"] = json!(changed_at > run.started_at);
                        entry["problems"] = json!(if code == 0 { Vec::new() } else { problems_in(&plain, &root) });
                    }
                }
            }
            if on {
                total += 1;
                match entry["state"].as_str() {
                    Some("passed") if entry["stale"] != true => done += 1,
                    Some("failed") if entry["stale"] != true => failed += 1,
                    Some("running") => running += 1,
                    _ => {}
                }
            }
            checks.push(entry);
        }
        let lines = self.lines(task_id)?;
        total += lines.len();
        done += lines.iter().filter(|line| line.done).count();
        Ok(
            json!({ "checks": checks, "lines": lines, "done": done, "total": total, "failed": failed, "running": running }),
        )
    }

    /// Starts one check of a task, or every check that is on when `only` is `None`.
    pub fn checks_run(self: &Arc<Self>, task_id: &str, only: Option<&str>) -> Result<()> {
        let (root, _, defs) = self.check_defs(task_id)?;
        let wanted: Vec<CheckDef> = defs
            .into_iter()
            .filter(|(check, on)| match only {
                Some(id) => check.id == id,
                None => *on,
            })
            .map(|(check, _)| check)
            .collect();
        if wanted.is_empty() {
            bail!("{}", if only.is_some() { "No such check" } else { "This project has no checks that are on" });
        }
        for check in wanted {
            self.start_check(task_id, &root, check)?;
        }
        self.broadcast("checks.changed", json!({ "taskId": task_id }));
        Ok(())
    }

    fn start_check(self: &Arc<Self>, task_id: &str, root: &Path, check: CheckDef) -> Result<()> {
        let key = (task_id.to_string(), check.id.clone());
        if let Some(run) = self.checks.runs.lock().unwrap().get(&key)
            && run.ended.lock().unwrap().is_none()
            && !run.stopped.load(std::sync::atomic::Ordering::SeqCst)
        {
            return Ok(()); // it is running already
        }
        let mut command = if cfg!(windows) {
            let mut command = Command::new(std::env::var("COMSPEC").unwrap_or_else(|_| "cmd.exe".into()));
            command.args(["/C", &check.command]);
            command
        } else {
            // A login shell, so tools from version managers are found as in the user's terminal.
            let shell =
                std::env::var("SHELL").ok().filter(|shell| !shell.is_empty()).unwrap_or_else(|| "/bin/sh".into());
            let mut command = Command::new(shell);
            command.args(["-l", "-c", &check.command]);
            command
        };
        command.current_dir(root).stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
        // Plain text, and a run that ends: with no terminal, test runners do not sit and watch.
        command.env("NO_COLOR", "1").env("FORCE_COLOR", "0");
        for name in self.checks.scrub.lock().unwrap().iter() {
            command.env_remove(name);
        }
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            command.process_group(0);
        }
        let mut child = command.spawn().map_err(|error| anyhow!("Could not start `{}`: {error}", check.command))?;
        let run = Arc::new(Run {
            started_at: now_ms(),
            output: Mutex::new(Vec::new()),
            ended: Mutex::new(None),
            pid: Mutex::new(Some(child.id())),
            stopped: Default::default(),
        });
        self.checks.runs.lock().unwrap().insert(key, run.clone());

        // Both streams go into one text, in the order they are read.
        let mut readers = Vec::new();
        for stream in [
            child.stdout.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
            child.stderr.take().map(|s| Box::new(s) as Box<dyn Read + Send>),
        ]
        .into_iter()
        .flatten()
        {
            let run = run.clone();
            readers.push(std::thread::spawn(move || {
                let mut stream = stream;
                let mut buffer = [0u8; 8192];
                while let Ok(n) = stream.read(&mut buffer) {
                    if n == 0 {
                        break;
                    }
                    let mut output = run.output.lock().unwrap();
                    output.extend_from_slice(&buffer[..n]);
                    let extra = output.len().saturating_sub(KEPT_BYTES);
                    output.drain(..extra);
                }
            }));
        }
        let (manager, task_id) = (self.clone(), task_id.to_string());
        std::thread::spawn(move || {
            let code = child.wait().ok().and_then(|status| status.code()).unwrap_or(1);
            for reader in readers {
                let _ = reader.join();
            }
            *run.pid.lock().unwrap() = None;
            if !run.stopped.load(std::sync::atomic::Ordering::SeqCst) {
                *run.ended.lock().unwrap() = Some((code, now_ms().saturating_sub(run.started_at)));
            }
            manager.broadcast("checks.changed", json!({ "taskId": task_id }));
        });
        Ok(())
    }

    /// Stops a running check. It gets no verdict.
    pub fn check_stop(&self, task_id: &str, id: &str) -> Result<()> {
        let run = self.checks.runs.lock().unwrap().get(&(task_id.to_string(), id.to_string())).cloned();
        if let Some(run) = run {
            run.stopped.store(true, std::sync::atomic::Ordering::SeqCst);
            if let Some(pid) = *run.pid.lock().unwrap() {
                kill_family(pid);
            }
        }
        self.broadcast("checks.changed", json!({ "taskId": task_id }));
        Ok(())
    }

    /// Stops every running check of a task, or of every task. Nothing a check started may outlive it.
    pub fn stop_checks(&self, task_id: Option<&str>) {
        let runs: Vec<Arc<Run>> = self
            .checks
            .runs
            .lock()
            .unwrap()
            .iter()
            .filter(|((task, _), _)| task_id.is_none_or(|id| id == task))
            .map(|(_, run)| run.clone())
            .collect();
        for run in runs {
            run.stopped.store(true, std::sync::atomic::Ordering::SeqCst);
            if let Some(pid) = *run.pid.lock().unwrap() {
                kill_family(pid);
            }
        }
    }

    /// Turns a check of the task's project on or off, for every task of that project.
    pub fn check_turn(&self, task_id: &str, id: &str, on: bool) -> Result<()> {
        let (_, project_id, defs) = self.check_defs(task_id)?;
        self.turn(&project_id, &defs, id, on)?;
        self.broadcast("checks.changed", json!({ "taskId": task_id }));
        Ok(())
    }

    /// The same, asked for a project and not through one of its tasks.
    pub fn project_check_turn(&self, project_id: &str, id: &str, on: bool) -> Result<()> {
        let project = self.store().project(project_id)?;
        let defs = self.checks_in(project_id, Path::new(&project.path));
        self.turn(project_id, &defs, id, on)?;
        // Every open task of the project has one check more or less now.
        for task in self.store().tasks()?.into_iter().filter(|task| task.project_id == project_id) {
            self.broadcast("checks.changed", json!({ "taskId": task.id }));
        }
        Ok(())
    }

    fn turn(&self, project_id: &str, defs: &[(CheckDef, bool)], id: &str, on: bool) -> Result<()> {
        let Some((check, _)) = defs.iter().find(|(check, _)| check.id == id) else { bail!("No such check") };
        let mut config = self.project_checks(project_id);
        // End-to-end tests are off unless turned on, so for them "on" is what is written down.
        let (mark, write) = if check.kind == "e2e" { ("-e2e".to_string(), on) } else { (id.to_string(), !on) };
        config.off.retain(|one| *one != mark);
        if write {
            config.off.push(mark);
        }
        self.store().set_setting(&project_key(project_id), Some(&serde_json::to_string(&config)?))
    }

    /// Adds a command of the user's own to the checks of the task's project.
    pub fn check_add(&self, task_id: &str, title: &str, command: &str) -> Result<()> {
        let (title, command) = (title.trim(), command.trim());
        if command.is_empty() {
            bail!("Give the command to run");
        }
        let (_, project_id, _) = self.check_defs(task_id)?;
        let mut config = self.project_checks(&project_id);
        let id = format!("own-{}", uuid::Uuid::new_v4().simple().to_string().split_at(8).0);
        let title = if title.is_empty() { command } else { title };
        config.custom.push(def(&id, title, command.to_string(), "custom", "you"));
        self.store().set_setting(&project_key(&project_id), Some(&serde_json::to_string(&config)?))?;
        self.broadcast("checks.changed", json!({ "taskId": task_id }));
        Ok(())
    }

    /// Takes a command the user added out of the project's checks.
    pub fn check_remove(&self, task_id: &str, id: &str) -> Result<()> {
        let (_, project_id, _) = self.check_defs(task_id)?;
        let mut config = self.project_checks(&project_id);
        let before = config.custom.len();
        config.custom.retain(|check| check.id != id);
        if config.custom.len() == before {
            bail!("Only a check you added can be removed. One found in the project can be turned off.");
        }
        config.off.retain(|one| one != id);
        self.store().set_setting(&project_key(&project_id), Some(&serde_json::to_string(&config)?))?;
        self.broadcast("checks.changed", json!({ "taskId": task_id }));
        Ok(())
    }

    /// Adds a line only a person can confirm.
    pub fn line_add(&self, task_id: &str, text: &str) -> Result<()> {
        let text = text.trim();
        if text.is_empty() {
            bail!("The line is empty");
        }
        let mut lines = self.lines(task_id)?;
        let id = format!("own-{}", uuid::Uuid::new_v4().simple().to_string().split_at(8).0);
        lines.push(Line { id, text: text.to_string(), done: false, from: "you".into() });
        self.save_lines(task_id, &lines)
    }

    /// Says a line is done, or not done after all.
    pub fn line_set(&self, task_id: &str, id: &str, done: bool) -> Result<()> {
        let mut lines = self.lines(task_id)?;
        let Some(line) = lines.iter_mut().find(|line| line.id == id) else { bail!("No such line") };
        line.done = done;
        self.save_lines(task_id, &lines)
    }

    pub fn line_remove(&self, task_id: &str, id: &str) -> Result<()> {
        let mut lines = self.lines(task_id)?;
        lines.retain(|line| line.id != id);
        self.save_lines(task_id, &lines)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn folder(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ae-checks-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src")).unwrap();
        dir.canonicalize().unwrap()
    }

    #[test]
    fn the_checks_of_a_project_are_read_from_its_own_files() {
        let dir = folder("detect");
        std::fs::write(
            dir.join("package.json"),
            r#"{"scripts":{"lint":"eslint .","typecheck":"tsc --noEmit","test":"vitest run","e2e":"playwright test","dev":"vite"}}"#,
        )
        .unwrap();
        std::fs::write(dir.join("pnpm-lock.yaml"), "").unwrap();
        let found = detect(&dir);
        let brief: Vec<(&str, &str)> = found.iter().map(|c| (c.kind.as_str(), c.command.as_str())).collect();
        assert_eq!(
            brief,
            [
                ("lint", "pnpm run lint"),
                ("types", "pnpm run typecheck"),
                ("test", "pnpm run test"),
                ("e2e", "pnpm run e2e")
            ]
        );
        assert_eq!(found[0].title, "Lint is clean");

        // What `npm init` writes is not a test. A Rust project brings three checks.
        let dir = folder("detect-rust");
        std::fs::write(
            dir.join("package.json"),
            r#"{"scripts":{"test":"echo \"Error: no test specified\" && exit 1"}}"#,
        )
        .unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"x\"\n").unwrap();
        let kinds: Vec<String> = detect(&dir).into_iter().map(|c| format!("{}: {}", c.kind, c.command)).collect();
        assert_eq!(
            kinds,
            [
                "format: cargo fmt --all -- --check",
                "lint: cargo clippy --all-targets -- -D warnings",
                "test: cargo test"
            ]
        );
        assert!(detect(&folder("detect-none")).is_empty());
    }

    #[test]
    fn problems_are_read_out_of_what_the_tools_print() {
        let dir = folder("problems");
        std::fs::write(dir.join("src/a.ts"), "").unwrap();
        std::fs::write(dir.join("src/main.rs"), "").unwrap();
        let absolute = dir.join("src/a.ts").to_string_lossy().into_owned();
        let output = format!(
            "src/a.ts(12,5): error TS2322: Type 'string' is not assignable to type 'number'.\n\
             src/a.ts:3:1: warning: unused import\n\
             src/a.ts:7: something is off here\n\
             {absolute}:9:2: error: absolute paths are understood too\n\
             error[E0308]: mismatched types\n  --> src/main.rs:4:9\n   |\n\
             \n{absolute}\n  1:10  error  'x' is defined but never used  no-unused-vars\n  2:1   warning  Unexpected console statement  no-console\n\n\
             src/missing.ts:1:1: error: a file that is not there is not a problem here\n\
             12:30:45 a time of day is not a file\n"
        );
        let found = problems_in(&output, &dir);
        let brief: Vec<String> = found
            .iter()
            .map(|p| format!("{}:{}:{} {} {}", p.path, p.line, p.column.unwrap_or(0), p.severity, p.message))
            .collect();
        assert_eq!(
            brief,
            [
                "src/a.ts:12:5 error error TS2322: Type 'string' is not assignable to type 'number'.",
                "src/a.ts:3:1 warning warning: unused import",
                "src/a.ts:7:0 error something is off here",
                "src/a.ts:9:2 error error: absolute paths are understood too",
                "src/main.rs:4:9 error error[E0308]: mismatched types",
                "src/a.ts:1:10 error 'x' is defined but never used  no-unused-vars",
                "src/a.ts:2:1 warning Unexpected console statement  no-console",
            ]
        );
    }

    #[test]
    fn a_task_list_in_an_issue_becomes_the_lines_to_confirm() {
        let text = "Login allows unlimited attempts.\n\n- [ ] The sixth attempt in a minute gets 429\n* [ ] The answer has a Retry-After header\n- [x] Already done\n- plain bullet\n";
        assert_eq!(
            lines_from_issue(text),
            ["The sixth attempt in a minute gets 429", "The answer has a Retry-After header"]
        );
    }
}
