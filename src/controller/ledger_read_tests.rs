use super::run_once;
use crate::config::{Defaults, GahConfig, Profile, RoutingPolicy};
use std::collections::HashMap;
use std::fs;
use std::os::unix::fs::PermissionsExt;

struct ProviderPathGuard;

impl Drop for ProviderPathGuard {
    fn drop(&mut self) {
        crate::provider::clear_test_provider_path();
    }
}

#[test]
fn loop_once_reads_its_profile_ledger_exactly_once() {
    let tmp = tempfile::tempdir().unwrap();
    let bin_dir = tmp.path().join("bin");
    fs::create_dir_all(&bin_dir).unwrap();
    let gh = bin_dir.join("gh");
    fs::write(&gh, "#!/bin/sh\nprintf '[]\\n'\n").unwrap();
    fs::set_permissions(&gh, fs::Permissions::from_mode(0o755)).unwrap();
    crate::provider::set_test_provider_path(bin_dir.to_str().unwrap());
    let _provider_guard = ProviderPathGuard;
    let _availability_guard =
        crate::test_support::AvailabilityEnvGuard::set(tmp.path().join("availability.json"));

    let repo = tmp.path().join("repo");
    fs::create_dir_all(&repo).unwrap();
    let profile: Profile = toml::from_str(&format!(
        r#"
display_name = "Test"
repo_id = "test"
provider = "github"
repo = "owner/test"
local_path = "{}"
artifact_root = "{}"
default_target_branch = "main"
"#,
        repo.display(),
        tmp.path().join("profile-artifacts").display()
    ))
    .unwrap();
    let mut cfg = GahConfig {
        context: Default::default(),
        defaults: Defaults {
            current_manager: None,
            artifact_root: tmp.path().join("artifacts").to_string_lossy().into_owned(),
            worktree_base: String::new(),
            llm_base_url: String::new(),
            llm_model_local: String::new(),
            llm_model_cloud: String::new(),
            routing: RoutingPolicy::default(),
        },
        profiles: HashMap::new(),
    };
    cfg.profiles.insert("test".to_string(), profile);

    crate::ledger::reset_read_entries_call_count(&cfg);
    run_once(&cfg, "test", false, 1, false).unwrap();

    assert_eq!(crate::ledger::read_entries_call_count(&cfg), 1);
}
