//! The shared layer: what every agent can ask of the editor itself.
//!
//! Agents of different makers do not know of each other. The editor gives
//! each of them the same few tools, as a Model Context Protocol server of its
//! own, so that work can pass between them and to the user:
//! - `list_tasks`: the open tasks of the project and what each waits for;
//! - `leave_note`: a note for whoever works on this task next;
//! - `read_notes`: the notes left on this task so far;
//! - `report_problem`: something wrong in the project that is outside this
//!   task. It shows on the Health screen, where the user decides.
//!
//! And for setting the editor up when the user asks for that (see `setup`):
//! - `setup_guide`: what "working" means and the steps to get there;
//! - `setup_status`: the projects, code hosts, Linear, Docker and agents;
//! - `add_project`: a repository of the user's becomes a project;
//! - `set_linear_team`: which Linear team a project's issues come from.
//!
//! The server is part of the core's own web server. Each running agent gets
//! an address with a key of its own, so a tool call is always known to come
//! from one task's agent and can touch nothing but what the tools allow.

use crate::tasks::{Manager, now_ms};
use serde_json::{Value, json};
use std::sync::Arc;

const NOTICED_KEY: &str = "health.noticed";
const PROTOCOL: &str = "2025-06-18";

fn tools() -> Value {
    let text = |description: &str| json!({ "type": "string", "description": description });
    json!([
        {
            "name": "list_tasks",
            "description": "List the open tasks of this project in the editor: the title of each, which agent has it, and what it waits for. Use it to see what else is going on before you start something that may overlap.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "leave_note",
            "description": "Leave a short note on this task for whoever works on it next: another agent or the user. Say what you decided and why, or what you left undone. The note is shown in the task's conversation and told to the next agent.",
            "inputSchema": { "type": "object", "properties": { "text": text("The note, in plain words.") }, "required": ["text"] }
        },
        {
            "name": "read_notes",
            "description": "Read the notes agents left on this task so far.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "report_problem",
            "description": "Tell the user about something wrong in the project that is outside this task: a missing test, a risk, a bug you noticed in passing. Do not fix it; the user decides. It is listed on the editor's Health screen.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "title": text("What is wrong, in one line."),
                    "detail": text("Where it is and why it matters, in a sentence or two.")
                },
                "required": ["title"]
            }
        },
        {
            "name": "setup_guide",
            "description": "How to set this editor up for the user's work: projects, GitHub or GitLab (also a company's own), Linear, Docker and the agents. Read it first when the user asks you to set the editor up or to make its services work.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "setup_status",
            "description": "Where the editor's setup stands: each project with its code host and where its issues come from, whether gh, glab and Docker are ready, whether Linear is connected and its teams, and which agents can start. Call it before asking the user anything and again after each change.",
            "inputSchema": { "type": "object", "properties": {} }
        },
        {
            "name": "add_project",
            "description": "Add a git repository on this computer as a project of the editor. Only when setting the editor up and the user works in that repository.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "path": text("The full path of the repository's folder."),
                    "name": text("A name to show instead of the folder's own. Optional.")
                },
                "required": ["path"]
            }
        },
        {
            "name": "set_linear_team",
            "description": "Say which Linear team a project's issues come from, by the team's key such as APP. \"none\" sends them back to the project's GitHub or GitLab. Only when setting the editor up and you know the team is right.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "project": text("The project's name, as setup_status lists it."),
                    "team": text("The team's key, or \"none\".")
                },
                "required": ["project", "team"]
            }
        }
    ])
}

impl Manager {
    /// Tells the manager where the core's own web server listens, so agents can be given its address.
    pub fn set_mcp_base(&self, base: String) {
        *self.mcp_base.lock().unwrap() = Some(base);
    }

    /// The server to name to a task's agent when its session opens: the
    /// core's own address with a key made for this task. `None` when the core
    /// serves nothing (a test without a listener).
    pub(crate) fn mcp_server_for(&self, task_id: &str) -> Option<Value> {
        let base = self.mcp_base.lock().unwrap().clone()?;
        let key = uuid::Uuid::new_v4().simple().to_string();
        let mut keys = self.mcp_keys.lock().unwrap();
        // One key per task: an agent that is started again gets a new one, and the old one stops working.
        keys.retain(|_, task| task != task_id);
        keys.insert(key.clone(), task_id.to_string());
        Some(json!({ "type": "http", "name": "agent-editor", "url": format!("{base}/mcp/{key}"), "headers": [] }))
    }

    /// The address a task's running agent was given for the editor's tools, if it was given one.
    pub fn mcp_address_of(&self, task_id: &str) -> Option<String> {
        let base = self.mcp_base.lock().unwrap().clone()?;
        let keys = self.mcp_keys.lock().unwrap();
        keys.iter().find(|(_, task)| task.as_str() == task_id).map(|(key, _)| format!("{base}/mcp/{key}"))
    }

    /// The task a key was made for.
    pub fn mcp_task(&self, key: &str) -> Option<String> {
        self.mcp_keys.lock().unwrap().get(key).cloned()
    }

    /// Answers one message of the protocol. `None` for a message that wants no answer.
    pub fn mcp_answer(self: &Arc<Self>, task_id: &str, message: &Value) -> Option<Value> {
        let id = message.get("id").cloned()?;
        let method = message.get("method").and_then(Value::as_str).unwrap_or_default();
        let result = match method {
            "initialize" => Ok(json!({
                "protocolVersion": message.pointer("/params/protocolVersion").and_then(Value::as_str).unwrap_or(PROTOCOL),
                "capabilities": { "tools": {} },
                "serverInfo": { "name": "agent-editor", "title": "Agent Editor", "version": env!("CARGO_PKG_VERSION") },
                "instructions": "Tools of the editor this task runs in. Use them to see the other tasks of the project, to leave a note for whoever works on this task next, and to tell the user about problems outside the task. When the user asks you to set the editor up, read setup_guide and follow it."
            })),
            "ping" => Ok(json!({})),
            "tools/list" => Ok(json!({ "tools": tools() })),
            "tools/call" => {
                let name = message.pointer("/params/name").and_then(Value::as_str).unwrap_or_default();
                let arguments = message.pointer("/params/arguments").cloned().unwrap_or(json!({}));
                let (text, failed) = match self.mcp_tool(task_id, name, &arguments) {
                    Ok(text) => (text, false),
                    Err(error) => (format!("{error:#}"), true),
                };
                Ok(json!({ "content": [{ "type": "text", "text": text }], "isError": failed }))
            }
            other => Err(json!({ "code": -32601, "message": format!("Method not found: {other}") })),
        };
        Some(match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(error) => json!({ "jsonrpc": "2.0", "id": id, "error": error }),
        })
    }

    fn mcp_tool(self: &Arc<Self>, task_id: &str, name: &str, arguments: &Value) -> anyhow::Result<String> {
        let task = self.store().task(task_id)?;
        let agent =
            self.registry().get(&task.agent_id).map_or_else(|| task.agent_id.clone(), |agent| agent.name.clone());
        let text_of = |field: &str| arguments.get(field).and_then(Value::as_str).unwrap_or_default().trim().to_string();
        match name {
            "list_tasks" => {
                let mut lines = Vec::new();
                for other in self.list()?.into_iter().filter(|other| other.row.project_id == task.project_id) {
                    let who =
                        self.registry().get(&other.row.agent_id).map_or(other.row.agent_id.clone(), |a| a.name.clone());
                    let waits = match other.state {
                        crate::tasks::TaskState::Running => "its agent is working",
                        crate::tasks::TaskState::WaitingPermission => "its agent waits for the user's answer",
                        _ => "waits for the user's next message",
                    };
                    let mine = if other.row.id == task.id { " (this task)" } else { "" };
                    lines.push(format!("- {}{mine}: {who}; {waits}", other.row.title));
                }
                Ok(lines.join("\n"))
            }
            "leave_note" => {
                let text = text_of("text");
                if text.is_empty() {
                    anyhow::bail!("The note is empty");
                }
                self.emit(
                    task_id,
                    json!({ "type": "agent_note", "agentId": task.agent_id, "from": agent, "text": text }),
                );
                Ok("The note is kept with the task. Whoever works on it next is told.".into())
            }
            "read_notes" => {
                let notes: Vec<String> = self
                    .store()
                    .events(task_id)?
                    .iter()
                    .filter(|event| event["type"] == "agent_note")
                    .map(|event| {
                        format!(
                            "- {}: {}",
                            event["from"].as_str().unwrap_or("an agent"),
                            event["text"].as_str().unwrap_or("")
                        )
                    })
                    .collect();
                Ok(if notes.is_empty() { "No notes were left on this task.".into() } else { notes.join("\n") })
            }
            "report_problem" => {
                let title = text_of("title");
                if title.is_empty() {
                    anyhow::bail!("Say what is wrong");
                }
                let mut noticed = self.noticed();
                // The same thing said twice is one finding.
                if !noticed
                    .iter()
                    .any(|one| one["projectId"] == task.project_id.as_str() && one["title"] == title.as_str())
                {
                    noticed.push(json!({
                        "id": uuid::Uuid::new_v4().simple().to_string().split_at(8).0,
                        "projectId": task.project_id,
                        "taskId": task.id,
                        "taskTitle": task.title,
                        "agent": agent,
                        "title": title,
                        "detail": text_of("detail"),
                        "at": now_ms(),
                    }));
                    self.store().set_setting(NOTICED_KEY, Some(&Value::Array(noticed).to_string()))?;
                    self.broadcast("health.changed", json!({}));
                }
                Ok("The user is told on the Health screen. Do not fix it as part of this task.".into())
            }
            "setup_guide" => Ok(crate::setup::GUIDE.into()),
            "setup_status" => Ok(self.setup_report()),
            "add_project" => {
                let name = Some(text_of("name")).filter(|name| !name.is_empty());
                let path = text_of("path");
                if !std::path::Path::new(&path).join(".git").exists() {
                    anyhow::bail!("{path} is not a git repository. Give the folder that holds .git");
                }
                let project = self.add_project(&path, name)?;
                Ok(format!("{} ({}) is a project of the editor.", project.name, project.path))
            }
            "set_linear_team" => self.setup_linear_team(&text_of("project"), &text_of("team")),
            other => anyhow::bail!("No such tool: {other}"),
        }
    }

    /// What agents noticed and reported while they worked, oldest first.
    pub(crate) fn noticed(&self) -> Vec<Value> {
        self.store()
            .setting(NOTICED_KEY)
            .ok()
            .flatten()
            .and_then(|text| serde_json::from_str::<Vec<Value>>(&text).ok())
            .unwrap_or_default()
    }
}
