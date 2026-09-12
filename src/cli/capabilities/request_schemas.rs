//! Issue #525: request JSON schemas for every remotely available operation.
//! Authored here — next to the operations they belong to — and
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

fn string_arrays(mut schema: Value, fields: &[&str]) -> Value {
    for field in fields {
        schema["properties"][field]["type"] = json!("array");
        schema["properties"][field]["items"] = json!({ "type": "string" });
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

    set(
        "profile.remove",
        object(&[
            ("name", "string", true, "Profile name to remove."),
            (
                "force",
                "boolean",
                false,
                "Remove without an interactive confirmation.",
            ),
        ]),
    );

    let external_scope = object(&[
        ("profile", "string", true, PROFILE),
        (
            "work_id",
            "string",
            true,
            "Exact work item the approval scopes.",
        ),
        (
            "credential_label",
            "string",
            true,
            "Credential scope label.",
        ),
        ("operation_kind", "string", true, "External operation kind."),
    ]);
    set(
        "external_approval.list",
        object(&[("profile", "string", true, PROFILE)]),
    );
    set(
        "external_approval.request",
        object(&[
            ("profile", "string", true, PROFILE),
            (
                "work_id",
                "string",
                true,
                "Exact work item the approval scopes.",
            ),
            (
                "credential_label",
                "string",
                true,
                "Credential scope label.",
            ),
            ("operation_kind", "string", true, "External operation kind."),
            (
                "max_requests",
                "number",
                false,
                "Maximum approved request count.",
            ),
            (
                "max_dollars",
                "number",
                false,
                "Maximum approved spend in dollars.",
            ),
            ("expires_at", "string", false, "Approval expiry as RFC3339."),
            (
                "purpose",
                "string",
                false,
                "Reason the external operation is needed.",
            ),
        ]),
    );
    for operation_id in [
        "external_approval.grant",
        "external_approval.deny",
        "external_approval.revoke",
        "external_approval.expire",
    ] {
        set(operation_id, external_scope.clone());
    }

    set(
        "pm.plans.list",
        object(&[
            ("profile", "string", true, PROFILE),
            ("cursor", "string", false, "Opaque pagination cursor."),
            (
                "limit",
                "number",
                false,
                "Maximum plans to return (1-100; default 25).",
            ),
        ]),
    );
    set(
        "pm.plans.show",
        object(&[
            ("profile", "string", true, PROFILE),
            ("plan_id", "string", true, "Profile-scoped plan identifier."),
        ]),
    );
    set(
        "pm.publish",
        object(&[
            ("profile", "string", true, PROFILE),
            (
                "plan_id",
                "string",
                true,
                "Profile-scoped plan identifier; never a filesystem path.",
            ),
            (
                "expected_fingerprint",
                "string",
                true,
                "Fingerprint returned by pm show.",
            ),
            (
                "dry_run",
                "boolean",
                false,
                "Validate publication without provider writes.",
            ),
        ]),
    );

    set("config.show", object(&[]));
    set(
        "config.set",
        string_arrays(
            object(&[
                (
                    "current_manager",
                    "string",
                    false,
                    "Default manager backend.",
                ),
                (
                    "notification_channel",
                    "string",
                    false,
                    "Notification channel: none, telegram, or discord.",
                ),
                (
                    "telegram_chat_id",
                    "string",
                    false,
                    "Telegram chat identifier.",
                ),
                ("clear", "string", false, "Configuration fields to clear."),
            ]),
            &["clear"],
        ),
    );

    let profile_fields = object(&[
        ("name", "string", false, "Profile name."),
        (
            "display_name",
            "string",
            false,
            "Human-readable profile name.",
        ),
        ("repo_id", "string", false, "Stable repository identifier."),
        ("provider", "string", false, "Repository provider."),
        ("repo", "string", false, "Provider repository path."),
        (
            "local_path",
            "string",
            false,
            "Repository path on the control-plane node.",
        ),
        (
            "artifact_root",
            "string",
            false,
            "Artifact root on the control-plane node.",
        ),
        (
            "default_target_branch",
            "string",
            false,
            "Default merge target branch.",
        ),
        (
            "provider_api_base",
            "string",
            false,
            "Provider API base URL.",
        ),
        (
            "provider_project_id",
            "string",
            false,
            "Provider project identifier.",
        ),
        (
            "openhands_args",
            "string",
            false,
            "OpenHands CLI arguments.",
        ),
        ("codex_args", "string", false, "Codex CLI arguments."),
        (
            "codex_path",
            "string",
            false,
            "Codex executable path on the node.",
        ),
        ("claude_args", "string", false, "Claude CLI arguments."),
        (
            "claude_path",
            "string",
            false,
            "Claude executable path on the node.",
        ),
        (
            "agy_path",
            "string",
            false,
            "Agy executable path on the node.",
        ),
        ("vibe_args", "string", false, "Vibe CLI arguments."),
        (
            "vibe_path",
            "string",
            false,
            "Vibe executable path on the node.",
        ),
        ("opencode_args", "string", false, "OpenCode CLI arguments."),
        (
            "opencode_path",
            "string",
            false,
            "OpenCode executable path on the node.",
        ),
        (
            "agy_second_home",
            "string",
            false,
            "HOME override for agy-second.",
        ),
        (
            "notify_command",
            "string",
            false,
            "Profile notification command.",
        ),
        (
            "policy_path",
            "string",
            false,
            "Profile policy path on the node.",
        ),
        (
            "env_file",
            "string",
            false,
            "Development environment file on the node.",
        ),
        (
            "env_file_prod",
            "string",
            false,
            "Production environment file on the node.",
        ),
        (
            "validation_commands",
            "string",
            false,
            "Validation commands.",
        ),
        (
            "auto_fix_commands",
            "string",
            false,
            "Automatic fix commands.",
        ),
        (
            "max_parallel_workers",
            "number",
            false,
            "Maximum concurrent workers.",
        ),
        (
            "validation_timeout_seconds",
            "number",
            false,
            "Per-command validation timeout.",
        ),
        (
            "manager_wake_autonomy",
            "string",
            false,
            "Manager wake autonomy level.",
        ),
        ("clear", "string", false, "Profile fields to clear."),
    ]);
    let profile_fields = string_arrays(
        profile_fields,
        &[
            "openhands_args",
            "codex_args",
            "claude_args",
            "vibe_args",
            "opencode_args",
            "validation_commands",
            "auto_fix_commands",
            "clear",
        ],
    );
    let mut profile_add = profile_fields.clone();
    profile_add["required"] = json!([
        "name",
        "display_name",
        "repo_id",
        "provider",
        "repo",
        "local_path",
        "artifact_root"
    ]);
    profile_add["properties"]["default_target_branch"]["default"] = json!("main");
    set("profile.add", profile_add);
    let mut profile_set = profile_fields;
    profile_set["required"] = json!(["name"]);
    set("profile.set", profile_set);

    set(
        "backend_instance.set_enabled",
        object(&[
            ("profile", "string", true, PROFILE),
            (
                "instance",
                "string",
                true,
                "Configured backend-instance identifier.",
            ),
            ("enabled", "boolean", true, "Target enabled state."),
        ]),
    );

    set(
        "quota.refresh",
        object(&[
            (
                "backend",
                "string",
                false,
                "Backend to refresh (default codex).",
            ),
            (
                "backend_instance",
                "string",
                false,
                "Backend-instance qualifier.",
            ),
            ("model", "string", false, "Model qualifier."),
            (
                "quota_pool",
                "string",
                false,
                "Shared capacity or billing pool.",
            ),
        ]),
    );
    set(
        "claims.clear",
        object(&[
            ("profile", "string", true, PROFILE),
            ("work_id", "string", true, "Work claim to clear."),
        ]),
    );
    set(
        "claims.reclaim",
        object(&[
            ("profile", "string", true, PROFILE),
            (
                "max_age_secs",
                "number",
                false,
                "Minimum stale age in seconds (default 3600).",
            ),
        ]),
    );
    set(
        "ledger.repair_tail",
        object(&[(
            "dry_run",
            "boolean",
            false,
            "Inspect without modifying the ledger.",
        )]),
    );
    set(
        "ledger.reconcile",
        object(&[
            ("profile", "string", true, PROFILE),
            (
                "dry_run",
                "boolean",
                false,
                "Preview reconciliation without writing.",
            ),
        ]),
    );

    set(
        "config.routing_candidate.add",
        object(&[
            ("profile", "string", true, PROFILE),
            (
                "list",
                "string",
                true,
                "Routing list: pm, improve, review, or escalatory.",
            ),
            ("backend", "string", true, "Logical backend."),
            ("instance", "string", false, "Backend-instance qualifier."),
            ("model", "string", false, "Model qualifier."),
            ("quota_pool", "string", false, "Shared quota pool."),
            (
                "priority",
                "number",
                false,
                "Candidate priority (default 0).",
            ),
            (
                "included_in_quota",
                "boolean",
                false,
                "Whether quota includes this route.",
            ),
            (
                "marginal_cost_usd",
                "number",
                false,
                "Marginal cost in dollars.",
            ),
            (
                "requires_approval",
                "boolean",
                false,
                "Whether this route requires approval.",
            ),
        ]),
    );
    set(
        "config.routing_candidate.remove",
        object(&[
            ("profile", "string", true, PROFILE),
            ("list", "string", true, "Routing list to edit."),
            ("index", "number", true, "Zero-based candidate index."),
        ]),
    );
    set(
        "config.routing_candidate.move",
        object(&[
            ("profile", "string", true, PROFILE),
            ("list", "string", true, "Routing list to edit."),
            ("from", "number", true, "Current zero-based index."),
            ("to", "number", true, "Target zero-based index."),
        ]),
    );
}
