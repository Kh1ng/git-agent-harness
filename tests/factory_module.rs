//! A recurring loop must observe a module change rather than reuse its startup policy.
#[test]
fn recurring_loop_reloads_disabled_factory_before_dispatch() {
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("config.toml");
    let contents = r#"
[defaults]
factory_enabled = false
[profiles.local]
display_name = "Local"
repo_id = "fixture/local"
provider = "github"
repo = "fixture/local"
local_path = "/does/not/exist"
artifact_root = "/does/not/exist"
default_target_branch = "main"
"#;
    std::fs::write(&path, contents).unwrap();
    let mut startup: git_agent_harness::config::GahConfig = toml::from_str(contents).unwrap();
    startup.defaults.factory_enabled = Some(true);
    // This test process owns its environment; never inherit an operator routing file.
    std::env::set_var(
        "GAH_CANONICAL_CONFIG",
        directory.path().join("canonical.toml"),
    );
    git_agent_harness::controller::run_loop(&startup, "local", false, 1, false, &path)
        .expect("disabled module must exit before attempting the nonexistent repository");
}
