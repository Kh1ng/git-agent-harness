# git-agent-harness

Local-first CLI control plane for git agents. Provides deterministic, read-only
repo inventory, scouting, candidate backlog generation, and safe local patch
execution. All operations are local-file-based and do not require external
providers unless explicitly configured.

## Prerequisites

- [Rust](https://rustup.rs/) (cargo 1.96+ / rustc 1.96+)
- Git
- Optional: [GitHub CLI](https://cli.github.com/) (`gh`) for GitHub integration

## Install

```bash
cargo build --release
```

The binary `gah` is produced at `target/release/gah`.

## Quick Start

```bash
# Show available commands
cargo run -- --help

# Run candidates pipeline
cargo run -- candidates --gate-artifact <path> --out-root <output-dir>

# Check model price limits
cargo run -- price-guard --watchlist <file> --model <model-id>

# Evaluate policy
cargo run -- policy-check --config <toml> --action <action>
```

## Test

```bash
cargo test
```

Runs the test suite against local fixtures. No network access required.

## Build

```bash
cargo build --release
```

## Project Structure

```
src/
  main.rs          — CLI entry point and command routing
  models.rs        — shared data types (Gate, Scout, Candidate, Policy, etc.)
  candidates.rs    — candidate generation and hydration from scout data
  price_guard.rs   — model price/availability guard logic
  policy.rs        — trust-mode and provider mutation policy evaluation
tests/
  gah_cli.rs       — integration tests using local fixtures
  fixtures/        — JSON and TOML test fixtures
```

## Verification

```bash
cargo fmt --check
cargo test
```

## Notes

- This is the control plane, not an agent executor.
- Provider mutation is disabled by default.
- All GitHub operations are read-only unless explicitly configured otherwise.
