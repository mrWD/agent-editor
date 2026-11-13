//! The core of Agent Editor.
//!
//! One long-lived process. It starts agents, keeps their sessions and does all
//! the heavy work; the interface is a thin client that talks to it over a
//! WebSocket. Closing the interface does not stop anything here.

pub mod acp;
pub mod agents;
pub mod changes;
pub mod checkpoints;
pub mod files;
pub mod git;
pub mod gitops;
pub mod hosts;
pub mod integrations;
pub mod marks;
pub mod review;
pub mod runner;
pub mod search;
pub mod server;
pub mod store;
pub mod tasks;
pub mod terminals;
