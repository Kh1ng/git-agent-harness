//! CLI Capability Drift Detection (Issue #531)
//!
//! This module provides validation that ensures the capability manifest
//! is complete and internally consistent.

use super::*;

/// Validate that the manifest is complete and accurate
pub fn validate_manifest_completeness() -> Vec<String> {
    let manifest = generate_manifest();
    let mut errors = Vec::new();

    // Check that all operations have the required metadata
    for (op_id, op) in &manifest.operations {
        // Required fields
        if op.operation_id.is_empty() {
            errors.push(format!("Operation '{}' has empty operation_id", op_id));
        }
        if op.cli_command_path.is_empty() {
            errors.push(format!("Operation '{}' has empty cli_command_path", op_id));
        }
        if op.display_name.is_empty() {
            errors.push(format!("Operation '{}' has empty display_name", op_id));
        }

        // Local-only operations must have reasons
        if op.remote_disposition == RemoteDisposition::LocalOnly && op.local_only_reason.is_none() {
            errors.push(format!("Local-only operation '{}' has no reason", op_id));
        }

        // Check that command path maps to this operation
        if let Some(mapped_op_id) = manifest
            .command_path_to_operation_id
            .get(&op.cli_command_path)
        {
            if mapped_op_id != op_id {
                errors.push(format!(
                    "Operation '{}' has command path '{}' but maps to operation '{}'",
                    op_id, op.cli_command_path, mapped_op_id
                ));
            }
        } else {
            errors.push(format!(
                "Operation '{}' has command path '{}' but path not found in command map",
                op_id, op.cli_command_path
            ));
        }
    }

    // Check remote operations list consistency
    for op_id in &manifest.remote_operations {
        if let Some(op) = manifest.operations.get(op_id) {
            if op.remote_disposition != RemoteDisposition::RemoteAvailable {
                errors.push(format!(
                    "Operation '{}' in remote_operations but disposition is {:?}",
                    op_id, op.remote_disposition
                ));
            }
        } else {
            errors.push(format!(
                "Remote operation '{}' not found in operations",
                op_id
            ));
        }
    }

    // Check local-only operations list consistency
    for (op_id, reason) in &manifest.local_only_operations {
        if let Some(op) = manifest.operations.get(op_id) {
            if op.remote_disposition != RemoteDisposition::LocalOnly {
                errors.push(format!(
                    "Operation '{}' in local_only_operations but disposition is {:?}",
                    op_id, op.remote_disposition
                ));
            }
            if op.local_only_reason != Some(reason.clone()) {
                errors.push(format!(
                    "Operation '{}' local_only_reason mismatch: manifest has {:?}, op has {:?}",
                    op_id, reason, op.local_only_reason
                ));
            }
        } else {
            errors.push(format!(
                "Local-only operation '{}' not found in operations",
                op_id
            ));
        }
    }

    errors
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_manifest_completeness() {
        let errors = validate_manifest_completeness();
        assert!(
            errors.is_empty(),
            "Manifest completeness errors: {:?}",
            errors
        );
    }

    #[test]
    fn test_drift_detection_basic() {
        let manifest = generate_manifest();

        // Verify basic structure
        assert!(
            !manifest.operations.is_empty(),
            "Manifest should have operations"
        );
        assert!(
            !manifest.command_path_to_operation_id.is_empty(),
            "Manifest should have command mappings"
        );

        // Verify we have both remote and local operations
        assert!(
            !manifest.remote_operations.is_empty(),
            "Should have remote operations"
        );
        assert!(
            !manifest.local_only_operations.is_empty(),
            "Should have local-only operations"
        );

        // Verify all manifest commands start with "gah "
        let manifest_only_commands: Vec<&String> = manifest
            .command_path_to_operation_id
            .keys()
            .filter(|&cmd| !cmd.starts_with("gah "))
            .collect();

        assert!(
            manifest_only_commands.is_empty(),
            "Found manifest commands not starting with 'gah ': {:?}",
            manifest_only_commands
        );
    }

    #[test]
    fn test_ci_validation_fails_on_missing_disposition() {
        let manifest = generate_manifest();

        // Verify that all local-only operations have reasons (CI requirement)
        for (op_id, op) in &manifest.operations {
            if op.remote_disposition == RemoteDisposition::LocalOnly {
                assert!(
                    op.local_only_reason.is_some(),
                    "CI FAIL: Local-only operation {} has no reason",
                    op_id
                );
            }
        }
    }
}
