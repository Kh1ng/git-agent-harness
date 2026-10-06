//! MCP server for GAH: exposes the control-plane REST API as MCP tools.
//!
//! Usage: gah-mcp-server                      (stdio, for a co-located client)
//!        gah-mcp-server --http [--host IP] [--port PORT]   (streamable HTTP)
//!
//! Environment: GAH_SERVER_URL (default http://127.0.0.1:3773),
//! GAH_SERVER_TOKEN, GAH_PROFILE (default "gah").

use anyhow::{Context, Result};
use clap::Parser;
use git_agent_harness::mcp::serve::{run_http, run_stdio, DEFAULT_HTTP_HOST, DEFAULT_HTTP_PORT};
use git_agent_harness::mcp::McpConfig;

#[derive(Parser)]
#[command(
    name = "gah-mcp-server",
    version,
    about = "MCP server for the GAH control plane"
)]
struct Args {
    /// Serve MCP over streamable HTTP instead of stdio. Requires
    /// GAH_SERVER_TOKEN, which clients must present as a bearer token.
    #[arg(long)]
    http: bool,

    /// Address the HTTP listener binds. Falls back to GAH_MCP_HOST, then
    /// loopback.
    #[arg(long)]
    host: Option<String>,

    /// Port the HTTP listener binds. Falls back to GAH_MCP_PORT, then 3774.
    #[arg(long)]
    port: Option<u16>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    let config = McpConfig::from_env();
    if args.http {
        let host = args
            .host
            .or_else(|| std::env::var("GAH_MCP_HOST").ok())
            .unwrap_or_else(|| DEFAULT_HTTP_HOST.to_string());
        let port = match args.port {
            Some(port) => port,
            None => match std::env::var("GAH_MCP_PORT") {
                Ok(port) => port.parse().context("GAH_MCP_PORT is not a port number")?,
                Err(_) => DEFAULT_HTTP_PORT,
            },
        };
        run_http(&config, &host, port).await
    } else {
        run_stdio(&config).await
    }
}
