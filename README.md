# git-agent-harness (GAH)

A local-first CLI control plane for git agents. GAH provides deterministic,
read-only repo inventory, scouting, candidate backlog generation, and safe
local patch execution. All operations are file-based and do not require
external providers unless explicitly configured.

This repository contains the Rust binary (`gah`) that serves as the core
control plane. The full GAH loop-router system (PM, dev, review, and
experiment modes) lives in the `/root/agent-lab/` workspace alongside
Hermes agent profiles, configuration policies, and orchestrator scripts.

## Prerequisites

- [Rust](https://rustup.rs/) toolchain (cargo 1.96+ / rustc 1.96+)
- Git

Optional:

- [GitHub CLI](https://cli.github.com/) (`gh`) for GitHub integration
- [Hermes Agent](https://hermes-agent.nousresearch.com) for the review backend

## Install

```bash
cargo build --release
```

The binary is produced at `target/release/gah`. Symlink it to your PATH:

```bash
ln -sf "$(pwd)/target/release/gah" ~/agent-lab/bin/gah
```

## Quick Start

```bash
# Show available commands
gah --help

# Generate backlog candidates from a scout artifact
gah candidates --gate-artifact <path> --out-root <output-dir>

# Check whether a model is allowed by the price guard
gah price-guard --watchlist <file> --model <model-id>

# Evaluate whether a provider action is permitted by policy
gah policy-check --config <toml> --action <action>
```

## Test

```bash
cargo test
```

The test suite runs against local fixtures in `tests/fixtures/`. No network
access is required.

## Build

```bash
cargo build --release
```

## Project Structure

```
src/
  main.rs              CLI entry point and command routing
  models.rs            Shared data types (Gate, Scout, Candidate, Policy)
  candidates.rs        Candidate generation and hydration from scout data
  price_guard.rs       Model price and availability guard logic
  policy.rs            Trust-mode and provider mutation policy evaluation
tests/
  gah_cli.rs           Integration tests using local fixtures
  fixtures/            JSON and TOML test fixtures
```

## The GAH Loop-Router System

GAH is the control plane for a four-mode loop system that automates repo
analysis, task execution, and code review. The loop-router runs from
`/root/agent-lab/` and is composed of orchestrator scripts, a repo
registry, loop policies, review backends, and a provider mutation gate.

The four loop modes are:

| Mode       | Purpose                                                    | Patches Code | Creates MR |
|------------|------------------------------------------------------------|-------------|------------|
| **pm**     | Assess repo maturity, prioritize next work                 | No          | Planning MR |
| **dev**    | Execute one bounded code, config, or documentation change  | Yes         | Patch MR   |
| **review** | Sandboxed pre-human review of an MR diff                   | No          | Review report |
| **experiment** | Run and evaluate ML, data, or model hypotheses        | No          | Experiment report |

All loops follow a strict input-ref / output-branch discipline:

- The agent may read `input_ref` (for example, `main` or `world-cup-adaptation`).
- The agent may create `output_branch` from `input_ref`.
- The agent may commit only to `output_branch`.
- The agent may push only `output_branch`.
- The agent may open a draft MR from `output_branch` into `input_ref`.
- The agent may never modify `input_ref` directly.

### Loop Details

**PM loop:** Inspects a repository (source files, tests, CI, data
pipelines, documentation) and determines its current maturity stage. It
then identifies the highest-leverage bottleneck toward the project
objective and produces up to five ticket proposals for review. Output
is a draft planning MR containing the assessment, opportunity rankings,
and ticket queue.

**Dev loop:** Given a single approved ticket from the PM backlog, the
dev loop checks out the repo, creates a fresh output branch, implements
the scoped change, runs relevant tests, and produces a draft patch MR.
It never chooses its own work, never widens scope, and never touches
the input ref.

**Review loop:** Builds a minimal, safe review bundle (diff, changed
files, check summaries, MR description) and dispatches it through a
backend chain. The first backend that produces a meaningful structured
review is used. The review agent never modifies code, pushes branches,
or comments on MRs.

**Experiment loop:** Runs ML, data, or model hypotheses and produces
an experiment report. Does not patch code. Currently implemented as a
stub.

### Orchestrator Scripts

All scripts live under `/root/agent-lab/bin/`. The most important ones
are:

| Script                            | Purpose |
|-----------------------------------|---------|
| `gah-job`                         | Creates a normalized job packet from the repo registry |
| `gah-run-pm`                      | Runs the PM loop against a repo |
| `gah-run-dev`                     | Runs the dev loop (patch, verify, draft MR) |
| `gah-run-review`                  | Runs the review loop with backend chain |
| `gah-run-experiment`              | Runs the experiment loop (stub) |
| `github-inventory`                | Discovers and clones GitHub repos for an owner |
| `github-scout-loop`               | Runs `scout-repo` against all cloned repos |
| `github-backlog-from-scouts`      | Aggregates scout artifacts into a cross-repo backlog |
| `scout-repo`                      | Deterministic, read-only repo inspector |
| `gah-review-bundle`               | Builds a minimal safe review bundle |
| `gah-review-backend-claude`       | Claude + Ponytail review backend (strong independence) |
| `gah-review-backend-hermes-ponytail` | Hermes code-review profile (medium independence) |
| `gah-detect-checks`               | Detects test, lint, and CI commands in a repo |
| `gah-run-checks`                  | Runs the commands detected by `gah-detect-checks` |
| `gah-feedback`                    | Classifies and routes human feedback back into the loop system |
| `gah-mr-status`                   | Summarizes MR readiness with a structured status card |
| `gah-notify`                      | Sends notifications through the Hermes messaging adapter |
| `gah-validate-review-output`      | Validates that a review backend produced meaningful output |
| `gah-repo-config`                 | Validates repo-specific configuration settings |
| `provider-gate-check`             | Checks the provider mutation gate before any write action |
| `create-provider-approval`        | Creates a local human-approval file for provider actions |
| `promote-draft-pr`                | Pushes a branch and creates a draft PR after gate approval |
| `gh-readonly-guard`               | Wraps `gh` to block write commands (safety guardrail) |

### Configuration Files

| File                                                    | Purpose |
|---------------------------------------------------------|---------|
| `config/repo-registry.toml`                             | Repo catalog with provider, URL, objective, and allowed loops |
| `config/gah-loop-policy.toml`                           | Loop boundaries, quality gates, and retry policy |
| `config/review-backends.toml`                           | Review backend chain and independence levels |
| `config/provider-mutation-gate.toml`                    | Controls for provider write operations |
| `config/notification-policy.toml`                       | Notification routing through Hermes |
| `config/repos/worldcup-props.toml`                      | Repo-specific settings for the World Cup Props project |
| `config/policies/worldcup-props-pm-permissions.toml`    | PM agent issue and ticket permissions |

### Review Backend Chain

Reviews use a three-level backend chain with documented independence:

1. **claude_ponytail** (strong) — Claude Code CLI with the Ponytail
   code review skill. Requires `claude` on PATH and authentication.
2. **hermes_ponytail** (medium) — A dedicated Hermes Agent profile
   (`code-review`) with the Ponytail skill, strict read-only tools,
   and no access to Hermes global context.
3. **hermes_fallback** (weak) — Generic Hermes fallback. Policy
   forbids this backend from gating MR readiness.

Each backend receives a minimal review bundle that excludes HERMES.md,
secrets, tokens, provider credentials, and global agent-lab
configuration. The bundle is scanned for security issues before it is
passed to any reviewer.

### Provider Mutation Gate

Provider mutations (push, draft PR, issue creation, project edits) are
gated at multiple levels:

1. **Policy file** (`provider-mutation-gate.toml`) sets global booleans
   for each operation type.
2. **Runtime check** (`provider-gate-check`) evaluates the policy for a
   specific action, repo, and branch.
3. **Human approval** (`create-provider-approval`) writes a local
   approval file that the gate check requires.
4. **Promotion** (`promote-draft-pr`) runs the gate check, pushes the
   branch, and creates the draft PR -- but only if all checks pass.

All gate booleans default to `false`. No provider mutation occurs
without explicit, locally-recorded human approval.

## Notes

- GAH is a control plane, not an agent executor.
- Provider mutation is disabled by default.
- All GitHub operations are read-only unless explicitly configured otherwise.
- Review agents never modify code, push branches, or post MR comments.
- The Hermes `code-review` profile at `/root/.hermes/profiles/code-review/`
  is the dedicated review environment.
