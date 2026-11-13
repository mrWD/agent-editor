//! Making, renaming and deleting files of a task by hand, from the file tree.
//!
//! Each of these is the user's own change, so it is treated like a save from
//! the editor: a checkpoint is taken first, the files are looked at again for
//! "who wrote what", and the agent is told with the user's next message.
//!
//! A deleted file is not destroyed. It is moved to the editor's own trash
//! (in the data folder) and can be put back, also when git never knew it.
//!
//! Everything here blocks. Call it from `spawn_blocking`.

use crate::files;
use crate::marks::Author;
use crate::tasks::Manager;
use anyhow::{Result, anyhow};
use serde_json::{Value, json};
use std::path::PathBuf;

/// How long a deleted file is kept before the core throws it away for good.
const KEPT_FOR_MS: u64 = 30 * 24 * 60 * 60 * 1000;

impl Manager {
    fn trash_root(&self) -> PathBuf {
        self.data_root().join("trash")
    }

    /// Looks at the given files again and tells every window and, later, the agent.
    fn after_file_change(&self, task_id: &str, paths: &[String]) {
        match self.sweep(task_id, Author::You, paths) {
            Ok(changed) => self.announce_review(task_id, &changed),
            Err(error) => tracing::debug!("Could not look at the files of {task_id}: {error:#}"),
        }
        // The tree and the Changes view of every window read the files again.
        self.broadcast("files.changed", json!({ "taskId": task_id, "paths": paths }));
        self.broadcast("git.changed", json!({ "taskId": task_id }));
    }

    /// Makes an empty file or a folder in a task.
    pub fn file_create(&self, task_id: &str, path: &str, folder: bool) -> Result<Value> {
        let root = self.cwd(task_id)?;
        self.checkpoint_before_edit(task_id);
        let made = files::create(&root, path, folder)?;
        if !folder {
            self.note_manual_quietly(task_id, &made);
        }
        self.emit(task_id, json!({ "type": "user_file", "action": "created", "path": made, "folder": folder }));
        self.after_file_change(task_id, std::slice::from_ref(&made));
        Ok(json!({ "path": made }))
    }

    /// Gives a file or folder of a task a new name or place.
    pub fn file_rename(&self, task_id: &str, from: &str, to: &str) -> Result<Value> {
        let root = self.cwd(task_id)?;
        self.checkpoint_before_edit(task_id);
        // What is known about each file is brought up to date before it moves,
        // so its lines keep their authors under the new name.
        for old in files::files_of(&root, from)? {
            let _ = self.observe(task_id, &old, self.acting_now(task_id), None);
        }
        let moved = files::rename(&root, from, to)?;
        let mut paths = Vec::new();
        for (old, new) in &moved {
            if let Some(row) = self.store().marks(task_id, old)? {
                // The file is the same file under a new name: what was known of it goes with it.
                self.store().put_marks(task_id, new, &row)?;
                self.store().remove_marks(task_id, old)?;
            }
            self.note_manual_quietly(task_id, new);
            paths.push(old.clone());
            paths.push(new.clone());
        }
        let to_shown = moved.first().map_or(to.to_string(), |(old, new)| {
            // For a folder the files moved; the folder's own new name is what the user gave.
            if moved.len() == 1 && old == from { new.clone() } else { to.trim_matches('/').to_string() }
        });
        self.emit(task_id, json!({ "type": "user_file", "action": "renamed", "path": from, "to": to_shown }));
        self.after_file_change(task_id, &paths);
        Ok(
            json!({ "moved": moved.iter().map(|(old, new)| json!({ "from": old, "to": new })).collect::<Vec<_>>(), "to": to_shown }),
        )
    }

    /// Takes a file or folder out of a task. It is kept and can be put back with `file_restore`.
    pub fn file_delete(&self, task_id: &str, path: &str) -> Result<Value> {
        let root = self.cwd(task_id)?;
        self.checkpoint_before_edit(task_id);
        let kept_id = uuid::Uuid::new_v4().simple().to_string();
        let place = self.trash_root().join(task_id).join(&kept_id);
        let (gone, _) = files::remove(&root, path, &place)?;
        for file in &gone {
            self.note_manual_quietly(task_id, file);
        }
        self.emit(
            task_id,
            json!({ "type": "user_file", "action": "deleted", "path": path, "files": gone.len(), "keptAs": kept_id }),
        );
        self.after_file_change(task_id, &gone);
        Ok(json!({ "files": gone, "keptAs": kept_id }))
    }

    /// Puts back what `file_delete` took out.
    pub fn file_restore(&self, task_id: &str, kept_id: &str) -> Result<Value> {
        let root = self.cwd(task_id)?;
        if kept_id.is_empty() || !kept_id.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(anyhow!("No such deleted file"));
        }
        // The event that recorded the deletion says where the thing was.
        let events = self.store().events(task_id)?;
        let path = events
            .iter()
            .rev()
            .find(|event| event["type"] == "user_file" && event["keptAs"] == kept_id)
            .and_then(|event| event["path"].as_str())
            .ok_or_else(|| anyhow!("No such deleted file"))?
            .to_string();
        let place = self.trash_root().join(task_id).join(kept_id);
        let kept = std::fs::read_dir(&place)
            .ok()
            .and_then(|mut entries| entries.next())
            .and_then(Result::ok)
            .map(|entry| entry.path())
            .ok_or_else(|| anyhow!("What was deleted is no longer kept"))?;
        self.checkpoint_before_edit(task_id);
        let back = files::restore(&root, &path, &kept)?;
        let _ = std::fs::remove_dir(&place);
        for file in &back {
            self.note_manual_quietly(task_id, file);
        }
        self.emit(
            task_id,
            json!({ "type": "user_file", "action": "restored", "path": path, "files": back.len(), "keptAs": kept_id }),
        );
        self.after_file_change(task_id, &back);
        Ok(json!({ "path": path, "files": back }))
    }

    /// Throws away deleted files that have been kept long enough. Called when the core starts.
    pub fn empty_old_trash(&self) {
        let Ok(tasks) = std::fs::read_dir(self.trash_root()) else { return };
        let now = std::time::SystemTime::now();
        for task in tasks.flatten() {
            let Ok(kept) = std::fs::read_dir(task.path()) else { continue };
            for item in kept.flatten() {
                let old = item
                    .metadata()
                    .and_then(|meta| meta.modified())
                    .ok()
                    .and_then(|at| now.duration_since(at).ok())
                    .is_some_and(|age| age.as_millis() as u64 > KEPT_FOR_MS);
                if old {
                    let _ = std::fs::remove_dir_all(item.path());
                }
            }
        }
    }
}
