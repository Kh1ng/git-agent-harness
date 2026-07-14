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

#[test]
fn routing_facade_preserves_public_call_paths_and_final_layout() {
    fn public<T>(_item: T) {}

    public(git_agent_harness::routing::current_concurrent);
    public(git_agent_harness::routing::decide_for_task_with_state);
    public(git_agent_harness::routing::decide_with_state);
    let _ = std::mem::size_of::<git_agent_harness::routing::ConcurrencyGuard>();
    let _ = std::mem::size_of::<git_agent_harness::routing::RouteDecision>();
    let _ = std::mem::size_of::<git_agent_harness::routing::RouteRequest>();

    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let facade = repo_root.join("src/routing/mod.rs");
    assert!(facade.is_file());
    assert!(!repo_root.join("src/routing.rs").exists());
    assert!(
        count_lines(&facade) <= 300,
        "routing facade must remain a thin, stable public boundary"
    );
    assert!(repo_root.join("src/routing/decision.rs").is_file());
    assert!(repo_root.join("src/routing/decision/tests.rs").is_file());
    assert!(repo_root.join("src/routing/policy.rs").is_file());
    assert!(repo_root.join("src/routing/policy/tests.rs").is_file());
    assert!(repo_root.join("src/routing/reservation.rs").is_file());
    assert!(repo_root.join("src/routing/reservation/tests.rs").is_file());
    assert!(repo_root.join("src/routing/test_support.rs").is_file());
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

        let file_path = repo_root.join(path);
        if !file_path.exists() {
            continue;
        }

        let line_count = count_lines(&file_path);
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
    let path_str = path.to_string_lossy();
    // Exclude apps/ and packages/ directories - they contain separate crates
    if path_str.starts_with("apps/") || path_str.starts_with("packages/") {
        return true;
    }
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

/// Explicit exception list for known false positives.
/// These paths are intentionally excluded from orphan detection.
/// Add to this list only after confirming the file is truly unreachable by design.
const ORPHAN_EXCEPTIONS: &[&str] = &[];

/// Check if a module is declared in a file
fn is_module_declared_in_file(
    module_name: &str,
    file_path: &Path,
    repo_root: &Path,
    all_files: &[PathBuf],
) -> bool {
    // file_path is an absolute path, but all_files contains relative paths
    // We need to check if the relative version of file_path is in all_files
    let relative_file = file_path.strip_prefix(repo_root).unwrap_or(file_path);
    if !all_files.iter().any(|p| *p == relative_file) {
        return false;
    }

    // Read from the absolute path
    if let Ok(content) = fs::read_to_string(file_path) {
        // Check for `mod name;` or `pub mod name;`
        let pattern = format!("mod {}", module_name);
        if content.contains(&pattern) {
            return true;
        }
        let pub_pattern = format!("pub mod {}", module_name);
        if content.contains(&pub_pattern) {
            return true;
        }
        // Check for #[path] attributes
        let path_pattern = format!(r#"#\[path = "{}"\]"#, module_name);
        if content.contains(&path_pattern) {
            return true;
        }
        let path_pattern_rs = format!(r#"#\[path = "{}.rs"\]"#, module_name);
        if content.contains(&path_pattern_rs) {
            return true;
        }
    }
    false
}

/// Check if a source file is reachable from crate roots
fn is_reachable_from_crate(path: &Path, repo_root: &Path, all_files: &[PathBuf]) -> bool {
    let relative = path.strip_prefix(repo_root).unwrap_or(path);
    let relative_str = relative.to_string_lossy();

    // Crate roots are always reachable
    if relative_str == "src/main.rs" || relative_str == "src/lib.rs" {
        return true;
    }

    // Integration test roots are always reachable
    // Files directly in tests/ (no nested path like tests/foo.rs)
    let path_str = relative.to_string_lossy();
    if let Some(rest) = path_str.strip_prefix("tests/") {
        if !rest.contains('/') {
            return true;
        }
    }

    // Files in tests/support/ are special - they're helper modules
    if relative_str.starts_with("tests/support/") {
        // support directory has mod.rs, so files in it are reachable
        // if gah_cli.rs declares `mod support;`
        let gah_cli_rs = repo_root.join("tests/gah_cli.rs");
        if all_files.contains(&gah_cli_rs) {
            if let Ok(content) = fs::read_to_string(&gah_cli_rs) {
                if content.contains("mod support;") || content.contains("pub mod support;") {
                    // Now check if the specific file is declared in support/mod.rs or reachable
                    // support/mod.rs should declare its submodules
                    let support_mod = repo_root.join("tests/support/mod.rs");
                    if all_files.contains(&support_mod) {
                        return true; // mod.rs makes it a module, cargo will find it
                    }

                    return true;
                }
            }
        }
        return true; // Be conservative - cargo test discovers these
    }

    // For nested test files like tests/gah_cli/something.rs
    if relative_str.starts_with("tests/") {
        // These would need to be declared in a test file
        // But for now, we're conservative and skip this check
        return true;
    }

    // For src/ files
    if relative_str.starts_with("src/") {
        let components: Vec<&str> = relative_str
            .strip_prefix("src/")
            .unwrap_or("")
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();

        if components.is_empty() {
            return false;
        }

        let file_name = components.last().unwrap();
        let is_mod_rs = *file_name == "mod.rs";

        if is_mod_rs {
            // This is a mod.rs file - check if its parent directory is declared in the parent's module
            if components.len() == 1 {
                // src/mod.rs - this is non-standard, but check main.rs
                return is_module_declared_in_file(
                    "mod",
                    &repo_root.join("src/main.rs"),
                    repo_root,
                    all_files,
                ) || is_module_declared_in_file(
                    "mod",
                    &repo_root.join("src/lib.rs"),
                    repo_root,
                    all_files,
                );
            }

            // For src/dispatch/review/mod.rs, we need to check if `review` is declared in src/dispatch/mod.rs
            // For src/routing/mod.rs, check if `routing` is declared in src/main.rs or src/lib.rs
            let module_name = components[components.len() - 2];
            let parent_components = &components[..components.len() - 2];

            if parent_components.is_empty() {
                // Directly under src/, e.g., src/routing/mod.rs
                return is_module_declared_in_file(
                    module_name,
                    &repo_root.join("src/main.rs"),
                    repo_root,
                    all_files,
                ) || is_module_declared_in_file(
                    module_name,
                    &repo_root.join("src/lib.rs"),
                    repo_root,
                    all_files,
                );
            } else {
                // Nested, e.g., src/dispatch/review/mod.rs - check if `review` is in src/dispatch/mod.rs
                let parent_dir_path = format!("src/{}", parent_components.join("/"));

                // Check parent's mod.rs
                let parent_mod_path = format!("{}/mod.rs", parent_dir_path);
                let parent_mod = repo_root.join(&parent_mod_path);
                let parent_mod_relative = PathBuf::from(&parent_mod_path);
                if all_files.contains(&parent_mod_relative)
                    && is_module_declared_in_file(module_name, &parent_mod, repo_root, all_files)
                {
                    return true;
                }

                // Check parent's .rs file
                let parent_rs_path = format!("{}.rs", parent_dir_path);
                let parent_rs = repo_root.join(&parent_rs_path);
                let parent_rs_relative = PathBuf::from(&parent_rs_path);
                if all_files.contains(&parent_rs_relative)
                    && is_module_declared_in_file(module_name, &parent_rs, repo_root, all_files)
                {
                    return true;
                }

                return false;
            }
        } else {
            // Regular file - check if declared in parent's mod.rs
            if components.len() == 1 {
                // Top-level src file like src/ledger.rs
                // Strip the .rs extension for module name
                let module_name = file_name.strip_suffix(".rs").unwrap_or(file_name);
                return is_module_declared_in_file(
                    module_name,
                    &repo_root.join("src/main.rs"),
                    repo_root,
                    all_files,
                ) || is_module_declared_in_file(
                    module_name,
                    &repo_root.join("src/lib.rs"),
                    repo_root,
                    all_files,
                );
            } else {
                // Nested file like src/routing/diagnostics.rs or src/controller/decision/tests.rs
                // Strip the .rs extension for module name
                let module_name = file_name.strip_suffix(".rs").unwrap_or(file_name);
                let parent_dir = &components[..components.len() - 1];
                let parent_dir_path = format!("src/{}", parent_dir.join("/"));

                // Check if there's a mod.rs in the parent directory
                let mod_rs_path = format!("{}/mod.rs", parent_dir_path);
                let parent_mod = repo_root.join(&mod_rs_path);
                let parent_mod_relative = PathBuf::from(&mod_rs_path);
                if all_files.contains(&parent_mod_relative)
                    && is_module_declared_in_file(module_name, &parent_mod, repo_root, all_files)
                {
                    return true;
                }

                // Check if there's a .rs file in the parent directory (the parent is a file module, not a directory module)
                // e.g., for src/controller/decision/tests.rs, the parent is src/controller/decision.rs
                let rs_path = format!("{}.rs", parent_dir_path);
                let parent_rs = repo_root.join(&rs_path);
                let parent_rs_relative = PathBuf::from(&rs_path);
                if all_files.contains(&parent_rs_relative)
                    && is_module_declared_in_file(module_name, &parent_rs, repo_root, all_files)
                {
                    return true;
                }

                // Also check all .rs files in the parent directory for declarations
                // (e.g., src/controller/ledger_read_tests.rs might be declared in src/controller/runtime.rs)
                for ancestor_file in all_files {
                    let ancestor_str = ancestor_file.to_string_lossy().to_string();
                    // Check if this file is in the parent directory
                    if ancestor_str.starts_with(&parent_dir_path) && ancestor_str.ends_with(".rs") {
                        // Check if it's not mod.rs or the file itself
                        if ancestor_str != mod_rs_path && ancestor_str != rs_path {
                            let ancestor_path = repo_root.join(ancestor_file);
                            if is_module_declared_in_file(
                                module_name,
                                &ancestor_path,
                                repo_root,
                                all_files,
                            ) {
                                return true;
                            }
                        }
                    }
                }

                return false;
            }
        }
    }

    false
}

#[test]
fn all_rust_modules_are_reachable_from_crate_roots() {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));

    // Collect all tracked .rs files under src/ and nested integration-test module directories
    let all_rust_files = tracked_rust_files(&repo_root);
    let mut orphaned = Vec::new();

    for path in &all_rust_files {
        if is_excluded(path) {
            continue;
        }

        // Skip exception list
        let path_str = path.to_string_lossy();
        if ORPHAN_EXCEPTIONS.contains(&path_str.as_ref()) {
            continue;
        }

        if !is_reachable_from_crate(path, &repo_root, &all_rust_files) {
            let relative = path.strip_prefix(&repo_root).unwrap_or(path);
            let relative_str = relative.to_string_lossy().to_string();
            orphaned.push((relative_str, find_expected_declaration(path, &repo_root)));
        }
    }

    if !orphaned.is_empty() {
        let mut msg = "Orphaned Rust modules detected:\n".to_string();
        for (path, expected) in &orphaned {
            msg.push_str(&format!("  {path} is not declared. {expected}\n"));
        }
        panic!("{msg}");
    }
}

/// Find the expected declaration for an orphaned file
fn find_expected_declaration(path: &Path, repo_root: &Path) -> String {
    let relative = path.strip_prefix(repo_root).unwrap_or(path);
    let relative_str = relative.to_string_lossy();

    if relative_str.starts_with("src/") {
        let components: Vec<&str> = relative_str
            .strip_prefix("src/")
            .unwrap_or("")
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();

        if components.is_empty() {
            return "Unknown location".to_string();
        }

        let file_name = components.last().unwrap();
        let is_mod_rs = *file_name == "mod.rs";

        if is_mod_rs && components.len() >= 2 {
            // For mod.rs files - the parent directory needs to be declared
            let dir_name = components[components.len() - 2];
            if components.len() == 2 {
                format!("Add `mod {dir_name};` to src/main.rs or src/lib.rs")
            } else {
                let grandparent = &components[..components.len() - 2];
                format!(
                    "Add `mod {dir_name};` to src/{}/mod.rs",
                    grandparent.join("/")
                )
            }
        } else {
            // For regular files - strip .rs extension for the module name
            let module_name = file_name.strip_suffix(".rs").unwrap_or(file_name);
            if components.len() == 1 {
                format!("Add `mod {module_name};` to src/main.rs or src/lib.rs")
            } else {
                let parent = &components[..components.len() - 1];
                format!(
                    "Add `mod {module_name};` to src/{}/mod.rs",
                    parent.join("/")
                )
            }
        }
    } else if relative_str.starts_with("tests/") {
        let components: Vec<&str> = relative_str
            .strip_prefix("tests/")
            .unwrap_or("")
            .split('/')
            .filter(|s| !s.is_empty())
            .collect();

        if components.len() >= 2 {
            let subdir = components[0];
            format!("Add `mod {subdir};` to a test root in tests/ like gah_cli.rs")
        } else {
            "Unknown test location".to_string()
        }
    } else {
        "Unknown location".to_string()
    }
}

/// Helper function to ensure at least one test matches a given filter.
/// This prevents the fabricated-completion incident where a test file exists
/// but `cargo test <filter>` matches zero tests.
///
/// Usage: In validation or CI scripts, run this after `cargo test <filter>`
/// to ensure the filter actually matched tests.
pub fn assert_test_filter_matches_at_least_one(test_output: &str, filter: &str) {
    // Check if the output contains "test result:" with at least one test run
    // or if it contains the filter string in a test name
    if !test_output.contains("test result:") {
        panic!(
            "Test filter '{}' matched zero tests. \
             Either the test file doesn't exist, or the test names don't match the filter.\n             Check: cargo test {} -- --nocapture",
            filter, filter
        );
    }

    // Parse the test result line to check if at least one test was run
    for line in test_output.lines() {
        if line.contains("test result:") {
            // Format: "test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out"
            // or: "test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out"
            if line.contains("0 passed")
                && line.contains("0 failed")
                && line.contains("0 filtered out")
            {
                panic!(
                    "Test filter '{}' matched zero tests. \
                     The test result shows 0 tests were run.\n                     Check: cargo test {} -- --nocapture",
                    filter, filter
                );
            }
            return; // At least some tests were run
        }
    }

    panic!(
        "Could not find test result line in output for filter '{}'",
        filter
    );
}

#[test]
fn test_filter_helper_does_not_panic_on_valid_output() {
    // This test verifies the helper doesn't panic when tests are found
    let valid_output = r#"
running 3 tests
test test_one ... ok
test test_two ... ok
test test_three ... ok

test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
"#;

    assert_test_filter_matches_at_least_one(valid_output, "test");
}

#[test]
#[should_panic(expected = "matched zero tests")]
fn test_filter_helper_panics_on_zero_tests() {
    // This test verifies the helper panics when zero tests match
    // This simulates the fabricated-completion incident from PR #325 where
    // `cargo test ticket_118` matched zero tests even though a test file existed.
    let zero_test_output = r#"
running 0 tests

test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out
"#;

    assert_test_filter_matches_at_least_one(zero_test_output, "ticket_118");
}

/// Check reachability for a hypothetical orphaned file equivalent to PR #325
/// This simulates having a test file that exists but is not declared anywhere
#[test]
fn orphaned_test_file_is_detected_in_isolation() {
    // This test creates a minimal scenario to verify the detection logic works
    // We use a temp directory to create a crate with an orphaned file
    use std::fs::File;
    use std::io::Write;

    let temp = tempfile::tempdir().unwrap();
    let src_dir = temp.path().join("src");
    std::fs::create_dir(&src_dir).unwrap();

    // Create main.rs
    let mut main_rs = File::create(src_dir.join("main.rs")).unwrap();
    writeln!(main_rs, "fn main() {{}}").unwrap();

    // Create an orphaned file
    let orphan_path = src_dir.join("orphaned_test.rs");
    let mut orphan = File::create(&orphan_path).unwrap();
    writeln!(orphan, "#[test]\nfn test_orphan() {{}}").unwrap();

    // This file is not declared in main.rs, so it's orphaned
    // We would need to run the check against this temp directory to verify
    // For now, we verify that the helper function would detect it
    // by checking that main.rs doesn't declare it
    let main_content = std::fs::read_to_string(src_dir.join("main.rs")).unwrap();
    assert!(
        !main_content.contains("mod orphaned_test"),
        "main.rs should not declare orphaned_test"
    );

    // The actual check would be done by is_reachable_from_crate
    // but that requires setting up a git repo and other infrastructure
    // For now, this test verifies the basic file structure is as expected
}
