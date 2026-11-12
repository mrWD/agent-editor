//! The Changes view of a task: its git state joined with what the core knows
//! about who wrote which line.
//!
//! Like `review` and `gitops`, everything here blocks. Call it from `spawn_blocking`.

use crate::gitops::{self, BranchState, Pick};
use crate::marks::{self, Author};
use crate::tasks::Manager;
use anyhow::{Result, bail};
use serde_json::{Value, json};
use std::path::PathBuf;

impl Manager {
    /// The working copy of a task and the branch it started from.
    fn git_place(&self, task_id: &str) -> Result<(PathBuf, Option<String>)> {
        let task = self.store().task(task_id)?;
        if task.branch.is_none() {
            bail!("This task works in a folder without git, so there is nothing to commit here");
        }
        Ok((PathBuf::from(task.cwd), task.base_branch))
    }

    /// Tells every interface that the git state of a task changed, after looking
    /// at the files again: a revert or an update changes them on disk.
    fn after_git(&self, task_id: &str) {
        if let Err(error) = self.sweep(task_id, Author::You, &[]) {
            tracing::debug!("Could not look at the files of {task_id}: {error:#}");
        }
        self.broadcast("git.changed", json!({ "taskId": task_id }));
    }

    fn note_git(&self, task_id: &str, action: &str, subject: &str, short: &str) {
        self.emit(task_id, json!({ "type": "git", "action": action, "subject": subject, "short": short }));
    }

    pub fn git_state(&self, task_id: &str) -> Result<BranchState> {
        let (root, base) = self.git_place(task_id)?;
        gitops::state(&root, base.as_deref())
    }

    /// One file's changes, each with who wrote its new lines when that is known.
    pub fn git_diff(&self, task_id: &str, path: &str) -> Result<Value> {
        let (root, _) = self.git_place(task_id)?;
        let diff = gitops::file_diff(&root, path)?;
        let _ = self.observe(task_id, path, self.acting_now(task_id), None);
        let authors = self.store().marks(task_id, path)?.map(|row| marks::decode(&row.authors)).unwrap_or_default();

        let mut value = json!(diff);
        if let Some(hunks) = value.get_mut("hunks").and_then(Value::as_array_mut) {
            for (hunk, parsed) in hunks.iter_mut().zip(&diff.hunks) {
                let mut agent = false;
                let mut you = false;
                for line in parsed.lines.iter().filter(|line| line.kind == "add") {
                    match line.new_no.and_then(|n| authors.get(n - 1)) {
                        Some(Author::Agent) => agent = true,
                        Some(Author::You) => you = true,
                        _ => {}
                    }
                }
                hunk["who"] = match (agent, you) {
                    (true, true) => json!("both"),
                    (true, false) => json!("agent"),
                    (false, true) => json!("you"),
                    (false, false) => Value::Null,
                };
            }
        }
        Ok(value)
    }

    /// Commits the picked changes; with `push`, sends the branch to the remote after.
    pub fn git_commit(
        &self,
        task_id: &str,
        message: &str,
        picks: &[Pick],
        amend: bool,
        push: bool,
    ) -> Result<BranchState> {
        let (root, base) = self.git_place(task_id)?;
        let made = gitops::commit(&root, message, picks, amend)?;
        // What is committed whole has been looked at: its lines stop being marked.
        for pick in picks.iter().filter(|pick| pick.changes.is_none()) {
            let _ = self.accept(task_id, &pick.path);
        }
        self.note_git(task_id, "committed", &made.subject, &made.short);
        let pushed = if push { gitops::push(&root) } else { Ok(()) };
        if push && pushed.is_ok() {
            self.note_git(task_id, "pushed", &made.subject, &made.short);
        }
        self.after_git(task_id);
        // The commit stands even when the push after it failed; say so instead of hiding it.
        if let Err(error) = pushed {
            bail!("The commit was made, but the push failed: {error}");
        }
        gitops::state(&root, base.as_deref())
    }

    pub fn git_push(&self, task_id: &str) -> Result<()> {
        let (root, base) = self.git_place(task_id)?;
        gitops::push(&root)?;
        let newest = gitops::state(&root, base.as_deref())?.commits.into_iter().next();
        if let Some(commit) = newest {
            self.note_git(task_id, "pushed", &commit.subject, &commit.short);
        }
        self.after_git(task_id);
        Ok(())
    }

    pub fn git_undo_commit(&self, task_id: &str, hash: &str) -> Result<()> {
        let (root, base) = self.git_place(task_id)?;
        let undone = gitops::state(&root, base.as_deref())?.commits.into_iter().find(|c| c.hash == hash);
        if undone.as_ref().is_some_and(|c| c.pushed) {
            bail!("This commit is already pushed. Take it back with Revert, which adds a new commit.");
        }
        gitops::undo_commit(&root, hash)?;
        if let Some(commit) = undone {
            self.note_git(task_id, "undid", &commit.subject, &commit.short);
        }
        self.after_git(task_id);
        Ok(())
    }

    pub fn git_revert_commit(&self, task_id: &str, hash: &str) -> Result<()> {
        let (root, _) = self.git_place(task_id)?;
        let made = gitops::revert_commit(&root, hash)?;
        self.note_git(task_id, "committed", &made.subject, &made.short);
        self.after_git(task_id);
        Ok(())
    }

    /// Puts a file, or one change of it, back as it is in the last commit.
    pub fn git_revert(&self, task_id: &str, path: &str, change: Option<usize>) -> Result<()> {
        let (root, _) = self.git_place(task_id)?;
        match change {
            Some(index) => gitops::revert_change(&root, path, index)?,
            None => gitops::revert_file(&root, path)?,
        }
        self.after_git(task_id);
        Ok(())
    }

    pub fn git_update(&self, task_id: &str) -> Result<()> {
        let (root, base) = self.git_place(task_id)?;
        let Some(base) = gitops::resolve_base(&root, base.as_deref()) else {
            bail!("It is not known which branch this task started from");
        };
        gitops::update_from_base(&root, &base)?;
        self.note_git(task_id, "updated", &base, "");
        self.after_git(task_id);
        Ok(())
    }
}
