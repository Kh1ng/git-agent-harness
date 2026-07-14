use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

const DEFAULT_MAX_LINES: usize = 1500;

#[derive(Debug, Deserialize)]
struct RustSourceBaseline {
    #[serde(default = "default_max_lines")]
    threshold: usize,
    #[serde(default)]
    files: BTreeMap<String, usize>,
}

fn default_max_lines() -> usize {
    DEFAULT_MAX_LINES
}

#[test]
fn rust_source_files_do_not_grow_past_baseline() {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let baseline_path = repo_root.join("config/rust-source-size-baseline.toml");
    let baseline: RustSourceBaseline = {
        let text = fs::read_to_string(&baseline_path).unwrap_or_else(|err| {
            panic!(
                "failed to read baseline file {}: {err}",
                baseline_path.display()
            )
        });
        toml::from_str(&text).unwrap_or_else(|err| {
            panic!("invalid baseline file {}: {err}", baseline_path.display())
        })
    };

    let mut tracked_files = tracked_rust_files(&repo_root);
    tracked_files.sort();

    let mut missing_baseline = Vec::new();
    let mut over_baseline = Vec::new();
    let mut stale_baseline = Vec::new();

    for path in &tracked_files {
        if is_excluded(path) {
            continue;
        }

        let line_count = count_lines(&repo_root.join(path));
        let path_string = path.to_string_lossy().to_string();
        match baseline.files.get(&path_string) {
            Some(&limit) => {
                if line_count > limit {
                    over_baseline.push((path_string, line_count, limit));
                }
            }
            None => {
                if line_count > baseline.threshold {
                    missing_baseline.push((path_string, line_count, baseline.threshold));
                }
            }
        }
    }

    let tracked_set = tracked_files.into_iter().collect::<BTreeSet<_>>();
    for (path, _) in baseline
        .files
        .into_iter()
        .filter(|(path, _)| !tracked_set.contains(&PathBuf::from(path.as_str())))
    {
        stale_baseline.push(path);
    }

    if !stale_baseline.is_empty() {
        eprintln!(
            "Stale rust source-size baseline entries remain in {}:",
            baseline_path.display()
        );
        for path in stale_baseline {
            eprintln!("  - {path}");
        }
        eprintln!("These entries are informative only and do not block the check.");
    }

    let mut failures = Vec::new();
    if !missing_baseline.is_empty() {
        failures.push(
            "New tracked Rust files exceed the configured threshold and are not in the baseline:"
                .to_string(),
        );
        for (path, observed, limit) in missing_baseline {
            failures.push(format!(
                "  - {path}: observed {observed} lines; limit is {limit}"
            ));
        }
    }

    if !over_baseline.is_empty() {
        failures
            .push("Baseline-tracked Rust files grew beyond their recorded ceiling:".to_string());
        for (path, observed, limit) in over_baseline {
            failures.push(format!(
                "  - {path}: observed {observed} lines; baseline ceiling is {limit}"
            ));
        }
    }

    assert!(
        failures.is_empty(),
        "Rust source-size guard failed:\n{}",
        failures.join("\n")
    );
}

fn tracked_rust_files(repo_root: &Path) -> Vec<PathBuf> {
    let output = Command::new("git")
        .current_dir(repo_root)
        .arg("ls-files")
        .arg("*.rs")
        .output()
        .unwrap_or_else(|err| panic!("failed to run git ls-files: {err}"));

    assert!(
        output.status.success(),
        "`git ls-files '*.rs'` failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(PathBuf::from)
        .collect()
}

fn is_excluded(path: &Path) -> bool {
    let mut previous_artifacts = false;
    for component in path.components() {
        if let Component::Normal(part) = component {
            let part = part.to_str().unwrap_or("");
            if matches!(part, "target" | "node_modules" | ".git") {
                return true;
            }
            if previous_artifacts && part == "worktrees" {
                return true;
            }
            previous_artifacts = part == "artifacts";
        }
    }
    false
}

fn count_lines(path: &Path) -> usize {
    let source = fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("failed to read {}: {err}", path.display()));
    source.lines().count()
}

#[test]
fn line_count_includes_a_final_line_without_a_trailing_newline() {
    let temp = tempfile::NamedTempFile::new().expect("create temporary Rust source");
    fs::write(temp.path(), "first\nsecond").expect("write temporary Rust source");

    assert_eq!(count_lines(temp.path()), 2);
}
