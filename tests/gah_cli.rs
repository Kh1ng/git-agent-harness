#![allow(unused_imports)]

#[path = "gah_cli/support.rs"]
mod cli_support;
use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::process::{Command as ProcessCommand, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use support::{isolate_command, test_tempdir, FakeBackend, IsolatedCommand, Scenario};
use tempfile::TempDir;
#[path = "gah_cli/already_satisfied.rs"]
mod already_satisfied;
#[path = "gah_cli/args.rs"]
mod args;
#[path = "gah_cli/availability.rs"]
mod availability_cli;
#[path = "gah_cli/basic.rs"]
mod basic;
#[path = "gah_cli/conflict_resolution.rs"]
mod conflict_resolution;
#[path = "gah_cli/dispatch_profiles.rs"]
mod dispatch_profiles;
#[path = "gah_cli/doctor_json.rs"]
mod doctor_json;
#[path = "gah_cli/doctor_pm.rs"]
mod doctor_pm;
#[path = "gah_cli/fix.rs"]
mod fix;
#[path = "gah_cli/gitlab_review.rs"]
mod gitlab_review;
#[path = "gah_cli/ledger_review.rs"]
mod ledger_review;
#[path = "gah_cli/loop_publish.rs"]
mod loop_publish;
#[path = "gah_cli/pm.rs"]
mod pm;
#[path = "gah_cli/review_format_retry.rs"]
mod review_format_retry;
#[path = "gah_cli/route_approval.rs"]
mod route_approval;
#[path = "gah_cli/stall_retry.rs"]
mod stall_retry;
mod support;
#[path = "gah_cli/sync_status.rs"]
mod sync_status;
#[path = "gah_cli/validation_gate.rs"]
mod validation_gate;
use cli_support::*;
