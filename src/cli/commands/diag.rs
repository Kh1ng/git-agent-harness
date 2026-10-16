//! `gah diag export`: download this machine's diagnostics bundle (memory
//! timeline, host and version details) from the local Parasaur server.
use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};

#[derive(Args)]
pub struct DiagArgs {
    #[command(subcommand)]
    action: Action,
}

#[derive(Subcommand)]
enum Action {
    /// Write the diagnostics bundle (.json.gz) to send to whoever is debugging.
    Export {
        /// Output file (default: ./parasaur-diagnostics-<time>.json.gz).
        #[arg(long)]
        out: Option<String>,
        /// Server to read from (default: GAH_SERVER_URL or http://127.0.0.1:3773).
        #[arg(long)]
        server: Option<String>,
    },
}

pub fn run(args: DiagArgs) -> Result<()> {
    let Action::Export { out, server } = args.action;
    let server = server
        .or_else(|| std::env::var("GAH_SERVER_URL").ok())
        .unwrap_or_else(|| crate::mcp::DEFAULT_SERVER_URL.to_string());
    let token = std::env::var("GAH_SERVER_TOKEN").ok();
    let url = format!("{}/api/diagnostics/export", server.trim_end_matches('/'));
    let response = crate::curl_http::request("GET", &url, None, token.as_deref(), 60)
        .with_context(|| format!("requesting {url}; is the Parasaur server running?"))?;
    if response.status != 200 {
        bail!(
            "{url} returned HTTP {}: {}",
            response.status,
            String::from_utf8_lossy(&response.body).trim()
        );
    }
    let out = out.unwrap_or_else(|| {
        format!(
            "parasaur-diagnostics-{}.json.gz",
            chrono::Utc::now().format("%Y-%m-%dT%H-%M-%SZ")
        )
    });
    std::fs::write(&out, &response.body).with_context(|| format!("writing {out}"))?;
    println!("Wrote {out} ({} KiB)", response.body.len() / 1024);
    Ok(())
}
