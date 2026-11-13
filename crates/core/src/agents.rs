//! Which agents the core knows how to start.
//!
//! Each agent is a command that speaks ACP on stdio, or, for one that does
//! not, a program the editor runs in a terminal and types into (see
//! `tasks/terminal.rs`). The list is built once at start: for every agent we
//! check whether its command can be found, so the interface can say "not
//! installed" instead of failing later.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// How the editor talks to an agent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Speaks {
    /// The Agent Client Protocol on its input and output.
    #[default]
    Acp,
    /// Nothing the editor understands: it runs in a terminal of the task, and
    /// a message is typed into it the way the user would type it.
    Terminal,
}

/// How to start one agent, and what the interface shows about it.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentDef {
    /// Stable id used in the protocol: "mock", "claude", "codex", "gemini".
    pub id: String,
    pub name: String,
    /// Two letters for the chip next to the agent's lines: "Cl", "Cx", "Gm".
    pub short: String,
    pub vendor: String,
    /// The command and its arguments. Not sent to the interface.
    #[serde(skip)]
    pub program: String,
    #[serde(skip)]
    pub args: Vec<String>,
    /// Extra environment variables for the agent process.
    #[serde(skip)]
    pub env: Vec<(String, String)>,
    /// False when the command was not found on this machine.
    pub available: bool,
    /// Why it is not available, or what the first start will do.
    pub note: Option<String>,
    /// True when using it spends tokens on the user's account.
    pub spends_tokens: bool,
    /// Whether it speaks the protocol or runs in a terminal.
    pub speaks: Speaks,
}

/// The agents found on this machine.
#[derive(Debug, Clone)]
pub struct Registry {
    agents: Vec<AgentDef>,
}

impl Registry {
    /// Looks for every known agent.
    pub fn detect() -> Self {
        let mut agents = vec![Self::mock()];
        agents.extend(Self::mock_twin());
        agents.extend(Self::mock_in_terminal());
        let npx = which::which("npx").ok();
        agents.push(Self::via_npx(
            &npx,
            "claude",
            "Claude",
            "Cl",
            "Anthropic",
            &["@agentclientprotocol/claude-agent-acp"],
        ));
        agents.push(Self::via_npx(&npx, "codex", "Codex", "Cx", "OpenAI", &["@agentclientprotocol/codex-acp"]));
        agents.push(Self::via_npx(&npx, "gemini", "Gemini", "Gm", "Google", &["@google/gemini-cli", "--acp"]));
        Self { agents }
    }

    /// A registry with a fixed list. Used by tests.
    pub fn with(agents: Vec<AgentDef>) -> Self {
        Self { agents }
    }

    pub fn list(&self) -> &[AgentDef] {
        &self.agents
    }

    pub fn get(&self, id: &str) -> Option<&AgentDef> {
        self.agents.iter().find(|a| a.id == id)
    }

    /// The practice agent: built with the core, lives next to it.
    fn mock() -> AgentDef {
        let path = mock_agent_path();
        let available = path.as_ref().is_some_and(|p| p.is_file());
        AgentDef {
            id: "mock".into(),
            name: "Practice agent".into(),
            short: "Pr".into(),
            vendor: "Agent Editor".into(),
            program: path.map(|p| p.to_string_lossy().into_owned()).unwrap_or_default(),
            args: vec![],
            env: vec![],
            available,
            note: (!available).then(|| "Build it with: cargo build -p ae-mock-agent".into()),
            // With `AE_MOCK_COST` set it pretends each turn costs that much, so cost and
            // budgets can be tried without an account. Nothing is ever charged.
            spends_tokens: std::env::var("AE_MOCK_COST")
                .ok()
                .and_then(|v| v.parse::<f64>().ok())
                .is_some_and(|c| c > 0.0),
            speaks: Speaks::Acp,
        }
    }

    /// The practice agent as a program in a terminal, there only when
    /// `AE_MOCK_TERMINAL=1`: the way for agents that do not speak the protocol
    /// can then be tried without installing one.
    fn mock_in_terminal() -> Option<AgentDef> {
        if std::env::var("AE_MOCK_TERMINAL").ok().as_deref() != Some("1") {
            return None;
        }
        let first = Self::mock();
        Some(AgentDef {
            id: "mock-terminal".into(),
            name: "Practice agent in a terminal".into(),
            short: "Pt".into(),
            args: vec!["--terminal".into()],
            speaks: Speaks::Terminal,
            ..first
        })
    }

    /// A second practice agent, there only when `AE_MOCK_TWIN=1`: handing a task
    /// from one agent to another can then be tried without any account.
    fn mock_twin() -> Option<AgentDef> {
        if std::env::var("AE_MOCK_TWIN").ok().as_deref() != Some("1") {
            return None;
        }
        let first = Self::mock();
        Some(AgentDef { id: "mock-2".into(), name: "Second practice agent".into(), short: "P2".into(), ..first })
    }

    fn via_npx(
        npx: &Option<PathBuf>,
        id: &str,
        name: &str,
        short: &str,
        vendor: &str,
        package_args: &[&str],
    ) -> AgentDef {
        let mut args = vec!["-y".to_string()];
        args.extend(package_args.iter().map(|s| s.to_string()));
        AgentDef {
            id: id.into(),
            name: name.into(),
            short: short.into(),
            vendor: vendor.into(),
            // Without npx the command is still named, so the settings can show what would be run.
            program: npx.as_ref().map_or_else(|| "npx".to_string(), |p| p.to_string_lossy().into_owned()),
            args,
            env: vec![],
            available: npx.is_some(),
            note: Some(if npx.is_some() {
                "The first start downloads the agent with npx and may ask you to sign in.".into()
            } else {
                "Needs Node.js: the npx command was not found.".into()
            }),
            spends_tokens: true,
            speaks: Speaks::Acp,
        }
    }
}

/// Where a program is, as the system would find it: a path is taken as it is,
/// a bare name is looked for on the PATH. `None` when there is no such program.
pub fn find_program(program: &str) -> Option<PathBuf> {
    let program = program.trim();
    if program.is_empty() {
        return None;
    }
    which::which(program).ok()
}

/// Where the practice agent's binary should be.
///
/// `AE_MOCK_AGENT` wins; otherwise it is expected next to the core's own binary.
pub fn mock_agent_path() -> Option<PathBuf> {
    if let Ok(path) = std::env::var("AE_MOCK_AGENT") {
        return Some(PathBuf::from(path));
    }
    let exe = std::env::current_exe().ok()?;
    let name = if cfg!(windows) { "ae-mock-agent.exe" } else { "ae-mock-agent" };
    Some(exe.parent()?.join(name))
}
