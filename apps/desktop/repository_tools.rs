//! Detect repository CLI packages before asking the user to authenticate.
#[cfg(not(windows))]
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
    #[cfg(not(windows))]
    let output = command("which").arg(program).output();
    output.is_ok_and(|output| output.status.success())
}
