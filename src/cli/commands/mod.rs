// Command-execution modules for individual `gah` subcommand families
// (ticket #407). Parser definitions stay in `crate::cli::args`; each module
// here owns only the dispatch body that used to live inline in
// `crate::cli::run`.

pub mod config;
pub mod controller;
pub mod dispatch;
pub mod doctor;
pub mod init;
pub mod profile;
