//! Finding lines that two open tasks of one project both change.
//!
//! Each task works in its own copy, so two tasks can change the same line and
//! nobody notices until the second of them is merged and gets the conflict.
//! Here the copies are compared while both tasks are still open: every change
//! of this task against every change of the others, each counted from the
//! commit both branches come from. Two changes overlap when they touch the
//! same lines or lines next to each other, which is when git cannot join them.
//!
//! Nothing is asked of any model, and nothing is changed: this only tells.
//!
//! Everything here blocks. Call it from `spawn_blocking`.

use crate::gitops::{self, Stretch};
use crate::tasks::Manager;
use anyhow::Result;
use serde_json::{Value, json};
use std::collections::HashSet;
use std::path::PathBuf;

/// How many lines of each side are sent along, at most.
const SHOWN_LINES: usize = 12;

/// True when two changes to the same file, counted from the same text, touch
/// the same lines or lines with nothing unchanged between them.
pub fn touch(a: &Stretch, b: &Stretch) -> bool {
    // A change that only adds lines has no length: it sits between two lines.
    let end = |s: &Stretch| (s.old_start + s.old_len) as i64 - 1;
    (a.old_start as i64) <= end(b) + 1 && (b.old_start as i64) <= end(a) + 1
}

fn dismissed_key(task_id: &str) -> String {
    format!("task.{task_id}.overlapsOk")
}

/// What one overlap is, in a few words that stay the same while it does.
fn mark(path: &str, other: &str, mine: &Stretch, theirs: &Stretch) -> String {
    format!("{path}|{other}|{}|{}", mine.added.join("\n"), theirs.added.join("\n"))
}

impl Manager {
    fn dismissed_overlaps(&self, task_id: &str) -> HashSet<String> {
        self.store()
            .setting(&dismissed_key(task_id))
            .ok()
            .flatten()
            .and_then(|text| serde_json::from_str::<Vec<String>>(&text).ok())
            .map(|list| list.into_iter().collect())
            .unwrap_or_default()
    }

    /// The lines of a task that another open task of the same project changes too.
    pub fn overlaps(&self, task_id: &str) -> Result<Value> {
        let task = self.store().task(task_id)?;
        if task.branch.is_none() {
            return Ok(json!({ "overlaps": [] }));
        }
        let root = PathBuf::from(&task.cwd);
        let (Some(head), Ok(tree)) = (gitops::commit_of(&root, "HEAD"), gitops::snapshot_tree(&root)) else {
            return Ok(json!({ "overlaps": [] }));
        };
        let dismissed = self.dismissed_overlaps(task_id);
        let mut found = Vec::new();

        for other in self.store().tasks()? {
            if other.id == task.id
                || other.project_id != task.project_id
                || other.branch.is_none()
                || other.cwd == task.cwd
            {
                continue;
            }
            let other_root = PathBuf::from(&other.cwd);
            let (Some(other_head), Ok(other_tree)) =
                (gitops::commit_of(&other_root, "HEAD"), gitops::snapshot_tree(&other_root))
            else {
                continue;
            };
            // Both copies belong to one repository, so each can be read from the other's folder.
            let Some(start) = gitops::common_start(&root, &head, &other_head) else { continue };
            let (Ok(mine), Ok(theirs)) = (
                gitops::stretches_between(&root, &start, &tree),
                gitops::stretches_between(&root, &start, &other_tree),
            ) else {
                continue;
            };
            for (path, my_changes) in &mine {
                let Some(their_changes) = theirs.get(path) else { continue };
                for my in my_changes {
                    for their in their_changes.iter().filter(|their| touch(my, their)) {
                        // The same change on both sides is no conflict: git takes it once.
                        if my.added == their.added && my.removed == their.removed {
                            continue;
                        }
                        let mark = mark(path, &other.id, my, their);
                        if dismissed.contains(&mark) {
                            continue;
                        }
                        let lines = |list: &[String]| list.iter().take(SHOWN_LINES).cloned().collect::<Vec<_>>();
                        found.push(json!({
                            "mark": mark,
                            "path": path,
                            "line": my.new_start.max(1),
                            "lines": my.added.len().max(1),
                            "mine": lines(&my.added),
                            "theirs": lines(&their.added),
                            "before": lines(&my.removed),
                            "theirLine": their.new_start.max(1),
                            "task": { "id": other.id, "title": other.title, "agentId": other.agent_id },
                        }));
                    }
                }
            }
        }
        found.sort_by(|a, b| (a["path"].as_str(), a["line"].as_u64()).cmp(&(b["path"].as_str(), b["line"].as_u64())));
        Ok(json!({ "overlaps": found }))
    }

    /// Remembers that the user looked at an overlap and it is fine. It is not shown again while it stays the same.
    pub fn dismiss_overlap(&self, task_id: &str, mark: &str) -> Result<()> {
        self.store().task(task_id)?;
        let mut all = self.dismissed_overlaps(task_id);
        all.insert(mark.to_string());
        let list: Vec<&String> = all.iter().collect();
        self.store().set_setting(&dismissed_key(task_id), Some(&serde_json::to_string(&list)?))?;
        self.broadcast("git.changed", json!({ "taskId": task_id }));
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn change(old_start: usize, old_len: usize) -> Stretch {
        Stretch { old_start, old_len, new_start: old_start, added: vec![], removed: vec![] }
    }

    #[test]
    fn changes_touch_when_they_share_lines_or_sit_next_to_each_other() {
        // The same line, and lines inside one another.
        assert!(touch(&change(12, 1), &change(12, 1)));
        assert!(touch(&change(10, 5), &change(12, 1)));
        // Next to each other, with no unchanged line between: git cannot join these.
        assert!(touch(&change(12, 1), &change(13, 1)));
        assert!(touch(&change(13, 2), &change(12, 1)));
        // One unchanged line between them is enough.
        assert!(!touch(&change(12, 1), &change(14, 1)));
        assert!(!touch(&change(1, 3), &change(20, 1)));
    }

    #[test]
    fn lines_put_in_touch_what_is_right_before_and_after_them() {
        // Lines put in after line 5 sit at 6 and take nothing out.
        let put_in = change(6, 0);
        assert!(touch(&put_in, &change(5, 1)), "the line right before");
        assert!(touch(&put_in, &change(6, 1)), "the line right after");
        assert!(touch(&put_in, &change(6, 0)), "more lines put in at the same place");
        assert!(!touch(&put_in, &change(7, 1)), "line 6 stays between them");
        assert!(!touch(&put_in, &change(4, 1)), "line 5 stays between them");
    }
}
