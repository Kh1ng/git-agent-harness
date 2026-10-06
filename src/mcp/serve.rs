//! MCP transports for the GAH tool surface: stdio for a co-located client and
//! streamable HTTP for a remote one.

use super::{GahTools, McpConfig, ToolDescriptor};
use anyhow::{bail, Context, Result};
use axum::extract::{Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    InitializeResult, ListToolsResult, PaginatedRequestParams, ServerCapabilities, Tool,
};
use rmcp::service::RequestContext;
use rmcp::transport::streamable_http_server::session::local::LocalSessionManager;
use rmcp::transport::streamable_http_server::{StreamableHttpServerConfig, StreamableHttpService};
use rmcp::{ErrorData, RoleServer, ServerHandler, ServiceExt};
use sha2::{Digest, Sha256};
use std::net::IpAddr;
use std::sync::Arc;

pub const DEFAULT_HTTP_HOST: &str = "127.0.0.1";
pub const DEFAULT_HTTP_PORT: u16 = 3774;
/// The streamable-HTTP endpoint path.
pub const HTTP_PATH: &str = "/mcp";

#[derive(Clone)]
struct GahMcpServer {
    tools: GahTools,
}

impl From<ToolDescriptor> for Tool {
    fn from(descriptor: ToolDescriptor) -> Self {
        Tool::new(
            descriptor.name,
            descriptor.description,
            Arc::new(descriptor.input_schema),
        )
        .with_title(descriptor.title)
    }
}

impl ServerHandler for GahMcpServer {
    fn get_info(&self) -> InitializeResult {
        InitializeResult::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("gah", env!("CARGO_PKG_VERSION")))
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        Ok(ListToolsResult::with_all_items(
            super::tool_descriptors()
                .into_iter()
                .map(Tool::from)
                .collect(),
        ))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, ErrorData> {
        let tools = self.tools.clone();
        let name = request.name.to_string();
        let arguments = request.arguments.unwrap_or_default();
        // The control-plane request blocks, for up to two hours on a
        // dispatch that waits; keep it off the async workers.
        let called = name.clone();
        let output = tokio::task::spawn_blocking(move || tools.call(&called, &arguments))
            .await
            .map_err(|error| ErrorData::internal_error(error.to_string(), None))?
            .ok_or_else(|| ErrorData::invalid_params(format!("Tool {name} not found"), None))?;
        let content = vec![ContentBlock::text(output.text)];
        Ok(if output.is_error {
            CallToolResult::error(content)
        } else {
            CallToolResult::success(content)
        }
        .into())
    }
}

/// Serves MCP over this process's stdin and stdout until the client leaves.
pub async fn run_stdio(config: &McpConfig) -> Result<()> {
    let server = GahMcpServer {
        tools: GahTools::new(config),
    };
    let service = server
        .serve(rmcp::transport::stdio())
        .await
        .context("starting the MCP stdio transport")?;
    eprintln!("gah-mcp-server listening on stdio");
    service.waiting().await.context("serving MCP over stdio")?;
    Ok(())
}

/// The streamable-HTTP application: one MCP endpoint at [`HTTP_PATH`] that
/// answers only requests carrying `token` as a bearer credential.
///
/// `allow_any_host` turns off the `Host` allow-list that otherwise limits
/// the endpoint to loopback names. That list defends an unauthenticated
/// local server against DNS rebinding; a remote client necessarily arrives
/// under another host name, and still has to present the token.
pub fn http_router(config: &McpConfig, token: &str, allow_any_host: bool) -> axum::Router {
    let tools = GahTools::new(config);
    let mut http_config = StreamableHttpServerConfig::default();
    if allow_any_host {
        http_config = http_config.disable_allowed_hosts();
    }
    let service = StreamableHttpService::new(
        move || {
            Ok(GahMcpServer {
                tools: tools.clone(),
            })
        },
        Arc::new(LocalSessionManager::default()),
        http_config,
    );
    let expected: [u8; 32] = Sha256::digest(format!("Bearer {token}")).into();
    axum::Router::new().nest_service(HTTP_PATH, service).layer(
        axum::middleware::from_fn_with_state(expected, require_bearer),
    )
}

async fn require_bearer(
    State(expected): State<[u8; 32]>,
    request: Request,
    next: Next,
) -> Response {
    let presented = request
        .headers()
        .get(header::AUTHORIZATION)
        .map(|value| Sha256::digest(value.as_bytes()));
    // Comparing digests keeps the comparison time independent of how much of
    // the token a guess got right.
    let authorized = presented.is_some_and(|digest| {
        digest
            .iter()
            .zip(expected)
            .fold(0, |diff, (a, b)| diff | (a ^ b))
            == 0
    });
    if authorized {
        next.run(request).await
    } else {
        (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            "Unauthorized",
        )
            .into_response()
    }
}

/// Serves MCP over streamable HTTP at `http://host:port/mcp` until the
/// process is stopped.
///
/// The listener refuses to start without `GAH_SERVER_TOKEN`: the endpoint
/// can mutate GAH, so it is never offered unauthenticated, even on loopback.
/// `host` must be a literal IP address.
pub async fn run_http(config: &McpConfig, host: &str, port: u16) -> Result<()> {
    let Some(token) = config.server_token.as_deref() else {
        bail!("the MCP HTTP listener requires GAH_SERVER_TOKEN; clients must send it as a bearer token");
    };
    let ip: IpAddr = host.parse().with_context(|| {
        format!("invalid bind host \"{host}\": expected a literal IPv4 or IPv6 address")
    })?;
    let listener = tokio::net::TcpListener::bind((ip, port))
        .await
        .with_context(|| format!("binding the MCP HTTP listener to {host}:{port}"))?;
    let address = listener.local_addr()?;
    eprintln!("gah-mcp-server listening on http://{address}{HTTP_PATH}");
    if !ip.is_loopback() {
        eprintln!(
            "gah-mcp-server is reachable beyond loopback on {address}; restrict network access and serve it over TLS or a private network"
        );
    }
    axum::serve(listener, http_router(config, token, !ip.is_loopback()))
        .await
        .context("serving MCP over HTTP")
}
