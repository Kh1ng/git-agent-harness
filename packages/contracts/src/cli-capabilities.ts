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
      "documentation": "Publish a validated PM plan artifact as native provider issues",
      "cli_command_path": "gah pm publish",
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
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Show details for a profile",
      "cli_command_path": "gah profile show",
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
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Show telemetry repository status",
      "cli_command_path": "gah telemetry status",
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
      "remote_disposition": "remote_available",
      "local_only_reason": null,
      "documentation": "Check repository policy for a given action",
      "cli_command_path": "gah policy-check",
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
    }
  },
  "command_path_to_operation_id": {
    "gah loop": "loop.run",
    "gah claims reclaim": "claims.reclaim",
    "gah availability clear": "availability.clear",
    "gah ledger reconcile": "ledger.reconcile",
    "gah external-approval request": "external_approval.request",
    "gah doctor": "doctor.validate",
    "gah update": "update.cli",
    "gah external-approval inspect": "external_approval.inspect",
    "gah tui": "tui.run",
    "gah quota list": "quota.list",
    "gah profile set": "profile.set",
    "gah ledger clear-attempts": "ledger.clear_attempts",
    "gah claims list": "claims.list",
    "gah profile add": "profile.add",
    "gah telemetry status": "telemetry.status",
    "gah quota snapshot": "quota.snapshot",
    "gah telemetry export": "telemetry.export",
    "gah candidates": "candidates.convert",
    "gah ledger summary": "ledger.summary",
    "gah ledger work": "ledger.work",
    "gah external-approval revoke": "external_approval.revoke",
    "gah route-approval grant": "route_approval.grant",
    "gah pm publish": "pm.publish",
    "gah claims clear": "claims.clear",
    "gah sync": "sync.classify",
    "gah route-approval revoke": "route_approval.revoke",
    "gah config show": "config.show",
    "gah config set": "config.set",
    "gah profile show": "profile.show",
    "gah server": "server.start",
    "gah dispatch": "dispatch.run",
    "gah events": "events.list",
    "gah profile remove": "profile.remove",
    "gah availability": "availability.get",
    "gah report": "report.generate",
    "gah hold clear": "hold.clear",
    "gah telemetry aggregate": "telemetry.aggregate",
    "gah status": "status.get",
    "gah external-approval expire": "external_approval.expire",
    "gah policy-check": "policy.check",
    "gah profile list": "profile.list",
    "gah prune": "prune.sessions",
    "gah hold set": "hold.set",
    "gah ledger repair-tail": "ledger.repair_tail",
    "gah init": "init.create",
    "gah external-approval grant": "external_approval.grant",
    "gah quota refresh": "quota.refresh"
  },
  "remote_operations": [
    "availability.get",
    "availability.clear",
    "policy.check",
    "doctor.validate",
    "ledger.summary",
    "ledger.work",
    "ledger.repair_tail",
    "ledger.reconcile",
    "ledger.clear_attempts",
    "hold.set",
    "hold.clear",
    "route_approval.grant",
    "route_approval.revoke",
    "external_approval.request",
    "external_approval.inspect",
    "external_approval.grant",
    "external_approval.revoke",
    "external_approval.expire",
    "events.list",
    "status.get",
    "sync.classify",
    "dispatch.run",
    "pm.publish",
    "config.show",
    "config.set",
    "profile.list",
    "profile.show",
    "profile.add",
    "profile.set",
    "profile.remove",
    "report.generate",
    "telemetry.status",
    "telemetry.aggregate",
    "quota.refresh",
    "quota.list",
    "quota.snapshot",
    "claims.list",
    "claims.clear",
    "claims.reclaim"
  ],
  "local_only_operations": {
    "server.start": "local_backend_execution_required",
    "candidates.convert": "filesystem_access_required",
    "tui.run": "interactive_terminal_required",
    "init.create": "filesystem_access_required",
    "loop.run": "local_backend_execution_required",
    "prune.sessions": "filesystem_access_required",
    "update.cli": "local_backend_execution_required",
    "telemetry.export": "filesystem_access_required"
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
