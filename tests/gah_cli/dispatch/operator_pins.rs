use crate::*;

#[test]
fn explicit_backend_unavailable_never_launches_fallback() {
    use git_agent_harness::availability::{record_unavailable, Reason, Source};
    for mode in ["review", "improve"] {
        let tmp = test_tempdir();
        let repo = tmp.path().join("repo");
        fs::create_dir_all(&repo).unwrap();
        init_git_repo(&repo);
        add_origin_and_feature_commit(&repo);
        let cfg = write_real_repo_config_with_extra(
            &tmp, &repo, "github",
            "[profiles.real.routing]\nallow_review_fallback = true\nallow_implementation_fallback = true\nreview_candidates = [{ backend = \"vibe\", model = \"mistral-medium-3.5\" }, { backend = \"codex\" }]\nimprove_candidates = [{ backend = \"vibe\", model = \"mistral-medium-3.5\" }, { backend = \"codex\" }]\n",
            "",
        );
        let state = tmp.path().join("availability.json");
        record_unavailable(
            &state,
            "vibe",
            Some("mistral-medium-3.5"),
            None,
            Reason::BackendOutage,
            Source::BackendError,
            None,
            None,
            time::OffsetDateTime::now_utc(),
        )
        .unwrap();
        let fake_bin = tmp.path().join("bin");
        fs::create_dir_all(&fake_bin).unwrap();
        let launched = tmp.path().join("backend-launched");
        for backend in ["vibe", "codex"] {
            make_fake_bin_with_body(
                &fake_bin,
                backend,
                &format!("#!/bin/sh\ntouch '{}'\nexit 1\n", launched.display()),
            );
        }
        make_fake_github_review_api(&fake_bin);
        bin()
            .args([
                "dispatch",
                "--profile",
                "real",
                "--mode",
                mode,
                "--backend",
                "vibe",
                "--model",
                "mistral-medium-3.5",
                "--branch",
                "feature/review",
                "--target",
                "fix the marker file",
                "--skip-validation-gate",
                "--config-path",
                cfg.to_str().unwrap(),
            ])
            .env("PATH", prepend_path(&fake_bin))
            .env("GAH_AVAILABILITY_PATH", &state)
            .assert()
            .failure()
            .stderr(predicate::str::contains(
                "no eligible backend available for preferred vibe/mistral-medium-3.5",
            ));
        assert!(
            !launched.exists(),
            "{mode} launched a backend despite the unavailable explicit route"
        );
    }
}

#[test]
fn explicit_backend_pin_survives_validation_retry() {
    let tmp = test_tempdir();
    let (repo, home, cfg) = setup_fix_dispatch_repo(
        &tmp,
        "validation_commands = [\"grep -q '^done$' marker.txt\"]\n",
    );
    fs::write(
        repo.join("docs/PROJECT_BRIEF.md"),
        "BROAD BACKGROUND SHOULD STAY DEFERRED\n## Working rules\nKeep operator backend selections exact.\n",
    ).unwrap();
    let config = fs::read_to_string(&cfg).unwrap();
    fs::write(
        &cfg,
        format!("{config}\n[context]\nsoft_limit_tokens = 1\nhard_limit_tokens = 20000\n"),
    )
    .unwrap();
    let captured_args = tmp.path().join("backend-args");
    let ledger_path = tmp.path().join("ledger.jsonl");
    let claude_count = tmp.path().join("claude-call-count");
    let codex_called = tmp.path().join("codex-called");
    let fake_bin = tmp.path().join("bin");
    fs::create_dir_all(&fake_bin).unwrap();
    make_fake_bin_with_body(
        &fake_bin,
        "claude",
        &format!(
            "#!/bin/sh\nn=$( [ -f '{claude_count}' ] && cat '{claude_count}' || echo 0 )\nn=$((n+1))\necho \"$n\" > '{claude_count}'\nprintf '%s\\n' \"$@\" > '{captured_args}'-\"$n\"\nif [ \"$n\" -eq 1 ]; then echo partial > marker.txt; else echo done > marker.txt; fi\nexit 0\n",
            claude_count = claude_count.display(),
            captured_args = captured_args.display(),
        ),
    );
    make_fake_bin_with_body(
        &fake_bin,
        "codex",
        &format!(
            "#!/bin/sh\ntouch '{}'\necho done > marker.txt\nexit 0\n",
            codex_called.display(),
        ),
    );
    make_fake_bin_with_body(
        &fake_bin,
        "gh",
        "#!/bin/sh\nif [ \"$1\" = \"pr\" ] && [ \"$2\" = \"create\" ]; then printf 'https://github.com/owner/real/pull/1\\n'; fi\nexit 0\n",
    );

    bin()
        .args([
            "dispatch",
            "--profile",
            "real",
            "--mode",
            "fix",
            "--backend",
            "claude",
            "--config-path",
            cfg.to_str().unwrap(),
            "--skip-validation-gate",
            "--target",
            "fix the marker file",
            "--retries",
            "1",
            "--allow-unknown-red-baseline",
        ])
        .env("PATH", prepend_path(&fake_bin))
        .env("HOME", &home)
        .env("GITHUB_TOKEN", "token")
        .env("GAH_LEDGER_PATH", &ledger_path)
        .assert()
        .success();

    assert_eq!(fs::read_to_string(&claude_count).unwrap().trim(), "2");
    assert!(
        !codex_called.exists(),
        "explicit Claude pin rerouted to Codex"
    );
    let entry: Value = serde_json::from_str(
        fs::read_to_string(&ledger_path)
            .unwrap()
            .lines()
            .next()
            .unwrap(),
    )
    .unwrap();
    assert_eq!(entry["attempts"][0]["backend"], "claude");
    assert_eq!(entry["attempts"][1]["backend"], "claude");
    let session = std::path::Path::new(entry["session_dir"].as_str().unwrap());
    for attempt in 1..=2 {
        let dir = session.join(format!("attempt-{attempt}"));
        let task = fs::read_to_string(dir.join("task.md")).unwrap();
        let context: Value =
            serde_json::from_str(&fs::read_to_string(dir.join("context-built.json")).unwrap())
                .unwrap();
        let argv = fs::read_to_string(format!("{}-{attempt}", captured_args.display())).unwrap();
        assert!(
            argv.contains(&task),
            "backend must receive the audited prompt"
        );
        assert_eq!(context["prompt"].as_str(), Some(task.as_str()));
        assert_eq!(context["compacted"], true);
        assert!(task.contains("Keep operator backend selections exact."));
        assert!(!task.contains("BROAD BACKGROUND"));
        assert_eq!(task.matches("## Deferred Context").count(), 1);
        let map = context["deferred_sources"]
            .as_array()
            .unwrap()
            .iter()
            .find(|source| source["name"] == "Repository Map")
            .unwrap();
        let map_path = map["path"].as_str().unwrap();
        assert!(task.contains(map_path));
        let map_body = fs::read_to_string(map_path).unwrap();
        assert!(map_body.contains("README.md"));
        assert!(!task.contains(&map_body));
        if attempt == 2 {
            assert!(
                task.contains("This retry starts from a clean target branch"),
                "current validation repair evidence must survive compaction"
            );
        }
    }
}
