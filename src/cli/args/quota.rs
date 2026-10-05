//! Node-local credential and account observation command grammar.
use clap::Subcommand;

/// Quota/usage observation management (issue #151 / #166).
#[derive(Subcommand)]
pub enum CredentialCommands {
    /// List safe metadata only. No credential values or storage paths are returned.
    List {
        #[arg(long)]
        json: bool,
    },
    /// Save or rotate one private credential. Read the value only from stdin.
    Save {
        #[arg(long)]
        id: String,
        #[arg(long)]
        provider: String,
        #[arg(long, value_enum)]
        kind: crate::credentials::CredentialKind,
        #[arg(long)]
        account_label: String,
        #[arg(long)]
        env_var: Option<String>,
        #[arg(long)]
        json: bool,
    },
    /// Remove only this node's named source.
    Remove {
        #[arg(long)]
        id: String,
    },
}

/// Quota/usage observation management (issue #151 / #166).
#[derive(Subcommand)]
pub enum QuotaCommands {
    /// Record one validated, account-scoped quota observation from JSON on stdin.
    Record {
        #[arg(long, name = "store")]
        store_path: Option<String>,
    },
    /// Refresh account-level quota (e.g. Codex app-server, or the
    /// Mistral Admin API for `--backend vibe`) and persist the observation
    /// so the Quota/Telemetry pages show real data.
    Refresh {
        /// Select only this named node-local credential, without ambient fallback.
        #[arg(long, conflicts_with_all = ["backend", "backend_instance", "model", "quota_pool", "command"])]
        credential: Option<String>,
        /// Backend whose account quota to refresh (e.g. "codex"). "vibe"
        /// refreshes from the Mistral Admin API (`MISTRAL_ADMIN_API_KEY`);
        /// "nous" reads the Nous account API (`NOUS_API_KEY`);
        /// "claude" reads the current native Claude OAuth login;
        /// "agy" reads the current native Antigravity `/usage` command;
        /// "mistral-dashboard" reads the owner-only dashboard Cookie file.
        #[arg(long, default_value = "codex")]
        backend: String,
        /// Stable, secret-safe execution instance for this account reading.
        /// Omit to write a legacy instance-unknown observation. Not
        /// supported for `--backend vibe`: the Admin API key is a single
        /// org-wide credential, not a per-instance one.
        #[arg(long, visible_alias = "instance")]
        backend_instance: Option<String>,
        /// Model qualifier for the observation (usually unset for
        /// account-level readings).
        #[arg(long)]
        model: Option<String>,
        /// Shared capacity/billing pool for this observation.
        #[arg(long)]
        quota_pool: Option<String>,
        /// Path/command for the backend CLI (defaults to the backend name on
        /// PATH, e.g. "codex"). Codex and Antigravity have native usage parsers.
        /// Ignored for `--backend vibe`, which always uses the
        /// Mistral Admin API rather than a subprocess. Claude uses the current
        /// native OAuth login and rejects command overrides.
        #[arg(long)]
        command: Option<String>,
        /// Override the durable store path (default: $XDG_STATE_HOME/gah/...).
        /// Mainly for testing/automation.
        #[arg(long, name = "store")]
        store_path: Option<String>,
    },
    /// Refresh account-level quota for every configured profile's
    /// quota-tracked backends (codex, claude, agy, vibe, nous, mistral-dashboard), throttled to one live check per
    /// source per interval (14 min) and bounded so a hung backend can never
    /// wedge the caller. Runs each due refresh to completion before exiting
    /// (it JOINS the refresh threads, unlike the fire-and-forget loop-tick
    /// probe), so a systemd oneshot timer can run it safely. Intended for
    /// unattended invocation -- a systemd timer -- not the manual per-backend
    /// `refresh` command.
    AutoRefresh {
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
