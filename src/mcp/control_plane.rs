//! HTTP client for the control-plane REST API (docs/openapi.yaml). Every MCP
//! tool call becomes exactly one request here; nothing is retried, so a
//! conflict or failure always reaches the caller.

use crate::curl_http;
use serde_json::Value;

/// Ordinary requests keep the five-minute ceiling the Node adapter inherited
/// from its HTTP client.
const DEFAULT_TIMEOUT_SECS: u32 = 300;
/// A dispatch that waits for completion may hold the request open for the
/// server's two-hour maximum wait, plus a minute for the response to arrive.
const DISPATCH_WAIT_TIMEOUT_SECS: u32 = 7_260;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    Get,
    Post,
}

impl Method {
    fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Post => "POST",
        }
    }
}

/// One control-plane request, fully determined by a tool call.
#[derive(Debug, Clone, PartialEq)]
pub struct ApiRequest {
    pub method: Method,
    /// Path and query, relative to the base URL.
    pub path: String,
    pub body: Option<Value>,
    /// The caller asked to wait for a dispatch to finish.
    pub wait_for_dispatch: bool,
}

impl ApiRequest {
    pub fn get(path: impl Into<String>) -> Self {
        Self {
            method: Method::Get,
            path: path.into(),
            body: None,
            wait_for_dispatch: false,
        }
    }

    pub fn post(path: impl Into<String>, body: Value) -> Self {
        Self {
            method: Method::Post,
            path: path.into(),
            body: Some(body),
            wait_for_dispatch: false,
        }
    }

    pub(super) fn timeout_secs(&self) -> u32 {
        if self.wait_for_dispatch {
            DISPATCH_WAIT_TIMEOUT_SECS
        } else {
            DEFAULT_TIMEOUT_SECS
        }
    }
}

#[derive(Debug)]
pub enum ApiError {
    /// The server answered with a non-2xx status.
    Status { message: String, status: u16 },
    /// The request never produced an HTTP response.
    Transport(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Status { message, status } => write!(f, "{message} (HTTP {status})"),
            Self::Transport(message) => f.write_str(message),
        }
    }
}

#[derive(Debug, Clone)]
pub struct ControlPlane {
    base_url: String,
    token: Option<String>,
}

impl ControlPlane {
    pub fn new(base_url: &str, token: Option<String>) -> Self {
        Self {
            base_url: base_url.strip_suffix('/').unwrap_or(base_url).to_string(),
            token: token.filter(|token| !token.is_empty()),
        }
    }

    /// Sends the request and returns the response body as indented JSON text.
    /// Blocks until the server answers; call from a blocking context.
    pub fn send(&self, request: &ApiRequest) -> Result<String, ApiError> {
        let url = format!("{}{}", self.base_url, request.path);
        let body = request.body.as_ref().map(Value::to_string);
        // Each mutation is one intended action: a fresh key per call lets the
        // server reject a replay, and no key is reserved for a read.
        let idempotency_key =
            (request.method == Method::Post).then(|| uuid::Uuid::new_v4().to_string());
        let response = curl_http::request_with_idempotency_key(
            request.method.as_str(),
            &url,
            body.as_deref(),
            self.token.as_deref(),
            request.timeout_secs(),
            idempotency_key.as_deref(),
        )
        .map_err(|error| ApiError::Transport(format!("{error:#}")))?;
        let text = String::from_utf8_lossy(&response.body);
        if !(200..300).contains(&response.status) {
            return Err(ApiError::Status {
                message: error_message(&text, response.status),
                status: response.status,
            });
        }
        Ok(pretty(&text))
    }
}

/// The server's own `message` when it sent one, else the raw body.
fn error_message(text: &str, status: u16) -> String {
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(map)) if map.contains_key("message") => match &map["message"] {
            Value::String(message) => message.clone(),
            other => other.to_string(),
        },
        _ if text.is_empty() => format!("HTTP {status}"),
        _ => text.to_string(),
    }
}

/// Re-indents a JSON document without reordering its keys. A body that is not
/// JSON is returned as a JSON string, and an empty body as `null`.
fn pretty(text: &str) -> String {
    if text.trim().is_empty() {
        return "null".to_string();
    }
    let mut out = Vec::new();
    let mut deserializer = serde_json::Deserializer::from_str(text);
    let mut serializer = serde_json::Serializer::pretty(&mut out);
    let transcoded = serde_transcode::transcode(&mut deserializer, &mut serializer).is_ok()
        && deserializer.end().is_ok();
    match String::from_utf8(out) {
        Ok(json) if transcoded => json,
        _ => Value::String(text.to_string()).to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_waiting_dispatch_gets_the_two_hour_allowance() {
        let mut request = ApiRequest::post("/api/dispatch", Value::Null);
        assert_eq!(request.timeout_secs(), 300);
        request.wait_for_dispatch = true;
        assert_eq!(request.timeout_secs(), 7_260);
    }

    #[test]
    fn pretty_keeps_the_servers_key_order() {
        assert_eq!(
            pretty(r#"{"b":1,"a":[]}"#),
            "{\n  \"b\": 1,\n  \"a\": []\n}"
        );
        assert_eq!(pretty("not json"), "\"not json\"");
        assert_eq!(pretty(""), "null");
    }

    #[test]
    fn error_message_prefers_the_servers_message() {
        assert_eq!(error_message(r#"{"message":"busy"}"#, 409), "busy");
        assert_eq!(error_message("upstream down", 502), "upstream down");
        assert_eq!(error_message("", 500), "HTTP 500");
    }
}
