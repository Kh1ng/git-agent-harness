// Library-owned CLI parser definitions (Clap structs/enums/value parsers).
//
// This module is the permanent parser foundation extracted from the binary
// crate root. It contains only the argument/option/command grammar and the
// directly-owned parser helper (`parse_wake_autonomy`). Orchestration and
// command dispatch live in `crate::cli` (`src/cli/mod.rs`); see ticket #406.

use clap::{Parser, Subcommand};
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "gah", about = "git agent harness")]
pub struct Cli {
    #[command(subcommand)]
    pub command: Commands,
}

/// Sub-actions of `gah availability`.
#[derive(Subcommand)]
pub enum AvailabilityAction {
    /// Issue #179: override stale availability once the backend is healthy. Appends a
    /// `status: available, source: manual` record for the given scope via the
    /// same lock-protected read-modify-write as every other availability
    /// write, so it's safe against concurrent parallel workers. Use --model to
    /// limit the clear to one model, or --quota-pool to clear a pool-wide
    /// block; omit both to mark the whole backend available.
    Clear {
        #[arg(long)]
        backend: String,
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        quota_pool: Option<String>,
    },
}

/// Explicit product-manager publication operations. Planning and provider
/// mutation are deliberately separate commands so a model response can never
/// create issues merely by completing a `dispatch --mode pm` run.
#[derive(Subcommand)]
pub enum PmCommands {
    /// Publish a validated PM plan artifact as native provider issues.
    Publish {
        #[arg(long)]
        profile: String,
        /// Path to the `pm-plan-v1.json` artifact produced by PM dispatch.
        #[arg(long)]
        plan: PathBuf,
        #[arg(long = "config", visible_alias = "config-path")]
        config_path: Option<String>,
        /// Resolve and validate the publication without provider writes.
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
pub enum Commands {
    /// Inspect or set global GAH config defaults (cross-profile facts such as
    /// `current_manager`). Per-profile settings live under `profile set`.
    Config {
        #[command(subcommand)]
        command: ConfigCommands,
    },
    /// Show durable backend/model availability state (global, not per-profile)
    Availability {
        #[arg(long, default_value_t = false)]
        json: bool,
        #[command(subcommand)]
        action: Option<AvailabilityAction>,
    },
    /// Convert gate findings into backlog candidates
    Candidates {
        #[arg(long)]
        gate_artifact: String,
        #[arg(long, default_value_t = false)]
        include_warnings: bool,
        #[arg(long)]
        out_root: String,
    },
    /// Check model price against watchlist policy
    PriceGuard {
        #[arg(long)]
        watchlist: String,
        #[arg(long)]
        model: String,
    },
    /// Check repo policy for a given action
    PolicyCheck {
        #[arg(long)]
        config: String,
        #[arg(long)]
        action: String,
    },
    /// Validate config and profile setup
    Doctor {
        #[arg(long)]
        profile: Option<String>,
        #[arg(long, name = "config")]
        config_path: Option<String>,
        /// Also verify execution prerequisites: validation commands resolve,
        /// declared env files exist, backend executables are present.
        #[arg(long)]
        validate: bool,
        /// Emit structured node-readiness checks for control-plane clients.
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Update the installed CLI and control-plane server deterministically.
    Update {
        /// Repository checkout to update (defaults to the current checkout).
        #[arg(long)]
        repo: Option<PathBuf>,
        /// Restart the system-wide control-plane service after a successful build.
        #[arg(long, default_value_t = false)]
        restart_server: bool,
        #[arg(long, default_value = "gah-server.service")]
        server_service: String,
    },
    /// Create or print a starter GAH config/profile
    Init {
        #[arg(long)]
        profile: String,
        #[arg(long)]
        display_name: String,
        #[arg(long)]
        provider: String,
        #[arg(long)]
        repo: String,
        #[arg(long)]
        local_path: String,
        #[arg(long, default_value = "main")]
        default_target_branch: String,
        #[arg(long)]
        provider_api_base: Option<String>,
        #[arg(long)]
        provider_project_id: Option<String>,
        #[arg(long)]
        artifact_root: Option<String>,
        #[arg(long)]
        worktree_base: Option<String>,
        #[arg(long)]
        oh_profile: Option<String>,
        #[arg(long, name = "config")]
        config_path: Option<String>,
        #[arg(long, default_value_t = false)]
        print: bool,
    },
    /// Delete old GAH-owned sessions and worktrees
    Prune {
        #[arg(long, default_value_t = false)]
        dry_run: bool,
        #[arg(long)]
        older_than: Option<u64>,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long, name = "config")]
        config_path: Option<String>,
    },
    /// Inspect ledger data
    Ledger {
        #[command(subcommand)]
        command: LedgerCommands,
    },
    /// Manager-session review hold: tells gah's own auto-merge loop to
    /// leave a work_id's PR alone while a human or supervising Claude
    /// Code/Codex/Hermes session is actively reviewing it out of band.
    /// gah's own automated loop never uses this -- only a manager session
    /// invokes it.
    Hold {
        #[command(subcommand)]
        command: HoldCommands,
    },
    /// Grant or revoke work-item-scoped permission for an exact paid/API
    /// implementation route. Paid candidates configured with
    /// `requires_approval = true` are never selected without this record.
    RouteApproval {
        #[command(subcommand)]
        command: RouteApprovalCommands,
    },
    /// Run the controller continuously. Use --once for one bounded
    /// observation/decision/execution cycle.
    Loop {
        #[arg(long)]
        profile: String,
        #[arg(long, name = "config")]
        config_path: Option<String>,
        #[arg(long, default_value_t = false)]
        json: bool,
        /// Run exactly one bounded controller iteration instead of the
        /// default recurring loop.
        #[arg(long, default_value_t = false)]
        once: bool,
        /// TICKET-096: Run up to N tickets concurrently instead of one at a time
        #[arg(long, default_value = "0")]
        parallel: usize,
        /// TICKET-073: skip the fresh-worktree self-verification of this
        /// profile's `validation_commands`. Only use after acknowledging a
        /// genuine `VALIDATION GATE FAILED` error.
        #[arg(long, default_value_t = false)]
        skip_validation_gate: bool,
    },
    /// Inspect the controller event stream (TICKET-083)
    Events {
        #[arg(long, name = "config")]
        config_path: Option<String>,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long, default_value_t = false)]
        json: bool,
        #[arg(long, default_value = "7d")]
        since: String,
    },
    /// Provide a single machine-readable controller snapshot of all state
    Status {
        #[arg(long)]
        profile: String,
        #[arg(long, default_value_t = false)]
        json: bool,
        #[arg(long, name = "config")]
        config_path: Option<String>,
    },
    /// Classify open GAH-created merge requests / pull requests
    Sync {
        #[arg(long)]
        profile: String,
        #[arg(long, name = "config")]
        config_path: Option<String>,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Dispatch a job to a backend (improve, pm, review, fix, experiment)
    Dispatch {
        #[arg(long)]
        profile: String,
        #[arg(long)]
        mode: String,
        /// Backend: openhands, cloud-coder, codex, claude, agy, vibe, opencode, auto
        #[arg(long, default_value = "auto")]
        backend: String,
        #[arg(long, default_value = "")]
        target: String,
        #[arg(long)]
        branch: Option<String>,
        #[arg(long)]
        mr: Option<String>,
        #[arg(long, default_value_t = false)]
        current_branch: bool,
        #[arg(long, default_value = "1")]
        budget: u32,
        #[arg(long, default_value_t = false)]
        dry_run: bool,
        /// Path to gah-config.toml (default: auto-discovered)
        #[arg(long, name = "config")]
        config_path: Option<String>,
        /// OpenHands profile name from ~/.openhands/profiles/<name>.json
        #[arg(long)]
        oh_profile: Option<String>,
        /// Override the model name (e.g. "litellm_proxy/cloud-coder").
        /// Takes precedence over profile model and backend defaults.
        #[arg(long)]
        model: Option<String>,
        /// How many times to retry after validation fails (0 = one attempt, no retries)
        #[arg(long, default_value_t = 2)]
        retries: u32,
        /// Push and open draft MR even if validation commands still fail after all retries
        #[arg(long, default_value_t = false)]
        allow_draft_fail: bool,
        /// Load production env file (env_file_prod) instead of dev env_file.
        /// Without this flag, only the dev env_file is loaded.
        #[arg(long, default_value_t = false)]
        prod: bool,
        /// Visible operator override for explicit issue dispatch when intake
        /// policy would otherwise reject the issue.
        #[arg(long, default_value_t = false)]
        issue_intake_override: bool,
        /// Proceed despite a baseline validation failure the classifier
        /// could not attribute to harness/environment/expected-red.
        #[arg(long, default_value_t = false)]
        allow_unknown_red_baseline: bool,
        /// Seed the initial route decision as a genuine agent-capability
        /// failure, activating cost-aware escalation to a stronger backend
        /// (used by `gah loop --once`'s Escalate action).
        #[arg(long, default_value_t = false)]
        escalate: bool,
        /// TICKET-118: reuse an existing branch for fix operations
        #[arg(long)]
        existing_branch: Option<String>,
        /// TICKET-073: skip the fresh-worktree self-verification of this
        /// profile's `validation_commands`. Only use after acknowledging a
        /// genuine `VALIDATION GATE FAILED` error.
        #[arg(long, default_value_t = false)]
        skip_validation_gate: bool,
    },
    /// Validate or explicitly publish product-manager decomposition plans.
    Pm {
        #[command(subcommand)]
        command: PmCommands,
    },
    /// Interactive terminal UI: observe state, confirm and run the one
    /// already-decided next action. Does not let you pick an arbitrary
    /// action -- see docs/MANAGER_MEMORY.md "Stretch Goal -- Optional
    /// Operator TUI" (override-next-action is explicitly out of scope).
    Tui {
        #[arg(long)]
        profile: Option<String>,
        #[arg(long, name = "config")]
        config_path: Option<String>,
    },
    /// Manage profiles
    Profile {
        #[command(subcommand)]
        command: Box<ProfileCommands>,
    },
    /// Generate backend/model comparison report (TICKET-098)
    Report {
        #[arg(long, default_value = "7d")]
        since: String,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long, name = "config")]
        config_path: Option<String>,
        #[arg(long, value_enum, default_value = "backend")]
        group_by: crate::ledger::GroupBy,
        #[arg(long, default_value_t = false)]
        json: bool,
        /// Emit a time-bucketed series (one row per bucket) instead of the
        /// single aggregate-per-backend/model report. Additive: the existing
        /// aggregate behavior is unchanged when this is absent.
        #[arg(long, default_value_t = false)]
        series: bool,
        /// Bucket granularity for `--series`. Only `daily` is supported.
        #[arg(long, default_value = "daily")]
        bucket: String,
    },
    /// Start the WebSocket server for desktop/web interface
    Server {
        #[arg(long, default_value_t = 3773)]
        port: u16,
        #[arg(long, default_value = "127.0.0.1")]
        host: String,
    },
    /// Export telemetry data to versioned repository (TICKET-130)
    Telemetry {
        #[command(subcommand)]
        command: TelemetryCommands,
    },
    /// Manage persisted account-level quota observations (issue #151 / #166)
    Quota {
        #[command(subcommand)]
        command: QuotaCommands,
    },
    /// Inspect and manage work claims (issue #234)
    Claims {
        #[command(subcommand)]
        command: ClaimsCommands,
    },
}

#[derive(Subcommand)]
pub enum ConfigCommands {
    /// Show global defaults (e.g. current_manager). Use --json for machine
    /// output consumed by the dashboard Settings UI.
    Show {
        #[arg(long, default_value_t = false)]
        json: bool,
        /// Emit the redacted, versioned effective configuration projection.
        /// Bare `--json` intentionally retains its legacy one-field shape.
        #[arg(long, default_value_t = false, requires = "json")]
        full: bool,
        /// Restrict `--full` output to one profile while retaining the
        /// provider-neutral profiles map response shape.
        #[arg(long, requires = "full")]
        profile: Option<String>,
        #[arg(long = "config", visible_alias = "config-path")]
        config_path: Option<String>,
    },
    /// Set one or more global default values.
    Set {
        #[arg(long = "config", visible_alias = "config-path")]
        config_path: Option<String>,
        /// Which agent CLI is currently acting as the operator's manager
        /// across all profiles/projects (the manager-wake "who's on call").
        #[arg(long)]
        current_manager: Option<String>,
        /// Clear the specified field(s).
        #[arg(long, value_delimiter = ',')]
        clear: Vec<String>,
    },
}

#[derive(Subcommand)]
pub enum ProfileCommands {
    /// List all profiles in config
    List {
        #[arg(long)]
        config: Option<String>,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Show details for a profile
    Show {
        name: String,
        #[arg(long)]
        config: Option<String>,
    },
    /// Add a new profile
    Add {
        /// Profile name (used as the TOML table key under [profiles])
        name: String,
        #[arg(long)]
        display_name: String,
        #[arg(long)]
        repo_id: String,
        #[arg(long)]
        provider: String,
        #[arg(long)]
        repo: String,
        #[arg(long)]
        local_path: String,
        #[arg(long)]
        artifact_root: String,
        #[arg(long, default_value = "main")]
        default_target_branch: String,
        #[arg(long)]
        provider_api_base: Option<String>,
        #[arg(long)]
        provider_project_id: Option<String>,
        #[arg(long = "config", visible_alias = "config-path")]
        config_path: Option<String>,
        /// Extra CLI args for openhands
        #[arg(long, value_delimiter = ',')]
        openhands_args: Vec<String>,
        /// Extra CLI args for codex
        #[arg(long, value_delimiter = ',')]
        codex_args: Vec<String>,
        /// Path to codex executable
        #[arg(long)]
        codex_path: Option<String>,
        /// Extra CLI args for claude
        #[arg(long, value_delimiter = ',')]
        claude_args: Vec<String>,
        /// Path to claude executable
        #[arg(long)]
        claude_path: Option<String>,
        /// Path to agy executable
        #[arg(long)]
        agy_path: Option<String>,
        /// Extra CLI args for vibe
        #[arg(long, value_delimiter = ',')]
        vibe_args: Vec<String>,
        /// Path to vibe executable
        #[arg(long)]
        vibe_path: Option<String>,
        /// Extra CLI args for opencode
        #[arg(long, value_delimiter = ',')]
        opencode_args: Vec<String>,
        /// Path to opencode executable
        #[arg(long)]
        opencode_path: Option<String>,
        /// HOME override for agy-second backend
        #[arg(long)]
        agy_second_home: Option<String>,
        /// Notification command
        #[arg(long)]
        notify_command: Option<String>,
        /// Policy path
        #[arg(long)]
        policy_path: Option<String>,
        /// Dev env file
        #[arg(long)]
        env_file: Option<String>,
        /// Production env file
        #[arg(long)]
        env_file_prod: Option<String>,
        /// Validation commands
        #[arg(long, value_delimiter = ',')]
        validation_commands: Vec<String>,
        /// Auto-fix commands
        #[arg(long, value_delimiter = ',')]
        auto_fix_commands: Vec<String>,
        /// How many tickets `gah loop` may execute concurrently for this
        /// profile (defaults to 1). Exposed in the dashboard Settings UI.
        #[arg(long)]
        max_parallel_workers: Option<u32>,
        /// Maximum open/in-flight managed PRs or MRs before implementation
        /// intake pauses and lifecycle work drains.
        #[arg(long)]
        max_open_managed_mrs: Option<u32>,
        /// Timeout in seconds for each validation command (defaults to 300).
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
        validation_timeout_seconds: Option<u64>,
        /// Manager-wake autonomy for this profile: off | review_only | full.
        /// Exposed in the dashboard Settings UI.
        #[arg(long)]
        manager_wake_autonomy: Option<String>,
        /// Delivery mode for work results: pr (default) | handoff.
        #[arg(long)]
        delivery_mode: Option<String>,
    },
    /// Set/Update fields of an existing profile
    Set {
        name: String,
        #[arg(long)]
        display_name: Option<String>,
        #[arg(long)]
        repo_id: Option<String>,
        #[arg(long)]
        provider: Option<String>,
        #[arg(long)]
        repo: Option<String>,
        #[arg(long)]
        local_path: Option<String>,
        #[arg(long)]
        artifact_root: Option<String>,
        #[arg(long)]
        default_target_branch: Option<String>,
        #[arg(long)]
        provider_api_base: Option<String>,
        #[arg(long)]
        provider_project_id: Option<String>,
        #[arg(long = "config", visible_alias = "config-path")]
        config_path: Option<String>,
        #[arg(long, value_delimiter = ',')]
        openhands_args: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        codex_args: Vec<String>,
        #[arg(long)]
        codex_path: Option<String>,
        #[arg(long, value_delimiter = ',')]
        claude_args: Vec<String>,
        #[arg(long)]
        claude_path: Option<String>,
        #[arg(long)]
        agy_path: Option<String>,
        #[arg(long, value_delimiter = ',')]
        vibe_args: Vec<String>,
        #[arg(long)]
        vibe_path: Option<String>,
        #[arg(long, value_delimiter = ',')]
        opencode_args: Vec<String>,
        #[arg(long)]
        opencode_path: Option<String>,
        #[arg(long)]
        agy_second_home: Option<String>,
        #[arg(long)]
        notify_command: Option<String>,
        #[arg(long)]
        policy_path: Option<String>,
        #[arg(long)]
        env_file: Option<String>,
        #[arg(long)]
        env_file_prod: Option<String>,
        #[arg(long, value_delimiter = ',')]
        validation_commands: Vec<String>,
        #[arg(long, value_delimiter = ',')]
        auto_fix_commands: Vec<String>,
        /// How many tickets `gah loop` may execute concurrently for this
        /// profile (defaults to 1). Exposed in the dashboard Settings UI.
        #[arg(long)]
        max_parallel_workers: Option<u32>,
        /// Maximum open/in-flight managed PRs or MRs before implementation
        /// intake pauses and lifecycle work drains.
        #[arg(long)]
        max_open_managed_mrs: Option<u32>,
        /// Timeout in seconds for each validation command (defaults to 300).
        #[arg(long, value_parser = clap::value_parser!(u64).range(1..))]
        validation_timeout_seconds: Option<u64>,
        /// Manager-wake autonomy for this profile: off | review_only | full.
        /// Exposed in the dashboard Settings UI.
        #[arg(long)]
        manager_wake_autonomy: Option<String>,
        /// Delivery mode for work results: pr | handoff.
        #[arg(long)]
        delivery_mode: Option<String>,
        /// Clear the specified field(s) - for fields that support it
        #[arg(long, value_delimiter = ',')]
        clear: Vec<String>,
    },
    /// Remove a profile
    Remove {
        name: String,
        #[arg(long = "config", visible_alias = "config-path")]
        config_path: Option<String>,
        #[arg(long, default_value_t = false)]
        force: bool,
    },
}

/// Parse a `manager_wake_autonomy` value from CLI text (snake_case, matching
/// the TOML/serde spelling) into the typed enum. Kept as a manual mapping so
/// the CLI error message can name the accepted values precisely.
pub fn parse_wake_autonomy(value: &str) -> anyhow::Result<crate::config::WakeAutonomy> {
    match value.to_ascii_lowercase().as_str() {
        "off" | "none" => Ok(crate::config::WakeAutonomy::Off),
        "review_only" | "reviewonly" | "review" => Ok(crate::config::WakeAutonomy::ReviewOnly),
        "full" => Ok(crate::config::WakeAutonomy::Full),
        other => anyhow::bail!(
            "invalid manager_wake_autonomy '{}' (expected off | review_only | full)",
            other
        ),
    }
}

/// Parse a `delivery_mode` value from CLI text into the typed enum.
pub fn parse_delivery_mode(value: &str) -> anyhow::Result<crate::config::DeliveryMode> {
    match value.to_ascii_lowercase().as_str() {
        "pr" => Ok(crate::config::DeliveryMode::Pr),
        "handoff" => Ok(crate::config::DeliveryMode::Handoff),
        other => anyhow::bail!("invalid delivery_mode '{}' (expected pr | handoff)", other),
    }
}

#[derive(Subcommand)]
pub enum LedgerCommands {
    /// Back up and remove one torn, unterminated final JSONL record. Refuses
    /// to alter any mid-file or newline-terminated corruption.
    RepairTail {
        #[arg(long, name = "config")]
        config_path: Option<String>,
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
    /// Summarize recent ledger entries
    Summary {
        #[arg(long, default_value = "7d")]
        since: String,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long, name = "config")]
        config_path: Option<String>,
        #[arg(long)]
        json: bool,
        #[arg(long, value_enum, default_value = "none")]
        group_by: crate::ledger::GroupBy,
    },
    /// Backfill dispatched work with later provider outcomes (MR merged/closed)
    Reconcile {
        #[arg(long)]
        profile: String,
        #[arg(long, name = "config")]
        config_path: Option<String>,
        #[arg(long)]
        json: bool,
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
    /// Full ledger history (dispatch/attempt/retry/review entries) for one
    /// work item, in chronological order -- the data source for a frontend
    /// attempt-timeline view. Thin CLI wrapper around the existing
    /// `ledger::entries_for_work_id`; no new ledger logic.
    Work {
        work_id: String,
        #[arg(long, name = "config")]
        config_path: Option<String>,
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Issue #95: append a tombstone ledger entry that marks all prior
    /// attempts for a work_id as stale. Does NOT rewrite history -- the
    /// original entries remain in the JSONL file but are superseded by the
    /// tombstone. After clearing, the ticket becomes dispatchable again.
    ClearAttempts {
        #[arg(long)]
        profile: String,
        work_id: String,
        #[arg(long, name = "config")]
        config_path: Option<String>,
        #[arg(long, default_value_t = false)]
        dry_run: bool,
    },
}

#[derive(Subcommand)]
pub enum HoldCommands {
    /// Mark a work_id as under active out-of-band manager review. gah's
    /// `decide_next_action` will skip auto-merging it until it's cleared
    /// (`gah hold clear`) or the hold self-expires after
    /// `REVIEW_HOLD_STALE_AFTER_HOURS`.
    Set {
        #[arg(long)]
        profile: String,
        work_id: String,
        #[arg(long)]
        reason: Option<String>,
        #[arg(long, name = "config")]
        config_path: Option<String>,
    },
    /// Release a previously set review hold on a work_id.
    Clear {
        #[arg(long)]
        profile: String,
        work_id: String,
        #[arg(long, name = "config")]
        config_path: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum RouteApprovalCommands {
    /// Allow one exact paid backend/model route for this work item.
    Grant {
        #[arg(long)]
        profile: String,
        work_id: String,
        #[arg(long)]
        backend: String,
        #[arg(long)]
        model: Option<String>,
        #[arg(long, name = "config")]
        config_path: Option<String>,
    },
    /// Remove a previously granted paid-route approval.
    Revoke {
        #[arg(long)]
        profile: String,
        work_id: String,
        #[arg(long)]
        backend: String,
        #[arg(long)]
        model: Option<String>,
        #[arg(long, name = "config")]
        config_path: Option<String>,
    },
}

#[derive(Subcommand)]
pub enum TelemetryCommands {
    /// Export telemetry data to versioned repository
    Export {
        #[arg(long)]
        telemetry_repo_path: Option<String>,
        #[arg(long, default_value = "jsonl")]
        format: String,
        #[arg(long)]
        output: Option<String>,
        #[arg(long, default_value = "7d")]
        since: String,
        #[arg(long)]
        profile: Option<String>,
        #[arg(long, value_enum, default_value = "none")]
        group_by: crate::telemetry::GroupBy,
        #[arg(long, default_value_t = true)]
        generate_manifests: bool,
        #[arg(long, name = "config")]
        config_path: Option<String>,
    },
    /// Show telemetry repository status
    Status {
        #[arg(long)]
        telemetry_repo_path: Option<String>,
        #[arg(long, name = "config")]
        config_path: Option<String>,
    },
    /// Generate aggregated telemetry reports by routing dimensions
    Aggregate {
        /// Aggregation dimensions (can specify multiple: project, ticket, backend, model, etc.)
        #[arg(long, value_delimiter = ',', required = true)]
        dimensions: Vec<String>,
        /// Start of time range (RFC3339 timestamp or YYYY-MM-DD date)
        #[arg(long)]
        since: Option<String>,
        /// End of time range (RFC3339 timestamp or YYYY-MM-DD date)
        #[arg(long)]
        until: Option<String>,
        /// Filter by profile
        #[arg(long)]
        profile: Option<String>,
        /// Include failed attempts in aggregation
        #[arg(long, default_value_t = true)]
        include_failed: bool,
        /// Include retried attempts in aggregation
        #[arg(long, default_value_t = true)]
        include_retried: bool,
        /// Output in JSON format
        #[arg(long, default_value_t = false)]
        json: bool,
        #[arg(long, name = "config")]
        config_path: Option<String>,
        /// Filter by project/repo ID
        #[arg(long)]
        project: Option<String>,
        /// Filter by ticket/work ID
        #[arg(long)]
        ticket: Option<String>,
        /// Filter by execution/task type (e.g. improve, fix, review)
        #[arg(long)]
        execution_type: Option<String>,
        /// Filter by backend instance
        #[arg(long)]
        backend_instance: Option<String>,
        /// Filter by provider
        #[arg(long)]
        provider: Option<String>,
        /// Filter by model
        #[arg(long)]
        model: Option<String>,
        /// Filter by account label
        #[arg(long)]
        account: Option<String>,
    },
}

/// Quota/usage observation management (issue #151 / #166).
#[derive(Subcommand)]
pub enum QuotaCommands {
    /// Refresh account-level quota (e.g. `codex status --json`) and persist
    /// the observation so the Quota/Telemetry pages show real data.
    Refresh {
        /// Backend whose account quota to refresh (e.g. "codex").
        #[arg(long, default_value = "codex")]
        backend: String,
        /// Model qualifier for the observation (usually unset for
        /// account-level readings).
        #[arg(long)]
        model: Option<String>,
        /// Path/command for the backend CLI (defaults to the backend name on
        /// PATH, e.g. "codex"). Only `codex` has a structured status parser
        /// today; other backends fall back to "no data" rather than guessing.
        #[arg(long)]
        command: Option<String>,
        /// Override the durable store path (default: $XDG_STATE_HOME/gah/...).
        /// Mainly for testing/automation.
        #[arg(long, name = "store")]
        store_path: Option<String>,
    },
    /// List persisted account-level quota observations.
    List {
        #[arg(long, default_value_t = false)]
        json: bool,
        #[arg(long, name = "store")]
        store_path: Option<String>,
    },
    /// Build the canonical profile-scoped quota snapshot used by the web
    /// dashboard and CLI inspection paths.
    Snapshot {
        #[arg(long)]
        profile: String,
        #[arg(long, default_value = "7d")]
        since: String,
        #[arg(long, default_value_t = false)]
        json: bool,
        #[arg(long, name = "config")]
        config_path: Option<String>,
    },
}

#[rustfmt::skip]
#[derive(Subcommand)]
pub enum ClaimsCommands {
    List { #[arg(long, default_value_t = false)] json: bool, #[arg(long)] profile: Option<String>, #[arg(long, name = "config")] config_path: Option<String> },
    Clear {
        #[arg(long)]
        work_id: String,
        #[arg(long)]
        profile: String,
    },
    Reclaim {
        #[arg(long)]
        profile: String,
        #[arg(long, default_value = "3600")]
        max_age_secs: u64,
    },
}
