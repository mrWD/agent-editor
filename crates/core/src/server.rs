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
use crate::tasks::Manager;
use crate::terminals::{TermEvent, Terminals};
use anyhow::{Result, anyhow, bail};
use axum::extract::State;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::response::IntoResponse;
use axum::routing::get;
use axum::{Json, Router};
use futures_util::{SinkExt, StreamExt};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::broadcast::error::RecvError;
use tower_http::services::{ServeDir, ServeFile};

/// What every request handler can reach.
#[derive(Clone)]
pub struct AppState {
    pub manager: Arc<Manager>,
    pub terminals: Arc<Terminals>,
    pub search: Arc<Search>,
    pub runner: Arc<Runner>,
}

impl AppState {
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
        let terminals = Arc::new(Terminals::without(own));
        let runner = Arc::new(Runner::new(terminals.clone(), manager.docker_path(), manager.data_dir().to_path_buf()));
        Self { manager, terminals, search: Arc::new(Search::new()), runner }
    }

    /// The task's project name and folder, which is what running it needs.
    fn run_place(&self, task_id: &str) -> Result<(String, PathBuf)> {
        let task = self.manager.task(task_id)?;
        Ok((self.manager.project_name(&task.row.project_id), PathBuf::from(task.row.cwd)))
    }

    /// Stops what a task runs and removes its containers and volumes. For a task that is finished for good.
    async fn stop_everything(&self, task_id: &str) {
        self.terminals.close_task(task_id);
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
    let api = Router::new().route("/api/health", get(health)).route("/ws", get(upgrade)).with_state(state);
    match ui_dir {
        Some(dir) => {
            // Unknown paths fall back to index.html so links inside the interface survive a reload.
            let index = ServeFile::new(dir.join("index.html"));
            api.fallback_service(ServeDir::new(dir).fallback(index))
        }
        None => api,
    }
}

/// Listens on `addr` until the process is asked to stop.
pub async fn serve(state: AppState, addr: SocketAddr, ui_dir: Option<PathBuf>) -> Result<()> {
    let listener = tokio::net::TcpListener::bind(addr).await?;
    tracing::info!("Core is listening on http://{}", listener.local_addr()?);
    let manager = state.manager.clone();
    let terminals = state.terminals.clone();
    axum::serve(listener, router(state, ui_dir))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await?;
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
    let (mut sink, mut stream) = socket.split();
    let mut events = state.manager.subscribe();
    let mut term_events = state.terminals.subscribe();
    let mut attached = Attached::new();

    loop {
        tokio::select! {
            incoming = stream.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        let (reply, frames) = answer(&state, &mut attached, text.as_str()).await;
                        if sink.send(Message::Text(reply.to_string().into())).await.is_err() {
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

/// Runs one command and builds its answer, plus any binary messages that must follow it.
async fn answer(state: &AppState, attached: &mut Attached, text: &str) -> (Value, Vec<Vec<u8>>) {
    let Ok(request) = serde_json::from_str::<Value>(text) else {
        return (json!({ "id": Value::Null, "error": { "message": "The message is not JSON" } }), vec![]);
    };
    let id = request.get("id").cloned().unwrap_or(Value::Null);
    let method = request.get("method").and_then(Value::as_str).unwrap_or_default();
    let params = request.get("params").cloned().unwrap_or(json!({}));
    let mut frames = Vec::new();
    let outcome = if method.starts_with("terminal.") {
        terminal_command(state, attached, &mut frames, method, &params)
    } else {
        dispatch(state, method, &params).await
    };
    match outcome {
        Ok(result) => (json!({ "id": id, "result": result }), frames),
        Err(error) => (json!({ "id": id, "error": { "message": format!("{error:#}") } }), vec![]),
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
            let manager = manager.clone();
            tokio::task::spawn_blocking(move || manager.git_diff(&task_id, &path)).await?
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
            let manager = manager.clone();
            let request = tokio::task::spawn_blocking(move || manager.open_request(&task_id, &title, &body)).await??;
            Ok(json!({ "request": request }))
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
