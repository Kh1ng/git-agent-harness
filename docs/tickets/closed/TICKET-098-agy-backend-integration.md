# TICKET-098: Add Antigravity CLI as a GAH backend

Goal: Minimal one-instance AGY runner integration so GAH can dispatch tasks to the installed `agy` CLI.

Difficulty: medium
Risk: low
Recommended backend: codex
Recommended model: gpt-5.4
Suggested MR Title: Add Antigravity CLI as a GAH backend

## Pre-Dispatch Investigation (Complete)

Installed AGY CLI:
- Path: `/home/khing/.local/bin/agy`
- Version: 1.0.16
- Non-interactive: `agy --print "prompt"` works without TTY
- Auth: functional (responded successfully)
- Model flag: `--model <name>`
- Permissions bypass: `--dangerously-skip-permissions`
- Timeout: `--print-timeout` (default 5m)
- Scratch dir: `/home/khing/.gemini/antigravity-cli/scratch`

## Acceptance Criteria

1. New `"agy"` backend in `src/runner.rs` that invokes `agy --print --model <model> --dangerously-skip-permissions`
2. Task text is passed via stdin or `--print` prompt argument
3. Worktree cwd is correctly set as the AGY working directory (not the scratch default)
4. Stdout, stderr, and exit code are captured
5. Ledger records `backend = "agy"` honestly
6. Routing can select `agy` via profile config
7. Availability state can disable `agy` (works through existing availability machinery)
8. Hermetic fake-backend tests pass for the `"agy"` name in `tests/fake_backend_harness.rs`
9. No second AGY instance or multi-account support in this ticket

## Affected Files

- `src/runner.rs` — New `run_agy()` function
- `src/dispatch.rs` — Wire `"agy"` backend in `run_backend()`
- Profile config — Backend/model routing for agy
- `tests/fake_backend_harness.rs` — Verify `"agy"` name supported
- `tests/gah_cli.rs` — Integration tests

## Constraints

- Single authenticated instance only
- No AGY-specific availability model
- No second-account support
- No changes to routing architecture beyond adding the candidate
- Ledger and existing infrastructure unchanged except for the new backend name

## Verification Commands

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`
- Manual: `agy --print "hello" --model gpt-5.4-mini` works
