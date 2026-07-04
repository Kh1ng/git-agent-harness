# TICKET-102: Harden Claude review execution and JSON parsing in GAH

Goal: Fix Claude review dispatch so it can find Claude regardless of shell PATH, parse review output robustly, preserve raw output on parse failure, and classify process outcomes accurately.

Difficulty: medium
Risk: medium
Recommended backend: codex
Recommended model: gpt-5.4
Suggested MR Title: Harden Claude review execution and JSON parsing

## Problem

Claude review currently fails at multiple points:

1. **Executable resolution** — GAH uses `which claude` in `preflight()` and `backend_available()`. Claude is installed at a non-PATH location (`/home/khing/.nvm/versions/node/v22.23.1/bin/claude`), so GAH considers it unavailable.

2. **ReviewVerdict JSON parsing** — The prompt asks for `blocking_findings` and `non_blocking_findings` as arrays, but Claude sometimes returns a single string. `serde_json::from_str::<ReviewVerdict>` then fails with "expected a sequence".

3. **Raw output lost on parse failure** — When `parse_review_verdict()` returns an error (`?` at dispatch.rs:1494), the entire Claude response is discarded. No `review-report.md` or `review-verdict.json` is written. The error propagates up and the only trace is the error message.

4. **Process outcome not classified** — SIGTERM, timeout, non-zero exit, and missing executable are all folded into the same failure path with no structured distinction.

5. **Review timeout — no backend-owned timeout** — The review function launches Claude synchronously with no configurable timeout. If Claude hangs, GAH hangs with it.

## Pre-Dispatch Investigation (Complete)

### Executable resolution
- `preflight()` maps backend names to binary names via hardcoded match, then calls `ensure_bin()` which runs `which <bin>`
- `backend_available()` in runner.rs has the same hardcoded match + `which` pattern
- `run_claude()` hardcodes `Command::new("claude")` — no way to specify an explicit path
- Profile config has `claude_args: Vec<String>` but no `claude_path` or `executable_path`
- Solution: add optional `claude_path` (and `codex_path`, `agy_path` etc.) to `Profile` config. Preflight/resolver checks explicit path first, then PATH fallback, then unavailable.

### ReviewVerdict struct (src/models.rs:172-190)
```rust
pub struct ReviewVerdict {
    pub verdict: String,
    pub confidence: String,
    pub human_required: bool,
    #[serde(default)]
    pub blocking_findings: Vec<String>,    // Claude sometimes returns string → parse error
    #[serde(default)]
    pub non_blocking_findings: Vec<String>, // Same issue
    #[serde(default)]
    pub risk_notes: Vec<String>,
    // ... backend/model fields
}
```

Fields are `Vec<String>` with `#[serde(default)]`. The fix: a custom deserializer or `#[serde(deserialize_with)]` that accepts string, array, null, and missing.

### Review prompt (dispatch.rs:1463-1468)
Current prompt: `"A JSON object with fields: verdict, confidence, human_required, blocking_findings, non_blocking_findings, risk_notes."`

Needs: explicit instruction that `blocking_findings` and `non_blocking_findings` MUST be JSON arrays of strings, even if empty.

### Parse failure path (dispatch.rs:1490-1534)
- Success: parse verdict, write report/verdict, post comment
- Failure (non-zero exit, missing claude, None): write bundle, print manual instruction
- JSON parse error: `?` propagates up as error — Claude's raw stdout is discarded

Needs: on JSON parse failure, write raw stdout to session dir before returning error.

### Config schema
`Profile` struct has `claude_args: Vec<String>` but no `claude_path`. Add optional `claude_path` field. The runner resolver checks: explicit path → PATH which → unavailable.

## Acceptance Criteria

1. **Executable resolution** — `Profile` gains optional `claude_path` (and `codex_path`, `agy_path` for consistency). `preflight()` and the runner use a shared resolver: explicit path → `which` → unavailable. Backward compatible: absent fields fall back to PATH lookup.

2. **ReviewVerdict JSON hardening** — `blocking_findings`, `non_blocking_findings`, `risk_notes` accept: array of strings (unchanged), single string (normalize to `vec![string]`), null (empty vec), missing (empty vec). Use `#[serde(deserialize_with)]` helper. Prompt explicitly tells Claude these must be JSON arrays.

3. **Raw output preserved on parse failure** — Before `parse_review_verdict()` error propagates, write raw review text to `session_dir/review-report.md`. The error still propagates (review failed), but the evidence survives.

4. **Process outcome classification** — Distinguish: executable unavailable, spawn failure, non-zero exit, signal termination (SIGTERM/SIGKILL), timeout, successful but invalid output. Use the existing `failure_class`/`failure_stage` ledger fields where they fit.

5. **Backend-owned timeout** — Add optional `review_timeout_seconds` to profile config (default 300). Runner applies timeout to the subprocess. On timeout: kill child, record timeout, preserve partial stdout/stderr.

6. **Tests** — See required tests below.

## Required Tests

### Executable resolution
1. explicit `claude_path` works when PATH does not contain claude
2. explicit path absent → PATH fallback still works
3. explicit path leads to nonexistent file → unavailable
4. preflight and runner use the same resolver
5. `agy_path` and `codex_path` follow the same pattern (consistency, not multi-instance)

### JSON hardening
6. array of strings parses correctly
7. single string normalizes to one-element Vec
8. null normalizes to empty Vec
9. missing field normalizes to empty Vec (existing `#[serde(default)]`)
10. genuinely malformed JSON (e.g. unclosed brace) still fails cleanly

### Raw output preservation
11. parse failure writes raw review text to session dir before returning error
12. parse success path is unchanged (evidence already written)

### Process classification
13. executable unavailable classified distinctly from spawn failure
14. nonzero exit captured correctly
15. signal termination preserved (Unix-gated if needed)
16. timeout classified distinctly

### Backward compatibility
17. existing configs without claude_path deserialize correctly
18. historical ledger entries with failure fields still load

## Affected Files

- `src/config.rs` — Add `claude_path`, `codex_path`, `agy_path`, `review_timeout_seconds`
- `src/runner.rs` — Shared executable resolver, profile-aware lookup, timeout
- `src/dispatch.rs` — `preflight()` uses resolver, improved prompt, raw output preservation
- `src/models.rs` — `ReviewVerdict` field deserialization helpers
- `src/availability.rs` or relevant — Process outcome classification where it fits existing patterns

## Constraints

- No `#![allow(warnings)]` anywhere
- No broad lint suppression
- No shell-profile or nvm.sh sourcing hack
- No hardcoded one-user paths as the architectural fix
- No real external model calls in tests
- No unrelated refactor
- No AGY multi-instance work beyond the shared executable resolver
- No new async job orchestration
- Preserve existing config compatibility

## Verification Commands

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`
