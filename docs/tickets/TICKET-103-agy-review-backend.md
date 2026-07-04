# TICKET-103: Add AGY as a supported review backend

Goal: Wire AGY (with Claude Sonnet) into `runner::run_review_backend()` so reviews can route through AGY before falling back to native Claude or Codex.

Difficulty: easy
Risk: low
Recommended backend: codex
Recommended model: gpt-5.4
Suggested MR Title: Add AGY Claude Sonnet as a supported review backend

## Problem

The review pipeline (`run_review_backend` in `src/runner.rs`) only supports `claude` and `codex` backends. AGY is fully operational for implementation but cannot be selected for review routing despite having `Claude Sonnet 4.6 (Thinking)` available as a model.

The config already has `review_candidates` listing AGY first, but the backend execution code doesn't know how to launch AGY for reviews.

Manually running `agy --print "review prompt" --model "Claude Sonnet 4.6 (Thinking)"` works perfectly — the review prompt format used by `run_review_backend` is compatible with AGY's `--print` mode.

## Acceptance Criteria

1. `resolve_backend_executable` returns `Found` for `"agy"` backends (already works via PATH lookup)
2. AGY review uses the same prompt format as Claude/Codex reviews
3. AGY review captures stdout/stderr/exit code correctly
4. AGY review writes `review-report.md` and `review-verdict.json` on success
5. AGY review timeout is respected (uses `review_timeout_seconds` config)
6. AGY review process outcomes (Success, Timeout, NonZeroExit, etc.) are classified correctly
7. Backward compatible: claude and codex review continue to work unchanged
8. Config `review_candidates` with AGY models routes correctly

## Affected Files

- `src/runner.rs` — `run_review_backend()` or the review dispatch in `src/dispatch.rs` needs to handle AGY backend

## Constraints

- No changes to AGY's `run_agy()` implementation
- No changes to the review prompt format
- No new dependencies
- Claude and Codex review paths unchanged

## Verification Commands

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`
- Manual: `gah dispatch --profile gah --mode review --mr <N> --backend agy --model "Claude Sonnet 4.6 (Thinking)"` works
