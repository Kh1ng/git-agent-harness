//! Detect repository CLI packages before asking the user to authenticate.
#[cfg(not(any(windows, target_os = "macos")))]
use super::command;
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
    #[cfg(not(any(windows, target_os = "macos")))]
    let output = command("which").arg(program).output();
    #[cfg(not(target_os = "macos"))]
    return output.is_ok_and(|output| output.status.success());
    #[cfg(target_os = "macos")]
    super::open_project::which(program).is_some()
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
