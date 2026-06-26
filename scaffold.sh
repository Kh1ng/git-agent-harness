

#!/usr/bin/env bash
set -euo pipefail

# Scaffold the first test-driven Rust CLI version of git-agent-harness.
# Run from the repo root:
#   bash scaffold.sh
#
# This creates/overwrites the initial Cargo project files, fixtures, tests,
# and an OpenHands task prompt. It does not call GitHub/GitLab, models, or agents.

ROOT="${1:-.}"
cd "$ROOT"

mkdir -p src tests/fixtures docs

cat > Cargo.toml <<'EOF'
[package]
name = "git-agent-harness"
version = "0.1.0"
edition = "2021"

[[bin]]
name = "gah"
path = "src/main.rs"

[dependencies]
anyhow = "1"
camino = { version = "1", features = ["serde1"] }
clap = { version = "4", features = ["derive"] }
serde = { version = "1", features = ["derive"] }
serde_json = "1"
thiserror = "2"
time = { version = "0.3", features = ["formatting", "macros"] }
toml = "0.8"

[dev-dependencies]
assert_cmd = "2"
predicates = "3"
tempfile = "3"
EOF

cat > src/main.rs <<'EOF'
fn main() {
    eprintln!("gah is not implemented yet");
    std::process::exit(1);
}
EOF

cat > tests/fixtures/scout_readme_missing.json <<'EOF'
{
  "tool": "scout-repo",
  "mode": "draft-only",
  "repo_url": "https://github.com/Kh1ng/llm-chat",
  "local_path": "/tmp/gah-fixtures/llm-chat",
  "findings": [
    {
      "id": "001",
      "type": "docs",
      "title": "README is missing obvious setup/run/test sections",
      "risk_guess": "low",
      "confidence": "medium",
      "evidence": [
        "README headings do not include setup/install/usage/run/test sections"
      ],
      "affected_files": [
        "README.md"
      ],
      "commands": [],
      "suggested_acceptance_criteria": [
        "README includes setup instructions",
        "README includes run/usage instructions",
        "README includes test instructions"
      ],
      "suggested_verification": [
        "inspect README headings"
      ],
      "likely_agent_safe": true,
      "finding_path": "/tmp/gah/scout/draft-issues/001-readme.md",
      "draft_issue_path": "/tmp/gah/scout/draft-issues/001-readme.md"
    }
  ]
}
EOF

cat > tests/fixtures/gate_readme_warn_sparse.json <<'EOF'
{
  "tool": "scout-finding-gate",
  "mode": "draft-only",
  "source_scout_artifact": "__SCOUT_ARTIFACT__",
  "findings": [
    {
      "id": "001",
      "title": "README is missing obvious setup/run/test sections",
      "type": "docs",
      "gate_status": "warn",
      "hard_rejects": [],
      "warnings": [
        "no command",
        "README-only finding"
      ],
      "source_finding_path": "/tmp/gah/scout/draft-issues/001-readme.md",
      "source_draft_issue_path": "/tmp/gah/scout/draft-issues/001-readme.md",
      "approved_draft_issue_path": null,
      "rejected_finding_path": null
    }
  ]
}
EOF

cat > tests/fixtures/model_watchlist.json <<'EOF'
{
  "active_default_candidate": "deepseek/deepseek-v4-flash",
  "models": [
    {
      "id": "deepseek/deepseek-v4-flash",
      "status": "active_default_candidate",
      "input_per_1m": 0.09,
      "output_per_1m": 0.18,
      "max_input_per_1m": 0.20,
      "max_output_per_1m": 0.40
    },
    {
      "id": "qwen/qwen3-235b-a22b-2507",
      "status": "watch_unavailable_in_hermes",
      "input_per_1m": 0.09,
      "output_per_1m": 0.10,
      "max_input_per_1m": 0.20,
      "max_output_per_1m": 0.40
    }
  ]
}
EOF

cat > tests/gah_cli.rs <<'EOF'
use assert_cmd::Command;
use predicates::prelude::*;
use serde_json::Value;
use std::fs;
use tempfile::TempDir;

fn bin() -> Command {
    Command::cargo_bin("gah").unwrap()
}

fn write_fixture_dir() -> TempDir {
    let tmp = tempfile::tempdir().unwrap();

    let scout_dir = tmp.path().join("scout");
    fs::create_dir_all(&scout_dir).unwrap();

    let scout = include_str!("fixtures/scout_readme_missing.json");
    fs::write(scout_dir.join("scout.json"), scout).unwrap();

    let gate_dir = tmp.path().join("gate");
    fs::create_dir_all(&gate_dir).unwrap();

    let gate = include_str!("fixtures/gate_readme_warn_sparse.json")
        .replace("__SCOUT_ARTIFACT__", scout_dir.to_str().unwrap());
    fs::write(gate_dir.join("gate.json"), gate).unwrap();

    let watchlist = include_str!("fixtures/model_watchlist.json");
    fs::write(tmp.path().join("model_watchlist.json"), watchlist).unwrap();

    tmp
}

fn latest_child_dir(root: &std::path::Path) -> std::path::PathBuf {
    let mut dirs: Vec<_> = fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();
    dirs.sort();
    dirs.pop().unwrap()
}

#[test]
fn help_works() {
    bin()
        .arg("--help")
        .assert()
        .success()
        .stdout(predicate::str::contains("git agent harness"));
}

#[test]
fn warn_candidates_are_skipped_by_default() {
    let tmp = write_fixture_dir();
    let out_root = tmp.path().join("runs");
    let gate = tmp.path().join("gate");

    bin()
        .args([
            "candidates",
            "--gate-artifact",
            gate.to_str().unwrap(),
            "--out-root",
            out_root.to_str().unwrap(),
        ])
        .assert()
        .success();

    let artifact = latest_child_dir(&out_root.join("scout-to-backlog-candidates"));
    let data: Value =
        serde_json::from_str(&fs::read_to_string(artifact.join("candidates.json")).unwrap()).unwrap();

    assert_eq!(data["counts"]["seen"], 1);
    assert_eq!(data["counts"]["converted"], 0);
    assert_eq!(data["counts"]["skipped_warning"], 1);
    assert_eq!(data["candidates"].as_array().unwrap().len(), 0);
}

#[test]
fn warn_candidates_are_included_with_flag_and_hydrated_from_scout() {
    let tmp = write_fixture_dir();
    let out_root = tmp.path().join("runs");
    let gate = tmp.path().join("gate");

    bin()
        .args([
            "candidates",
            "--gate-artifact",
            gate.to_str().unwrap(),
            "--include-warnings",
            "--out-root",
            out_root.to_str().unwrap(),
        ])
        .assert()
        .success();

    let artifact = latest_child_dir(&out_root.join("scout-to-backlog-candidates"));
    let data: Value =
        serde_json::from_str(&fs::read_to_string(artifact.join("candidates.json")).unwrap()).unwrap();

    assert_eq!(data["counts"]["converted"], 1);
    let c = &data["candidates"][0];

    assert_eq!(c["candidate_id"], "001");
    assert_eq!(c["source_gate_status"], "warn");
    assert_eq!(c["suggested_blueprint_phase"], "needs:human");
    assert_eq!(c["provider_mutation_allowed"], false);

    let labels = c["suggested_labels"].as_array().unwrap();
    assert!(labels.iter().any(|v| v == "type:docs"));
    assert!(labels.iter().any(|v| v == "risk:low"));
    assert!(labels.iter().any(|v| v == "needs:human-review"));
    assert!(!labels.iter().any(|v| v == "agent:ready"));

    assert!(c["affected_files"].as_array().unwrap().iter().any(|v| v == "README.md"));
    assert!(!c["evidence"].as_array().unwrap().is_empty());
    assert!(!c["acceptance_criteria"].as_array().unwrap().is_empty());
    assert!(!c["verification"].as_array().unwrap().is_empty());

    assert_eq!(c["hydration_used"], true);
    assert_eq!(c["hydration_match_method"], "id");
}

#[test]
fn candidate_artifacts_are_unique_and_never_overwritten() {
    let tmp = write_fixture_dir();
    let out_root = tmp.path().join("runs");
    let gate = tmp.path().join("gate");

    for _ in 0..2 {
        bin()
            .args([
                "candidates",
                "--gate-artifact",
                gate.to_str().unwrap(),
                "--include-warnings",
                "--out-root",
                out_root.to_str().unwrap(),
            ])
            .assert()
            .success();
    }

    let root = out_root.join("scout-to-backlog-candidates");
    let dirs: Vec<_> = fs::read_dir(root)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_dir())
        .collect();

    assert_eq!(dirs.len(), 2);
    assert_ne!(dirs[0], dirs[1]);
}

#[test]
fn price_guard_allows_active_default() {
    let tmp = write_fixture_dir();
    let watchlist = tmp.path().join("model_watchlist.json");

    bin()
        .args([
            "price-guard",
            "--watchlist",
            watchlist.to_str().unwrap(),
            "--model",
            "deepseek/deepseek-v4-flash",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("allowed"));
}

#[test]
fn price_guard_blocks_unavailable_model() {
    let tmp = write_fixture_dir();
    let watchlist = tmp.path().join("model_watchlist.json");

    bin()
        .args([
            "price-guard",
            "--watchlist",
            watchlist.to_str().unwrap(),
            "--model",
            "qwen/qwen3-235b-a22b-2507",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("blocked"));
}

#[test]
fn work_trust_mode_blocks_provider_mutation() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("work-readonly.toml");

    fs::write(
        &cfg,
        r#"
[repo]
name = "work/private-repo"
provider = "github"
trust_mode = "read_only"
allow_provider_mutation = false
allow_push = false
allow_draft_pr = false
allow_issue_write = false
allow_project_write = false
"#,
    )
    .unwrap();

    bin()
        .args([
            "policy-check",
            "--config",
            cfg.to_str().unwrap(),
            "--action",
            "open-draft-pr",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("blocked"));
}

#[test]
fn personal_draft_pr_mode_allows_only_draft_pr() {
    let tmp = tempfile::tempdir().unwrap();
    let cfg = tmp.path().join("personal-draft.toml");

    fs::write(
        &cfg,
        r#"
[repo]
name = "personal/repo"
provider = "github"
trust_mode = "draft_pr_allowed"
allow_provider_mutation = true
allow_push = true
allow_draft_pr = true
allow_issue_write = false
allow_project_write = false
"#,
    )
    .unwrap();

    bin()
        .args([
            "policy-check",
            "--config",
            cfg.to_str().unwrap(),
            "--action",
            "open-draft-pr",
        ])
        .assert()
        .success()
        .stdout(predicate::str::contains("allowed"));

    bin()
        .args([
            "policy-check",
            "--config",
            cfg.to_str().unwrap(),
            "--action",
            "edit-issue",
        ])
        .assert()
        .failure()
        .stdout(predicate::str::contains("blocked"));
}
EOF

cat > OPENHANDS_TASK.md <<'EOF'
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
EOF

cat > README.md <<'EOF'
# git-agent-harness

Local-first CLI control plane for git agents.

This repo starts test-first. The initial MVP does not call GitHub, GitLab, paid models, OpenHands, or any external provider. It only reads local fixtures/artifacts and writes local artifacts.

## Bootstrap

```bash
bash scaffold.sh
cargo test
```

The initial `cargo test` run is expected to fail until `gah` is implemented.
EOF

echo "Scaffold written. Next commands:"
echo "  cargo test"
echo "  cat OPENHANDS_TASK.md"