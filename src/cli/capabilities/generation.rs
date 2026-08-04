//! CLI Capability Manifest Generation (Issue #531)
//!
//! This module provides build-time generation of TypeScript and JSON Schema
//! artifacts from the Rust-owned capability manifest.

use std::fs::File;
use std::io::Write;
use std::path::Path;

use super::*;

/// Generate TypeScript type definitions and write them to a file
pub fn generate_typescript_file(output_path: &str) -> anyhow::Result<()> {
    let ts_types = generate_enhanced_typescript_types();

    let path = Path::new(output_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut file = File::create(path)?;
    file.write_all(ts_types.as_bytes())?;

    Ok(())
}

/// Generate JSON manifest and write it to a file
pub fn generate_json_manifest_file(output_path: &str) -> anyhow::Result<()> {
    let manifest = generate_manifest();
    let json = serde_json::to_string_pretty(&manifest)?;

    let path = Path::new(output_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut file = File::create(path)?;
    file.write_all(json.as_bytes())?;

    Ok(())
}

/// Generate JSON Schema for the manifest
pub fn generate_json_schema() -> serde_json::Value {
    use serde_json::json;

    json!({
        "$schema": "http://json-schema.org/draft-07/schema#",
        "$id": "https://github.com/Kh1ng/git-agent-harness/schemas/cli-capabilities/v1.json",
        "title": "CLI Capability Manifest Schema",
        "description": "Schema for GAH CLI capability and remote-disposition manifest (Issue #531)",
        "version": "1.0.0",
        "type": "object",
        "required": ["schema_version", "manifest_version", "operations"],
        "properties": {
            "schema_version": {
                "type": "integer",
                "description": "Schema version of this manifest",
                "minimum": 1
            },
            "manifest_version": {
                "type": "string",
                "description": "Manifest version identifier"
            },
            "operations": {
                "type": "object",
                "description": "Map of operation IDs to operation definitions",
                "additionalProperties": {
                    "$ref": "#/definitions/OperationDefinition"
                }
            },
            "command_path_to_operation_id": {
                "type": "object",
                "description": "Map from CLI command paths to operation IDs",
                "additionalProperties": {
                    "type": "string"
                }
            },
            "remote_operations": {
                "type": "array",
                "description": "List of remotely available operation IDs",
                "items": {
                    "type": "string"
                }
            },
            "local_only_operations": {
                "type": "object",
                "description": "Map of local-only operation IDs to their reasons",
                "additionalProperties": {
                    "$ref": "#/definitions/LocalOnlyReason"
                }
            }
        },
        "definitions": {
            "OperationDefinition": {
                "type": "object",
                "required": [
                    "operation_id", "display_name", "class", "profile_scope",
                    "streaming", "idempotency", "remote_disposition",
                    "cli_command_path", "is_stable"
                ],
                "properties": {
                    "operation_id": {
                        "type": "string",
                        "description": "Stable operation identifier"
                    },
                    "display_name": {
                        "type": "string",
                        "description": "Human-readable display name"
                    },
                    "class": {
                        "$ref": "#/definitions/OperationClass"
                    },
                    "profile_scope": {
                        "$ref": "#/definitions/ProfileScope"
                    },
                    "request_schema": {
                        "$ref": "#/definitions/SchemaReference",
                        "description": "Request schema reference"
                    },
                    "response_schema": {
                        "$ref": "#/definitions/SchemaReference",
                        "description": "Response schema reference"
                    },
                    "streaming": {
                        "$ref": "#/definitions/StreamingBehavior"
                    },
                    "idempotency": {
                        "$ref": "#/definitions/Idempotency"
                    },
                    "secret_fields": {
                        "type": "array",
                        "description": "Fields that contain secrets",
                        "items": {
                            "$ref": "#/definitions/SecretFieldSpec"
                        }
                    },
                    "remote_disposition": {
                        "$ref": "#/definitions/RemoteDisposition"
                    },
                    "local_only_reason": {
                        "$ref": "#/definitions/LocalOnlyReason",
                        "description": "Reason for local-only disposition"
                    },
                    "documentation": {
                        "type": ["string", "null"],
                        "description": "Additional documentation"
                    },
                    "cli_command_path": {
                        "type": "string",
                        "description": "CLI command path"
                    },
                    "is_stable": {
                        "type": "boolean",
                        "description": "Whether this operation is stable"
                    }
                }
            },
            "SchemaReference": {
                "type": "object",
                "properties": {
                    "rust_type": {
                        "type": "string",
                        "description": "Rust module path to the type"
                    },
                    "ts_type": {
                        "type": ["string", "null"],
                        "description": "TypeScript type name in contracts"
                    },
                    "is_primitive": {
                        "type": "boolean",
                        "description": "Whether this is a built-in primitive type"
                    }
                },
                "required": ["rust_type", "is_primitive"]
            },
            "SecretFieldSpec": {
                "type": "object",
                "properties": {
                    "field_path": {
                        "type": "string",
                        "description": "Field name path"
                    },
                    "is_secret": {
                        "type": "boolean",
                        "description": "Whether this field is always considered secret"
                    },
                    "may_contain_secrets": {
                        "type": "boolean",
                        "description": "Whether this field may contain secrets depending on context"
                    }
                },
                "required": ["field_path", "is_secret", "may_contain_secrets"]
            },
            "OperationClass": {
                "type": "string",
                "enum": ["read", "mutation", "admin"]
            },
            "ProfileScope": {
                "type": "string",
                "enum": ["global", "profile_required", "profile_optional"]
            },
            "StreamingBehavior": {
                "type": "string",
                "enum": ["none", "sse", "web_socket", "jsonl"]
            },
            "Idempotency": {
                "type": "string",
                "enum": ["idempotent", "non_idempotent", "conditional"]
            },
            "RemoteDisposition": {
                "type": "string",
                "enum": ["remote_available", "local_only", "not_implemented"]
            },
            "LocalOnlyReason": {
                "type": "string",
                "enum": [
                    "filesystem_access_required",
                    "interactive_terminal_required",
                    "security_sensitive",
                    "development_only",
                    "local_backend_execution_required",
                    "legacy",
                    "other"
                ]
            }
        }
    })
}

/// Generate JSON Schema and write it to a file
pub fn generate_json_schema_file(output_path: &str) -> anyhow::Result<()> {
    let schema = generate_json_schema();
    let json = serde_json::to_string_pretty(&schema)?;

    let path = Path::new(output_path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let mut file = File::create(path)?;
    file.write_all(json.as_bytes())?;

    Ok(())
}

/// Generate enhanced TypeScript types with actual manifest data
pub fn generate_enhanced_typescript_types() -> String {
    let manifest = generate_manifest();
    let json_data = serde_json::to_string_pretty(&manifest).expect("Failed to serialize manifest");

    let template = r#"// Generated CLI Capability Manifest Types (Issue #531)
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
export const CLI_CAPABILITIES_MANIFEST: CapabilityManifest = {};

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
"#;

    template.replacen(
        "export const CLI_CAPABILITIES_MANIFEST: CapabilityManifest = {};",
        &format!("export const CLI_CAPABILITIES_MANIFEST: CapabilityManifest = {json_data};"),
        1,
    )
}
