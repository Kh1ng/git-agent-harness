//! Issue #525: request JSON schemas for the operations a client can invoke
//! directly. Authored here — next to the operations they belong to — and
//! generated into the shipped manifest, so clients (the MCP server) derive
//! their tool input schemas from the manifest instead of hand-maintaining a
//! second copy that drifts.
//!
//! The schemas use a deliberately small JSON-Schema subset: type:object with
//! string/number/boolean properties, per-property descriptions, and a
//! required list. Everything else a tool might accept is server-owned
//! context, not operator input.

use super::CapabilityManifest;
use serde_json::{json, Value};

fn object(properties: &[(&str, &str, bool, &str)]) -> Value {
    let mut map = serde_json::Map::new();
    let mut required = Vec::new();
    for (name, kind, is_required, description) in properties {
        map.insert(
            name.to_string(),
            json!({ "type": kind, "description": description }),
        );
        if *is_required {
            required.push(name.to_string());
        }
    }
    let mut schema = json!({ "type": "object", "properties": map });
    if !required.is_empty() {
        schema["required"] = json!(required);
    }
    schema
}

const PROFILE: &str = "GAH profile name; defaults to the configured default profile.";

pub(super) fn add_request_schemas(manifest: &mut CapabilityManifest) {
    let mut set = |operation_id: &str, schema: Value| {
        manifest.set_request_schema(operation_id, schema);
    };

    set(
        "status.get",
        object(&[("profile", "string", false, PROFILE)]),
    );
    set(
        "quota.snapshot",
        object(&[
            ("profile", "string", false, PROFILE),
            ("since", "string", false, "Usage window, e.g. \"7d\"."),
        ]),
    );
    set(
        "doctor.validate",
        object(&[("profile", "string", false, PROFILE)]),
    );
    set(
        "report.generate",
        object(&[
            ("profile", "string", false, PROFILE),
            ("since", "string", false, "Usage window, e.g. \"7d\"."),
            (
                "groupBy",
                "string",
                false,
                "Aggregation grouping: \"backend\" or \"model\".",
            ),
        ]),
    );
    set("profile.list", object(&[]));
    set(
        "ledger.work",
        object(&[(
            "work_id",
            "string",
            true,
            "Work item identifier, e.g. \"#123\".",
        )]),
    );
    set(
        "sync.classify",
        object(&[("profile", "string", false, PROFILE)]),
    );
    set(
        "ledger.summary",
        object(&[
            ("profile", "string", false, PROFILE),
            ("since", "string", false, "Usage window, e.g. \"7d\"."),
            (
                "groupBy",
                "string",
                false,
                "Aggregation grouping: \"backend\" or \"model\".",
            ),
        ]),
    );
    set(
        "ledger.clear_attempts",
        object(&[
            ("profile", "string", false, PROFILE),
            (
                "work_id",
                "string",
                true,
                "Work item whose attempt history is cleared.",
            ),
            (
                "dry_run",
                "boolean",
                false,
                "Preview what would be cleared without writing.",
            ),
        ]),
    );
    set("availability.get", object(&[]));
    set("availability.clear", object(&[]));
    set(
        "hold.set",
        object(&[
            ("profile", "string", false, PROFILE),
            ("work_id", "string", true, "Work item to hold."),
            (
                "reason",
                "string",
                false,
                "Why the hold was placed; recorded in the ledger.",
            ),
        ]),
    );
    set(
        "hold.clear",
        object(&[
            ("profile", "string", false, PROFILE),
            ("work_id", "string", true, "Work item to release."),
        ]),
    );
    set(
        "events.list",
        object(&[
            ("profile", "string", false, PROFILE),
            ("since", "string", false, "Usage window, e.g. \"7d\"."),
        ]),
    );
    let mut dispatch_schema = object(&[
        ("profile", "string", false, PROFILE),
        ("instanceId", "string", false, "Backend instance qualifier."),
        (
            "repo",
            "string",
            true,
            "Repository identifier (owner/name).",
        ),
        (
            "mode",
            "string",
            true,
            "Dispatch mode: improve, fix, pm, review, experiment.",
        ),
        ("branch", "string", false, "Worktree branch to create/use."),
        ("target", "string", false, "Target branch for the MR/PR."),
        (
            "mr",
            "string",
            false,
            "Existing MR/PR number to repair or review.",
        ),
        ("backend", "string", false, "Explicit backend override."),
        ("model", "string", false, "Explicit model override."),
        ("budget", "number", false, "Spend budget in dollars."),
        (
            "dryRun",
            "boolean",
            false,
            "Validate and plan without launching.",
        ),
        ("retries", "number", false, "Retry count after failure."),
        (
            "allowDraftFail",
            "boolean",
            false,
            "Allow publishing a draft MR even when checks fail.",
        ),
        (
            "providerKind",
            "string",
            false,
            "Provider for the work identity: github or gitlab.",
        ),
        (
            "waitForCompletion",
            "boolean",
            false,
            "Hold the HTTP request open until the run finishes (default true).",
        ),
        (
            "waitTimeoutSeconds",
            "number",
            false,
            "Wait ceiling in seconds (1-7200, default 3600).",
        ),
        (
            "requestId",
            "string",
            false,
            "Caller-supplied idempotency id.",
        ),
        ("nodeId", "string", false, "Worker node to run on."),
        (
            "coordinatorNodeId",
            "string",
            false,
            "Coordinator identity for remote runs.",
        ),
    ]);
    // Defaults (#525): a dispatch tool call waits for completion unless the
    // caller opts out — matching the server's own wait semantics.
    dispatch_schema["properties"]["waitForCompletion"]["default"] = json!(true);
    dispatch_schema["properties"]["waitTimeoutSeconds"]["default"] = json!(3600);
    set("dispatch.run", dispatch_schema);
    set(
        "telemetry.aggregate",
        object(&[
            (
                "dimensions",
                "string",
                true,
                "Comma-separated aggregation dimensions (project, ticket, backend, model, ...).",
            ),
            ("since", "string", false, "Range start (RFC3339 or date)."),
            ("until", "string", false, "Range end (RFC3339 or date)."),
            ("profile", "string", false, PROFILE),
            ("project", "string", false, "Filter by project/repo id."),
            ("ticket", "string", false, "Filter by ticket/work id."),
            (
                "execution_type",
                "string",
                false,
                "Filter by execution type (improve, fix, review).",
            ),
            (
                "backend_instance",
                "string",
                false,
                "Filter by backend instance.",
            ),
            ("provider", "string", false, "Filter by provider."),
            ("model", "string", false, "Filter by model."),
            ("account", "string", false, "Filter by account."),
        ]),
    );
    set(
        "claims.list",
        object(&[(
            "profile",
            "string",
            false,
            "Restrict to one profile's claims.",
        )]),
    );
    set("quota.list", object(&[]));
    set(
        "external_approval.inspect",
        object(&[
            ("profile", "string", true, PROFILE),
            ("work_id", "string", true, "Work item the approval scopes."),
            (
                "credential_label",
                "string",
                true,
                "Credential scope label, e.g. \"odds\".",
            ),
            (
                "operation_kind",
                "string",
                true,
                "Operation kind, e.g. \"env_credential\".",
            ),
        ]),
    );
    set(
        "route_approval.list",
        object(&[("profile", "string", false, PROFILE)]),
    );
    set(
        "route_approval.grant",
        object(&[
            ("profile", "string", true, PROFILE),
            (
                "work_id",
                "string",
                true,
                "Exact work item the approval applies to.",
            ),
            (
                "backend",
                "string",
                true,
                "Logical backend, e.g. \"opencode\".",
            ),
            (
                "backend_instance",
                "string",
                false,
                "Backend instance qualifier from the request, if any.",
            ),
            (
                "model",
                "string",
                false,
                "Exact model the request named, if any.",
            ),
        ]),
    );
    set(
        "route_approval.revoke",
        object(&[
            ("profile", "string", true, PROFILE),
            (
                "work_id",
                "string",
                true,
                "Exact work item the approval applied to.",
            ),
            ("backend", "string", true, "Logical backend."),
            (
                "backend_instance",
                "string",
                false,
                "Backend instance qualifier, if any.",
            ),
            ("model", "string", false, "Exact model, if any."),
        ]),
    );
}
