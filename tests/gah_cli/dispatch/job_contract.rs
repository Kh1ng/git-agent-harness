use crate::*;

fn run_job(
    body: &str,
    script: &str,
    draft: bool,
    succeeds: bool,
    profile_commands: &str,
) -> (Value, String, String) {
    run_job_options(body, script, draft, succeeds, profile_commands, true, true)
}

fn run_job_options(
    body: &str,
    script: &str,
    draft: bool,
    succeeds: bool,
    profile_commands: &str,
    enforce: bool,
    local_md: bool,
) -> (Value, String, String) {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(&tmp, profile_commands);
    let job = tmp.path().join(if body.starts_with("# TICKET-1469:") {
        "TICKET-1469-job.md"
    } else {
        "job.md"
    });
    fs::write(&job, body).unwrap();
    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    let marker = tmp.path().join("invoked");
    let prompt = tmp.path().join("prompt");
    let counter = tmp.path().join("counter");
    make_fake_bin_with_body(&fake_bin, "codex", &format!(
        "#!/bin/sh\ntouch '{}'\nprintf '%s\\n' \"$@\" > '{}'\nn=$(cat '{}' 2>/dev/null || echo 0)\nn=$((n+1))\necho \"$n\" > '{}'\n{}\nexit 0\n",
        marker.display(), prompt.display(), counter.display(), counter.display(), script));
    let gh_log = tmp.path().join("gh.log");
    make_fake_bin_with_body(&fake_bin, "gh", &format!(
        "#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{}'\nif [ \"$1\" = pr ] && [ \"$2\" = create ]; then echo https://github.com/owner/real/pull/1; fi\nexit 0\n", gh_log.display()));
    let ledger = tmp.path().join("ledger.jsonl");
    let mut command = bin();
    command
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "fix",
            "--config-path",
            cfg.to_str().unwrap(),
            "--target",
            if local_md {
                job.to_str().unwrap()
            } else {
                "ordinary-target"
            },
            "--skip-validation-gate",
            "--retries",
            "2",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger);
    if enforce {
        command.arg("--enforce-job-file");
    }
    if draft {
        command.arg("--allow-draft-fail");
    }
    let output = command.output().unwrap();
    assert_eq!(
        output.status.success(),
        succeeds,
        "{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    if enforce && (body.contains("paragraph") || !local_md || !body.contains("## ")) {
        // A bad job file fails before the validation gate, the claim and the
        // ledger entry, and before the agent runs.
        assert!(!marker.exists());
        let error = String::from_utf8_lossy(&output.stderr);
        assert!(error.contains("Verification commands") || error.contains("--enforce-job-file"));
        let text = fs::read_to_string(&ledger).unwrap_or_default();
        assert!(
            !text.contains("\"mode\":\"fix\""),
            "no ledger entry expected: {text}"
        );
        return (Value::Null, String::new(), String::new());
    }
    let text = fs::read_to_string(&ledger).unwrap();
    let entry: Value = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|entry| entry["mode"] == "fix")
        .unwrap();
    let published = entry["push_succeeded"].as_bool().unwrap_or(false);
    assert_eq!(
        published,
        succeeds,
        "ledger: {entry}\nstdout: {}\nstderr: {}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let gh = fs::read_to_string(&gh_log).unwrap_or_default();
    assert_eq!(gh.contains("pr create"), succeeds);
    if let Some(branch) = entry["branch"].as_str() {
        assert_eq!(
            branch_exists_on_bare_origin(&tmp.path().join("github-root"), branch),
            succeeds
        );
    } else {
        assert!(!succeeds, "successful dispatch must record its branch");
    }
    let prompt_text = fs::read_to_string(prompt).unwrap_or_default();
    (entry, prompt_text, gh)
}

const PASS: &str = "validation_commands = [\"true\"]\n";

#[test]
fn job_contract_allowed_file_publishes() {
    run_job(
        "# Job\n## Allowed files\n- `README.md`",
        "echo change >> README.md",
        false,
        true,
        PASS,
    );
}

#[test]
fn job_contract_untracked_directory_is_rejected() {
    let (entry, _, _) = run_job(
        "## Allowed files\n- README.md",
        "echo change >> README.md\nmkdir -p outside\necho bad > outside/new.txt",
        false,
        false,
        PASS,
    );
    assert!(entry["error_summary"]
        .as_str()
        .unwrap()
        .contains("outside/new.txt"));
}

#[test]
fn job_contract_verification_retries_then_publishes() {
    let (entry, _, _) = run_job(
        "## Verification commands\n- `grep -q done marker.txt`",
        "if [ \"$n\" = 1 ]; then echo partial > marker.txt; else echo done > marker.txt; fi",
        false,
        true,
        PASS,
    );
    assert_eq!(entry["attempts_started"], 2);
    assert_eq!(entry["attempts_completed"], 2);
}

#[test]
fn job_contract_verification_never_passes() {
    run_job(
        "## Verification commands\n- `false`",
        "echo change >> README.md",
        false,
        false,
        PASS,
    );
}

#[test]
fn job_contract_draft_cannot_bypass_scope() {
    let (entry, _, _) = run_job(
        "## Allowed files\n- README.md",
        "echo change >> README.md\nmkdir -p outside\necho bad > outside/new.txt",
        true,
        false,
        PASS,
    );
    assert_eq!(entry["failure_class"], "validation_failure");
    assert_eq!(entry["failure_stage"], "post_validation");
    assert!(entry["error_summary"]
        .as_str()
        .unwrap()
        .contains("outside/new.txt"));
}

#[test]
fn job_contract_paragraph_fails_before_agent() {
    run_job(
        "## Verification commands\nparagraph",
        "echo change >> README.md",
        false,
        false,
        PASS,
    );
}

#[test]
fn job_contract_missing_sections_preserves_title() {
    let (_, _, gh) = run_job_options(
        "# TICKET-1469: Job Title\n\nOrdinary job",
        "echo change >> README.md",
        false,
        true,
        PASS,
        false,
        true,
    );
    assert!(gh.contains("TICKET-1469 Job Title"));
}

#[test]
fn job_contract_full_text_in_prompt() {
    let body = "# Local job\n\nDistinctive job instructions\n## Allowed files\n- README.md\n## Verification commands\n- true\n";
    let (_, prompt, _) = run_job(body, "echo change >> README.md", false, true, PASS);
    assert!(prompt.contains(body));
    assert!(prompt.contains("## Focus"));
    assert!(prompt.contains("## Job file"));
    assert!(prompt.contains("Allowed files and Verification commands sections are enforced"));
}

#[test]
fn job_contract_without_profile_commands_still_enforced() {
    run_job(
        "## Verification commands\n- false",
        "echo change >> README.md",
        false,
        false,
        "validation_commands = []\n",
    );
}

#[test]
fn job_contract_draft_cannot_bypass_commands() {
    let (entry, _, _) = run_job(
        "## Verification commands\n- false",
        "echo change >> README.md",
        true,
        false,
        PASS,
    );
    assert_eq!(entry["failure_stage"], "post_validation");
}

#[test]
fn job_contract_without_flag_preserves_publication_and_prompt() {
    let body = "# Distinctive unflagged instructions\n## Allowed files\n- only.txt\n## Verification commands\n- false";
    let (_, prompt, _) = run_job_options(
        body,
        "echo change >> README.md",
        false,
        true,
        PASS,
        false,
        true,
    );
    assert!(!prompt.contains("Distinctive unflagged instructions"));
    assert!(!prompt.contains("## Job file"));
}

#[test]
fn job_contract_flag_requires_sections_before_agent() {
    run_job_options(
        "# No contract",
        "echo change >> README.md",
        false,
        false,
        PASS,
        true,
        true,
    );
}

#[test]
fn job_contract_flag_requires_local_markdown_before_agent() {
    run_job_options(
        "# No contract",
        "echo change >> README.md",
        false,
        false,
        PASS,
        true,
        false,
    );
}

#[test]
fn job_contract_commands_run_once_on_success() {
    let tmp = test_tempdir();
    let runs = tmp.path().join("runs.txt");
    run_job(
        &format!(
            "## Verification commands\n- `echo run >> '{}'`",
            runs.display()
        ),
        "echo change >> README.md",
        false,
        true,
        PASS,
    );
    assert_eq!(fs::read_to_string(&runs).unwrap().lines().count(), 1);
}

#[test]
fn job_contract_dry_run_validates_the_flag() {
    let tmp = test_tempdir();
    let (_repo, home, cfg) = setup_fix_dispatch_repo(&tmp, PASS);
    let job = tmp.path().join("job.md");
    fs::write(&job, "# No contract").unwrap();
    let output = bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "fix",
            "--config-path",
            cfg.to_str().unwrap(),
            "--target",
            job.to_str().unwrap(),
            "--dry-run",
            "--enforce-job-file",
        ])
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", tmp.path().join("ledger.jsonl"))
        .output()
        .unwrap();
    assert!(!output.status.success());
    assert!(String::from_utf8_lossy(&output.stderr).contains("--enforce-job-file"));
}
