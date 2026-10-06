//! MCP (Model Context Protocol) tool surface for GAH. A thin adapter: each
//! tool call is translated into one HTTP request against the control-plane
//! REST API (docs/openapi.yaml) and the response is returned as text. It does
//! not re-implement any `gah` logic of its own.
//!
//! Tool input schemas come from the capability manifest compiled into this
//! binary, so the CLI, the contracts package, and MCP share one source of
//! truth. Tools backed by HTTP-only server routes have no CLI operation and
//! keep the hand-written schemas in [`http_only`].

mod control_plane;
mod input;
pub mod serve;

use control_plane::{ApiRequest, ControlPlane};
use input::{Args, Field, Kind};
use serde_json::{json, Map, Value};

pub const DEFAULT_SERVER_URL: &str = "http://127.0.0.1:3773";
pub const DEFAULT_PROFILE: &str = "gah";

/// Where the adapter sends requests and which profile a tool call means when
/// it names none.
#[derive(Debug, Clone)]
pub struct McpConfig {
    pub server_url: String,
    /// Sent as the control-plane bearer token; also the credential the
    /// streamable-HTTP listener demands from its own clients.
    pub server_token: Option<String>,
    pub default_profile: String,
}

impl McpConfig {
    /// Reads `GAH_SERVER_URL`, `GAH_SERVER_TOKEN`, and `GAH_PROFILE`.
    pub fn from_env() -> Self {
        Self {
            server_url: std::env::var("GAH_SERVER_URL")
                .unwrap_or_else(|_| DEFAULT_SERVER_URL.to_string()),
            server_token: std::env::var("GAH_SERVER_TOKEN")
                .ok()
                .filter(|token| !token.is_empty()),
            default_profile: std::env::var("GAH_PROFILE")
                .unwrap_or_else(|_| DEFAULT_PROFILE.to_string()),
        }
    }
}

/// How a tool's input schema is obtained.
enum Input {
    /// The tool takes no arguments.
    None,
    /// Derived from the manifest request schema of this CLI operation.
    Operation(&'static str),
    /// Hand-written, for a server route with no CLI operation.
    HttpOnly(fn() -> Vec<Field>),
}

struct ToolSpec {
    name: &'static str,
    title: &'static str,
    description: &'static str,
    input: Input,
    request: fn(&Args) -> ApiRequest,
}

/// Schemas for tools whose server routes have no CLI operation.
mod http_only {
    use super::{Field, Kind};

    fn profile() -> Field {
        Field::optional(
            "profile",
            Kind::String,
            "GAH profile name; defaults to GAH_PROFILE / \"gah\"",
        )
    }

    pub(super) fn profile_only() -> Vec<Field> {
        vec![profile()]
    }

    pub(super) fn usage_rollup() -> Vec<Field> {
        vec![
            profile(),
            Field::optional(
                "days",
                Kind::Integer { min: 1, max: 90 },
                "Number of days to include (1-90).",
            )
            .with_default(30.into()),
        ]
    }

    pub(super) fn controller_activity() -> Vec<Field> {
        vec![
            profile(),
            Field::optional("since", Kind::String, "e.g. \"24h\" or \"7d\""),
        ]
    }
}

const TOOLS: &[ToolSpec] = &[
    ToolSpec {
        name: "gah_info",
        title: "GAH server info",
        description: "Identify the connected GAH control-plane node and API version.",
        input: Input::None,
        request: |_| ApiRequest::get("/api/info"),
    },
    ToolSpec {
        name: "gah_cli_router",
        title: "GAH CLI router status",
        description: "Read-only snapshot of the CLI Proxy API connection status, settings, quotas, and allowed models.",
        input: Input::None,
        request: |_| ApiRequest::get("/api/cli-router"),
    },
    ToolSpec {
        name: "gah_status",
        title: "GAH status",
        description: "Full status snapshot for a profile: merge requests, blockers, availability, ledger summary.",
        input: Input::Operation("status.get"),
        request: |args| ApiRequest::get(query("/api/status", &[("profile", Some(args.profile()))])),
    },
    ToolSpec {
        name: "gah_quota",
        title: "GAH quota snapshot",
        description: "Usage/quota snapshot for a profile over a time window.",
        input: Input::Operation("quota.snapshot"),
        request: |args| {
            ApiRequest::get(query(
                "/api/quota",
                &[("profile", Some(args.profile())), ("since", args.text("since"))],
            ))
        },
    },
    ToolSpec {
        name: "gah_usage_rollup",
        title: "GAH usage rollup",
        description: "Actual manager-chat usage by day, backend, and model; use days=30 for a monthly view.",
        input: Input::HttpOnly(http_only::usage_rollup),
        request: |args| {
            ApiRequest::get(query(
                "/api/usage/rollup",
                &[("profile", Some(args.profile())), ("days", args.text("days"))],
            ))
        },
    },
    ToolSpec {
        name: "gah_doctor",
        title: "GAH doctor",
        description: "Run readiness checks for a profile (auth, config, backend availability).",
        input: Input::Operation("doctor.validate"),
        request: |args| ApiRequest::get(query("/api/doctor", &[("profile", Some(args.profile()))])),
    },
    ToolSpec {
        name: "gah_report",
        title: "GAH report",
        description: "Aggregate usage/cost/success-rate report, optionally grouped by backend or model.",
        input: Input::Operation("report.generate"),
        // No default profile: an omitted profile reports across all of them.
        request: |args| {
            ApiRequest::get(query(
                "/api/report",
                &[
                    ("profile", args.text("profile")),
                    ("since", args.text("since")),
                    ("groupBy", args.text("groupBy")),
                ],
            ))
        },
    },
    ToolSpec {
        name: "gah_profiles",
        title: "List GAH profiles",
        description: "List all configured GAH profiles.",
        input: Input::None,
        request: |_| ApiRequest::get("/api/profiles"),
    },
    ToolSpec {
        name: "gah_work_history",
        title: "Work item ledger history",
        description: "Full chronological ledger history (all attempts) for one work item.",
        input: Input::Operation("ledger.work"),
        request: |args| {
            let work_id = args.text("work_id").unwrap_or_default();
            ApiRequest::get(format!("/api/work/{}", encode_path_segment(&work_id)))
        },
    },
    ToolSpec {
        name: "gah_sync",
        title: "GAH sync",
        description: "Classified open (and recently resolved) merge requests/pull requests for a profile.",
        input: Input::Operation("sync.classify"),
        request: |args| ApiRequest::get(query("/api/sync", &[("profile", Some(args.profile()))])),
    },
    ToolSpec {
        name: "gah_ledger_summary",
        title: "GAH ledger summary",
        description: "Aggregate ledger counts (success/fail, by mode/backend/model, token usage) over a window.",
        input: Input::Operation("ledger.summary"),
        request: |args| {
            ApiRequest::get(query(
                "/api/ledger/summary",
                &[
                    ("profile", args.text("profile")),
                    ("since", args.text("since")),
                    ("groupBy", args.text("groupBy")),
                ],
            ))
        },
    },
    ToolSpec {
        name: "gah_ledger_clear_attempts",
        title: "Clear ledger attempts",
        description: "Append a tombstone ledger entry so a stuck work_id becomes dispatchable again.",
        input: Input::Operation("ledger.clear_attempts"),
        request: |args| {
            ApiRequest::post(
                "/api/ledger/clear-attempts",
                object(&[
                    ("profile", Some(args.profile().into())),
                    ("workId", args.value("work_id")),
                    ("dryRun", args.value("dry_run")),
                ]),
            )
        },
    },
    ToolSpec {
        name: "gah_availability",
        title: "GAH availability",
        description: "Durable backend/model availability state, global (not per-profile).",
        input: Input::None,
        request: |_| ApiRequest::get("/api/availability"),
    },
    ToolSpec {
        name: "gah_availability_clear",
        title: "Clear availability override",
        description: "Override a stale unavailable record once the backend is confirmed healthy again.",
        input: Input::Operation("availability.clear"),
        request: |args| {
            ApiRequest::post(
                "/api/availability/clear",
                object(&[
                    ("backend", args.value("backend")),
                    ("backendInstance", args.value("backendInstance")),
                    ("model", args.value("model")),
                    ("quotaPool", args.value("quotaPool")),
                ]),
            )
        },
    },
    ToolSpec {
        name: "gah_hold",
        title: "List review holds",
        description: "Work IDs currently under an out-of-band manager review hold for a profile.",
        input: Input::HttpOnly(http_only::profile_only),
        request: |args| ApiRequest::get(query("/api/hold", &[("profile", Some(args.profile()))])),
    },
    ToolSpec {
        name: "gah_hold_set",
        title: "Set a review hold",
        description: "Mark a work_id as under active out-of-band manager review; gah's auto-merge loop will skip it.",
        input: Input::Operation("hold.set"),
        request: |args| {
            ApiRequest::post(
                "/api/hold/set",
                object(&[
                    ("profile", Some(args.profile().into())),
                    ("workId", args.value("work_id")),
                    ("reason", args.value("reason")),
                ]),
            )
        },
    },
    ToolSpec {
        name: "gah_hold_clear",
        title: "Clear a review hold",
        description: "Release a previously set review hold on a work_id.",
        input: Input::Operation("hold.clear"),
        request: |args| {
            ApiRequest::post(
                "/api/hold/clear",
                object(&[
                    ("profile", Some(args.profile().into())),
                    ("workId", args.value("work_id")),
                ]),
            )
        },
    },
    ToolSpec {
        name: "gah_events",
        title: "GAH events",
        description: "Recent controller and dispatch events for a profile.",
        input: Input::Operation("events.list"),
        request: |args| {
            ApiRequest::get(query(
                "/api/events",
                &[("profile", Some(args.profile())), ("since", args.text("since"))],
            ))
        },
    },
    ToolSpec {
        name: "gah_controller_activity",
        title: "GAH controller activity",
        description: "Summarized agent/controller activity for a profile.",
        input: Input::HttpOnly(http_only::controller_activity),
        request: |args| {
            ApiRequest::get(query(
                "/api/controller-activity",
                &[("profile", Some(args.profile())), ("since", args.text("since"))],
            ))
        },
    },
    ToolSpec {
        name: "gah_loop_status",
        title: "GAH loop status",
        description: "Report whether the autonomous GAH loop is running for a profile.",
        input: Input::HttpOnly(http_only::profile_only),
        request: |args| ApiRequest::get(query("/api/loop/status", &[("profile", Some(args.profile()))])),
    },
    ToolSpec {
        name: "gah_dispatch",
        title: "Dispatch a GAH job",
        description: "Submit a dispatch as a fleet session and wait for its terminal push event by default. Set waitForCompletion=false to return immediately with the running session.",
        input: Input::Operation("dispatch.run"),
        request: |args| {
            let mut body = args.values().clone();
            body.insert("profile".into(), args.profile().into());
            let wait = body.get("waitForCompletion") == Some(&Value::Bool(true));
            let mut request = ApiRequest::post("/api/dispatch", Value::Object(body));
            request.wait_for_dispatch = wait;
            request
        },
    },
    // Paid-route approval tools: a manager agent can see stuck approval
    // requests and grant/revoke the exact scope through the same owner-gated
    // mutation API the dashboard uses (confirm carries the exact work item,
    // backend, account, and model).
    ToolSpec {
        name: "gah_route_approvals",
        title: "GAH paid-route approvals",
        description: "List pending and active paid-route approval requests for a profile (state, exact scope, consumption).",
        input: Input::Operation("route_approval.list"),
        request: |args| {
            ApiRequest::get(query("/api/route-approvals", &[("profile", Some(args.profile()))]))
        },
    },
    ToolSpec {
        name: "gah_route_approval_grant",
        title: "GAH paid-route approval grant",
        description: "Grant one exact paid backend/model route for one work item. The scope must match the pending request exactly — it cannot be broadened here.",
        input: Input::Operation("route_approval.grant"),
        request: |args| ApiRequest::post("/api/route-approvals/grant", route_scope(args)),
    },
    ToolSpec {
        name: "gah_route_approval_revoke",
        title: "GAH paid-route approval revoke",
        description: "Revoke a previously granted paid-route approval for one exact scope.",
        input: Input::Operation("route_approval.revoke"),
        request: |args| ApiRequest::post("/api/route-approvals/revoke", route_scope(args)),
    },
];

/// The exact scope of one paid-route approval. An absent instance or model is
/// sent as an explicit null: it is part of the scope, not an omission.
fn route_scope(args: &Args) -> Value {
    json!({
        "profile": args.value("profile"),
        "work_id": args.value("work_id"),
        "backend": args.value("backend"),
        "backend_instance": args.value("backend_instance"),
        "model": args.value("model"),
        "confirm": true,
    })
}

/// A JSON object holding only the entries that have a value.
fn object(entries: &[(&str, Option<Value>)]) -> Value {
    Value::Object(
        entries
            .iter()
            .filter_map(|(key, value)| Some((key.to_string(), value.clone()?)))
            .collect(),
    )
}

/// `path` plus a query string of the parameters that have a value.
fn query(path: &str, params: &[(&str, Option<String>)]) -> String {
    let mut serializer = url::form_urlencoded::Serializer::new(String::new());
    for (key, value) in params {
        if let Some(value) = value {
            serializer.append_pair(key, value);
        }
    }
    let encoded = serializer.finish();
    if encoded.is_empty() {
        path.to_string()
    } else {
        format!("{path}?{encoded}")
    }
}

/// Percent-encodes everything outside the URI unreserved marks, so a work id
/// such as `#123` stays inside its path segment.
fn encode_path_segment(segment: &str) -> String {
    let mut encoded = String::new();
    for byte in segment.bytes() {
        if byte.is_ascii_alphanumeric() || b"-_.!~*'()".contains(&byte) {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// Request schemas by operation id, from the manifest the Rust generator
/// emits (`cargo run --bin generate-cli-capabilities`).
fn request_schemas() -> &'static Map<String, Value> {
    static SCHEMAS: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();
    SCHEMAS.get_or_init(|| {
        let manifest: Value = serde_json::from_str(include_str!(
            "../packages/contracts/src/cli-capabilities.manifest.json"
        ))
        .expect("the committed capability manifest is valid JSON");
        match manifest.get("request_schemas") {
            Some(Value::Object(schemas)) => schemas.clone(),
            _ => Map::new(),
        }
    })
}

impl ToolSpec {
    /// The accepted arguments, or `None` for a tool that takes none.
    fn fields(&self) -> Option<Vec<Field>> {
        match self.input {
            Input::None => None,
            Input::Operation(operation_id) => {
                request_schemas().get(operation_id).map(input::fields)
            }
            Input::HttpOnly(fields) => Some(fields()),
        }
    }
}

/// What an MCP client sees when it lists tools.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolDescriptor {
    pub name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
    pub input_schema: Map<String, Value>,
}

pub fn tool_descriptors() -> Vec<ToolDescriptor> {
    TOOLS
        .iter()
        .map(|tool| ToolDescriptor {
            name: tool.name,
            title: tool.title,
            description: tool.description,
            input_schema: input::json_schema(tool.fields().as_deref()),
        })
        .collect()
}

/// The single text block a tool call returns.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolOutput {
    pub text: String,
    pub is_error: bool,
}

impl ToolOutput {
    fn error(text: String) -> Self {
        Self {
            text,
            is_error: true,
        }
    }
}

#[derive(Debug, Clone)]
pub struct GahTools {
    control_plane: ControlPlane,
    default_profile: String,
}

impl GahTools {
    pub fn new(config: &McpConfig) -> Self {
        Self {
            control_plane: ControlPlane::new(&config.server_url, config.server_token.clone()),
            default_profile: config.default_profile.clone(),
        }
    }

    /// Runs one tool call to completion, or returns `None` for an unknown
    /// tool. Failures the caller should see (rejected arguments, a non-2xx
    /// response, an unreachable server) come back as an error output.
    /// Blocks on the HTTP request; call from a blocking context.
    pub fn call(&self, name: &str, arguments: &Map<String, Value>) -> Option<ToolOutput> {
        let tool = TOOLS.iter().find(|tool| tool.name == name)?;
        let values = match input::parse(tool.fields().as_deref().unwrap_or_default(), arguments) {
            Ok(values) => values,
            Err(problem) => {
                return Some(ToolOutput::error(format!(
                    "MCP error -32602: Input validation error: Invalid arguments for tool {name}: {problem}"
                )))
            }
        };
        let request = (tool.request)(&Args::new(values, &self.default_profile));
        Some(match self.control_plane.send(&request) {
            Ok(text) => ToolOutput {
                text,
                is_error: false,
            },
            Err(error) => ToolOutput::error(error.to_string()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_cli_backed_tool_finds_its_schema_in_the_compiled_manifest() {
        for tool in TOOLS {
            if let Input::Operation(operation_id) = tool.input {
                assert!(
                    request_schemas().contains_key(operation_id),
                    "{} names operation {operation_id}, which has no request schema",
                    tool.name
                );
            }
        }
    }

    #[test]
    fn path_segments_and_queries_are_encoded() {
        assert_eq!(encode_path_segment("#12/a b"), "%2312%2Fa%20b");
        assert_eq!(
            query("/api/x", &[("a", Some("b c&d".into())), ("skip", None)]),
            "/api/x?a=b+c%26d"
        );
        assert_eq!(query("/api/x", &[("skip", None)]), "/api/x");
    }
}
