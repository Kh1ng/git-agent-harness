mod support;

use std::fs;
use std::path::Path;
use support::gah_command as bin;
use tempfile::TempDir;

fn profile_set(cfg_path: &Path, args: &[&str]) -> assert_cmd::assert::Assert {
    bin()
        .args(["profile", "set", "scaled"])
        .args(args)
        .args(["--config", cfg_path.to_str().unwrap()])
        .assert()
}

fn worker_scaling(cfg_path: &Path) -> serde_json::Value {
    profile(cfg_path)["worker_scaling"].clone()
}

fn profile(cfg_path: &Path) -> serde_json::Value {
    let output = bin()
        .args(["profile", "list", "--json", "--config"])
        .arg(cfg_path)
        .output()
        .unwrap();
    assert!(output.status.success());
    let profiles: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
    profiles[0].clone()
}

#[test]
fn profile_cli_sets_scaling_and_a_boost_then_clears_the_boost() {
    let home = TempDir::new().unwrap();
    let cfg_path = home.path().join("gah.toml");
    fs::write(&cfg_path, "[profiles]\n").unwrap();
    let dir = home.path().to_str().unwrap();
    bin()
        .args(["profile", "add", "scaled", "--display-name", "Scaled"])
        .args(["--repo-id", "scaled", "--provider", "github"])
        .args(["--repo", "owner/repo", "--local-path", dir])
        .args(["--artifact-root", dir, "--config"])
        .arg(&cfg_path)
        .assert()
        .success();

    let defaults = worker_scaling(&cfg_path);
    assert_eq!(defaults["enabled"], false);
    assert_eq!(defaults["extra_per_model"], 1);
    assert_eq!(defaults["min_remaining_percent"], 50.0);

    profile_set(
        &cfg_path,
        &[
            "--worker-scaling",
            "on",
            "--worker-scaling-max-workers",
            "6",
            "--worker-scaling-extra-per-model",
            "2",
            "--worker-scaling-min-remaining-percent",
            "40",
            "--boost-workers",
            "3",
            "--boost-model",
            "codex/gpt-5",
            "--boost-hours",
            "2",
        ],
    )
    .success();
    let scaling = worker_scaling(&cfg_path);
    assert_eq!(scaling["enabled"], true);
    assert_eq!(scaling["max_workers"], 6);
    assert_eq!(scaling["extra_per_model"], 2);
    assert_eq!(scaling["min_remaining_percent"], 40.0);
    assert_eq!(scaling["boost_workers"], 3);
    assert_eq!(scaling["boost_model"], "codex/gpt-5");
    assert!(scaling["boost_until"].as_str().unwrap().ends_with('Z'));

    profile_set(
        &cfg_path,
        &["--clear", "worker_boost,worker_scaling_max_workers"],
    )
    .success();
    let scaling = worker_scaling(&cfg_path);
    assert_eq!(scaling["enabled"], true);
    for cleared in ["max_workers", "boost_workers", "boost_model", "boost_until"] {
        assert!(scaling.get(cleared).is_none(), "{cleared} was not cleared");
    }

    profile_set(
        &cfg_path,
        &[
            "--max-concurrent",
            "codex/gpt-5=3",
            "--max-concurrent",
            "agy/Gemini 3.1 Pro (High)=2",
        ],
    )
    .success();
    profile_set(&cfg_path, &["--max-concurrent", "codex/gpt-5=1"]).success();
    assert_eq!(
        profile(&cfg_path)["max_concurrent_per_model"],
        serde_json::json!({ "codex/gpt-5": 1, "agy/Gemini 3.1 Pro (High)": 2 })
    );
    profile_set(&cfg_path, &["--max-concurrent", "codex=2"]).failure();
    profile_set(&cfg_path, &["--max-concurrent", "codex/gpt-5=0"]).failure();
    profile_set(&cfg_path, &["--clear", "max_concurrent_per_model"]).success();
    assert_eq!(
        profile(&cfg_path)["max_concurrent_per_model"],
        serde_json::json!({})
    );

    profile_set(&cfg_path, &["--worker-scaling", "maybe"]).failure();
    profile_set(
        &cfg_path,
        &["--worker-scaling-min-remaining-percent", "140"],
    )
    .failure();
    profile_set(&cfg_path, &["--boost-workers", "1", "--boost-hours", "0"]).failure();
    profile_set(&cfg_path, &["--boost-model", "codex/gpt-5"]).failure();
}
