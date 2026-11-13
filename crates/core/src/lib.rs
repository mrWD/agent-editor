//! The core of Agent Editor.
//!
//! One long-lived process. It starts agents, keeps their sessions and does all
//! the heavy work; the interface is a thin client that talks to it over a
//! WebSocket. Closing the interface does not stop anything here.

pub mod acp;
pub mod agents;
pub mod automations;
pub mod changes;
pub mod checkpoints;
pub mod checks;
pub mod fileops;
pub mod files;
pub mod git;
pub mod gitops;
pub mod health;
pub mod hosts;
pub mod integrations;
pub mod lsp;
pub mod map;
pub mod marks;
pub mod overlaps;
pub mod plan;
pub mod review;
pub mod rules;
pub mod runner;
pub mod search;
pub mod server;
pub mod startup;
pub mod store;
pub mod tasks;
pub mod terminals;
pub mod usage;

pub use startup::{Core, DEFAULT_PORT, Desktop, Options, Port, default_data_dir, log_to_stdout, run, start};
