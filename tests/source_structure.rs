use regex::Regex;
use serde::Deserialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

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

#[test]
fn main_rs_is_thin_wrapper_below_50_lines_and_has_no_module_list() {
    let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let main_rs = repo_root.join("src/main.rs");
    assert!(main_rs.is_file(), "src/main.rs must exist");

    let line_count = count_lines(&main_rs);
    assert!(
        line_count < 50,
        "src/main.rs must be below 50 lines (observed: {} lines)",
        line_count
    );

    let content = fs::read_to_string(&main_rs).expect("failed to read src/main.rs");
    let mod_decls = parse_mod_decls(&content);
    assert!(
        mod_decls.is_empty(),
        "src/main.rs must have no module list; modules belong in src/lib.rs (found module declarations: {:?})",
        mod_decls.iter().map(|d| &d.name).collect::<Vec<_>>()
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

/// A single `mod name;` (or `#[path = ...] mod name;`) declaration discovered
/// in a source file. `path_attr` is `Some` when the declaration is annotated
/// with `#[path = "..."]`, in which case that explicit path (resolved relative
/// to the declaring file) identifies the module's source file.
struct ModDecl {
    name: String,
    path_attr: Option<String>,
}

fn mod_decl_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // `mod name;` — a file/module declaration. Requires a trailing `;` so
        // that inline `mod name { ... }` blocks (which carry no separate source
        // file) are never treated as a file reference. Visibility prefixes
        // (`pub`, `pub(crate)`, `pub(super)`, ...) and preceding `#[...]`
        // attributes are tolerated and ignored.
        Regex::new(
            r"(?x)^\s*
            (?:pub\s*(?:\([^)]*\))?\s*)?
            mod\s+([A-Za-z_][A-Za-z0-9_]*)\s*;",
        )
        .unwrap()
    })
}

fn path_attr_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        // Matches `#[path = "rel/path.rs"]`. Intentionally NOT a raw string so
        // the bracket is a real regex escape rather than a literal backslash.
        Regex::new(r#"#\s*\[\s*path\s*=\s*"([^"]+)"\s*\]"#).unwrap()
    })
}

fn attr_token_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| Regex::new(r"#\s*\[[^\]]*\]").unwrap())
}

/// Remove line and nested block comments while preserving quoted strings.
///
/// This is deliberately a narrow lexical pass, not a Rust parser. It is enough
/// to prevent commented-out `mod` and `#[path]` text from being treated as live
/// declarations, including when a block comment spans several lines.
fn uncommented_rust_lines(content: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut block_depth = 0usize;

    for raw_line in content.lines() {
        let chars: Vec<char> = raw_line.chars().collect();
        let mut output = String::new();
        let mut index = 0usize;
        let mut in_string = false;
        let mut escaped = false;

        while index < chars.len() {
            let current = chars[index];
            let next = chars.get(index + 1).copied();

            if block_depth > 0 {
                if current == '/' && next == Some('*') {
                    block_depth += 1;
                    index += 2;
                } else if current == '*' && next == Some('/') {
                    block_depth -= 1;
                    index += 2;
                } else {
                    index += 1;
                }
                continue;
            }

            if in_string {
                output.push(current);
                if escaped {
                    escaped = false;
                } else if current == '\\' {
                    escaped = true;
                } else if current == '"' {
                    in_string = false;
                }
                index += 1;
                continue;
            }

            if current == '"' {
                in_string = true;
                output.push(current);
                index += 1;
            } else if current == '/' && next == Some('/') {
                break;
            } else if current == '/' && next == Some('*') {
                block_depth += 1;
                index += 2;
            } else {
                output.push(current);
                index += 1;
            }
        }

        lines.push(output);
    }

    lines
}

/// Parse the `mod` declarations out of a source file's content, with proper
/// word-boundary matching and comment/attribute awareness.
///
/// This is the core anti-fabrication routine: a declaration is only recognized
/// as an exact module name (`mod foo;` does not match a lookup for `foobar`),
/// commented-out declarations are ignored, and `#[path = ...]` attributes are
/// captured so the explicit file location can be resolved.
fn parse_mod_decls(content: &str) -> Vec<ModDecl> {
    let mut decls = Vec::new();
    let mut pending_path: Option<String> = None;
    let path_re = path_attr_re();
    let mod_re = mod_decl_re();
    let attr_re = attr_token_re();

    for uncommented_line in uncommented_rust_lines(content) {
        let raw_line = uncommented_line.as_str();
        // A `#[path = "..."]` attribute applies to the very next `mod` decl.
        if let Some(cap) = path_re.captures(raw_line) {
            pending_path = Some(cap.get(1).unwrap().as_str().to_string());
        }
        // Drop `#[...]` attribute tokens (e.g. `#[cfg(test)]`) so they don't
        // interfere with the `mod` regex. The `path` attribute was already
        // captured above.
        let stripped = attr_re.replace_all(raw_line, "");

        if let Some(cap) = mod_re.captures(&stripped) {
            let name = cap.get(1).unwrap().as_str().to_string();
            decls.push(ModDecl {
                name,
                path_attr: pending_path.take(),
            });
        } else {
            // A non-attribute, non-comment code line ends any pending path
            // attribute (it only ever applies to the immediately following decl).
            let trimmed = raw_line.trim();
            if !trimmed.starts_with('#') && !trimmed.is_empty() {
                pending_path = None;
            }
        }
    }
    decls
}

/// A crate root owns its own directory: its submodules live in the same
/// directory rather than a sibling `name/` directory.
///   * `src/main.rs` / `src/lib.rs` -> `src/`
///   * a direct integration test root `tests/foo.rs` -> `tests/`
fn is_crate_root(relative: &Path) -> bool {
    let s = relative.to_string_lossy();
    if s == "src/main.rs" || s == "src/lib.rs" {
        return true;
    }
    if let Some(rest) = s.strip_prefix("tests/") {
        return !rest.contains('/') && rest.ends_with(".rs");
    }
    false
}

/// The directory a submodule declared in `declaring_file` resolves within.
///
/// In Rust a *directory* module (`foo/mod.rs`) owns the `foo/` directory, while
/// a *file* module (`foo.rs`) owns the sibling `foo/` directory. Mixing these
/// up is exactly how orphaned files slip through, so we distinguish them by the
/// declaring file's own name, except for crate roots:
///   * `src/dispatch/mod.rs`  -> `src/dispatch/`
///   * `src/ledger.rs`        -> `src/ledger/`
///   * `src/main.rs`          -> `src/`          (crate root)
///   * `tests/gah_cli.rs`     -> `tests/`        (crate root)
fn module_dir(declaring_file: &Path, repo_root: &Path) -> PathBuf {
    let parent = declaring_file.parent().unwrap_or_else(|| Path::new("."));
    let relative = strip_repo_prefix(declaring_file, repo_root);
    if is_crate_root(&relative) {
        return parent.to_path_buf();
    }
    match declaring_file.file_stem().and_then(|s| s.to_str()) {
        Some("mod") => parent.to_path_buf(),
        Some(stem) => parent.join(stem),
        None => parent.to_path_buf(),
    }
}

/// Resolve a module declaration to the absolute source-file path it refers to,
/// or `None` if no tracked file corresponds to it.
fn resolve_module(
    declaring_file: &Path,
    decl: &ModDecl,
    repo_root: &Path,
    all_files: &[PathBuf],
) -> Option<PathBuf> {
    // `#[path = "..."]` is resolved relative to the directory of the source
    // file that carries the attribute (Rust's rule), independent of whether the
    // declaring module is a file module or a directory module.
    let file_dir = declaring_file.parent()?;

    // Candidate locations, in resolution priority (Rust prefers the file
    // module `<name>.rs` over the directory module `<name>/mod.rs`).
    let candidates: Vec<PathBuf> = if let Some(path_attr) = &decl.path_attr {
        let mut p = file_dir.join(path_attr);
        if p.extension().is_none() {
            p = p.with_extension("rs");
        }
        vec![p]
    } else {
        let mdir = module_dir(declaring_file, repo_root);
        vec![
            mdir.join(format!("{}.rs", decl.name)),
            mdir.join(decl.name.clone()).join("mod.rs"),
        ]
    };

    for cand in candidates {
        let rel = strip_repo_prefix(&cand, repo_root);
        if all_files.contains(&rel) {
            return Some(cand);
        }
    }
    None
}

fn strip_repo_prefix(path: &Path, repo_root: &Path) -> PathBuf {
    path.strip_prefix(repo_root).unwrap_or(path).to_path_buf()
}

/// Compute the set of source files reachable from every crate root by following
/// module declarations (`mod` + `#[path]`).
///
/// Crate roots:
///   * `src/main.rs` and `src/lib.rs` (the lib/bin crate)
///   * every direct `tests/*.rs` integration-test root
///
/// A `.rs` file nested under `tests/<subdir>/` is only reachable if some test
/// root declares the `<subdir>` module (e.g. `mod support;` in `tests/gah_cli.rs`)
/// and the file is then reachable through that subtree's `mod` declarations — the
/// exact PR #325 scenario (an unreferenced test file cargo silently ignores).
fn compute_reachable(repo_root: &Path, all_files: &[PathBuf]) -> BTreeSet<PathBuf> {
    let mut reachable: BTreeSet<PathBuf> = BTreeSet::new();
    let mut queue: Vec<PathBuf> = Vec::new();

    for root in [PathBuf::from("src/main.rs"), PathBuf::from("src/lib.rs")] {
        if all_files.contains(&root) {
            queue.push(repo_root.join(&root));
        }
    }

    for f in all_files {
        let s = f.to_string_lossy();
        if let Some(rest) = s.strip_prefix("tests/") {
            if !rest.contains('/') && rest.ends_with(".rs") {
                queue.push(repo_root.join(f));
            }
        }
    }

    while let Some(file) = queue.pop() {
        if !reachable.insert(file.clone()) {
            continue;
        }
        if let Ok(content) = fs::read_to_string(&file) {
            for decl in parse_mod_decls(&content) {
                if let Some(target) = resolve_module(&file, &decl, repo_root, all_files) {
                    queue.push(target);
                }
            }
        }
    }

    reachable
}

/// Check if a source file is reachable from a crate root.
///
/// `path` may be either absolute or relative to the repository; it is normalized
/// against `repo_root` before lookup so that callers passing git-tracked relative
/// paths (as the `all_rust_modules_are_reachable_from_crate_roots` test does) and
/// callers passing absolute fixture paths (as the isolation tests do) both work.
fn is_reachable_from_crate(path: &Path, repo_root: &Path, all_files: &[PathBuf]) -> bool {
    let abs = if path.is_absolute() {
        path.to_path_buf()
    } else {
        repo_root.join(path)
    };
    compute_reachable(repo_root, all_files).contains(&abs)
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
    static RESULT_RE: OnceLock<Regex> = OnceLock::new();
    let result_re = RESULT_RE.get_or_init(|| {
        Regex::new(
            r"test result:.*?([0-9]+) passed; ([0-9]+) failed; ([0-9]+) ignored; ([0-9]+) measured;",
        )
        .unwrap()
    });

    let matched = result_re.captures_iter(test_output).any(|captures| {
        (1..=4).any(|index| captures[index].parse::<u64>().is_ok_and(|count| count > 0))
    });

    assert!(
        matched,
        "Test filter '{}' matched zero tests. Either the test file doesn't exist, \
         or the test names don't match the filter.\nCheck: cargo test {} -- --nocapture",
        filter, filter
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

#[test]
fn test_filter_helper_accepts_a_later_matching_test_binary() {
    let workspace_output = r#"
running 0 tests
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 12 filtered out
running 1 test
test ticket_118_is_wired ... ok
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 4 filtered out
"#;

    assert_test_filter_matches_at_least_one(workspace_output, "ticket_118");
}

/// PR #325 scenario: an unreferenced integration-test file nested under a
/// `tests/` subdirectory must be flagged as orphaned. Cargo silently ignores
/// such files — a test root (`tests/*.rs`) only compiles a subdirectory when
/// it declares it as a module.
#[test]
fn orphaned_nested_test_file_is_detected() {
    use std::fs::File;
    use std::io::Write;

    let temp = tempfile::tempdir().unwrap();
    let repo_root = temp.path();
    let tests_dir = repo_root.join("tests");
    std::fs::create_dir(&tests_dir).unwrap();

    // A real test root that declares nothing about the orphaned subtree.
    let mut gah_cli = File::create(tests_dir.join("gah_cli.rs")).unwrap();
    writeln!(gah_cli, "fn smoke() {{}}").unwrap();

    // An undeclared nested test file — the exact cargo-ignored shape.
    let orphan_dir = tests_dir.join("orphaned");
    std::fs::create_dir(&orphan_dir).unwrap();
    let mut orphan = File::create(orphan_dir.join("foo.rs")).unwrap();
    writeln!(orphan, "#[test]\nfn test_orphan() {{}}").unwrap();

    let all_files = vec![
        PathBuf::from("tests/gah_cli.rs"),
        PathBuf::from("tests/orphaned/foo.rs"),
    ];

    assert!(
        !is_reachable_from_crate(&orphan_dir.join("foo.rs"), repo_root, &all_files),
        "an undeclared nested test file must be flagged as orphaned (PR #325)"
    );
}

/// PR #378 scenario: a `src/` module that no crate root declares must be
/// flagged as orphaned, while a properly declared sibling module stays reachable.
#[test]
fn undeclared_src_module_is_detected() {
    use std::fs::File;
    use std::io::Write;

    let temp = tempfile::tempdir().unwrap();
    let repo_root = temp.path();
    let src_dir = repo_root.join("src");
    std::fs::create_dir(&src_dir).unwrap();
    std::fs::create_dir(src_dir.join("routing")).unwrap();

    let mut main_rs = File::create(src_dir.join("main.rs")).unwrap();
    writeln!(main_rs, "fn main() {{}}").unwrap();
    writeln!(main_rs, "mod config;").unwrap();

    let mut config = File::create(src_dir.join("config.rs")).unwrap();
    writeln!(config, "// declared").unwrap();

    let mut diagnostics = File::create(src_dir.join("routing/diagnostics.rs")).unwrap();
    writeln!(diagnostics, "// undeclared").unwrap();

    let all_files = vec![
        PathBuf::from("src/main.rs"),
        PathBuf::from("src/config.rs"),
        PathBuf::from("src/routing/diagnostics.rs"),
    ];

    assert!(
        is_reachable_from_crate(&src_dir.join("config.rs"), repo_root, &all_files),
        "a properly declared src module must be reachable"
    );
    assert!(
        !is_reachable_from_crate(
            &src_dir.join("routing/diagnostics.rs"),
            repo_root,
            &all_files
        ),
        "an undeclared src submodule must be flagged as orphaned (PR #378)"
    );
}

/// A module reachable through a `tests/` root declaring a subdirectory (as
/// `gah_cli.rs` does for `support`) must NOT be flagged as orphaned.
#[test]
fn declared_nested_test_module_is_reachable() {
    use std::fs::File;
    use std::io::Write;

    let temp = tempfile::tempdir().unwrap();
    let repo_root = temp.path();
    let tests_dir = repo_root.join("tests");
    std::fs::create_dir(&tests_dir).unwrap();
    std::fs::create_dir(tests_dir.join("support")).unwrap();

    let mut gah_cli = File::create(tests_dir.join("gah_cli.rs")).unwrap();
    writeln!(gah_cli, "mod support;").unwrap();

    let mut support_mod = File::create(tests_dir.join("support/mod.rs")).unwrap();
    writeln!(support_mod, "pub mod fake_ledger;").unwrap();

    let mut fake_ledger = File::create(tests_dir.join("support/fake_ledger.rs")).unwrap();
    writeln!(fake_ledger, "// support helper").unwrap();

    let all_files = vec![
        PathBuf::from("tests/gah_cli.rs"),
        PathBuf::from("tests/support/mod.rs"),
        PathBuf::from("tests/support/fake_ledger.rs"),
    ];

    assert!(
        is_reachable_from_crate(
            &tests_dir.join("support/fake_ledger.rs"),
            repo_root,
            &all_files
        ),
        "a test module declared through a test-root subtree must be reachable"
    );
}

/// `mod foobar;` must not satisfy a lookup for the module `foo`: the declaration
/// parser matches exact module names via word boundaries, so prefix substrings
/// do not produce false "declared" results (anti-fabrication guarantee).
#[test]
fn module_declaration_match_is_not_a_prefix_substring() {
    let content = "mod foo;\n\
        // mod bar;\n\
        mod foobar;\n\
        /* mod baz; */\n\
        /*\n\
        mod hidden_across_lines;\n\
        #[path = \"wrong.rs\"]\n\
        */\n\
        pub mod qux;\n\
        mod quux { }\n";
    let decls = parse_mod_decls(content);
    let names: Vec<&str> = decls.iter().map(|d| d.name.as_str()).collect();

    assert_eq!(
        names,
        vec!["foo", "foobar", "qux"],
        "only live file-module declarations should be parsed, as exact names"
    );
    assert!(
        decls.iter().all(|decl| decl.path_attr.is_none()),
        "a commented path attribute must not leak onto the next live module"
    );
}

/// `#[path = \"...\"]` attributes are recognized and bound to the following
/// `mod` declaration, so explicit file locations are honored by resolution.
#[test]
fn module_declaration_recognizes_path_attribute() {
    let content = "#[cfg(test)]\n#[path = \"decision/tests.rs\"]\nmod tests;\n";
    let decls = parse_mod_decls(content);

    assert_eq!(decls.len(), 1);
    assert_eq!(decls[0].name, "tests");
    assert_eq!(
        decls[0].path_attr.as_deref(),
        Some("decision/tests.rs"),
        "the #[path] attribute must be captured for the following mod decl"
    );
}
