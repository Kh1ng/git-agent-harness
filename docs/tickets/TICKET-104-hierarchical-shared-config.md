# TICKET-104: Implement hierarchical shared configuration

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
