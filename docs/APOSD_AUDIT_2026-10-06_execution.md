# Execution and persistence quantitative audit — 2026-10-06

Scope: every production source file under `src/runner/`, `src/dispatch/`, and `src/ledger/`, at source snapshot `c4730902` (PR merge snapshot `8d71ec0a`). **86 files scanned: 19 runner, 53 dispatch, 14 ledger.** This includes the Vibe Python credential shim. Dedicated test files/directories and test utilities are excluded; inline Rust `#[cfg(test)]` items are excluded by syntax-tree traversal, rather than truncating at the first test attribute (production declarations sometimes follow test-only items). Platform-specific production variants are included. The complete inventory below makes the scope reproducible.

The scan inspected function bodies, public declarations, identifier bindings, error implementations, and repeated literals across every file, then checked candidates against the skill's exceptions. Counts are static design observations, not claims about correctness or runtime reliability. Duplication counts below are the individually confirmed knowledge units; other repeated literals are not automatically classified as duplicated knowledge.

## Design Health Score

| Dimension | Score | Count and rubric mapping |
|---|---:|---|
| Pass-through proliferation | 3/4 | 2 same-signature delegates: `runner/output.rs:264` and `ledger/summary.rs:694`; 1–2 maps to 3. No qualifying A→B→C chain identified. |
| Information duplication | 0/4 | 11 confirmed knowledge units below; ≥8 maps to 0. The backend log artifact name also occurs in ≥5 independent files. |
| Interface documentation | 2/4 | 55/114 public function/trait contracts documented (48.2%); 59 gaps. 40–69% maps to 2. |
| Naming quality | 3/4 | 1 confirmed vague binding, `ledger/summary.rs:166` (`data`); 1–2 maps to 3. |
| Exception discipline | 0/4 | 10 custom Rust error types listed below; the explicit ≥10 threshold maps to 0. This mechanical penalty does not establish that those typed errors should be deleted. |
| **Total** | **8/20 — Poor** | **Tactical Tornado Risk: Medium** (three dimensions ≤2). |

Paths in the tables are relative to `src/`. The skill overlaps “6–10” and “10+” at exactly ten custom errors; this report uses its explicit 10+ lower-score threshold consistently.

## Findings

**E1 [P2] Ledger wire vocabulary has multiple owners — Duplication, 10 knowledge units.** The following literals are interpreted or constructed independently; each row counts once regardless of the number of occurrences. Failure classes/stages are counted as two vocabulary definitions rather than inflating the count for every member.

| Knowledge | Evidence: file:line:pattern |
|---|---|
| paid grant event | `dispatch/claims.rs:393`, `ledger/approvals.rs:37`: `"paid_route_approval_grant"` |
| paid revoke event | `dispatch/claims.rs:408`, `ledger/approvals.rs:40`: `"paid_route_approval_revoke"` |
| external request event | `dispatch/claims.rs:395`, `ledger/approvals.rs:219`: `"external_approval_request"` |
| external grant event | `dispatch/claims.rs:394`, `ledger/approvals.rs:231`: `"external_approval_grant"` |
| external consume event | `dispatch/claims.rs:396`, `ledger/approvals.rs:243`: `"external_approval_consume"` |
| external revoke event | `dispatch/claims.rs:397`, `ledger/approvals.rs:263`: `"external_approval_revoke"` |
| external expire event | `dispatch/claims.rs:398`, `ledger/approvals.rs:277`: `"external_approval_expire"` |
| external deny event | `dispatch/claims.rs:399`, `ledger/approvals.rs:291`: `"external_approval_deny"` |
| failure-class vocabulary | `dispatch/prior_attempts.rs:333`: `ALLOWED_FAILURE_CLASSES`; `ledger/entry.rs:57`: `FailureClass::as_str` mapping |
| failure-stage vocabulary | `dispatch/prior_attempts.rs:350`: `ALLOWED_FAILURE_STAGES`; `ledger/entry.rs:100`: `FailureStage::as_str` mapping |

Impact: adding a ledger event or attribution value requires remembering independent readers as well as the writer. Recommendation: put the persisted event discriminants and failure-vocabulary validation beside ledger definitions, retaining unknown/historical string handling at the persistence boundary. Keep event-specific transition rules in their current owners; sharing the strings does not imply collapsing those rules.

**E2 [P2] Backend log artifact path is independently constructed — Duplication, 1 knowledge unit.** `runner/backends/agy.rs:250`, `claude.rs:49`, `codex.rs:27`, `hermes.rs:32`, `opencode.rs:58`, `openhands.rs:68`, and `vibe.rs:29` each use `session_dir.join("backend-output.log")`; dispatch failure paths repeat that filename in `dispatch/workflows/experiment.rs:149`, `pm.rs:190`, and `research.rs:146`. Impact: renaming the artifact requires synchronized runner and failure-path changes. Recommendation: expose one artifact filename constant and reuse it at all ten constructors; a new service or trait is unnecessary.

**E3 [P2] Public contracts have documentation gaps — Documentation, 59/114 contracts.** Examples: `runner/resolve.rs:29` (`pub fn require_backend_executable`), `runner/review.rs:80` (`pub fn run_review_backend_for_identity`), `ledger/jsonl.rs:161` (`pub fn append`), and `ledger/summary.rs:405` (`pub fn build_summary`). These interfaces require callers to read implementation for side effects, failure behavior, and identity/unknown-value semantics. Recommendation: document those contracts and the remaining declarations in the denominator inventory below when touching the owning module. Module-level commentary alone does not document a method contract.

Documentation denominator: 110 explicit unrestricted `pub fn` declarations plus the four declarations/default methods of public `BackendRunner` (`runner/backend_runner.rs:101`, `:102`, `:108`, `:116`). Restricted `pub(crate)`, `pub(super)`, and `pub(in ...)` functions, private methods, trait implementations (already counted at their public trait declaration), fields, and reexports are excluded. This is declaration visibility, not downstream crate reachability: this application uses public declarations even inside modules it does not externally export. The 110 declarations have 57 attached doc comments; four name-only checkpoint comments at `dispatch/checkpoints.rs:83`, `:161`, `:259`, and `:266` do not qualify. The trait contributes two documented and two undocumented contracts. Thus `(57 - 4 + 2)/(110 + 4) = 55/114`. Trivial field getters are excluded; transformed values and multi-field operations remain included.

**E4 [P3] Two aliases introduce an extra same-signature hop — Pass-through, 2.** `runner/output.rs:264`: `crate::usage::extract_agy_output_summary(text)` and `ledger/summary.rs:694`: `build_grouped_summary_inner(entries, entry_group_key_fn, usage_group_key_fn, attempt_group_key_fn)`. Neither adds validation, policy, transformations, or error context. Recommendation: use a renamed reexport for the former if the name remains useful; move the latter implementation into the public function once its call sites are checked. These are local navigation costs, not major architectural failures.

**E5 [P3] Summary binding does not identify its contents — Naming, 1.** `ledger/summary.rs:166`: `let data = build_summary(&cfg, since, profile, group_by)?;`. The binding spans a long rendering routine. Rename it `summary` to avoid the generic blocklisted name. `runner/resources.rs:104` and `:133` use `handle` specifically for a `thread::JoinHandle`; those are contextually precise and excluded. Module/import names such as `std::process` and test-local `tmp` are not vague production bindings.

## Exception count and calibration

These ten types explicitly implement `std::error::Error`:

| File:line | Type |
|---|---|
| `dispatch/attempts/external_approval_gap.rs:48` | `ExternalApprovalRequiredError` |
| `dispatch/attempts.rs:186` | `PostAttemptCapacityDeferred` |
| `dispatch/claims.rs:101` | `DuplicateWorkError` |
| `dispatch/claims.rs:127` | `ActiveClaimError` |
| `dispatch/dependencies.rs:51` | `DependencyRelationshipError` |
| `dispatch/repair_context.rs:74` | `StaleSourceError` |
| `dispatch/repair_context.rs:108` | `NoReviewFoundError` |
| `dispatch/review/policy.rs:36` | `ReviewBudgetExhausted` |
| `dispatch/review/policy.rs:68` | `ReviewOutputInvalid` |
| `dispatch/validation.rs:24` | `ValidationGateError` |

This count alone saturates the exception rubric; it is not a severity finding. `PlanReadError` in `dispatch/workflows/pm/plans.rs:57` is serialized result data, not an `Error` implementation, and is excluded. Standard Python `RuntimeError`, Rust `anyhow::Error`, ordinary `?` propagation, and boundary conversion with additional context do not create custom error types. Do not replace typed control outcomes with string matching merely to improve this metric. For example, `dispatch/attempts.rs:144` uses typed downcasts to distinguish capacity contention from execution failure.

## Positive findings and rejected false positives

- `dispatch/review/mod.rs:1–2` declares `context` and `policy`; it contains **zero functions**. Calling this file a pass-through implementation is incorrect. Reexports elsewhere are also not pass-through methods.
- `runner/backend_runner.rs:161` adapts a `RunContext` into backend-specific arguments; that signature transformation is excluded from the same-signature count. `dispatch/workflows/pm/publish.rs:33–59` captures the profile in an `IssuePublisher` implementation and adapts receiver-based calls to provider functions; it is a testable boundary, not six identical-signature aliases.
- `ledger/locking.rs:33` and `:37` choose exclusive versus shared policy (`lock_path(path, false/true)`); `runner/process.rs:627` and `:649` choose supervision policy. Their added policy arguments exclude them from the pass-through count.
- `ledger/jsonl.rs:198` documents and owns the cross-process atomic gate check/append. `ledger/sqlite.rs:308` owns mirror synchronization locking. These concrete persistence contracts reduce caller obligations.

There are **5 actionable clusters: P0=0, P1=0, P2=3, P3=2**. Prioritize ledger vocabulary ownership and public interface documentation, then the shared artifact constant; remove trivial aliases and rename the one binding opportunistically. Apply the `aposd` principles to those focused changes and re-run the same counts afterward. No runtime redesign is justified solely by this score.

## Complete production file inventory

- `src/runner/backend_runner.rs`
- `src/runner/backends/agy.rs`
- `src/runner/backends/claude.rs`
- `src/runner/backends/codex.rs`
- `src/runner/backends/hermes.rs`
- `src/runner/backends/mod.rs`
- `src/runner/backends/opencode.rs`
- `src/runner/backends/openhands.rs`
- `src/runner/backends/vibe/credential_guard.py`
- `src/runner/backends/vibe/credential_guard.rs`
- `src/runner/backends/vibe.rs`
- `src/runner/backends/write_refusal.rs`
- `src/runner/mod.rs`
- `src/runner/output.rs`
- `src/runner/process.rs`
- `src/runner/resolve.rs`
- `src/runner/resources.rs`
- `src/runner/review.rs`
- `src/runner/review_usage.rs`
- `src/dispatch/already_satisfied.rs`
- `src/dispatch/attempts/execution_env.rs`
- `src/dispatch/attempts/external_approval_gap.rs`
- `src/dispatch/attempts/routing_record.rs`
- `src/dispatch/attempts.rs`
- `src/dispatch/checkpoints.rs`
- `src/dispatch/claims.rs`
- `src/dispatch/command.rs`
- `src/dispatch/dependencies.rs`
- `src/dispatch/dry_run.rs`
- `src/dispatch/environment.rs`
- `src/dispatch/error.rs`
- `src/dispatch/external_approval_pause.rs`
- `src/dispatch/identity.rs`
- `src/dispatch/issues.rs`
- `src/dispatch/metrics.rs`
- `src/dispatch/mod.rs`
- `src/dispatch/mutation_policy.rs`
- `src/dispatch/prior_attempts.rs`
- `src/dispatch/prompts.rs`
- `src/dispatch/publish.rs`
- `src/dispatch/repair_context.rs`
- `src/dispatch/repo_inspection.rs`
- `src/dispatch/review/context.rs`
- `src/dispatch/review/mod.rs`
- `src/dispatch/review/policy.rs`
- `src/dispatch/terminal.rs`
- `src/dispatch/text.rs`
- `src/dispatch/validation.rs`
- `src/dispatch/workflows/already_satisfied_reconcile.rs`
- `src/dispatch/workflows/estimator.rs`
- `src/dispatch/workflows/experiment.rs`
- `src/dispatch/workflows/improve/attempt_bookkeeping.rs`
- `src/dispatch/workflows/improve/bounded_validation.rs`
- `src/dispatch/workflows/improve/conflict_resolution.rs`
- `src/dispatch/workflows/improve/finish.rs`
- `src/dispatch/workflows/improve/handoff.rs`
- `src/dispatch/workflows/improve/publish_mr.rs`
- `src/dispatch/workflows/improve/repair.rs`
- `src/dispatch/workflows/improve/shutdown.rs`
- `src/dispatch/workflows/improve/stall.rs`
- `src/dispatch/workflows/improve/work_identity.rs`
- `src/dispatch/workflows/improve.rs`
- `src/dispatch/workflows/mod.rs`
- `src/dispatch/workflows/pm/plans.rs`
- `src/dispatch/workflows/pm/publish.rs`
- `src/dispatch/workflows/pm.rs`
- `src/dispatch/workflows/research.rs`
- `src/dispatch/workflows/review/identity.rs`
- `src/dispatch/workflows/review/source_issue_context.rs`
- `src/dispatch/workflows/review/source_issue_sections.rs`
- `src/dispatch/workflows/review.rs`
- `src/dispatch/workflows/review_external_env.rs`
- `src/ledger/approvals.rs`
- `src/ledger/dispatch_notify.rs`
- `src/ledger/entry.rs`
- `src/ledger/gates.rs`
- `src/ledger/jsonl.rs`
- `src/ledger/locking.rs`
- `src/ledger/mod.rs`
- `src/ledger/paid_route_notify.rs`
- `src/ledger/paid_routes.rs`
- `src/ledger/reconcile.rs`
- `src/ledger/resources.rs`
- `src/ledger/sqlite.rs`
- `src/ledger/summary.rs`
- `src/ledger/usage_unknown.rs`

## Public declaration denominator inventory

Each row lists the explicit public declarations included above. `D` means qualifying documentation; `U` means undocumented or name-only commentary. Public trait contracts are added in the final row.

| File | Line:name (D/U) |
|---|---|
| `src/runner/backend_runner.rs` | 128:for_kind (D) |
| `src/runner/backends/openhands.rs` | 11:load_oh_profile (D), 32:list_oh_profiles (D) |
| `src/runner/mod.rs` | 43:load_env_file (D) |
| `src/runner/process.rs` | 31:install_shutdown_handler (U), 44:run_bounded (D), 137:shutdown_requested (U) |
| `src/runner/resolve.rs` | 16:backend_available (U), 22:backend_available_for_profile (U), 29:require_backend_executable (U), 44:resolve_backend_executable (U), 64:resolve_backend_instance_executable (D), 88:codex_model_args (U), 94:filtered_codex_args (U), 98:extract_model_from_backend_args (U), 141:filtered_backend_args (U), 177:filtered_configured_backend_args (U), 203:extract_model_from_args (U), 245:is_executable_path (U) |
| `src/runner/review.rs` | 63:run_review_backend (U), 80:run_review_backend_for_identity (U) |
| `src/dispatch/already_satisfied.rs` | 50:is_grounded (D), 83:is_test_only_coverage_regression (D), 94:is_test_path_public (U), 142:classify_backend_disposition (D), 168:evidence_is_grounded_in_worktree (D), 251:is_trusted_autonomous_provider (D), 261:reconcile_already_satisfied (D) |
| `src/dispatch/attempts.rs` | 144:capacity_deferred_error (D), 158:node_capacity_deferred_error (U), 853:review_preflight (D), 862:review_preflight_for_identity (U) |
| `src/dispatch/checkpoints.rs` | 52:load (D), 73:save (D), 83:register_checkpoint (U), 120:get_latest_checkpoint (D), 132:get_latest_checkpoint_for_branch (D), 142:tombstone_checkpoint (D), 161:prune_expired (U), 227:find_existing_checkpoints (D), 259:get_checkpoint_sha (U), 266:create_worktree_from_checkpoint (U), 299:resume_checkpoint_into_worktree (D), 318:is_valid_checkpoint (D), 334:record_checkpoint_in_ledger (D), 349:mark_attempt_as_resumable (D), 368:prune_checkpoints (D), 451:find_latest_resumable_checkpoint (U) |
| `src/dispatch/claims.rs` | 820:merge_branch (U) |
| `src/dispatch/mod.rs` | 158:run (U) |
| `src/dispatch/review/policy.rs` | 38:review_budget_exhausted_error (U) |
| `src/dispatch/validation.rs` | 115:self_check_validation_gate (D) |
| `src/dispatch/workflows/already_satisfied_reconcile.rs` | 25:new (U), 43:reconcile (U), 64:enforce_post_validation_changes (U) |
| `src/dispatch/workflows/improve/repair.rs` | 22:new (U) |
| `src/ledger/approvals.rs` | 12:active_paid_route_approval_destinations_from_entries (D), 327:external_approval_snapshot_from_entries (D), 358:external_approval_snapshots_from_entries (D), 532:active_external_approval_env_vars_from_entries (U), 565:record_external_approval_consumption_for_work_item (U), 632:complete_external_approvals_for_work_item (D) |
| `src/ledger/entry.rs` | 57:as_str (U), 100:as_str (U), 210:unavailable (D), 239:is_fully_unknown (D), 474:review_generation (U), 739:normalized_for_persistence (D), 760:new (U), 872:set_failure (D), 882:new_clear_attempts (D), 995:new_claim (D), 1009:new_review_hold (D), 1025:new_review_hold_release (D), 1035:new_paid_route_approval (D), 1054:new_paid_route_approval_for_instance (U), 1086:new_external_approval (U) |
| `src/ledger/gates.rs` | 33:is_review_derived_gate (D), 84:is_entry_stale (U), 106:effective_human_gate_from_entries (D), 242:effective_human_gate_from_index (U), 255:work_id_aliases (D), 271:index_entries_by_work_id (U) |
| `src/ledger/jsonl.rs` | 22:active_paid_route_approvals (D), 35:active_paid_route_approvals_from_entries (U), 97:active_review_hold_work_ids (D), 108:active_review_hold_work_ids_from_entries (U), 161:append (U), 179:append_external_approval (U), 198:append_human_gate_if_transition (D), 249:repair_truncated_tail (D), 369:read_entries (U), 426:backfill_review_verdict (U), 488:entries_for_work_id (D), 504:review_already_exists (D) |
| `src/ledger/paid_routes.rs` | 34:paid_route_approvals_from_entries (D) |
| `src/ledger/reconcile.rs` | 134:read_reconciliation_entries (U), 334:run (U) |
| `src/ledger/resources.rs` | 38:measured (U), 46:unsupported (U), 54:unknown (U), 77:never_launched (D) |
| `src/ledger/sqlite.rs` | 9:db_path (U), 308:sync_from_jsonl (D), 313:rebuild_from_jsonl (U) |
| `src/ledger/summary.rs` | 158:run_with_json (U), 405:build_summary (U), 694:build_grouped_summary (D), 1151:is_strong_model (D), 1172:usage_summary_for_backend (U) |
| `src/runner/backend_runner.rs` (trait) | 101:kind (U), 102:run (U), 108:observe_skills (D), 116:review_invocation (D) |
