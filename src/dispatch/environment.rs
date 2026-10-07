use crate::config::Profile;
use crate::runner;

/// Exports `profile.env_file` (or `env_file_prod` with `--prod`) into the
/// real process environment, as early as possible.
///
/// `profile.pat()` and other provider.rs calls (GitLab/GitHub API lookups
/// made by the harness itself -- MR creation, review-target resolution,
/// posting comments) read GITLAB_PAT/GITHUB_TOKEN etc. via `std::env::var`
/// directly, and those calls can happen before any backend is spawned.
/// Loading the env file into a `Vec<(String, String)>` for a spawned
/// child's environment (done later, per mode, for the backend process
/// itself) never reaches these in-process calls -- confirmed live: a
/// review dispatch failed 3 layers downstream with a git refspec error
/// because GITLAB_PAT was never actually in this process's environment.
pub(in crate::dispatch) fn export_profile_env(profile: &Profile, prod: bool) {
    let resolved_env = if prod {
        profile.env_file_prod.as_deref().unwrap_or("")
    } else {
        profile.env_file.as_deref().unwrap_or("")
    };
    if resolved_env.is_empty() {
        return;
    }
    for (key, value) in runner::load_env_file(resolved_env) {
        if is_reserved(&key) {
            continue;
        }
        std::env::set_var(key, value);
    }
}

/// Keys a profile env file may not set. `GAH_ENFORCE_JOB_FILE` carries the
/// `gah dispatch --enforce-job-file` opt-in, which decides whether commands
/// from a job file are executed (issue #1474): only that flag may set it, so
/// an env file can neither switch enforcement on for every direct dispatch
/// nor silently switch the flag off.
fn is_reserved(key: &str) -> bool {
    key == "GAH_ENFORCE_JOB_FILE"
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_file_cannot_set_the_job_file_opt_in() {
        let _exec_guard = crate::test_support::ExecGuard::new();
        let tmp = tempfile::tempdir().unwrap();
        let env_file = tmp.path().join("profile.env");
        std::fs::write(
            &env_file,
            "GAH_ENFORCE_JOB_FILE=1\nGAH_TEST_1474_CONTROL=exported\n",
        )
        .unwrap();
        let mut profile = crate::dispatch::test_util::profile(tmp.path());
        profile.env_file = Some(env_file.to_string_lossy().into_owned());
        let before = std::env::var("GAH_ENFORCE_JOB_FILE").ok();
        std::env::remove_var("GAH_TEST_1474_CONTROL");

        export_profile_env(&profile, false);

        assert_eq!(std::env::var("GAH_ENFORCE_JOB_FILE").ok(), before);
        assert_eq!(
            std::env::var("GAH_TEST_1474_CONTROL").as_deref(),
            Ok("exported"),
            "ordinary keys are still exported"
        );
        std::env::remove_var("GAH_TEST_1474_CONTROL");
    }
}
