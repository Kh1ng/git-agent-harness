# TICKET-100: Add isolated multiple AGY backend instances

Goal: Support two independently authenticated AGY instances (agy-main, agy-second) as distinct GAH backend resources with separate availability and ledger identity.

Difficulty: medium
Risk: low
Recommended backend: agy
Recommended model: Gemini 3.5 Flash (Medium)
Suggested MR Title: Add isolated AGY multi-instance backend support

## Pre-Discovery (Complete)

AGY stores all state under `$HOME/.gemini/antigravity-cli/`:
- OAuth token: `antigravity-oauth-token`
- Config: `settings.json`
- State: `brain/`, `cache/`, `conversations/`, etc.

Changing `HOME` fully isolates the instance. No environment variables needed beyond `HOME`.

**Wrapper scripts already created:**

- `/home/khing/.local/bin/agy-main` — uses default HOME, existing auth
- `/home/khing/.local/bin/agy-second` — sets `HOME=/home/khing/.local/share/gah/agy-instances/agy-second`, unauthenticated until manual login

**Proven:**
- `agy-main --print "hello"` works
- `agy-second --print "hello"` fails with "authentication failed" (correct — no token)

## Required GAH Changes

### 1. Runner (`src/runner.rs`)

- `run_agy()` must accept an executable name parameter (default `"agy"`)
- `backend_available()` must accept `"agy-main"` and `"agy-second"` as valid backend names (checking for the wrapper scripts)
- Each instance uses `Command::new(executable_name)` where executable is `"agy-main"` or `"agy-second"`

### 2. Dispatch (`src/dispatch.rs`)

- `preflight()` must handle `"agy-main"` and `"agy-second"` (currently falls through to `_ => "openhands"`)
- `run_backend()` match must add `"agy-main" | "agy-second"` routing to `run_agy()`
- `"agy"` remains backward-compatible as a generic fallback

### 3. Routing/Config

- Profile routing config can specify `agy-main` or `agy-second` as backend candidates
- Availability automatically separates because backend names differ (`"agy-main"` vs `"agy-second"`)
- Ledger `effective_backend` records `"agy-main"` or `"agy-second"` naturally

### 4. Availability

- Built-in: availability keys on backend name. `agy-main` and `agy-second` are different keys.
- No availability schema change needed.

### 5. Ledger

- Built-in: `effective_backend` records the instance name
- No ledger schema change needed.
- Historical `"agy"` entries continue to deserialize.

## Acceptance Criteria

1. `run_agy()` accepts executable name parameter
2. `backend_available("agy-main")` returns true when wrapper exists
3. `backend_available("agy-second")` returns true when wrapper exists
4. `preflight("agy-main")` succeeds
5. `preflight("agy-second")` succeeds
6. `run_backend("agy-main")` launches `agy-main` wrapper
7. `run_backend("agy-second")` launches `agy-second` wrapper
8. `run_backend("agy")` remains backward-compatible (launches `agy`)
9. Availability for `agy-main` and `agy-second` is independent
10. Ledger records correct instance name
11. Fake backend tests cover separate instance identity
12. Second instance starts unauthenticated (tested: auth failure)
13. Running second instance does not alter first instance auth
14. Running first instance does not authenticate second

## Constraints

- Same AGY executable reused for both instances
- No broad backend-instance abstraction rewrite
- No AGY-specific availability model changes
- No ledger schema changes
- Backward compatible: `"agy"` still works as a backend name

## Manual Sign-In

After implementation is merged:
```
home=/home/khing/.local/share/gah/agy-instances/agy-second agy --print "hello"
```

This will trigger AGY's OAuth flow interactively.

## Verification Commands

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`
- `agy-main --print "ok"` (proves first instance still works)
- `agy-second --print "ok"` (should fail with auth error until login)
