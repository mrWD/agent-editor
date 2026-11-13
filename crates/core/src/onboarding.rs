//! The first run: what the editor needs to know to greet someone who opens it
//! for the first time, and to remember that it has.
//!
//! The greeting itself is the interface's. The core keeps one switch (shown
//! already, or not yet), looks for the programs an agent could be made of, and
//! says what a folder that has just become a project holds, so the interface
//! can tell the user what they have now.

use crate::agents::find_program;
use crate::tasks::Manager;
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};
use std::path::Path;
use std::sync::Arc;

/// "yes" once the user has been through the first run, or has put it aside.
const DONE: &str = "onboarding.done";

/// Programs worth knowing about on a first run: command-line agents that can
/// run in a terminal, what serves a local model, and what starts the agents
/// the editor comes with.
const PROGRAMS: [&str; 6] = ["claude", "codex", "gemini", "aider", "ollama", "npx"];

impl Manager {
    /// Whether the first run is behind the user.
    pub fn onboarded(&self) -> bool {
        self.store().setting(DONE).ok().flatten().as_deref() == Some("yes")
    }

    /// Notes that the first run is over, or brings it back. Every window is told.
    pub fn set_onboarded(&self, done: bool) -> Result<()> {
        self.store().set_setting(DONE, done.then_some("yes"))?;
        self.broadcast("onboarding.changed", json!({ "done": done }));
        Ok(())
    }

    /// Where the programs of a first run are on this computer; `null` for one that is not there.
    fn programs_found(&self) -> Value {
        let found: serde_json::Map<String, Value> = PROGRAMS
            .iter()
            .map(|name| {
                let path = find_program(name).map(|path| path.to_string_lossy().into_owned());
                (name.to_string(), json!(path))
            })
            .collect();
        Value::Object(found)
    }

    /// What a project's folder holds, in the words of "this is what you have now".
    fn project_glance(&self, project_id: &str) -> Result<Value> {
        let project = self.store().project(project_id)?;
        let root = Path::new(&project.path);
        if !root.is_dir() {
            bail!("The project's folder is gone: {}", project.path);
        }
        let remote = crate::hosts::remote_of(root);
        Ok(json!({
            "name": project.name,
            "path": project.path,
            // A folder that is a repository gives every task a working copy and a branch of its own.
            "git": root.join(".git").exists(),
            "host": remote.as_ref().map(|remote| remote.kind.name()),
            "server": remote.as_ref().map(|remote| remote.host.clone()),
            "repository": remote.as_ref().map(|remote| remote.slug.clone()),
            "scripts": crate::runner::scripts(root).into_iter().map(|script| script.name).collect::<Vec<_>>(),
            "compose": crate::runner::compose_file(root).is_some(),
            "checks": crate::checks::detect(root).into_iter().map(|check| check.title).collect::<Vec<_>>()
        }))
    }
}

/// The commands of the interface that are about the first run.
pub async fn command(manager: &Arc<Manager>, method: &str, params: &Value) -> Result<Value> {
    match method {
        "onboarding.get" => {
            let manager = manager.clone();
            // Looking for programs reads the disk: not on the thread that answers everyone.
            Ok(tokio::task::spawn_blocking(
                move || json!({ "done": manager.onboarded(), "programs": manager.programs_found() }),
            )
            .await?)
        }
        "onboarding.set" => {
            let done = params.get("done").and_then(Value::as_bool).ok_or_else(|| anyhow!("Say whether it is done"))?;
            manager.set_onboarded(done)?;
            Ok(json!({ "done": done }))
        }
        "onboarding.project" => {
            let id = params
                .get("projectId")
                .and_then(Value::as_str)
                .ok_or_else(|| anyhow!("Missing parameter: projectId"))?
                .to_string();
            let manager = manager.clone();
            tokio::task::spawn_blocking(move || manager.project_glance(&id)).await?
        }
        other => bail!("Unknown method: {other}"),
    }
}
