// Generated CLI Capability Manifest Types (Issue #531)
// This file is generated from Rust-owned types - do not edit manually
// Source: src/cli/capabilities.rs

// ---------------------------------------------------------------------------
// Core Types
// ---------------------------------------------------------------------------

export interface CapabilityManifest {
    schema_version: number;
    manifest_version: string;
    operations: Record<string, OperationDefinition>;
    command_path_to_operation_id: Record<string, string>;
    remote_operations: string[];
    local_only_operations: Record<string, LocalOnlyReason>;
    request_schemas?: Record<string, Record<string, unknown>>;
}

export type OperationClass = 'read' | 'mutation' | 'admin';
export type ProfileScope = 'global' | 'profile_required' | 'profile_optional';
export type StreamingBehavior = 'none' | 'sse' | 'web_socket' | 'jsonl';
export type Idempotency = 'idempotent' | 'non_idempotent' | 'conditional';
export type RemoteDisposition = 'remote_available' | 'local_only' | 'not_implemented';

export type LocalOnlyReason =
    | 'filesystem_access_required'
    | 'interactive_terminal_required'
    | 'security_sensitive'
    | 'development_only'
    | 'local_backend_execution_required'
    | 'legacy'
    | 'other';

export interface SchemaReference {
    rust_type: string;
    ts_type?: string | null;
    is_primitive: boolean;
}

export interface SecretFieldSpec {
    field_path: string;
    is_secret: boolean;
    may_contain_secrets: boolean;
}

export interface OperationDefinition {
    operation_id: string;
    display_name: string;
    class: OperationClass;
    profile_scope: ProfileScope;
    request_schema: SchemaReference | null;
    response_schema: SchemaReference | null;
    streaming: StreamingBehavior;
    idempotency: Idempotency;
    secret_fields: SecretFieldSpec[];
    remote_disposition: RemoteDisposition;
    local_only_reason: LocalOnlyReason | null;
    documentation: string | null;
    cli_command_path: string;
    is_stable: boolean;
}

// ---------------------------------------------------------------------------
// Manifest Data
// ---------------------------------------------------------------------------

/** The complete CLI capability manifest */
export const CLI_CAPABILITIES_MANIFEST: CapabilityManifest = {
  "schema_version": 1,
  "manifest_version": "v1",
  "operations": {
    "hold.clear": {
      "operation_id": "hold.clear",
      "display_name": "Clear Review Hold",
      "class": "mutation",
      "profile_scope": "profile_required",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Release a previously set review hold on a work_id",
      "cli_command_path": "gah hold clear",
      "is_stable": true
    },
    "claims.clear": {
      "operation_id": "claims.clear",
      "display_name": "Clear Work Claim",
      "class": "mutation",
      "profile_scope": "profile_required",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Clear a work claim",
      "cli_command_path": "gah claims clear",
      "is_stable": true
    },
    "profile.remove": {
      "operation_id": "profile.remove",
      "display_name": "Remove Profile",
      "class": "mutation",
      "profile_scope": "global",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "non_idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Remove a profile",
      "cli_command_path": "gah profile remove",
      "is_stable": true
    },
    "availability.clear": {
      "operation_id": "availability.clear",
      "display_name": "Clear Availability Status",
      "class": "mutation",
      "profile_scope": "global",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "non_idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Manually override stale availability",
      "cli_command_path": "gah availability clear",
      "is_stable": true
    },
    "ledger.clear_attempts": {
      "operation_id": "ledger.clear_attempts",
      "display_name": "Clear Ledger Attempts",
      "class": "mutation",
      "profile_scope": "profile_required",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "non_idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Mark all prior attempts for a work_id as stale",
      "cli_command_path": "gah ledger clear-attempts",
      "is_stable": true
    },
    "claims.reclaim": {
      "operation_id": "claims.reclaim",
      "display_name": "Reclaim Stale Claims",
      "class": "mutation",
      "profile_scope": "profile_required",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "non_idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Reclaim stale work claims",
      "cli_command_path": "gah claims reclaim",
      "is_stable": true
    },
    "telemetry.aggregate": {
      "operation_id": "telemetry.aggregate",
      "display_name": "Aggregate Telemetry",
      "class": "read",
      "profile_scope": "profile_optional",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Generate aggregated telemetry reports by routing dimensions",
      "cli_command_path": "gah telemetry aggregate",
      "is_stable": true
    },
    "hold.set": {
      "operation_id": "hold.set",
      "display_name": "Set Review Hold",
      "class": "mutation",
      "profile_scope": "profile_required",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Mark a work_id as under active out-of-band manager review",
      "cli_command_path": "gah hold set",
      "is_stable": true
    },
    "external_approval.request": {
      "operation_id": "external_approval.request",
      "display_name": "Request External Approval",
      "class": "mutation",
      "profile_scope": "profile_required",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [
        {
          "field_path": "credential_label",
          "is_secret": false,
          "may_contain_secrets": true
        },
        {
          "field_path": "max_dollars",
          "is_secret": false,
          "may_contain_secrets": false
        }
      ],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Record a requested external operation approval for a specific profile/repo/work item and credential label",
      "cli_command_path": "gah external-approval request",
      "is_stable": true
    },
    "profile.set": {
      "operation_id": "profile.set",
      "display_name": "Set Profile",
      "class": "mutation",
      "profile_scope": "global",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Set/Update fields of an existing profile",
      "cli_command_path": "gah profile set",
      "is_stable": true
    },
    "pm.plans.show": {
      "operation_id": "pm.plans.show",
      "display_name": "Read PM plans (gah pm show)",
      "class": "read",
      "profile_scope": "profile_required",
      "request_schema": null,
      "response_schema": {
        "rust_type": "PlanDetail",
        "ts_type": "PmPlanDetail",
        "is_primitive": false
      },
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Read bounded artifacts within a configured profile; IDs never accept filesystem paths.",
      "cli_command_path": "gah pm show",
      "is_stable": true
    },
    "events.list": {
      "operation_id": "events.list",
      "display_name": "List Controller Events",
      "class": "read",
      "profile_scope": "profile_optional",
      "request_schema": null,
      "response_schema": {
        "rust_type": "crate::events::ControllerEvent",
        "ts_type": "ControllerEvent[]",
        "is_primitive": false
      },
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Inspect the controller event stream",
      "cli_command_path": "gah events",
      "is_stable": true
    },
    "external_approval.expire": {
      "operation_id": "external_approval.expire",
      "display_name": "Expire External Approval",
      "class": "mutation",
      "profile_scope": "profile_required",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Expire an external approval immediately",
      "cli_command_path": "gah external-approval expire",
      "is_stable": true
    },
    "pm.plans.list": {
      "operation_id": "pm.plans.list",
      "display_name": "Read PM plans (gah pm plans)",
      "class": "read",
      "profile_scope": "profile_required",
      "request_schema": null,
      "response_schema": {
        "rust_type": "PlanList",
        "ts_type": "PmPlanList",
        "is_primitive": false
      },
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Read bounded artifacts within a configured profile; IDs never accept filesystem paths.",
      "cli_command_path": "gah pm plans",
      "is_stable": true
    },
    "profile.list": {
      "operation_id": "profile.list",
      "display_name": "List Profiles",
      "class": "read",
      "profile_scope": "global",
      "request_schema": null,
      "response_schema": {
        "rust_type": "crate::config::ProfileSummary",
        "ts_type": "ProfileSummary[]",
        "is_primitive": false
      },
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "List all profiles in config",
      "cli_command_path": "gah profile list",
      "is_stable": true
    },
    "profile.add": {
      "operation_id": "profile.add",
      "display_name": "Add Profile",
      "class": "mutation",
      "profile_scope": "global",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [
        {
          "field_path": "provider_api_base",
          "is_secret": false,
          "may_contain_secrets": true
        },
        {
          "field_path": "provider_project_id",
          "is_secret": false,
          "may_contain_secrets": true
        }
      ],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Add a new profile",
      "cli_command_path": "gah profile add",
      "is_stable": true
    },
    "tui.run": {
      "operation_id": "tui.run",
      "display_name": "Run TUI",
      "class": "read",
      "profile_scope": "profile_optional",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "local_only",
      "local_only_reason": "interactive_terminal_required",
      "documentation": "Interactive terminal UI - requires interactive terminal",
      "cli_command_path": "gah tui",
      "is_stable": true
    },
    "external_approval.revoke": {
      "operation_id": "external_approval.revoke",
      "display_name": "Revoke External Approval",
      "class": "mutation",
      "profile_scope": "profile_required",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Revoke an active external approval",
      "cli_command_path": "gah external-approval revoke",
      "is_stable": true
    },
    "pm.publish": {
      "operation_id": "pm.publish",
      "display_name": "Publish PM Plan",
      "class": "mutation",
      "profile_scope": "profile_required",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "non_idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Publish native provider issues. Remote callers must use a profile-scoped plan ID and reviewed fingerprint; --plan paths remain local-only.",
      "cli_command_path": "gah pm publish",
      "is_stable": true
    },
    "backend_instance.set_enabled": {
      "operation_id": "backend_instance.set_enabled",
      "display_name": "Set Backend Instance Enabled",
      "class": "mutation",
      "profile_scope": "profile_required",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Enable or disable one declared backend instance for a profile (#822)",
      "cli_command_path": "gah config set-backend-instance-enabled",
      "is_stable": true
    },
    "setup.memory_hooks": {
      "operation_id": "setup.memory_hooks",
      "display_name": "Set Up Agent Memory Hooks",
      "class": "mutation",
      "profile_scope": "global",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "local_only",
      "local_only_reason": "filesystem_access_required",
      "documentation": "Install shared memory hooks in selected local agent configurations",
      "cli_command_path": "gah setup memory-hooks",
      "is_stable": true
    },
    "ledger.reconcile": {
      "operation_id": "ledger.reconcile",
      "display_name": "Reconcile Ledger",
      "class": "mutation",
      "profile_scope": "profile_required",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Backfill dispatched work with later provider outcomes",
      "cli_command_path": "gah ledger reconcile",
      "is_stable": true
    },
    "config.routing_candidate.add": {
      "operation_id": "config.routing_candidate.add",
      "display_name": "Add Routing Candidate",
      "class": "mutation",
      "profile_scope": "profile_required",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Append a candidate to a profile's ordered routing list",
      "cli_command_path": "gah config routing-candidate add",
      "is_stable": true
    },
    "external_approval.inspect": {
      "operation_id": "external_approval.inspect",
      "display_name": "Inspect External Approval",
      "class": "read",
      "profile_scope": "profile_required",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Inspect the current external approval state for one exact scope",
      "cli_command_path": "gah external-approval inspect",
      "is_stable": true
    },
    "prune.sessions": {
      "operation_id": "prune.sessions",
      "display_name": "Prune Old Sessions",
      "class": "mutation",
      "profile_scope": "profile_optional",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "non_idempotent",
      "secret_fields": [],
      "remote_disposition": "local_only",
      "local_only_reason": "filesystem_access_required",
      "documentation": "Delete old GAH-owned sessions and worktrees - requires local filesystem access",
      "cli_command_path": "gah prune",
      "is_stable": true
    },
    "config.show": {
      "operation_id": "config.show",
      "display_name": "Show Configuration",
      "class": "read",
      "profile_scope": "global",
      "request_schema": null,
      "response_schema": {
        "rust_type": "crate::config_show::ConfigShowFull",
        "ts_type": "ConfigShowFull",
        "is_primitive": false
      },
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Show global defaults (e.g. current_manager)",
      "cli_command_path": "gah config show",
      "is_stable": true
    },
    "telemetry.export": {
      "operation_id": "telemetry.export",
      "display_name": "Export Telemetry",
      "class": "mutation",
      "profile_scope": "profile_optional",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "local_only",
      "local_only_reason": "filesystem_access_required",
      "documentation": "Export telemetry data to versioned repository - requires filesystem access",
      "cli_command_path": "gah telemetry export",
      "is_stable": true
    },
    "telemetry.status": {
      "operation_id": "telemetry.status",
      "display_name": "Telemetry Repository Status",
      "class": "read",
      "profile_scope": "profile_optional",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "local_only",
      "local_only_reason": "filesystem_access_required",
      "documentation": "Show local telemetry repository status",
      "cli_command_path": "gah telemetry status",
      "is_stable": true
    },
    "candidates.convert": {
      "operation_id": "candidates.convert",
      "display_name": "Convert Gate Findings to Candidates",
      "class": "mutation",
      "profile_scope": "global",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "local_only",
      "local_only_reason": "filesystem_access_required",
      "documentation": "Converts gate findings into backlog candidates - requires local filesystem",
      "cli_command_path": "gah candidates",
      "is_stable": true
    },
    "init.create": {
      "operation_id": "init.create",
      "display_name": "Initialize GAH Configuration",
      "class": "mutation",
      "profile_scope": "global",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [
        {
          "field_path": "provider_api_base",
          "is_secret": false,
          "may_contain_secrets": true
        },
        {
          "field_path": "provider_project_id",
          "is_secret": false,
          "may_contain_secrets": true
        }
      ],
      "remote_disposition": "local_only",
      "local_only_reason": "filesystem_access_required",
      "documentation": "Create or print a starter GAH config/profile - requires filesystem access",
      "cli_command_path": "gah init",
      "is_stable": true
    },
    "external_approval.grant": {
      "operation_id": "external_approval.grant",
      "display_name": "Grant External Approval",
      "class": "mutation",
      "profile_scope": "profile_required",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Grant the requested external approval scope",
      "cli_command_path": "gah external-approval grant",
      "is_stable": true
    },
    "external_approval.list": {
      "operation_id": "external_approval.list",
      "display_name": "List External Approvals",
      "class": "read",
      "profile_scope": "profile_required",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "List external approval scopes for a profile",
      "cli_command_path": "gah external-approval list",
      "is_stable": true
    },
    "external_approval.deny": {
      "operation_id": "external_approval.deny",
      "display_name": "Deny External Approval",
      "class": "mutation",
      "profile_scope": "profile_required",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Deny a pending external approval request",
      "cli_command_path": "gah external-approval deny",
      "is_stable": true
    },
    "update.cli": {
      "operation_id": "update.cli",
      "display_name": "Update GAH CLI",
      "class": "mutation",
      "profile_scope": "global",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "local_only",
      "local_only_reason": "local_backend_execution_required",
      "documentation": "Update the installed CLI - requires local execution",
      "cli_command_path": "gah update",
      "is_stable": true
    },
    "config.routing_candidate.move": {
      "operation_id": "config.routing_candidate.move",
      "display_name": "Move Routing Candidate",
      "class": "mutation",
      "profile_scope": "profile_required",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Reorder one candidate within a profile's ordered routing list",
      "cli_command_path": "gah config routing-candidate move",
      "is_stable": true
    },
    "loop.run": {
      "operation_id": "loop.run",
      "display_name": "Run Controller Loop",
      "class": "mutation",
      "profile_scope": "profile_required",
      "request_schema": null,
      "response_schema": {
        "rust_type": "crate::controller::ControllerActivity",
        "ts_type": "ControllerActivity",
        "is_primitive": false
      },
      "streaming": "sse",
      "idempotency": "non_idempotent",
      "secret_fields": [],
      "remote_disposition": "local_only",
      "local_only_reason": "local_backend_execution_required",
      "documentation": "Run the controller continuously - requires local backend execution",
      "cli_command_path": "gah loop",
      "is_stable": true
    },
    "quota.list": {
      "operation_id": "quota.list",
      "display_name": "List Quota Observations",
      "class": "read",
      "profile_scope": "global",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "List persisted account-level quota observations",
      "cli_command_path": "gah quota list",
      "is_stable": true
    },
    "route_approval.revoke": {
      "operation_id": "route_approval.revoke",
      "display_name": "Revoke Route Approval",
      "class": "mutation",
      "profile_scope": "profile_required",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Remove a previously granted paid-route approval",
      "cli_command_path": "gah route-approval revoke",
      "is_stable": true
    },
    "quota.snapshot": {
      "operation_id": "quota.snapshot",
      "display_name": "Quota Snapshot",
      "class": "read",
      "profile_scope": "profile_required",
      "request_schema": null,
      "response_schema": {
        "rust_type": "crate::quota_snapshot::QuotaSnapshot",
        "ts_type": "QuotaSnapshot",
        "is_primitive": false
      },
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Build the canonical profile-scoped quota snapshot",
      "cli_command_path": "gah quota snapshot",
      "is_stable": true
    },
    "sync.classify": {
      "operation_id": "sync.classify",
      "display_name": "Classify Merge Requests",
      "class": "read",
      "profile_scope": "profile_required",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Classify open GAH-created merge requests / pull requests",
      "cli_command_path": "gah sync",
      "is_stable": true
    },
    "claims.list": {
      "operation_id": "claims.list",
      "display_name": "List Work Claims",
      "class": "read",
      "profile_scope": "profile_optional",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "List work claims",
      "cli_command_path": "gah claims list",
      "is_stable": true
    },
    "route_approval.grant": {
      "operation_id": "route_approval.grant",
      "display_name": "Grant Route Approval",
      "class": "mutation",
      "profile_scope": "profile_required",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Allow one exact paid backend/model route for this work item",
      "cli_command_path": "gah route-approval grant",
      "is_stable": true
    },
    "status.get": {
      "operation_id": "status.get",
      "display_name": "Get Controller Status",
      "class": "read",
      "profile_scope": "profile_required",
      "request_schema": null,
      "response_schema": {
        "rust_type": "crate::status::StatusSnapshot",
        "ts_type": "StatusSnapshot",
        "is_primitive": false
      },
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Provide a single machine-readable controller snapshot of all state",
      "cli_command_path": "gah status",
      "is_stable": true
    },
    "report.generate": {
      "operation_id": "report.generate",
      "display_name": "Generate Report",
      "class": "read",
      "profile_scope": "profile_optional",
      "request_schema": null,
      "response_schema": {
        "rust_type": "crate::report::ReportData",
        "ts_type": "ReportData",
        "is_primitive": false
      },
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Generate backend/model comparison report",
      "cli_command_path": "gah report",
      "is_stable": true
    },
    "dispatch.run": {
      "operation_id": "dispatch.run",
      "display_name": "Dispatch Job",
      "class": "mutation",
      "profile_scope": "profile_required",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "non_idempotent",
      "secret_fields": [
        {
          "field_path": "model",
          "is_secret": false,
          "may_contain_secrets": true
        },
        {
          "field_path": "oh_profile",
          "is_secret": false,
          "may_contain_secrets": false
        }
      ],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Dispatch a job to a backend (improve, pm, review, fix, experiment)",
      "cli_command_path": "gah dispatch",
      "is_stable": true
    },
    "server.start": {
      "operation_id": "server.start",
      "display_name": "Start WebSocket Server",
      "class": "mutation",
      "profile_scope": "global",
      "request_schema": null,
      "response_schema": null,
      "streaming": "web_socket",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "local_only",
      "local_only_reason": "local_backend_execution_required",
      "documentation": "Start the WebSocket server for desktop/web interface - requires local execution",
      "cli_command_path": "gah server",
      "is_stable": true
    },
    "quota.refresh": {
      "operation_id": "quota.refresh",
      "display_name": "Refresh Quota",
      "class": "mutation",
      "profile_scope": "global",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [
        {
          "field_path": "backend_instance",
          "is_secret": false,
          "may_contain_secrets": true
        }
      ],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Refresh account-level quota and persist the observation",
      "cli_command_path": "gah quota refresh",
      "is_stable": true
    },
    "policy.check": {
      "operation_id": "policy.check",
      "display_name": "Check Repo Policy",
      "class": "read",
      "profile_scope": "global",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "local_only",
      "local_only_reason": "filesystem_access_required",
      "documentation": "Check repository policy from a local configuration file",
      "cli_command_path": "gah policy-check",
      "is_stable": true
    },
    "price_guard.check": {
      "operation_id": "price_guard.check",
      "display_name": "Check Model Price",
      "class": "read",
      "profile_scope": "global",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "local_only",
      "local_only_reason": "filesystem_access_required",
      "documentation": "Check a model against a local price watchlist",
      "cli_command_path": "gah price-guard",
      "is_stable": true
    },
    "ledger.repair_tail": {
      "operation_id": "ledger.repair_tail",
      "display_name": "Repair Ledger Tail",
      "class": "mutation",
      "profile_scope": "profile_optional",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Back up and remove one torn, unterminated final JSONL record",
      "cli_command_path": "gah ledger repair-tail",
      "is_stable": true
    },
    "profile.show": {
      "operation_id": "profile.show",
      "display_name": "Show Profile",
      "class": "read",
      "profile_scope": "global",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "local_only",
      "local_only_reason": "filesystem_access_required",
      "documentation": "Show profile details from local configuration",
      "cli_command_path": "gah profile show",
      "is_stable": true
    },
    "ledger.work": {
      "operation_id": "ledger.work",
      "display_name": "Ledger Work History",
      "class": "read",
      "profile_scope": "profile_optional",
      "request_schema": null,
      "response_schema": {
        "rust_type": "crate::ledger::LedgerEntry",
        "ts_type": "LedgerEntry[]",
        "is_primitive": false
      },
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Get full ledger history for one work item",
      "cli_command_path": "gah ledger work",
      "is_stable": true
    },
    "ledger.summary": {
      "operation_id": "ledger.summary",
      "display_name": "Ledger Summary",
      "class": "read",
      "profile_scope": "profile_optional",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Summarize recent ledger entries",
      "cli_command_path": "gah ledger summary",
      "is_stable": true
    },
    "config.set": {
      "operation_id": "config.set",
      "display_name": "Set Configuration",
      "class": "mutation",
      "profile_scope": "global",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Set one or more global default values",
      "cli_command_path": "gah config set",
      "is_stable": true
    },
    "route_approval.list": {
      "operation_id": "route_approval.list",
      "display_name": "List Paid Route Approvals",
      "class": "read",
      "profile_scope": "profile_required",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "List work-item paid-route requests and active grants",
      "cli_command_path": "gah route-approval list",
      "is_stable": true
    },
    "config.routing_candidate.remove": {
      "operation_id": "config.routing_candidate.remove",
      "display_name": "Remove Routing Candidate",
      "class": "mutation",
      "profile_scope": "profile_required",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Remove one candidate from a profile's ordered routing list",
      "cli_command_path": "gah config routing-candidate remove",
      "is_stable": true
    },
    "availability.get": {
      "operation_id": "availability.get",
      "display_name": "Get Availability Status",
      "class": "read",
      "profile_scope": "global",
      "request_schema": null,
      "response_schema": {
        "rust_type": "crate::availability::AvailabilityScope",
        "ts_type": "AvailabilityScope[]",
        "is_primitive": false
      },
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Get backend/model availability state",
      "cli_command_path": "gah availability",
      "is_stable": true
    },
    "node.register": {
      "operation_id": "node.register",
      "display_name": "Register Worker Node",
      "class": "mutation",
      "profile_scope": "profile_optional",
      "request_schema": null,
      "response_schema": null,
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [
        {
          "field_path": "secret_ref",
          "is_secret": true,
          "may_contain_secrets": true
        }
      ],
      "remote_disposition": "local_only",
      "local_only_reason": "security_sensitive",
      "documentation": "Register this host as a worker node against the central registry (issue #944)",
      "cli_command_path": "gah node register",
      "is_stable": true
    },
    "doctor.validate": {
      "operation_id": "doctor.validate",
      "display_name": "Validate Config and Profile Setup",
      "class": "read",
      "profile_scope": "profile_optional",
      "request_schema": null,
      "response_schema": {
        "rust_type": "crate::doctor::DoctorSnapshot",
        "ts_type": "DoctorSnapshot",
        "is_primitive": false
      },
      "streaming": "none",
      "idempotency": "idempotent",
      "secret_fields": [],
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Validate configuration and profile setup",
      "cli_command_path": "gah doctor",
      "is_stable": true
    }
  },
  "command_path_to_operation_id": {
    "gah availability": "availability.get",
    "gah telemetry aggregate": "telemetry.aggregate",
    "gah claims reclaim": "claims.reclaim",
    "gah loop": "loop.run",
    "gah pm plans": "pm.plans.list",
    "gah profile add": "profile.add",
    "gah node register": "node.register",
    "gah update": "update.cli",
    "gah config set-backend-instance-enabled": "backend_instance.set_enabled",
    "gah profile list": "profile.list",
    "gah prune": "prune.sessions",
    "gah doctor": "doctor.validate",
    "gah dispatch": "dispatch.run",
    "gah server": "server.start",
    "gah telemetry status": "telemetry.status",
    "gah price-guard": "price_guard.check",
    "gah setup memory-hooks": "setup.memory_hooks",
    "gah route-approval grant": "route_approval.grant",
    "gah external-approval revoke": "external_approval.revoke",
    "gah quota refresh": "quota.refresh",
    "gah profile show": "profile.show",
    "gah config routing-candidate remove": "config.routing_candidate.remove",
    "gah external-approval inspect": "external_approval.inspect",
    "gah status": "status.get",
    "gah availability clear": "availability.clear",
    "gah init": "init.create",
    "gah ledger work": "ledger.work",
    "gah external-approval grant": "external_approval.grant",
    "gah hold set": "hold.set",
    "gah profile remove": "profile.remove",
    "gah config set": "config.set",
    "gah external-approval expire": "external_approval.expire",
    "gah hold clear": "hold.clear",
    "gah ledger clear-attempts": "ledger.clear_attempts",
    "gah ledger repair-tail": "ledger.repair_tail",
    "gah sync": "sync.classify",
    "gah config routing-candidate move": "config.routing_candidate.move",
    "gah pm show": "pm.plans.show",
    "gah candidates": "candidates.convert",
    "gah pm publish": "pm.publish",
    "gah route-approval revoke": "route_approval.revoke",
    "gah external-approval request": "external_approval.request",
    "gah external-approval list": "external_approval.list",
    "gah external-approval deny": "external_approval.deny",
    "gah config show": "config.show",
    "gah profile set": "profile.set",
    "gah ledger summary": "ledger.summary",
    "gah quota snapshot": "quota.snapshot",
    "gah quota list": "quota.list",
    "gah claims list": "claims.list",
    "gah claims clear": "claims.clear",
    "gah events": "events.list",
    "gah ledger reconcile": "ledger.reconcile",
    "gah policy-check": "policy.check",
    "gah route-approval list": "route_approval.list",
    "gah config routing-candidate add": "config.routing_candidate.add",
    "gah report": "report.generate",
    "gah telemetry export": "telemetry.export",
    "gah tui": "tui.run"
  },
  "remote_operations": [
    "availability.get",
    "availability.clear",
    "doctor.validate",
    "ledger.summary",
    "ledger.work",
    "ledger.repair_tail",
    "ledger.reconcile",
    "ledger.clear_attempts",
    "hold.set",
    "hold.clear",
    "route_approval.list",
    "route_approval.grant",
    "route_approval.revoke",
    "backend_instance.set_enabled",
    "config.routing_candidate.add",
    "config.routing_candidate.remove",
    "config.routing_candidate.move",
    "external_approval.request",
    "external_approval.list",
    "external_approval.inspect",
    "external_approval.grant",
    "external_approval.deny",
    "external_approval.revoke",
    "external_approval.expire",
    "events.list",
    "status.get",
    "sync.classify",
    "dispatch.run",
    "pm.plans.list",
    "pm.plans.show",
    "pm.publish",
    "config.show",
    "config.set",
    "profile.list",
    "profile.add",
    "profile.set",
    "profile.remove",
    "report.generate",
    "telemetry.aggregate",
    "quota.refresh",
    "quota.list",
    "quota.snapshot",
    "claims.list",
    "claims.clear",
    "claims.reclaim"
  ],
  "local_only_operations": {
    "price_guard.check": "filesystem_access_required",
    "policy.check": "filesystem_access_required",
    "profile.show": "filesystem_access_required",
    "telemetry.status": "filesystem_access_required",
    "tui.run": "interactive_terminal_required",
    "candidates.convert": "filesystem_access_required",
    "init.create": "filesystem_access_required",
    "telemetry.export": "filesystem_access_required",
    "node.register": "security_sensitive",
    "setup.memory_hooks": "filesystem_access_required",
    "loop.run": "local_backend_execution_required",
    "update.cli": "local_backend_execution_required",
    "prune.sessions": "filesystem_access_required",
    "server.start": "local_backend_execution_required"
  },
  "request_schemas": {
    "dispatch.run": {
      "properties": {
        "allowDraftFail": {
          "description": "Allow publishing a draft MR even when checks fail.",
          "type": "boolean"
        },
        "backend": {
          "description": "Explicit backend override.",
          "type": "string"
        },
        "branch": {
          "description": "Worktree branch to create/use.",
          "type": "string"
        },
        "budget": {
          "description": "Spend budget in dollars.",
          "type": "number"
        },
        "coordinatorNodeId": {
          "description": "Coordinator identity for remote runs.",
          "type": "string"
        },
        "dryRun": {
          "description": "Validate and plan without launching.",
          "type": "boolean"
        },
        "instanceId": {
          "description": "Backend instance qualifier.",
          "type": "string"
        },
        "mode": {
          "description": "Dispatch mode: improve, fix, pm, review, experiment.",
          "type": "string"
        },
        "model": {
          "description": "Explicit model override.",
          "type": "string"
        },
        "mr": {
          "description": "Existing MR/PR number to repair or review.",
          "type": "string"
        },
        "nodeId": {
          "description": "Worker node to run on.",
          "type": "string"
        },
        "profile": {
          "description": "GAH profile name; defaults to the configured default profile.",
          "type": "string"
        },
        "providerKind": {
          "description": "Provider for the work identity: github or gitlab.",
          "type": "string"
        },
        "repo": {
          "description": "Repository identifier (owner/name).",
          "type": "string"
        },
        "requestId": {
          "description": "Caller-supplied idempotency id.",
          "type": "string"
        },
        "retries": {
          "description": "Retry count after failure.",
          "type": "number"
        },
        "target": {
          "description": "Target branch for the MR/PR.",
          "type": "string"
        },
        "waitForCompletion": {
          "default": true,
          "description": "Hold the HTTP request open until the run finishes (default true).",
          "type": "boolean"
        },
        "waitTimeoutSeconds": {
          "default": 3600,
          "description": "Wait ceiling in seconds (1-7200, default 3600).",
          "type": "number"
        }
      },
      "required": [
        "repo",
        "mode"
      ],
      "type": "object"
    },
    "availability.clear": {
      "properties": {},
      "type": "object"
    },
    "hold.clear": {
      "properties": {
        "profile": {
          "description": "GAH profile name; defaults to the configured default profile.",
          "type": "string"
        },
        "work_id": {
          "description": "Work item to release.",
          "type": "string"
        }
      },
      "required": [
        "work_id"
      ],
      "type": "object"
    },
    "telemetry.aggregate": {
      "properties": {
        "account": {
          "description": "Filter by account.",
          "type": "string"
        },
        "backend_instance": {
          "description": "Filter by backend instance.",
          "type": "string"
        },
        "dimensions": {
          "description": "Comma-separated aggregation dimensions (project, ticket, backend, model, ...).",
          "type": "string"
        },
        "execution_type": {
          "description": "Filter by execution type (improve, fix, review).",
          "type": "string"
        },
        "model": {
          "description": "Filter by model.",
          "type": "string"
        },
        "profile": {
          "description": "GAH profile name; defaults to the configured default profile.",
          "type": "string"
        },
        "project": {
          "description": "Filter by project/repo id.",
          "type": "string"
        },
        "provider": {
          "description": "Filter by provider.",
          "type": "string"
        },
        "since": {
          "description": "Range start (RFC3339 or date).",
          "type": "string"
        },
        "ticket": {
          "description": "Filter by ticket/work id.",
          "type": "string"
        },
        "until": {
          "description": "Range end (RFC3339 or date).",
          "type": "string"
        }
      },
      "required": [
        "dimensions"
      ],
      "type": "object"
    },
    "report.generate": {
      "properties": {
        "groupBy": {
          "description": "Aggregation grouping: \"backend\" or \"model\".",
          "type": "string"
        },
        "profile": {
          "description": "GAH profile name; defaults to the configured default profile.",
          "type": "string"
        },
        "since": {
          "description": "Usage window, e.g. \"7d\".",
          "type": "string"
        }
      },
      "type": "object"
    },
    "quota.list": {
      "properties": {},
      "type": "object"
    },
    "route_approval.grant": {
      "properties": {
        "backend": {
          "description": "Logical backend, e.g. \"opencode\".",
          "type": "string"
        },
        "backend_instance": {
          "description": "Backend instance qualifier from the request, if any.",
          "type": "string"
        },
        "model": {
          "description": "Exact model the request named, if any.",
          "type": "string"
        },
        "profile": {
          "description": "GAH profile name; defaults to the configured default profile.",
          "type": "string"
        },
        "work_id": {
          "description": "Exact work item the approval applies to.",
          "type": "string"
        }
      },
      "required": [
        "profile",
        "work_id",
        "backend"
      ],
      "type": "object"
    },
    "ledger.work": {
      "properties": {
        "work_id": {
          "description": "Work item identifier, e.g. \"#123\".",
          "type": "string"
        }
      },
      "required": [
        "work_id"
      ],
      "type": "object"
    },
    "external_approval.inspect": {
      "properties": {
        "credential_label": {
          "description": "Credential scope label, e.g. \"odds\".",
          "type": "string"
        },
        "operation_kind": {
          "description": "Operation kind, e.g. \"env_credential\".",
          "type": "string"
        },
        "profile": {
          "description": "GAH profile name; defaults to the configured default profile.",
          "type": "string"
        },
        "work_id": {
          "description": "Work item the approval scopes.",
          "type": "string"
        }
      },
      "required": [
        "profile",
        "work_id",
        "credential_label",
        "operation_kind"
      ],
      "type": "object"
    },
    "route_approval.list": {
      "properties": {
        "profile": {
          "description": "GAH profile name; defaults to the configured default profile.",
          "type": "string"
        }
      },
      "type": "object"
    },
    "profile.list": {
      "properties": {},
      "type": "object"
    },
    "claims.list": {
      "properties": {
        "profile": {
          "description": "Restrict to one profile's claims.",
          "type": "string"
        }
      },
      "type": "object"
    },
    "route_approval.revoke": {
      "properties": {
        "backend": {
          "description": "Logical backend.",
          "type": "string"
        },
        "backend_instance": {
          "description": "Backend instance qualifier, if any.",
          "type": "string"
        },
        "model": {
          "description": "Exact model, if any.",
          "type": "string"
        },
        "profile": {
          "description": "GAH profile name; defaults to the configured default profile.",
          "type": "string"
        },
        "work_id": {
          "description": "Exact work item the approval applied to.",
          "type": "string"
        }
      },
      "required": [
        "profile",
        "work_id",
        "backend"
      ],
      "type": "object"
    },
    "availability.get": {
      "properties": {},
      "type": "object"
    },
    "ledger.summary": {
      "properties": {
        "groupBy": {
          "description": "Aggregation grouping: \"backend\" or \"model\".",
          "type": "string"
        },
        "profile": {
          "description": "GAH profile name; defaults to the configured default profile.",
          "type": "string"
        },
        "since": {
          "description": "Usage window, e.g. \"7d\".",
          "type": "string"
        }
      },
      "type": "object"
    },
    "ledger.clear_attempts": {
      "properties": {
        "dry_run": {
          "description": "Preview what would be cleared without writing.",
          "type": "boolean"
        },
        "profile": {
          "description": "GAH profile name; defaults to the configured default profile.",
          "type": "string"
        },
        "work_id": {
          "description": "Work item whose attempt history is cleared.",
          "type": "string"
        }
      },
      "required": [
        "work_id"
      ],
      "type": "object"
    },
    "doctor.validate": {
      "properties": {
        "profile": {
          "description": "GAH profile name; defaults to the configured default profile.",
          "type": "string"
        }
      },
      "type": "object"
    },
    "hold.set": {
      "properties": {
        "profile": {
          "description": "GAH profile name; defaults to the configured default profile.",
          "type": "string"
        },
        "reason": {
          "description": "Why the hold was placed; recorded in the ledger.",
          "type": "string"
        },
        "work_id": {
          "description": "Work item to hold.",
          "type": "string"
        }
      },
      "required": [
        "work_id"
      ],
      "type": "object"
    },
    "events.list": {
      "properties": {
        "profile": {
          "description": "GAH profile name; defaults to the configured default profile.",
          "type": "string"
        },
        "since": {
          "description": "Usage window, e.g. \"7d\".",
          "type": "string"
        }
      },
      "type": "object"
    },
    "status.get": {
      "properties": {
        "profile": {
          "description": "GAH profile name; defaults to the configured default profile.",
          "type": "string"
        }
      },
      "type": "object"
    },
    "sync.classify": {
      "properties": {
        "profile": {
          "description": "GAH profile name; defaults to the configured default profile.",
          "type": "string"
        }
      },
      "type": "object"
    },
    "quota.snapshot": {
      "properties": {
        "profile": {
          "description": "GAH profile name; defaults to the configured default profile.",
          "type": "string"
        },
        "since": {
          "description": "Usage window, e.g. \"7d\".",
          "type": "string"
        }
      },
      "type": "object"
    }
  }
};

// ---------------------------------------------------------------------------
// Utility Functions
// ---------------------------------------------------------------------------

/** Get all operation IDs */
export function getAllOperationIds(): string[] {
    return Object.keys(CLI_CAPABILITIES_MANIFEST.operations);
}

/** Get all remote operations */
export function getRemoteOperations(): string[] {
    return CLI_CAPABILITIES_MANIFEST.remote_operations;
}

/** Get all local-only operations */
export function getLocalOnlyOperations(): Record<string, LocalOnlyReason> {
    return CLI_CAPABILITIES_MANIFEST.local_only_operations;
}

/** Check if an operation is remotely available */
export function isRemoteAvailable(operationId: string): boolean {
    const op = CLI_CAPABILITIES_MANIFEST.operations[operationId];
    return op?.remote_disposition === 'remote_available';
}

/** Get operation by command path */
export function getOperationByCommandPath(commandPath: string): OperationDefinition | null {
    const opId = CLI_CAPABILITIES_MANIFEST.command_path_to_operation_id[commandPath];
    if (!opId) return null;
    return CLI_CAPABILITIES_MANIFEST.operations[opId] || null;
}

/** Get operation class color for UI */
export function getOperationClassColor(opClass: OperationClass): string {
    switch (opClass) {
        case 'read': return 'blue';
        case 'mutation': return 'green';
        case 'admin': return 'red';
        default: return 'gray';
    }
}

/** Get remote disposition color for UI */
export function getRemoteDispositionColor(disposition: RemoteDisposition): string {
    switch (disposition) {
        case 'remote_available': return 'green';
        case 'local_only': return 'orange';
        case 'not_implemented': return 'red';
        default: return 'gray';
    }
}

// ---------------------------------------------------------------------------
// Manifest Validation Utilities
// ---------------------------------------------------------------------------

/** Validate that the manifest has all required fields */
export function validateManifest(): string[] {
    const errors: string[] = [];
    
    if (!CLI_CAPABILITIES_MANIFEST.schema_version) {
        errors.push('Missing schema_version');
    }
    if (!CLI_CAPABILITIES_MANIFEST.manifest_version) {
        errors.push('Missing manifest_version');
    }
    if (!CLI_CAPABILITIES_MANIFEST.operations || Object.keys(CLI_CAPABILITIES_MANIFEST.operations).length === 0) {
        errors.push('Missing or empty operations');
    }
    
    // Check that local-only operations have reasons
    for (const [opId, op] of Object.entries(CLI_CAPABILITIES_MANIFEST.operations)) {
        if (op.remote_disposition === 'local_only' && !op.local_only_reason) {
            errors.push("Local-only operation " + opId + " has no reason");
        }
    }
    
    // Check that command paths map to valid operations
    for (const [cmdPath, opId] of Object.entries(CLI_CAPABILITIES_MANIFEST.command_path_to_operation_id)) {
        if (!CLI_CAPABILITIES_MANIFEST.operations[opId]) {
            errors.push("Command path '" + cmdPath + "' references unknown operation '" + opId + "'");
        }
    }
    
    return errors;
}

/** Check if the manifest covers a specific command */
export function hasCommand(commandPath: string): boolean {
    return commandPath in CLI_CAPABILITIES_MANIFEST.command_path_to_operation_id;
}
