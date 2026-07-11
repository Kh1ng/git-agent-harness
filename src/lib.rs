// Library module for testing
// This makes the modules accessible to integration tests

pub mod availability;
pub mod baseline;
pub mod candidates;
pub mod capability;
pub mod claude_monitor;
pub mod config;
pub mod context;
pub mod controller;
pub mod dispatch;
pub mod doctor;
pub mod events;
pub mod init;
pub mod ledger;
pub mod models;
pub mod notifications;
pub mod policy;
pub mod price_guard;
pub mod provider;
pub mod prune;
pub mod quota;
pub mod quota_parser;
pub mod quota_store;
pub mod report;
pub mod routing;
pub mod runner;
pub mod server;
pub mod status;
pub mod sync;
pub mod telemetry;
pub mod tui;
pub mod usage;
pub mod work_claim;
pub mod worktree;

#[cfg(test)]
pub mod test_support;
pub mod tui_state;
pub mod validation_check;
