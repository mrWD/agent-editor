//! Rules: what the user wants every agent to keep to in a project.
//!
//! A rule is one plain sentence, such as "Never hardcode limits; read them
//! from config/limits.ts". Rules are kept by the editor, one list per project
//! plus one list for all projects, and are given to an agent with the first
//! message of each session (and again when the list has changed). They are
//! not written into the repository: the same list reaches Claude, Codex and
//! Gemini alike, and nothing in the project's files changes.
//!
//! A rule can be written by hand, or offered after the user rewrites a line
//! an agent wrote: that is the moment a correction is worth keeping.

use crate::tasks::{Manager, now_ms};
use anyhow::{Result, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Rule {
    pub id: String,
    pub text: String,
    /// "you" when written by hand, "edit" when it came from a correction of an agent's line.
    pub from: String,
    pub at: u64,
    /// The agents it is given to. Empty means all.
    #[serde(default)]
    pub agents: Vec<String>,
}

/// The list of all projects is kept under this name in place of a project's id.
pub const ALL_PROJECTS: &str = "all";

fn key(scope: &str) -> String {
    format!("rules.{scope}")
}

impl Manager {
    /// The rules of one project, or of all projects when `scope` is `ALL_PROJECTS`.
    pub fn rules(&self, scope: &str) -> Vec<Rule> {
        self.store()
            .setting(&key(scope))
            .ok()
            .flatten()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    fn save_rules(&self, scope: &str, rules: &[Rule]) -> Result<()> {
        self.store().set_setting(&key(scope), Some(&serde_json::to_string(rules)?))?;
        self.broadcast("rules.changed", json!({ "scope": scope }));
        Ok(())
    }

    /// Adds a rule to a project's list, or to the list of all projects.
    pub fn rule_add(&self, scope: &str, text: &str, from: &str, agents: Vec<String>) -> Result<Rule> {
        let text = text.split_whitespace().collect::<Vec<_>>().join(" ");
        if text.is_empty() {
            bail!("The rule is empty");
        }
        if scope != ALL_PROJECTS {
            self.store().project(scope)?;
        }
        if let Some(unknown) = agents.iter().find(|id| self.registry().get(id).is_none()) {
            bail!("No such agent: {unknown}");
        }
        let mut rules = self.rules(scope);
        if rules.iter().any(|rule| rule.text == text) {
            bail!("That rule is on the list already");
        }
        let rule = Rule {
            id: uuid::Uuid::new_v4().simple().to_string().split_at(8).0.to_string(),
            text,
            from: if from == "edit" { "edit" } else { "you" }.to_string(),
            at: now_ms(),
            agents,
        };
        rules.push(rule.clone());
        self.save_rules(scope, &rules)?;
        Ok(rule)
    }

    pub fn rule_remove(&self, scope: &str, id: &str) -> Result<()> {
        let mut rules = self.rules(scope);
        let before = rules.len();
        rules.retain(|rule| rule.id != id);
        if rules.len() == before {
            bail!("No such rule");
        }
        self.save_rules(scope, &rules)
    }

    /// Every list at once, for the screen that shows them: all projects first, then each project.
    pub fn all_rules(&self) -> Result<Value> {
        let mut lists = vec![json!({ "scope": ALL_PROJECTS, "rules": self.rules(ALL_PROJECTS) })];
        for project in self.store().projects()? {
            lists.push(json!({ "scope": project.id, "rules": self.rules(&project.id) }));
        }
        Ok(json!({ "lists": lists }))
    }

    /// What an agent of a project is told about the rules: one note, or `None` when it has none.
    pub(crate) fn rules_note(&self, project_id: &str, agent_id: &str) -> Option<String> {
        let mut lines: Vec<String> = Vec::new();
        for scope in [ALL_PROJECTS, project_id] {
            for rule in self.rules(scope) {
                if rule.agents.is_empty() || rule.agents.iter().any(|id| id == agent_id) {
                    lines.push(format!("- {}", rule.text));
                }
            }
        }
        if lines.is_empty() {
            return None;
        }
        Some(format!(
            "Note from the editor: the user has set these rules for this project. Keep to them in everything you do here.\n{}",
            lines.join("\n")
        ))
    }

    /// Whether the user wants to be offered a rule after correcting an agent's line in this project.
    pub fn rule_offers(&self, project_id: &str) -> bool {
        self.store().setting(&format!("project.{project_id}.ruleOffers")).ok().flatten().as_deref() != Some("off")
    }

    pub fn set_rule_offers(&self, project_id: &str, on: bool) -> Result<()> {
        self.store().project(project_id)?;
        self.store().set_setting(&format!("project.{project_id}.ruleOffers"), (!on).then_some("off"))
    }
}
