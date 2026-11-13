//! Setting the editor up through an agent.
//!
//! Someone opening the editor for the first time can ask Claude or Codex to
//! set it up. The agent reads the guide (`docs/AGENT-SETUP.md`), looks at
//! where things stand, and does the parts that need no secret: adding the
//! user's repositories as projects and picking each project's Linear team.
//! Signing in to `gh`, `glab` and Linear stays with the user.

use crate::hosts::{HostKind, ToolState, remote_of};
use crate::store::Project;
use crate::tasks::Manager;
use anyhow::{Result, anyhow, bail};
use serde_json::Value;
use std::fmt::Write as _;
use std::path::Path;

/// What the agent is told to do, word for word the guide in the repository.
pub const GUIDE: &str = include_str!("../../../docs/AGENT-SETUP.md");

/// The first message of a setup task, followed by what the user said about their team.
pub const SETUP_PROMPT: &str = "Set up this editor for my work. Start with the editor's `setup_guide` tool and follow \
it: look at where things stand with `setup_status`, then do what the guide says. Do not change code, commit or push.";

fn tool_line(name: &str, what: &str, state: &ToolState) -> String {
    match (state.installed, state.signed_in) {
        (false, _) => format!("- `{name}` ({what}): not installed. {}", state.hint.as_deref().unwrap_or("")),
        (true, true) => format!("- `{name}` ({what}): ready."),
        (true, false) => format!("- `{name}` ({what}): installed, not ready. {}", state.hint.as_deref().unwrap_or("")),
    }
}

impl Manager {
    /// A project by its id, its name (any case) or its folder.
    pub fn project_named(&self, wanted: &str) -> Result<Project> {
        let wanted = wanted.trim();
        let projects = self.store().projects()?;
        projects
            .iter()
            .find(|project| project.id == wanted)
            .or_else(|| projects.iter().find(|project| project.name.eq_ignore_ascii_case(wanted)))
            .or_else(|| projects.iter().find(|project| project.path == wanted))
            .cloned()
            .ok_or_else(|| {
                let names: Vec<&str> = projects.iter().map(|project| project.name.as_str()).collect();
                if names.is_empty() {
                    anyhow!("There is no project \"{wanted}\". There are no projects yet: add one first.")
                } else {
                    anyhow!("There is no project \"{wanted}\". The projects are: {}.", names.join(", "))
                }
            })
    }

    /// Where the setup stands, in words an agent reads: the projects with their servers and trackers, the
    /// tools, Linear and the agents. Asks `gh`, `glab`, Docker and Linear, so call it off the async threads.
    pub fn setup_report(&self) -> String {
        let mut out = String::from("# Where the setup stands\n\n## Projects\n");
        let projects = self.store().projects().unwrap_or_default();
        if projects.is_empty() {
            out.push_str("No projects yet. Add the user's repositories with `add_project`.\n");
        }
        for project in &projects {
            let host = match remote_of(Path::new(&project.path)) {
                Some(remote) => format!("{} at {}, {}", remote.kind.name(), remote.host, remote.slug),
                None if Path::new(&project.path).join(".git").exists() => {
                    "no GitHub or GitLab remote the editor recognises".into()
                }
                None => "not a git repository".into(),
            };
            let issues = match self.linear_team(&project.id) {
                Some(team) => format!("Linear team {team}"),
                None if remote_of(Path::new(&project.path)).is_some() => "the code host".into(),
                None => "nowhere".into(),
            };
            let _ = writeln!(out, "- {} ({}): code on {host}; issues come from {issues}.", project.name, project.path);
        }

        let [github, gitlab] = self.code_servers();
        out.push_str("\n## Code hosts\n");
        out.push_str(&tool_line("gh", "GitHub", &self.hosts.tool_state(HostKind::GitHub, &github)));
        out.push('\n');
        out.push_str(&tool_line("glab", "GitLab", &self.hosts.tool_state(HostKind::GitLab, &gitlab)));
        out.push('\n');
        if !gitlab.is_empty() {
            let _ = writeln!(out, "GitLab servers of the projects: {}.", gitlab.join(", "));
        }
        if !github.is_empty() {
            let _ = writeln!(out, "GitHub servers of the projects: {}.", github.join(", "));
        }

        out.push_str("\n## Linear\n");
        match self.linear_key() {
            None => out.push_str(
                "Not connected: there is no key. The user creates a personal API key at \
                 https://linear.app/settings/account/security and pastes it in the Issues screen.\n",
            ),
            Some(key) => match self.hosts.linear_account(&key) {
                Ok(account) => {
                    let text = |pointer: &str| account.pointer(pointer).and_then(Value::as_str).unwrap_or("?");
                    let _ = writeln!(out, "Connected as {} in the workspace {}.", text("/name"), text("/workspace"));
                    let teams: Vec<String> = account["teams"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .map(|team| {
                            format!("{} ({})", team["key"].as_str().unwrap_or("?"), team["name"].as_str().unwrap_or(""))
                        })
                        .collect();
                    let _ =
                        writeln!(out, "Teams: {}.", if teams.is_empty() { "none".into() } else { teams.join(", ") });
                }
                Err(error) => {
                    let _ = writeln!(out, "A key is saved, but Linear did not take it: {error:#}");
                }
            },
        }

        let docker = self.hosts.docker_state();
        out.push_str("\n## Docker\n");
        out.push_str(match (docker.installed, docker.signed_in) {
            (false, _) => "Not installed. Needed only for projects with a compose file.\n",
            (true, false) => "Installed, but its engine is not running.\n",
            (true, true) => "Ready.\n",
        });

        out.push_str("\n## Agents\n");
        for agent in self.registry().list().iter().filter(|agent| agent.spends_tokens) {
            let state = if agent.available { "can start" } else { "cannot start" };
            let _ = writeln!(out, "- {}: {state}. {}", agent.name, agent.note.as_deref().unwrap_or(""));
        }
        out
    }

    /// Sets a project's Linear team for an agent: the team must exist in the connected workspace. An empty
    /// team, or "none", sends the project's issues back to its code host. Returns what was done, in words.
    pub fn setup_linear_team(&self, project: &str, team: &str) -> Result<String> {
        let project = self.project_named(project)?;
        let team = team.trim();
        if team.is_empty() || team.eq_ignore_ascii_case("none") {
            self.set_linear_team(&project.id, None)?;
            return Ok(format!("{}: issues come from its code host again.", project.name));
        }
        let key = self.linear_key().ok_or_else(|| {
            anyhow!("Linear is not connected yet. Ask the user to paste a personal API key in the Issues screen first.")
        })?;
        let account = self.hosts.linear_account(&key)?;
        let teams: Vec<&str> =
            account["teams"].as_array().into_iter().flatten().filter_map(|team| team["key"].as_str()).collect();
        let Some(found) = teams.iter().find(|known| known.eq_ignore_ascii_case(team)) else {
            bail!("The workspace has no team \"{team}\". Its teams are: {}.", teams.join(", "));
        };
        self.set_linear_team(&project.id, Some(found))?;
        Ok(format!("{}: issues now come from Linear team {found}.", project.name))
    }
}
