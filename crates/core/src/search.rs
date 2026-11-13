//! Finding things: files by name, text inside files, and things said in tasks.
//!
//! Folders are walked the way git sees them: what `.gitignore` hides is not
//! searched. The list of a folder's files is kept for a few seconds, so typing
//! in the search box does not walk the disk on every key.
//!
//! The task on screen is searched by itself, so its answer is quick. The
//! other tasks, of every project, are searched apart from it ("everywhere"):
//! that answer may take longer and is added when it arrives.

use crate::tasks::Manager;
use anyhow::Result;
use ignore::{WalkBuilder, WalkState};
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long a folder's list of files is trusted.
const FILE_LIST_TTL: Duration = Duration::from_secs(5);
/// A folder with more files than this is listed only up to here.
const MAX_FILES: usize = 200_000;
/// Files larger than this are not searched for text.
const MAX_TEXT_BYTES: u64 = 1024 * 1024;
/// The longest piece of a line sent back with a hit.
const MAX_LINE_CHARS: usize = 240;
/// In another task, this many times the wanted number of files is compared
/// with the copies already listed, before the search gives up on finding more.
const COPIES_LOOKED_AT: usize = 4;

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileHit {
    pub path: String,
    /// Where the query matched in `path`, as character positions, for highlighting.
    pub positions: Vec<usize>,
    #[serde(skip)]
    score: i64,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TextHit {
    pub path: String,
    /// Line number, from 1.
    pub line: usize,
    /// Where the match starts in `text`, in characters, from 0.
    pub column: usize,
    /// How many characters matched.
    pub length: usize,
    /// The line, cut to a readable length around the match.
    pub text: String,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ChatHit {
    pub task_id: String,
    /// "you" or "agent".
    pub who: &'static str,
    pub at: u64,
    pub column: usize,
    pub length: usize,
    pub text: String,
}

/// A file found in a task other than the one on screen.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileHitIn {
    pub task_id: String,
    pub project_id: String,
    #[serde(flatten)]
    pub hit: FileHit,
}

/// A line found in a task other than the one on screen.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct TextHitIn {
    pub task_id: String,
    pub project_id: String,
    #[serde(flatten)]
    pub hit: TextHit,
}

/// The folder of one task, to be searched.
struct Place {
    task_id: String,
    project_id: String,
    root: PathBuf,
}

/// One thing said in a task, put together from its events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Said {
    pub who: &'static str,
    pub at: u64,
    pub text: String,
}

/// Per project: the folders whose files a search has listed so far.
type Listed = HashMap<String, Vec<PathBuf>>;

/// A folder's files and when they were listed.
type ListedFiles = (Instant, Arc<Vec<String>>);
/// What was said in a task, and the number of its next event when that was put together.
type SaidSoFar = (u64, Arc<Vec<Said>>);

/// Keeps what is expensive to find twice.
#[derive(Default)]
pub struct Search {
    files: Mutex<HashMap<PathBuf, ListedFiles>>,
    /// Per task: the number of its next event when the list was built, and the list.
    said: Mutex<HashMap<String, SaidSoFar>>,
}

fn walker(root: &Path) -> WalkBuilder {
    let mut builder = WalkBuilder::new(root);
    builder
        // Dot files are part of a project; only git's own folder is noise.
        .hidden(false)
        .filter_entry(|entry| entry.file_name() != ".git")
        .git_ignore(true)
        .git_exclude(true)
        .require_git(false);
    builder
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

/// Every file under `root`, as paths relative to it.
fn list_files(root: &Path) -> Vec<String> {
    let mut files = Vec::new();
    for entry in walker(root).build().flatten() {
        if entry.file_type().is_some_and(|t| t.is_file()) {
            files.push(relative(root, entry.path()));
            if files.len() >= MAX_FILES {
                break;
            }
        }
    }
    files
}

/// Scores how well `query` matches `path`. `None` when it does not match.
///
/// A query that appears whole in the file's name is best, whole anywhere in
/// the path next, and letters in order with gaps last. Shorter paths win ties.
pub fn match_path(path: &str, query: &str) -> Option<(i64, Vec<usize>)> {
    let path_chars: Vec<char> = path.chars().collect();
    let lower: Vec<char> = path.to_lowercase().chars().collect();
    // Lowercasing can change the number of characters; then positions would lie.
    let lower = if lower.len() == path_chars.len() { lower } else { path_chars.clone() };
    let query: Vec<char> = query.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
    if query.is_empty() {
        return None;
    }
    let name_start = lower.iter().rposition(|c| *c == '/').map(|i| i + 1).unwrap_or(0);
    let find = |from: usize| {
        (from..lower.len().saturating_sub(query.len() - 1)).find(|&i| lower[i..i + query.len()] == query[..])
    };
    let length_cost = path_chars.len() as i64;

    if let Some(at) = find(name_start) {
        let at_start = if at == name_start { 200 } else { 0 };
        return Some((3000 + at_start - length_cost, (at..at + query.len()).collect()));
    }
    if let Some(at) = find(0) {
        let after_slash = if at == 0 || lower[at - 1] == '/' { 100 } else { 0 };
        return Some((2000 + after_slash - length_cost, (at..at + query.len()).collect()));
    }

    // Letters in order, anywhere.
    let mut positions = Vec::with_capacity(query.len());
    let mut next = 0;
    for wanted in &query {
        let at = (next..lower.len()).find(|&i| lower[i] == *wanted)?;
        positions.push(at);
        next = at + 1;
    }
    let spread = (positions[positions.len() - 1] - positions[0]) as i64;
    Some((1000 - spread * 4 - length_cost, positions))
}

/// Cuts a long line to a window around the match and says where the match is in it.
fn window(line: &str, match_start: usize, match_len: usize) -> (String, usize) {
    let chars: Vec<char> = line.chars().collect();
    if chars.len() <= MAX_LINE_CHARS {
        return (line.to_string(), match_start);
    }
    let before = (MAX_LINE_CHARS.saturating_sub(match_len)) / 3;
    let from = match_start.saturating_sub(before);
    let to = (from + MAX_LINE_CHARS).min(chars.len());
    let mut text: String = chars[from..to].iter().collect();
    let mut column = match_start - from;
    if from > 0 {
        text.insert(0, '…');
        column += 1;
    }
    if to < chars.len() {
        text.push('…');
    }
    (text, column)
}

/// True when two files hold the same bytes. Files too large to read for this
/// count as the same when they are the same size.
fn same_file(a: &Path, b: &Path) -> bool {
    let (Ok(one), Ok(other)) = (std::fs::metadata(a), std::fs::metadata(b)) else { return false };
    if !one.is_file() || !other.is_file() || one.len() != other.len() {
        return false;
    }
    one.len() > MAX_TEXT_BYTES || matches!((std::fs::read(a), std::fs::read(b)), (Ok(x), Ok(y)) if x == y)
}

/// The folders of every open task but the one on screen, newest task first,
/// and for each project the folders whose files are listed already.
///
/// The tasks of one project are working copies of the same code, so most of
/// their files are the same file many times over. A file is listed for a task
/// only when no folder searched before it, in the same project, holds a copy:
/// first the task on screen, then the newer tasks.
fn places(manager: &Manager, here: Option<&str>) -> Result<(Vec<Place>, Listed)> {
    let mut tasks = manager.list()?;
    tasks.sort_by_key(|task| std::cmp::Reverse(task.row.created_at));
    let mut listed = Listed::new();
    let mut places = Vec::new();
    for task in tasks {
        let root = PathBuf::from(&task.row.cwd);
        if here == Some(task.row.id.as_str()) {
            listed.entry(task.row.project_id).or_default().push(root);
        } else {
            places.push(Place { task_id: task.row.id, project_id: task.row.project_id, root });
        }
    }
    Ok((places, listed))
}

/// Finds `needle` in `line` without regard to case. Both positions are in characters.
fn find_in_line(line: &str, needle_lower: &str) -> Option<usize> {
    if line.is_ascii() && needle_lower.is_ascii() {
        let hay = line.as_bytes();
        let needle = needle_lower.as_bytes();
        if needle.len() > hay.len() {
            return None;
        }
        return (0..=hay.len() - needle.len())
            .find(|&i| hay[i..i + needle.len()].iter().zip(needle).all(|(a, b)| a.to_ascii_lowercase() == *b));
    }
    let lower = line.to_lowercase();
    let byte = lower.find(needle_lower)?;
    Some(lower[..byte].chars().count())
}

impl Search {
    pub fn new() -> Self {
        Self::default()
    }

    fn files_of(&self, root: &Path) -> Arc<Vec<String>> {
        if let Some((at, list)) = self.files.lock().unwrap().get(root)
            && at.elapsed() < FILE_LIST_TTL
        {
            return list.clone();
        }
        let list = Arc::new(list_files(root));
        self.files.lock().unwrap().insert(root.to_path_buf(), (Instant::now(), list.clone()));
        list
    }

    /// Every file under `root` whose path matches `query`, best first.
    fn matching_files(&self, root: &Path, query: &str) -> Vec<FileHit> {
        let mut hits: Vec<FileHit> = self
            .files_of(root)
            .iter()
            .filter_map(|path| {
                match_path(path, query).map(|(score, positions)| FileHit { path: path.clone(), positions, score })
            })
            .collect();
        hits.sort_by(|a, b| b.score.cmp(&a.score).then_with(|| a.path.cmp(&b.path)));
        hits
    }

    /// Files under `root` whose path matches `query`, best first.
    pub fn files(&self, root: &Path, query: &str, limit: usize) -> Vec<FileHit> {
        let mut hits = self.matching_files(root, query);
        hits.truncate(limit);
        hits
    }

    /// Files of every open task but `here` (the task on screen, searched by
    /// itself) whose path matches `query`, best first. See `places` for which
    /// of several copies of one file is listed.
    pub fn files_everywhere(
        &self,
        manager: &Manager,
        here: Option<&str>,
        query: &str,
        limit: usize,
    ) -> Result<Vec<FileHitIn>> {
        if query.trim().is_empty() {
            return Ok(vec![]);
        }
        let (places, mut listed) = places(manager, here)?;
        let mut hits = Vec::new();
        for place in places {
            let before = listed.entry(place.project_id.clone()).or_default();
            // Tasks of a project without git all work in the project's own folder.
            if before.contains(&place.root) {
                continue;
            }
            let fresh = self
                .matching_files(&place.root, query)
                .into_iter()
                // Comparing reads files. In a working copy that differs in nothing, that would be every match.
                .take(limit.saturating_mul(COPIES_LOOKED_AT))
                .filter(|hit| {
                    !before.iter().any(|other| same_file(&other.join(&hit.path), &place.root.join(&hit.path)))
                })
                .take(limit)
                .map(|hit| FileHitIn { task_id: place.task_id.clone(), project_id: place.project_id.clone(), hit });
            hits.extend(fresh);
            before.push(place.root);
        }
        // Best first. Equal matches stay in the order they were found: the newer task first.
        hits.sort_by_key(|found| std::cmp::Reverse(found.hit.score));
        hits.truncate(limit);
        Ok(hits)
    }

    /// Lines of every open task but `here` that contain `query`, in any
    /// letter case, task by task from the newest. A file that a folder
    /// searched before holds a copy of is left out, as in `files_everywhere`.
    pub fn text_everywhere(
        &self,
        manager: &Manager,
        here: Option<&str>,
        query: &str,
        limit: usize,
    ) -> Result<Vec<TextHitIn>> {
        let (places, mut listed) = places(manager, here)?;
        let mut hits = Vec::new();
        for place in places {
            if hits.len() >= limit {
                break;
            }
            let before = listed.entry(place.project_id.clone()).or_default();
            if before.contains(&place.root) {
                continue;
            }
            let is_new = |path: &str, bytes: &[u8]| {
                !before.iter().any(|other| std::fs::read(other.join(path)).is_ok_and(|theirs| theirs == bytes))
            };
            let fresh = self
                .text_where(&place.root, query, limit - hits.len(), &is_new)
                .into_iter()
                .map(|hit| TextHitIn { task_id: place.task_id.clone(), project_id: place.project_id.clone(), hit });
            hits.extend(fresh);
            before.push(place.root);
        }
        Ok(hits)
    }

    /// Lines under `root` that contain `query`, in any letter case.
    pub fn text(&self, root: &Path, query: &str, limit: usize) -> Vec<TextHit> {
        self.text_where(root, query, limit, &|_, _| true)
    }

    /// Lines under `root` that contain `query`, from the files `wanted` lets
    /// through. It is asked about a file only once a line of it has matched,
    /// with the file's path from `root` and its bytes.
    fn text_where(
        &self,
        root: &Path,
        query: &str,
        limit: usize,
        wanted: &(dyn Fn(&str, &[u8]) -> bool + Sync),
    ) -> Vec<TextHit> {
        let needle = query.to_lowercase();
        if needle.trim().is_empty() {
            return vec![];
        }
        let needle_chars = needle.chars().count();
        let found = Mutex::new(Vec::new());
        let count = AtomicUsize::new(0);
        let full = AtomicBool::new(false);

        walker(root).build_parallel().run(|| {
            Box::new(|entry| {
                if full.load(Ordering::Relaxed) {
                    return WalkState::Quit;
                }
                let Ok(entry) = entry else { return WalkState::Continue };
                if !entry.file_type().is_some_and(|t| t.is_file()) {
                    return WalkState::Continue;
                }
                if entry.metadata().map(|m| m.len() > MAX_TEXT_BYTES).unwrap_or(true) {
                    return WalkState::Continue;
                }
                let Ok(bytes) = std::fs::read(entry.path()) else { return WalkState::Continue };
                // A zero byte near the start means this is not text.
                if bytes.iter().take(8000).any(|b| *b == 0) {
                    return WalkState::Continue;
                }
                let Ok(content) = std::str::from_utf8(&bytes) else { return WalkState::Continue };
                let path = relative(root, entry.path());
                let mut local = Vec::new();
                for (index, line) in content.lines().enumerate() {
                    if let Some(at) = find_in_line(line, &needle) {
                        let (text, column) = window(line, at, needle_chars);
                        local.push(TextHit { path: path.clone(), line: index + 1, column, length: needle_chars, text });
                        // More than the limit is never sent back, whatever the other files hold.
                        if local.len() >= limit {
                            break;
                        }
                    }
                }
                if !local.is_empty() && wanted(&path, &bytes) {
                    if count.fetch_add(local.len(), Ordering::Relaxed) + local.len() >= limit {
                        full.store(true, Ordering::Relaxed);
                    }
                    found.lock().unwrap().extend(local);
                }
                WalkState::Continue
            })
        });

        let mut hits = found.into_inner().unwrap();
        hits.sort_by(|a, b| a.path.cmp(&b.path).then_with(|| a.line.cmp(&b.line)));
        hits.truncate(limit);
        hits
    }

    fn said_in(&self, manager: &Manager, task_id: &str) -> Result<Arc<Vec<Said>>> {
        let next = manager.store().next_seq(task_id)?;
        if let Some((built_at, list)) = self.said.lock().unwrap().get(task_id)
            && *built_at == next
        {
            return Ok(list.clone());
        }
        let list = Arc::new(said(&manager.store().events(task_id)?));
        self.said.lock().unwrap().insert(task_id.to_string(), (next, list.clone()));
        Ok(list)
    }

    /// Things said in any open task that contain `query`, newest first.
    pub fn chats(&self, manager: &Manager, query: &str, limit: usize) -> Result<Vec<ChatHit>> {
        let needle = query.to_lowercase();
        if needle.trim().is_empty() {
            return Ok(vec![]);
        }
        let needle_chars = needle.chars().count();
        let mut hits = Vec::new();
        for task in manager.list()? {
            for message in self.said_in(manager, &task.row.id)?.iter() {
                // A message is searched as one line, so a match can span its line breaks.
                let flat = message.text.replace(['\n', '\r'], " ");
                if let Some(at) = find_in_line(&flat, &needle) {
                    let (text, column) = window(&flat, at, needle_chars);
                    hits.push(ChatHit {
                        task_id: task.row.id.clone(),
                        who: message.who,
                        at: message.at,
                        column,
                        length: needle_chars,
                        text,
                    });
                }
            }
        }
        hits.sort_by_key(|hit| std::cmp::Reverse(hit.at));
        hits.truncate(limit);
        Ok(hits)
    }
}

/// Puts a task's events together into the things that were said.
/// An agent's answer arrives in pieces; pieces in a row are one message.
pub fn said(events: &[Value]) -> Vec<Said> {
    let mut list: Vec<Said> = Vec::new();
    let mut agent_open = false;
    for event in events {
        let at = event.get("at").and_then(Value::as_u64).unwrap_or(0);
        match event.get("type").and_then(Value::as_str) {
            Some("user_message") => {
                agent_open = false;
                if let Some(text) = event.get("text").and_then(Value::as_str) {
                    list.push(Said { who: "you", at, text: text.to_string() });
                }
            }
            Some("update") => {
                let update = &event["update"];
                let chunk = update.get("sessionUpdate").and_then(Value::as_str) == Some("agent_message_chunk");
                let text = update.get("content").and_then(|c| c.get("text")).and_then(Value::as_str);
                match (chunk, text) {
                    (true, Some(text)) if agent_open => {
                        if let Some(last) = list.last_mut() {
                            last.text.push_str(text);
                        }
                    }
                    (true, Some(text)) => {
                        list.push(Said { who: "agent", at, text: text.to_string() });
                        agent_open = true;
                    }
                    _ => agent_open = false,
                }
            }
            // A change of state between two pieces does not split a message.
            Some("state") => {}
            _ => agent_open = false,
        }
    }
    list
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn sandbox(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ae-search-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("src/middleware")).unwrap();
        std::fs::create_dir_all(dir.join("config")).unwrap();
        std::fs::create_dir_all(dir.join("node_modules/pkg")).unwrap();
        std::fs::create_dir_all(dir.join(".git")).unwrap();
        std::fs::write(dir.join(".gitignore"), "node_modules/\n").unwrap();
        std::fs::write(dir.join(".git/config"), "limit\n").unwrap();
        std::fs::write(dir.join("src/middleware/rateLimit.ts"), "export const LIMIT = 5;\n// the limit\n").unwrap();
        std::fs::write(dir.join("config/limits.ts"), "export const limits = { login: 5 };\n").unwrap();
        std::fs::write(dir.join("node_modules/pkg/limit.js"), "limit\n").unwrap();
        std::fs::write(dir.join("README.md"), "Nothing here.\n").unwrap();
        std::fs::write(dir.join("logo.bin"), [0u8, 1, 2, b'l', b'i', b'm', b'i', b't']).unwrap();
        dir
    }

    #[test]
    fn a_name_match_beats_a_path_match_beats_scattered_letters() {
        let in_name = match_path("src/middleware/rateLimit.ts", "limit").unwrap();
        let in_path = match_path("limits/index.ts", "limit").unwrap();
        let scattered = match_path("lib/main/index.test.ts", "limit").unwrap();
        assert!(in_name.0 > in_path.0 && in_path.0 > scattered.0);
        assert_eq!(in_name.1, vec![19, 20, 21, 22, 23]);
        assert_eq!(match_path("src/a.ts", "zzz"), None);
        assert_eq!(match_path("src/a.ts", "  "), None);
    }

    #[test]
    fn finds_files_by_name_and_skips_what_git_ignores() {
        let root = sandbox("files");
        let search = Search::new();
        let paths: Vec<_> = search.files(&root, "limit", 10).into_iter().map(|h| h.path).collect();
        assert_eq!(paths, vec!["config/limits.ts", "src/middleware/rateLimit.ts"]);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn finds_text_in_any_case_and_skips_binary_and_ignored_files() {
        let root = sandbox("text");
        let search = Search::new();
        let hits = search.text(&root, "Limit", 50);
        let places: Vec<_> = hits.iter().map(|h| (h.path.as_str(), h.line, h.column)).collect();
        assert_eq!(
            places,
            vec![
                ("config/limits.ts", 1, 13),
                ("src/middleware/rateLimit.ts", 1, 13),
                ("src/middleware/rateLimit.ts", 2, 7),
            ]
        );
        assert_eq!(search.text(&root, "limit", 2).len(), 2, "the limit is respected");
        assert!(search.text(&root, "   ", 10).is_empty());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn long_lines_are_cut_around_the_match() {
        let line = format!("{}needle{}", "a".repeat(500), "b".repeat(500));
        let (text, column) = window(&line, 500, 6);
        assert!(text.chars().count() <= MAX_LINE_CHARS + 2);
        assert_eq!(text.chars().skip(column).take(6).collect::<String>(), "needle");
        assert!(text.starts_with('…') && text.ends_with('…'));
    }

    #[test]
    fn pieces_of_an_answer_become_one_message() {
        let chunk = |text: &str, at: u64| json!({ "type": "update", "at": at, "update": { "sessionUpdate": "agent_message_chunk", "content": { "type": "text", "text": text } } });
        let events = vec![
            json!({ "type": "user_message", "at": 1, "text": "Raise the limit" }),
            json!({ "type": "state", "at": 2, "state": "running" }),
            chunk("I can add one with ", 3),
            chunk("a higher limit.", 4),
            json!({ "type": "update", "at": 5, "update": { "sessionUpdate": "tool_call", "toolCallId": "t1" } }),
            chunk("Done.", 6),
        ];
        assert_eq!(
            said(&events),
            vec![
                Said { who: "you", at: 1, text: "Raise the limit".into() },
                Said { who: "agent", at: 3, text: "I can add one with a higher limit.".into() },
                Said { who: "agent", at: 6, text: "Done.".into() },
            ]
        );
    }
}
