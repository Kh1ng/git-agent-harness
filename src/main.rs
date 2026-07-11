mod availability;
mod baseline;
mod candidates;
mod capability;
pub mod claude_monitor;
mod config;
mod controller;
mod dispatch;
mod doctor;
mod events;
mod init;
mod ledger;
mod models;
mod notifications;
mod policy;
mod price_guard;
mod provider;
mod prune;
mod quota;
mod quota_parser;
mod quota_store;
mod report;
mod routing;
mod runner;
mod server;
mod status;
mod sync;
mod telemetry;
#[cfg(test)]
mod test_support;
mod tui;
mod tui_state;
mod usage;
mod validation_check;
mod work_claim;
mod worktree;

use anyhow::Result;
use clap::{Parser, Subcommand};
use config::Profile;
use std::path::PathBuf;

#[derive(Parser)]
#[command(name = "gah", about = "git agent harness")]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Show durable backend/model availability state (global, not per-profile)
    Availability {
        #[arg(long, default_value_t = false)]
        json: bool,
    },
    /// Issue #179: clear a stale availability/quota-exhaustion record once
    /// the backend is confirmed actually healthy again -- goes through the
    /// same locked read-modify-write as every other availability write, so
    /// it's safe against concurrent parallel workers (unlike hand-editing
    /// availability.json directly). Omit --model to clear every record for
    /// the backend regardless of model.
    AvailabilityClear {
        #[arg(long)]
        backend: String,
        #[arg(long)]
        model: Option<String>,
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
        group_by: ledger::GroupBy,
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
}

#[derive(Subcommand)]
enum ProfileCommands {
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
        #[arg(long, name = "config")]
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
        #[arg(long, name = "config")]
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
        /// Clear the specified field(s) - for fields that support it
        #[arg(long, value_delimiter = ',')]
        clear: Vec<String>,
    },
    /// Remove a profile
    Remove {
        name: String,
        #[arg(long, name = "config")]
        config_path: Option<String>,
        #[arg(long, default_value_t = false)]
        force: bool,
    },
}

#[derive(Subcommand)]
enum LedgerCommands {
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
        group_by: ledger::GroupBy,
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
enum HoldCommands {
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
enum TelemetryCommands {
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
        group_by: telemetry::GroupBy,
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
}

/// Quota/usage observation management (issue #151 / #166).
#[derive(Subcommand)]
enum QuotaCommands {
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
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    match cli.command {
        Commands::Availability { json } => availability::cli::run(json)?,
        Commands::AvailabilityClear { backend, model } => {
            let removed = availability::cli::clear(
                &availability::resolve_state_path(),
                &backend,
                model.as_deref(),
            )?;
            println!(
                "Cleared {removed} availability record(s) for backend '{backend}'{}",
                model
                    .as_deref()
                    .map(|m| format!(" / model '{m}'"))
                    .unwrap_or_default()
            );
        }

        Commands::Candidates {
            gate_artifact,
            include_warnings,
            out_root,
        } => candidates::run(&gate_artifact, include_warnings, &out_root)?,

        Commands::PriceGuard { watchlist, model } => price_guard::run(&watchlist, &model)?,

        Commands::PolicyCheck { config, action } => policy::run(&config, &action)?,

        Commands::Doctor {
            profile,
            config_path,
            validate,
        } => doctor::run_with_validate(profile.as_deref(), config_path.as_deref(), validate)?,

        Commands::Init {
            profile,
            display_name,
            provider,
            repo,
            local_path,
            default_target_branch,
            provider_api_base,
            provider_project_id,
            artifact_root,
            worktree_base,
            oh_profile,
            config_path,
            print,
        } => init::run(init::InitArgs {
            profile,
            display_name,
            provider,
            repo,
            local_path,
            default_target_branch,
            provider_api_base,
            provider_project_id,
            artifact_root,
            worktree_base,
            oh_profile,
            config_path,
            print,
        })?,

        Commands::Prune {
            dry_run,
            older_than,
            profile,
            config_path,
        } => prune::run(
            profile.as_deref(),
            config_path.as_deref(),
            older_than,
            dry_run,
        )?,

        Commands::Ledger { command } => match command {
            LedgerCommands::Summary {
                since,
                profile,
                config_path,
                json,
                group_by,
            } => ledger::summary::run_with_json(
                &since,
                profile.as_deref(),
                config_path.as_deref(),
                json,
                group_by,
            )?,
            LedgerCommands::Reconcile {
                profile,
                config_path,
                json,
                dry_run,
            } => {
                let cfg = config::load(config_path.as_deref())?;
                ledger::reconcile::run(&cfg, &profile, json, dry_run)?;
            }
            LedgerCommands::Work {
                work_id,
                config_path,
                json,
            } => {
                let cfg = config::load(config_path.as_deref())?;
                let mut entries = ledger::entries_for_work_id(&cfg, &work_id)?;
                entries.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));
                if json {
                    println!("{}", serde_json::to_string(&entries)?);
                } else if entries.is_empty() {
                    println!("No ledger entries found for work item '{}'.", work_id);
                } else {
                    println!("Work item: {} ({} entries)", work_id, entries.len());
                    for e in &entries {
                        let cost = e
                            .usage
                            .actual_cost_usd
                            .or(e.usage.estimated_cost_usd)
                            .map(|c| format!("${c:.4}"))
                            .unwrap_or_else(|| "unknown cost".into());
                        println!(
                            "  {}  {}  {}/{}  validation={} failure={} duration={} {}",
                            e.timestamp,
                            e.mode,
                            e.effective_backend,
                            e.effective_model.as_deref().unwrap_or("?"),
                            e.validation_result.as_deref().unwrap_or("-"),
                            e.failure_class.as_deref().unwrap_or("-"),
                            e.duration_seconds
                                .map(|d| format!("{d:.0}s"))
                                .unwrap_or_else(|| "unknown".into()),
                            cost,
                        );
                    }
                }
            }
            LedgerCommands::ClearAttempts {
                profile,
                work_id,
                config_path,
                dry_run,
            } => {
                let cfg = config::load(config_path.as_deref())?;
                let prof = config::get_profile(&cfg, &profile)?;
                let entry = ledger::LedgerEntry::new_clear_attempts(&profile, prof, &work_id);
                if dry_run {
                    println!(
                        "Dry run: would append tombstone entry for work_id '{}':",
                        work_id
                    );
                    println!("{}", serde_json::to_string_pretty(&entry)?);
                } else {
                    let path = ledger::append(&cfg, &entry)?;
                    println!(
                        "Appended tombstone entry for work_id '{}' to {}",
                        work_id,
                        path.display()
                    );
                }
            }
        },

        Commands::Hold { command } => match command {
            HoldCommands::Set {
                profile,
                work_id,
                reason,
                config_path,
            } => {
                let cfg = config::load(config_path.as_deref())?;
                let prof = config::get_profile(&cfg, &profile)?;
                let entry = ledger::LedgerEntry::new_review_hold(&profile, prof, &work_id, reason);
                let path = ledger::append(&cfg, &entry)?;
                println!(
                    "Review hold set for work_id '{}' on profile '{}' ({})",
                    work_id,
                    profile,
                    path.display()
                );
            }
            HoldCommands::Clear {
                profile,
                work_id,
                config_path,
            } => {
                let cfg = config::load(config_path.as_deref())?;
                let prof = config::get_profile(&cfg, &profile)?;
                let entry = ledger::LedgerEntry::new_review_hold_release(&profile, prof, &work_id);
                let path = ledger::append(&cfg, &entry)?;
                println!(
                    "Review hold cleared for work_id '{}' on profile '{}' ({})",
                    work_id,
                    profile,
                    path.display()
                );
            }
        },

        Commands::Loop {
            profile,
            config_path,
            json,
            once,
            parallel,
            skip_validation_gate,
        } => {
            let cfg = config::load(config_path.as_deref())?;
            let parallel = if parallel == 0 {
                config::get_profile(&cfg, &profile)?.max_parallel_workers() as usize
            } else {
                parallel
            };
            if once {
                controller::run_once(&cfg, &profile, json, parallel, skip_validation_gate)?;
            } else {
                controller::run_loop(&cfg, &profile, json, parallel, skip_validation_gate)?;
            }
        }

        Commands::Events {
            config_path,
            profile,
            json,
            since,
        } => {
            let cfg = config::load(config_path.as_deref())?;
            events::run(&cfg, &since, profile.as_deref(), json)?;
        }

        Commands::Status {
            profile,
            json,
            config_path,
        } => {
            let cfg = config::load(config_path.as_deref())?;
            status::run(&cfg, &profile, json)?;
        }

        Commands::Sync {
            profile,
            config_path,
            json,
        } => {
            let cfg = config::load(config_path.as_deref())?;
            sync::run(&cfg, &profile, json)?;
        }

        Commands::Dispatch {
            profile,
            mode,
            backend,
            target,
            branch,
            mr,
            current_branch,
            budget,
            dry_run,
            config_path,
            model,
            oh_profile,
            retries,
            allow_draft_fail,
            prod,
            allow_unknown_red_baseline,
            escalate,
            existing_branch,
            skip_validation_gate,
        } => {
            let cfg = config::load(config_path.as_deref())?;
            dispatch::run(
                &cfg,
                &dispatch::DispatchArgs {
                    profile,
                    mode,
                    backend,
                    target,
                    branch,
                    mr,
                    current_branch,
                    budget,
                    dry_run,
                    config_path,
                    model,
                    oh_profile,
                    retries,
                    allow_draft_fail,
                    prod,
                    allow_unknown_red_baseline,
                    escalate,
                    existing_branch,
                    skip_validation_gate,
                    dispatch_reason: None,
                    work_id: None,
                    run_id: None,
                },
            )?;
        }

        Commands::Tui {
            profile,
            config_path,
        } => {
            let cfg = config::load(config_path.as_deref())?;
            tui::run(&cfg, profile.as_deref())?;
        }

        Commands::Profile { command } => match *command {
            ProfileCommands::List { config, json } => {
                let cfg = config::load(config.as_deref())?;
                let mut names: Vec<&str> = cfg.profiles.keys().map(String::as_str).collect();
                names.sort_unstable();
                if json {
                    #[derive(serde::Serialize)]
                    struct ProfileSummary<'a> {
                        name: &'a str,
                        display_name: &'a str,
                        provider: &'a str,
                        repo: &'a str,
                        local_path: &'a str,
                        web_url: Option<String>,
                    }
                    let summaries: Vec<ProfileSummary> = names
                        .iter()
                        .map(|name| {
                            let p = &cfg.profiles[*name];
                            ProfileSummary {
                                name,
                                display_name: &p.display_name,
                                provider: &p.provider,
                                repo: &p.repo,
                                local_path: &p.local_path,
                                web_url: p.web_url(),
                            }
                        })
                        .collect();
                    println!("{}", serde_json::to_string(&summaries)?);
                } else {
                    for name in names {
                        let p = &cfg.profiles[name];
                        println!("{:<25} {} ({})", name, p.display_name, p.provider);
                    }
                }
            }
            ProfileCommands::Show { name, config } => {
                let cfg = config::load(config.as_deref())?;
                let p = config::get_profile(&cfg, &name)?;
                println!("name:                  {}", name);
                println!("display_name:          {}", p.display_name);
                println!("repo_id:               {}", p.repo_id);
                println!("provider:              {}", p.provider);
                println!("repo:                  {}", p.repo);
                println!("local_path:            {}", p.local_path);
                println!("artifact_root:         {}", p.artifact_root);
                println!("default_target_branch: {}", p.default_target_branch);
                if let Some(api) = &p.provider_api_base {
                    println!("provider_api_base:     {}", api);
                }
                if let Some(id) = &p.provider_project_id {
                    println!("provider_project_id:   {}", id);
                }
                if !p.openhands_args.is_empty() {
                    println!("openhands_args:        {:?}", p.openhands_args);
                }
                if !p.codex_args.is_empty() {
                    println!("codex_args:            {:?}", p.codex_args);
                }
                if !p.claude_args.is_empty() {
                    println!("claude_args:           {:?}", p.claude_args);
                }
                if !p.validation_commands.is_empty() {
                    println!("validation_commands:");
                    for cmd in &p.validation_commands {
                        println!("  - {}", cmd);
                    }
                }
            }
            ProfileCommands::Add {
                name,
                display_name,
                repo_id,
                provider,
                repo,
                local_path,
                artifact_root,
                default_target_branch,
                provider_api_base,
                provider_project_id,
                config_path,
                openhands_args,
                codex_args,
                codex_path,
                claude_args,
                claude_path,
                agy_path,
                vibe_args,
                vibe_path,
                opencode_args,
                opencode_path,
                agy_second_home,
                notify_command,
                policy_path,
                env_file,
                env_file_prod,
                validation_commands,
                auto_fix_commands,
            } => {
                let mut cfg = config::load(config_path.as_deref())?;
                let profile = Profile {
                    display_name,
                    repo_id,
                    provider,
                    repo,
                    local_path,
                    artifact_root,
                    default_target_branch,
                    provider_api_base,
                    provider_project_id,
                    oh_profile: None,
                    openhands_args,
                    codex_args,
                    codex_path,
                    claude_args,
                    claude_path,
                    agy_path,
                    vibe_args,
                    vibe_path,
                    opencode_args,
                    opencode_path,
                    agy_second_home,
                    agy_print_timeout_seconds: std::collections::HashMap::new(),
                    agy_idle_timeout_seconds: None,
                    opencode_idle_timeout_seconds: None,
                    openhands_idle_timeout_seconds: None,
                    vibe_idle_timeout_seconds: None,
                    codex_idle_timeout_seconds: None,
                    claude_idle_timeout_seconds: None,
                    max_parallel_workers: None,
                    notify_command,
                    manager_wake_autonomy: config::WakeAutonomy::default(),
                    policy_path,
                    env_file,
                    env_file_prod,
                    validation_commands,
                    auto_fix_commands,
                    test_file_patterns: vec![],
                    known_baseline_failure_markers: vec![],
                    model_improve: None,
                    model_pm: None,
                    model_review: None,
                    review_timeout_seconds: None,
                    routing: config::RoutingPolicy::default(),
                    publishing: Default::default(),
                    pacing: Default::default(),
                    prune_older_than_days: None,
                };
                config::add_profile(&mut cfg, &name, profile)?;
                config::save(&cfg, config_path.as_deref())?;
                println!("Added profile '{}'", name);
            }
            ProfileCommands::Set {
                name,
                display_name,
                repo_id,
                provider,
                repo,
                local_path,
                artifact_root,
                default_target_branch,
                provider_api_base,
                provider_project_id,
                config_path,
                openhands_args,
                codex_args,
                codex_path,
                claude_args,
                claude_path,
                agy_path,
                vibe_args,
                vibe_path,
                opencode_args,
                opencode_path,
                agy_second_home,
                notify_command,
                policy_path,
                env_file,
                env_file_prod,
                validation_commands,
                auto_fix_commands,
                clear,
            } => {
                let mut cfg = config::load(config_path.as_deref())?;
                let existing = config::get_profile_mut(&mut cfg, &name)?;

                // Helper to clear a field if requested
                fn should_clear(field: &str, clear_list: &[String]) -> bool {
                    clear_list.contains(&field.to_string())
                }

                // Update fields if provided, or clear if requested
                if let Some(v) = display_name {
                    existing.display_name = v;
                } else if should_clear("display_name", &clear) {
                    // display_name is required, so we don't actually clear it
                }

                if let Some(v) = repo_id {
                    existing.repo_id = v;
                } else if should_clear("repo_id", &clear) {
                    // repo_id is required
                }

                if let Some(v) = provider {
                    existing.provider = v;
                } else if should_clear("provider", &clear) {
                    // provider is required
                }

                if let Some(v) = repo {
                    existing.repo = v;
                } else if should_clear("repo", &clear) {
                    // repo is required
                }

                if let Some(v) = local_path {
                    existing.local_path = v;
                } else if should_clear("local_path", &clear) {
                    // local_path is required
                }

                if let Some(v) = artifact_root {
                    existing.artifact_root = v;
                } else if should_clear("artifact_root", &clear) {
                    // artifact_root is required
                }

                if let Some(v) = default_target_branch {
                    existing.default_target_branch = v;
                } else if should_clear("default_target_branch", &clear) {
                    // default_target_branch is required
                }

                if let Some(v) = provider_api_base {
                    existing.provider_api_base = Some(v);
                } else if should_clear("provider_api_base", &clear) {
                    existing.provider_api_base = None;
                }

                if let Some(v) = provider_project_id {
                    existing.provider_project_id = Some(v);
                } else if should_clear("provider_project_id", &clear) {
                    existing.provider_project_id = None;
                }

                if !openhands_args.is_empty() {
                    existing.openhands_args = openhands_args;
                } else if should_clear("openhands_args", &clear) {
                    existing.openhands_args.clear();
                }

                if !codex_args.is_empty() {
                    existing.codex_args = codex_args;
                } else if should_clear("codex_args", &clear) {
                    existing.codex_args.clear();
                }

                if let Some(v) = codex_path {
                    existing.codex_path = Some(v);
                } else if should_clear("codex_path", &clear) {
                    existing.codex_path = None;
                }

                if !claude_args.is_empty() {
                    existing.claude_args = claude_args;
                } else if should_clear("claude_args", &clear) {
                    existing.claude_args.clear();
                }

                if let Some(v) = claude_path {
                    existing.claude_path = Some(v);
                } else if should_clear("claude_path", &clear) {
                    existing.claude_path = None;
                }

                if let Some(v) = agy_path {
                    existing.agy_path = Some(v);
                } else if should_clear("agy_path", &clear) {
                    existing.agy_path = None;
                }

                if !vibe_args.is_empty() {
                    existing.vibe_args = vibe_args;
                } else if should_clear("vibe_args", &clear) {
                    existing.vibe_args.clear();
                }

                if let Some(v) = vibe_path {
                    existing.vibe_path = Some(v);
                } else if should_clear("vibe_path", &clear) {
                    existing.vibe_path = None;
                }

                if !opencode_args.is_empty() {
                    existing.opencode_args = opencode_args;
                } else if should_clear("opencode_args", &clear) {
                    existing.opencode_args.clear();
                }

                if let Some(v) = opencode_path {
                    existing.opencode_path = Some(v);
                } else if should_clear("opencode_path", &clear) {
                    existing.opencode_path = None;
                }

                if let Some(v) = agy_second_home {
                    existing.agy_second_home = Some(v);
                } else if should_clear("agy_second_home", &clear) {
                    existing.agy_second_home = None;
                }

                if let Some(v) = notify_command {
                    existing.notify_command = Some(v);
                } else if should_clear("notify_command", &clear) {
                    existing.notify_command = None;
                }

                if let Some(v) = policy_path {
                    existing.policy_path = Some(v);
                } else if should_clear("policy_path", &clear) {
                    existing.policy_path = None;
                }

                if let Some(v) = env_file {
                    existing.env_file = Some(v);
                } else if should_clear("env_file", &clear) {
                    existing.env_file = None;
                }

                if let Some(v) = env_file_prod {
                    existing.env_file_prod = Some(v);
                } else if should_clear("env_file_prod", &clear) {
                    existing.env_file_prod = None;
                }

                if !validation_commands.is_empty() {
                    existing.validation_commands = validation_commands;
                } else if should_clear("validation_commands", &clear) {
                    existing.validation_commands.clear();
                }

                if !auto_fix_commands.is_empty() {
                    existing.auto_fix_commands = auto_fix_commands;
                } else if should_clear("auto_fix_commands", &clear) {
                    existing.auto_fix_commands.clear();
                }

                config::save(&cfg, config_path.as_deref())?;
                println!("Updated profile '{}'", name);
            }
            ProfileCommands::Remove {
                name,
                config_path,
                force,
            } => {
                if !force {
                    eprintln!("Warning: Removing profile '{}' cannot be undone.", name);
                    eprintln!("Use --force to confirm.");
                    anyhow::bail!("Aborted");
                }
                let mut cfg = config::load(config_path.as_deref())?;
                config::remove_profile(&mut cfg, &name)?;
                config::save(&cfg, config_path.as_deref())?;
                println!("Removed profile '{}'", name);
            }
        },

        Commands::Report {
            since,
            profile,
            config_path,
            group_by,
            json,
            series,
            bucket,
        } => {
            report::run(report::ReportArgs {
                since,
                profile,
                config_path,
                group_by,
                json,
                series,
                bucket,
            })?;
        }

        Commands::Server { port, host } => {
            println!("Starting WebSocket server on {}:{}", host, port);
            server::run_blocking(&host, port)?;
            // This will run forever, so we don't need to return
            std::thread::park();
        }
        Commands::Telemetry { command } => match command {
            TelemetryCommands::Export {
                telemetry_repo_path,
                format,
                output,
                since,
                profile,
                group_by,
                generate_manifests,
                config_path,
            } => {
                let format_enum = format
                    .parse::<telemetry::exporter::ExportFormat>()
                    .map_err(|e| anyhow::anyhow!("Invalid format: {}", e))?;
                telemetry::cli::run_export(
                    telemetry_repo_path.as_deref(),
                    Some(format_enum),
                    output.as_deref(),
                    Some(&since),
                    profile.as_deref(),
                    Some(group_by),
                    generate_manifests,
                    config_path.as_deref(),
                )?;
            }
            TelemetryCommands::Status {
                telemetry_repo_path,
                config_path,
            } => {
                telemetry::cli::run_status(telemetry_repo_path.as_deref(), config_path.as_deref())?;
            }
        },
        Commands::Quota { command } => match command {
            QuotaCommands::Refresh {
                backend,
                model,
                command: cmd,
                store_path: store_arg,
            } => {
                let codex_cmd = cmd.unwrap_or_else(|| backend.clone());
                let path = store_arg
                    .map(PathBuf::from)
                    .unwrap_or_else(quota_store::store_path);
                match quota_store::refresh_codex_and_store(&codex_cmd, model.as_deref(), &path) {
                    Ok(Some(rec)) => {
                        println!(
                            "Refreshed {} {} quota: used={:?}% remaining={:?}% window={:?} reset={:?} (source={})",
                            rec.backend,
                            rec.model.as_deref().unwrap_or(""),
                            rec.quota_used_percent,
                            rec.quota_remaining_percent,
                            rec.quota_window,
                            rec.quota_reset_at,
                            rec.usage_source.as_deref().unwrap_or(""),
                        );
                    }
                    Ok(None) => {
                        println!(
                            "No account-level quota data from `{} status --json` (ok: nothing fabricated).",
                            codex_cmd
                        );
                    }
                    Err(e) => {
                        eprintln!("Quota refresh failed: {e}");
                        std::process::exit(1);
                    }
                }
            }
            QuotaCommands::List {
                json,
                store_path: store_arg,
            } => {
                let path = store_arg
                    .map(PathBuf::from)
                    .unwrap_or_else(quota_store::store_path);
                let records = quota_store::load(&path).unwrap_or_default();
                if json {
                    println!("{}", serde_json::to_string(&records)?);
                } else if records.is_empty() {
                    println!("No persisted quota observations.");
                } else {
                    for rec in &records {
                        println!(
                            "{} {}/{}: used={:?}% remaining={:?}% window={:?} reset={:?} ({})",
                            rec.observed_at.as_deref().unwrap_or(""),
                            rec.backend,
                            rec.model.as_deref().unwrap_or(""),
                            rec.quota_used_percent,
                            rec.quota_remaining_percent,
                            rec.quota_window,
                            rec.quota_reset_at,
                            rec.usage_source.as_deref().unwrap_or(""),
                        );
                    }
                }
            }
        },
    }
    Ok(())
}
