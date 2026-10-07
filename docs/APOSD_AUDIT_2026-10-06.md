# APOSD audit: core subsystems

Date: 2026-10-06. Addresses [issue #1391](https://github.com/Kh1ng/git-agent-harness/issues/1391).

## Scope and method

Audited **212 production files** at source snapshot `c4730902` (PR merge snapshot
`8d71ec0a`): config, routing, controller, runner, dispatch, ledger, and all server
production TypeScript, including routes, session management, fleet dispatch and
worker protocol. Dedicated tests/fixtures and inline test-only Rust items are
excluded. The three appendices contain complete inventories, counting rules,
documentation denominators and file:line evidence. Scans covered every scoped
file, followed by contextual review of candidate findings; this is a design
audit, not a proof of runtime correctness.

The `aposd-audit` rubric scores five dimensions **0–4**, with higher scores better.
Pass-through and naming scores decrease at 1, 3, 6 and 10 constructs; duplicated
knowledge at 1, 3, 5 and 8 units. Documentation scores use <10%, 10–39%, 40–69%,
70–89% and ≥90% documented. Exception scoring counts custom error types and
redundant handlers; at overlapping thresholds the lower score is used.
Counts are structural signals: useful domain errors and transport adapters
can lower a score without constituting a defect or a high-priority finding.

## Five-dimension scores

| Scope | Files | Pass-throughs | Duplication | Documentation | Naming | Exceptions | Total |
|---|---:|---:|---:|---:|---:|---:|---:|
| [Config, routing, controller](APOSD_AUDIT_2026-10-06_config.md) | 42 | 2 | 2 | 2 | 3 | 3 | 12/20 (Acceptable) |
| [Runner, dispatch, ledger](APOSD_AUDIT_2026-10-06_execution.md) | 86 | 3 | 0 | 2 | 3 | 0 | 8/20 (Poor) |
| [Server](APOSD_AUDIT_2026-10-06_server.md) | 84 | 0 | 2 | 2 | 0 | 0 | 4/20 (Critical rubric band) |

Underlying counts respectively:

- Pass-through constructs: **4 / 2 / 25**, including a server forwarding chain.
- Confirmed duplicated knowledge units: **4 / 11 / 4**.
- Documented public contracts: **49/114 / 55/114 / 258/536**.
- Vague production bindings: **1 / 1 / 13**.
- Custom error types: **2 / 10 / 8**; server additionally has **48** pure
  log/rethrow/empty catches. The appendices distinguish intentional boundaries.

Scope scores are shown separately rather than averaged: language visibility,
module size and intentional error handling affect the raw denominators.
Tactical tornado risk is Medium for each Rust group and High for the server
under the rubric; these labels do not establish release blockers.

## Findings by severity and disposition

**P0: 0. P1: 0. P2: 9. P3: 7 clustered findings.** No verified P0/P1 finding
requires a fix or follow-up issue. The following are the highest-priority P2
clusters; all counts, locations, impact and concrete recommendations are in the
linked appendices.

1. **Controller decisions parse prose across modules.** Two protocols connect
   `controller/runtime/route_state.rs`, admission/runtime and recovery. Introduce
   typed in-process outcomes and one compatible persisted protocol codec when
   changing these decisions.
2. **Ledger vocabulary has multiple owners.** Ten knowledge units are repeated
   across dispatch and ledger. Centralize discriminants and vocabulary validation
   while preserving historical/unknown records.
3. **Server policy and payload knowledge is repeated.** Four clusters cover chat
   storage paths, terminal retention, approval identifiers and dispatch option
   projection. Share each rule at its narrowest common owner.
4. **Public contract documentation is incomplete.** Document config precedence,
   reservation ownership, controller side effects, persistence guarantees and
   session start/cancel/replay semantics first. The appendices identify the
   **402 undocumented of 764 eligible declarations**.

Other P2 findings cover mutable candidate traversal, review-family classification
and the backend log artifact filename. P3 findings cover local delegates, names
and explicit error-boundary policy. Lower-priority recommendations are recorded
for work in those modules; this audit does not introduce speculative runtime
refactors solely to improve a metric.

## Corrections to the original PR

- `src/dispatch/review/mod.rs` contains module declarations, not forwarding
  functions. The original P1 claim was unsupported.
- CLI file length alone did not demonstrate a P1 design defect and CLI was not
  a requested core subsystem. Removed that finding and both unfiled ticket stubs;
  their numbers also referred to unrelated GitHub work.
- Main already contains the candidate traversal extraction with allow-list
  support. The resolved PR retains main's implementation and has no production
  code delta.
- Removed unrelated timeout/locking test changes and accidental patch, reject
  and backup files. Existing test assertions remain intact.

## Validation

The final change is documentation-only against the audited main snapshot.
Acceptance still requires `cargo clippy --all-targets --all-features -- -D warnings`
and `cargo test`, plus green PR CI. Final command results are recorded in
[PR #1451](https://github.com/Kh1ng/git-agent-harness/pull/1451).
