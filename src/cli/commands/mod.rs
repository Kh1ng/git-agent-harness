// Command-execution modules for individual `gah` subcommand families
// (ticket #407). Parser definitions stay in `crate::cli::args`; each module
// here owns only the dispatch body that used to live inline in
// `crate::cli::run`.

pub mod availability;
pub mod candidates;
pub mod claims;
pub mod config;
pub mod controller;
pub mod dispatch;
pub mod doctor;
pub mod external_approval;
pub mod init;
pub mod ledger;
pub mod policy;
pub mod price_guard;
pub mod profile;
pub mod prune;
pub mod quota;
pub mod report;
pub mod server;
pub mod telemetry;
pub mod tui;
pub mod update;
