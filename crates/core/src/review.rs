//! Review: what changed in a task's files, who changed it, and taking it back.
//!
//! The core looks at files at known moments: when the user saves from the
//! editor, when an agent finishes a tool call or a turn, and before a new
//! message goes to the agent. Whoever was acting at that moment is the author
//! of the lines that changed. `marks` does the line arithmetic; this file reads
//! the disk, asks git for the starting point and stores the result.
//!
//! Everything here touches the disk and may run git, so it blocks. Call it
//! from `spawn_blocking`.

use crate::marks::{self, Author, Run};
use crate::store::MarksRow;
use crate::tasks::{Manager, TaskState};
use anyhow::{Context, Result, bail};
use serde::Serialize;
use serde_json::json;
use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

/// Files larger than this are not tracked line by line.
const MAX_TRACKED_BYTES: u64 = 2 * 1024 * 1024;

/// How many changed files one sweep looks at. A task that changes more than
/// this at once is doing something line marks would not help with.
const MAX_SWEEP: usize = 400;

/// What the editor needs to mark the lines of one file.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct FileReview {
    pub path: String,
    /// Stretches of lines that are not as they were at the last review.
    pub runs: Vec<Run>,
    pub agent_lines: usize,
    pub your_lines: usize,
    /// False when the file was deleted.
    pub exists: bool,
    /// False when the file did not exist at the last review.
    pub existed: bool,
}

impl FileReview {
    fn clean(path: &str, exists: bool) -> Self {
        Self { path: path.to_string(), runs: vec![], agent_lines: 0, your_lines: 0, exists, existed: exists }
    }

    fn of(path: &str, row: &MarksRow) -> Self {
        let authors = marks::decode(&row.authors);
        let exists = row.seen.is_some();
        Self {
            path: path.to_string(),
            // A deleted file has no lines to mark; its one letter says who deleted it.
            runs: if exists { marks::runs(&authors) } else { vec![] },
            agent_lines: marks::count(&authors, Author::Agent),
            your_lines: marks::count(&authors, Author::You),
            exists,
            existed: row.base.is_some(),
        }
    }

    pub fn is_changed(&self) -> bool {
        self.agent_lines > 0 || self.your_lines > 0 || self.exists != self.existed
    }
}

/// Turns a path an agent or the interface named into one relative to `root`,
/// with forward slashes. `None` when the path is outside `root`.
pub fn relative_to(root: &Path, named: &str) -> Option<String> {
    let named_path = Path::new(named);
    let inside: PathBuf = if named_path.is_absolute() {
        match named_path.strip_prefix(root) {
            Ok(rest) => rest.to_path_buf(),
            Err(_) => {
                // The same folder may be spelled two ways (links, /tmp on macOS).
                let parent = named_path.parent()?.canonicalize().ok()?;
                let real = parent.join(named_path.file_name()?);
                real.strip_prefix(root.canonicalize().ok()?).ok()?.to_path_buf()
            }
        }
    } else {
        named_path.to_path_buf()
    };
    let mut parts = Vec::new();
    for component in inside.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_string_lossy().into_owned()),
            Component::CurDir => {}
            _ => return None,
        }
    }
    if parts.is_empty() { None } else { Some(parts.join("/")) }
}

/// The text of a file, or why it cannot be tracked.
enum OnDisk {
    Missing,
    Text(String),
    /// Too large or not text.
    Untracked,
}

fn read_disk(root: &Path, path: &str) -> OnDisk {
    let full = root.join(path);
    let Ok(meta) = std::fs::metadata(&full) else {
        return OnDisk::Missing;
    };
    if !meta.is_file() || meta.len() > MAX_TRACKED_BYTES {
        return OnDisk::Untracked;
    }
    match std::fs::read(&full).map(String::from_utf8) {
        Ok(Ok(text)) => OnDisk::Text(text),
        Ok(Err(_)) => OnDisk::Untracked,
        Err(_) => OnDisk::Missing,
    }
}

/// The text of a file in the last commit. `Ok(None)` when the commit has no such file.
fn committed_text(root: &Path, path: &str) -> Result<Option<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["show", &format!("HEAD:./{path}")])
        .output()
        .context("Could not run git")?;
    if !output.status.success() {
        return Ok(None);
    }
    match String::from_utf8(output.stdout) {
        Ok(text) => Ok(Some(text)),
        Err(_) => bail!("Not a text file"),
    }
}

/// The paths git reports as changed or new in a working copy.
fn changed_in_git(root: &Path) -> Result<Vec<String>> {
    let output = Command::new("git")
        .arg("-C")
        .arg(root)
        .args(["status", "--porcelain=v1", "-z", "--untracked-files=all"])
        .output()
        .context("Could not run git")?;
    if !output.status.success() {
        bail!("git status failed: {}", String::from_utf8_lossy(&output.stderr).trim());
    }
    let text = String::from_utf8_lossy(&output.stdout);
    let mut paths = Vec::new();
    let mut entries = text.split('\0');
    while let Some(entry) = entries.next() {
        if entry.len() < 4 {
            continue;
        }
        let (status, path) = entry.split_at(3);
        paths.push(path.to_string());
        // A rename or copy is followed by the old name as its own entry.
        if (status.starts_with('R') || status.starts_with('C'))
            && let Some(old) = entries.next()
        {
            paths.push(old.to_string());
        }
    }
    Ok(paths)
}

impl Manager {
    /// Who is changing files in a task right now, when nobody says so.
    pub fn acting_now(&self, task_id: &str) -> Author {
        match self.state_of(task_id) {
            TaskState::Running | TaskState::WaitingPermission => Author::Agent,
            TaskState::Idle | TaskState::Asleep => Author::You,
        }
    }

    /// Looks at one file on disk and records who wrote its new lines.
    ///
    /// `before` is the file's text before the change, when the caller knows it
    /// (`Some(None)`: there was no file). Without it the first look at a file
    /// compares against the last commit. Returns true when something changed.
    pub fn observe(&self, task_id: &str, path: &str, who: Author, before: Option<Option<String>>) -> Result<bool> {
        let task = self.store().task(task_id)?;
        let root = PathBuf::from(&task.cwd);
        let current = match read_disk(&root, path) {
            OnDisk::Missing => None,
            OnDisk::Text(text) => Some(text),
            OnDisk::Untracked => return Ok(false),
        };

        let (base, seen, old_authors) = match self.store().marks(task_id, path)? {
            Some(row) => {
                let authors = marks::decode(&row.authors);
                (row.base, row.seen, authors)
            }
            None => {
                let start = match before {
                    Some(text) => text,
                    None if task.branch.is_some() => match committed_text(&root, path) {
                        Ok(text) => text,
                        Err(_) => return Ok(false),
                    },
                    // Not a git project and nobody knows the earlier text:
                    // there is nothing to compare against.
                    None => return Ok(false),
                };
                (start.clone(), start, vec![])
            }
        };
        if seen == current {
            return Ok(false);
        }

        let authors = match (&seen, &current) {
            (Some(old), Some(new)) => marks::carry(old, &old_authors, new, who),
            (None, Some(new)) => vec![who; marks::lines(new).len()],
            // A deleted file keeps one letter: who deleted it.
            (_, None) => vec![who],
        };
        let authors = match (&base, &current) {
            (None, None) => vec![],
            // Lines that read as they did at the last review are nobody's change.
            (Some(before), Some(now)) => marks::settle(before, now, authors),
            _ => authors,
        };
        self.store().put_marks(task_id, path, &MarksRow { base, seen: current, authors: marks::encode(&authors) })?;
        Ok(true)
    }

    /// Looks at every file that may have changed and credits `who`.
    /// `also` are extra paths worth a look, for example those an agent named.
    pub fn sweep(&self, task_id: &str, who: Author, also: &[String]) -> Result<Vec<String>> {
        let task = self.store().task(task_id)?;
        let root = PathBuf::from(&task.cwd);
        let mut paths: BTreeSet<String> = also.iter().cloned().collect();
        for (path, ..) in self.store().marked_files(task_id)? {
            paths.insert(path);
        }
        if task.branch.is_some() {
            match changed_in_git(&root) {
                Ok(list) => paths.extend(list),
                Err(error) => tracing::debug!("Could not ask git what changed in {task_id}: {error:#}"),
            }
        }
        let mut changed = Vec::new();
        for path in paths.into_iter().take(MAX_SWEEP) {
            match self.observe(task_id, &path, who, None) {
                Ok(true) => changed.push(path),
                Ok(false) => {}
                Err(error) => tracing::debug!("Could not look at {path}: {error:#}"),
            }
        }
        self.announce_review(task_id, &changed);
        Ok(changed)
    }

    /// Tells every interface that the marks of these files changed.
    pub fn announce_review(&self, task_id: &str, paths: &[String]) {
        if !paths.is_empty() {
            self.broadcast("review.changed", json!({ "taskId": task_id, "paths": paths }));
        }
    }

    /// The marks of one file as last recorded.
    pub fn review_of(&self, task_id: &str, path: &str) -> Result<FileReview> {
        Ok(match self.store().marks(task_id, path)? {
            Some(row) => FileReview::of(path, &row),
            None => {
                let root = self.cwd(task_id)?;
                FileReview::clean(path, root.join(path).is_file())
            }
        })
    }

    /// Every file of a task that is not as it was at the last review.
    pub fn changed_files(&self, task_id: &str) -> Result<Vec<FileReview>> {
        self.store().task(task_id)?;
        let mut list = Vec::new();
        for (path, authors, existed, exists) in self.store().marked_files(task_id)? {
            let authors = marks::decode(&authors);
            let review = FileReview {
                runs: vec![],
                agent_lines: marks::count(&authors, Author::Agent),
                your_lines: marks::count(&authors, Author::You),
                exists,
                existed,
                path,
            };
            if review.is_changed() {
                list.push(review);
            }
        }
        Ok(list)
    }

    /// Accepts a file as it is now: the agent's lines stop being marked.
    pub fn keep(&self, task_id: &str, path: &str) -> Result<FileReview> {
        self.observe(task_id, path, self.acting_now(task_id), None)?;
        let Some(row) = self.store().marks(task_id, path)? else {
            return self.review_of(task_id, path);
        };
        let authors: Vec<Author> = if row.seen.is_some() {
            marks::decode(&row.authors).into_iter().map(|a| if a == Author::Agent { Author::Base } else { a }).collect()
        } else {
            vec![]
        };
        let kept = MarksRow { base: row.seen.clone(), seen: row.seen, authors: marks::encode(&authors) };
        self.store().put_marks(task_id, path, &kept)?;
        self.announce_review(task_id, &[path.to_string()]);
        Ok(FileReview::of(path, &kept))
    }

    /// Accepts a file whole, the user's own lines included: after a commit
    /// nothing in it is waiting for anyone.
    pub fn accept(&self, task_id: &str, path: &str) -> Result<()> {
        self.observe(task_id, path, self.acting_now(task_id), None)?;
        if let Some(row) = self.store().marks(task_id, path)? {
            let lines = row.seen.as_deref().map(|text| marks::lines(text).len()).unwrap_or(0);
            let accepted =
                MarksRow { base: row.seen.clone(), seen: row.seen, authors: marks::encode(&vec![Author::Base; lines]) };
            self.store().put_marks(task_id, path, &accepted)?;
            self.announce_review(task_id, &[path.to_string()]);
        }
        Ok(())
    }

    /// Takes back what the agent changed in a file since the last review.
    /// Blocks of lines the user changed stay as they are.
    pub fn undo(&self, task_id: &str, path: &str) -> Result<FileReview> {
        self.observe(task_id, path, self.acting_now(task_id), None)?;
        let Some(row) = self.store().marks(task_id, path)? else {
            bail!("Nothing in this file is waiting for review");
        };
        let full = self.cwd(task_id)?.join(path);
        let authors = marks::decode(&row.authors);

        let after = match (&row.base, &row.seen) {
            // The file was deleted: put it back, unless the user deleted it.
            (Some(base), None) => {
                if authors.first() == Some(&Author::You) {
                    bail!("You deleted this file yourself, so there is nothing of the agent's to undo");
                }
                write_file(&full, base)?;
                MarksRow {
                    base: row.base.clone(),
                    seen: row.base.clone(),
                    authors: marks::encode(&vec![Author::Base; marks::lines(base).len()]),
                }
            }
            (base, Some(seen)) => {
                let had_agent_lines = marks::count(&authors, Author::Agent) > 0;
                let (text, authors) = marks::undo_agent(base.as_deref().unwrap_or(""), seen, &authors);
                if had_agent_lines && text == *seen {
                    bail!(
                        "The agent's lines here are mixed with lines you changed, so they cannot be taken back on their own. \
                         Change them by hand, or keep them."
                    );
                }
                // A file the agent made, with nothing of the user's in it, goes away again.
                if base.is_none() && authors.is_empty() {
                    std::fs::remove_file(&full).with_context(|| format!("Could not remove {path}"))?;
                    MarksRow { base: None, seen: None, authors: String::new() }
                } else {
                    write_file(&full, &text)?;
                    MarksRow { base: row.base.clone(), seen: Some(text), authors: marks::encode(&authors) }
                }
            }
            (None, None) => row.clone(),
        };
        self.store().put_marks(task_id, path, &after)?;
        self.announce_review(task_id, &[path.to_string()]);
        Ok(FileReview::of(path, &after))
    }
}

fn write_file(full: &Path, text: &str) -> Result<()> {
    if let Some(parent) = full.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(full, text).with_context(|| format!("Could not write {}", full.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_made_relative_and_kept_inside() {
        let root = std::env::temp_dir().join(format!("ae-review-{}", std::process::id()));
        std::fs::create_dir_all(root.join("src")).unwrap();
        let root = root.canonicalize().unwrap();
        let inside = root.join("src").join("a.ts");

        assert_eq!(relative_to(&root, &inside.to_string_lossy()).as_deref(), Some("src/a.ts"));
        assert_eq!(relative_to(&root, "src/a.ts").as_deref(), Some("src/a.ts"));
        assert_eq!(relative_to(&root, "./src/a.ts").as_deref(), Some("src/a.ts"));
        assert_eq!(relative_to(&root, "../elsewhere.ts"), None);
        let outside = root.parent().unwrap().join("elsewhere.ts");
        assert_eq!(relative_to(&root, &outside.to_string_lossy()), None);
        assert_eq!(relative_to(&root, ""), None);
        std::fs::remove_dir_all(root).unwrap();
    }
}
