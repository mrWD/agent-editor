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

/// Programs looked for when the interface names none: a few command-line
/// agents, what serves a local model, and what starts the agents the editor
/// comes with. The interface knows many more and names the ones it wants.
const PROGRAMS: [&str; 6] = ["claude", "codex", "gemini", "aider", "ollama", "npx"];
/// The most programs one question may name. The interface's whole list is a few dozen.
const MOST_PROGRAMS: usize = 200;

/// Whether a word can be the name of a program to look for on the PATH: a bare
/// name, never a path, so the question cannot be made to look anywhere else.
fn is_program_name(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && !name.starts_with(['-', '.'])
        && name.chars().all(|letter| letter.is_ascii_alphanumeric() || matches!(letter, '-' | '_' | '.'))
}

/// The programs a question names, or the usual ones when it names none.
fn programs_asked(params: &Value) -> Result<Vec<String>> {
    let Some(named) = params.get("programs").filter(|named| !named.is_null()) else {
        return Ok(PROGRAMS.iter().map(|name| name.to_string()).collect());
    };
    let Some(list) = named.as_array() else { bail!("The programs to look for are a list of names") };
    if list.len() > MOST_PROGRAMS {
        bail!("That is more programs than one question may name");
    }
    list.iter()
        .map(|name| match name.as_str() {
            Some(name) if is_program_name(name) => Ok(name.to_string()),
            _ => bail!("{name} is not the name of a program"),
        })
        .collect()
}

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

    /// Where each of these programs is on this computer; `null` for one that is not there.
    fn programs_found(&self, wanted: &[String]) -> Value {
        let found: serde_json::Map<String, Value> = wanted
            .iter()
            .map(|name| {
                let path = find_program(name).map(|path| path.to_string_lossy().into_owned());
                (name.clone(), json!(path))
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
            let wanted = programs_asked(params)?;
            let manager = manager.clone();
            // Looking for programs reads the disk: not on the thread that answers everyone.
            Ok(tokio::task::spawn_blocking(
                move || json!({ "done": manager.onboarded(), "programs": manager.programs_found(&wanted) }),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_bare_name_is_looked_for() {
        for name in ["claude", "cursor-agent", "kiro-cli", "vibe_acp", "python3.12"] {
            assert!(is_program_name(name), "{name}");
        }
        // A path would let the question look anywhere on the disk; an option is not a name at all.
        for name in ["", "/bin/sh", "../x", "a b", "-rf", ".hidden", "a;b", "$(x)", &"x".repeat(65)] {
            assert!(!is_program_name(name), "{name}");
        }
    }

    #[test]
    fn a_question_names_its_programs_or_gets_the_usual_ones() {
        assert_eq!(programs_asked(&json!({})).unwrap().len(), PROGRAMS.len());
        assert_eq!(programs_asked(&json!({ "programs": null })).unwrap().len(), PROGRAMS.len());
        assert_eq!(programs_asked(&json!({ "programs": ["qwen", "agy"] })).unwrap(), ["qwen", "agy"]);
        assert!(programs_asked(&json!({ "programs": "qwen" })).is_err());
        assert!(programs_asked(&json!({ "programs": ["ok", "/etc/passwd"] })).is_err());
        assert!(programs_asked(&json!({ "programs": [7] })).is_err());
        let many: Vec<String> = (0..=MOST_PROGRAMS).map(|n| format!("p{n}")).collect();
        assert!(programs_asked(&json!({ "programs": many })).is_err());
    }
}
