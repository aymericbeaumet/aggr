//! Git-native feed ingestion, archival storage, and static reader generation.
//!
//! Use [`config::Config`] to read configuration or [`commands`] for complete workflows.

pub mod cache;
pub mod cli;
pub mod commands;
pub mod config;
pub mod content;
pub mod discussions;
pub mod document;
pub mod git;
pub mod http;
pub mod media;
pub mod media_duration;
pub mod model;
pub mod platform;
pub mod preview;
pub mod site;
pub mod sources;
pub mod store;
pub mod threads;

pub use cli::Cli;
pub use commands::run;
