//! CLI capability operation definitions, grouped by command module (Issue #531).
//!
//! Split out of `capabilities.rs` to keep that file under the repo's line-count
//! guard; these functions are purely data population for `generate_manifest`.

use super::*;

// Operation definitions for each CLI command module

pub(super) fn add_availability_operations(manifest: &mut CapabilityManifest) {
    // gah availability
    manifest.add_operation(OperationDefinition {
        operation_id: "availability.get".to_string(),
        display_name: "Get Availability Status".to_string(),
        class: OperationClass::Read,
        profile_scope: ProfileScope::Global,
        request_schema: None,
        response_schema: Some(SchemaReference {
            rust_type: "crate::availability::AvailabilityScope".to_string(),
            ts_type: Some("AvailabilityScope[]".to_string()),
            is_primitive: false,
        }),
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some("Get backend/model availability state".to_string()),
        cli_command_path: "gah availability".to_string(),
        is_stable: true,
    });

    // gah availability clear
    manifest.add_operation(OperationDefinition {
        operation_id: "availability.clear".to_string(),
        display_name: "Clear Availability Status".to_string(),
        class: OperationClass::Mutation,
        profile_scope: ProfileScope::Global,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::NonIdempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some("Manually override stale availability".to_string()),
        cli_command_path: "gah availability clear".to_string(),
        is_stable: true,
    });
}

pub(super) fn add_candidates_operations(manifest: &mut CapabilityManifest) {
    manifest.add_operation(OperationDefinition {
        operation_id: "candidates.convert".to_string(),
        display_name: "Convert Gate Findings to Candidates".to_string(),
        class: OperationClass::Mutation,
        profile_scope: ProfileScope::Global,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::LocalOnly,
        local_only_reason: Some(LocalOnlyReason::FilesystemAccessRequired),
        documentation: Some(
            "Converts gate findings into backlog candidates - requires local filesystem"
                .to_string(),
        ),
        cli_command_path: "gah candidates".to_string(),
        is_stable: true,
    });
}

pub(super) fn add_policy_operations(manifest: &mut CapabilityManifest) {
    manifest.add_operation(OperationDefinition {
        operation_id: "policy.check".to_string(),
        display_name: "Check Repo Policy".to_string(),
        class: OperationClass::Read,
        profile_scope: ProfileScope::Global,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some("Check repository policy for a given action".to_string()),
        cli_command_path: "gah policy-check".to_string(),
        is_stable: true,
    });
}

pub(super) fn add_doctor_operations(manifest: &mut CapabilityManifest) {
    manifest.add_operation(OperationDefinition {
        operation_id: "doctor.validate".to_string(),
        display_name: "Validate Config and Profile Setup".to_string(),
        class: OperationClass::Read,
        profile_scope: ProfileScope::ProfileOptional,
        request_schema: None,
        response_schema: Some(SchemaReference {
            rust_type: "crate::doctor::DoctorSnapshot".to_string(),
            ts_type: Some("DoctorSnapshot".to_string()),
            is_primitive: false,
        }),
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some("Validate configuration and profile setup".to_string()),
        cli_command_path: "gah doctor".to_string(),
        is_stable: true,
    });
}

pub(super) fn add_update_operations(manifest: &mut CapabilityManifest) {
    manifest.add_operation(OperationDefinition {
        operation_id: "update.cli".to_string(),
        display_name: "Update GAH CLI".to_string(),
        class: OperationClass::Mutation,
        profile_scope: ProfileScope::Global,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::LocalOnly,
        local_only_reason: Some(LocalOnlyReason::LocalBackendExecutionRequired),
        documentation: Some("Update the installed CLI - requires local execution".to_string()),
        cli_command_path: "gah update".to_string(),
        is_stable: true,
    });
}

pub(super) fn add_init_operations(manifest: &mut CapabilityManifest) {
    manifest.add_operation(OperationDefinition {
        operation_id: "init.create".to_string(),
        display_name: "Initialize GAH Configuration".to_string(),
        class: OperationClass::Mutation,
        profile_scope: ProfileScope::Global,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![
            SecretFieldSpec {
                field_path: "provider_api_base".to_string(),
                is_secret: false,
                may_contain_secrets: true,
            },
            SecretFieldSpec {
                field_path: "provider_project_id".to_string(),
                is_secret: false,
                may_contain_secrets: true,
            },
        ],
        remote_disposition: RemoteDisposition::LocalOnly,
        local_only_reason: Some(LocalOnlyReason::FilesystemAccessRequired),
        documentation: Some(
            "Create or print a starter GAH config/profile - requires filesystem access".to_string(),
        ),
        cli_command_path: "gah init".to_string(),
        is_stable: true,
    });
}

pub(super) fn add_prune_operations(manifest: &mut CapabilityManifest) {
    manifest.add_operation(OperationDefinition {
        operation_id: "prune.sessions".to_string(),
        display_name: "Prune Old Sessions".to_string(),
        class: OperationClass::Mutation,
        profile_scope: ProfileScope::ProfileOptional,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::NonIdempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::LocalOnly,
        local_only_reason: Some(LocalOnlyReason::FilesystemAccessRequired),
        documentation: Some(
            "Delete old GAH-owned sessions and worktrees - requires local filesystem access"
                .to_string(),
        ),
        cli_command_path: "gah prune".to_string(),
        is_stable: true,
    });
}

pub(super) fn add_ledger_operations(manifest: &mut CapabilityManifest) {
    // gah ledger
    manifest.add_operation(OperationDefinition {
        operation_id: "ledger.summary".to_string(),
        display_name: "Ledger Summary".to_string(),
        class: OperationClass::Read,
        profile_scope: ProfileScope::ProfileOptional,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some("Summarize recent ledger entries".to_string()),
        cli_command_path: "gah ledger summary".to_string(),
        is_stable: true,
    });

    // gah ledger work
    manifest.add_operation(OperationDefinition {
        operation_id: "ledger.work".to_string(),
        display_name: "Ledger Work History".to_string(),
        class: OperationClass::Read,
        profile_scope: ProfileScope::ProfileOptional,
        request_schema: None,
        response_schema: Some(SchemaReference {
            rust_type: "crate::ledger::LedgerEntry".to_string(),
            ts_type: Some("LedgerEntry[]".to_string()),
            is_primitive: false,
        }),
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some("Get full ledger history for one work item".to_string()),
        cli_command_path: "gah ledger work".to_string(),
        is_stable: true,
    });

    // gah ledger repair-tail
    manifest.add_operation(OperationDefinition {
        operation_id: "ledger.repair_tail".to_string(),
        display_name: "Repair Ledger Tail".to_string(),
        class: OperationClass::Mutation,
        profile_scope: ProfileScope::ProfileOptional,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some(
            "Back up and remove one torn, unterminated final JSONL record".to_string(),
        ),
        cli_command_path: "gah ledger repair-tail".to_string(),
        is_stable: true,
    });

    // gah ledger reconcile
    manifest.add_operation(OperationDefinition {
        operation_id: "ledger.reconcile".to_string(),
        display_name: "Reconcile Ledger".to_string(),
        class: OperationClass::Mutation,
        profile_scope: ProfileScope::ProfileRequired,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some("Backfill dispatched work with later provider outcomes".to_string()),
        cli_command_path: "gah ledger reconcile".to_string(),
        is_stable: true,
    });

    // gah ledger clear-attempts
    manifest.add_operation(OperationDefinition {
        operation_id: "ledger.clear_attempts".to_string(),
        display_name: "Clear Ledger Attempts".to_string(),
        class: OperationClass::Mutation,
        profile_scope: ProfileScope::ProfileRequired,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::NonIdempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some("Mark all prior attempts for a work_id as stale".to_string()),
        cli_command_path: "gah ledger clear-attempts".to_string(),
        is_stable: true,
    });
}

pub(super) fn add_hold_operations(manifest: &mut CapabilityManifest) {
    // gah hold set
    manifest.add_operation(OperationDefinition {
        operation_id: "hold.set".to_string(),
        display_name: "Set Review Hold".to_string(),
        class: OperationClass::Mutation,
        profile_scope: ProfileScope::ProfileRequired,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some(
            "Mark a work_id as under active out-of-band manager review".to_string(),
        ),
        cli_command_path: "gah hold set".to_string(),
        is_stable: true,
    });

    // gah hold clear
    manifest.add_operation(OperationDefinition {
        operation_id: "hold.clear".to_string(),
        display_name: "Clear Review Hold".to_string(),
        class: OperationClass::Mutation,
        profile_scope: ProfileScope::ProfileRequired,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some("Release a previously set review hold on a work_id".to_string()),
        cli_command_path: "gah hold clear".to_string(),
        is_stable: true,
    });
}

pub(super) fn add_route_approval_operations(manifest: &mut CapabilityManifest) {
    // gah route-approval grant
    manifest.add_operation(OperationDefinition {
        operation_id: "route_approval.grant".to_string(),
        display_name: "Grant Route Approval".to_string(),
        class: OperationClass::Mutation,
        profile_scope: ProfileScope::ProfileRequired,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some(
            "Allow one exact paid backend/model route for this work item".to_string(),
        ),
        cli_command_path: "gah route-approval grant".to_string(),
        is_stable: true,
    });

    // gah route-approval revoke
    manifest.add_operation(OperationDefinition {
        operation_id: "route_approval.revoke".to_string(),
        display_name: "Revoke Route Approval".to_string(),
        class: OperationClass::Mutation,
        profile_scope: ProfileScope::ProfileRequired,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some("Remove a previously granted paid-route approval".to_string()),
        cli_command_path: "gah route-approval revoke".to_string(),
        is_stable: true,
    });
}

pub(super) fn add_external_approval_operations(manifest: &mut CapabilityManifest) {
    // gah external-approval request
    manifest.add_operation(OperationDefinition {
        operation_id: "external_approval.request".to_string(),
        display_name: "Request External Approval".to_string(),
        class: OperationClass::Mutation,
        profile_scope: ProfileScope::ProfileRequired,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![
            SecretFieldSpec {
                field_path: "credential_label".to_string(),
                is_secret: false,
                may_contain_secrets: true,
            },
            SecretFieldSpec {
                field_path: "max_dollars".to_string(),
                is_secret: false,
                may_contain_secrets: false,
            },
        ],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some("Record a requested external operation approval for a specific profile/repo/work item and credential label".to_string()),
        cli_command_path: "gah external-approval request".to_string(),
        is_stable: true,
    });

    // gah external-approval inspect
    manifest.add_operation(OperationDefinition {
        operation_id: "external_approval.inspect".to_string(),
        display_name: "Inspect External Approval".to_string(),
        class: OperationClass::Read,
        profile_scope: ProfileScope::ProfileRequired,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some(
            "Inspect the current external approval state for one exact scope".to_string(),
        ),
        cli_command_path: "gah external-approval inspect".to_string(),
        is_stable: true,
    });

    // gah external-approval grant
    manifest.add_operation(OperationDefinition {
        operation_id: "external_approval.grant".to_string(),
        display_name: "Grant External Approval".to_string(),
        class: OperationClass::Mutation,
        profile_scope: ProfileScope::ProfileRequired,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some("Grant the requested external approval scope".to_string()),
        cli_command_path: "gah external-approval grant".to_string(),
        is_stable: true,
    });

    // gah external-approval revoke
    manifest.add_operation(OperationDefinition {
        operation_id: "external_approval.revoke".to_string(),
        display_name: "Revoke External Approval".to_string(),
        class: OperationClass::Mutation,
        profile_scope: ProfileScope::ProfileRequired,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some("Revoke an active external approval".to_string()),
        cli_command_path: "gah external-approval revoke".to_string(),
        is_stable: true,
    });

    // gah external-approval expire
    manifest.add_operation(OperationDefinition {
        operation_id: "external_approval.expire".to_string(),
        display_name: "Expire External Approval".to_string(),
        class: OperationClass::Mutation,
        profile_scope: ProfileScope::ProfileRequired,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some("Expire an external approval immediately".to_string()),
        cli_command_path: "gah external-approval expire".to_string(),
        is_stable: true,
    });
}

pub(super) fn add_loop_operations(manifest: &mut CapabilityManifest) {
    manifest.add_operation(OperationDefinition {
        operation_id: "loop.run".to_string(),
        display_name: "Run Controller Loop".to_string(),
        class: OperationClass::Mutation,
        profile_scope: ProfileScope::ProfileRequired,
        request_schema: None,
        response_schema: Some(SchemaReference {
            rust_type: "crate::controller::ControllerActivity".to_string(),
            ts_type: Some("ControllerActivity".to_string()),
            is_primitive: false,
        }),
        streaming: StreamingBehavior::Sse,
        idempotency: Idempotency::NonIdempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::LocalOnly,
        local_only_reason: Some(LocalOnlyReason::LocalBackendExecutionRequired),
        documentation: Some(
            "Run the controller continuously - requires local backend execution".to_string(),
        ),
        cli_command_path: "gah loop".to_string(),
        is_stable: true,
    });
}

pub(super) fn add_events_operations(manifest: &mut CapabilityManifest) {
    manifest.add_operation(OperationDefinition {
        operation_id: "events.list".to_string(),
        display_name: "List Controller Events".to_string(),
        class: OperationClass::Read,
        profile_scope: ProfileScope::ProfileOptional,
        request_schema: None,
        response_schema: Some(SchemaReference {
            rust_type: "crate::events::ControllerEvent".to_string(),
            ts_type: Some("ControllerEvent[]".to_string()),
            is_primitive: false,
        }),
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some("Inspect the controller event stream".to_string()),
        cli_command_path: "gah events".to_string(),
        is_stable: true,
    });
}

pub(super) fn add_status_operations(manifest: &mut CapabilityManifest) {
    manifest.add_operation(OperationDefinition {
        operation_id: "status.get".to_string(),
        display_name: "Get Controller Status".to_string(),
        class: OperationClass::Read,
        profile_scope: ProfileScope::ProfileRequired,
        request_schema: None,
        response_schema: Some(SchemaReference {
            rust_type: "crate::status::StatusSnapshot".to_string(),
            ts_type: Some("StatusSnapshot".to_string()),
            is_primitive: false,
        }),
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some(
            "Provide a single machine-readable controller snapshot of all state".to_string(),
        ),
        cli_command_path: "gah status".to_string(),
        is_stable: true,
    });
}

pub(super) fn add_sync_operations(manifest: &mut CapabilityManifest) {
    manifest.add_operation(OperationDefinition {
        operation_id: "sync.classify".to_string(),
        display_name: "Classify Merge Requests".to_string(),
        class: OperationClass::Read,
        profile_scope: ProfileScope::ProfileRequired,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some("Classify open GAH-created merge requests / pull requests".to_string()),
        cli_command_path: "gah sync".to_string(),
        is_stable: true,
    });
}

pub(super) fn add_dispatch_operations(manifest: &mut CapabilityManifest) {
    manifest.add_operation(OperationDefinition {
        operation_id: "dispatch.run".to_string(),
        display_name: "Dispatch Job".to_string(),
        class: OperationClass::Mutation,
        profile_scope: ProfileScope::ProfileRequired,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::NonIdempotent,
        secret_fields: vec![
            SecretFieldSpec {
                field_path: "model".to_string(),
                is_secret: false,
                may_contain_secrets: true,
            },
            SecretFieldSpec {
                field_path: "oh_profile".to_string(),
                is_secret: false,
                may_contain_secrets: false,
            },
        ],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some(
            "Dispatch a job to a backend (improve, pm, review, fix, experiment)".to_string(),
        ),
        cli_command_path: "gah dispatch".to_string(),
        is_stable: true,
    });
}

pub(super) fn add_pm_operations(manifest: &mut CapabilityManifest) {
    // gah pm publish
    manifest.add_operation(OperationDefinition {
        operation_id: "pm.publish".to_string(),
        display_name: "Publish PM Plan".to_string(),
        class: OperationClass::Mutation,
        profile_scope: ProfileScope::ProfileRequired,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::NonIdempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some(
            "Publish a validated PM plan artifact as native provider issues".to_string(),
        ),
        cli_command_path: "gah pm publish".to_string(),
        is_stable: true,
    });
}

pub(super) fn add_tui_operations(manifest: &mut CapabilityManifest) {
    manifest.add_operation(OperationDefinition {
        operation_id: "tui.run".to_string(),
        display_name: "Run TUI".to_string(),
        class: OperationClass::Read,
        profile_scope: ProfileScope::ProfileOptional,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::LocalOnly,
        local_only_reason: Some(LocalOnlyReason::InteractiveTerminalRequired),
        documentation: Some("Interactive terminal UI - requires interactive terminal".to_string()),
        cli_command_path: "gah tui".to_string(),
        is_stable: true,
    });
}

pub(super) fn add_config_operations(manifest: &mut CapabilityManifest) {
    // gah config show
    manifest.add_operation(OperationDefinition {
        operation_id: "config.show".to_string(),
        display_name: "Show Configuration".to_string(),
        class: OperationClass::Read,
        profile_scope: ProfileScope::Global,
        request_schema: None,
        response_schema: Some(SchemaReference {
            rust_type: "crate::config_show::ConfigShowFull".to_string(),
            ts_type: Some("ConfigShowFull".to_string()),
            is_primitive: false,
        }),
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some("Show global defaults (e.g. current_manager)".to_string()),
        cli_command_path: "gah config show".to_string(),
        is_stable: true,
    });

    // gah config set
    manifest.add_operation(OperationDefinition {
        operation_id: "config.set".to_string(),
        display_name: "Set Configuration".to_string(),
        class: OperationClass::Mutation,
        profile_scope: ProfileScope::Global,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some("Set one or more global default values".to_string()),
        cli_command_path: "gah config set".to_string(),
        is_stable: true,
    });
}

pub(super) fn add_profile_operations(manifest: &mut CapabilityManifest) {
    // gah profile list
    manifest.add_operation(OperationDefinition {
        operation_id: "profile.list".to_string(),
        display_name: "List Profiles".to_string(),
        class: OperationClass::Read,
        profile_scope: ProfileScope::Global,
        request_schema: None,
        response_schema: Some(SchemaReference {
            rust_type: "crate::config::ProfileSummary".to_string(),
            ts_type: Some("ProfileSummary[]".to_string()),
            is_primitive: false,
        }),
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some("List all profiles in config".to_string()),
        cli_command_path: "gah profile list".to_string(),
        is_stable: true,
    });

    // gah profile show
    manifest.add_operation(OperationDefinition {
        operation_id: "profile.show".to_string(),
        display_name: "Show Profile".to_string(),
        class: OperationClass::Read,
        profile_scope: ProfileScope::Global,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some("Show details for a profile".to_string()),
        cli_command_path: "gah profile show".to_string(),
        is_stable: true,
    });

    // gah profile add
    manifest.add_operation(OperationDefinition {
        operation_id: "profile.add".to_string(),
        display_name: "Add Profile".to_string(),
        class: OperationClass::Mutation,
        profile_scope: ProfileScope::Global,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![
            SecretFieldSpec {
                field_path: "provider_api_base".to_string(),
                is_secret: false,
                may_contain_secrets: true,
            },
            SecretFieldSpec {
                field_path: "provider_project_id".to_string(),
                is_secret: false,
                may_contain_secrets: true,
            },
        ],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some("Add a new profile".to_string()),
        cli_command_path: "gah profile add".to_string(),
        is_stable: true,
    });

    // gah profile set
    manifest.add_operation(OperationDefinition {
        operation_id: "profile.set".to_string(),
        display_name: "Set Profile".to_string(),
        class: OperationClass::Mutation,
        profile_scope: ProfileScope::Global,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some("Set/Update fields of an existing profile".to_string()),
        cli_command_path: "gah profile set".to_string(),
        is_stable: true,
    });

    // gah profile remove
    manifest.add_operation(OperationDefinition {
        operation_id: "profile.remove".to_string(),
        display_name: "Remove Profile".to_string(),
        class: OperationClass::Mutation,
        profile_scope: ProfileScope::Global,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::NonIdempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some("Remove a profile".to_string()),
        cli_command_path: "gah profile remove".to_string(),
        is_stable: true,
    });
}

pub(super) fn add_report_operations(manifest: &mut CapabilityManifest) {
    manifest.add_operation(OperationDefinition {
        operation_id: "report.generate".to_string(),
        display_name: "Generate Report".to_string(),
        class: OperationClass::Read,
        profile_scope: ProfileScope::ProfileOptional,
        request_schema: None,
        response_schema: Some(SchemaReference {
            rust_type: "crate::report::ReportData".to_string(),
            ts_type: Some("ReportData".to_string()),
            is_primitive: false,
        }),
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some("Generate backend/model comparison report".to_string()),
        cli_command_path: "gah report".to_string(),
        is_stable: true,
    });
}

pub(super) fn add_server_operations(manifest: &mut CapabilityManifest) {
    manifest.add_operation(OperationDefinition {
        operation_id: "server.start".to_string(),
        display_name: "Start WebSocket Server".to_string(),
        class: OperationClass::Mutation,
        profile_scope: ProfileScope::Global,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::WebSocket,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::LocalOnly,
        local_only_reason: Some(LocalOnlyReason::LocalBackendExecutionRequired),
        documentation: Some(
            "Start the WebSocket server for desktop/web interface - requires local execution"
                .to_string(),
        ),
        cli_command_path: "gah server".to_string(),
        is_stable: true,
    });
}

pub(super) fn add_telemetry_operations(manifest: &mut CapabilityManifest) {
    // gah telemetry export
    manifest.add_operation(OperationDefinition {
        operation_id: "telemetry.export".to_string(),
        display_name: "Export Telemetry".to_string(),
        class: OperationClass::Mutation,
        profile_scope: ProfileScope::ProfileOptional,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::LocalOnly,
        local_only_reason: Some(LocalOnlyReason::FilesystemAccessRequired),
        documentation: Some(
            "Export telemetry data to versioned repository - requires filesystem access"
                .to_string(),
        ),
        cli_command_path: "gah telemetry export".to_string(),
        is_stable: true,
    });

    // gah telemetry status
    manifest.add_operation(OperationDefinition {
        operation_id: "telemetry.status".to_string(),
        display_name: "Telemetry Repository Status".to_string(),
        class: OperationClass::Read,
        profile_scope: ProfileScope::ProfileOptional,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some("Show telemetry repository status".to_string()),
        cli_command_path: "gah telemetry status".to_string(),
        is_stable: true,
    });

    // gah telemetry aggregate
    manifest.add_operation(OperationDefinition {
        operation_id: "telemetry.aggregate".to_string(),
        display_name: "Aggregate Telemetry".to_string(),
        class: OperationClass::Read,
        profile_scope: ProfileScope::ProfileOptional,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some(
            "Generate aggregated telemetry reports by routing dimensions".to_string(),
        ),
        cli_command_path: "gah telemetry aggregate".to_string(),
        is_stable: true,
    });
}

pub(super) fn add_quota_operations(manifest: &mut CapabilityManifest) {
    // gah quota refresh
    manifest.add_operation(OperationDefinition {
        operation_id: "quota.refresh".to_string(),
        display_name: "Refresh Quota".to_string(),
        class: OperationClass::Mutation,
        profile_scope: ProfileScope::Global,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![SecretFieldSpec {
            field_path: "backend_instance".to_string(),
            is_secret: false,
            may_contain_secrets: true,
        }],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some("Refresh account-level quota and persist the observation".to_string()),
        cli_command_path: "gah quota refresh".to_string(),
        is_stable: true,
    });

    // gah quota list
    manifest.add_operation(OperationDefinition {
        operation_id: "quota.list".to_string(),
        display_name: "List Quota Observations".to_string(),
        class: OperationClass::Read,
        profile_scope: ProfileScope::Global,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some("List persisted account-level quota observations".to_string()),
        cli_command_path: "gah quota list".to_string(),
        is_stable: true,
    });

    // gah quota snapshot
    manifest.add_operation(OperationDefinition {
        operation_id: "quota.snapshot".to_string(),
        display_name: "Quota Snapshot".to_string(),
        class: OperationClass::Read,
        profile_scope: ProfileScope::ProfileRequired,
        request_schema: None,
        response_schema: Some(SchemaReference {
            rust_type: "crate::quota_snapshot::QuotaSnapshot".to_string(),
            ts_type: Some("QuotaSnapshot".to_string()),
            is_primitive: false,
        }),
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some("Build the canonical profile-scoped quota snapshot".to_string()),
        cli_command_path: "gah quota snapshot".to_string(),
        is_stable: true,
    });
}

pub(super) fn add_claims_operations(manifest: &mut CapabilityManifest) {
    // gah claims list
    manifest.add_operation(OperationDefinition {
        operation_id: "claims.list".to_string(),
        display_name: "List Work Claims".to_string(),
        class: OperationClass::Read,
        profile_scope: ProfileScope::ProfileOptional,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some("List work claims".to_string()),
        cli_command_path: "gah claims list".to_string(),
        is_stable: true,
    });

    // gah claims clear
    manifest.add_operation(OperationDefinition {
        operation_id: "claims.clear".to_string(),
        display_name: "Clear Work Claim".to_string(),
        class: OperationClass::Mutation,
        profile_scope: ProfileScope::ProfileRequired,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::Idempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some("Clear a work claim".to_string()),
        cli_command_path: "gah claims clear".to_string(),
        is_stable: true,
    });

    // gah claims reclaim
    manifest.add_operation(OperationDefinition {
        operation_id: "claims.reclaim".to_string(),
        display_name: "Reclaim Stale Claims".to_string(),
        class: OperationClass::Mutation,
        profile_scope: ProfileScope::ProfileRequired,
        request_schema: None,
        response_schema: None,
        streaming: StreamingBehavior::None,
        idempotency: Idempotency::NonIdempotent,
        secret_fields: vec![],
        remote_disposition: RemoteDisposition::RemoteAvailable,
        local_only_reason: None,
        documentation: Some("Reclaim stale work claims".to_string()),
        cli_command_path: "gah claims reclaim".to_string(),
        is_stable: true,
    });
}
