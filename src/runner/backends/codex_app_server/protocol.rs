//! Typed JSON-RPC 2.0 message shapes for the `codex app-server` protocol.
//!
//! The wire format is newline-delimited JSON over stdio (or a Unix
//! socket): one JSON-RPC 2.0 object per line, no `Content-Length` framing.
//! This was confirmed against the installed `codex-cli` binary and its
//! `codex app-server generate-json-schema` output (`JSONRPCRequest`,
//! `JSONRPCNotification`, `JSONRPCResponse`, `JSONRPCError`, `RequestId`).

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// JSON-RPC request/response correlation id. The generated `RequestId`
/// schema accepts either a string or an int64.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub(crate) enum RequestId {
    Number(i64),
    Text(String),
}

impl std::fmt::Display for RequestId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RequestId::Number(n) => write!(f, "{n}"),
            RequestId::Text(s) => write!(f, "{s}"),
        }
    }
}

/// A JSON-RPC error object (`JSONRPCErrorError` in the generated schema).
#[derive(Debug, Clone)]
pub(crate) struct JsonRpcError {
    pub(crate) code: i64,
    pub(crate) message: String,
    pub(crate) data: Option<Value>,
}

/// Why the raw JSON-RPC error is believed to indicate the app-server (or
/// upstream provider) is overloaded rather than a hard failure. Codex
/// surfaces this via `CodexErrorInfo::ServerOverloaded` in error `data` or
/// in a `TurnError`-shaped notification payload -- never via a dedicated
/// top-level JSON-RPC error code, so both shapes are checked.
pub(crate) fn is_overload_error(error: &JsonRpcError) -> bool {
    if codex_error_info(error.data.as_ref()).as_deref() == Some("serverOverloaded") {
        return true;
    }
    error.message.to_ascii_lowercase().contains("overload")
}

/// Same check for a notification's `params`, e.g. a `turn/failed`
/// notification carrying a `TurnError` with `codexErrorInfo`.
pub(crate) fn notification_is_overload(params: &Value) -> bool {
    if codex_error_info(Some(params)).as_deref() == Some("serverOverloaded") {
        return true;
    }
    codex_error_info(params.get("error")).as_deref() == Some("serverOverloaded")
}

fn codex_error_info(data: Option<&Value>) -> Option<String> {
    data?.get("codexErrorInfo")?.as_str().map(str::to_string)
}

/// A message received from the app-server, classified by shape rather
/// than by an exhaustive method enum -- the protocol is large and
/// versioned, and GAH must not choke on methods/fields it doesn't yet
/// model (see [`ServerEvent`]).
#[derive(Debug, Clone)]
pub(crate) enum IncomingMessage {
    /// A successful response to a request GAH sent.
    Response { id: RequestId, result: Value },
    /// An error response to a request GAH sent.
    ErrorResponse { id: RequestId, error: JsonRpcError },
    /// A request *from* the server (e.g. an approval prompt) that expects
    /// a response GAH must send back with the same `id`.
    ServerRequest {
        id: RequestId,
        method: String,
        params: Value,
    },
    /// A one-way notification from the server.
    Notification { method: String, params: Value },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ParseError {
    InvalidJson(String),
    UnrecognizedShape(String),
}

impl std::fmt::Display for ParseError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ParseError::InvalidJson(reason) => write!(f, "invalid JSON: {reason}"),
            ParseError::UnrecognizedShape(reason) => {
                write!(f, "unrecognized JSON-RPC message shape: {reason}")
            }
        }
    }
}

impl IncomingMessage {
    /// Parse one line of the newline-delimited wire format. Never panics
    /// on unexpected content -- returns [`ParseError`] so the caller can
    /// retain the raw line rather than silently dropping it.
    pub(crate) fn parse(line: &str) -> Result<IncomingMessage, ParseError> {
        let value: Value =
            serde_json::from_str(line).map_err(|e| ParseError::InvalidJson(e.to_string()))?;
        let obj = value
            .as_object()
            .ok_or_else(|| ParseError::UnrecognizedShape(line.to_string()))?;

        let method = obj
            .get("method")
            .and_then(|m| m.as_str())
            .map(str::to_string);

        let Some(id_value) = obj.get("id") else {
            let Some(method) = method else {
                return Err(ParseError::UnrecognizedShape(line.to_string()));
            };
            let params = obj.get("params").cloned().unwrap_or(Value::Null);
            return Ok(IncomingMessage::Notification { method, params });
        };

        let id: RequestId = serde_json::from_value(id_value.clone())
            .map_err(|_| ParseError::UnrecognizedShape(line.to_string()))?;

        if let Some(method) = method {
            let params = obj.get("params").cloned().unwrap_or(Value::Null);
            return Ok(IncomingMessage::ServerRequest { id, method, params });
        }
        if let Some(result) = obj.get("result") {
            return Ok(IncomingMessage::Response {
                id,
                result: result.clone(),
            });
        }
        if let Some(error) = obj.get("error") {
            let err_obj = error
                .as_object()
                .ok_or_else(|| ParseError::UnrecognizedShape(line.to_string()))?;
            let code = err_obj.get("code").and_then(|c| c.as_i64()).unwrap_or(0);
            let message = err_obj
                .get("message")
                .and_then(|m| m.as_str())
                .unwrap_or_default()
                .to_string();
            let data = err_obj.get("data").cloned();
            return Ok(IncomingMessage::ErrorResponse {
                id,
                error: JsonRpcError {
                    code,
                    message,
                    data,
                },
            });
        }
        Err(ParseError::UnrecognizedShape(line.to_string()))
    }
}

/// A server-originated method (notification or server-request) GAH has
/// observed. `known` distinguishes methods this ticket's typed layer
/// explicitly models from everything else -- unrecognized methods are
/// still retained as typed unknown events rather than dropped, per the
/// issue's "unknown methods/items are retained as typed unknown events"
/// acceptance criterion.
#[derive(Debug, Clone, Serialize)]
pub(crate) struct ServerEvent {
    pub(crate) method: String,
    pub(crate) params: Value,
    pub(crate) known: bool,
}

/// Methods this transport ticket explicitly interprets. Empty for now --
/// no ticket in this series yet consumes a specific notification's
/// contents -- but kept as a named seam so later tickets (thread/turn
/// event handling) grow this list instead of inventing a new mechanism.
const KNOWN_SERVER_METHODS: &[&str] = &[];

pub(crate) fn classify_server_event(method: String, params: Value) -> ServerEvent {
    let known = KNOWN_SERVER_METHODS.contains(&method.as_str());
    ServerEvent {
        method,
        params,
        known,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_response() {
        let msg = IncomingMessage::parse(r#"{"id":1,"result":{"ok":true}}"#).unwrap();
        match msg {
            IncomingMessage::Response { id, result } => {
                assert_eq!(id, RequestId::Number(1));
                assert_eq!(result, serde_json::json!({"ok": true}));
            }
            other => panic!("expected Response, got {other:?}"),
        }
    }

    #[test]
    fn parses_string_request_id() {
        let msg = IncomingMessage::parse(r#"{"id":"abc","result":null}"#).unwrap();
        match msg {
            IncomingMessage::Response { id, .. } => {
                assert_eq!(id, RequestId::Text("abc".to_string()));
            }
            other => panic!("expected Response, got {other:?}"),
        }
    }

    #[test]
    fn parses_error_response() {
        let msg = IncomingMessage::parse(
            r#"{"id":2,"error":{"code":-32600,"message":"bad request","data":{"codexErrorInfo":"serverOverloaded"}}}"#,
        )
        .unwrap();
        match msg {
            IncomingMessage::ErrorResponse { id, error } => {
                assert_eq!(id, RequestId::Number(2));
                assert_eq!(error.code, -32600);
                assert_eq!(error.message, "bad request");
                assert!(is_overload_error(&error));
            }
            other => panic!("expected ErrorResponse, got {other:?}"),
        }
    }

    #[test]
    fn parses_server_request() {
        let msg = IncomingMessage::parse(r#"{"id":3,"method":"execCommandApproval","params":{}}"#)
            .unwrap();
        assert!(matches!(msg, IncomingMessage::ServerRequest { .. }));
    }

    #[test]
    fn parses_notification() {
        let msg = IncomingMessage::parse(
            r#"{"method":"remoteControl/status/changed","params":{"status":"disabled"}}"#,
        )
        .unwrap();
        match msg {
            IncomingMessage::Notification { method, params } => {
                assert_eq!(method, "remoteControl/status/changed");
                assert_eq!(params["status"], "disabled");
            }
            other => panic!("expected Notification, got {other:?}"),
        }
    }

    #[test]
    fn rejects_malformed_json_without_panicking() {
        let err = IncomingMessage::parse("not json at all").unwrap_err();
        assert!(matches!(err, ParseError::InvalidJson(_)));
    }

    #[test]
    fn rejects_shape_with_neither_method_nor_result_nor_error() {
        let err = IncomingMessage::parse(r#"{"id":1,"unexpected":true}"#).unwrap_err();
        assert!(matches!(err, ParseError::UnrecognizedShape(_)));
    }

    #[test]
    fn overload_detected_via_message_text_fallback() {
        let error = JsonRpcError {
            code: -32000,
            message: "upstream provider overloaded, retry later".to_string(),
            data: None,
        };
        assert!(is_overload_error(&error));
    }

    #[test]
    fn non_overload_error_is_not_misclassified() {
        let error = JsonRpcError {
            code: -32601,
            message: "method not found".to_string(),
            data: None,
        };
        assert!(!is_overload_error(&error));
    }

    #[test]
    fn notification_overload_detected_from_turn_error_shape() {
        let params = serde_json::json!({
            "threadId": "t1",
            "turnId": "u1",
            "error": {"message": "boom", "codexErrorInfo": "serverOverloaded"}
        });
        assert!(notification_is_overload(&params));
    }

    #[test]
    fn unknown_method_is_retained_not_dropped() {
        let event = classify_server_event(
            "some/future-method-gah-does-not-model".to_string(),
            serde_json::json!({"anything": 1}),
        );
        assert!(!event.known);
        assert_eq!(event.method, "some/future-method-gah-does-not-model");
        assert_eq!(event.params["anything"], 1);
    }
}
