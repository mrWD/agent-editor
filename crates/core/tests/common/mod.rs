//! What the end-to-end tests share: the practice agent, a sandbox and ways to wait for events.
#![allow(dead_code)]

use ae_core::agents::{AgentDef, Registry};
use ae_core::hosts::Hosts;
use ae_core::store::Store;
use ae_core::tasks::Manager;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, OnceLock};
use std::time::Duration;
use tokio::sync::broadcast::Receiver;

/// Path to the practice agent's binary, building it first if needed.
pub fn mock_agent() -> PathBuf {
    static PATH: OnceLock<PathBuf> = OnceLock::new();
    PATH.get_or_init(|| {
        // The agent is built into <target>/<profile>. The test program is somewhere under that folder, and
        // where exactly changed between versions of cargo (`deps/`, then `build/<package>/<hash>/deps/`),
        // so the folder is looked for by its name on the way up.
        let exe = std::env::current_exe().expect("test binary path");
        let dir = exe
            .ancestors()
            .skip(1)
            .find(|folder| folder.file_name().is_some_and(|name| name == "debug" || name == "release"))
            .expect("profile directory");
        let path = dir.join(if cfg!(windows) { "ae-mock-agent.exe" } else { "ae-mock-agent" });
        if !path.is_file() {
            // Build it into the very folder these tests were built into. That folder is
            // not always `target/`: a coverage run, for one, keeps its own.
            let target = dir.parent().expect("target directory");
            let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
            let mut build = std::process::Command::new(cargo);
            build.args(["build", "-p", "ae-mock-agent", "--target-dir"]).arg(target);
            if dir.file_name().is_some_and(|name| name == "release") {
                build.arg("--release");
            }
            assert!(build.status().expect("cargo runs").success(), "the practice agent builds");
        }
        assert!(path.is_file(), "the practice agent is at {}", path.display());
        path
    })
    .clone()
}

/// The practice agent three ways: as it is, able to pick an earlier session up
/// again ("mock-load"), and offering its settings the older way ("mock-modes").
pub fn registry() -> Registry {
    let practice = |id: &str, switch: Option<&str>| {
        // No pauses between chunks: tests should be fast.
        let mut env = vec![("AE_MOCK_DELAY_MS".to_string(), "0".to_string())];
        env.extend(switch.map(|name| (name.to_string(), "1".to_string())));
        AgentDef {
            id: id.into(),
            name: "Practice agent".into(),
            short: "Pr".into(),
            vendor: "Agent Editor".into(),
            program: mock_agent().to_string_lossy().into_owned(),
            args: vec![],
            env,
            available: true,
            note: None,
            spends_tokens: false,
        }
    };
    // The practice agent behind a launcher, the way `npx` starts real agents: the
    // launcher is the process the core starts, the agent is the launcher's child.
    let mut wrapped = practice("mock-wrapped", None);
    // Beside the agent the launcher starts a helper that outlives a closed input
    // and keeps the output open, as the helpers of real agents do.
    let script = format!("sh -c 'sleep 30; true' \"$0\" & '{}' \"$0\"; true", wrapped.program);
    wrapped.args = vec!["-c".into(), script, wrapped_marker()];
    wrapped.program = "/bin/sh".into();
    // A practice agent that says each turn cost a quarter, to try adding up cost. Nothing is charged.
    let mut paid = practice("mock-paid", None);
    paid.env.push(("AE_MOCK_COST".into(), "0.25".into()));
    paid.name = "Paid practice agent".into();
    paid.spends_tokens = true;
    let mut missing = practice("mock-missing", None);
    missing.available = false;
    missing.note = Some("Install it first".into());
    Registry::with(vec![
        paid,
        missing,
        practice("mock", None),
        practice("mock-load", Some("AE_MOCK_LOAD")),
        practice("mock-modes", Some("AE_MOCK_MODES")),
        wrapped,
    ])
}

/// A word on the command line of the wrapped practice agent, to find its processes by.
pub fn wrapped_marker() -> String {
    format!("ae-wrapped-agent-{}", std::process::id())
}

/// A fresh folder for one test, removed when the value is dropped.
pub struct Sandbox(pub PathBuf);

impl Sandbox {
    pub fn new(name: &str) -> Self {
        static COUNTER: AtomicU32 = AtomicU32::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let dir = std::env::temp_dir().join(format!("ae-test-{name}-{}-{n}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("project")).unwrap();
        std::fs::create_dir_all(dir.join("data")).unwrap();
        Self(dir.canonicalize().unwrap())
    }
    pub fn project(&self) -> PathBuf {
        self.0.join("project")
    }
    pub fn data(&self) -> PathBuf {
        self.0.join("data")
    }
    /// A second way into the same database, for a test that changes what is stored.
    pub fn store(&self) -> Store {
        Store::open(&self.data().join("test.db")).unwrap()
    }
    /// A manager that cannot reach any real service: the tools it would call do not exist.
    pub fn manager(&self) -> Arc<Manager> {
        self.manager_with(no_services())
    }

    /// A manager whose way out to the services is the given stand-ins.
    pub fn manager_with(&self, hosts: Hosts) -> Arc<Manager> {
        let store = Store::open(&self.data().join("test.db")).unwrap();
        Manager::with_hosts(registry(), store, self.data(), hosts)
    }
}

/// Tools that are not there and a Linear address nothing listens on.
/// Tests must never talk to the real GitHub, GitLab, Linear or Docker.
pub fn no_services() -> Hosts {
    let missing = |name: &str| PathBuf::from(format!("/nonexistent/ae-test/{name}"));
    Hosts::with(missing("gh"), missing("glab"), missing("docker"), "http://127.0.0.1:9/graphql".into())
}

/// Writes a shell script that stands in for a command-line tool and returns its path.
#[cfg(unix)]
pub fn fake_tool(dir: &Path, name: &str, script: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{script}")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

impl Drop for Sandbox {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub fn git(dir: &Path, args: &[&str]) -> String {
    let output = std::process::Command::new("git").arg("-C").arg(dir).args(args).output().expect("git runs");
    assert!(output.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&output.stderr));
    String::from_utf8_lossy(&output.stdout).trim().to_string()
}

/// Reads events of one task until one satisfies `done`, and returns everything read.
pub async fn until(events: &mut Receiver<Value>, task_id: &str, done: impl Fn(&Value) -> bool) -> Vec<Value> {
    let mut seen = Vec::new();
    let wait = async {
        loop {
            let message = events.recv().await.expect("event stream stays open");
            if message["event"] != "task.event" || message["data"]["taskId"] != task_id {
                continue;
            }
            let event = message["data"].clone();
            let stop = done(&event);
            seen.push(event);
            if stop {
                break;
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(20), wait)
        .await
        .unwrap_or_else(|_| panic!("timed out; events so far: {seen:#?}"));
    seen
}

pub fn is(event: &Value, kind: &str) -> bool {
    event["type"] == kind
}

pub fn state_is(event: &Value, state: &str) -> bool {
    is(event, "state") && event["state"] == state
}

/// The agent's answer, glued back together from its chunks.
pub fn answer_text(events: &[Value]) -> String {
    events
        .iter()
        .filter(|e| is(e, "update") && e["update"]["sessionUpdate"] == "agent_message_chunk")
        .filter_map(|e| e["update"]["content"]["text"].as_str())
        .collect()
}

/// A project folder that is a git repository with one commit holding the demo file.
pub fn git_project(sandbox: &Sandbox) -> PathBuf {
    let project = sandbox.project();
    std::fs::create_dir_all(project.join("src/middleware")).unwrap();
    std::fs::write(
        project.join("src/middleware/rateLimit.ts"),
        "export const loginLimiter = rateLimit({\n  windowMs: 60 * 1000,\n  limit: 5,\n});\n",
    )
    .unwrap();
    std::fs::write(project.join("README.md"), "# Shop\n").unwrap();
    git(&project, &["init", "-q", "-b", "main"]);
    git(&project, &["add", "."]);
    git(&project, &["-c", "user.name=Test", "-c", "user.email=test@example.com", "commit", "-q", "-m", "Start"]);
    project
}
