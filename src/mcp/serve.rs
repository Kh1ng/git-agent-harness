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
pub fn http_router(
    config: &McpConfig,
    token: &str,
    host: &str,
    allow_any_host: bool,
) -> axum::Router {
    let tools = GahTools::new(config);
    let mut http_config = StreamableHttpServerConfig::default();
    http_config = http_config.with_allowed_hosts(vec![
        "localhost".to_string(),
        "127.0.0.1".to_string(),
        "[::1]".to_string(),
        host.to_string(),
    ]);
    if allow_any_host {
        http_config = http_config.disable_allowed_hosts();
    }
    let service = StreamableHttpService::new(
        move || {
            Ok(GahMcpServer {
                tools: tools.clone(),
            })
        },
        http_session_manager(),
        http_config,
    );
    let expected: [u8; 32] = Sha256::digest(format!("Bearer {token}")).into();
    axum::Router::new().nest_service(HTTP_PATH, service).layer(
        axum::middleware::from_fn_with_state(expected, require_bearer),
    )
}

/// Keep sessions alive beyond the longest outbound request, including time to
/// deliver its result. rmcp's idle timer also runs while tool calls are pending.
fn http_session_manager() -> Arc<LocalSessionManager> {
    let mut manager = LocalSessionManager::default();
    manager.session_config.keep_alive = Some(std::time::Duration::from_secs(
        u64::from(super::control_plane::DISPATCH_WAIT_TIMEOUT_SECS) + 60,
    ));
    Arc::new(manager)
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
    axum::serve(
        listener,
        http_router(config, token, host, !ip.is_loopback()),
    )
    .await
    .context("serving MCP over HTTP")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::{to_bytes, Body};
    use serde_json::{json, Value};
    use std::time::Duration;
    use tokio::sync::Notify;
    use tower::ServiceExt;

    #[derive(Clone)]
    struct PendingDispatch {
        started: Arc<Notify>,
    }

    impl ServerHandler for PendingDispatch {
        async fn call_tool(
            &self,
            request: CallToolRequestParams,
            _context: RequestContext<RoleServer>,
        ) -> Result<CallToolResponse, ErrorData> {
            assert_eq!(request.name, "gah_dispatch");
            self.started.notify_one();
            tokio::time::sleep(Duration::from_secs(u64::from(
                super::super::control_plane::DISPATCH_WAIT_TIMEOUT_SECS,
            )))
            .await;
            Ok(CallToolResult::success(vec![ContentBlock::text("dispatch completed")]).into())
        }
    }

    fn post(message: Value, session: Option<&str>) -> Request {
        let mut request = Request::builder()
            .method("POST")
            .uri(HTTP_PATH)
            .header("host", "localhost")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream");
        if let Some(session) = session {
            request = request.header("mcp-session-id", session);
        }
        request.body(Body::from(message.to_string())).unwrap()
    }

    #[tokio::test(start_paused = true)]
    async fn pending_http_dispatch_survives_default_session_idle_timeout() {
        let started = Arc::new(Notify::new());
        let handler = PendingDispatch {
            started: started.clone(),
        };
        let service = StreamableHttpService::new(
            move || Ok(handler.clone()),
            http_session_manager(),
            StreamableHttpServerConfig::default(),
        );
        let router = axum::Router::new().nest_service(HTTP_PATH, service);
        let initialized = router
            .clone()
            .oneshot(post(
                json!({"jsonrpc":"2.0", "id":1, "method":"initialize", "params":{
                    "protocolVersion":"2025-06-18", "capabilities":{},
                    "clientInfo":{"name":"test", "version":"1"}
                }}),
                None,
            ))
            .await
            .unwrap();
        assert_eq!(initialized.status(), StatusCode::OK);
        let session = initialized.headers()["mcp-session-id"]
            .to_str()
            .unwrap()
            .to_owned();
        to_bytes(initialized.into_body(), usize::MAX).await.unwrap();
        let notified = router
            .clone()
            .oneshot(post(
                json!({"jsonrpc":"2.0", "method":"notifications/initialized"}),
                Some(&session),
            ))
            .await
            .unwrap();
        assert_eq!(notified.status(), StatusCode::ACCEPTED);
        let pending = tokio::spawn(async move {
            let response = router
                .oneshot(post(
                    json!({"jsonrpc":"2.0", "id":2, "method":"tools/call", "params":{
                        "name":"gah_dispatch", "arguments":{"waitForCompletion":true}
                    }}),
                    Some(&session),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::OK);
            to_bytes(response.into_body(), usize::MAX).await.unwrap()
        });
        started.notified().await;
        // No further client traffic while the dispatch is pending.
        tokio::time::advance(Duration::from_secs(301)).await;
        for _ in 0..10 {
            tokio::task::yield_now().await;
        }
        assert!(!pending.is_finished(), "session expired during dispatch");
        tokio::time::advance(Duration::from_secs(
            u64::from(super::super::control_plane::DISPATCH_WAIT_TIMEOUT_SECS) - 301,
        ))
        .await;
        let body = pending.await.unwrap();
        let body = String::from_utf8(body.to_vec()).unwrap();
        let message: Value = body
            .lines()
            .filter_map(|line| line.strip_prefix("data: "))
            .filter_map(|data| serde_json::from_str::<Value>(data).ok())
            .find(|message| message["id"] == 2)
            .expect("dispatch response survived the HTTP session");
        assert_eq!(
            message["result"]["content"][0]["text"],
            "dispatch completed"
        );
    }

    #[tokio::test]
    async fn configured_loopback_host_is_accepted() {
        let config = McpConfig {
            server_url: String::new(),
            server_token: None,
            default_profile: String::new(),
        };
        let router = http_router(&config, "test-token", "127.0.0.2", false);

        let request = Request::builder()
            .method("POST")
            .uri(HTTP_PATH)
            .header("host", "127.0.0.2:3774")
            .header("authorization", "Bearer test-token")
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .body(Body::from(
                json!({"jsonrpc":"2.0", "id":1, "method":"initialize", "params":{
                    "protocolVersion":"2025-06-18", "capabilities":{},
                    "clientInfo":{"name":"test", "version":"1"}
                }})
                .to_string(),
            ))
            .unwrap();

        let response = router.oneshot(request).await.unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }
}
