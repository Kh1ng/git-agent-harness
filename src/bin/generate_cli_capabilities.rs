//! CLI Capability Manifest Generation Binary (Issue #531)
//!
//! This binary generates TypeScript and JSON Schema artifacts from the
//! Rust-owned capability manifest.
//!
//! Usage: cargo run --bin generate-cli-capabilities [--output-dir DIR]

use anyhow::{Context, Result};
use clap::Parser;
use git_agent_harness::cli::capabilities::generation::*;
use std::path::{Path, PathBuf};

#[derive(Parser)]
#[command(
    name = "generate-cli-capabilities",
    about = "Generate CLI capability manifest artifacts"
)]
struct Args {
    /// Output directory for generated artifacts (defaults to packages/contracts/src)
    #[arg(long, default_value = "packages/contracts/src")]
    output_dir: PathBuf,

    /// Also generate to shared package
    #[arg(long, default_value_t = true)]
    shared_package: bool,

    /// Generate JSON manifest
    #[arg(long, default_value_t = true)]
    json_manifest: bool,

    /// Generate JSON Schema
    #[arg(long, default_value_t = true)]
    json_schema: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Create output directory if it doesn't exist
    if !args.output_dir.exists() {
        std::fs::create_dir_all(&args.output_dir)
            .with_context(|| format!("Failed to create output directory: {:?}", args.output_dir))?;
    }

    println!(
        "Generating CLI capability artifacts to: {:?}",
        args.output_dir
    );

    // Generate TypeScript types
    let ts_path = args.output_dir.join("cli-capabilities.ts");
    generate_typescript_file(ts_path.to_str().unwrap())?;
    println!("✓ Generated TypeScript types: {:?}", ts_path);

    // Generate JSON manifest
    if args.json_manifest {
        let json_path = args.output_dir.join("cli-capabilities.manifest.json");
        generate_json_manifest_file(json_path.to_str().unwrap())?;
        println!("✓ Generated JSON manifest: {:?}", json_path);
    }

    // Generate JSON Schema
    if args.json_schema {
        let schema_dir = args.output_dir.join("../schemas");
        if !schema_dir.exists() {
            std::fs::create_dir_all(&schema_dir)?;
        }
        let schema_path = schema_dir.join("cli-capabilities.v1.schema.json");
        generate_json_schema_file(schema_path.to_str().unwrap())?;
        println!("✓ Generated JSON Schema: {:?}", schema_path);
    }

    // Generate to shared package
    if args.shared_package {
        let shared_dir = Path::new("packages/shared/src");
        if !shared_dir.exists() {
            std::fs::create_dir_all(shared_dir)?;
        }
        let shared_ts_path = shared_dir.join("cli-capabilities.ts");
        generate_typescript_file(shared_ts_path.to_str().unwrap())?;
        println!(
            "✓ Generated TypeScript types to shared: {:?}",
            shared_ts_path
        );
    }

    println!("CLI capability artifact generation complete!");
    Ok(())
}
