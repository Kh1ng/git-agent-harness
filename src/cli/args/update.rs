//! `gah update` command grammar.
use std::path::PathBuf;

#[derive(clap::Args)]
pub struct UpdateArgs {
    /// Repository checkout to update (defaults to the current checkout).
    #[arg(long)]
    pub repo: Option<PathBuf>,
    /// Fetch and fast-forward the checkout before building (opt-in).
    #[arg(long)]
    pub pull: bool,
    /// Agent integrations to install; existing assets are also refreshed.
    /// Repeat or use comma-separated names.
    #[arg(long, value_delimiter = ',', value_parser = ["claude", "codex", "opencode", "vibe"])]
    pub agent: Vec<String>,
    /// Accept the printed installation plan without prompting.
    #[arg(long)]
    pub yes: bool,
    /// "central" (builds/serves the control plane, default) or "worker"
    /// (CLI + dispatch loop only -- never builds apps/server or touches
    /// gah-server.service).
    #[arg(long, default_value = "central")]
    pub role: String,
    /// Restart the system-wide control-plane service after a successful build.
    #[arg(long, default_value_t = false)]
    pub restart_server: bool,
    #[arg(long, default_value = "gah-server.service")]
    pub server_service: String,
    /// Install published release artifacts (issue #1416) instead of
    /// rebuilding from source: no git pull, no cargo, no npm build. The
    /// checkout stays the deployment root; only the artifact source changes.
    #[arg(long, default_value_t = false)]
    pub from_release: bool,
    /// URL or local path of the release manifest (edge-manifest.json).
    /// Defaults to the edge channel feed derived from the checkout's
    /// origin remote.
    #[arg(long)]
    pub release_manifest: Option<String>,
}
