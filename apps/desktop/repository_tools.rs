//! Detect repository CLI packages before asking the user to authenticate.
use super::{command, local_only};
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
            installed: command(if cfg!(windows) { "where.exe" } else { "which" })
                .arg(program)
                .output()
                .is_ok_and(|output| output.status.success()),
        })
        .collect())
}
