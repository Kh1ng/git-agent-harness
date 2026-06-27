# git-agent-harness (GAH)

Local-first CLI control plane for git agents. Provides deterministic, read-only
repo inventory, scouting, candidate backlog generation, and safe local patch
execution. All operations are local-file-based and do not require external
providers unless explicitly configured.

## Prerequisites

- [Rust](https://rustup.rs/) (cargo 1.96+ / rustc 1.96+)
- Git
- Optional: [GitHub CLI](https://cli.github.com/) (`gh`) for GitHub integration

## Install

```bash
cargo build --release
```

The binary `gah` is produced at `target/release/gah`.

## Quick Start

```bash
# Show available commands
gah --help

# Run candidates pipeline
gah candidates --gate-artifact <path> --out-root <output-dir>

# Check model price limits
gah price-guard --watchlist <file> --model <model-id>

# Evaluate policy
gah policy-check --config <toml> --action <action>
```

## Test

```bash
cargo test
```

Runs the test suite against local fixtures. No network access required.

## Build

```bash
cargo build --release
```

## Project Structure

```
src/
  main.rs          — CLI entry point and command routing
  models.rs        — shared data types (Gate, Scout, Candidate, Policy, etc.)
  candidates.rs    — candidate generation and hydration from scout data
  price_guard.rs   — model price/availability guard logic
  policy.rs        — trust-mode and provider mutation policy evaluation
tests/
  gah_cli.rs       — integration tests using local fixtures
  fixtures/        — JSON and TOML test fixtures
```

## Verification

```bash
cargo fmt --check
cargo test
```

## GAH Loop-Router System

GAH is the control plane for a four-mode loop system that automates
repo analysis, task execution, and review. The loop-router operates
from `/root/agent-lab/` and provides:

### Loop Modes

| Mode | Purpose | Patches Code | Creates MR/PR |
|------|---------|-------------|---------------|
| **pm** | Assess repo maturity, prioritize next move | No | Draft planning MR |
| **dev** | Execute one bounded code/doc/config change | Yes | Draft patch MR |
| **review** | Sandboxed pre-human review of MR diff | No | Review report only |
| **experiment** | Run/evaluate ML/data/model hypotheses | No | Draft experiment MR |

### Key Scripts

All scripts are under `/root/agent-lab/bin/`:

| Script | Purpose |
|--------|---------|
| `gah-job` | Create normalized job packet from repo registry |
| `gah-run-pm` | Run PM loop against a repo |
| `gah-run-dev` | Run dev loop (patch + verify + draft MR) |
| `gah-run-review` | Run review loop with backend chain |
| `gah-run-experiment` | Run experiment loop (stub) |
| `github-inventory` | Discover and clone GitHub repos |
| `github-scout-loop` | Run scout-repo against all cloned repos |
| `github-backlog-from-scouts` | Aggregate scout artifacts into backlog |
| `scout-repo` | Deterministic read-only repo inspector |
| `gah-review-bundle` | Build minimal safe review bundle |
| `gah-review-backend-claude` | Claude + Ponytail review backend (strong) |
| `gah-review-backend-hermes-ponytail` | Hermes code-review profile (medium) |
| `gah-review-backend-hermes` | Generic Hermes fallback (weak) |
| `gah-detect-checks` | Detect test/lint/CI commands in repo |
| `gah-run-checks` | Run discovered checks |
| `gah-feedback` | Classify and route human feedback |
| `gah-mr-status` | Summarize MR readiness |
| `gah-notify` | Send notifications via Hermes |
| `gah-validate-review-output` | Validate structured review output |
| `provider-gate-check` | Check provider mutation gate |
| `create-provider-approval` | Create local approval file |
| `promote-draft-pr` | Push branch and create draft PR |
| `gh-readonly-guard` | Block write gh commands |

### Config Files

| File | Purpose |
|------|---------|
| `/root/agent-lab/config/repo-registry.toml` | Repo catalog with provider, URL, objective, allowed loops |
| `/root/agent-lab/config/gah-loop-policy.toml` | Loop boundaries and quality gates |
| `/root/agent-lab/config/review-backends.toml` | Review backend chain config |
| `/root/agent-lab/config/provider-mutation-gate.toml` | Provider mutation gate controls |
| `/root/agent-lab/config/notification-policy.toml` | Notification routing through Hermes |

### Review Backend Chain

Reviews use a backend chain with independence levels:

1. **claude_ponytail** (strong) — Claude CLI with Ponytail code review skill
2. **hermes_ponytail** (medium) — Dedicated Hermes code-review profile with Ponytail
3. **hermes_fallback** (weak) — Generic Hermes fallback (not allowed to gate readiness)

Each backend receives a minimal safe review bundle that excludes HERMES.md,
secrets, tokens, provider credentials, and agent-lab config.

### Provider Mutation Gate

Provider mutations are gated at multiple levels:

- `provider-mutation-gate.toml` — Global policy (push, draft PR, issue, project)
- `provider-gate-check` — Runtime gate checker per action
- `create-provider-approval` — Human approval file creation
- `promote-draft-pr` — Push + draft PR with gate check

All gate booleans default to `false`. No provider mutation occurs without
explicit human approval.

## Notes

- This is the control plane, not an agent executor.
- Provider mutation is disabled by default.
- All GitHub operations are read-only unless explicitly configured otherwise.
- Review agents never modify code, push, or comment on MRs.
