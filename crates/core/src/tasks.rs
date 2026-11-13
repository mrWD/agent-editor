//! Tasks: the unit of work. A task belongs to a project, has one conversation
//! with an agent, and lives in its own folder.
//!
//! A task is stored; its agent is not. The agent is a process that exists only
//! while the core runs. A task without a running agent is "asleep": everything
//! said so far is kept, and the next message wakes it with a fresh agent that
//! is told what was said before.
//!
//! Everything that happens in a task becomes an event. Events are written to
//! the store and sent to every connected interface at the same time.
//!
//! An agent speaks ACP, and what follows here is about that. One that does
//! not runs in a terminal instead: see `terminal`.

mod terminal;

use crate::acp::{Connection, Incoming, RpcError};
use crate::agents::{AgentDef, Registry, Speaks};
use crate::git;
use crate::hosts::Hosts;
use crate::marks::Author;
use crate::review;
use crate::store::{Project, Store, TaskIssue, TaskRow};
use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::sync::{broadcast, mpsc};

/// How long an agent may take to answer `initialize` and `session/new`.
/// Generous on purpose: the first start of a real agent downloads it.
const START_TIMEOUT: Duration = Duration::from_secs(180);

/// How much of the earlier conversation a newly woken agent is told, in characters.
const RECAP_LIMIT: usize = 4000;

/// Where a task stands right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// The agent finished its turn and waits for the next message.
    Idle,
    /// The agent is working on a message.
    Running,
    /// The agent asked for permission and waits for the answer.
    WaitingPermission,
    /// No agent process. The next message starts one.
    Asleep,
}

/// A task as the interface sees it: what is stored plus its live state.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskInfo {
    #[serde(flatten)]
    pub row: TaskRow,
    pub state: TaskState,
    /// What the running agent lets the user set for this session (mode, model and
    /// the like) and the commands it knows. `None` while no agent is running.
    pub config: Option<Value>,
    /// The race this task is an entrant of, while that race is not decided. See `race`.
    pub race: Option<crate::race::RaceRef>,
}

/// The running agent of one task.
struct Live {
    conn: Arc<Connection>,
    /// The id the agent gave its session.
    acp_session_id: String,
    state: Mutex<TaskState>,
    /// Permission requests waiting for the user: our request id -> the agent's JSON-RPC id.
    permissions: Mutex<HashMap<String, Value>>,
    /// The folder the agent works in.
    cwd: PathBuf,
    /// Files the agent named in its tool calls this turn, relative to `cwd`.
    touched: Mutex<BTreeSet<String>>,
    /// The settings the agent offers for this session, as the agent describes them.
    options: Mutex<Value>,
    /// The commands the agent says it knows, such as its slash commands.
    commands: Mutex<Value>,
    /// True when the agent takes settings through `session/set_config_option`;
    /// older agents only know `session/set_mode`.
    has_config_options: bool,
    /// The project's rules as this session was last told them. They are told
    /// again only when they have changed.
    rules_told: Mutex<Option<String>>,
    /// True when the agent said it takes images in a message.
    takes_images: bool,
}

impl Live {
    /// What the agent offers to set and the commands it knows, for the interface.
    fn config(&self) -> Value {
        json!({
            "options": self.options.lock().unwrap().clone(),
            "commands": self.commands.lock().unwrap().clone(),
            "images": self.takes_images
        })
    }

    /// Takes in an update about settings or commands. Returns false for any other update.
    fn note_config(&self, update: &Value) -> bool {
        match update.get("sessionUpdate").and_then(Value::as_str) {
            Some("config_option_update") => {
                if let Some(options) = update.get("configOptions").filter(|options| options.is_array()) {
                    *self.options.lock().unwrap() = options.clone();
                }
                true
            }
            Some("current_mode_update") => {
                if let Some(mode) = update.get("currentModeId").and_then(Value::as_str) {
                    set_current(&mut self.options.lock().unwrap(), "mode", mode);
                }
                true
            }
            Some("available_commands_update") => {
                *self.commands.lock().unwrap() = update.get("availableCommands").cloned().unwrap_or(json!([]));
                true
            }
            _ => false,
        }
    }
}

/// The settings a session offers, from the agent's answer to opening it. Agents
/// that only know modes get one setting, "mode", made from them.
fn options_of(session: &Value) -> Value {
    if let Some(options) = session.get("configOptions").filter(|options| options.is_array()) {
        return options.clone();
    }
    let Some(modes) = session.pointer("/modes/availableModes").and_then(Value::as_array) else {
        return json!([]);
    };
    let values: Vec<Value> = modes
        .iter()
        .map(
            |mode| json!({ "value": mode.get("id"), "name": mode.get("name"), "description": mode.get("description") }),
        )
        .collect();
    json!([{
        "id": "mode",
        "name": "Mode",
        "category": "mode",
        "type": "select",
        "currentValue": session.pointer("/modes/currentModeId"),
        "options": values
    }])
}

/// Marks `value` as the current one of a setting.
fn set_current(options: &mut Value, option_id: &str, value: &str) {
    for option in options.as_array_mut().into_iter().flatten() {
        if option.get("id").and_then(Value::as_str) == Some(option_id) {
            option["currentValue"] = json!(value);
        }
    }
}

/// Where the user's choice of a setting is kept, per agent.
fn option_setting(agent_id: &str, option_id: &str) -> String {
    format!("agent.{agent_id}.{option_id}")
}

/// Where a task's own choice of a setting is kept.
fn task_setting(task_id: &str, option_id: &str) -> String {
    format!("task.{task_id}.{option_id}")
}

/// True for the setting that says how much the agent may do without asking.
fn is_mode(option: &Value) -> bool {
    option.get("category").and_then(Value::as_str) == Some("mode")
        || option.get("id").and_then(Value::as_str) == Some("mode")
}

/// Owns every task and every running agent.
pub struct Manager {
    /// The agents found at start, with the commands they come with.
    registry: Registry,
    /// The agents as they are started now: the ones above with what the user changed in Settings, and the user's own.
    pub(crate) agents_now: RwLock<Arc<Registry>>,
    /// Who is looking at the editor, and how the user is told when nobody is.
    pub(crate) notifier: crate::notify::Notifier,
    /// The helper that keeps the computer awake while an agent works.
    pub(crate) awake: crate::awake::KeepAwake,
    store: Store,
    data_dir: PathBuf,
    live: RwLock<HashMap<String, Arc<Live>>>,
    /// The agents that run in a terminal because they do not speak the protocol, by task.
    seated: RwLock<HashMap<String, Arc<terminal::Seat>>>,
    /// The terminals such agents run in, lent by the server once it has them.
    terminals: OnceLock<Arc<crate::terminals::Terminals>>,
    /// How long such an agent must print nothing for its turn to be over, in
    /// milliseconds. Zero: as long as the core was started to wait.
    terminal_quiet_ms: std::sync::atomic::AtomicU64,
    /// The number the next event of each task gets.
    seqs: Mutex<HashMap<String, u64>>,
    /// Files the user changed by hand that the task's agent has not been told about yet.
    manual: Mutex<HashMap<String, BTreeSet<String>>>,
    /// Tasks whose user is in the middle of a run of edits by hand: one checkpoint covers the run.
    pub(crate) hand_edits: Mutex<std::collections::HashSet<String>>,
    /// The checks that were run in each task, and how they ended.
    pub checks: crate::checks::Checks,
    /// Where the core's own web server listens, once it does. Agents are given this address for the editor's tools.
    pub(crate) mcp_base: Mutex<Option<String>>,
    /// The keys agents call the editor's tools with, each with the task it was made for.
    pub(crate) mcp_keys: Mutex<HashMap<String, String>>,
    /// The pictures of the message that is on its way to a task's agent.
    pictures: Mutex<HashMap<String, Vec<crate::pictures::Picture>>>,
    /// The way out to GitHub, GitLab, Linear and Docker.
    pub(crate) hosts: Hosts,
    /// What was last read from those services, so screens do not ask them on every look.
    pub(crate) remembered: crate::integrations::Remembered,
    /// What each task's agent is doing for the task's plan right now.
    pub(crate) plans: crate::plan::Plans,
    events: broadcast::Sender<Value>,
}

pub(crate) fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_millis() as u64).unwrap_or(0)
}

/// The first line of a message, cut to a length that fits a list row.
fn title_from(text: &str) -> String {
    let line = text.trim().lines().next().unwrap_or("").trim();
    let mut title: String = line.chars().take(90).collect();
    if line.chars().count() > 90 {
        title.push('…');
    }
    title
}

impl Manager {
    pub fn new(registry: Registry, store: Store, data_dir: PathBuf) -> Arc<Self> {
        Self::with_hosts(registry, store, data_dir, Hosts::detect())
    }

    /// Like `new`, with the way out to GitHub, GitLab, Linear and Docker given. Tests pass stand-ins.
    pub fn with_hosts(registry: Registry, store: Store, data_dir: PathBuf, hosts: Hosts) -> Arc<Self> {
        let (events, _) = broadcast::channel(4096);
        let agents_now = RwLock::new(Arc::new(crate::settings::agents_as_set(&registry, &store)));
        Arc::new(Self {
            registry,
            agents_now,
            notifier: Default::default(),
            awake: Default::default(),
            store,
            data_dir,
            live: RwLock::new(HashMap::new()),
            seated: RwLock::new(HashMap::new()),
            terminals: OnceLock::new(),
            terminal_quiet_ms: Default::default(),
            seqs: Mutex::new(HashMap::new()),
            manual: Mutex::new(HashMap::new()),
            hand_edits: Mutex::new(Default::default()),
            checks: Default::default(),
            mcp_base: Mutex::new(None),
            mcp_keys: Mutex::new(HashMap::new()),
            pictures: Mutex::new(HashMap::new()),
            hosts,
            remembered: Default::default(),
            plans: Default::default(),
            events,
        })
    }

    /// The agents as they are started now. What comes back is the list of this
    /// moment: a change made in Settings afterwards is in the next one asked for.
    pub fn registry(&self) -> Arc<Registry> {
        self.agents_now.read().unwrap().clone()
    }

    /// The agents as they were found at start, before anything the user changed.
    pub(crate) fn agents_found(&self) -> &Registry {
        &self.registry
    }

    pub fn data_dir(&self) -> &Path {
        &self.data_dir
    }

    /// Puts the stored history right after the core was stopped without warning.
    ///
    /// At start no agent is running, so every task is asleep. If the last run
    /// ended abruptly, a task's history may still end with "working" or with a
    /// question nobody can answer any more. This closes those questions and
    /// records that the task is asleep. Call once, before serving.
    pub fn recover(&self) -> Result<()> {
        for row in self.store.tasks()? {
            let events = self.store.events(&row.id)?;
            let mut open: Vec<String> = Vec::new();
            let mut last_state: Option<String> = None;
            for event in &events {
                let request = event.get("requestId").and_then(Value::as_str);
                match (event.get("type").and_then(Value::as_str), request) {
                    (Some("permission_request"), Some(id)) => open.push(id.to_string()),
                    (Some("permission_resolved"), Some(id)) => open.retain(|other| other != id),
                    (Some("state"), _) => {
                        last_state = event.get("state").and_then(Value::as_str).map(str::to_string);
                    }
                    _ => {}
                }
            }
            for request_id in open {
                self.emit(
                    &row.id,
                    json!({ "type": "permission_resolved", "requestId": request_id, "optionId": Value::Null }),
                );
            }
            if last_state.is_some_and(|state| state != "asleep") {
                // The last run ended without a word. The task says so, not only that it sleeps.
                self.emit(
                    &row.id,
                    json!({
                        "type": "tracker",
                        "text": "The editor stopped without warning while this task’s agent was running. Nothing said so far is lost: your next message starts the agent again."
                    }),
                );
                self.emit(&row.id, json!({ "type": "state", "state": TaskState::Asleep }));
            }
        }
        // Deleted files are kept for a while, not for ever.
        self.empty_old_trash();
        // A team that was in the middle of a step is not any more.
        self.settle_teams();
        Ok(())
    }

    /// Live events, already wrapped for the interface.
    pub fn subscribe(&self) -> broadcast::Receiver<Value> {
        self.events.subscribe()
    }

    /// Writes one setting down. For the few places outside this crate's modules that keep a note.
    pub(crate) fn store_setting(&self, key: &str, value: &str) -> Result<()> {
        self.store.set_setting(key, Some(value))
    }

    pub(crate) fn store(&self) -> &Store {
        &self.store
    }

    pub(crate) fn broadcast(&self, event: &str, data: Value) {
        // No receivers is fine: nothing is lost that the store does not have.
        let _ = self.events.send(json!({ "event": event, "data": data }));
    }

    // ----- projects -----

    pub fn projects(&self) -> Result<Vec<Project>> {
        self.store.projects()
    }

    /// Adds a folder as a project. Adding the same folder again returns the project it already is.
    pub fn add_project(&self, path: &str, name: Option<String>) -> Result<Project> {
        let path = path.trim();
        if path.is_empty() {
            bail!("Give the folder of the project");
        }
        let real = crate::paths::real_path(Path::new(path)).with_context(|| format!("No such folder: {path}"))?;
        if !real.is_dir() {
            bail!("Not a folder: {path}");
        }
        let real_text = real.to_string_lossy().into_owned();
        if let Some(existing) = self.store.project_by_path(&real_text)? {
            return Ok(existing);
        }
        let fallback = real.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| real_text.clone());
        let project = Project {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.map(|n| n.trim().to_string()).filter(|n| !n.is_empty()).unwrap_or(fallback),
            path: real_text,
            folder: None,
            created_at: now_ms(),
        };
        self.store.insert_project(&project)?;
        // A new project starts with what Settings say new projects start with.
        self.start_project_as_set(&project.id);
        self.broadcast("projects.changed", json!({}));
        Ok(project)
    }

    /// Puts a project into a folder of projects. The folder is only a name: it is there while a project carries
    /// it, and an empty name takes the project out.
    pub fn set_project_folder(&self, id: &str, folder: Option<&str>) -> Result<Project> {
        let name = folder.map(str::trim).filter(|name| !name.is_empty());
        if name.is_some_and(|name| name.chars().count() > 40) {
            bail!("A folder's name is 40 characters at most.");
        }
        self.store.set_project_folder(id, name)?;
        self.broadcast("projects.changed", json!({}));
        self.store.project(id)
    }

    pub fn remove_project(&self, id: &str) -> Result<()> {
        self.store.remove_project(id)?;
        self.broadcast("projects.changed", json!({}));
        Ok(())
    }

    // ----- reading tasks -----

    pub(crate) fn data_root(&self) -> &Path {
        &self.data_dir
    }

    fn live_of(&self, task_id: &str) -> Option<Arc<Live>> {
        self.live.read().unwrap().get(task_id).cloned()
    }

    pub(crate) fn state_of(&self, task_id: &str) -> TaskState {
        self.live_of(task_id)
            .map(|live| *live.state.lock().unwrap())
            .or_else(|| self.seat_of(task_id).map(|seat| seat.state()))
            .unwrap_or(TaskState::Asleep)
    }

    fn info(&self, row: TaskRow) -> TaskInfo {
        let state = self.state_of(&row.id);
        let config =
            self.live_of(&row.id).map(|live| live.config()).or_else(|| self.seat_of(&row.id).map(|seat| seat.config()));
        let race = self.race_ref(&row.id);
        TaskInfo { row, state, config, race }
    }

    pub fn list(&self) -> Result<Vec<TaskInfo>> {
        Ok(self.store.tasks()?.into_iter().map(|row| self.info(row)).collect())
    }

    pub fn task(&self, task_id: &str) -> Result<TaskInfo> {
        Ok(self.info(self.store.task(task_id)?))
    }

    /// Every event of a task so far, oldest first.
    pub fn history(&self, task_id: &str) -> Result<Vec<Value>> {
        self.store.task(task_id)?;
        self.store.events(task_id)
    }

    /// The folder a task works in.
    pub fn cwd(&self, task_id: &str) -> Result<PathBuf> {
        Ok(PathBuf::from(self.store.task(task_id)?.cwd))
    }

    /// Remembers how far the user has read in a task.
    pub fn seen(&self, task_id: &str, seq: i64) -> Result<()> {
        self.store.set_seen(task_id, seq)
    }

    // ----- events -----

    /// Stores an event and sends it to every connected interface.
    pub(crate) fn emit(&self, task_id: &str, mut event: Value) {
        let seq = {
            let mut seqs = self.seqs.lock().unwrap();
            let next = match seqs.get(task_id) {
                Some(n) => *n,
                None => self.store.next_seq(task_id).unwrap_or(0),
            };
            seqs.insert(task_id.to_string(), next + 1);
            next
        };
        let at = now_ms();
        event["taskId"] = json!(task_id);
        event["seq"] = json!(seq);
        event["at"] = json!(at);
        if let Err(error) = self.store.insert_event(task_id, seq, at, &event) {
            tracing::error!("Could not store an event of task {task_id}: {error:#}");
        }
        // Some events are worth telling a user who is not looking, and say whether an agent is at work.
        self.after_event(task_id, &event);
        self.broadcast("task.event", event);
    }

    fn set_state(&self, task_id: &str, live: &Live, state: TaskState) {
        {
            let mut current = live.state.lock().unwrap();
            if *current == state {
                return;
            }
            *current = state;
        }
        self.emit(task_id, json!({ "type": "state", "state": state }));
    }

    // ----- starting agents -----

    fn agent(&self, agent_id: &str) -> Result<AgentDef> {
        let agent = self.registry().get(agent_id).ok_or_else(|| anyhow!("Unknown agent: {agent_id}"))?.clone();
        if !agent.available {
            bail!("{} is not available: {}", agent.name, agent.note.as_deref().unwrap_or("its command was not found"));
        }
        Ok(agent)
    }

    /// Starts the agent process in `cwd` and opens a session in it. With `resume`,
    /// and an agent that can, the session with that id is picked up again instead
    /// of a new one: the agent then remembers the conversation by itself.
    /// Returns the agent, what it sends, its answer to `initialize`, and whether
    /// the old session was picked up.
    async fn start_agent(
        &self,
        agent: &AgentDef,
        cwd: &Path,
        resume: Option<&str>,
        task_id: &str,
    ) -> Result<(Arc<Live>, mpsc::UnboundedReceiver<Incoming>, Value, bool)> {
        if !cwd.is_dir() {
            bail!("The task's folder is gone: {}", cwd.display());
        }
        let (conn, mut incoming) = Connection::spawn(&agent.program, &agent.args, cwd, &agent.env)?;

        let handshake = async {
            let init = conn
                .request(
                    "initialize",
                    json!({
                        "protocolVersion": 1,
                        "clientCapabilities": {
                            "fs": { "readTextFile": false, "writeTextFile": false },
                            "terminal": false
                        },
                        "clientInfo": {
                            "name": "agent-editor",
                            "title": "Agent Editor",
                            "version": env!("CARGO_PKG_VERSION")
                        }
                    }),
                )
                .await?;
            let can_load = init.pointer("/agentCapabilities/loadSession").and_then(Value::as_bool).unwrap_or(false);
            // The editor's own tools, for an agent that can reach a tool server by address.
            let can_reach =
                init.pointer("/agentCapabilities/mcpCapabilities/http").and_then(Value::as_bool).unwrap_or(false);
            let servers: Vec<Value> =
                if can_reach { self.mcp_server_for(task_id).into_iter().collect() } else { vec![] };
            if let (Some(id), true) = (resume, can_load) {
                // A session the agent no longer has is not an error: a new one is opened below.
                if let Ok(mut loaded) =
                    conn.request("session/load", json!({ "sessionId": id, "cwd": cwd, "mcpServers": servers })).await
                {
                    loaded["sessionId"] = json!(id);
                    return Ok((init, loaded, true));
                }
            }
            let created = conn.request("session/new", json!({ "cwd": cwd, "mcpServers": servers })).await?;
            Ok::<_, RpcError>((init, created, false))
        };

        let (init, session, loaded) = match tokio::time::timeout(START_TIMEOUT, handshake).await {
            Ok(Ok(found)) => found,
            Ok(Err(error)) => {
                conn.kill().await;
                bail!("{} did not start: {error}{}", agent.name, stderr_tail(&mut incoming));
            }
            Err(_) => {
                conn.kill().await;
                bail!("{} did not answer in time{}", agent.name, stderr_tail(&mut incoming));
            }
        };
        let Some(acp_session_id) = session.get("sessionId").and_then(Value::as_str) else {
            conn.kill().await;
            bail!("{} gave no session id", agent.name);
        };
        // While loading, the agent sends the old conversation again. It is in the
        // task's history already, so it is dropped; only what it says about its
        // settings and commands is kept.
        let mut replayed = Vec::new();
        if loaded {
            while let Ok(message) = incoming.try_recv() {
                if let Incoming::Notification { params, .. } = message {
                    replayed.push(params);
                }
            }
        }

        let has_config_options = session.get("configOptions").is_some_and(Value::is_array);
        let live = Arc::new(Live {
            conn,
            acp_session_id: acp_session_id.to_string(),
            state: Mutex::new(TaskState::Idle),
            permissions: Mutex::new(HashMap::new()),
            cwd: cwd.to_path_buf(),
            touched: Mutex::new(BTreeSet::new()),
            options: Mutex::new(options_of(&session)),
            commands: Mutex::new(json!([])),
            has_config_options,
            rules_told: Mutex::new(None),
            takes_images: init
                .pointer("/agentCapabilities/promptCapabilities/image")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        });
        for params in &replayed {
            live.note_config(params.get("update").unwrap_or(&Value::Null));
        }
        Ok((live, incoming, init, loaded))
    }

    /// Sets what the user chose earlier on a session that has just started: the
    /// task's own choices first, then the ones made for this agent in other tasks.
    /// The mode is never carried from one task to another: a task that may edit
    /// without asking must not make every later task do the same. A choice the
    /// agent no longer offers is left alone.
    async fn apply_remembered(&self, live: &Live, agent_id: &str, task_id: &str) {
        let offered: Vec<(String, bool, String, Vec<String>)> = live
            .options
            .lock()
            .unwrap()
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|option| {
                let values = option
                    .get("options")?
                    .as_array()?
                    .iter()
                    .filter_map(|o| o.get("value")?.as_str().map(str::to_string));
                Some((
                    option.get("id")?.as_str()?.to_string(),
                    is_mode(option),
                    option.get("currentValue")?.as_str()?.to_string(),
                    values.collect(),
                ))
            })
            .collect();
        for (option_id, mode, current, values) in offered {
            let own = self.store.setting(&task_setting(task_id, &option_id)).ok().flatten();
            let shared = || self.store.setting(&option_setting(agent_id, &option_id)).ok().flatten();
            let Some(wanted) = own.or_else(|| if mode { None } else { shared() }) else { continue };
            if wanted != current
                && values.contains(&wanted)
                && let Err(error) = self.send_option(live, &option_id, &wanted).await
            {
                tracing::debug!("Could not set {option_id} for {agent_id}: {}", error.message);
            }
        }
    }

    /// Tells the agent to change one setting of its session and keeps what it answers.
    async fn send_option(&self, live: &Live, option_id: &str, value: &str) -> Result<(), RpcError> {
        if live.has_config_options {
            let answer = live
                .conn
                .request(
                    "session/set_config_option",
                    json!({ "sessionId": live.acp_session_id, "configId": option_id, "value": value }),
                )
                .await?;
            if let Some(options) = answer.get("configOptions").filter(|options| options.is_array()) {
                *live.options.lock().unwrap() = options.clone();
                return Ok(());
            }
        } else if option_id == "mode" {
            live.conn.request("session/set_mode", json!({ "sessionId": live.acp_session_id, "modeId": value })).await?;
        } else {
            return Err(RpcError {
                code: -32601,
                message: format!("This agent has no setting called {option_id}"),
                data: None,
            });
        }
        set_current(&mut live.options.lock().unwrap(), option_id, value);
        Ok(())
    }

    /// Changes one setting of a task's agent, such as its model or its mode, and
    /// remembers the choice for the next tasks with the same agent.
    pub async fn set_option(&self, task_id: &str, option_id: &str, value: &str) -> Result<Value> {
        let row = self.store.task(task_id)?;
        let Some(live) = self.live_of(task_id) else {
            bail!("No agent is running in this task. Send a message first; it starts one.");
        };
        let mode = live
            .options
            .lock()
            .unwrap()
            .as_array()
            .into_iter()
            .flatten()
            .any(|option| option.get("id").and_then(Value::as_str) == Some(option_id) && is_mode(option));
        self.send_option(&live, option_id, value).await.map_err(|error| anyhow!("{}", error.message))?;
        self.store.set_setting(&task_setting(task_id, option_id), Some(value))?;
        if !mode {
            self.store.set_setting(&option_setting(&row.agent_id, option_id), Some(value))?;
        }
        let config = live.config();
        self.broadcast("task.config", json!({ "taskId": task_id, "config": config }));
        Ok(config)
    }

    /// Registers a started agent as the task's live agent and begins reading what it sends.
    fn attach(self: &Arc<Self>, task_id: &str, live: Arc<Live>, incoming: mpsc::UnboundedReceiver<Incoming>) {
        self.live.write().unwrap().insert(task_id.to_string(), live.clone());
        // From here on the agent's answers are read in line with its other messages,
        // so the end of a turn is never seen before the last words of that turn.
        live.conn.answers_in_order();
        let manager = self.clone();
        let task_id = task_id.to_string();
        tokio::spawn(async move { manager.pump(task_id, live, incoming).await });
    }

    /// Where a new task of `project` works: its own working copy when the project
    /// is a git repository with commits, the project folder itself otherwise.
    async fn prepare_folder(
        &self,
        project: &Project,
        task_id: &str,
        title: &str,
    ) -> Result<(PathBuf, Option<String>, Option<String>)> {
        let project_path = PathBuf::from(&project.path);
        if !project_path.is_dir() {
            bail!("The project's folder is gone: {}", project.path);
        }
        if !git::is_repo_with_commits(&project_path).await {
            return Ok((project_path, None, None));
        }
        let short = &task_id[..4];
        let slug = git::slug(title);
        let branch = format!("task/{slug}-{short}");
        let dir = self
            .data_dir
            .join("worktrees")
            .join(format!("{}-{}", git::slug(&project.name), &project.id[..4]))
            .join(format!("{slug}-{short}"));
        // The new branch starts from whatever the project has checked out now.
        let base = git::current_branch(&project_path).await;
        git::add_worktree(&project_path, &dir, &branch).await?;
        Ok((dir, Some(branch), base))
    }

    /// Creates a task in a project, starts its agent and sends the first message.
    pub async fn create(self: &Arc<Self>, project_id: &str, agent_id: &str, text: &str) -> Result<TaskInfo> {
        self.create_for(project_id, agent_id, text, None).await
    }

    /// Creates a task, optionally for an issue. The task's title is then the issue's.
    pub async fn create_for(
        self: &Arc<Self>,
        project_id: &str,
        agent_id: &str,
        text: &str,
        issue: Option<TaskIssue>,
    ) -> Result<TaskInfo> {
        if text.trim().is_empty() {
            bail!("Describe the task first");
        }
        let project = self.store.project(project_id)?;
        let agent = self.agent(agent_id)?;
        if agent.speaks == Speaks::Terminal {
            return self.create_seated(project, agent, text, issue).await;
        }
        let id = uuid::Uuid::new_v4().to_string();
        let title = match &issue {
            Some(issue) => title_from(&issue.title),
            None => title_from(text),
        };

        let (cwd, branch, base_branch) = self.prepare_folder(&project, &id, &title).await?;
        let (live, incoming, init, _) = match self.start_agent(&agent, &cwd, None, &id).await {
            Ok(started) => started,
            Err(error) => {
                // Leave nothing behind for a task that never existed.
                if let Some(branch) = &branch {
                    git::remove_worktree(Path::new(&project.path), &cwd, branch).await;
                }
                return Err(error);
            }
        };

        let row = TaskRow {
            id: id.clone(),
            project_id: project.id.clone(),
            title,
            agent_id: agent.id.clone(),
            cwd: cwd.to_string_lossy().into_owned(),
            branch,
            base_branch,
            issue,
            agent_session: Some(live.acp_session_id.clone()),
            created_at: now_ms(),
            seen_seq: -1,
        };
        self.store.insert_task(&row)?;
        self.apply_remembered(&live, &agent.id, &row.id).await;
        self.broadcast(
            "task.created",
            json!(TaskInfo { row: row.clone(), state: TaskState::Idle, config: Some(live.config()), race: None }),
        );
        self.emit(
            &id,
            json!({
                "type": "agent_started",
                "agentId": agent.id,
                "resumed": false,
                "agentInfo": init.get("agentInfo").cloned().unwrap_or(Value::Null)
            }),
        );
        self.attach(&id, live, incoming);
        self.prompt(&id, text.to_string()).await?;
        self.task(&id)
    }

    /// Starts a fresh agent for a task that is asleep.
    async fn wake(self: &Arc<Self>, row: &TaskRow) -> Result<(Arc<Live>, bool)> {
        let agent = self.agent(&row.agent_id)?;
        let (live, incoming, init, loaded) =
            self.start_agent(&agent, Path::new(&row.cwd), row.agent_session.as_deref(), &row.id).await?;
        if row.agent_session.as_deref() != Some(live.acp_session_id.as_str()) {
            self.store.set_agent_session(&row.id, &live.acp_session_id)?;
        }
        self.apply_remembered(&live, &agent.id, &row.id).await;
        self.emit(
            &row.id,
            json!({
                "type": "agent_started",
                "agentId": agent.id,
                "resumed": true,
                // True when the agent picked up its own session and so remembers the conversation itself.
                "loaded": loaded,
                "agentInfo": init.get("agentInfo").cloned().unwrap_or(Value::Null)
            }),
        );
        self.attach(&row.id, live.clone(), incoming);
        self.broadcast("task.config", json!({ "taskId": row.id, "config": live.config() }));
        Ok((live, loaded))
    }

    /// Turns what the agent sends into events until the agent ends.
    async fn pump(self: Arc<Self>, task_id: String, live: Arc<Live>, mut incoming: mpsc::UnboundedReceiver<Incoming>) {
        while let Some(message) = incoming.recv().await {
            match message {
                Incoming::Notification { method, params } if method == "session/update" => {
                    let update = params.get("update").cloned().unwrap_or(Value::Null);
                    // What the agent offers to set is not part of the conversation: it is
                    // kept with the running agent and shown as it stands now.
                    if live.note_config(&update) {
                        self.broadcast("task.config", json!({ "taskId": task_id, "config": live.config() }));
                        continue;
                    }
                    let finished_call = note_touched(&live, &update);
                    self.emit(&task_id, json!({ "type": "update", "update": update }));
                    if finished_call {
                        // The agent just changed something: mark its lines right away.
                        self.look_at_touched(&task_id, &live).await;
                    }
                }
                Incoming::Notification { .. } => {}
                Incoming::Request { id, method, params } if method == "session/request_permission" => {
                    let request_id = uuid::Uuid::new_v4().to_string();
                    live.permissions.lock().unwrap().insert(request_id.clone(), id);
                    self.set_state(&task_id, &live, TaskState::WaitingPermission);
                    self.emit(
                        &task_id,
                        json!({
                            "type": "permission_request",
                            "requestId": request_id,
                            "toolCall": params.get("toolCall").cloned().unwrap_or(Value::Null),
                            "options": params.get("options").cloned().unwrap_or(json!([]))
                        }),
                    );
                }
                Incoming::Request { id, method, .. } => {
                    // File and terminal access for agents is not offered yet.
                    live.conn.respond_error(&id, -32601, &format!("Not supported: {method}"));
                }
                Incoming::Answer { id, outcome } => live.conn.settle(id, outcome),
                Incoming::Stderr(text) => tracing::debug!("agent of {task_id}: {text}"),
                Incoming::Closed => break,
            }
        }

        // The agent is gone. Forget it only if it is still the task's current agent.
        let was_current = {
            let mut live_map = self.live.write().unwrap();
            match live_map.get(&task_id) {
                Some(current) if Arc::ptr_eq(current, &live) => {
                    live_map.remove(&task_id);
                    true
                }
                _ => false,
            }
        };
        if was_current {
            let open: Vec<String> = live.permissions.lock().unwrap().drain().map(|(id, _)| id).collect();
            for request_id in open {
                self.emit(
                    &task_id,
                    json!({ "type": "permission_resolved", "requestId": request_id, "optionId": Value::Null }),
                );
            }
            *live.state.lock().unwrap() = TaskState::Asleep;
            self.emit(&task_id, json!({ "type": "state", "state": TaskState::Asleep }));
        }
    }

    // ----- talking to the agent -----

    /// Sends a message. Returns once the agent has it; the answer arrives as events.
    pub async fn prompt(self: &Arc<Self>, task_id: &str, text: String) -> Result<()> {
        self.prompt_as(task_id, text, None).await
    }

    /// Sends a message with pictures in it. A message of pictures alone needs no words.
    pub async fn prompt_with_pictures(
        self: &Arc<Self>,
        task_id: &str,
        text: String,
        pictures: Vec<crate::pictures::Picture>,
    ) -> Result<()> {
        if pictures.is_empty() {
            return self.prompt(task_id, text).await;
        }
        self.pictures.lock().unwrap().insert(task_id.to_string(), pictures);
        let sent = self.prompt_as(task_id, text, None).await;
        // Whatever happened, the pictures belonged to this message only.
        self.pictures.lock().unwrap().remove(task_id);
        sent
    }

    /// Sends a message to a task's agent. With `shown`, that event is put in
    /// the conversation in place of "the user said": for messages the editor
    /// writes itself, such as the hand-over note between two agents of a team.
    pub(crate) async fn prompt_as(self: &Arc<Self>, task_id: &str, text: String, shown: Option<Value>) -> Result<()> {
        let pictures = self.pictures.lock().unwrap().remove(task_id).unwrap_or_default();
        if text.trim().is_empty() && pictures.is_empty() {
            bail!("The message is empty");
        }
        let row = self.store.task(task_id)?;
        if self.in_terminal(&row) {
            if !pictures.is_empty() {
                let name =
                    self.registry().get(&row.agent_id).map(|agent| agent.name.clone()).unwrap_or(row.agent_id.clone());
                bail!(
                    "{name} runs in a terminal and takes no pictures. Say it in words, or hand the task to an agent that does."
                );
            }
            return self.prompt_seated(row, text, shown, false).await;
        }
        // An agent that was started afresh knows nothing of the task, unless it picked up its old session.
        let (live, needs_recap) = match self.live_of(task_id) {
            Some(live) => (live, false),
            None => {
                let (live, loaded) = self.wake(&row).await?;
                (live, !loaded)
            }
        };
        if *live.state.lock().unwrap() != TaskState::Idle {
            bail!("The agent is still working. Wait for it or stop the turn.");
        }
        if !pictures.is_empty() && !live.takes_images {
            let name =
                self.registry().get(&row.agent_id).map(|agent| agent.name.clone()).unwrap_or(row.agent_id.clone());
            bail!("{name} does not take pictures. Say it in words, or hand the task to an agent that does.");
        }
        let pictures = self.keep_pictures(task_id, &pictures)?;

        // The code as the user leaves it for this turn: a point to look back at and rewind to.
        {
            let (manager, id, said) = (self.clone(), task_id.to_string(), text.clone());
            let _ = tokio::task::spawn_blocking(move || manager.checkpoint_before_message(&id, &said)).await;
        }
        // Whatever changed on disk before this turn is not the agent's doing.
        self.sweep_in_background(task_id, Author::You, vec![]).await;
        live.touched.lock().unwrap().clear();
        let edited: Vec<String> =
            self.manual.lock().unwrap().remove(task_id).map(|set| set.into_iter().collect()).unwrap_or_default();

        // A fresh agent knows nothing of the task: tell it what was said before.
        let mut sent = if needs_recap {
            match recap(&self.store.events(task_id)?) {
                Some(before) => format!(
                    "This task continues an earlier conversation. What was said before:\n\n{before}\n\nNow the user says:\n\n{text}"
                ),
                None => text.clone(),
            }
        } else {
            text.clone()
        };
        if !edited.is_empty() {
            sent = format!("{}\n\n{sent}", manual_edit_note(&edited));
        }
        // What stands for the whole task goes in front: the plan the agent must keep to,
        // and the project's rules, told once to each session and again when they change.
        if let Some(note) = self.plan_note(task_id) {
            sent = format!("{note}\n\n{sent}");
        }
        let rules = self.rules_note(&row.project_id, &row.agent_id);
        {
            let mut told = live.rules_told.lock().unwrap();
            if *told != rules {
                if let Some(note) = &rules {
                    sent = format!("{note}\n\n{sent}");
                }
                *told = rules;
            }
        }

        match shown {
            Some(event) => self.emit(task_id, event),
            None => {
                let mut said = json!({ "type": "user_message", "text": text, "edited": edited });
                if !pictures.is_empty() {
                    said["images"] = pictures.iter().map(crate::pictures::Kept::named).collect();
                }
                self.emit(task_id, said)
            }
        }
        let mut blocks = vec![json!({ "type": "text", "text": sent })];
        blocks.extend(pictures.iter().map(crate::pictures::Kept::block));
        self.set_state(task_id, &live, TaskState::Running);

        let manager = self.clone();
        let task_id = task_id.to_string();
        // Taken as the turn starts, so its end is never mistaken for the end of a later turn with the same words.
        let turn = crate::plan::turn_number();
        tokio::spawn(async move {
            let result = live
                .conn
                .request(
                    "session/prompt",
                    json!({
                        "sessionId": live.acp_session_id,
                        "prompt": blocks
                    }),
                )
                .await;
            let ended_well = result.as_ref().is_ok_and(|answer| answer["stopReason"] == "end_turn");
            match result {
                Ok(answer) => {
                    let stop = answer.get("stopReason").cloned().unwrap_or(Value::Null);
                    manager.emit(&task_id, json!({ "type": "turn_end", "stopReason": stop }));
                }
                Err(error) => {
                    manager.emit(&task_id, json!({ "type": "error", "message": error.message, "code": error.code }));
                }
            }
            // Everything that changed during the turn is the agent's.
            let touched: Vec<String> = live.touched.lock().unwrap().iter().cloned().collect();
            manager.sweep_in_background(&task_id, Author::Agent, touched).await;
            // The plan hears how the turn ended: a block that was being built is built now, or planned again.
            manager.plan_turn_ended(&task_id, &text, turn, ended_well).await;
            if *live.state.lock().unwrap() != TaskState::Asleep {
                manager.set_state(&task_id, &live, TaskState::Idle);
            }
        });
        Ok(())
    }

    /// Runs a sweep off the event loop and waits for it. Failures are logged:
    /// line marks are a convenience and must never stop a turn.
    async fn sweep_in_background(self: &Arc<Self>, task_id: &str, who: Author, also: Vec<String>) {
        let manager = self.clone();
        let id = task_id.to_string();
        let done = tokio::task::spawn_blocking(move || manager.sweep(&id, who, &also)).await;
        if let Ok(Err(error)) = done {
            tracing::debug!("Could not look at the files of {task_id}: {error:#}");
        }
    }

    /// Looks at the files the agent named so far in this turn.
    async fn look_at_touched(self: &Arc<Self>, task_id: &str, live: &Live) {
        let touched: Vec<String> = live.touched.lock().unwrap().iter().cloned().collect();
        if touched.is_empty() {
            return;
        }
        let manager = self.clone();
        let id = task_id.to_string();
        let _ = tokio::task::spawn_blocking(move || {
            let mut changed = Vec::new();
            for path in touched {
                if let Ok(true) = manager.observe(&id, &path, Author::Agent, None) {
                    changed.push(path);
                }
            }
            manager.announce_review(&id, &changed);
        })
        .await;
    }

    /// Records that the user changed a file by hand: it shows in the
    /// conversation, and the agent is told with the next message.
    /// Like `note_manual_edit`, for a change that is told to the conversation in its own words.
    pub(crate) fn note_manual_quietly(&self, task_id: &str, path: &str) {
        self.manual.lock().unwrap().entry(task_id.to_string()).or_default().insert(path.to_string());
    }

    pub fn note_manual_edit(&self, task_id: &str, path: &str) {
        let first_since_last_turn =
            self.manual.lock().unwrap().entry(task_id.to_string()).or_default().insert(path.to_string());
        if first_since_last_turn {
            self.emit(task_id, json!({ "type": "user_edit", "path": path }));
        }
    }

    fn live_or_asleep(&self, task_id: &str) -> Result<Arc<Live>> {
        self.store.task(task_id)?;
        self.live_of(task_id).ok_or_else(|| anyhow!("The agent of this task is not running"))
    }

    /// Answers a permission request. `option_id` of `None` means "cancelled".
    pub fn answer_permission(&self, task_id: &str, request_id: &str, option_id: Option<String>) -> Result<()> {
        let live = self.live_or_asleep(task_id)?;
        let rpc_id = live
            .permissions
            .lock()
            .unwrap()
            .remove(request_id)
            .ok_or_else(|| anyhow!("This question was already answered"))?;
        let outcome = match &option_id {
            Some(option) => json!({ "outcome": "selected", "optionId": option }),
            None => json!({ "outcome": "cancelled" }),
        };
        live.conn.respond(&rpc_id, json!({ "outcome": outcome }));
        self.emit(task_id, json!({ "type": "permission_resolved", "requestId": request_id, "optionId": option_id }));
        if live.permissions.lock().unwrap().is_empty() && *live.state.lock().unwrap() == TaskState::WaitingPermission {
            self.set_state(task_id, &live, TaskState::Running);
        }
        Ok(())
    }

    /// Asks the agent to stop the current turn.
    pub fn cancel(&self, task_id: &str) -> Result<()> {
        if let Some(seat) = self.seat_of(task_id) {
            return self.cancel_seated(&seat);
        }
        let live = self.live_or_asleep(task_id)?;
        // The protocol wants every open permission request answered with "cancelled".
        let open: Vec<(String, Value)> = live.permissions.lock().unwrap().drain().collect();
        for (request_id, rpc_id) in open {
            live.conn.respond(&rpc_id, json!({ "outcome": { "outcome": "cancelled" } }));
            self.emit(
                task_id,
                json!({ "type": "permission_resolved", "requestId": request_id, "optionId": Value::Null }),
            );
        }
        live.conn.notify("session/cancel", json!({ "sessionId": live.acp_session_id }));
        Ok(())
    }

    /// Stops the agent process of a task. The task falls asleep and keeps its conversation.
    pub async fn stop_agent(&self, task_id: &str) -> Result<()> {
        self.store.task(task_id)?;
        if let Some(live) = self.live_of(task_id) {
            live.conn.kill().await;
        }
        if let Some(seat) = self.seat_of(task_id) {
            self.stop_seated(&seat);
        }
        Ok(())
    }

    /// Stops the agent of a task and waits until the task is asleep: everything
    /// the agent's end leaves in the conversation has been written by then.
    pub async fn stop_agent_and_wait(&self, task_id: &str) -> Result<()> {
        self.stop_agent(task_id).await?;
        for _ in 0..200 {
            if self.live_of(task_id).is_none() && self.seat_of(task_id).is_none() {
                return Ok(());
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        bail!("The agent did not stop in time")
    }

    /// Takes a task off every list. Its agent is stopped; its folder and branch stay.
    pub async fn archive(&self, task_id: &str) -> Result<()> {
        self.stop_agent(task_id).await?;
        self.store.archive_task(task_id, now_ms())?;
        self.drop_pictures(task_id);
        self.broadcast("task.archived", json!({ "taskId": task_id }));
        Ok(())
    }

    /// Stops every agent. Called when the core shuts down.
    pub async fn stop_all(&self) {
        // A task whose agent is stopped here says why: otherwise it is just found asleep later.
        let live: Vec<String> = self.live.read().unwrap().keys().cloned().collect();
        let seated: Vec<String> = self.seated.read().unwrap().keys().cloned().collect();
        for task_id in live.iter().chain(seated.iter()) {
            self.emit(
                task_id,
                json!({
                    "type": "tracker",
                    "text": "The editor was closed, so this task’s agent was stopped. Nothing said so far is lost: your next message starts it again."
                }),
            );
        }
        self.stop_checks(None);
        // Nothing of the core's may outlive it, the helper that keeps the computer awake least of all.
        self.awake.stop();
        let all: Vec<_> = self.live.read().unwrap().values().cloned().collect();
        for live in all {
            live.conn.kill().await;
        }
        self.stop_all_seated();
    }
}

/// What the agent is told when the user changed files by hand since its last turn.
fn manual_edit_note(paths: &[String]) -> String {
    let list = paths.iter().map(|p| format!("`{p}`")).collect::<Vec<_>>().join(", ");
    format!(
        "Note from the editor: since your last turn the user changed these files by hand: {list}. \
         What is on disk now is what the user wants. Read these files again before you change them, \
         and keep the user's changes."
    )
}

/// Remembers the files a tool call names. Returns true when the update says a
/// tool call has finished, which is the moment its changes are on disk.
fn note_touched(live: &Live, update: &Value) -> bool {
    let kind = update.get("sessionUpdate").and_then(Value::as_str).unwrap_or_default();
    if kind != "tool_call" && kind != "tool_call_update" {
        return false;
    }
    let mut named: Vec<&str> = Vec::new();
    if let Some(locations) = update.get("locations").and_then(Value::as_array) {
        named.extend(locations.iter().filter_map(|l| l.get("path").and_then(Value::as_str)));
    }
    if let Some(content) = update.get("content").and_then(Value::as_array) {
        named.extend(
            content
                .iter()
                .filter(|c| c.get("type").and_then(Value::as_str) == Some("diff"))
                .filter_map(|c| c.get("path").and_then(Value::as_str)),
        );
    }
    if !named.is_empty() {
        let mut touched = live.touched.lock().unwrap();
        for path in named {
            if let Some(relative) = review::relative_to(&live.cwd, path) {
                touched.insert(relative);
            }
        }
    }
    update.get("status").and_then(Value::as_str) == Some("completed")
}

/// The earlier conversation in plain lines, newest part kept when it is too long.
fn recap(events: &[Value]) -> Option<String> {
    let mut lines: Vec<String> = Vec::new();
    let mut agent = String::new();
    let flush = |agent: &mut String, lines: &mut Vec<String>| {
        if !agent.trim().is_empty() {
            lines.push(format!("Agent: {}", agent.trim()));
        }
        agent.clear();
    };
    for event in events {
        match event.get("type").and_then(Value::as_str) {
            Some("user_message") => {
                flush(&mut agent, &mut lines);
                if let Some(text) = event.get("text").and_then(Value::as_str) {
                    // A picture cannot be told again in words; that there was one can.
                    let shown = match event.get("images").and_then(Value::as_array).map(Vec::len) {
                        Some(1) => "(with a picture the user showed)".to_string(),
                        Some(count) if count > 1 => format!("(with {count} pictures the user showed)"),
                        _ => String::new(),
                    };
                    let said: Vec<&str> =
                        [text.trim(), shown.as_str()].into_iter().filter(|part| !part.is_empty()).collect();
                    lines.push(format!("User: {}", said.join(" ")));
                }
            }
            Some("update") if event["update"]["sessionUpdate"] == "agent_message_chunk" => {
                if let Some(text) = event["update"]["content"]["text"].as_str() {
                    agent.push_str(text);
                }
            }
            // A note an agent left for whoever comes next is exactly what the next agent must hear.
            Some("agent_note") => {
                flush(&mut agent, &mut lines);
                if let Some(text) = event.get("text").and_then(Value::as_str) {
                    let from = event.get("from").and_then(Value::as_str).unwrap_or("an agent");
                    lines.push(format!("Note left by {from}: {}", text.trim()));
                }
            }
            _ => {}
        }
    }
    flush(&mut agent, &mut lines);
    if lines.is_empty() {
        return None;
    }
    let whole = lines.join("\n");
    let count = whole.chars().count();
    if count <= RECAP_LIMIT {
        return Some(whole);
    }
    let tail: String = whole.chars().skip(count - RECAP_LIMIT).collect();
    Some(format!("(earlier part left out)\n…{tail}"))
}

/// The last few stderr lines an agent wrote before it failed, as a readable suffix.
pub(crate) fn stderr_tail(incoming: &mut mpsc::UnboundedReceiver<Incoming>) -> String {
    let mut lines = Vec::new();
    while let Ok(message) = incoming.try_recv() {
        if let Incoming::Stderr(line) = message {
            lines.push(line);
        }
    }
    if lines.is_empty() {
        return String::new();
    }
    let tail: Vec<_> = lines.iter().rev().take(6).rev().cloned().collect();
    format!(". It wrote: {}", tail.join(" | "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recap_joins_chunks_and_keeps_the_order() {
        let events = vec![
            json!({ "type": "user_message", "text": "Add a limit" }),
            json!({ "type": "update", "update": { "sessionUpdate": "agent_message_chunk", "content": { "text": "Done. " } } }),
            json!({ "type": "update", "update": { "sessionUpdate": "tool_call", "title": "Read" } }),
            json!({ "type": "update", "update": { "sessionUpdate": "agent_message_chunk", "content": { "text": "One file." } } }),
            json!({ "type": "user_message", "text": "Thanks" }),
        ];
        assert_eq!(recap(&events).unwrap(), "User: Add a limit\nAgent: Done. One file.\nUser: Thanks");
        assert!(recap(&[]).is_none());
    }

    #[test]
    fn recap_keeps_the_newest_part_when_long() {
        let events: Vec<Value> =
            (0..400).map(|n| json!({ "type": "user_message", "text": format!("message number {n}") })).collect();
        let text = recap(&events).unwrap();
        assert!(text.starts_with("(earlier part left out)"));
        assert!(text.ends_with("message number 399"));
        assert!(text.chars().count() < RECAP_LIMIT + 40);
    }

    #[test]
    fn titles_are_one_short_line() {
        assert_eq!(title_from("  Fix the crash\nand more"), "Fix the crash");
        assert_eq!(title_from(&"a".repeat(200)).chars().count(), 91);
    }
}
