//! The plan of a task: what is to be built, block by block.
//!
//! This file holds only the one thing the rest of the core needs from a plan:
//! what to tell an agent about it with a message. The plan itself is built
//! on top of it.

use crate::tasks::Manager;

impl Manager {
    /// What an agent is told about the task's plan with a message, if anything.
    pub(crate) fn plan_note(&self, _task_id: &str) -> Option<String> {
        None
    }
}
