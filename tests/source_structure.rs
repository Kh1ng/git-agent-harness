use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;

const DEFAULT_MAX_LINES: usize = 1500;

#[test]
fn runner_adapter_facade_preserves_public_call_paths() {
    fn public<T>(_item: T) {}

    public(git_agent_harness::runner::run_agy);
    public(git_agent_harness::runner::run_agy_with_executable);
    public(git_agent_harness::runner::run_claude);
    public(git_agent_harness::runner::run_claude_with_executable);
    public(git_agent_harness::runner::run_codex);
    public(git_agent_harness::runner::run_codex_with_executable);
    public(git_agent_harness::runner::extract_model_from_args);
    public(git_agent_harness::runner::extract_model_from_backend_args);
    public(git_agent_harness::runner::filtered_backend_args);
    public(git_agent_harness::runner::list_oh_profiles);
    public(git_agent_harness::runner::load_oh_profile);
    public(git_agent_harness::runner::run_openhands);
    public(git_agent_harness::runner::run_opencode);
    public(git_agent_harness::runner::run_opencode_with_executable);
    public(git_agent_harness::runner::run_vibe);
    public(git_agent_harness::runner::run_vibe_with_executable);
}

#[test]
fn dispatch_facade_preserves_public_call_paths_and_final_layout() {
    fn public<T>(_item: T) {}

    public(git_agent_harness::dispatch::run);
    public(git_agent_harness::dispatch::review_budget_exhausted_error);
    public(git_agent_harness::dispatch::review_preflight);
    public(git_agent_harness::dispatch::merge_branch);
    public(git_agent_harness::dispatch::scan_available_tickets);
    public(git_agent_harness::dispatch::self_check_validation_gate);
    let _ = std::mem::size_of::<git_agent_harness::dispatch::DispatchArgs>();
    let _ = std::mem::size_of::<git_agent_harness::dispatch::ValidationGateError>();

    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let facade = repo_root.join("src/dispatch/mod.rs");
    assert!(facade.is_file());
    assert!(!repo_root.join("src/dispatch.rs").exists());
    assert!(
        count_lines(&facade) <= 300,
        "dispatch facade must remain a thin, stable public boundary"
    );
}

#[derive(Debug, Deserialize)]
struct RustSourceBaseline {
    #[serde(default = "default_max_lines")]
    threshold: usize,
    #[serde(default)]
    files: BTreeMap<String, usize>,
}

#[derive(Debug, PartialEq, Eq)]
enum SizeViolation {
    TrackedGrew {
        path: String,
        observed: usize,
        ceiling: usize,
    },
    TrackedShrank {
        path: String,
        observed: usize,
        ceiling: usize,
    },
    UntrackedTooLarge {
        path: String,
        observed: usize,
        threshold: usize,
    },
}

fn default_max_lines() -> usize {
    DEFAULT_MAX_LINES
}

#[test]
fn rust_source_files_do_not_grow_past_baseline() {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let baseline_path = repo_root.join("config/rust-source-size-baseline.toml");
    let baseline = read_baseline(&baseline_path);

    let mut tracked_files = tracked_rust_files(&repo_root);
    tracked_files.sort();

    let mut size_violations = Vec::new();
    let mut stale_baseline = Vec::new();

    for path in &tracked_files {
        if is_excluded(path) {
            continue;
        }

        let line_count = count_lines(&repo_root.join(path));
        let path_string = path.to_string_lossy().to_string();
        if let Some(violation) = classify_size(
            &path_string,
            line_count,
            baseline.files.get(&path_string).copied(),
            baseline.threshold,
        ) {
            size_violations.push(violation);
        }
    }

    let tracked_set = tracked_files.into_iter().collect::<BTreeSet<_>>();
    for (path, _) in baseline
        .files
        .iter()
        .filter(|(path, _)| !tracked_set.contains(&PathBuf::from(path.as_str())))
    {
        stale_baseline.push(path.clone());
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

    let mut failures = size_violations
        .iter()
        .map(render_size_violation)
        .collect::<Vec<_>>();

    if let Some((base_ref, base_baseline)) = comparison_baseline(&repo_root) {
        failures.extend(baseline_relaxations(&base_baseline, &baseline).into_iter().map(
            |failure| {
                format!(
                    "Source-size ratchet was relaxed relative to {base_ref}: {failure}. Reduce or split the source file instead."
                )
            },
        ));
    }

    assert!(
        failures.is_empty(),
        "Rust source-size guard failed:\n{}",
        failures.join("\n")
    );
}

fn read_baseline(path: &Path) -> RustSourceBaseline {
    let text = fs::read_to_string(path)
        .unwrap_or_else(|err| panic!("failed to read baseline file {}: {err}", path.display()));
    toml::from_str(&text)
        .unwrap_or_else(|err| panic!("invalid baseline file {}: {err}", path.display()))
}

fn classify_size(
    path: &str,
    observed: usize,
    tracked_ceiling: Option<usize>,
    threshold: usize,
) -> Option<SizeViolation> {
    match tracked_ceiling {
        Some(ceiling) if observed > ceiling => Some(SizeViolation::TrackedGrew {
            path: path.to_string(),
            observed,
            ceiling,
        }),
        Some(ceiling) if observed < ceiling => Some(SizeViolation::TrackedShrank {
            path: path.to_string(),
            observed,
            ceiling,
        }),
        None if observed > threshold => Some(SizeViolation::UntrackedTooLarge {
            path: path.to_string(),
            observed,
            threshold,
        }),
        _ => None,
    }
}

fn render_size_violation(violation: &SizeViolation) -> String {
    match violation {
        SizeViolation::TrackedGrew {
            path,
            observed,
            ceiling,
        } => format!(
            "{path} grew to {observed} lines; its exact baseline is {ceiling}. Reduce or split the file; increasing the baseline is forbidden."
        ),
        SizeViolation::TrackedShrank {
            path,
            observed,
            ceiling,
        } => format!(
            "{path} shrank to {observed} lines; its stale baseline is {ceiling}. Lower the baseline to {observed} in this change."
        ),
        SizeViolation::UntrackedTooLarge {
            path,
            observed,
            threshold,
        } => format!(
            "{path} has {observed} lines and is not baseline-tracked; the limit is {threshold}. Split the file instead of adding a baseline entry."
        ),
    }
}

fn comparison_baseline(repo_root: &Path) -> Option<(String, RustSourceBaseline)> {
    let base_ref = std::env::var("GITHUB_BASE_REF")
        .ok()
        .filter(|value| !value.is_empty())
        .map(|value| format!("origin/{value}"))
        .or_else(|| {
            let head = git_output(repo_root, &["rev-parse", "HEAD"])?;
            let origin_main = git_output(repo_root, &["rev-parse", "origin/main"])?;
            (head != origin_main).then_some("origin/main".to_string())
        })?;

    let object = format!("{base_ref}:config/rust-source-size-baseline.toml");
    let Some(text) = git_output(repo_root, &["show", &object]) else {
        assert!(
            std::env::var_os("CI").is_none(),
            "CI could not read {object}; checkout history must include the pull request base"
        );
        return None;
    };
    let baseline = toml::from_str(&text)
        .unwrap_or_else(|err| panic!("invalid source-size baseline at {object}: {err}"));
    Some((base_ref, baseline))
}

fn git_output(repo_root: &Path, args: &[&str]) -> Option<String> {
    let output = Command::new("git")
        .current_dir(repo_root)
        .args(args)
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn baseline_relaxations(base: &RustSourceBaseline, current: &RustSourceBaseline) -> Vec<String> {
    let mut failures = Vec::new();
    if current.threshold > base.threshold {
        failures.push(format!(
            "global threshold increased from {} to {}",
            base.threshold, current.threshold
        ));
    }
    for (path, &ceiling) in &current.files {
        match base.files.get(path) {
            Some(&base_ceiling) if ceiling > base_ceiling => failures.push(format!(
                "{path} baseline increased from {base_ceiling} to {ceiling}"
            )),
            None => failures.push(format!(
                "{path} was added as a new baseline exception at {ceiling} lines"
            )),
            _ => {}
        }
    }
    failures
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

#[test]
fn tracked_file_below_its_exact_ceiling_fails() {
    assert_eq!(
        classify_size("src/large.rs", 99, Some(100), DEFAULT_MAX_LINES),
        Some(SizeViolation::TrackedShrank {
            path: "src/large.rs".to_string(),
            observed: 99,
            ceiling: 100,
        })
    );
}

#[test]
fn tracked_file_above_its_exact_ceiling_fails() {
    assert_eq!(
        classify_size("src/large.rs", 101, Some(100), DEFAULT_MAX_LINES),
        Some(SizeViolation::TrackedGrew {
            path: "src/large.rs".to_string(),
            observed: 101,
            ceiling: 100,
        })
    );
}

#[test]
fn tracked_file_at_its_exact_ceiling_passes() {
    assert_eq!(
        classify_size("src/large.rs", 100, Some(100), DEFAULT_MAX_LINES),
        None
    );
}

#[test]
fn untracked_file_at_or_below_the_global_threshold_passes() {
    assert_eq!(
        classify_size("src/bounded.rs", DEFAULT_MAX_LINES, None, DEFAULT_MAX_LINES),
        None
    );
}

#[test]
fn baseline_values_cannot_be_increased_or_added() {
    let base = RustSourceBaseline {
        threshold: 1500,
        files: BTreeMap::from([("src/existing.rs".to_string(), 2000)]),
    };
    let relaxed = RustSourceBaseline {
        threshold: 1501,
        files: BTreeMap::from([
            ("src/existing.rs".to_string(), 2001),
            ("src/new.rs".to_string(), 1600),
        ]),
    };

    assert_eq!(baseline_relaxations(&base, &relaxed).len(), 3);
}

#[test]
fn lowering_a_baseline_preserves_the_ratchet() {
    let base = RustSourceBaseline {
        threshold: 1500,
        files: BTreeMap::from([("src/existing.rs".to_string(), 2000)]),
    };
    let tightened = RustSourceBaseline {
        threshold: 1400,
        files: BTreeMap::from([("src/existing.rs".to_string(), 1900)]),
    };

    assert!(baseline_relaxations(&base, &tightened).is_empty());
}
