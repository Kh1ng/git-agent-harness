# TICKET-059: Fix strong-run counting to key on model strength, not mode

Goal: Change strong-run counting in usage_summary_for_backend so routing caps are based on model strength, not command mode.

Difficulty: easy
Risk: low
Recommended backend: codex
Suggested MR Title: TICKET-059: Fix strong-run counting to key on model strength

## Why This Is Uncovered
usage_summary_for_backend counts every improve/fix/review run as "strong" regardless of model. Cheap models like deepseek-flash running those modes should not count against strong-model caps. The routing module's max_total_strong_model_runs_per_* caps are useless because every cheap run inflates the count.

## Affected Files
- src/ledger.rs

## Acceptance Criteria
1. Add a small helper function `is_strong_model(model: &str) -> bool` in ledger.rs that returns false for model names containing "flash", "mini", "tiny", "lite" (case-insensitive match on the final path segment after last `/`), and true otherwise. Document the heuristic assumption in a comment.
2. Change `usage_summary_for_backend` so strong-run counting keys off the resolved model/backend strength via `is_strong_model(entry.effective_model)`, not the mode name.
3. The confidence_impact guard (entry.confidence_impact.as_deref() != Some("low")) should remain — fallback/review-fallback still should not count as strong even if the model name looks strong.
4. Add 3 tests proving:
   a. A cheap/flash model run in improve/fix/review mode does NOT increment strong-run count.
   b. A strong model run in improve/fix/review mode DOES increment strong-run count.
   c. Existing usage summary behavior (runs_this_week, cost, etc.) remains unchanged except for the corrected strong count.

## Constraints
- Keep the change minimal.
- Do not rename public CLI flags.
- Do not rewrite routing.
- Do not alter ledger entries unrelated to strong-run classification.

## Verification Commands
- `cargo test usage_summary_for_backend 2>&1 | tail -10`
- `cargo test 2>&1 | tail -5`
- `cargo build --quiet 2>&1`
