//! Conversations from Claude Code, brought into the editor as tasks.
//!
//! Claude Code keeps each conversation as a file of lines under
//! `~/.claude/projects/<the folder it ran in>/<session>.jsonl`: one line for
//! everything the user typed, the model said, a tool was asked and answered.
//! The editor reads such a file and makes a task of it: the same words in the
//! conversation, the same tool calls, in the folder the conversation worked in
//! and on the branch that folder is on. The task sleeps. The next message
//! wakes Claude in it, which picks its own session up again when it can.
//!
//! Nothing of Claude Code's is changed: its files are only read.

use crate::store::TaskRow;
use crate::tasks::{Manager, TaskInfo, TaskState, now_ms};
use anyhow::{Context, Result, anyhow, bail};
use serde::Serialize;
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead as _, Read as _, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// How much of the start of a file is read, at most, to find the first thing the user said. That is
/// near the start, but not always in the first lines: a picture pasted into the first message is one
/// line of more than half a megabyte.
const HEAD: u64 = 8 * 1024 * 1024;
/// How much of its end is read, for the branch it ended on and when.
const TAIL: u64 = 96 * 1024;
/// The longest answer of a tool that is kept. More than this is cut: it was read by the model, not by a person.
const MOST_TEXT: usize = 20_000;
/// How many conversations are listed for one project, newest first.
const MOST_LISTED: usize = 200;
/// The agent that picks such a conversation up.
const CLAUDE: &str = "claude";
/// What the task a conversation became is kept under, followed by the name of the conversation.
const BROUGHT_IN: &str = "imported.claude.";

/// A conversation as it is listed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Found {
    /// The name Claude Code gave the session.
    pub id: String,
    /// The first thing the user said, as one short line.
    pub title: String,
    pub started_at: u64,
    pub last_at: u64,
    /// The folder it worked in, and whether that folder is still there.
    pub cwd: String,
    pub folder_there: bool,
    /// The branch it ended on, as Claude Code noted it.
    pub branch: Option<String>,
    /// The task it was brought in as, when it was.
    pub task_id: Option<String>,
}

/// A conversation read whole, ready to become a task.
#[derive(Debug)]
struct Conversation {
    session: String,
    cwd: String,
    title: String,
    started_at: u64,
    /// Each event with the time it happened; `seq`, `at` and the task are put in when it is stored.
    events: Vec<(u64, Value)>,
}

/// Where Claude Code keeps its files: where the core was told, where Claude Code was told, or `~/.claude`.
pub fn claude_home() -> Option<PathBuf> {
    let named = |name: &str| std::env::var_os(name).filter(|value| !value.is_empty()).map(PathBuf::from);
    named("AE_CLAUDE_DIR")
        .or_else(|| named("CLAUDE_CONFIG_DIR"))
        .or_else(|| dirs::home_dir().map(|home| home.join(".claude")))
}

/// A line of such a file that is part of the conversation itself: not a helper agent's own
/// conversation, not a note of the program, not the summary that replaces an older part.
fn counts(row: &Value) -> bool {
    !["isSidechain", "isMeta", "isCompactSummary"]
        .iter()
        .any(|flag| row.get(*flag).and_then(Value::as_bool) == Some(true))
}

/// The lines of a file that could be read.
fn rows(text: &str) -> impl Iterator<Item = Value> + '_ {
    text.lines().filter_map(|line| serde_json::from_str::<Value>(line.trim()).ok())
}

fn time_of(row: &Value) -> Option<u64> {
    let text = row.get("timestamp").and_then(Value::as_str)?;
    Some(crate::hosts::parse_time(text)).filter(|at| *at > 0)
}

/// Text without what the program put into the user's message for the model alone.
fn without_reminders(text: &str) -> String {
    let (open, close) = ("<system-reminder>", "</system-reminder>");
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(start) = rest.find(open) {
        out.push_str(&rest[..start]);
        match rest[start..].find(close) {
            Some(end) => rest = &rest[start + end + close.len()..],
            // One that never ends takes the rest with it.
            None => rest = "",
        }
    }
    out.push_str(rest);
    out
}

/// What the user typed in a "user" line, or nothing when the line is not the user's words:
/// the answer of a tool, the output of a command they ran, a notice of the program.
fn said(content: &Value) -> String {
    const NOT_THE_USER: [&str; 9] = [
        "<task-notification",
        "<local-command",
        "<command-name",
        "<command-message",
        "<bash-input",
        "<bash-stdout",
        "<bash-stderr",
        "[Request interrupted",
        "<user-prompt-submit-hook",
    ];
    let parts: Vec<&str> = match content {
        Value::String(text) => vec![text.as_str()],
        Value::Array(parts) => parts
            .iter()
            .filter(|part| part.get("type").and_then(Value::as_str) == Some("text"))
            .filter_map(|part| part.get("text").and_then(Value::as_str))
            .collect(),
        _ => vec![],
    };
    let text = without_reminders(&parts.join("\n")).trim().to_string();
    if NOT_THE_USER.iter().any(|start| text.starts_with(start)) { String::new() } else { text }
}

/// The first line of what was said, short enough for a list. A first line that only announces what
/// follows ("Background (from the session that started this one):") gives way to the line after it.
fn title_of(text: &str) -> String {
    let mut lines = text.lines().map(str::trim).filter(|line| !line.is_empty());
    let first = lines.next().unwrap_or("");
    let line = match lines.next() {
        Some(next) if first.ends_with(':') => next,
        _ => first,
    };
    let mut title: String = line.chars().take(90).collect();
    if line.chars().count() > 90 {
        title.push('…');
    }
    title
}

/// What a tool answered, as text.
fn result_text(content: Option<&Value>) -> String {
    let text = match content {
        Some(Value::String(text)) => text.clone(),
        Some(Value::Array(parts)) => {
            let lines: Vec<String> = parts
                .iter()
                .filter_map(|part| match part.get("type").and_then(Value::as_str)? {
                    "text" => part.get("text").and_then(Value::as_str).map(str::to_string),
                    "image" => Some("(an image)".to_string()),
                    _ => None,
                })
                .collect();
            lines.join("\n")
        }
        _ => String::new(),
    };
    match text.char_indices().nth(MOST_TEXT) {
        Some((cut, _)) => format!("{}\n(the rest was left out)", &text[..cut]),
        None => text,
    }
}

/// A tool call of Claude Code as the conversation shows the calls of any agent.
fn call(part: &Value) -> Value {
    let name = part.get("name").and_then(Value::as_str).unwrap_or("tool");
    let given = part.get("input").cloned().unwrap_or_else(|| json!({}));
    let word = |key: &str| given.get(key).and_then(Value::as_str);
    let path = word("file_path").or_else(|| word("notebook_path"));
    let file = path.and_then(|path| Path::new(path).file_name()).map(|name| name.to_string_lossy().into_owned());
    let named = |what: &str| format!("{what} {}", file.clone().unwrap_or_default()).trim().to_string();
    let (title, kind) = match name {
        "Bash" => (format!("`{}`", word("command").unwrap_or("").chars().take(400).collect::<String>()), "execute"),
        "Read" => (named("Read"), "read"),
        "Edit" | "MultiEdit" | "Write" | "NotebookEdit" => (named(name), "edit"),
        "Grep" | "Glob" | "ToolSearch" | "WebSearch" => (name.to_string(), "search"),
        "Agent" | "Task" => (format!("Agent: {}", word("description").unwrap_or("")).trim().to_string(), "think"),
        _ => (name.to_string(), "other"),
    };
    let mut update = json!({
        "sessionUpdate": "tool_call",
        "toolCallId": part.get("id"),
        "status": "pending",
        "title": title,
        "kind": kind,
        "rawInput": given,
        "content": [],
        "_meta": { "claudeCode": { "toolName": name } }
    });
    if let Some(path) = path {
        update["locations"] = json!([{ "path": path }]);
    }
    update
}

/// Reads up to `most` bytes of a file from `from`, as text.
fn piece(file: &mut std::fs::File, from: u64, most: u64) -> Option<String> {
    file.seek(SeekFrom::Start(from)).ok()?;
    let mut bytes = Vec::new();
    file.take(most).read_to_end(&mut bytes).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

/// What a list needs to know of one conversation, read from its start and its end only:
/// such a file runs to many megabytes, and a list has hundreds of them.
fn peek(path: &Path) -> Option<Found> {
    let id = path.file_stem()?.to_string_lossy().into_owned();
    let mut file = std::fs::File::open(path).ok()?;
    let length = file.metadata().ok()?.len();
    let (mut cwd, mut started, mut branch, mut title, mut last) = (None, None, None, String::new(), None);
    // Line by line, and no further than the first thing the user said: how it ended is at its end.
    let mut lines = std::io::BufReader::new(&mut file).take(HEAD);
    let mut line = Vec::new();
    loop {
        line.clear();
        if lines.read_until(b'\n', &mut line).ok()? == 0 {
            break;
        }
        let Ok(row) = serde_json::from_slice::<Value>(&line) else { continue };
        if !counts(&row) {
            continue;
        }
        cwd = cwd.or_else(|| row.get("cwd").and_then(Value::as_str).map(str::to_string));
        started = started.or_else(|| time_of(&row));
        last = time_of(&row).or(last);
        branch = row.get("gitBranch").and_then(Value::as_str).map(str::to_string).or(branch);
        if title.is_empty() && row.get("type").and_then(Value::as_str) == Some("user") {
            title =
                title_of(&said(row.get("message").and_then(|message| message.get("content")).unwrap_or(&Value::Null)));
        }
        if !title.is_empty() && cwd.is_some() && started.is_some() {
            break;
        }
    }
    // A conversation nobody said anything in is not one to bring in.
    if title.is_empty() {
        return None;
    }
    // Read from the middle of the file, the first line of this piece is the end of one: it is passed over.
    let tail = piece(&mut file, length.saturating_sub(TAIL), TAIL)?;
    for row in rows(&tail).filter(counts) {
        last = time_of(&row).or(last);
        branch = row.get("gitBranch").and_then(Value::as_str).map(str::to_string).or(branch);
    }
    let cwd = cwd?;
    let started_at = started?;
    Some(Found {
        id,
        title,
        started_at,
        last_at: last.unwrap_or(started_at),
        folder_there: Path::new(&cwd).is_dir(),
        cwd,
        // Claude Code writes "HEAD" for a folder that is on no branch.
        branch: branch.filter(|name| !name.is_empty() && name != "HEAD"),
        task_id: None,
    })
}

/// The conversations that worked in a project's folder or in a folder inside it, each as a list shows it.
///
/// Claude Code names the folder it keeps them in after the folder it ran in, with every sign made
/// a dash, so two folders can share a name. Which folder a conversation worked in is read from the
/// conversation itself, one file for each of Claude Code's folders: all in one folder began in the same place.
fn found_in(claude: &Path, project: &Path) -> Vec<(PathBuf, Found)> {
    let mut found = Vec::new();
    let dashed = dashed(project);
    let Ok(folders) = std::fs::read_dir(claude.join("projects")) else { return found };
    for folder in folders.flatten().map(|entry| entry.path()).filter(|path| path.is_dir()) {
        let Ok(files) = std::fs::read_dir(&folder) else { continue };
        let mut sessions: Vec<PathBuf> = files
            .flatten()
            .map(|entry| entry.path())
            .filter(|path| path.extension().is_some_and(|ending| ending == "jsonl") && path.is_file())
            .collect();
        sessions.sort();
        let inside = |one: &Found| Path::new(&one.cwd).starts_with(project);
        let mut listed = sessions.into_iter().filter_map(|path| peek(&path).map(|one| (path, one)));
        // A folder named after the project, or after a folder inside it, is looked through whole. Any
        // other is judged by the first conversation in it that can be listed: all in one folder began
        // in the same place, unless two places share the name, which the name itself would show.
        let named = folder.file_name().is_some_and(|name| name.to_string_lossy().starts_with(&dashed));
        if !named {
            match listed.next() {
                Some(first) if inside(&first.1) => found.push(first),
                _ => continue,
            }
        }
        // Each is still asked where it worked: two folders can share one name, with every sign a dash.
        found.extend(listed.filter(|(_, one)| inside(one)));
    }
    found
}

/// The name Claude Code gives the folder it keeps a folder's conversations in: the path with every
/// sign that is not a letter or a digit made a dash.
fn dashed(folder: &Path) -> String {
    folder.to_string_lossy().chars().map(|sign| if sign.is_ascii_alphanumeric() { sign } else { '-' }).collect()
}

/// Reads a conversation whole and turns it into the events of a task.
fn read(path: &Path) -> Result<Conversation> {
    let text = std::fs::read_to_string(path).with_context(|| format!("Could not read {}", path.display()))?;
    let session = path.file_stem().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default();
    let mut events: Vec<(u64, Value)> = Vec::new();
    let mut tools: HashMap<String, String> = HashMap::new();
    let (mut cwd, mut title, mut at, mut started, mut turn_open) = (None::<String>, String::new(), 0, None, false);
    let end_turn = |events: &mut Vec<(u64, Value)>, open: &mut bool, at: u64| {
        if *open {
            events.push((at, json!({ "type": "turn_end", "stopReason": "end_turn" })));
            events.push((at, json!({ "type": "state", "state": "idle" })));
        }
        *open = false;
    };
    for row in rows(&text).filter(counts) {
        cwd = cwd.or_else(|| row.get("cwd").and_then(Value::as_str).map(str::to_string));
        at = time_of(&row).unwrap_or(at);
        started = started.or(Some(at).filter(|at| *at > 0));
        let content = row.get("message").and_then(|message| message.get("content"));
        match row.get("type").and_then(Value::as_str) {
            Some("user") => {
                let words = content.map(said).unwrap_or_default();
                if !words.is_empty() {
                    end_turn(&mut events, &mut turn_open, at);
                    if title.is_empty() {
                        title = title_of(&words);
                    }
                    events.push((at, json!({ "type": "user_message", "text": words, "edited": [] })));
                    events.push((at, json!({ "type": "state", "state": "running" })));
                    turn_open = true;
                }
                // The answers of the tools the model asked for come back as lines of the user.
                for part in content.and_then(Value::as_array).into_iter().flatten() {
                    if part.get("type").and_then(Value::as_str) != Some("tool_result") {
                        continue;
                    }
                    let call_id = part.get("tool_use_id").and_then(Value::as_str).unwrap_or_default();
                    let failed = part.get("is_error").and_then(Value::as_bool) == Some(true);
                    events.push((
                        at,
                        json!({ "type": "update", "update": {
                            "sessionUpdate": "tool_call_update",
                            "toolCallId": call_id,
                            "status": if failed { "failed" } else { "completed" },
                            "_meta": { "claudeCode": { "toolName": tools.get(call_id).cloned().unwrap_or_else(|| "tool".into()) } },
                            "content": [{ "type": "content", "content": { "type": "text", "text": result_text(part.get("content")) } }]
                        } }),
                    ));
                }
            }
            Some("assistant") => {
                for part in content.and_then(Value::as_array).into_iter().flatten() {
                    match part.get("type").and_then(Value::as_str) {
                        Some("text") => {
                            let words = part.get("text").and_then(Value::as_str).unwrap_or_default();
                            if !words.trim().is_empty() {
                                events.push((
                                    at,
                                    json!({ "type": "update", "update": {
                                        "sessionUpdate": "agent_message_chunk",
                                        "content": { "type": "text", "text": words }
                                    } }),
                                ));
                            }
                        }
                        Some("tool_use") => {
                            if let Some(id) = part.get("id").and_then(Value::as_str) {
                                let name = part.get("name").and_then(Value::as_str).unwrap_or("tool");
                                tools.insert(id.to_string(), name.to_string());
                            }
                            events.push((at, json!({ "type": "update", "update": call(part) })));
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
    }
    end_turn(&mut events, &mut turn_open, at);
    if title.is_empty() {
        bail!("Nobody said anything in that conversation, so there is nothing to bring in");
    }
    let cwd = cwd.ok_or_else(|| anyhow!("That conversation does not say which folder it worked in"))?;
    Ok(Conversation { session, cwd, title, started_at: started.unwrap_or_else(now_ms), events })
}

/// Whether text can be the name of a session: it becomes the name of a file.
fn is_session_name(text: &str) -> bool {
    !text.is_empty() && text.len() <= 100 && text.chars().all(|letter| letter.is_ascii_alphanumeric() || letter == '-')
}

impl Manager {
    /// The open task a conversation was brought in as, when it was.
    ///
    /// Kept under the conversation's own name, not read from the task's session: an agent that could
    /// not pick the session up starts a new one, and the task is the same conversation all the same.
    fn brought_in(&self, session: &str) -> Option<String> {
        let task_id = self.store().setting(&format!("{BROUGHT_IN}{session}")).ok().flatten()?;
        // One that was archived since is out of sight: the conversation can be brought in again.
        let open = self.store().tasks().ok()?.iter().any(|task| task.id == task_id);
        open.then_some(task_id)
    }

    /// The conversations Claude Code had in a project's folder, newest first.
    pub fn claude_sessions(&self, project_id: &str, claude: &Path) -> Result<Vec<Found>> {
        let project = self.store().project(project_id)?;
        let mut found: Vec<Found> = found_in(claude, Path::new(&project.path))
            .into_iter()
            .map(|(_, one)| Found { task_id: self.brought_in(&one.id), ..one })
            .collect();
        found.sort_by(|a, b| b.last_at.cmp(&a.last_at).then_with(|| a.id.cmp(&b.id)));
        found.truncate(MOST_LISTED);
        Ok(found)
    }

    /// Makes a task of one conversation: its words and tool calls, in the folder it worked in.
    pub async fn import_claude_session(
        self: &Arc<Self>,
        project_id: &str,
        session: &str,
        claude: &Path,
    ) -> Result<TaskInfo> {
        if !is_session_name(session) {
            bail!("That is not the name of a conversation");
        }
        let project = self.store().project(project_id)?;
        if let Some(task_id) = self.brought_in(session) {
            bail!("That conversation is a task here already: {}", self.task(&task_id)?.row.title);
        }
        // It is looked for by its name among Claude Code's folders, and taken only when it began inside
        // the project. A name is never used as a path: it was checked above to be letters, digits and dashes.
        let (home, root, named) = (claude.to_path_buf(), PathBuf::from(&project.path), format!("{session}.jsonl"));
        let file = tokio::task::spawn_blocking(move || {
            let folders = std::fs::read_dir(home.join("projects")).ok()?;
            let file = folders.flatten().map(|entry| entry.path().join(&named)).find(|path| path.is_file())?;
            // One that cannot be listed (nobody spoke in it) is read anyway, to be refused in those words.
            let inside = peek(&file).is_none_or(|one| Path::new(&one.cwd).starts_with(&root));
            inside.then_some(file)
        })
        .await?
        .ok_or_else(|| anyhow!("That conversation is not among the ones of this project any more"))?;
        let read = tokio::task::spawn_blocking(move || read(&file)).await??;
        // Read whole, it says for certain where it worked: only a conversation of this project is taken.
        if !Path::new(&read.cwd).starts_with(&project.path) {
            bail!("That conversation is not among the ones of this project any more");
        }

        let id = uuid::Uuid::new_v4().to_string();
        let project_path = PathBuf::from(&project.path);
        let there = Path::new(&read.cwd).is_dir();
        // Its own folder, on the branch that folder is on now. When the folder is gone (a working copy
        // that was removed), the task gets one of its own, as any new task does, and Claude cannot pick
        // its session up there: it is told what was said instead.
        let (cwd, branch, base_branch, resumes) = if there {
            let cwd = PathBuf::from(&read.cwd);
            let in_git = crate::git::is_repo_with_commits(&cwd).await;
            let branch = if in_git { crate::git::current_branch(&cwd).await } else { None };
            let base = if in_git { crate::git::current_branch(&project_path).await } else { None };
            (cwd, branch, base, true)
        } else {
            let (cwd, branch, base) = self.prepare_folder(&project, &id, &read.title).await?;
            (cwd, branch, base, false)
        };
        // Claude takes it when this computer knows Claude at all; otherwise the agent new tasks get,
        // or the first one that can start.
        let registry = self.registry();
        let known = registry.get(CLAUDE).is_some();
        let agent_id = match known {
            true => CLAUDE.to_string(),
            false => self
                .default_agent()
                .or_else(|| registry.list().iter().find(|agent| agent.available).map(|agent| agent.id.clone()))
                .ok_or_else(|| anyhow!("No agent is set up to take the conversation"))?,
        };

        let mut events = Vec::with_capacity(read.events.len() + 3);
        let started = json!({
            "type": "agent_started", "agentId": agent_id, "resumed": false, "imported": true,
            "agentInfo": { "name": "claude-code", "title": "Claude Code", "version": "brought in" }
        });
        events.push((read.started_at, started));
        events.extend(read.events);
        let last_at = events.last().map_or(read.started_at, |(at, _)| *at);
        let note = if there {
            "This conversation was brought in from Claude Code. Your next message wakes Claude in it."
        } else {
            "This conversation was brought in from Claude Code. The folder it worked in is gone, so the task has a working copy of its own; the agent is told what was said."
        };
        events.push((last_at, json!({ "type": "tracker", "text": note })));
        events.push((last_at, json!({ "type": "state", "state": TaskState::Asleep })));
        let stored: Vec<(u64, u64, Value)> = events
            .into_iter()
            .enumerate()
            .map(|(seq, (at, mut event))| {
                event["taskId"] = json!(id);
                event["seq"] = json!(seq);
                event["at"] = json!(at);
                (seq as u64, at, event)
            })
            .collect();

        let row = TaskRow {
            id: id.clone(),
            project_id: project.id.clone(),
            title: read.title,
            agent_id,
            cwd: cwd.to_string_lossy().into_owned(),
            branch,
            base_branch,
            issue: None,
            agent_session: (resumes && known).then(|| read.session.clone()),
            created_at: read.started_at,
            // Everything in it was read already, where it was said.
            seen_seq: stored.len() as i64 - 1,
        };
        self.store().insert_task(&row)?;
        self.store().insert_events(&id, &stored)?;
        self.store().set_setting(&format!("{BROUGHT_IN}{}", read.session), Some(&id))?;
        let info = self.task(&id)?;
        self.broadcast("task.created", json!(info));
        Ok(info)
    }
}

/// The commands that bring conversations in.
///
/// - `import.claude.list { projectId }` gives `{ sessions: [{ id, title, startedAt, lastAt, cwd,
///   folderThere, branch, taskId }] }`, newest first. `taskId` names the task a session already is.
/// - `import.claude.take { projectId, sessionId }` makes the task and gives it.
pub async fn command(manager: &Arc<Manager>, method: &str, params: &Value) -> Result<Value> {
    let text =
        |name: &str| params.get(name).and_then(Value::as_str).ok_or_else(|| anyhow!("Missing parameter: {name}"));
    let project_id = text("projectId")?.to_string();
    let claude = claude_home().ok_or_else(|| anyhow!("This computer has no home folder to look for Claude Code in"))?;
    match method {
        "import.claude.list" => {
            let manager = manager.clone();
            let found = tokio::task::spawn_blocking(move || manager.claude_sessions(&project_id, &claude)).await??;
            Ok(json!({ "sessions": found }))
        }
        "import.claude.take" => {
            let info = manager.import_claude_session(&project_id, text("sessionId")?, &claude).await?;
            Ok(json!(info))
        }
        other => bail!("Unknown method: {other}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_the_user_typed_is_told_from_what_the_program_put_around_it() {
        assert_eq!(said(&json!("Fix the login limit")), "Fix the login limit");
        let wrapped = json!([
            { "type": "text", "text": "<system-reminder>\nToday is Friday.\n</system-reminder>\nFix the login limit" },
            { "type": "text", "text": "and add a test<system-reminder>hidden</system-reminder>" },
            { "type": "tool_result", "tool_use_id": "t1", "content": "not the user" }
        ]);
        assert_eq!(said(&wrapped), "Fix the login limit\nand add a test");
        // The output of a command they ran, a notice, a stop: none is something they said.
        for other in [
            "<bash-input>git status</bash-input>",
            "<local-command-stdout>ok</local-command-stdout>",
            "<command-name>/clear</command-name>",
            "[Request interrupted by user]",
            "<task-notification>done</task-notification>",
        ] {
            assert_eq!(said(&json!(other)), "", "{other}");
        }
        assert_eq!(said(&json!([{ "type": "tool_result", "content": "x" }])), "");
        assert_eq!(said(&Value::Null), "");
        assert_eq!(without_reminders("a<system-reminder>never ends"), "a");
    }

    #[test]
    fn a_tool_call_reads_like_the_calls_of_any_agent() {
        let bash = call(&json!({ "id": "t1", "name": "Bash", "input": { "command": "npm test" } }));
        assert_eq!((bash["title"].as_str(), bash["kind"].as_str()), (Some("`npm test`"), Some("execute")));
        assert_eq!(bash["toolCallId"], "t1");
        let edit = call(&json!({ "id": "t2", "name": "Edit", "input": { "file_path": "/shop/src/limit.ts" } }));
        assert_eq!((edit["title"].as_str(), edit["kind"].as_str()), (Some("Edit limit.ts"), Some("edit")));
        assert_eq!(edit["locations"], json!([{ "path": "/shop/src/limit.ts" }]));
        let other = call(&json!({ "id": "t3", "name": "mcp__linear__get_issue" }));
        assert_eq!((other["title"].as_str(), other["kind"].as_str()), (Some("mcp__linear__get_issue"), Some("other")));
    }

    #[test]
    fn a_long_answer_of_a_tool_is_cut_and_says_so() {
        let long = "я".repeat(MOST_TEXT + 5);
        let cut = result_text(Some(&json!(long)));
        assert!(cut.ends_with("\n(the rest was left out)"));
        assert_eq!(cut.chars().filter(|letter| *letter == 'я').count(), MOST_TEXT);
        let parts = json!([{ "type": "text", "text": "one" }, { "type": "image" }, { "type": "text", "text": "two" }]);
        assert_eq!(result_text(Some(&parts)), "one\n(an image)\ntwo");
        assert_eq!(result_text(None), "");
    }

    #[test]
    fn a_title_is_the_first_line_that_says_something() {
        assert_eq!(title_of("\n  Fix the login limit  \nand more"), "Fix the login limit");
        assert_eq!(
            title_of("Background (from the session that started this one):\n\nRename the limiter"),
            "Rename the limiter"
        );
        assert_eq!(title_of("Only a heading:"), "Only a heading:");
        assert_eq!(title_of(&"x".repeat(200)).chars().count(), 91);
        assert_eq!(title_of(""), "");
    }

    #[test]
    fn the_name_of_a_session_is_never_a_path() {
        assert!(is_session_name("44ad9d37-c7c7-4257-8277-3c1d6c74921e"));
        assert!(!is_session_name("../../etc/passwd") && !is_session_name("") && !is_session_name("a/b"));
    }
}
