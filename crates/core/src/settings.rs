//! Settings: what the user chose for the whole editor, not for one project or task.
//!
//! Everything here is kept in the settings table and holds from the moment it
//! is set. There are four kinds of it:
//!
//! - the agents: a changed command for one the editor comes with, agents of the
//!   user's own, and which agent a new task starts with;
//! - what the user is told about while not looking (see `notify`);
//! - whether the computer is kept awake while an agent works (see `awake`);
//! - the editor: whether a new project starts with formatting on save, and
//!   whether a correction of an agent's line offers to make a rule of it.
//!
//! Formatting on save is a start, not a rule: each project has its own switch,
//! and a project that exists already keeps what it has. The offer of a rule is
//! a switch for everywhere, because a project can only stop the offer for
//! itself and has no way to bring it back.

use crate::acp::{Connection, RpcError};
use crate::agents::{AgentDef, Registry, Speaks, find_program};
use crate::store::Store;
use crate::tasks::Manager;
use anyhow::{Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::BTreeMap;
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

/// The commands the user gave to agents the editor comes with: agent id -> command.
const CHANGED: &str = "agents.changed";
/// The user's own agents, in the order they were added.
const OWN: &str = "agents.own";
/// The agent a new task starts with. Absent: the first one that is installed.
const DEFAULT_AGENT: &str = "agents.default";
/// "off" when a new project starts without formatting on save.
const FORMAT_ON_SAVE: &str = "newProjects.formatOnSave";
/// "off" when a correction of an agent's line offers no rule, in any project.
const RULE_OFFERS: &str = "ruleOffers";

/// How long "Try it" waits for an agent's first answer. The first start of an
/// agent that comes through `npx` downloads it, which takes a while.
/// `AE_TRY_PATIENCE_MS` shortens it for tests.
fn try_patience() -> Duration {
    let ms = std::env::var("AE_TRY_PATIENCE_MS").ok().and_then(|text| text.parse::<u64>().ok()).unwrap_or(120_000);
    Duration::from_millis(ms.max(100))
}

/// A command line that starts an agent, as the user wrote it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Command {
    pub program: String,
    #[serde(default)]
    pub args: Vec<String>,
    /// Extra environment variables: name, then value.
    #[serde(default)]
    pub env: Vec<(String, String)>,
}

impl Command {
    /// The command an agent has now.
    fn of(agent: &AgentDef) -> Self {
        Self { program: agent.program.clone(), args: agent.args.clone(), env: agent.env.clone() }
    }

    /// Reads a command the interface sent, and says in words what is wrong with it.
    pub(crate) fn read(params: &Value) -> Result<Self> {
        let program = params.get("program").and_then(Value::as_str).unwrap_or_default().trim().to_string();
        if program.is_empty() {
            bail!("Give the program that starts the agent");
        }
        let args = match params.get("args") {
            None | Some(Value::Null) => vec![],
            Some(Value::Array(list)) => list
                .iter()
                .map(|arg| arg.as_str().map(str::to_string).ok_or_else(|| anyhow!("Every argument is a piece of text")))
                .collect::<Result<_>>()?,
            Some(_) => bail!("The arguments are a list"),
        };
        let mut env: Vec<(String, String)> = Vec::new();
        for pair in params.get("env").and_then(Value::as_array).into_iter().flatten() {
            let (Some(name), Some(value)) =
                (pair.get(0).and_then(Value::as_str).map(str::trim), pair.get(1).and_then(Value::as_str))
            else {
                bail!("Every environment variable is a name and a value");
            };
            if name.is_empty() || name.contains(['=', ' ', '\t', '\0']) || value.contains('\0') {
                bail!("\"{name}\" cannot be the name of an environment variable");
            }
            if env.iter().any(|(known, _)| known == name) {
                bail!("The environment variable {name} is there twice");
            }
            env.push((name.to_string(), value.to_string()));
        }
        Ok(Self { program, args, env })
    }
}

/// An agent the user added.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct OwnAgent {
    id: String,
    name: String,
    short: String,
    /// True for a program that does not speak the protocol and runs in a terminal.
    #[serde(default)]
    terminal: bool,
    #[serde(flatten)]
    command: Command,
}

impl OwnAgent {
    fn def(&self) -> AgentDef {
        let blank = AgentDef {
            id: self.id.clone(),
            name: self.name.clone(),
            short: self.short.clone(),
            vendor: "Yours".into(),
            program: String::new(),
            args: vec![],
            env: vec![],
            available: false,
            note: None,
            about: None,
            // Nobody knows what an agent of the user's own costs; saying "it may cost" is the safe side.
            spends_tokens: true,
            speaks: if self.terminal { Speaks::Terminal } else { Speaks::Acp },
        };
        with_command(blank, &self.command)
    }
}

/// An agent with another command: whether it is installed is found out anew.
fn with_command(mut agent: AgentDef, command: &Command) -> AgentDef {
    let found = find_program(&command.program);
    agent.available = found.is_some();
    agent.note = (!agent.available).then(|| format!("Its command was not found: {}", command.program));
    // Another command may start something else: what the editor said of its own no longer holds.
    agent.about = None;
    // The full path, where there is one: a bare name does not start everything on every system.
    agent.program = found.map_or_else(|| command.program.clone(), |path| path.to_string_lossy().into_owned());
    agent.args = command.args.clone();
    agent.env = command.env.clone();
    agent
}

fn read_json<T: for<'de> Deserialize<'de> + Default>(store: &Store, key: &str) -> T {
    store.setting(key).ok().flatten().and_then(|text| serde_json::from_str(&text).ok()).unwrap_or_default()
}

/// The agents as they are started: the ones found at start with the commands
/// the user changed, then the user's own.
pub(crate) fn agents_as_set(found: &Registry, store: &Store) -> Registry {
    let changed: BTreeMap<String, Command> = read_json(store, CHANGED);
    let own: Vec<OwnAgent> = read_json(store, OWN);
    let mut agents: Vec<AgentDef> = found
        .list()
        .iter()
        .map(|agent| match changed.get(&agent.id) {
            Some(command) => with_command(agent.clone(), command),
            None => agent.clone(),
        })
        .collect();
    for agent in own {
        if !agents.iter().any(|known| known.id == agent.id) {
            agents.push(agent.def());
        }
    }
    Registry::with(agents)
}

/// A name as an id: small letters, digits and dashes, for it ends up in addresses and style names.
fn slug(name: &str) -> String {
    let mut slug = String::new();
    for letter in name.trim().chars() {
        if letter.is_ascii_alphanumeric() {
            slug.push(letter.to_ascii_lowercase());
        } else if !slug.ends_with('-') && !slug.is_empty() {
            slug.push('-');
        }
    }
    let slug = slug.trim_end_matches('-');
    if slug.is_empty() { "agent".into() } else { slug.chars().take(24).collect() }
}

/// What an agent said about itself when it was tried, in one or two sentences.
fn introduction(agent: &AgentDef, init: &Value) -> Value {
    let info = init.get("agentInfo").cloned().unwrap_or(Value::Null);
    let said = |name: &str| info.get(name).and_then(Value::as_str).map(str::trim).filter(|text| !text.is_empty());
    let name = said("title").or(said("name"));
    let version = said("version");
    let can = |path: &str| init.pointer(path).and_then(Value::as_bool).unwrap_or(false);
    let (continues, pictures, tools) = (
        can("/agentCapabilities/loadSession"),
        can("/agentCapabilities/promptCapabilities/image"),
        can("/agentCapabilities/mcpCapabilities/http"),
    );
    let mut says = match (name, version) {
        (Some(name), Some(version)) => format!("It started and calls itself {name}, version {version}."),
        (Some(name), None) => format!("It started and calls itself {name}."),
        _ => format!("{} started. It did not say what it is called.", agent.name),
    };
    says.push_str(&format!(
        " It {}, {} and {}.",
        if continues {
            "picks its session up again after a restart"
        } else {
            "is told the conversation again after a restart"
        },
        if pictures { "takes pictures" } else { "takes no pictures" },
        if tools { "can use the editor's own tools" } else { "cannot use the editor's own tools" },
    ));
    let protocol = init.get("protocolVersion").and_then(Value::as_u64);
    if let Some(other) = protocol.filter(|version| *version != 1) {
        says.push_str(&format!(" It speaks version {other} of the protocol; the editor speaks version 1."));
    }
    json!({
        "ok": true,
        "name": name,
        "version": version,
        "continues": continues,
        "pictures": pictures,
        "tools": tools,
        "says": says
    })
}

/// Why an agent's program did not even start, in words.
fn why_not_started(agent: &AgentDef, error: &anyhow::Error) -> String {
    let program = &agent.program;
    match error.root_cause().downcast_ref::<std::io::Error>().map(std::io::Error::kind) {
        Some(std::io::ErrorKind::NotFound) => {
            format!("The program \"{program}\" was not found. Check how it is spelled, or give its full path.")
        }
        Some(std::io::ErrorKind::PermissionDenied) => {
            format!("\"{program}\" is there, but it is not a program this user may run.")
        }
        _ => format!("\"{program}\" could not be started: {}.", error.root_cause()),
    }
}

/// Starts an agent, reads how it introduces itself and stops it again. No
/// session is opened and nothing is sent to a model. The answer always comes
/// back as `ok` with what the agent said, or not `ok` with what went wrong.
pub async fn try_agent(agent: &AgentDef, cwd: &Path, patience: Duration) -> Value {
    let failed = |problem: String| json!({ "ok": false, "problem": problem });
    let (conn, mut incoming) = match Connection::spawn(&agent.program, &agent.args, cwd, &agent.env) {
        Ok(started) => started,
        Err(error) => return failed(why_not_started(agent, &error)),
    };
    // The same first message a task's agent gets.
    let hello = conn.request(
        "initialize",
        json!({
            "protocolVersion": 1,
            "clientCapabilities": { "fs": { "readTextFile": false, "writeTextFile": false }, "terminal": false },
            "clientInfo": { "name": "agent-editor", "title": "Agent Editor", "version": env!("CARGO_PKG_VERSION") }
        }),
    );
    let outcome = tokio::time::timeout(patience, hello).await;
    conn.kill().await;
    // What it wrote on the way is often the whole explanation.
    let wrote = format!("{}.", crate::tasks::stderr_tail(&mut incoming));
    let name = &agent.name;
    match outcome {
        Ok(Ok(init)) => introduction(agent, &init),
        Ok(Err(RpcError { code: -32000, .. })) => {
            failed(format!("{name} started and stopped again before it said anything{wrote}"))
        }
        Ok(Err(error)) => failed(format!("{name} started, but refused the first message: {}", error.message)),
        Err(_) => failed(format!(
            "{name} did not answer within {}{wrote} If this was its first start it may still be \
             downloading: try again. Otherwise check that the command speaks the Agent Client Protocol.",
            match patience.as_secs() {
                0 | 1 => "a second".to_string(),
                seconds => format!("{seconds} seconds"),
            }
        )),
    }
}

/// "Try it" for an agent that runs in a terminal. There is nothing to ask it:
/// all that can be said without a task is whether its program is there.
fn try_in_terminal(agent: &AgentDef) -> Value {
    match find_program(&agent.program) {
        Some(path) => json!({
            "ok": true,
            "name": Value::Null,
            "version": Value::Null,
            "continues": false,
            "pictures": false,
            "tools": false,
            "says": format!(
                "{} runs in a terminal: its program was found at {}. In a task the editor starts it in the \
                 task's folder and types your messages into it. Whether it answers is seen there, not here. \
                 It takes no pictures and cannot use the editor's own tools.",
                agent.name,
                path.display()
            )
        }),
        None => json!({
            "ok": false,
            "problem": format!(
                "The program \"{}\" was not found. Check how it is spelled, or give its full path.",
                agent.program
            )
        }),
    }
}

impl Manager {
    fn changed_commands(&self) -> BTreeMap<String, Command> {
        read_json(self.store(), CHANGED)
    }

    fn own_agents(&self) -> Vec<OwnAgent> {
        read_json(self.store(), OWN)
    }

    /// Takes in what was just written about the agents, and tells every window.
    fn agents_changed(&self) {
        *self.agents_now.write().unwrap() = Arc::new(agents_as_set(self.agents_found(), self.store()));
        self.broadcast("settings.changed", json!({ "agents": true }));
    }

    /// The agent the user picked in Settings for new tasks, when it is still there.
    fn chosen_agent(&self) -> Option<String> {
        let id = self.store().setting(DEFAULT_AGENT).ok().flatten()?;
        self.registry().get(&id).map(|agent| agent.id.clone())
    }

    /// The agent a new task starts with: the one picked in Settings; else the agent with a model
    /// behind it that was given a task last. `None` leaves it to the interface, which takes the first
    /// one that is installed: on a new computer that is the practice agent, as it should be. Once a
    /// real agent has been used, the practice agent no longer takes real work by default.
    pub fn default_agent(&self) -> Option<String> {
        self.chosen_agent().or_else(|| self.agent_used_last())
    }

    /// The agent of the newest open task whose agent has a model behind it and is still installed.
    fn agent_used_last(&self) -> Option<String> {
        let registry = self.registry();
        let tasks = self.store().tasks().ok()?;
        tasks
            .iter()
            .rev()
            .filter_map(|task| registry.get(&task.agent_id))
            .find(|agent| agent.available && agent.spends_tokens)
            .map(|agent| agent.id.clone())
    }

    /// The agents as the interface lists them wherever one is picked.
    pub fn agents_listed(&self) -> Value {
        let default = self.default_agent();
        let listed: Vec<Value> = self
            .registry()
            .list()
            .iter()
            .map(|agent| {
                let mut one = json!(agent);
                one["isDefault"] = json!(default.as_deref() == Some(agent.id.as_str()));
                one
            })
            .collect();
        json!(listed)
    }

    /// The agents as Settings shows them: each with its command, and whether that command is the user's.
    fn agents_for_settings(&self) -> Vec<Value> {
        let changed = self.changed_commands();
        let own = self.own_agents();
        // Settings shows what was chosen there, not what a new task falls back to.
        let default = self.chosen_agent();
        let tasks = self.store().tasks().unwrap_or_default();
        self.registry()
            .list()
            .iter()
            .map(|agent| {
                // The command as it was written, not as it was found on the PATH.
                let written = own
                    .iter()
                    .find(|mine| mine.id == agent.id)
                    .map(|mine| mine.command.clone())
                    .or_else(|| changed.get(&agent.id).cloned())
                    .unwrap_or_else(|| Command::of(agent));
                json!({
                    "id": agent.id,
                    "name": agent.name,
                    "short": agent.short,
                    "vendor": agent.vendor,
                    "program": written.program,
                    "args": written.args,
                    "env": written.env,
                    "own": own.iter().any(|mine| mine.id == agent.id),
                    "changed": changed.contains_key(&agent.id),
                    "available": agent.available,
                    "note": agent.note,
                    "about": agent.about,
                    "spendsTokens": agent.spends_tokens,
                    "speaks": agent.speaks,
                    "isDefault": default.as_deref() == Some(agent.id.as_str()),
                    // Open tasks that have this agent: one of the user's own cannot be removed while there are any.
                    "tasks": tasks.iter().filter(|task| task.agent_id == agent.id).count()
                })
            })
            .collect()
    }

    /// Changes the command of an agent, or adds one of the user's own (no `id`).
    /// Returns the agent's id. It holds for the next start of that agent; one
    /// that is running goes on as it was started.
    pub fn agent_save(&self, params: &Value) -> Result<String> {
        let command = Command::read(params)?;
        let text = |name: &str| params.get(name).and_then(Value::as_str).map(str::trim).unwrap_or_default();
        // Only said of an agent of the user's own: the ones the editor comes with speak the protocol.
        let terminal = params.get("terminal").and_then(Value::as_bool).unwrap_or(false);
        let mut own = self.own_agents();

        let id = match params.get("id").and_then(Value::as_str) {
            Some(id) if self.agents_found().get(id).is_some() => {
                // One the editor comes with: only its command is the user's to change.
                let mut changed = self.changed_commands();
                let as_found = self.agents_found().get(id).map(Command::of);
                if as_found.as_ref() == Some(&command) {
                    changed.remove(id);
                } else {
                    changed.insert(id.to_string(), command);
                }
                self.store().set_setting(CHANGED, Some(&serde_json::to_string(&changed)?))?;
                id.to_string()
            }
            given => {
                let (name, short) = (text("name"), text("short"));
                if name.is_empty() {
                    bail!("Give the agent a name");
                }
                if name.chars().count() > 40 {
                    bail!("An agent's name is 40 characters at most");
                }
                if short.is_empty() || short.chars().count() > 2 {
                    bail!("The short name is one or two characters: it goes on the mark next to the agent's lines");
                }
                let taken = self
                    .registry()
                    .list()
                    .iter()
                    .any(|other| Some(other.id.as_str()) != given && other.name.eq_ignore_ascii_case(name));
                if taken {
                    bail!("There is an agent called {name} already");
                }
                let id = match given {
                    Some(id) => {
                        let Some(mine) = own.iter_mut().find(|mine| mine.id == id) else {
                            bail!("No such agent: {id}")
                        };
                        (mine.name, mine.short, mine.command) = (name.to_string(), short.to_string(), command);
                        mine.terminal = terminal;
                        id.to_string()
                    }
                    None => {
                        // An id of its own, whatever else is called alike.
                        let base = format!("own-{}", slug(name));
                        let known = self.registry();
                        let id = (1..)
                            .map(|n| if n == 1 { base.clone() } else { format!("{base}-{n}") })
                            .find(|id| known.get(id).is_none())
                            .unwrap_or(base);
                        own.push(OwnAgent {
                            id: id.clone(),
                            name: name.to_string(),
                            short: short.to_string(),
                            terminal,
                            command,
                        });
                        id
                    }
                };
                self.store().set_setting(OWN, Some(&serde_json::to_string(&own)?))?;
                id
            }
        };
        self.agents_changed();
        Ok(id)
    }

    /// Puts an agent the editor comes with back to the command it comes with.
    pub fn agent_reset(&self, id: &str) -> Result<()> {
        if self.agents_found().get(id).is_none() {
            bail!("Only an agent the editor comes with has a command to go back to");
        }
        let mut changed = self.changed_commands();
        if changed.remove(id).is_some() {
            self.store().set_setting(CHANGED, Some(&serde_json::to_string(&changed)?))?;
            self.agents_changed();
        }
        Ok(())
    }

    /// Removes an agent of the user's own. Open tasks that have it would be left with nobody to wake.
    pub fn agent_remove(&self, id: &str) -> Result<()> {
        let mut own = self.own_agents();
        let Some(place) = own.iter().position(|mine| mine.id == id) else {
            bail!("Only an agent you added yourself can be removed");
        };
        let tasks = self.store().tasks()?.into_iter().filter(|task| task.agent_id == id).count();
        if tasks > 0 {
            bail!(
                "{} has {}. Hand {} to another agent or archive {} first.",
                own[place].name,
                if tasks == 1 { "one open task".to_string() } else { format!("{tasks} open tasks") },
                if tasks == 1 { "it" } else { "them" },
                if tasks == 1 { "it" } else { "them" },
            );
        }
        own.remove(place);
        self.store().set_setting(OWN, Some(&serde_json::to_string(&own)?))?;
        if self.store().setting(DEFAULT_AGENT)?.as_deref() == Some(id) {
            self.store().set_setting(DEFAULT_AGENT, None)?;
        }
        self.agents_changed();
        Ok(())
    }

    /// "Try it": starts the agent as a task would, reads how it introduces itself and stops it.
    pub async fn agent_try(&self, id: &str) -> Result<Value> {
        let agent = self.registry().get(id).cloned().ok_or_else(|| anyhow!("No such agent: {id}"))?;
        if agent.speaks == Speaks::Terminal {
            return Ok(try_in_terminal(&agent));
        }
        Ok(try_agent(&agent, self.data_dir(), try_patience()).await)
    }

    /// Whether a new project starts with its files formatted on save.
    fn new_projects_format(&self) -> bool {
        self.store().setting(FORMAT_ON_SAVE).ok().flatten().as_deref() != Some("off")
    }

    /// Whether rewriting a line an agent wrote offers to make a rule of it at all.
    /// A project can still stop the offer for itself; see `rule_offers`.
    pub(crate) fn rule_offers_wanted(&self) -> bool {
        self.store().setting(RULE_OFFERS).ok().flatten().as_deref() != Some("off")
    }

    /// Gives a project that was just added what Settings say a new project starts with.
    /// It is the project's own from then on, switched where it always was.
    pub(crate) fn start_project_as_set(&self, project_id: &str) {
        if !self.new_projects_format() {
            let _ = self.store().set_setting(&crate::lsp::format_setting(project_id), Some("off"));
        }
    }

    /// Everything the Settings section shows.
    pub fn settings(&self) -> Value {
        json!({
            "agents": self.agents_for_settings(),
            "defaultAgent": self.chosen_agent(),
            "notifications": self.notifications_as_set(),
            "keepAwake": self.keep_awake(),
            "formatOnSave": self.new_projects_format(),
            "ruleOffers": self.rule_offers_wanted()
        })
    }

    /// Changes the settings that are named in `params` and leaves the rest alone.
    pub fn settings_set(&self, params: &Value) -> Result<()> {
        let Some(given) = params.as_object() else { bail!("Say which setting to change") };
        let switch = |name: &str, value: &Value| value.as_bool().ok_or_else(|| anyhow!("{name} is on or off"));
        let mut agents = false;
        for (name, value) in given {
            match name.as_str() {
                "defaultAgent" => {
                    match value.as_str().filter(|id| !id.is_empty()) {
                        Some(id) if self.registry().get(id).is_none() => bail!("No such agent: {id}"),
                        id => self.store().set_setting(DEFAULT_AGENT, id)?,
                    }
                    agents = true;
                }
                "notifications" => self.set_notifications(value)?,
                "keepAwake" => self.set_keep_awake(switch(name, value)?)?,
                "formatOnSave" => {
                    self.store().set_setting(FORMAT_ON_SAVE, (!switch(name, value)?).then_some("off"))?;
                }
                "ruleOffers" => self.store().set_setting(RULE_OFFERS, (!switch(name, value)?).then_some("off"))?,
                other => bail!("There is no setting called {other}"),
            }
        }
        self.broadcast("settings.changed", json!({ "agents": agents }));
        Ok(())
    }
}

/// The commands of the interface that are about settings.
pub async fn command(manager: &Arc<Manager>, method: &str, params: &Value) -> Result<Value> {
    let text =
        |name: &str| params.get(name).and_then(Value::as_str).ok_or_else(|| anyhow!("Missing parameter: {name}"));
    match method {
        "settings.get" => {}
        "settings.set" => manager.settings_set(params)?,
        "agents.save" => {
            let id = manager.agent_save(params)?;
            let mut settings = manager.settings();
            settings["saved"] = json!(id);
            return Ok(settings);
        }
        "agents.reset" => manager.agent_reset(text("id")?)?,
        "agents.remove" => manager.agent_remove(text("id")?)?,
        "agents.try" => return manager.agent_try(text("id")?).await,
        "notifications.attention" => {
            // Which window is speaking is filled in by the connection, not by the interface.
            let window = params.get("window").and_then(Value::as_u64).unwrap_or(0);
            let front = params.get("front").and_then(Value::as_bool).unwrap_or(false);
            manager.window_attention(window, front);
            return Ok(json!({}));
        }
        other => bail!("Unknown method: {other}"),
    }
    Ok(manager.settings())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_becomes_an_id_of_small_letters_digits_and_dashes() {
        assert_eq!(slug("My Agent 2"), "my-agent-2");
        assert_eq!(slug("  ÄÖ!  "), "agent");
        assert_eq!(slug("a__b--c"), "a-b-c");
        assert_eq!(slug(&"x".repeat(60)).len(), 24);
    }

    #[test]
    fn a_command_is_read_and_what_is_wrong_with_it_is_said() {
        let read =
            Command::read(&json!({ "program": " npx ", "args": ["-y", "pkg"], "env": [["KEY", "a b"]] })).unwrap();
        assert_eq!(read.program, "npx");
        assert_eq!(read.args, vec!["-y", "pkg"]);
        assert_eq!(read.env, vec![("KEY".to_string(), "a b".to_string())]);
        assert_eq!(Command::read(&json!({ "program": "agent" })).unwrap().args, Vec::<String>::new());

        let wrong = |params: Value| Command::read(&params).unwrap_err().to_string();
        assert!(wrong(json!({ "program": "  " })).contains("Give the program"));
        assert!(wrong(json!({ "program": "a", "args": [1] })).contains("piece of text"));
        assert!(wrong(json!({ "program": "a", "env": [["A=B", "1"]] })).contains("cannot be the name"));
        assert!(wrong(json!({ "program": "a", "env": [["A", "1"], ["A", "2"]] })).contains("twice"));
        assert!(wrong(json!({ "program": "a", "env": [["A"]] })).contains("a name and a value"));
    }

    #[test]
    fn what_stands_behind_a_known_agent_is_said_until_its_command_is_another() {
        let known = crate::agents::Registry::detect();
        let claude = known.get("claude").expect("Claude is always in the list");
        assert_eq!(
            claude.about.as_deref(),
            Some("Claude Code, behind an adapter that speaks the Agent Client Protocol.")
        );
        assert_eq!(known.get("mock").expect("the practice agent").about, None);

        let command = Command { program: "/nonexistent/ae-test/x".into(), args: vec![], env: vec![] };
        assert_eq!(with_command(claude.clone(), &command).about, None);
    }

    #[test]
    fn an_introduction_says_what_the_agent_called_itself_and_what_it_can_do() {
        let agent = OwnAgent {
            id: "own-x".into(),
            name: "X".into(),
            short: "X".into(),
            terminal: false,
            command: Command { program: "/nonexistent/ae-test/x".into(), args: vec![], env: vec![] },
        }
        .def();
        assert!(!agent.available);
        assert_eq!(agent.note.as_deref(), Some("Its command was not found: /nonexistent/ae-test/x"));

        let full = introduction(
            &agent,
            &json!({
                "protocolVersion": 1,
                "agentInfo": { "name": "x", "title": "Agent X", "version": "2.1" },
                "agentCapabilities": { "loadSession": true, "promptCapabilities": { "image": true } }
            }),
        );
        assert_eq!(
            full["says"],
            "It started and calls itself Agent X, version 2.1. It picks its session up again after a restart, \
             takes pictures and cannot use the editor's own tools."
        );
        assert_eq!((full["ok"].as_bool(), full["tools"].as_bool()), (Some(true), Some(false)));

        let bare = introduction(&agent, &json!({ "protocolVersion": 2 }));
        let says = bare["says"].as_str().unwrap();
        assert!(says.starts_with("X started. It did not say what it is called."), "{says}");
        assert!(says.ends_with("It speaks version 2 of the protocol; the editor speaks version 1."), "{says}");
    }
}
