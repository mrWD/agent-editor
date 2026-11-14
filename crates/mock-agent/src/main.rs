//! A practice agent that speaks the Agent Client Protocol (v1) over stdio.
//!
//! It calls no model and spends no tokens. Every reply is scripted, so tests
//! are repeatable and the editor can be tried without any account.
//!
//! What a prompt does:
//! - always: a short plan, a thought, a "read" tool call, then the answer in chunks;
//! - if the prompt mentions an edit ("edit", "change", "write", "fix"): an "edit"
//!   tool call that first asks the client for permission and, when allowed,
//!   really changes one file in the session's folder. The file is the one the
//!   prompt names, else `src/middleware/rateLimit.ts`, else a new `NOTES.md`;
//! - if the prompt asks for "markdown": an answer with a heading, a list, code and a link;
//! - if the prompt asks for a "commit message": a subject line, an empty line and a reason;
//! - if the prompt asks to end with a "VERDICT:" line: a short review and that line, and nothing is edited;
//! - if the prompt says "leave a note", "read the notes", "list the tasks" or "report a problem": the
//!   editor's own tool of that name is called and its answer said back;
//! - if the prompt asks for a flow (it holds a ```flow block as the pattern to follow): a fixed
//!   flow in that format, and nothing is edited. A note from the editor in front of the message
//!   may hold a ```flow block too, the task's plan; that is not a request for a flow;
//! - if the prompt asks for one block of a plan to be built (it has a line that starts with
//!   "Build only this block:"): the same edit as above, although the prompt holds the plan as a
//!   ```flow block. The first time the block's name has the word "limit" in it, the answer ends
//!   with a line "PROPOSE: …", as an agent does that thinks a block is missing from the plan;
//!   when the block's name has the word "test" in it and the edit was made, the answer also ends with a line
//!   "PROVED BY: …", naming the test that shows the block works;
//! - a prompt that starts with "Explain" is a question, even when it has the word "changed" in it;
//! - `session/cancel` stops the turn with the stop reason `cancelled`;
//! - every turn ends with a note on how much of the context is used. It costs nothing.
//!
//! A session offers two settings, as real agents do: a mode ("ask" before each
//! edit, or "auto" to edit without asking) and a model. Neither changes the answers.
//!
//! Switches, all through the environment:
//! - `AE_MOCK_DELAY_MS`: the pause between streamed pieces;
//! - `AE_MOCK_LOAD=1`: the agent can pick an earlier session up again (`session/load`);
//! - `AE_MOCK_MODES=1`: the agent only knows modes, the older way to offer a setting, and takes no pictures;
//! - `AE_MOCK_COST=0.25`: every turn adds this much to what the session says it cost.
//!
//! Started with `--lsp`, the same program is a practice language server instead: see `lsp.rs`.
//! Started with `--terminal`, it is an agent that speaks no protocol and talks in a terminal: see `terminal.rs`.

mod lsp;
mod terminal;

use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Sender};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

/// Shared state of the agent process.
struct Agent {
    /// The only writer to stdout. Every message is one line.
    out: Mutex<io::Stdout>,
    /// Requests we sent to the client and still wait on, by id.
    waiting: Mutex<HashMap<u64, Sender<Value>>>,
    /// One cancel flag per session.
    cancelled: Mutex<HashMap<String, Arc<AtomicBool>>>,
    /// What is known about each session.
    sessions: Mutex<HashMap<String, Session>>,
    /// True when earlier sessions can be picked up again.
    can_load: bool,
    /// True when settings are offered the older way, as modes only.
    modes_only: bool,
    /// What a turn "costs", in dollars. Nothing is ever charged; it is there so
    /// that adding up cost can be tried without an account.
    cost_per_turn: f64,
    next_request: AtomicU64,
    next_session: AtomicU64,
    next_tool: AtomicU64,
    /// Pause between streamed pieces, in milliseconds.
    delay_ms: u64,
}

/// One session: where it works and what the user set for it.
#[derive(Clone)]
struct Session {
    folder: PathBuf,
    mode: String,
    model: String,
    /// How much of the context is used, in tokens. It only grows.
    used: u64,
    /// What the session has "cost" so far.
    cost: f64,
    /// Where the editor's own tools are, when the editor named them at the start of the session.
    tools: Option<String>,
    /// Every tool server the session was given, as one line each: its name and how it is reached.
    servers: Vec<String>,
    /// True once the session has proposed a block for a plan. It does so once.
    proposed: bool,
}

impl Session {
    fn in_folder(params: &Value) -> Self {
        let folder = params.get("cwd").and_then(Value::as_str).map(PathBuf::from).unwrap_or_default();
        let given = params.get("mcpServers").and_then(Value::as_array).cloned().unwrap_or_default();
        let word = |server: &Value, key: &str| server.get(key).and_then(Value::as_str).unwrap_or_default().to_string();
        // The editor's own tool server is the one called after it.
        let tools =
            given.iter().find(|server| word(server, "name") == "agent-editor").map(|server| word(server, "url"));
        // The others are the user's: said back by name, with how each is reached and the names of what
        // it is handed (never the values: those are keys).
        let servers = given
            .iter()
            .filter(|server| word(server, "name") != "agent-editor")
            .map(|server| {
                let names = |key: &str| {
                    let list = server.get(key).and_then(Value::as_array).cloned().unwrap_or_default();
                    list.iter().map(|one| word(one, "name")).collect::<Vec<_>>().join(", ")
                };
                match server.get("type").and_then(Value::as_str) {
                    Some(kind) => format!(
                        "{} ({kind} {}; headers: {})",
                        word(server, "name"),
                        word(server, "url"),
                        names("headers")
                    ),
                    None => {
                        let args = server.get("args").and_then(Value::as_array).cloned().unwrap_or_default();
                        let args: Vec<&str> = args.iter().filter_map(Value::as_str).collect();
                        format!(
                            "{} (runs {} {}; env: {})",
                            word(server, "name"),
                            word(server, "command"),
                            args.join(" "),
                            names("env")
                        )
                    }
                }
            })
            .collect();
        Self {
            folder,
            mode: "ask".into(),
            model: "practice".into(),
            used: 0,
            cost: 0.0,
            tools,
            servers,
            proposed: false,
        }
    }

    /// The settings as the protocol describes them.
    fn options(&self) -> Value {
        json!([
            {
                "id": "mode",
                "name": "Mode",
                "description": "What the agent may do without asking",
                "category": "mode",
                "type": "select",
                "currentValue": self.mode,
                "options": [
                    { "value": "ask", "name": "Ask first", "description": "Asks before each edit" },
                    { "value": "auto", "name": "Edit freely", "description": "Edits without asking" }
                ]
            },
            {
                "id": "model",
                "name": "Model",
                "description": "Which practice model answers",
                "category": "model",
                "type": "select",
                "currentValue": self.model,
                "options": [
                    { "value": "practice", "name": "Practice", "description": "The scripted answers" },
                    { "value": "practice-fast", "name": "Practice fast", "description": "The same scripted answers" }
                ]
            }
        ])
    }

    /// The same mode setting, the older way.
    fn modes(&self) -> Value {
        json!({
            "currentModeId": self.mode,
            "availableModes": [
                { "id": "ask", "name": "Ask first", "description": "Asks before each edit" },
                { "id": "auto", "name": "Edit freely", "description": "Edits without asking" }
            ]
        })
    }
}

impl Agent {
    /// What a session offers to set, in the answer to opening or loading it.
    fn offer(&self, session: &Session) -> Value {
        if self.modes_only {
            json!({ "modes": session.modes() })
        } else {
            json!({ "configOptions": session.options() })
        }
    }

    fn send(&self, message: &Value) {
        let mut out = self.out.lock().unwrap();
        // A broken pipe means the client is gone; there is nobody to tell.
        let _ = writeln!(out, "{message}");
        let _ = out.flush();
    }

    fn respond(&self, id: &Value, result: Value) {
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "result": result }));
    }

    fn respond_error(&self, id: &Value, code: i64, message: &str) {
        self.send(&json!({
            "jsonrpc": "2.0",
            "id": id,
            "error": { "code": code, "message": message }
        }));
    }

    fn update(&self, session: &str, update: Value) {
        self.send(&json!({
            "jsonrpc": "2.0",
            "method": "session/update",
            "params": { "sessionId": session, "update": update }
        }));
    }

    /// Sends a request to the client and blocks until the answer arrives.
    fn request(&self, method: &str, params: Value) -> Option<Value> {
        let id = self.next_request.fetch_add(1, Ordering::SeqCst);
        let (tx, rx) = mpsc::channel();
        self.waiting.lock().unwrap().insert(id, tx);
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
        rx.recv().ok()
    }

    fn pause(&self) {
        if self.delay_ms > 0 {
            thread::sleep(Duration::from_millis(self.delay_ms));
        }
    }

    fn cancel_flag(&self, session: &str) -> Arc<AtomicBool> {
        self.cancelled
            .lock()
            .unwrap()
            .entry(session.to_string())
            .or_insert_with(|| Arc::new(AtomicBool::new(false)))
            .clone()
    }
}

fn main() {
    if std::env::args().any(|arg| arg == "--lsp") {
        return lsp::run();
    }
    if std::env::args().any(|arg| arg == "--terminal") {
        return terminal::run();
    }
    let delay_ms = std::env::var("AE_MOCK_DELAY_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(35);

    let agent = Arc::new(Agent {
        out: Mutex::new(io::stdout()),
        waiting: Mutex::new(HashMap::new()),
        cancelled: Mutex::new(HashMap::new()),
        sessions: Mutex::new(HashMap::new()),
        can_load: std::env::var("AE_MOCK_LOAD").is_ok_and(|v| v == "1"),
        modes_only: std::env::var("AE_MOCK_MODES").is_ok_and(|v| v == "1"),
        cost_per_turn: std::env::var("AE_MOCK_COST").ok().and_then(|v| v.parse().ok()).unwrap_or(0.0),
        next_request: AtomicU64::new(1),
        next_session: AtomicU64::new(1),
        next_tool: AtomicU64::new(1),
        delay_ms,
    });

    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        let Ok(line) = line else { break };
        if line.trim().is_empty() {
            continue;
        }
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            eprintln!("mock-agent: not JSON: {line}");
            continue;
        };
        handle(&agent, message);
    }
}

fn handle(agent: &Arc<Agent>, message: Value) {
    let id = message.get("id").cloned();
    let method = message.get("method").and_then(Value::as_str).map(str::to_string);
    let params = message.get("params").cloned().unwrap_or(Value::Null);

    match (method.as_deref(), id) {
        // An answer to a request we made.
        (None, Some(id)) => {
            let key = id.as_u64().unwrap_or(0);
            if let Some(tx) = agent.waiting.lock().unwrap().remove(&key) {
                let _ = tx.send(message);
            }
        }
        (Some("initialize"), Some(id)) => agent.respond(
            &id,
            json!({
                "protocolVersion": 1,
                "agentCapabilities": {
                    "loadSession": agent.can_load,
                    // The editor's own tools are reached by address.
                    "mcpCapabilities": { "http": true, "sse": false },
                    // The agent that only knows modes is the older kind in this too: it takes no pictures.
                    "promptCapabilities": { "image": !agent.modes_only, "audio": false, "embeddedContext": false }
                },
                "agentInfo": { "name": "mock-agent", "title": "Practice agent", "version": env!("CARGO_PKG_VERSION") },
                "authMethods": []
            }),
        ),
        (Some("session/new"), Some(id)) => {
            // The process id keeps the name apart from sessions of earlier runs.
            let n = agent.next_session.fetch_add(1, Ordering::SeqCst);
            let name = format!("mock-{}-{n}", std::process::id());
            let session = Session::in_folder(&params);
            let mut answer = agent.offer(&session);
            answer["sessionId"] = json!(name);
            agent.sessions.lock().unwrap().insert(name, session);
            agent.respond(&id, answer);
        }
        (Some("session/load"), Some(id)) if agent.can_load => {
            let name = params.get("sessionId").and_then(Value::as_str).unwrap_or_default().to_string();
            if !name.starts_with("mock-") {
                agent.respond_error(&id, -32002, &format!("No session called {name}"));
                return;
            }
            let session = Session::in_folder(&params);
            let answer = agent.offer(&session);
            agent.sessions.lock().unwrap().insert(name.clone(), session);
            // A loaded session sends its conversation again before it answers.
            agent.update(
                &name,
                json!({
                    "sessionUpdate": "agent_message_chunk",
                    "content": { "type": "text", "text": "An earlier answer, sent again." }
                }),
            );
            agent.respond(&id, answer);
        }
        (Some("session/set_config_option"), Some(id)) if !agent.modes_only => {
            let name = params.get("sessionId").and_then(Value::as_str).unwrap_or_default();
            let option = params.get("configId").and_then(Value::as_str).unwrap_or_default();
            let value = params.get("value").and_then(Value::as_str).unwrap_or_default();
            let mut sessions = agent.sessions.lock().unwrap();
            let Some(session) = sessions.get_mut(name) else {
                agent.respond_error(&id, -32002, &format!("No session called {name}"));
                return;
            };
            let offered = session.options();
            let known = offered
                .as_array()
                .into_iter()
                .flatten()
                .filter(|one| one["id"] == option)
                .flat_map(|one| one["options"].as_array().cloned().unwrap_or_default())
                .any(|one| one["value"] == value);
            if !known {
                agent.respond_error(&id, -32602, &format!("{option} cannot be {value}"));
                return;
            }
            match option {
                "mode" => session.mode = value.to_string(),
                _ => session.model = value.to_string(),
            }
            let options = session.options();
            drop(sessions);
            agent.update(name, json!({ "sessionUpdate": "config_option_update", "configOptions": options }));
            agent.respond(&id, json!({ "configOptions": options }));
        }
        (Some("session/set_mode"), Some(id)) => {
            let name = params.get("sessionId").and_then(Value::as_str).unwrap_or_default();
            let mode = params.get("modeId").and_then(Value::as_str).unwrap_or_default();
            let mut sessions = agent.sessions.lock().unwrap();
            match sessions.get_mut(name) {
                Some(session) if matches!(mode, "ask" | "auto") => {
                    session.mode = mode.to_string();
                    drop(sessions);
                    agent.update(name, json!({ "sessionUpdate": "current_mode_update", "currentModeId": mode }));
                    agent.respond(&id, json!({}));
                }
                _ => agent.respond_error(&id, -32602, &format!("No mode called {mode}")),
            }
        }
        (Some("session/prompt"), Some(id)) => {
            let agent = agent.clone();
            thread::spawn(move || run_turn(&agent, &id, &params));
        }
        (Some("session/cancel"), None) => {
            if let Some(session) = params.get("sessionId").and_then(Value::as_str) {
                agent.cancel_flag(session).store(true, Ordering::SeqCst);
            }
        }
        (Some(other), Some(id)) => {
            agent.respond_error(&id, -32601, &format!("Method not found: {other}"));
        }
        _ => {}
    }
}

/// Calls one of the editor's own tools and returns what it said.
fn call_tool(url: &str, name: &str, arguments: Value) -> String {
    let request = json!({ "jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": { "name": name, "arguments": arguments } });
    let answer: Result<Value, String> = ureq::post(url)
        .send_json(&request)
        .map_err(|error| error.to_string())
        .and_then(|mut response| response.body_mut().read_json::<Value>().map_err(|error| error.to_string()));
    match answer {
        Ok(answer) => answer
            .pointer("/result/content/0/text")
            .and_then(Value::as_str)
            .map(str::to_string)
            .unwrap_or_else(|| format!("The tool gave no text: {answer}")),
        Err(error) => format!("The tool could not be reached: {error}"),
    }
}

/// The pictures of a prompt, each told as its kind and its size.
fn pictures_of(params: &Value) -> Vec<String> {
    let blocks = params.get("prompt").and_then(Value::as_array).cloned().unwrap_or_default();
    blocks
        .iter()
        .filter(|block| block.get("type").and_then(Value::as_str) == Some("image"))
        .map(|block| {
            let kind = block.get("mimeType").and_then(Value::as_str).unwrap_or("of no known kind");
            let data = block.get("data").and_then(Value::as_str).unwrap_or_default();
            // Four letters of base64 are three bytes; the "=" at the end stand for none.
            let bytes = data.len() / 4 * 3 - data.bytes().rev().take_while(|byte| *byte == b'=').count();
            format!("{kind}, {bytes} bytes")
        })
        .collect()
}

/// Joins the text blocks of a prompt into one string.
fn prompt_text(params: &Value) -> String {
    params
        .get("prompt")
        .and_then(Value::as_array)
        .map(|blocks| blocks.iter().filter_map(|b| b.get("text").and_then(Value::as_str)).collect::<Vec<_>>().join(" "))
        .unwrap_or_default()
}

/// Only the last paragraph counts: a prompt may start with a recap of an
/// earlier conversation, and the newest instruction is always at the end.
fn last_paragraph(text: &str) -> &str {
    text.split("\n\n").map(str::trim).filter(|part| !part.is_empty()).last().unwrap_or("")
}

fn wants_edit(text: &str) -> bool {
    let last = last_paragraph(text).to_lowercase();
    // "Explain what changed here." asks about a change; it does not ask for one.
    if last.starts_with("explain") {
        return false;
    }
    ["edit", "change", "write", "fix", "add "].iter().any(|w| last.contains(w))
}

/// The editor's Map view asks for a flow by showing the format to answer in.
/// A recap of an earlier conversation may hold such a request too; only the
/// newest message counts, and the core puts it after "Now the user says:".
///
/// The editor may put a note in front of a message that shows the task's plan
/// in the same format. That is something to keep to, not a request to draw.
fn wants_flow(text: &str) -> bool {
    let newest = text.rsplit("Now the user says:").next().unwrap_or(text);
    newest.split("\n\n").any(|part| part.contains("```flow") && !part.trim_start().starts_with("Note from the editor:"))
}

/// The editor's Plan view asks for one block of a plan to be built, and names
/// it on a line of its own. That line is returned.
fn build_request(text: &str) -> Option<&str> {
    let newest = text.rsplit("Now the user says:").next().unwrap_or(text);
    newest.lines().find(|line| line.starts_with("Build only this block:"))
}

/// The flow the practice agent always draws: the one on the design's Map screen.
/// When the session's folder has the demo file, two steps say where in it they happen.
fn flow_answer(folder: &Path) -> String {
    let place = if folder.join(DEMO_FILE).is_file() { format!(" @ {DEMO_FILE}:3") } else { String::new() };
    format!(
        "Here is the flow.\n\n```flow\nstep a: Request to POST /login\nask b: Over the limit?{place}\nstep* c: Respond 429{place}\nstep d: Check the password\na -> b\nb -> c: yes\nb -> d: no\n```"
    )
}

const DEMO_FILE: &str = "src/middleware/rateLimit.ts";

/// An answer that uses Markdown the way real agents do, for a prompt that asks
/// for "markdown". The end of it tries to slip a script and a picture through:
/// an interface must show neither.
const MARKDOWN_ANSWER: &str = "## What I found\n\n- The limiter is in `src/middleware/rateLimit.ts:3`.\n- It allows **five** attempts a minute.\n\n```ts\nlimit: 5,\n```\n\nSee [the Express docs](https://expressjs.com/) for more. <img src=x onerror=\"window.hacked=1\"><script>window.hacked=1</script>";

/// The file a turn is about, relative to the session's folder: one the prompt
/// names, else the demo file, else a notes file that may not exist yet.
fn target_file(folder: &Path, text: &str) -> String {
    let named = last_paragraph(text)
        .split_whitespace()
        .map(|word| word.trim_matches(|c: char| !(c.is_alphanumeric() || c == '/' || c == '.' || c == '_' || c == '-')))
        .map(|word| word.trim_end_matches('.'))
        .find(|word| !word.is_empty() && !word.contains("..") && folder.join(word).is_file());
    match named {
        Some(word) => word.to_string(),
        None if folder.join(DEMO_FILE).is_file() => DEMO_FILE.to_string(),
        None => "NOTES.md".to_string(),
    }
}

/// The change the practice agent makes to a file: the old text, the new text
/// and the changed piece as (before, after) for showing.
fn planned_change(folder: &Path, file: &str) -> (Option<String>, String, Option<String>, String) {
    let old = std::fs::read_to_string(folder.join(file)).ok();
    match &old {
        Some(text) if text.contains("limit: 5,") => {
            let new = text.replacen("limit: 5,", "limit: limits.login,", 1);
            (old.clone(), new, Some("  limit: 5,\n".into()), "  limit: limits.login,\n".into())
        }
        Some(text) => {
            let line = if file.ends_with(".md") || file.ends_with(".txt") {
                "Edited by the practice agent.\n"
            } else {
                "// Edited by the practice agent.\n"
            };
            let gap = if text.is_empty() || text.ends_with('\n') { "" } else { "\n" };
            (old.clone(), format!("{text}{gap}{line}"), None, line.to_string())
        }
        None => {
            let line = "Notes from the practice agent.\n";
            (None, line.to_string(), None, line.to_string())
        }
    }
}

fn run_turn(agent: &Agent, id: &Value, params: &Value) {
    let session = params.get("sessionId").and_then(Value::as_str).unwrap_or("mock-0").to_string();
    let text = prompt_text(params);
    let cancel = agent.cancel_flag(&session);
    cancel.store(false, Ordering::SeqCst);

    let stop = |agent: &Agent| -> bool {
        if cancel.load(Ordering::SeqCst) {
            agent.respond(id, json!({ "stopReason": "cancelled" }));
            true
        } else {
            false
        }
    };

    // A request for a commit message is answered with one, and changes no file.
    let wants_message = last_paragraph(&text).to_lowercase().contains("commit message");
    // A request to build a block shows the plan as a flow, and is still a request to edit.
    let build = build_request(&text);
    let flow = build.is_none() && wants_flow(&text);
    // A reviewer is asked to end with a verdict, and changes nothing.
    let wants_verdict = last_paragraph(&text).contains("VERDICT:");
    // A message with pictures is answered by saying what arrived, and changes nothing.
    let pictures = pictures_of(params);
    let edit =
        pictures.is_empty() && !flow && !wants_message && !wants_verdict && (build.is_some() || wants_edit(&text));
    let known = agent.sessions.lock().unwrap().get(&session).cloned();
    let folder = known.as_ref().map(|s| s.folder.clone()).unwrap_or_default();
    let asks = known.as_ref().is_none_or(|s| s.mode != "auto");
    let file = target_file(&folder, &text);
    // Agents name files by their full path.
    let full_path = folder.join(&file).to_string_lossy().into_owned();

    let mut plan =
        vec![json!({ "content": "Read the file the task is about", "priority": "high", "status": "in_progress" })];
    if edit {
        plan.push(json!({ "content": "Make the change", "priority": "high", "status": "pending" }));
    }
    plan.push(json!({ "content": "Say what was done", "priority": "medium", "status": "pending" }));
    agent.update(&session, json!({ "sessionUpdate": "plan", "entries": plan }));
    agent.pause();

    agent.update(
        &session,
        json!({
            "sessionUpdate": "agent_thought_chunk",
            "content": { "type": "text", "text": "The user asked: " .to_string() + text.trim() }
        }),
    );
    agent.pause();
    if stop(agent) {
        return;
    }

    // A read never needs permission.
    let read_id = format!("tool-{}", agent.next_tool.fetch_add(1, Ordering::SeqCst));
    agent.update(
        &session,
        json!({
            "sessionUpdate": "tool_call",
            "toolCallId": read_id,
            "title": format!("Read {file}"),
            "kind": "read",
            "status": "in_progress",
            "locations": [{ "path": full_path }]
        }),
    );
    agent.pause();
    agent
        .update(&session, json!({ "sessionUpdate": "tool_call_update", "toolCallId": read_id, "status": "completed" }));
    agent.pause();
    if stop(agent) {
        return;
    }

    let mut edited = false;
    if edit {
        let edit_id = format!("tool-{}", agent.next_tool.fetch_add(1, Ordering::SeqCst));
        let (_, new_text, piece_before, piece_after) = planned_change(&folder, &file);
        let tool_call = json!({
            "toolCallId": edit_id,
            "title": format!("Edit {file}"),
            "kind": "edit",
            "status": "pending",
            "locations": [{ "path": full_path }],
            "content": [{
                "type": "diff",
                "path": full_path,
                "oldText": piece_before,
                "newText": piece_after
            }]
        });
        let mut announce = tool_call.clone();
        announce["sessionUpdate"] = json!("tool_call");
        agent.update(&session, announce);

        // In the "auto" mode the edit is made without a question.
        edited = !asks;
        if asks {
            let answer = agent.request(
                "session/request_permission",
                json!({
                    "sessionId": session,
                    "toolCall": tool_call,
                    "options": [
                        { "optionId": "allow-once", "name": "Allow this edit", "kind": "allow_once" },
                        { "optionId": "allow-always", "name": "Always allow edits", "kind": "allow_always" },
                        { "optionId": "reject-once", "name": "Do not edit", "kind": "reject_once" }
                    ]
                }),
            );
            let outcome = answer.as_ref().and_then(|a| a.get("result")).and_then(|r| r.get("outcome"));
            let kind = outcome.and_then(|o| o.get("outcome")).and_then(Value::as_str);
            let option = outcome.and_then(|o| o.get("optionId")).and_then(Value::as_str);

            if kind == Some("cancelled") || cancel.load(Ordering::SeqCst) {
                agent.update(
                    &session,
                    json!({ "sessionUpdate": "tool_call_update", "toolCallId": edit_id, "status": "failed" }),
                );
                agent.respond(id, json!({ "stopReason": "cancelled" }));
                return;
            }
            edited = matches!(option, Some("allow-once" | "allow-always"));
        }
        if edited {
            let target = folder.join(&file);
            if let Some(parent) = target.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            edited = std::fs::write(&target, new_text).is_ok();
        }
        agent.update(
            &session,
            json!({
                "sessionUpdate": "tool_call_update",
                "toolCallId": edit_id,
                "status": if edited { "completed" } else { "failed" }
            }),
        );
        agent.pause();
    }

    // A prompt that asks for one of the editor's tools gets it called, and the tool's answer said back.
    let asked = last_paragraph(&text).to_lowercase();
    let tool = if asked.contains("leave a note") {
        Some(("leave_note", json!({ "text": "The limit lives in config/limits.ts. Do not put the number back." })))
    } else if asked.contains("report a problem") {
        Some((
            "report_problem",
            json!({ "title": "POST /register has no rate limit", "detail": "Only /login is limited, in src/middleware/rateLimit.ts." }),
        ))
    } else if asked.contains("list the tasks") {
        Some(("list_tasks", json!({})))
    } else if asked.contains("read the notes") {
        Some(("read_notes", json!({})))
    } else {
        None
    };
    let edit = edit && tool.is_none();
    let tool_said = tool.map(|(name, arguments)| {
        let url = known.as_ref().and_then(|s| s.tools.clone());
        match url {
            Some(url) => call_tool(&url, name, arguments),
            None => "The editor gave this session no tools.".to_string(),
        }
    });

    let mut answer = if !pictures.is_empty() {
        let what = if pictures.len() == 1 { "one picture".to_string() } else { format!("{} pictures", pictures.len()) };
        format!("I see {what}: {}. Nothing was changed.", pictures.join(", "))
    } else if let Some(said) = tool_said {
        said
    } else if asked.contains("which tool servers") {
        // What the editor handed this session beside its own tools, said back so it can be seen.
        match known.as_ref().map(|session| session.servers.clone()).filter(|servers| !servers.is_empty()) {
            Some(servers) => format!("This session was given these tool servers: {}.", servers.join("; ")),
            None => "This session was given no tool servers of the user's.".to_string(),
        }
    } else if wants_verdict {
        // A task that asks to be "strict" is sent back once; a second look, and everything else, is fine.
        if text.to_lowercase().contains("strict") && !text.contains("You looked at this change before") {
            "Line 3 still holds a number that should come from config.\n\nVERDICT: redo".to_string()
        } else {
            "The change does what the plan says.\n\nVERDICT: fine".to_string()
        }
    } else if wants_message {
        "Limit login attempts to five a minute\n\nThe limit comes from config now, so it can change without a release."
            .to_string()
    } else if flow {
        flow_answer(&folder)
    } else if last_paragraph(&text).to_lowercase().contains("markdown") {
        MARKDOWN_ANSWER.to_string()
    } else if !edit {
        "The limiter allows five attempts a minute and answers 429 after that. Nothing was changed.".to_string()
    } else if edited && file == DEMO_FILE {
        "Done. The limit now comes from config instead of a fixed 5. One file changed.".to_string()
    } else if edited {
        format!("Done. I changed {file}. One file changed.")
    } else {
        "I left the file as it was, because the edit was not allowed.".to_string()
    };
    // Building a block about a limit, the practice agent misses one block in the plan. It says so once.
    if build.is_some_and(|line| line.to_lowercase().contains("limit")) {
        let mut sessions = agent.sessions.lock().unwrap();
        if let Some(known) = sessions.get_mut(&session).filter(|known| !known.proposed) {
            known.proposed = true;
            answer.push_str("\n\nPROPOSE: Reset the count after a correct password");
        }
    }
    // A block that asks for a test gets one named as its proof, when the edit was made.
    if edited && build.is_some_and(|line| line.to_lowercase().contains("test")) {
        answer.push_str("\n\nPROVED BY: answers 429 on the sixth attempt");
    }
    for word in answer.split_inclusive(' ') {
        if stop(agent) {
            return;
        }
        agent.update(
            &session,
            json!({ "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": word } }),
        );
        agent.pause();
    }

    // How full the context is now. The practice agent counts words, not tokens, and costs nothing.
    let (used, cost) = {
        let mut sessions = agent.sessions.lock().unwrap();
        let grown = 900 + 6 * (text.len() + answer.len()) as u64;
        sessions.get_mut(&session).map_or((grown, agent.cost_per_turn), |s| {
            s.used += grown;
            s.cost += agent.cost_per_turn;
            (s.used, s.cost)
        })
    };
    agent.update(
        &session,
        json!({
            "sessionUpdate": "usage_update",
            "used": used,
            "size": 200_000,
            "cost": { "amount": cost, "currency": "USD" }
        }),
    );

    agent.respond(id, json!({ "stopReason": "end_turn" }));
}
