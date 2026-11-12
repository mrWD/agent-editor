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
//! - `session/cancel` stops the turn with the stop reason `cancelled`.

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
    /// The folder each session works in.
    folders: Mutex<HashMap<String, PathBuf>>,
    next_request: AtomicU64,
    next_session: AtomicU64,
    next_tool: AtomicU64,
    /// Pause between streamed pieces, in milliseconds.
    delay_ms: u64,
}

impl Agent {
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
    let delay_ms = std::env::var("AE_MOCK_DELAY_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(35);

    let agent = Arc::new(Agent {
        out: Mutex::new(io::stdout()),
        waiting: Mutex::new(HashMap::new()),
        cancelled: Mutex::new(HashMap::new()),
        folders: Mutex::new(HashMap::new()),
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
                    "loadSession": false,
                    "promptCapabilities": { "image": false, "audio": false, "embeddedContext": false }
                },
                "agentInfo": { "name": "mock-agent", "title": "Practice agent", "version": env!("CARGO_PKG_VERSION") },
                "authMethods": []
            }),
        ),
        (Some("session/new"), Some(id)) => {
            let n = agent.next_session.fetch_add(1, Ordering::SeqCst);
            let session = format!("mock-{n}");
            let folder = params.get("cwd").and_then(Value::as_str).map(PathBuf::from).unwrap_or_default();
            agent.folders.lock().unwrap().insert(session.clone(), folder);
            agent.respond(&id, json!({ "sessionId": session }));
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
    ["edit", "change", "write", "fix", "add "].iter().any(|w| last.contains(w))
}

const DEMO_FILE: &str = "src/middleware/rateLimit.ts";

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

    let edit = wants_edit(&text);
    let folder = agent.folders.lock().unwrap().get(&session).cloned().unwrap_or_default();
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

    let answer = if !edit {
        "The limiter allows five attempts a minute and answers 429 after that. Nothing was changed.".to_string()
    } else if edited && file == DEMO_FILE {
        "Done. The limit now comes from config instead of a fixed 5. One file changed.".to_string()
    } else if edited {
        format!("Done. I changed {file}. One file changed.")
    } else {
        "I left the file as it was, because the edit was not allowed.".to_string()
    };
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

    agent.respond(id, json!({ "stopReason": "end_turn" }));
}
