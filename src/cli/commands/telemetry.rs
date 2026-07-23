// Command execution for `gah telemetry` (ticket #409).

use anyhow::{anyhow, Result};

use crate::cli::args::TelemetryCommands;

pub fn run(command: TelemetryCommands) -> Result<()> {
    match command {
        TelemetryCommands::Export {
            telemetry_repo_path,
            format,
            output,
            since,
            profile,
            group_by,
            generate_manifests,
            config_path,
        } => {
            let format_enum = format
                .parse::<crate::telemetry::exporter::ExportFormat>()
                .map_err(|e| anyhow!("Invalid format: {}", e))?;
            crate::telemetry::cli::run_export(
                telemetry_repo_path.as_deref(),
                Some(format_enum),
                output.as_deref(),
                Some(&since),
                profile.as_deref(),
                Some(group_by),
                generate_manifests,
                config_path.as_deref(),
            )?;
        }
        TelemetryCommands::Status {
            telemetry_repo_path,
            config_path,
        } => {
            crate::telemetry::cli::run_status(
                telemetry_repo_path.as_deref(),
                config_path.as_deref(),
            )?;
        }
        TelemetryCommands::Aggregate {
            dimensions,
            since,
            until,
            profile,
            include_failed,
            include_retried,
            json,
            config_path,
            project,
            ticket,
            execution_type,
            backend_instance,
            provider,
            model,
            account,
        } => {
            let parsed_dimensions = dimensions
                .iter()
                .map(|dim| dim.parse::<crate::telemetry::AggregationDimension>())
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| anyhow!("Invalid dimension: {}", e))?;

            crate::telemetry::cli::run_aggregate(
                parsed_dimensions,
                since.as_deref(),
                until.as_deref(),
                profile.as_deref(),
                include_failed,
                include_retried,
                json,
                config_path.as_deref(),
                project.as_deref(),
                ticket.as_deref(),
                execution_type.as_deref(),
                backend_instance.as_deref(),
                provider.as_deref(),
                model.as_deref(),
                account.as_deref(),
            )?;
        }
    }
    Ok(())
}
