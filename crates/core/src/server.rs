//! The door between the core and the interface.
//!
//! One WebSocket per interface. Three kinds of JSON messages travel on it:
//!
//! - interface to core, a command: `{ "id": 1, "method": "tasks.create", "params": {...} }`
//! - core to interface, its answer: `{ "id": 1, "result": ... }` or `{ "id": 1, "error": { "message": ... } }`
//! - core to interface, an event nobody asked for: `{ "event": "task.event", "data": {...} }`
//!
//! Terminal output and keystrokes are raw bytes, so they travel as binary
//! messages: one byte that says what it is, the terminal's number as four
//! bytes, then the bytes themselves.

use crate::marks::Author;
use crate::runner::Runner;
use crate::search::Search;
use crate::startup::Desktop;
use crate::tasks::Manager;
use crate::terminals::{TermEvent, Terminals};
use anyhow::{Result, anyhow, bail};
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::http::HeaderValue;
use axum::http::header::{CACHE_CONTROL, CONTENT_TYPE};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::future::Future;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::broadcast::error::RecvError;
use tower_http::services::{ServeDir, ServeFile};

/// How often the core looks at what may be due: requests that merge by
/// themselves, automations. `AE_WATCH_EVERY_MS` shortens it for tests.
fn watch_every() -> std::time::Duration {
    let ms = std::env::var("AE_WATCH_EVERY_MS").ok().and_then(|text| text.parse::<u64>().ok()).unwrap_or(45_000);
    std::time::Duration::from_millis(ms.max(200))
}

/// What every request handler can reach.
#[derive(Clone)]
pub struct AppState {
    pub manager: Arc<Manager>,
    pub terminals: Arc<Terminals>,
    pub search: Arc<Search>,
    pub runner: Arc<Runner>,
    /// What the desktop app lends to the core. `None` when the core runs on its own.
    pub desktop: Option<Desktop>,
}

impl AppState {
    /// Starts looking, now and then, at the requests that were asked to merge
    /// by themselves. Does nothing outside a running core (in a plain unit test).
    pub fn watch_requests(&self) {
        let Ok(runtime) = tokio::runtime::Handle::try_current() else { return };
        let state = self.clone();
        runtime.spawn(async move {
            loop {
                tokio::time::sleep(watch_every()).await;
                state.auto_merge_round().await;
                state.automations_round().await;
            }
        });
    }

    /// Lets the automations do what is due: by the clock, by a new issue, by a
    /// request that was merged on its host.
    pub async fn automations_round(&self) {
        self.manager.automations_tick(crate::tasks::now_ms()).await;
        let manager = self.manager.clone();
        let Ok(clean) = tokio::task::spawn_blocking(move || manager.merged_elsewhere()).await else { return };
        self.clean_up_all(clean).await;
    }

    /// Finishes tasks an automation decided to clean up: stops what they run,
    /// removes the working copy and the branch, archives them.
    async fn clean_up_all(&self, tasks: Vec<String>) {
        for task_id in tasks {
            self.stop_everything(&task_id).await;
            if let Err(error) = self.manager.clean_up(&task_id, true, true).await {
                tracing::debug!("Could not clean up {task_id}: {error:#}");
            }
        }
    }

    /// Merges every request that was asked to merge by itself and can merge
    /// now, with the cleaning up the user chose when asking.
    pub async fn auto_merge_round(&self) {
        let manager = self.manager.clone();
        let Ok(due) = tokio::task::spawn_blocking(move || manager.auto_merge_due()).await else { return };
        for (task_id, mut options) in due {
            options["taskId"] = json!(task_id);
            // The same steps as "Merge now", so a merge is a merge however it was started.
            match dispatch(self, "mr.merge", &options).await {
                Ok(_) => self.manager.auto_merge_done(&task_id),
                Err(error) => self.manager.auto_merge_failed(&task_id, &format!("{error:#}")),
            }
        }
    }

    pub fn new(manager: Arc<Manager>) -> Self {
        Self::listening_on(manager, None)
    }

    /// The state of a core that listens on `port`. What the core was configured
    /// with is kept out of the programs it starts for the user.
    pub fn listening_on(manager: Arc<Manager>, port: Option<u16>) -> Self {
        let mut own: Vec<String> =
            std::env::vars().map(|(name, _)| name).filter(|name| name.starts_with("AE_")).collect();
        // `PORT` is the core's only when it names the core's own port; otherwise it is the user's.
        if port.is_some_and(|port| std::env::var("PORT").ok().as_deref() == Some(port.to_string().as_str())) {
            own.push("PORT".into());
        }
        manager.checks.set_scrub(own.clone());
        let terminals = Arc::new(Terminals::without(own));
        let runner = Arc::new(Runner::new(terminals.clone(), manager.docker_path(), manager.data_dir().to_path_buf()));
        Self { manager, terminals, search: Arc::new(Search::new()), runner, desktop: None }
    }

    /// The desktop app, when the one who asks is the app's own window.
    fn desktop_for(&self, params: &Value) -> Result<&Desktop> {
        let key = params.get("key").and_then(Value::as_str);
        self.desktop
            .as_ref()
            .filter(|desktop| key == Some(desktop.key.as_str()))
            .ok_or_else(|| anyhow!("Only the desktop app can show a folder dialog. Type the path instead."))
    }

    /// The task's project name and folder, which is what running it needs.
    fn run_place(&self, task_id: &str) -> Result<(String, PathBuf)> {
        let task = self.manager.task(task_id)?;
        Ok((self.manager.project_name(&task.row.project_id), PathBuf::from(task.row.cwd)))
    }

    /// Stops what a task runs and removes its containers and volumes. For a task that is finished for good.
    async fn stop_everything(&self, task_id: &str) {
        self.terminals.close_task(task_id);
        self.manager.stop_checks(Some(task_id));
        if let Ok((project, root)) = self.run_place(task_id) {
            let runner = self.runner.clone();
            let id = task_id.to_string();
            let _ = tokio::task::spawn_blocking(move || runner.remove(&id, &project, &root)).await;
        }
    }
}

/// Binary message, core to interface: bytes a terminal printed.
const FRAME_OUTPUT: u8 = 1;
/// Binary message, core to interface: clear the terminal, then show these bytes.
/// Sent when an interface joins a terminal that already has output.
const FRAME_REPLACE: u8 = 2;
/// Binary message, interface to core: bytes the user typed.
const FRAME_INPUT: u8 = 1;

fn frame(kind: u8, term_id: u32, bytes: &[u8]) -> Vec<u8> {
    let mut message = Vec::with_capacity(5 + bytes.len());
    message.push(kind);
    message.extend_from_slice(&term_id.to_be_bytes());
    message.extend_from_slice(bytes);
    message
}

/// The terminals one interface is showing: terminal number -> how many of its
/// bytes this interface already has.
type Attached = HashMap<u32, u64>;

/// Builds the HTTP routes. `ui_dir`, when given, is served as the interface.
pub fn router(state: AppState, ui_dir: Option<PathBuf>) -> Router {
    state.watch_requests();
    let api = Router::new().route("/api/health", get(health)).route("/ws", get(upgrade)).with_state(state);
    match ui_dir {
        Some(dir) => {
            // Unknown paths fall back to index.html so links inside the interface survive a reload.
            let index = ServeFile::new(dir.join("index.html"));
            api.fallback_service(ServeDir::new(dir).fallback(index))
                .layer(axum::middleware::map_response(never_keep_pages))
        }
        None => api,
    }
}

/// Tells the browser not to keep the page that starts the interface.
///
/// That page names the script files of one build. A kept copy of it, shown
/// after the interface was built anew or the app was updated, would ask for
/// files that are gone and leave an empty window. The script files themselves
/// may be kept: a new build gives them new names.
async fn never_keep_pages(mut response: Response) -> Response {
    let is_page = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|kind| kind.to_str().ok())
        .is_some_and(|kind| kind.starts_with("text/html"));
    if is_page {
        response.headers_mut().insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

/// Serves on `listener` until `stop` is done, then stops every terminal and every agent.
pub async fn serve_until(
    listener: tokio::net::TcpListener,
    state: AppState,
    ui_dir: Option<PathBuf>,
    stop: impl Future<Output = ()> + Send + 'static,
) -> Result<()> {
    let manager = state.manager.clone();
    let terminals = state.terminals.clone();
    axum::serve(listener, router(state, ui_dir)).with_graceful_shutdown(stop).await?;
    terminals.close_all();
    manager.stop_all().await;
    Ok(())
}

async fn health() -> Json<Value> {
    Json(json!({ "ok": true, "version": env!("CARGO_PKG_VERSION") }))
}

async fn upgrade(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| connection(socket, state))
}

async fn connection(socket: WebSocket, state: AppState) {
    // Quiet unless asked for (`RUST_LOG=ae_core=debug`): it answers "did the window ever reach the core?".
    tracing::debug!("An interface connected");
    converse(socket, state).await;
    tracing::debug!("An interface disconnected");
}

/// Talks to one interface until it goes away.
async fn converse(socket: WebSocket, state: AppState) {
    let (mut sink, mut stream) = socket.split();
    let mut events = state.manager.subscribe();
    let mut term_events = state.terminals.subscribe();
    let mut attached = Attached::new();
    // Commands run in lanes: one lane per kind of command and task. Inside a
    // lane they run one after another, so their answers come in the order they
    // were asked (the interface counts on "the last answer is the newest").
    // Lanes run side by side: a slow command (git on a large project, a question
    // to GitHub) holds up neither the other lanes nor the events.
    let (answers, mut ready) = tokio::sync::mpsc::unbounded_channel::<Value>();
    let mut lanes: HashMap<String, tokio::sync::mpsc::UnboundedSender<(Value, String, Value)>> = HashMap::new();

    loop {
        tokio::select! {
            incoming = stream.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        let (id, method, params) = match parse(text.as_str()) {
                            Ok(request) => request,
                            Err(refusal) => {
                                if sink.send(Message::Text(refusal.to_string().into())).await.is_err() {
                                    break;
                                }
                                continue;
                            }
                        };
                        if !method.starts_with("terminal.") {
                            let lane = lanes.entry(lane_of(&method, &params)).or_insert_with(|| {
                                let (queue, mut waiting) = tokio::sync::mpsc::unbounded_channel::<(Value, String, Value)>();
                                let (state, answers) = (state.clone(), answers.clone());
                                // The lane ends when this connection does: its queue is dropped with `lanes`.
                                tokio::spawn(async move {
                                    while let Some((id, method, params)) = waiting.recv().await {
                                        let outcome = dispatch(&state, &method, &params).await;
                                        if answers.send(reply(id, outcome)).is_err() {
                                            break;
                                        }
                                    }
                                });
                                queue
                            });
                            let _ = lane.send((id, method, params));
                            continue;
                        }
                        // Terminal commands change which terminals this window is shown,
                        // and are quick: they are answered here, in the order they came.
                        let mut frames = Vec::new();
                        let outcome = terminal_command(&state, &mut attached, &mut frames, &method, &params);
                        let frames = if outcome.is_ok() { frames } else { Vec::new() };
                        if sink.send(Message::Text(reply(id, outcome).to_string().into())).await.is_err() {
                            break;
                        }
                        for bytes in frames {
                            if sink.send(Message::Binary(bytes.into())).await.is_err() {
                                break;
                            }
                        }
                    }
                    Some(Ok(Message::Binary(bytes))) => {
                        // Keystrokes. A terminal that has ended simply ignores them.
                        if bytes.len() > 5 && bytes[0] == FRAME_INPUT {
                            let term_id = u32::from_be_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
                            if attached.contains_key(&term_id) {
                                let _ = state.terminals.input(term_id, &bytes[5..]);
                            }
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => continue,
                    Some(Err(_)) => break,
                }
            }
            Some(answer) = ready.recv() => {
                if sink.send(Message::Text(answer.to_string().into())).await.is_err() {
                    break;
                }
            }
            event = events.recv() => {
                let message = match event {
                    Ok(event) => event,
                    // The interface fell behind and events were dropped: tell it to reload.
                    Err(RecvError::Lagged(_)) => json!({ "event": "resync", "data": {} }),
                    Err(RecvError::Closed) => break,
                };
                if sink.send(Message::Text(message.to_string().into())).await.is_err() {
                    break;
                }
            }
            event = term_events.recv() => {
                let outgoing = match event {
                    Ok(TermEvent::Data { term_id, offset, bytes }) => {
                        let Some(have) = attached.get_mut(&term_id) else { continue };
                        let end = offset + bytes.len() as u64;
                        if end <= *have {
                            continue;
                        }
                        // Part of this piece may already be in what was sent on joining.
                        let skip = have.saturating_sub(offset) as usize;
                        *have = end;
                        vec![Message::Binary(frame(FRAME_OUTPUT, term_id, &bytes[skip..]).into())]
                    }
                    Ok(TermEvent::Exit { term_id, task_id, purpose, code }) => {
                        attached.remove(&term_id);
                        let gone = json!({
                            "event": "terminal.exit",
                            "data": { "termId": term_id, "taskId": task_id, "purpose": purpose, "code": code }
                        });
                        let mut messages = vec![Message::Text(gone.to_string().into())];
                        // A program of the Run tab ended: what the tab shows is no longer true.
                        if purpose != crate::terminals::SHELL {
                            let changed = json!({ "event": "run.changed", "data": { "taskId": task_id } });
                            messages.push(Message::Text(changed.to_string().into()));
                        }
                        messages
                    }
                    // Output was dropped: show each terminal afresh from what the core kept.
                    Err(RecvError::Lagged(_)) => {
                        let mut fresh = Vec::new();
                        for (term_id, have) in attached.iter_mut() {
                            if let Ok((kept, total)) = state.terminals.scrollback(*term_id) {
                                *have = total;
                                fresh.push(Message::Binary(frame(FRAME_REPLACE, *term_id, &kept).into()));
                            }
                        }
                        fresh
                    }
                    Err(RecvError::Closed) => break,
                };
                for message in outgoing {
                    if sink.send(message).await.is_err() {
                        return;
                    }
                }
            }
        }
    }
}

/// Reads one request: its id, the command and the command's parameters.
/// A message that is not a request gives the answer to send back instead.
fn parse(text: &str) -> Result<(Value, String, Value), Value> {
    let Ok(request) = serde_json::from_str::<Value>(text) else {
        return Err(json!({ "id": Value::Null, "error": { "message": "The message is not JSON" } }));
    };
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request.get("method").and_then(Value::as_str).unwrap_or_default().to_string();
    let params = request.get("params").cloned().unwrap_or(json!({}));
    tracing::trace!("Command {method}");
    Ok((id, method, params))
}

/// Which lane a command runs in: its kind (the part before the dot) and its task.
fn lane_of(method: &str, params: &Value) -> String {
    let kind = method.split('.').next().unwrap_or_default();
    // What is asked about requests is one lane for all tasks: the overview of
    // every task's request and a fresh look at one of them must not overtake each other.
    if kind == "mr" {
        return "mr:".to_string();
    }
    let task = params.get("taskId").and_then(Value::as_str).unwrap_or_default();
    format!("{kind}:{task}")
}

/// The answer to a request, as it travels to the interface.
fn reply(id: Value, outcome: Result<Value>) -> Value {
    match outcome {
        Ok(result) => json!({ "id": id, "result": result }),
        Err(error) => json!({ "id": id, "error": { "message": format!("{error:#}") } }),
    }
}

fn number_param(params: &Value, name: &str) -> Result<u64> {
    params.get(name).and_then(Value::as_u64).ok_or_else(|| anyhow!("Missing parameter: {name}"))
}

fn size_params(params: &Value) -> (u16, u16) {
    let side =
        |name: &str, fallback: u64| params.get(name).and_then(Value::as_u64).unwrap_or(fallback).min(1000) as u16;
    (side("cols", 80), side("rows", 24))
}

/// Terminal commands. They are apart from the rest because they change which
/// terminals this one interface is shown.
fn terminal_command(
    state: &AppState,
    attached: &mut Attached,
    frames: &mut Vec<Vec<u8>>,
    method: &str,
    params: &Value,
) -> Result<Value> {
    let terminals = &state.terminals;
    match method {
        "terminal.list" => Ok(json!({ "terminals": terminals.list(text_param(params, "taskId")?) })),
        "terminal.open" => {
            let task_id = text_param(params, "taskId")?;
            let cwd = state.manager.cwd(task_id)?;
            let (cols, rows) = size_params(params);
            let info = terminals.open(task_id, &cwd, cols, rows)?;
            attached.insert(info.term_id, 0);
            Ok(json!(info))
        }
        "terminal.attach" => {
            let term_id = number_param(params, "termId")? as u32;
            let (kept, total) = terminals.scrollback(term_id)?;
            attached.insert(term_id, total);
            frames.push(frame(FRAME_REPLACE, term_id, &kept));
            Ok(json!({}))
        }
        "terminal.detach" => {
            attached.remove(&(number_param(params, "termId")? as u32));
            Ok(json!({}))
        }
        "terminal.resize" => {
            let (cols, rows) = size_params(params);
            terminals.resize(number_param(params, "termId")? as u32, cols, rows)?;
            Ok(json!({}))
        }
        "terminal.close" => {
            terminals.close(number_param(params, "termId")? as u32)?;
            Ok(json!({}))
        }
        other => bail!("Unknown method: {other}"),
    }
}

fn text_param<'a>(params: &'a Value, name: &str) -> Result<&'a str> {
    params.get(name).and_then(Value::as_str).ok_or_else(|| anyhow!("Missing parameter: {name}"))
}

/// Every command the interface can send.
pub async fn dispatch(state: &AppState, method: &str, params: &Value) -> Result<Value> {
    let manager = &state.manager;
    match method {
        "app.info" => Ok(json!({
            "version": env!("CARGO_PKG_VERSION"),
            "platform": std::env::consts::OS,
            "cwd": std::env::current_dir().ok().map(|p| p.to_string_lossy().into_owned()),
            "dataDir": manager.data_dir().to_string_lossy()
        })),
        "agents.list" => Ok(json!(manager.registry().list())),

        "projects.list" => Ok(json!(manager.projects()?)),
        "projects.add" => {
            let name = params.get("name").and_then(Value::as_str).map(str::to_string);
            Ok(json!(manager.add_project(text_param(params, "path")?, name)?))
        }
        "projects.remove" => {
            manager.remove_project(text_param(params, "projectId")?)?;
            Ok(json!({}))
        }

        "tasks.list" => Ok(json!(manager.list()?)),
        "tasks.create" => {
            let task = manager
                .create(text_param(params, "projectId")?, text_param(params, "agentId")?, text_param(params, "text")?)
                .await?;
            Ok(json!(task))
        }
        "tasks.history" => {
            let events = manager.history(text_param(params, "taskId")?)?;
            Ok(json!({ "events": events }))
        }
        "tasks.prompt" => {
            let text = text_param(params, "text")?.to_string();
            manager.prompt(text_param(params, "taskId")?, text).await?;
            Ok(json!({}))
        }
        "tasks.permission" => {
            let option = params.get("optionId").and_then(Value::as_str).map(str::to_string);
            manager.answer_permission(text_param(params, "taskId")?, text_param(params, "requestId")?, option)?;
            Ok(json!({}))
        }
        "tasks.setOption" => {
            let config = manager
                .set_option(
                    text_param(params, "taskId")?,
                    text_param(params, "optionId")?,
                    text_param(params, "value")?,
                )
                .await?;
            Ok(json!({ "config": config }))
        }
        "tasks.cancel" => {
            manager.cancel(text_param(params, "taskId")?)?;
            Ok(json!({}))
        }
        "tasks.stopAgent" => {
            manager.stop_agent(text_param(params, "taskId")?).await?;
            Ok(json!({}))
        }
        "tasks.archive" => {
            let task_id = text_param(params, "taskId")?;
            state.stop_everything(task_id).await;
            manager.archive(task_id).await?;
            Ok(json!({}))
        }
        "tasks.seen" => {
            let seq = params.get("seq").and_then(Value::as_i64).ok_or_else(|| anyhow!("Missing parameter: seq"))?;
            manager.seen(text_param(params, "taskId")?, seq)?;
            Ok(json!({}))
        }

        "files.create" => {
            let task_id = text_param(params, "taskId")?.to_string();
            let path = text_param(params, "path")?.to_string();
            let folder = params.get("folder").and_then(Value::as_bool).unwrap_or(false);
            let manager = manager.clone();
            tokio::task::spawn_blocking(move || manager.file_create(&task_id, &path, folder)).await?
        }
        "files.rename" => {
            let task_id = text_param(params, "taskId")?.to_string();
            let from = text_param(params, "from")?.to_string();
            let to = text_param(params, "to")?.to_string();
            let manager = manager.clone();
            tokio::task::spawn_blocking(move || manager.file_rename(&task_id, &from, &to)).await?
        }
        "files.delete" => {
            let task_id = text_param(params, "taskId")?.to_string();
            let path = text_param(params, "path")?.to_string();
            let manager = manager.clone();
            tokio::task::spawn_blocking(move || manager.file_delete(&task_id, &path)).await?
        }
        "files.restore" => {
            let task_id = text_param(params, "taskId")?.to_string();
            let kept = text_param(params, "keptAs")?.to_string();
            let manager = manager.clone();
            tokio::task::spawn_blocking(move || manager.file_restore(&task_id, &kept)).await?
        }
        "files.list" => {
            let root = manager.cwd(text_param(params, "taskId")?)?;
            let path = params.get("path").and_then(Value::as_str).unwrap_or("").to_string();
            // Disk access must not stall the event loop.
            let entries = tokio::task::spawn_blocking(move || crate::files::list(&root, &path)).await??;
            Ok(json!({ "entries": entries }))
        }
        "files.read" => {
            let task_id = text_param(params, "taskId")?.to_string();
            let path = text_param(params, "path")?.to_string();
            let root = manager.cwd(&task_id)?;
            let manager = manager.clone();
            let (file, review) = tokio::task::spawn_blocking(move || {
                let file = crate::files::read_text(&root, &path)?;
                // Whatever changed on disk since the last look is credited before the file is shown.
                let _ = manager.observe(&task_id, &file.path, manager.acting_now(&task_id), None);
                let review = manager.review_of(&task_id, &file.path)?;
                anyhow::Ok((file, review))
            })
            .await??;
            Ok(json!({ "file": file, "review": review }))
        }
        "files.write" => {
            let task_id = text_param(params, "taskId")?.to_string();
            let path = text_param(params, "path")?.to_string();
            let text = text_param(params, "text")?.to_string();
            let version = params.get("version").and_then(Value::as_str).map(str::to_string);
            let root = manager.cwd(&task_id)?;
            let manager = manager.clone();
            let (file, review) = tokio::task::spawn_blocking(move || {
                let path = crate::review::relative_to(&root, &path)
                    .ok_or_else(|| anyhow!("That path is outside the task's folder"))?;
                // The code before a run of edits by hand: a point to look back at.
                manager.checkpoint_before_edit(&task_id);
                // What is on disk right before the save, so the save's own lines can be told apart.
                let before = std::fs::read(root.join(&path)).ok().map(String::from_utf8);
                let _ = manager.observe(&task_id, &path, manager.acting_now(&task_id), None);
                let file = crate::files::write_text(&root, &path, &text, version.as_deref())?;
                let before = match before {
                    Some(Ok(text)) => Some(Some(text)),
                    None => Some(None),
                    // The file was not text before; there is nothing to compare line by line.
                    Some(Err(_)) => None,
                };
                if let Ok(true) = manager.observe(&task_id, &file.path, Author::You, before) {
                    manager.announce_review(&task_id, std::slice::from_ref(&file.path));
                }
                manager.note_manual_edit(&task_id, &file.path);
                let review = manager.review_of(&task_id, &file.path)?;
                anyhow::Ok((file, review))
            })
            .await??;
            Ok(json!({ "file": file, "review": review }))
        }

        "review.list" => {
            let task_id = text_param(params, "taskId")?.to_string();
            let manager = manager.clone();
            let files = tokio::task::spawn_blocking(move || manager.changed_files(&task_id)).await??;
            Ok(json!({ "files": files }))
        }
        "review.keep" | "review.undo" => {
            let task_id = text_param(params, "taskId")?.to_string();
            let path = text_param(params, "path")?.to_string();
            let undo = method == "review.undo";
            let manager = manager.clone();
            let review = tokio::task::spawn_blocking(move || {
                if undo { manager.undo(&task_id, &path) } else { manager.keep(&task_id, &path) }
            })
            .await??;
            Ok(json!({ "review": review }))
        }

        "git.state" => {
            let task_id = text_param(params, "taskId")?.to_string();
            let manager = manager.clone();
            Ok(json!(tokio::task::spawn_blocking(move || manager.git_state(&task_id)).await??))
        }
        "git.diff" => {
            let task_id = text_param(params, "taskId")?.to_string();
            let path = text_param(params, "path")?.to_string();
            // Without `from` the file is compared with the last commit; with it, over a span of the task's life.
            let from = params.get("from").and_then(Value::as_str).map(str::to_string);
            let manager = manager.clone();
            tokio::task::spawn_blocking(move || match from {
                Some(from) => manager.diff_between(&task_id, &path, &from),
                None => manager.git_diff(&task_id, &path),
            })
            .await?
        }
        "git.files" => {
            let task_id = text_param(params, "taskId")?.to_string();
            let from = text_param(params, "from")?.to_string();
            let manager = manager.clone();
            tokio::task::spawn_blocking(move || manager.files_between(&task_id, &from)).await?
        }
        "checkpoints.list" => {
            let task_id = text_param(params, "taskId")?.to_string();
            let manager = manager.clone();
            tokio::task::spawn_blocking(move || manager.checkpoints(&task_id)).await?
        }
        "checkpoints.rewind" => {
            let task_id = text_param(params, "taskId")?.to_string();
            let n = params.get("n").and_then(Value::as_i64).ok_or_else(|| anyhow!("Missing \"n\""))?;
            let keep_mine = params.get("keepMine").and_then(Value::as_bool).unwrap_or(true);
            let drop_messages = params.get("dropMessages").and_then(Value::as_bool).unwrap_or(false);
            if drop_messages {
                // The agent remembers what is being taken out; a new one will be told what is left.
                manager.stop_agent_and_wait(&task_id).await?;
            }
            let manager = manager.clone();
            tokio::task::spawn_blocking(move || manager.rewind(&task_id, n, keep_mine, drop_messages)).await?
        }
        "tasks.left" => {
            let task_id = text_param(params, "taskId")?.to_string();
            let manager = manager.clone();
            tokio::task::spawn_blocking(move || manager.leave(&task_id)).await??;
            Ok(json!({}))
        }
        "tasks.since" => {
            let task_id = text_param(params, "taskId")?.to_string();
            let manager = manager.clone();
            tokio::task::spawn_blocking(move || manager.since(&task_id)).await?
        }
        "git.commit" => {
            let task_id = text_param(params, "taskId")?.to_string();
            let message = text_param(params, "message")?.to_string();
            let amend = params.get("amend").and_then(Value::as_bool).unwrap_or(false);
            let push = params.get("push").and_then(Value::as_bool).unwrap_or(false);
            let picks: Vec<crate::gitops::Pick> = params
                .get("picks")
                .and_then(Value::as_array)
                .ok_or_else(|| anyhow!("Missing parameter: picks"))?
                .iter()
                .filter_map(|pick| {
                    Some(crate::gitops::Pick {
                        path: pick.get("path")?.as_str()?.to_string(),
                        changes: pick
                            .get("changes")
                            .and_then(Value::as_array)
                            .map(|list| list.iter().filter_map(|n| n.as_u64().map(|n| n as usize)).collect()),
                    })
                })
                .collect();
            let manager = manager.clone();
            let state =
                tokio::task::spawn_blocking(move || manager.git_commit(&task_id, &message, &picks, amend, push))
                    .await??;
            Ok(json!(state))
        }
        "git.push" | "git.update" | "git.undoCommit" | "git.revertCommit" | "git.revert" => {
            let task_id = text_param(params, "taskId")?.to_string();
            let hash = params.get("hash").and_then(Value::as_str).map(str::to_string);
            let path = params.get("path").and_then(Value::as_str).map(str::to_string);
            let change = params.get("change").and_then(Value::as_u64).map(|n| n as usize);
            let method = method.to_string();
            let manager = manager.clone();
            tokio::task::spawn_blocking(move || {
                let need =
                    |value: Option<String>, name: &str| value.ok_or_else(|| anyhow!("Missing parameter: {name}"));
                match method.as_str() {
                    "git.push" => manager.git_push(&task_id),
                    "git.update" => manager.git_update(&task_id),
                    "git.undoCommit" => manager.git_undo_commit(&task_id, &need(hash, "hash")?),
                    "git.revertCommit" => manager.git_revert_commit(&task_id, &need(hash, "hash")?),
                    _ => manager.git_revert(&task_id, &need(path, "path")?, change),
                }
            })
            .await??;
            Ok(json!({}))
        }

        "integrations.status" => {
            let manager = manager.clone();
            Ok(tokio::task::spawn_blocking(move || manager.integrations()).await?)
        }
        "integrations.setLinearKey" => {
            manager.set_linear_key(params.get("key").and_then(Value::as_str))?;
            Ok(json!({}))
        }
        "integrations.setLinearTeam" => {
            manager.set_linear_team(text_param(params, "projectId")?, params.get("team").and_then(Value::as_str))?;
            Ok(json!({}))
        }
        "issues.list" => {
            let manager = manager.clone();
            let projects = tokio::task::spawn_blocking(move || manager.all_issues()).await??;
            Ok(json!({ "projects": projects }))
        }
        "issues.start" => {
            let task = manager
                .start_issue(
                    text_param(params, "projectId")?,
                    text_param(params, "key")?,
                    text_param(params, "agentId")?,
                )
                .await?;
            Ok(json!(task))
        }

        "mr.get" => {
            let task_id = text_param(params, "taskId")?.to_string();
            let fresh = params.get("fresh").and_then(Value::as_bool).unwrap_or(false);
            let manager = manager.clone();
            tokio::task::spawn_blocking(move || manager.request_of(&task_id, fresh)).await?
        }
        "mr.overview" => {
            let manager = manager.clone();
            let requests = tokio::task::spawn_blocking(move || manager.requests_overview()).await??;
            Ok(json!({ "requests": requests }))
        }
        "mr.create" => {
            let task_id = text_param(params, "taskId")?.to_string();
            let title = text_param(params, "title")?.to_string();
            let body = params.get("body").and_then(Value::as_str).unwrap_or_default().to_string();
            let blocking = manager.clone();
            let id = task_id.clone();
            let request = tokio::task::spawn_blocking(move || blocking.open_request(&id, &title, &body)).await??;
            // The automations that wait for a request to open get their turn.
            let waiting = manager.clone();
            let _ = tokio::task::spawn_blocking(move || waiting.on_request_event(&task_id, false)).await;
            Ok(json!({ "request": request }))
        }
        "mr.comments" => {
            let task_id = text_param(params, "taskId")?.to_string();
            let manager = manager.clone();
            tokio::task::spawn_blocking(move || manager.request_comments(&task_id)).await?
        }
        "mr.reply" => {
            let task_id = text_param(params, "taskId")?.to_string();
            let comment = text_param(params, "commentId")?.to_string();
            let body = text_param(params, "body")?.to_string();
            let manager = manager.clone();
            tokio::task::spawn_blocking(move || manager.reply_to_comment(&task_id, &comment, &body)).await??;
            Ok(json!({}))
        }
        "mr.auto" => {
            let task_id = text_param(params, "taskId")?.to_string();
            let flag = |name: &str, fallback: bool| params.get(name).and_then(Value::as_bool).unwrap_or(fallback);
            // What to do at the merge is fixed when the user asks, like the ticks next to "Merge now".
            let options = flag("on", true).then(|| {
                json!({ "squash": flag("squash", true), "deleteBranch": flag("deleteBranch", true), "archive": flag("archive", true) })
            });
            let manager = manager.clone();
            tokio::task::spawn_blocking(move || manager.set_auto_merge(&task_id, options)).await??;
            Ok(json!({}))
        }
        "automations.list" => Ok(json!({ "automations": manager.automations() })),
        "automations.turn" => {
            let on = params.get("on").and_then(Value::as_bool).unwrap_or(true);
            manager.automation_turn(text_param(params, "id")?, on)?;
            Ok(json!({}))
        }
        "automations.save" => {
            let when = serde_json::from_value(params.get("when").cloned().unwrap_or(Value::Null))
                .map_err(|_| anyhow!("Say what sets the automation off"))?;
            let then = serde_json::from_value(params.get("then").cloned().unwrap_or(Value::Null))
                .map_err(|_| anyhow!("Say what the automation does"))?;
            let projects: Vec<String> = params
                .get("projects")
                .and_then(Value::as_array)
                .map(|list| list.iter().filter_map(|id| id.as_str().map(str::to_string)).collect())
                .unwrap_or_default();
            let id = manager.automation_save(
                params.get("id").and_then(Value::as_str),
                text_param(params, "name")?,
                when,
                then,
                projects,
            )?;
            Ok(json!({ "id": id }))
        }
        "automations.remove" => {
            manager.automation_remove(text_param(params, "id")?)?;
            Ok(json!({}))
        }
        "automations.clock" => {
            let offset = params
                .get("utcOffsetMinutes")
                .and_then(Value::as_i64)
                .ok_or_else(|| anyhow!("Missing parameter: utcOffsetMinutes"))?;
            manager.set_clock(offset)?;
            Ok(json!({}))
        }
        "usage.summary" => {
            let since = |name: &str| params.get(name).and_then(Value::as_u64).unwrap_or(0);
            let (day, week) = (since("dayStart"), since("weekStart"));
            let manager = manager.clone();
            tokio::task::spawn_blocking(move || manager.usage_summary(day, week)).await?
        }
        "usage.budget" => {
            // `daily: null` takes the budget away; a missing `daily` leaves it as it is.
            let daily = params.get("daily").map(Value::as_f64);
            manager.set_budget(daily, params.get("ask").and_then(Value::as_bool))?;
            Ok(manager.budget())
        }
        "tasks.switchAgent" => {
            let task = manager.switch_agent(text_param(params, "taskId")?, text_param(params, "agentId")?).await?;
            Ok(json!(task))
        }
        "checks.state" => {
            let task_id = text_param(params, "taskId")?.to_string();
            let manager = manager.clone();
            tokio::task::spawn_blocking(move || manager.checks_state(&task_id)).await?
        }
        "checks.run" => {
            let task_id = text_param(params, "taskId")?.to_string();
            let only = params.get("id").and_then(Value::as_str).map(str::to_string);
            let manager = manager.clone();
            tokio::task::spawn_blocking(move || manager.checks_run(&task_id, only.as_deref())).await??;
            Ok(json!({}))
        }
        "checks.stop" => {
            manager.check_stop(text_param(params, "taskId")?, text_param(params, "id")?)?;
            Ok(json!({}))
        }
        "checks.turn" => {
            let on = params.get("on").and_then(Value::as_bool).unwrap_or(true);
            manager.check_turn(text_param(params, "taskId")?, text_param(params, "id")?, on)?;
            Ok(json!({}))
        }
        "checks.add" => {
            let title = params.get("title").and_then(Value::as_str).unwrap_or_default();
            manager.check_add(text_param(params, "taskId")?, title, text_param(params, "command")?)?;
            Ok(json!({}))
        }
        "checks.remove" => {
            manager.check_remove(text_param(params, "taskId")?, text_param(params, "id")?)?;
            Ok(json!({}))
        }
        "checks.addLine" => {
            manager.line_add(text_param(params, "taskId")?, text_param(params, "text")?)?;
            Ok(json!({}))
        }
        "checks.setLine" => {
            let done = params.get("done").and_then(Value::as_bool).unwrap_or(true);
            manager.line_set(text_param(params, "taskId")?, text_param(params, "id")?, done)?;
            Ok(json!({}))
        }
        "checks.removeLine" => {
            manager.line_remove(text_param(params, "taskId")?, text_param(params, "id")?)?;
            Ok(json!({}))
        }
        "overlaps.list" => {
            let task_id = text_param(params, "taskId")?.to_string();
            let manager = manager.clone();
            tokio::task::spawn_blocking(move || manager.overlaps(&task_id)).await?
        }
        "overlaps.dismiss" => {
            let task_id = text_param(params, "taskId")?.to_string();
            let mark = text_param(params, "mark")?.to_string();
            let manager = manager.clone();
            tokio::task::spawn_blocking(move || manager.dismiss_overlap(&task_id, &mark)).await??;
            Ok(json!({}))
        }
        "mr.merge" => {
            let task_id = text_param(params, "taskId")?.to_string();
            let flag = |name: &str, fallback: bool| params.get(name).and_then(Value::as_bool).unwrap_or(fallback);
            let (squash, delete_branch, archive) =
                (flag("squash", true), flag("deleteBranch", true), flag("archive", true));
            let blocking = manager.clone();
            let id = task_id.clone();
            let request =
                tokio::task::spawn_blocking(move || blocking.merge_request(&id, squash, delete_branch)).await??;
            // The merge stands whatever happens next; the cleaning up is a second, separate step.
            if delete_branch || archive {
                state.stop_everything(&task_id).await;
                manager.clean_up(&task_id, delete_branch, archive).await?;
            }
            if !archive {
                // The task is still there: the automations that wait for a merge get their turn.
                let _ = manager.store_setting(&format!("task.{task_id}.mergeHandled"), "1");
                state.clean_up_all(manager.on_request_event(&task_id, true)).await;
            }
            Ok(json!({ "request": request }))
        }
        "tasks.cleanUp" => {
            let task_id = text_param(params, "taskId")?;
            state.stop_everything(task_id).await;
            manager.clean_up(task_id, true, true).await?;
            Ok(json!({}))
        }

        "run.state" | "run.start" | "run.stop" | "run.logs" => {
            let task_id = text_param(params, "taskId")?.to_string();
            let service = params.get("service").and_then(Value::as_str).map(str::to_string);
            let (project, root) = state.run_place(&task_id)?;
            let runner = state.runner.clone();
            let method = method.to_string();
            let manager = manager.clone();
            tokio::task::spawn_blocking(move || {
                match method.as_str() {
                    "run.start" => runner.start(&task_id, &project, &root, service.as_deref())?,
                    "run.stop" => runner.stop(&task_id, &project, &root, service.as_deref())?,
                    "run.logs" => {
                        let service = service.ok_or_else(|| anyhow!("Missing parameter: service"))?;
                        return Ok(json!({ "termId": runner.logs(&task_id, &project, &root, &service)? }));
                    }
                    _ => {}
                }
                if method != "run.state" {
                    manager.broadcast_run_changed(&task_id);
                }
                Ok(json!(runner.state(&task_id, &project, &root)))
            })
            .await?
        }

        "search.files" | "search.text" => {
            let root = manager.cwd(text_param(params, "taskId")?)?;
            let query = text_param(params, "query")?.to_string();
            let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(50).min(500) as usize;
            let search = state.search.clone();
            let by_name = method == "search.files";
            let hits = tokio::task::spawn_blocking(move || {
                if by_name {
                    json!(search.files(&root, &query, limit))
                } else {
                    json!(search.text(&root, &query, limit))
                }
            })
            .await?;
            Ok(json!({ "hits": hits }))
        }
        "dialogs.pickFolder" => {
            let pick = state.desktop_for(params)?.pick_folder.clone();
            // The dialog stays open for as long as the user likes; that must not hold a worker of the event loop.
            let picked = tokio::task::spawn_blocking(move || pick()).await?;
            Ok(json!({ "path": picked.map(|path| path.to_string_lossy().into_owned()) }))
        }

        "search.chats" => {
            let query = text_param(params, "query")?.to_string();
            let limit = params.get("limit").and_then(Value::as_u64).unwrap_or(20).min(200) as usize;
            let search = state.search.clone();
            let manager = manager.clone();
            let hits = tokio::task::spawn_blocking(move || search.chats(&manager, &query, limit)).await??;
            Ok(json!({ "hits": hits }))
        }

        "" => bail!("Missing method"),
        other => bail!("Unknown method: {other}"),
    }
}
