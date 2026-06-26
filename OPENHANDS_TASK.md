You are working in a Rust CLI repo named `git-agent-harness`.

Goal:
Implement the initial Rust CLI app so the existing tests pass.

Command:
`gah`

This is a local-first git agent harness. It is not a coding agent. It is the safety/control-plane CLI that turns scout/gate artifacts into backlog candidates, checks model price policy, and enforces repo trust mode.

Hard constraints:
- Do not remove tests.
- Do not weaken tests.
- Do not change fixture semantics to make tests easier.
- Do not add network calls.
- Do not call GitHub.
- Do not call GitLab.
- Do not call paid models.
- Do not create PRs/MRs.
- Do not push branches.
- Do not create or edit issues.
- Filesystem-only behavior for this MVP.
- Rust stable.
- Prefer simple, explicit code over clever abstractions.

Implement enough for:
`cargo test` to pass.

Expected CLI behavior:

1. `gah --help`
- exits 0
- help text contains `git agent harness`

2. `gah candidates --gate-artifact <DIR> [--include-warnings] --out-root <DIR>`
- Reads `<DIR>/gate.json`.
- Writes a unique artifact directory under:
  `<out-root>/scout-to-backlog-candidates/<timestamp-or-unique-slug>/`
- Never overwrites an existing artifact dir.
- Writes:
  - `candidates.json`
  - candidate markdown files under `candidates/`

Candidate policy:
- Default: convert only gate findings with `gate_status: approved`.
- `gate_status: warn` is skipped by default.
- With `--include-warnings`, convert `approved` and `warn`.
- `rejected` is never converted.
- Never output `agent:ready`.
- `provider_mutation_allowed` must always be false in this candidate layer.

Hydration:
- Gate findings may be sparse.
- If `gate.json` has `source_scout_artifact`, read `<source_scout_artifact>/scout.json`.
- Match scout finding by same `id`, else same `title`.
- Hydrate missing candidate fields from the scout finding:
  - `affected_files`
  - `evidence`
  - `commands`
  - `suggested_acceptance_criteria`
  - `suggested_verification`
  - `risk_guess`
  - `confidence`
  - `likely_agent_safe`
  - `finding_path`
  - `draft_issue_path`
- Candidate field mapping:
  - `affected_files` from hydrated `affected_files`
  - `evidence` from hydrated `evidence`
  - `acceptance_criteria` from hydrated `suggested_acceptance_criteria`
  - `verification` from hydrated `suggested_verification`
  - `source_finding_path` from gate `source_finding_path`, else hydrated `finding_path`
  - `source_draft_issue_path` from gate `source_draft_issue_path`, else hydrated `draft_issue_path`

README warning candidate expected result:
- `source_gate_status: warn`
- `suggested_blueprint_phase: needs:human`
- labels include:
  - `type:docs`
  - `risk:low`
  - `needs:human-review`
- labels do not include:
  - `agent:ready`
- `affected_files` includes `README.md`
- `evidence` non-empty
- `acceptance_criteria` non-empty
- `verification` non-empty
- `hydration_used: true`
- `hydration_match_method: id`

3. `gah price-guard --watchlist <FILE> --model <MODEL_ID>`
- Reads local JSON watchlist only.
- If model price is within max and status does not include `unavailable`, print `allowed` and exit 0.
- If model status includes `unavailable`, print `blocked` and exit nonzero.
- If model price exceeds max input/output, print `blocked` and exit nonzero.
- No provider calls.

4. `gah policy-check --config <TOML> --action <ACTION>`
- Reads repo policy TOML.
- For `trust_mode = "read_only"`:
  - block `open-draft-pr`
  - block issue/project mutation
  - print `blocked`
  - exit nonzero
- For `trust_mode = "draft_pr_allowed"`:
  - allow `open-draft-pr` only when `allow_draft_pr = true`, `allow_push = true`, and `allow_provider_mutation = true`
  - block `edit-issue` unless explicit issue write is allowed
  - print `allowed` or `blocked`
- Do not perform the action. This is only a policy check.

Implementation guidance:
- Use `clap` derive for CLI.
- Use `serde` structs/enums for artifacts.
- Use `anyhow` for CLI errors.
- Keep artifact schemas simple and test-compatible.
- Use `serde_json::Value` only where flexible fixture parsing is easier.
- Prefer explicit structs for output candidate JSON.
- Use unique artifact dir creation with create-dir failure/retry, not overwrite.

Validation:
Run:
`cargo fmt`
`cargo test`

Report:
- files changed
- tests passing/failing
- any intentional limitations

Stop when tests pass. Do not add provider integrations, workers, web UI, schedulers, or background daemons.
