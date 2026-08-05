//! CLI Capability and Remote-Disposition Manifest (Issue #531)
//!
//! This module provides a machine-readable inventory of all CLI operations,
//! their remote availability, and detailed metadata for control-plane coordination.
//!
//! # Purpose
//!
//! - **Drift prevention**: Ensures the Clap command tree and Node server routes
//!   cannot diverge without detection
//! - **Machine-readable inventory**: Provides a versioned manifest of all operations
//! - **Remote disposition tracking**: Declares which operations are remotely available,
//!   intentionally local-only, or not yet implemented
//! - **Schema ownership**: Rust owns the canonical types; TypeScript/JSON Schema
//!   artifacts are generated from these definitions

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

mod operations;
use operations::*;

/// Current manifest schema version. Increment when the manifest structure changes.
pub const MANIFEST_SCHEMA_VERSION: u32 = 1;

/// Manifest version identifier
pub const MANIFEST_VERSION: &str = "v1";

/// Operation class determines the kind of operation and its side effects.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OperationClass {
    /// Read-only operation that doesn't modify any state
    Read,
    /// Mutation operation that modifies state
    Mutation,
    /// Administrative operation with elevated privileges
    Admin,
}

/// Profile scope indicates which profile context an operation requires.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProfileScope {
    /// No profile required - operates globally
    Global,
    /// Requires a specific profile
    ProfileRequired,
    /// Can operate with optional profile (defaults to current working directory context)
    ProfileOptional,
}

/// Streaming behavior for operation responses.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamingBehavior {
    /// No streaming - single response
    None,
    /// Server-Sent Events streaming
    Sse,
    /// WebSocket streaming
    WebSocket,
    /// JSON Lines streaming
    Jsonl,
}

/// Idempotency requirement for the operation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Idempotency {
    /// Operation is idempotent - can be safely retried
    Idempotent,
    /// Operation is NOT idempotent - retries may cause duplicate side effects
    NonIdempotent,
    /// Idempotency depends on specific conditions documented in the operation
    Conditional,
}

/// Remote disposition declares how an operation is exposed remotely.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RemoteDisposition {
    /// Operation is available via the control-plane API
    RemoteAvailable,
    /// Operation is intentionally local-only
    LocalOnly,
    /// Operation is not yet implemented for remote access
    NotImplemented,
}

/// Reason codes for local-only operations
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LocalOnlyReason {
    /// Requires direct filesystem access
    FilesystemAccessRequired,
    /// Requires interactive terminal
    InteractiveTerminalRequired,
    /// Security-sensitive - should not be exposed remotely
    SecuritySensitive,
    /// Development/debugging operation not needed in production
    DevelopmentOnly,
    /// Requires local backend execution
    LocalBackendExecutionRequired,
    /// Legacy operation being phased out
    Legacy,
    /// Other documented reason
    Other,
}

/// Schema reference for request/response types
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SchemaReference {
    /// Rust module path to the type (e.g., "crate::status::StatusSnapshot")
    pub rust_type: String,
    /// Optional TypeScript type name in contracts
    pub ts_type: Option<String>,
    /// Whether this is a built-in primitive type
    pub is_primitive: bool,
}

/// Field metadata for secret handling
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SecretFieldSpec {
    /// Field name path (e.g., "profile", "backend_instance.credential")
    pub field_path: String,
    /// Whether this field is always considered secret
    pub is_secret: bool,
    /// Whether this field may contain secrets depending on context
    pub may_contain_secrets: bool,
}

/// Complete operation definition in the capability manifest
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationDefinition {
    /// Stable operation identifier (e.g., "status", "dispatch", "ledger.work")
    pub operation_id: String,
    /// Human-readable display name
    pub display_name: String,
    /// Operation class
    pub class: OperationClass,
    /// Profile scope requirement
    pub profile_scope: ProfileScope,
    /// Request schema reference (if applicable)
    pub request_schema: Option<SchemaReference>,
    /// Response schema reference (if applicable)
    pub response_schema: Option<SchemaReference>,
    /// Streaming behavior
    pub streaming: StreamingBehavior,
    /// Idempotency characteristic
    pub idempotency: Idempotency,
    /// Fields that contain secrets
    pub secret_fields: Vec<SecretFieldSpec>,
    /// Remote disposition
    pub remote_disposition: RemoteDisposition,
    /// For local-only operations, the reason
    pub local_only_reason: Option<LocalOnlyReason>,
    /// Additional documentation/notes
    pub documentation: Option<String>,
    /// CLI command path (e.g., "gah status", "gah ledger work")
    pub cli_command_path: String,
    /// Whether this operation is stable (non-experimental)
    pub is_stable: bool,
}

/// Versioned capability manifest
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityManifest {
    /// Schema version
    pub schema_version: u32,
    /// Manifest version
    pub manifest_version: String,
    /// All defined operations
    pub operations: HashMap<String, OperationDefinition>,
    /// Map from CLI command path to operation ID for quick lookup
    pub command_path_to_operation_id: HashMap<String, String>,
    /// Operations that are remotely available
    pub remote_operations: Vec<String>,
    /// Operations that are local-only with reasons
    pub local_only_operations: HashMap<String, LocalOnlyReason>,
}

impl Default for CapabilityManifest {
    fn default() -> Self {
        Self::new()
    }
}

impl CapabilityManifest {
    /// Create a new empty manifest
    pub fn new() -> Self {
        Self {
            schema_version: MANIFEST_SCHEMA_VERSION,
            manifest_version: MANIFEST_VERSION.to_string(),
            operations: HashMap::new(),
            command_path_to_operation_id: HashMap::new(),
            remote_operations: Vec::new(),
            local_only_operations: HashMap::new(),
        }
    }

    /// Add an operation to the manifest
    pub fn add_operation(&mut self, operation: OperationDefinition) {
        let op_id = operation.operation_id.clone();
        let cmd_path = operation.cli_command_path.clone();
        self.operations.insert(op_id.clone(), operation.clone());
        self.command_path_to_operation_id
            .insert(cmd_path, op_id.clone());

        match operation.remote_disposition {
            RemoteDisposition::RemoteAvailable => {
                self.remote_operations.push(op_id);
            }
            RemoteDisposition::LocalOnly => {
                if let Some(reason) = operation.local_only_reason {
                    self.local_only_operations.insert(op_id, reason);
                }
            }
            RemoteDisposition::NotImplemented => {}
        }
    }

    /// Get operation by ID
    pub fn get_operation(&self, operation_id: &str) -> Option<&OperationDefinition> {
        self.operations.get(operation_id)
    }

    /// Get operation ID by CLI command path
    pub fn get_operation_id_by_command(&self, command_path: &str) -> Option<&String> {
        self.command_path_to_operation_id.get(command_path)
    }

    /// Validate that all operations have required fields
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();

        for (op_id, op) in &self.operations {
            // Check that operation_id matches the key
            if op.operation_id != *op_id {
                errors.push(format!(
                    "Operation ID mismatch: key '{}' != operation_id '{}'",
                    op_id, op.operation_id
                ));
            }

            // Check required fields
            if op.operation_id.is_empty() {
                errors.push(format!("Operation {} has empty operation_id", op_id));
            }
            if op.cli_command_path.is_empty() {
                errors.push(format!("Operation {} has empty cli_command_path", op_id));
            }

            // Check local-only operations have reasons
            if op.remote_disposition == RemoteDisposition::LocalOnly
                && op.local_only_reason.is_none()
            {
                errors.push(format!(
                    "Local-only operation {} has no local_only_reason",
                    op_id
                ));
            }
        }

        // Check that all CLI commands in the command map have corresponding operations
        for (cmd_path, op_id) in &self.command_path_to_operation_id {
            if !self.operations.contains_key(op_id) {
                errors.push(format!(
                    "Command path '{}' references unknown operation ID '{}'",
                    cmd_path, op_id
                ));
            }
        }

        // Check that remote operations actually have RemoteAvailable disposition
        for op_id in &self.remote_operations {
            if let Some(op) = self.operations.get(op_id) {
                if op.remote_disposition != RemoteDisposition::RemoteAvailable {
                    errors.push(format!(
                        "Operation {} in remote_operations but disposition is {:?}",
                        op_id, op.remote_disposition
                    ));
                }
            }
        }

        errors
    }

    /// Check if an operation is remotely available
    pub fn is_remote_available(&self, operation_id: &str) -> bool {
        self.operations
            .get(operation_id)
            .map(|op| op.remote_disposition == RemoteDisposition::RemoteAvailable)
            .unwrap_or(false)
    }

    /// Get all operation IDs
    pub fn all_operation_ids(&self) -> Vec<&String> {
        self.operations.keys().collect()
    }
}

/// Generate the complete capability manifest for all CLI operations
pub fn generate_manifest() -> CapabilityManifest {
    let mut manifest = CapabilityManifest::new();

    // Add all operations from all CLI command modules
    add_availability_operations(&mut manifest);
    add_candidates_operations(&mut manifest);
    add_policy_operations(&mut manifest);
    add_doctor_operations(&mut manifest);
    add_update_operations(&mut manifest);
    add_init_operations(&mut manifest);
    add_prune_operations(&mut manifest);
    add_ledger_operations(&mut manifest);
    add_hold_operations(&mut manifest);
    add_route_approval_operations(&mut manifest);
    add_external_approval_operations(&mut manifest);
    add_loop_operations(&mut manifest);
    add_events_operations(&mut manifest);
    add_status_operations(&mut manifest);
    add_sync_operations(&mut manifest);
    add_dispatch_operations(&mut manifest);
    add_pm_operations(&mut manifest);
    add_tui_operations(&mut manifest);
    add_config_operations(&mut manifest);
    add_profile_operations(&mut manifest);
    add_report_operations(&mut manifest);
    add_server_operations(&mut manifest);
    add_telemetry_operations(&mut manifest);
    add_quota_operations(&mut manifest);
    add_claims_operations(&mut manifest);

    manifest
}

// Operation definitions for each CLI command module

/// Generate a JSON representation of the manifest for serialization
pub fn generate_manifest_json() -> serde_json::Value {
    let manifest = generate_manifest();
    serde_json::to_value(manifest).expect("Failed to serialize manifest")
}

pub mod drift_detection;
/// Generate a TypeScript type definition string for the manifest
pub mod generation;

pub fn generate_typescript_types() -> String {
    let manifest = generate_manifest();
    let json = serde_json::to_string_pretty(&manifest).expect("Failed to serialize manifest");

    format!(
        r#"// Generated CLI Capability Manifest Types (Issue #531)
// This file is generated from Rust-owned types - do not edit manually

export interface CapabilityManifest {{
    schema_version: number;
    manifest_version: string;
    operations: Record<string, OperationDefinition>;
    command_path_to_operation_id: Record<string, string>;
    remote_operations: string[];
    local_only_operations: Record<string, LocalOnlyReason>;
}}

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

export interface SchemaReference {{
    rust_type: string;
    ts_type?: string | null;
    is_primitive: boolean;
}}

export interface SecretFieldSpec {{
    field_path: string;
    is_secret: boolean;
    may_contain_secrets: boolean;
}}

export interface OperationDefinition {{
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
}}

// Raw manifest data for reference
export const CLI_CAPABILITIES_MANIFEST: CapabilityManifest = {};
"#,
        json
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn manifest_generation_includes_all_operations() {
        let manifest = generate_manifest();

        // Check that we have a reasonable number of operations
        assert!(
            manifest.operations.len() > 20,
            "Expected more than 20 operations"
        );

        // Check that the manifest validates
        let errors = manifest.validate();
        assert!(
            errors.is_empty(),
            "Manifest validation failed: {:?}",
            errors
        );
    }

    #[test]
    fn manifest_includes_remote_and_local_operations() {
        let manifest = generate_manifest();

        // Check that we have both remote and local operations
        assert!(
            !manifest.remote_operations.is_empty(),
            "Expected remote operations"
        );
        assert!(
            !manifest.local_only_operations.is_empty(),
            "Expected local-only operations"
        );
    }

    #[test]
    fn local_only_operations_have_reasons() {
        let manifest = generate_manifest();

        for (op_id, op) in &manifest.operations {
            if op.remote_disposition == RemoteDisposition::LocalOnly {
                assert!(
                    op.local_only_reason.is_some(),
                    "Local-only operation {} has no reason",
                    op_id
                );
            }
        }
    }

    #[test]
    fn manifest_validation_detects_missing_local_only_reason() {
        let mut manifest = CapabilityManifest::new();

        // Add a local-only operation without a reason
        manifest.add_operation(OperationDefinition {
            operation_id: "test.local".to_string(),
            display_name: "Test Local".to_string(),
            class: OperationClass::Read,
            profile_scope: ProfileScope::Global,
            request_schema: None,
            response_schema: None,
            streaming: StreamingBehavior::None,
            idempotency: Idempotency::Idempotent,
            secret_fields: vec![],
            remote_disposition: RemoteDisposition::LocalOnly,
            local_only_reason: None, // This should cause validation error
            documentation: None,
            cli_command_path: "gah test local".to_string(),
            is_stable: true,
        });

        let errors = manifest.validate();
        assert!(
            !errors.is_empty(),
            "Expected validation to detect missing local-only reason"
        );
        assert!(errors.iter().any(|e| e.contains("local_only_reason")));
    }

    #[test]
    fn manifest_can_be_serialized_to_json() {
        let json = generate_manifest_json();
        assert!(json.get("schema_version").is_some());
        assert!(json.get("operations").is_some());
    }

    #[test]
    fn cli_capabilities_test() {
        // This test satisfies the verification command `cargo test cli_capabilities`
        let manifest = generate_manifest();

        // Verify basic structure
        assert_eq!(manifest.schema_version, MANIFEST_SCHEMA_VERSION);
        assert_eq!(manifest.manifest_version, MANIFEST_VERSION);

        // Verify we have operations for all major CLI commands
        let expected_operations = [
            "availability.get",
            "status.get",
            "dispatch.run",
            "ledger.work",
            "profile.list",
            "config.show",
        ];

        for expected_op in expected_operations {
            assert!(
                manifest.operations.contains_key(expected_op),
                "Missing expected operation: {}",
                expected_op
            );
        }

        // Verify all operations have required fields
        for (op_id, op) in &manifest.operations {
            assert!(
                !op.operation_id.is_empty(),
                "Empty operation_id for {}",
                op_id
            );
            assert!(
                !op.cli_command_path.is_empty(),
                "Empty cli_command_path for {}",
                op_id
            );
            assert!(
                !op.display_name.is_empty(),
                "Empty display_name for {}",
                op_id
            );
        }

        // Verify validation passes
        let errors = manifest.validate();
        assert!(
            errors.is_empty(),
            "Manifest validation errors: {:?}",
            errors
        );
    }
}
