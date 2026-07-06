# TICKET-104: Implement hierarchical shared configuration

Status: CLOSED — SUPERSEDED BY TICKET-106 (2026-07-05)

This ticket and TICKET-106 ("Add Canonical Shared Routing Policy Inheritance") describe the
same feature — a scope duplication, not an ID collision (both numbers were validly assigned,
but to the same idea). TICKET-106 is the broader, canonical version (full test list, the
actual priority-board entry, introduces the genuinely new "shared canonical config file"
layer this ticket's own Pre-Discovery below shows doesn't exist yet — the existing
`profile.routing` → `defaults.routing` fallback this ticket describes was already true
before either ticket existed). This ticket's Pre-Discovery was folded into TICKET-106's
implementation. See docs/tickets/TICKET-106-shared-routing-policy-inheritance.md.

This ticket's narrower remaining ask — actually editing this repo's own `[profiles.gah.routing]`
to stop duplicating fields now covered by `[defaults.routing]`/the new canonical layer — is an
operational config edit, not a code change, and is left to the operator (out of scope for a
"GAH-side only, don't touch other repos' real config" implementation pass).

Goal: Move the current known-good routing/model/backend policy from the GAH repo config into a shared canonical defaults, so all repos inherit automatically unless they explicitly override.

Difficulty: medium
Risk: medium
Recommended backend: codex
Recommended model: gpt-5.4
Suggested MR Title: Hierarchical shared configuration with canonical routing defaults

## Pre-Discovery (Complete)

### Current config loading path
- `config.rs::load(config_path)` → resolves path → `toml::from_str` → `GahConfig { defaults: Defaults, profiles: HashMap<String, Profile> }`
- `Defaults` has `routing: RoutingPolicy` (already exists as global fallback)
- `Profile` has `routing: RoutingPolicy` (per-repo override)
- Search order: `$GAH_CONFIG` env var → `/home/khing/workspace/agent-lab/config/gah-config.toml` → `~/.config/gah/config.toml`

### Existing inheritance (in routing.rs)
- `policy_candidates(&profile.routing, mode)` → if empty → `policy_candidates(&defaults.routing, mode)`
- `policy_backend_model(&profile.routing, mode)` → if None → `policy_backend_model(&defaults.routing, mode)`
- Fallback flags: `profile.routing.allow_review_fallback || defaults.routing.allow_review_fallback`
- `explicit_candidates` receives both `&profile.routing` and `&defaults.routing`

### Current GAH repo routing policy (in `[profiles.gah.routing]`)
All routing fields are fully specified, duplicating what could be shared defaults.

### What needs to change
1. Move reusable routing policy from `[profiles.gah.routing]` to `[defaults.routing]`
2. Remove duplicated fields from `[profiles.gah.routing]`
3. Ensure field-level merge: profile override of one routing field doesn't erase inherited defaults for other fields
4. Backward compatible: standalone repo configs without defaults still work

## Acceptance Criteria

1. Shared `[defaults.routing]` is the canonical policy source
2. GAH profile inherits from defaults, overrides only repo-specific settings
3. New minimal repo config automatically receives canonical routing
4. Profile override of one routing field preserves inherited defaults for non-overridden fields
5. Ordered candidate list in profile replaces inherited list (not concatenate)
6. No shared defaults → legacy standalone config behavior preserved
7. Repo-specific validation commands remain repo-specific
8. CLI/runtime override still wins over everything
9. Effective GAH routing is behaviorally equivalent before and after migration

## Affected Files

- `src/config.rs` — Merge logic for layered config
- `src/routing.rs` — May need adjustment if merge semantics change candidate resolution
- Config files — Move shared policy to defaults

## Constraints

- No warning suppression
- No dead-code laundering
- No duplicated canonical routing
- No hardcoded one-user paths
- Preserve existing configs
- One reviewable PR

## Verification Commands

- `cargo fmt --check`
- `cargo test`
- `cargo clippy --all-targets --all-features -- -D warnings`
