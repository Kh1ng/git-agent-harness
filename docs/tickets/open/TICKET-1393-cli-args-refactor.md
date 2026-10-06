# TICKET-1393: Refactor CLI Args God Class

## Problem
The `src/cli/args.rs` module violates the APOSD principles regarding information hiding and modularity. At ~1500 lines, it acts as a God Class, intertwining parsing, validation, and subcommand definitions into a single, overly complex file.

## Acceptance Criteria
- `src/cli/args.rs` is broken down into smaller, specialized modules (e.g., separating subcommand parsing logic from the main parser).
- Subcommands are modeled as deep modules that hide their specific implementation details from the root command parser.
- `cargo check` and `cargo test` pass.

## Labels
refactor, P1
