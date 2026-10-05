//! Detect repository CLI packages before asking the user to authenticate.
use super::local_only;
use serde::Serialize;

#[derive(Serialize)]
pub struct RepositoryTool {
    program: &'static str,
    installed: bool,
}

#[tauri::command]
pub fn repository_tools(window: tauri::WebviewWindow) -> Result<Vec<RepositoryTool>, String> {
    local_only(&window)?;
    Ok(["gh", "glab"]
        .into_iter()
        .map(|program| RepositoryTool {
            program,
            installed: repository_tool_installed(program),
        })
        .collect())
}

// Check the environment where GAH runs, including the worker environment in WSL.
fn repository_tool_installed(program: &str) -> bool {
    #[cfg(windows)]
    let output = super::wsl_command(&super::read_settings())
        .args([
            "--exec",
            "bash",
            "-lc",
            super::WSL_TOOL_PROBE,
            "gah-tool-probe",
            program,
        ])
        .output();
    #[cfg(windows)]
    return output.is_ok_and(|output| output.status.success());
    // `which` is not guaranteed on minimal Linux images, so search PATH here.
    #[cfg(not(any(windows, target_os = "macos")))]
    return std::env::var_os("PATH").is_some_and(|paths| path_has_executable(&paths, program));
    #[cfg(target_os = "macos")]
    super::open_project::which(program).is_some()
}

/// Minimal `which` for Linux: find an executable file named `program` in `paths`.
#[cfg(not(any(windows, target_os = "macos")))]
fn path_has_executable(paths: &std::ffi::OsStr, program: &str) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::env::split_paths(paths).any(|dir| {
        std::fs::metadata(dir.join(program))
            .is_ok_and(|meta| meta.is_file() && (meta.permissions().mode() & 0o111) != 0)
    })
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    #[test]
    fn detects_homebrew_repository_tools_outside_launchd_path() {
        if std::env::var_os("GAH_TEST_LAUNCHD_PATH").is_none() {
            let result = std::process::Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "repository_tools::tests::detects_homebrew_repository_tools_outside_launchd_path"])
                .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
                .env("GAH_TEST_LAUNCHD_PATH", "1")
                .status().unwrap();
            assert!(result.success());
            return;
        }
        // Hosts without Homebrew tools still check that missing tools stay missing.
        for program in ["gh", "glab"] {
            if ["/opt/homebrew/bin", "/usr/local/bin"]
                .iter()
                .any(|dir| std::path::Path::new(dir).join(program).is_file())
            {
                assert!(super::repository_tool_installed(program), "{program}");
            }
        }
        assert!(!super::repository_tool_installed(
            "gah-missing-repository-tool"
        ));
    }
}

#[cfg(all(test, not(any(windows, target_os = "macos"))))]
mod tests {
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn path_search_finds_only_executable_files() {
        let dir = std::env::temp_dir().join(format!(
            "gah-repository-tools-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&dir).unwrap();
        let executable = dir.join("gah-path-search-tool");
        std::fs::write(&executable, "#!/bin/sh\n").unwrap();
        std::fs::set_permissions(&executable, std::fs::Permissions::from_mode(0o755)).unwrap();
        let plain = dir.join("gah-path-search-plain");
        std::fs::write(&plain, "not executable").unwrap();
        let missing = dir.join("missing");
        let paths = std::env::join_paths([&dir, &missing]).unwrap();
        assert!(super::path_has_executable(&paths, "gah-path-search-tool"));
        assert!(!super::path_has_executable(&paths, "gah-path-search-plain"));
        assert!(!super::path_has_executable(
            &paths,
            "gah-path-search-missing"
        ));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
