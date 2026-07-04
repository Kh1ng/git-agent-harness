# TICKET-088: Weekly quota pacing policy

Goal: Pure deterministic policy function that computes a pacing band from quota usage, time remaining, and configured thresholds.

Difficulty: medium
Risk: low
Recommended backend: codex
Recommended model: gpt-5.4
Suggested MR Title: Add weekly quota pacing policy with deterministic pacing bands

## Problem

Without quota pacing, the routing system cannot distinguish early-week overspend from healthy consumption. This leads to:
- burning the week's quota in the first day
- hard rate-limit stops rather than soft pacing
- no incentive to share scarce subscription quota across the week

## Acceptance Criteria

1. Pure function: `quota_pace(usage_pct, days_remaining, config) -> PaceBand`
2. PaceBand variants: `AggressiveBurn`, `MildBurn`, `Normal`, `Conserve`, `HardConserve`
3. Target linear pace calculation: `target_pct = 100 - (100 / 7) * days_remaining`
4. Pace delta: `actual_pct - target_pct`
5. Default threshold bands with hysteresis:
   - `>= +20` → AggressiveBurn (under-consuming, headroom)
   - `>= +7` → MildBurn
   - `between -7 and +7` → Normal
   - `<= -7` → Conserve
   - `<= -20` → HardConserve
6. Missing usage data handled honestly (returns Normal, not a fabricated aggressive result)
7. Invalid inputs (negative usage, negative days, usage > 100) return explicit error or HardConserve
8. Thresholds are configurable via profile config
9. Pure unit tests only — no integration or CLI tests required

## Affected Files

- `src/routing.rs` or new `src/quota.rs` — PacePolicy pure function

## Constraints

- No integration with routing ordering yet
- No availability changes
- Pure function, no side effects
- No CLI changes
- No ledger changes

## Verification Commands

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`
