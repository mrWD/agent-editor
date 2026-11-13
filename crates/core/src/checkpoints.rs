//! Checkpoints of a task and what they make possible: looking back at the code
//! as it was, rewinding to it, and telling the user what happened since they left.
//!
//! A checkpoint is every file of the task's working copy at one moment, kept as
//! a git commit outside the branches (`refs/ae/checkpoints/<task>/…`), so it is
//! never pushed and never shows in the history. One is taken before every
//! message the user sends, before an edit by hand, when the user leaves the
//! task, and before a rewind (so a rewind can itself be undone).
//!
//! A task in a folder without git has no checkpoints: there is nowhere to keep them.
//!
//! Everything here blocks. Call it from `spawn_blocking`.

use crate::gitops;
use crate::marks::{self, Author};
use crate::store::CheckpointRow;
use crate::tasks::{Manager, TaskState, now_ms};
use anyhow::{Result, anyhow, bail};
use serde_json::{Value, json};
use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

/// How many checkpoints the list shows: the newest ones.
const LISTED: usize = 60;

/// What the core remembers about the moment the user last left a task.
#[derive(Debug, Clone, Copy)]
struct Left {
    at: u64,
    seq: i64,
    checkpoint: Option<i64>,
}

fn left_key(task_id: &str) -> String {
    format!("task.{task_id}.left")
}

fn ref_prefix(task_id: &str) -> String {
    format!("refs/ae/checkpoints/{task_id}/")
}

impl Manager {
    /// The working copy of a task, or `None` when the task works in a folder without git.
    fn checkpoint_root(&self, task_id: &str) -> Result<Option<PathBuf>> {
        let task = self.store().task(task_id)?;
        Ok(task.branch.is_some().then(|| PathBuf::from(task.cwd)))
    }

    fn last_seq(&self, task_id: &str) -> i64 {
        self.store().next_seq(task_id).map(|next| next as i64 - 1).unwrap_or(-1)
    }

    /// Takes a checkpoint of a task now. `None` for a task without git.
    pub fn take_checkpoint(&self, task_id: &str, kind: &str, label: &str) -> Result<Option<CheckpointRow>> {
        let Some(root) = self.checkpoint_root(task_id)? else { return Ok(None) };
        // The moment is noted first: taking the snapshot takes a while, and what
        // happens meanwhile comes after the checkpoint, not before it.
        let (at, seq) = (now_ms(), self.last_seq(task_id));
        let tree = gitops::snapshot_tree(&root)?;
        // Leaving a task many times with nothing changed needs one checkpoint, not many.
        if kind == "left"
            && let Some(last) = self.store().checkpoints(task_id)?.pop()
            && last.tree == tree
        {
            return Ok(Some(last));
        }
        let name = format!("{}{}", ref_prefix(task_id), uuid::Uuid::new_v4().simple());
        let commit = gitops::keep_tree(&root, &tree, &name, label)?;
        let mut row = CheckpointRow {
            n: 0,
            at,
            kind: kind.to_string(),
            label: label.to_string(),
            seq,
            commit,
            tree,
            hidden: None,
        };
        row.n = self.store().add_checkpoint(task_id, &row)?;
        Ok(Some(row))
    }

    /// Called before a message goes to the agent: the code as the user left it for this turn.
    pub fn checkpoint_before_message(&self, task_id: &str, text: &str) {
        self.hand_edits.lock().unwrap().remove(task_id);
        let first = self.store().checkpoints(task_id).map(|rows| rows.iter().all(|row| row.kind == "left"));
        let (kind, label) = match first {
            Ok(true) => ("start", "Start of the task".to_string()),
            _ => ("message", text.lines().next().unwrap_or("").trim().chars().take(120).collect()),
        };
        if let Err(error) = self.take_checkpoint(task_id, kind, &label) {
            tracing::debug!("No checkpoint for {task_id}: {error:#}");
        }
    }

    /// Called before the user's own edit is written: one checkpoint for a run of edits by hand.
    pub fn checkpoint_before_edit(&self, task_id: &str) {
        if !self.hand_edits.lock().unwrap().insert(task_id.to_string()) {
            return;
        }
        if let Err(error) = self.take_checkpoint(task_id, "edit", "Before your edit by hand") {
            tracing::debug!("No checkpoint for {task_id}: {error:#}");
        }
    }

    /// Forgets the checkpoints of a task whose working copy is going away.
    pub fn drop_checkpoints(&self, project_root: &Path, task_id: &str) {
        gitops::drop_snapshots(project_root, &ref_prefix(task_id));
    }

    /// The checkpoints of a task, oldest first, each with what changed between it and the next one.
    pub fn checkpoints(&self, task_id: &str) -> Result<Value> {
        let Some(root) = self.checkpoint_root(task_id)? else {
            return Ok(json!({ "checkpoints": [], "git": false }));
        };
        let mut rows: Vec<CheckpointRow> =
            self.store().checkpoints(task_id)?.into_iter().filter(|row| row.kind != "left").collect();
        if rows.len() > LISTED {
            rows.drain(..rows.len() - LISTED);
        }
        let now = gitops::snapshot_tree(&root)?;
        let mut listed = Vec::new();
        for (index, row) in rows.iter().enumerate() {
            let next = rows.get(index + 1).map_or(now.as_str(), |next| next.tree.as_str());
            let changed = if row.tree == next { Vec::new() } else { gitops::changes_between(&root, &row.tree, next)? };
            listed.push(json!({
                "n": row.n,
                "at": row.at,
                "kind": row.kind,
                "label": row.label,
                "seq": row.seq,
                // Whose work the stretch after this checkpoint is: after an edit by hand, the user's.
                "who": if row.kind == "edit" { "you" } else { "agent" },
                "files": changed.len(),
                "added": changed.iter().map(|file| file.added).sum::<usize>(),
                "removed": changed.iter().map(|file| file.removed).sum::<usize>(),
                "paths": changed.iter().take(3).map(|file| file.path.clone()).collect::<Vec<_>>(),
                "sameAsNow": row.tree == now,
            }));
        }
        Ok(json!({ "checkpoints": listed, "git": true }))
    }

    /// The two points a comparison is between, as (root, from, to).
    /// `from` is "visit" (since the user last left), "branch" (since the task's
    /// branch started) or "checkpoint:<n>" (what rewinding to it would change).
    fn span(&self, task_id: &str, from: &str) -> Result<(PathBuf, String, String)> {
        let task = self.store().task(task_id)?;
        if task.branch.is_none() {
            bail!("This task works in a folder without git, so there is nothing to compare with");
        }
        let root = PathBuf::from(&task.cwd);
        let now = gitops::snapshot_tree(&root)?;
        if let Some(n) = from.strip_prefix("checkpoint:") {
            let n: i64 = n.parse().map_err(|_| anyhow!("No such checkpoint"))?;
            return Ok((root, now, self.store().checkpoint(task_id, n)?.commit));
        }
        match from {
            "visit" => {
                let checkpoint = self.left(task_id).and_then(|left| left.checkpoint);
                let Some(n) = checkpoint else { bail!("Nothing to compare with yet: this is your first visit") };
                Ok((root, self.store().checkpoint(task_id, n)?.commit, now))
            }
            "branch" => {
                let base = gitops::resolve_base(&root, task.base_branch.as_deref())
                    .ok_or_else(|| anyhow!("The branch this task started from is not known"))?;
                let start = gitops::fork_point(&root, &base)
                    .ok_or_else(|| anyhow!("Could not find where this branch left {base}"))?;
                Ok((root, start, now))
            }
            other => bail!("Unknown comparison: {other}"),
        }
    }

    /// The files that differ over a span. See `span`.
    pub fn files_between(&self, task_id: &str, from: &str) -> Result<Value> {
        let (root, a, b) = self.span(task_id, from)?;
        let files = if a == b { Vec::new() } else { gitops::changes_between(&root, &a, &b)? };
        // For a rewind: which of those files hold lines the user wrote, so they can be kept.
        let mut value = json!(files);
        for file in value.as_array_mut().into_iter().flatten() {
            let path = file["path"].as_str().unwrap_or_default().to_string();
            let yours = self
                .store()
                .marks(task_id, &path)?
                .is_some_and(|row| marks::decode(&row.authors).contains(&Author::You));
            file["yours"] = json!(yours);
        }
        Ok(json!({ "files": value }))
    }

    /// One file's changes over a span, line by line.
    pub fn diff_between(&self, task_id: &str, path: &str, from: &str) -> Result<Value> {
        let (root, a, b) = self.span(task_id, from)?;
        Ok(json!(gitops::diff_between(&root, &a, &b, path)?))
    }

    /// Puts the files of a task back as they were at checkpoint `n`. Commits
    /// are not touched. With `keep_mine`, lines the user wrote since stay.
    /// With `drop_messages`, what was said after that point leaves the
    /// conversation too; the caller must have stopped the agent first.
    pub fn rewind(&self, task_id: &str, n: i64, keep_mine: bool, drop_messages: bool) -> Result<Value> {
        let Some(root) = self.checkpoint_root(task_id)? else {
            bail!("This task works in a folder without git, so it has no checkpoints");
        };
        if matches!(self.state_of(task_id), TaskState::Running | TaskState::WaitingPermission) {
            bail!("The agent is working. Stop its turn first, then rewind.");
        }
        let target = self.store().checkpoint(task_id, n)?;
        // The code as it is now, so this rewind can be undone from the same list.
        let before = self
            .take_checkpoint(task_id, "rewind", "Before the rewind")?
            .ok_or_else(|| anyhow!("Could not save the code as it is now"))?;
        let files = gitops::changes_between(&root, &before.tree, &target.tree)?;

        let mut kept = Vec::new();
        for file in &files {
            let place = root.join(&file.path);
            let then = gitops::bytes_at(&root, &target.commit, &file.path)?;
            let authors = if keep_mine {
                self.store().marks(task_id, &file.path)?.map(|row| marks::decode(&row.authors)).unwrap_or_default()
            } else {
                Vec::new()
            };
            let has_mine = authors.contains(&Author::You);
            match then {
                // A file the user wrote lines in stays, even though it was not there then.
                None if has_mine => kept.push(file.path.clone()),
                None => {
                    let _ = std::fs::remove_file(&place);
                }
                Some(bytes) => {
                    let mut text = bytes;
                    if has_mine
                        && let (Ok(then_text), Ok(now_text)) =
                            (String::from_utf8(text.clone()), std::fs::read_to_string(&place))
                    {
                        // The agent's lines go back to how they were; the user's stay where they are.
                        text = marks::undo_agent(&then_text, &now_text, &authors).0.into_bytes();
                        kept.push(file.path.clone());
                    }
                    if let Some(parent) = place.parent() {
                        std::fs::create_dir_all(parent)?;
                    }
                    std::fs::write(&place, text)?;
                }
            }
        }

        if drop_messages {
            // The rewind's own checkpoint remembers what was taken out, so undoing it brings the messages back.
            self.store().hide_events(task_id, before.n, target.seq, before.seq)?;
            self.store().clear_agent_session(task_id)?;
        }
        // Rewinding to the point before an earlier rewind undoes that rewind: its messages come back.
        if let Some((after, upto)) = target.hidden {
            self.store().show_events(task_id, after, upto)?;
            self.store().clear_agent_session(task_id)?;
        }

        let conversation_changed = drop_messages || target.hidden.is_some();
        if conversation_changed {
            // What is left of the conversation may end with an older state than the
            // task has now. Saying the state again keeps the two in step.
            self.emit(task_id, json!({ "type": "state", "state": self.state_of(task_id) }));
        }

        let paths: Vec<String> = files.iter().map(|file| file.path.clone()).collect();
        if let Err(error) = self.sweep(task_id, Author::You, &paths) {
            tracing::debug!("Could not look at the files of {task_id}: {error:#}");
        }
        self.emit(
            task_id,
            json!({
                "type": "rewound",
                "to": target.n,
                "toAt": target.at,
                "label": target.label,
                "files": files.len(),
                "kept": kept,
                "droppedMessages": drop_messages,
                "undo": before.n,
            }),
        );
        self.broadcast("git.changed", json!({ "taskId": task_id }));
        if conversation_changed {
            self.broadcast("task.reload", json!({ "taskId": task_id }));
        }
        Ok(json!({ "files": files.len(), "kept": kept, "undo": before.n }))
    }

    fn left(&self, task_id: &str) -> Option<Left> {
        let text = self.store().setting(&left_key(task_id)).ok().flatten()?;
        let value: Value = serde_json::from_str(&text).ok()?;
        Some(Left { at: value["at"].as_u64()?, seq: value["seq"].as_i64()?, checkpoint: value["checkpoint"].as_i64() })
    }

    /// Notes that the user has left a task: from here "since your last visit" is counted.
    pub fn leave(&self, task_id: &str) -> Result<()> {
        self.store().task(task_id)?;
        // Noted before the snapshot, for the same reason as in `take_checkpoint`.
        let (at, seq) = (now_ms(), self.last_seq(task_id));
        let checkpoint = match self.take_checkpoint(task_id, "left", "You left") {
            Ok(row) => row.map(|row| row.n),
            Err(error) => {
                tracing::debug!("No checkpoint on leaving {task_id}: {error:#}");
                None
            }
        };
        let left = json!({ "at": at, "seq": seq, "checkpoint": checkpoint });
        self.store().set_setting(&left_key(task_id), Some(&left.to_string()))
    }

    /// Where the user stopped in a task and what happened after they left it.
    /// `null` for a task the user has not left yet.
    pub fn since(&self, task_id: &str) -> Result<Value> {
        let task = self.store().task(task_id)?;
        let Some(left) = self.left(task_id) else { return Ok(Value::Null) };
        let agent =
            self.registry().get(&task.agent_id).map_or_else(|| task.agent_id.clone(), |agent| agent.name.clone());
        let events = self.store().events(task_id)?;
        let split = events.partition_point(|event| event["seq"].as_i64().unwrap_or(-1) <= left.seq);
        let (before, after) = events.split_at(split);

        let (mut happened, last_words) = happened_since(before, after, &agent, &task.cwd);
        let mut files = Vec::new();
        if task.branch.is_some()
            && let Some(n) = left.checkpoint
            && let Ok(then) = self.store().checkpoint(task_id, n)
        {
            let root = PathBuf::from(&task.cwd);
            let now = gitops::snapshot_tree(&root)?;
            if now != then.tree {
                files = gitops::changes_between(&root, &then.tree, &now)?;
            }
            // Commits nobody made through the editor: the agent ran git itself.
            let told: BTreeSet<String> = after
                .iter()
                .filter(|e| e["type"] == "git")
                .filter_map(|e| e["short"].as_str().map(str::to_string))
                .collect();
            for (short, subject) in gitops::commits_since(&root, &format!("{}^", then.commit)) {
                if !told.contains(&short) {
                    happened.push(
                        json!({ "who": "agent", "text": format!("{agent} committed “{subject}”"), "short": short }),
                    );
                }
            }
        }
        let quiet = happened.is_empty() && files.is_empty();
        Ok(json!({
            "leftAt": left.at,
            "leftSeq": left.seq,
            "awayMs": now_ms().saturating_sub(left.at),
            "stopped": stopped_sentence(before, &agent),
            "happened": happened,
            "lastWords": last_words,
            "files": files,
            "canCompare": left.checkpoint.is_some(),
            "quiet": quiet,
        }))
    }
}

fn base_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn short(text: &str, limit: usize) -> String {
    let line = text.lines().find(|line| !line.trim().is_empty()).unwrap_or("").trim();
    if line.chars().count() <= limit {
        return line.to_string();
    }
    let cut: String = line.chars().take(limit).collect();
    format!("{}…", cut.trim_end())
}

/// "a", "a and b", "a, b and c".
fn listed(parts: &[String]) -> String {
    match parts {
        [] => String::new(),
        [one] => one.clone(),
        [first @ .., last] => format!("{} and {last}", first.join(", ")),
    }
}

fn files_phrase(paths: &[String]) -> String {
    match paths.len() {
        0 => "nothing".into(),
        1..=3 => listed(&paths.iter().map(|path| base_name(path).to_string()).collect::<Vec<_>>()),
        n => format!("{n} files"),
    }
}

/// One thing the user did, as the last part of "You …".
enum Act {
    Said(String),
    Edited(Vec<String>),
    Did(String),
}

/// One sentence on what the user last did in a task, from the events up to the
/// moment they left. It is put together from what happened, word for word: no
/// model writes it, so it costs nothing and cannot invent anything.
pub fn stopped_sentence(events: &[Value], agent: &str) -> Option<String> {
    let mut acts: Vec<Act> = Vec::new();
    let mut asked: HashMap<&str, &Value> = HashMap::new();
    let mut state = "";
    let mut first_message = true;
    for event in events {
        match event["type"].as_str().unwrap_or("") {
            "user_message" => {
                let text = short(event["text"].as_str().unwrap_or(""), 90);
                let verb = if first_message { "started this task with" } else { "told" };
                acts.push(Act::Said(if first_message {
                    format!("{verb} “{text}”")
                } else {
                    format!("{verb} {agent} “{text}”")
                }));
                first_message = false;
            }
            "user_edit" => {
                let path = event["path"].as_str().unwrap_or("a file").to_string();
                match acts.last_mut() {
                    Some(Act::Edited(paths)) => {
                        if !paths.contains(&path) {
                            paths.push(path);
                        }
                    }
                    _ => acts.push(Act::Edited(vec![path])),
                }
            }
            "git" => {
                let subject = event["subject"].as_str().unwrap_or("");
                let did = match event["action"].as_str().unwrap_or("") {
                    "committed" => format!("committed “{}”", short(subject, 70)),
                    "pushed" => "pushed the branch".to_string(),
                    "opened" => "opened the merge request".to_string(),
                    "merged" => "merged the request".to_string(),
                    "undid" => "took the last commit back".to_string(),
                    "updated" => format!("updated the branch from {subject}"),
                    _ => continue,
                };
                acts.push(Act::Did(did));
            }
            "permission_request" => {
                asked.insert(event["requestId"].as_str().unwrap_or(""), event);
            }
            "permission_resolved" => {
                let Some(option) = event["optionId"].as_str() else { continue };
                let request = asked.get(event["requestId"].as_str().unwrap_or(""));
                let name = request
                    .and_then(|request| request["options"].as_array())
                    .and_then(|options| options.iter().find(|one| one["optionId"] == option))
                    .and_then(|one| one["name"].as_str())
                    .unwrap_or(option);
                acts.push(Act::Did(format!("answered “{name}”")));
            }
            "rewound" => acts.push(Act::Did("rewound the code".to_string())),
            "state" => state = event["state"].as_str().unwrap_or(""),
            _ => {}
        }
    }
    if acts.is_empty() {
        return None;
    }
    // The last three things say where the user was; older ones are in the conversation.
    let from = acts.len().saturating_sub(3);
    let parts: Vec<String> = acts[from..]
        .iter()
        .map(|act| match act {
            Act::Said(text) | Act::Did(text) => text.clone(),
            Act::Edited(paths) => format!("changed {} yourself", files_phrase(paths)),
        })
        .collect();
    let tail = match state {
        "running" => format!(", and left while {agent} was working"),
        "waiting_permission" => format!(", and left while {agent} was waiting for your answer"),
        _ => String::new(),
    };
    Some(format!("You {}{tail}.", listed(&parts)))
}

/// What happened in a task after the user left it, oldest first, and the first
/// lines of the agent's last message. Like `stopped_sentence`, it only repeats
/// what the events say.
///
/// `earlier` is what came before the user left. It is only read to learn what a
/// tool call is about: a call announced before the user left can finish after.
pub fn happened_since(earlier: &[Value], events: &[Value], agent: &str, cwd: &str) -> (Vec<Value>, Option<String>) {
    let mut lines: Vec<Value> = Vec::new();
    // What each tool call is: whether it changes a file, and which.
    let mut edits: HashMap<String, (bool, Option<String>)> = HashMap::new();
    let mut learn = |update: &Value| -> Option<(bool, Option<String>)> {
        let id = update["toolCallId"].as_str()?.to_string();
        let entry = edits.entry(id).or_insert((false, None));
        if let Some(kind) = update["kind"].as_str() {
            entry.0 = matches!(kind, "edit" | "delete" | "move");
        }
        if let Some(path) = update["locations"][0]["path"].as_str() {
            entry.1 = Some(crate::review::relative_to(Path::new(cwd), path).unwrap_or_else(|| path.into()));
        }
        Some(entry.clone())
    };
    for event in earlier.iter().filter(|event| event["type"] == "update") {
        learn(&event["update"]);
    }
    let mut changed: Vec<String> = Vec::new();
    let mut changed_at = 0;
    let mut turns = 0;
    let mut turn_at = 0;
    let mut open: Vec<(String, String, u64)> = Vec::new();
    let mut message = String::new();
    let mut last_message: Option<String> = None;

    for event in events {
        let at = event["at"].as_u64().unwrap_or(0);
        if event["type"] != "update" || event["update"]["sessionUpdate"] != "agent_message_chunk" {
            if !message.trim().is_empty() {
                last_message = Some(std::mem::take(&mut message));
            }
            message.clear();
        }
        match event["type"].as_str().unwrap_or("") {
            "update" => {
                let update = &event["update"];
                match update["sessionUpdate"].as_str().unwrap_or("") {
                    "agent_message_chunk" => message.push_str(update["content"]["text"].as_str().unwrap_or("")),
                    "tool_call" | "tool_call_update" => {
                        if let Some((true, Some(path))) = learn(update)
                            && update["status"] == "completed"
                        {
                            if !changed.contains(&path) {
                                changed.push(path);
                            }
                            changed_at = at;
                        }
                    }
                    _ => {}
                }
            }
            "turn_end" => {
                turns += 1;
                turn_at = at;
            }
            "permission_request" => open.push((
                event["requestId"].as_str().unwrap_or("").to_string(),
                event["toolCall"]["title"].as_str().unwrap_or("something").to_string(),
                at,
            )),
            "permission_resolved" => open.retain(|(id, _, _)| Some(id.as_str()) != event["requestId"].as_str()),
            "error" => lines.push(json!({
                "who": "agent",
                "text": format!("{agent} stopped with an error: {}", short(event["message"].as_str().unwrap_or(""), 120)),
                "at": at,
            })),
            "git" => {
                let subject = event["subject"].as_str().unwrap_or("");
                let text = match event["action"].as_str().unwrap_or("") {
                    "committed" => format!("You committed “{}”", short(subject, 70)),
                    "pushed" => "You pushed the branch".to_string(),
                    "opened" => "You opened the merge request".to_string(),
                    "merged" => "The request was merged".to_string(),
                    _ => continue,
                };
                lines.push(json!({ "who": "you", "text": text, "at": at, "short": event["short"] }));
            }
            "tracker" => lines.push(json!({ "who": "host", "text": event["text"], "at": at })),
            _ => {}
        }
    }
    if !message.trim().is_empty() {
        last_message = Some(message);
    }
    if !changed.is_empty() {
        let what = match changed.len() {
            1 => base_name(&changed[0]).to_string(),
            n => format!("{n} files"),
        };
        lines.push(
            json!({ "who": "agent", "text": format!("{agent} changed {what}"), "at": changed_at, "paths": changed }),
        );
    }
    if turns > 0 {
        let text =
            if turns == 1 { format!("{agent} finished its turn") } else { format!("{agent} finished {turns} turns") };
        lines.push(json!({ "who": "agent", "text": text, "at": turn_at }));
    }
    for (_, title, at) in open {
        lines.push(
            json!({ "who": "agent", "text": format!("{agent} asks: {}", short(&title, 90)), "at": at, "asks": true }),
        );
    }
    lines.sort_by_key(|line| line["at"].as_u64().unwrap_or(0));
    // The agent's own last words, cut to their first paragraph: usually a summary of what it did.
    let last_words = last_message.map(|text| {
        let paragraph = text.trim().split("\n\n").next().unwrap_or("").trim().to_string();
        if paragraph.chars().count() > 320 {
            format!("{}…", paragraph.chars().take(320).collect::<String>().trim_end())
        } else {
            paragraph
        }
    });
    (lines, last_words.filter(|text| !text.is_empty()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn said(text: &str) -> Value {
        json!({ "type": "user_message", "text": text })
    }
    fn state(state: &str) -> Value {
        json!({ "type": "state", "state": state })
    }

    #[test]
    fn the_sentence_names_the_last_things_the_user_did() {
        let events = vec![
            said("Add rate limiting to POST /login\nfive attempts a minute"),
            state("running"),
            state("idle"),
            json!({ "type": "user_edit", "path": "src/middleware/rateLimit.ts" }),
            json!({ "type": "user_edit", "path": "src/middleware/rateLimit.ts" }),
            json!({ "type": "user_edit", "path": "src/routes/auth.ts" }),
        ];
        assert_eq!(
            stopped_sentence(&events, "Codex").unwrap(),
            "You started this task with “Add rate limiting to POST /login” and changed rateLimit.ts and auth.ts yourself."
        );
    }

    #[test]
    fn the_sentence_keeps_three_things_and_says_what_the_agent_was_doing() {
        let events = vec![
            said("First"),
            json!({ "type": "git", "action": "committed", "subject": "Move the limit into config", "short": "c4d3e2f" }),
            json!({ "type": "git", "action": "opened", "subject": "", "short": "" }),
            said("Address both review comments"),
            state("running"),
        ];
        assert_eq!(
            stopped_sentence(&events, "Codex").unwrap(),
            "You committed “Move the limit into config”, opened the merge request and told Codex “Address both review comments”, and left while Codex was working."
        );
    }

    #[test]
    fn an_answer_to_a_question_is_named_by_what_the_button_said() {
        let events = vec![
            said("Change the limit"),
            json!({ "type": "permission_request", "requestId": "r1", "toolCall": { "title": "Edit a.ts" },
                    "options": [{ "optionId": "allow-once", "name": "Allow this edit" }] }),
            json!({ "type": "permission_resolved", "requestId": "r1", "optionId": "allow-once" }),
            // A question that was cancelled is not something the user did.
            json!({ "type": "permission_resolved", "requestId": "r2", "optionId": null }),
        ];
        assert_eq!(
            stopped_sentence(&events, "Codex").unwrap(),
            "You started this task with “Change the limit” and answered “Allow this edit”."
        );
        assert!(stopped_sentence(&[state("idle")], "Codex").is_none(), "nothing done, nothing to say");
    }

    #[test]
    fn what_happened_since_is_read_from_the_events() {
        let call = |id: &str, status: &str, path: &str, at: u64| {
            json!({ "type": "update", "at": at, "update": { "sessionUpdate": "tool_call_update", "toolCallId": id,
                    "kind": "edit", "status": status, "locations": [{ "path": path }] } })
        };
        let chunk = |text: &str| json!({ "type": "update", "at": 50, "update": { "sessionUpdate": "agent_message_chunk", "content": { "text": text } } });
        let events = vec![
            call("t1", "completed", "/work/src/a.ts", 10),
            call("t2", "completed", "/work/src/b.ts", 20),
            // An edit that was refused changed nothing.
            call("t3", "failed", "/work/src/c.ts", 25),
            chunk("Moved the limit into config.\n\n"),
            chunk("Details follow."),
            json!({ "type": "turn_end", "at": 60, "stopReason": "end_turn" }),
            json!({ "type": "permission_request", "at": 70, "requestId": "r9", "toolCall": { "title": "Run npm test" } }),
        ];
        // A call announced before the user left finishes after: what it is about is known from before.
        let earlier = vec![json!({ "type": "update", "update": { "sessionUpdate": "tool_call", "toolCallId": "t2",
                    "kind": "edit", "status": "pending", "locations": [{ "path": "/work/src/b.ts" }] } })];
        let mut events = events;
        events[1] = json!({ "type": "update", "at": 20, "update": { "sessionUpdate": "tool_call_update", "toolCallId": "t2", "status": "completed" } });
        let (lines, last) = happened_since(&earlier, &events, "Codex", "/work");
        let texts: Vec<&str> = lines.iter().map(|line| line["text"].as_str().unwrap()).collect();
        assert_eq!(texts, ["Codex changed 2 files", "Codex finished its turn", "Codex asks: Run npm test"]);
        assert_eq!(lines[0]["paths"], json!(["src/a.ts", "src/b.ts"]));
        assert_eq!(last.as_deref(), Some("Moved the limit into config."));

        let (lines, last) = happened_since(&[], &[], "Codex", "/work");
        assert!(lines.is_empty() && last.is_none());
    }
}
