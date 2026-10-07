# APOSD audit: config, routing, controller

Date: 2026-10-06. Baseline: `8d71ec0a` (PR #1451 after merging main).

## Method and scope

Full static construct scan of all 42 production Rust files in `src/config.rs`, `src/config/`, `src/routing/`, and `src/controller/`; complete inventory below. Dedicated test files and `cfg(test)` items are excluded from scores; platform-specific production paths are included. This measures design constructs, not runtime correctness, performance, or test coverage.

Documentation denominator: unrestricted `pub fn` declarations, including those inside implementation modules. Excludes restricted `pub(crate)`/`pub(super)`, private functions, trait implementations without `pub`, test-only functions, and five direct field-projecting NextAction getters (`reason`, `work_id`, three `human_required_*` methods). Defaulting/clamping methods remain because they encode behavior. Method comments describing behavior/contracts count; comments on fields do not substitute for method comments.

## Score and executive summary

| Dimension | Concrete count and rubric | Score |
|---|---|---:|
| Pass-through proliferation | 4 single-delegate methods, no identified pass-through chain; 3-5 maps to 2 | 2/4 |
| Information duplication | 4 distinct knowledge clusters across modules; 3-4 maps to 2 | 2/4 |
| Interface documentation | 49/114 documented (43.0%); 65 missing; 40-69% maps to 2 | 2/4 |
| Naming quality | 1 blocklisted binding; 1-2 maps to 3 | 3/4 |
| Exception discipline | 2 custom error types, 0 redundant catch-and-rethrow handlers; 1-2 maps to 3 | 3/4 |
| **Total** | **Acceptable** | **12/20** |

Tactical tornado risk: **Medium** (three dimensions score at most 2). Findings: **P0 0, P1 0, P2 4, P3 2**. Counts do not automatically require removing justified boundaries or domain errors. No P0/P1 refactoring ticket is warranted from this scope.

## Findings

### [P2] Mutable candidate collection independently repeats configuration structure

Dimension: duplication. Count: 1 knowledge cluster in two modules. `src/config/routing_policy.rs:179` (`pub fn labeled_candidates`) includes all collections including allow-list entries. `src/config/backend_instances.rs:368` starts a separate `for candidate in self.pm_candidates.iter_mut().flatten().chain(...)` for subscription normalization. It lists six collections without `allowed_models`. Read-only traversal is now centralized through `backend_instances.rs:396` (`routing.labeled_candidates().into_iter().map(...)`). Recommendation: define a mutable visitor beside the read-only collection when editing normalization and explicitly specify how allow-list entries participate; test every category. Otherwise adding a candidate collection requires updating different traversals. This is inherited main-branch debt, not a regression of the documentation-only final PR.

### [P2] Controller decisions parse prose produced elsewhere

Dimension: duplication. Count: 2 separate knowledge protocols. `src/controller/runtime/route_state.rs:66` emits `Deferred {label} because {capacity} capacity is busy; no backend launched`; `src/controller/runtime/admission.rs:338` tests `starts_with("Deferred ")` and `contains("because node capacity is busy")`, while `src/controller/runtime.rs:1043` also tests `no backend launched`. Separately, `route_state.rs:36,44` emits `route_state=` and `: deferred_capacity:`, parsed at `src/controller/recovery.rs:95,102` and recognized at line 251. Wording changes can affect refill or cooldown decisions. Recommendation: return a typed in-process dispatch outcome and render prose at reporting; centralize encoding/decoding of the existing persisted event-detail protocol while preserving old ledger compatibility.

### [P2] Review-family classification is independently restated

Dimension: duplication. Count: 1 knowledge cluster. `src/routing/decision.rs:34` and `src/routing/policy.rs:392` both define `JobKind::parse(mode).map(|kind| kind.family()) == Ok(JobFamily::Review)`. Recommendation: share this predicate when changing classification, retaining unknown-mode behavior. Future classification edits otherwise need synchronized changes.

### [P2] Public contracts have documentation gaps

Dimension: documentation. Count: 65 undocumented of 114 eligible functions. Examples: `src/config.rs:489` (`pub fn load`), `src/config/routing_policy.rs:214` (`pub fn allowed_models_for`), `src/controller/runtime.rs:262` (`pub fn run_once`), `src/controller/remediation.rs:319` (`pub fn plan_remediation`), `src/routing/reservation.rs:46` (`pub fn acquire`). Recommendation: document config precedence, controller side effects, allow-list semantics, and reservation ownership first. The allow-list explanatory comment currently precedes `labeled_candidates` at `routing_policy.rs:171`; move that part to `allowed_models_for`. The inventory gives every file's documented numerator and denominator.

### [P3] Four single-delegate boundaries

Dimension: pass-through. Count: 4. `src/routing/mod.rs:26`: `reservation::current_concurrent(backend, model)`; `src/routing/diagnostics.rs:15`: `self.backend()`; `src/config/external_credential_scopes.rs:22`: `self.external_credential_scopes.get(label)`; `src/controller/runtime/admission.rs:186`: `channel()`. Recommendation: use a facade re-export if visibility permits; retain the diagnostic default and typed channel boundary when their interface benefit justifies them, and avoid further forwarding layers. Argument-transforming identity adapters, validation wrappers, re-exports, and field getters do not count here.

### [P3] Vague credential binding

Dimension: naming. Count: 1. `src/config/routing_policy.rs:296`: `crate::credentials::get(id).is_ok_and(|info| { info.kind == ... })`. Recommendation: rename `info` to `credential` when changing this path. No other production binding matching the exact skill blocklist or convention violation was identified; words in comments and test `tmp` bindings are excluded.

## Exception evidence and positive findings

Custom errors: `src/routing/types.rs:192` (`RouteError`, Error impl line 275) and `src/controller/runtime/admission.rs:176` (`NodeAdmissionDeferred`, Error impl line 184). Both encode domain information; no recommendation removes them merely to raise the rubric score. Selective propagation after handling WouldBlock/NotFound (`routing/reservation.rs:108`, `controller/runtime/node_capacity.rs:319`) is meaningful error handling, not redundant catch-and-rethrow. No redundant handler was identified.

Positive boundaries: RAII releases reservations (`routing/reservation.rs:205`); `config/node_capacity.rs:43` owns the adaptive memory-floor formula; the controller facade uses re-exports without forwarding methods; read-only candidate validation shares `labeled_candidates`. Naming and exception dimensions score at least 3 with concrete counts above.

## PR refactor correctness

The original extraction preserved six candidate categories, labels, order, and optional-list behavior. Main subsequently introduced a broader collection including allow-list entries. The merged PR retains main's implementation: `src/config.rs:454` calls `labeled_candidates`, and `src/config/backend_instances.rs:396` derives unlabeled candidates from it. There is no remaining production-code delta relative to merged main. No correctness regression or P0/P1 issue was found in this extraction. File length and re-exports alone are not major findings.

## Complete production inventory and documentation ledger

Numbers are documented/eligible public functions. Zero means no eligible public function, not an omitted scan.

| File | Documented / total |
|---|---:|
| `src/config.rs` | 9/18 |
| `src/config/autonomy.rs` | 0/0 |
| `src/config/backend_instances.rs` | 3/7 |
| `src/config/backend_paths.rs` | 1/10 |
| `src/config/default_paths.rs` | 3/4 |
| `src/config/delivery.rs` | 0/1 |
| `src/config/external_credential_scopes.rs` | 0/3 |
| `src/config/issue_intake.rs` | 0/1 |
| `src/config/merge_policy.rs` | 1/1 |
| `src/config/node_capacity.rs` | 3/5 |
| `src/config/publishing.rs` | 0/5 |
| `src/config/routing_policy.rs` | 8/23 |
| `src/config/worker_scaling.rs` | 0/2 |
| `src/controller/action.rs` | 1/1 |
| `src/controller/decision.rs` | 1/1 |
| `src/controller/human_required_reason.rs` | 2/2 |
| `src/controller/mod.rs` | 0/0 |
| `src/controller/ownership.rs` | 0/0 |
| `src/controller/recovery.rs` | 0/0 |
| `src/controller/remediation.rs` | 0/2 |
| `src/controller/runtime/action_execution.rs` | 0/0 |
| `src/controller/runtime/admission.rs` | 0/0 |
| `src/controller/runtime/dispatch_policy.rs` | 0/0 |
| `src/controller/runtime/dispatch_state.rs` | 0/0 |
| `src/controller/runtime/intake.rs` | 0/0 |
| `src/controller/runtime/merge.rs` | 0/0 |
| `src/controller/runtime/node_capacity.rs` | 0/0 |
| `src/controller/runtime/node_reprobe.rs` | 0/0 |
| `src/controller/runtime/pm.rs` | 0/0 |
| `src/controller/runtime/probe.rs` | 0/1 |
| `src/controller/runtime/profile_lock.rs` | 1/1 |
| `src/controller/runtime/route_state.rs` | 0/0 |
| `src/controller/runtime.rs` | 2/3 |
| `src/routing/decision.rs` | 1/2 |
| `src/routing/diagnostics.rs` | 0/0 |
| `src/routing/mod.rs` | 1/1 |
| `src/routing/policy.rs` | 0/0 |
| `src/routing/reservation.rs` | 2/6 |
| `src/routing/reviewer_history.rs` | 0/0 |
| `src/routing/subscription.rs` | 4/4 |
| `src/routing/types.rs` | 2/6 |
| `src/routing/worker_scaling.rs` | 4/4 |

Recommended order: behavior documentation, controller outcome protocols, mutable candidate traversal, then local predicate/wrapper/naming cleanup. Apply the `aposd` design skill during implementation and rerun this audit after fixes.
