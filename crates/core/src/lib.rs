//! The core of Agent Editor.
//!
//! One long-lived process. It starts agents, keeps their sessions and does all
//! the heavy work; the interface is a thin client that talks to it over a
//! WebSocket. Closing the interface does not stop anything here.

pub mod acp;
pub mod agents;
pub mod api;
pub mod automations;
pub mod awake;
pub mod changes;
pub mod checkpoints;
pub mod checks;
pub mod ci;
pub mod database;
pub mod deploys;
pub mod devices;
pub mod fileops;
pub mod files;
pub mod git;
pub mod gitops;
pub mod health;
pub mod hosts;
pub mod imports;
pub mod integrations;
pub mod lsp;
pub mod map;
pub mod marks;
pub mod mcp;
pub mod notify;
pub mod onboarding;
pub mod overlaps;
pub mod paths;
pub mod pictures;
pub mod plan;
pub mod preview;
pub mod race;
pub mod review;
pub mod rules;
pub mod runner;
pub mod search;
pub mod sentry;
pub mod server;
pub mod settings;
pub mod setup;
pub mod shots;
pub mod sonar;
pub mod startup;
pub mod store;
pub mod tasks;
pub mod team;
pub mod terminals;
pub mod tools;
pub mod toolservers;
pub mod usage;

pub use notify::Notice;
pub use startup::{Core, DEFAULT_PORT, Desktop, Options, Port, default_data_dir, log_to_stdout, run, start};
