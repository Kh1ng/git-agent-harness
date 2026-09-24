use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use super::{central_url, command, config_dir, read_settings, write_settings, DesktopSettings};

const PROFILE_LIMIT: usize = 256;

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenProjectRef {
    profile: String,
    node_id: Option<String>,
    session_id: Option<String>,
}

#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct OpenTool {
    id: &'static str,
    label: &'static str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopOpenContext {
    available: bool,
    preferred_tool: Option<String>,
    tools: Vec<OpenTool>,
    reason: Option<&'static str>,
}

#[derive(Clone, Deserialize)]
struct Profile {
    name: String,
    local_path: String,
    repo_id: String,
    worktree_base: String,
}

#[derive(Clone, Copy, PartialEq)]
enum Environment {
    Native,
    #[cfg(windows)]
    Wsl,
}

#[derive(Clone)]
struct LocalProject {
    node_id: Option<String>,
    profile: Profile,
    environment: Environment,
}

struct Checkout {
    path: String,
}

fn configured_central(window: &tauri::WebviewWindow) -> Result<(), String> {
    if window.label() != "dashboard" {
        return Err("The local checkout bridge is unavailable in this window.".into());
    }
    let current = window.url().map_err(|error| error.to_string())?;
    if !same_central_origin(&current, &read_settings().central_url) {
        return Err(
            "The local checkout bridge only accepts the configured central dashboard.".into(),
        );
    }
    Ok(())
}

fn same_central_origin(current: &tauri::Url, configured: &str) -> bool {
    central_url(configured).is_ok_and(|central| current.origin() == central.origin())
}

fn node_id(path: &Path) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path).ok()?).ok()?;
    value
        .get("node_id")?
        .as_str()
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

fn native_node_id(settings: &DesktopSettings) -> Option<String> {
    let worker = config_dir()
        .parent()?
        .parent()?
        .join(".local/share/gah/worker/identity.json");
    let configured = config_dir().join("coordinator-identity.json");
    let repository =
        PathBuf::from(&settings.repository_path).join("config/coordinator-identity.json");
    let candidates = if settings.node_role == "worker" {
        [worker, configured, repository]
    } else {
        [configured, repository, worker]
    };
    candidates.iter().find_map(|path| node_id(path))
}

fn parse_profiles(
    output: &[u8],
    node_id: Option<String>,
    environment: Environment,
) -> Result<Vec<LocalProject>, String> {
    let profiles: Vec<Profile> = serde_json::from_slice(output)
        .map_err(|_| "GAH returned an invalid local profile list.")?;
    if profiles.len() > PROFILE_LIMIT {
        return Err("The local profile list is too large.".into());
    }
    Ok(profiles
        .into_iter()
        .map(|profile| LocalProject {
            node_id: node_id.clone(),
            profile,
            environment,
        })
        .collect())
}

#[cfg(not(windows))]
fn local_projects(settings: &DesktopSettings) -> Result<Vec<LocalProject>, String> {
    let gah = super::installed_gah()?;
    let output = command(gah.to_string_lossy().as_ref())
        .args(["profile", "list", "--json"])
        .output()
        .map_err(|error| format!("Cannot read local GAH profiles: {error}"))?;
    if !output.status.success() {
        return Err("GAH could not read local profiles.".into());
    }
    parse_profiles(
        &output.stdout,
        native_node_id(settings),
        Environment::Native,
    )
}

#[cfg(windows)]
const WSL_PROJECTS_SCRIPT: &str = r#"
set -euo pipefail
worker_env="${HOME}/.local/share/gah/worker/worker.env"
[ -f "$worker_env" ]
source "$worker_env" >/dev/null 2>&1
python3 - <<'PY'
import json, os, pathlib, subprocess
identity = json.loads((pathlib.Path.home() / '.local/share/gah/worker/identity.json').read_text())
profiles = json.loads(subprocess.check_output([os.environ['GAH_BINARY'], 'profile', 'list', '--json']))
print(json.dumps({'node_id': identity['node_id'], 'profiles': profiles}))
PY
"#;

#[cfg(windows)]
#[derive(Deserialize)]
struct WslProjects {
    node_id: String,
    profiles: Vec<Profile>,
}

#[cfg(windows)]
fn local_projects(settings: &DesktopSettings) -> Result<Vec<LocalProject>, String> {
    let mut projects = Vec::new();
    if let Some(gah) = where_program(&["gah.exe"]) {
        if let Ok(output) = command(gah.to_string_lossy().as_ref())
            .args(["profile", "list", "--json"])
            .output()
        {
            if output.status.success() {
                projects.extend(parse_profiles(
                    &output.stdout,
                    native_node_id(settings),
                    Environment::Native,
                )?);
            }
        }
    }
    let output = super::wsl_command(settings)
        .args(["--exec", "bash", "-lc", WSL_PROJECTS_SCRIPT])
        .output();
    if let Ok(output) = output {
        if output.status.success() {
            let info: WslProjects = serde_json::from_slice(&output.stdout)
                .map_err(|_| "The WSL worker returned invalid project data.")?;
            if info.profiles.len() > PROFILE_LIMIT {
                return Err("The local profile list is too large.".into());
            }
            projects.extend(info.profiles.into_iter().map(|profile| LocalProject {
                node_id: Some(info.node_id.clone()),
                profile,
                environment: Environment::Wsl,
            }));
        }
    }
    if projects.is_empty() {
        return Err("No local GAH profiles are available on this computer.".into());
    }
    Ok(projects)
}

fn safe_identifier(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value.chars().all(|character| {
            character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.')
        })
}

#[cfg(any(windows, test))]
fn posix_join(base: &str, name: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), name)
}

fn checkout_candidate(
    project: &LocalProject,
    session_id: Option<&str>,
) -> Result<(String, Option<String>), String> {
    if project.profile.name.is_empty()
        || project.profile.name.chars().any(char::is_control)
        || project.profile.local_path.is_empty()
        || project.profile.local_path.chars().any(char::is_control)
    {
        return Err("The local profile identity is invalid.".into());
    }
    let Some(session_id) = session_id.filter(|id| *id != "default") else {
        return Ok((project.profile.local_path.clone(), None));
    };
    if !safe_identifier(session_id)
        || project.profile.repo_id.is_empty()
        || project.profile.repo_id.chars().any(char::is_control)
        || project.profile.worktree_base.is_empty()
        || project.profile.worktree_base.chars().any(char::is_control)
    {
        return Err("The chat worktree identity is invalid.".into());
    }
    let name = format!("gah-chat-{}-{session_id}", project.profile.repo_id);
    #[cfg(windows)]
    if project.environment == Environment::Wsl {
        return Ok((
            posix_join(&project.profile.worktree_base, &name),
            Some(project.profile.worktree_base.clone()),
        ));
    }
    Ok((
        Path::new(&project.profile.worktree_base)
            .join(name)
            .to_string_lossy()
            .into_owned(),
        Some(project.profile.worktree_base.clone()),
    ))
}

fn validate_native_checkout(candidate: &str, base: Option<&str>) -> Result<String, String> {
    if candidate.chars().any(char::is_control)
        || base.is_some_and(|value| value.chars().any(char::is_control))
    {
        return Err("The local checkout path is invalid.".into());
    }
    let path =
        std::fs::canonicalize(candidate).map_err(|_| "The local checkout no longer exists.")?;
    if !path.is_dir() {
        return Err("The local checkout is not a directory.".into());
    }
    if let Some(base) = base {
        let base =
            std::fs::canonicalize(base).map_err(|_| "The local worktree root no longer exists.")?;
        if path == base || !path.starts_with(&base) {
            return Err("The chat worktree is outside the configured worktree root.".into());
        }
    }
    let top = command("git")
        .args([
            "-C",
            path.to_string_lossy().as_ref(),
            "rev-parse",
            "--show-toplevel",
        ])
        .output()
        .map_err(|_| "Git is unavailable on this computer.")?;
    if !top.status.success() {
        return Err("The resolved folder is not a Git checkout.".into());
    }
    let root = std::fs::canonicalize(String::from_utf8_lossy(&top.stdout).trim())
        .map_err(|_| "Git returned an invalid checkout path.")?;
    if root != path {
        return Err("The resolved folder is outside the checkout root.".into());
    }
    let path = path.to_string_lossy().into_owned();
    if path.chars().any(char::is_control) {
        return Err("The local checkout path is invalid.".into());
    }
    Ok(path)
}

#[cfg(any(windows, test))]
const WSL_RESOLVE_SCRIPT: &str = r#"
set -euo pipefail
target="$(readlink -f -- "$1")"
[ -d "$target" ]
if [ "$3" = session ]; then
  base="$(readlink -f -- "$2")"
  [ "$target" != "$base" ]
  case "$target" in "$base"/*) ;; *) exit 1 ;; esac
fi
root="$(git -C "$target" rev-parse --show-toplevel)"
[ "$(readlink -f -- "$root")" = "$target" ]
wslpath -w "$target"
"#;

#[cfg(any(windows, test))]
fn wsl_resolve_args(candidate: &str, base: Option<&str>) -> Vec<String> {
    [
        "--exec",
        "bash",
        "-lc",
        WSL_RESOLVE_SCRIPT,
        "gah-open-project",
        candidate,
        base.unwrap_or(""),
        if base.is_some() { "session" } else { "profile" },
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}

#[cfg(windows)]
fn resolve_wsl_checkout(
    settings: &DesktopSettings,
    candidate: &str,
    base: Option<&str>,
) -> Result<String, String> {
    let output = super::wsl_command(settings)
        .args(wsl_resolve_args(candidate, base))
        .output()
        .map_err(|error| format!("Cannot resolve the WSL checkout: {error}"))?;
    if !output.status.success() {
        return Err("The WSL checkout is missing or outside its configured worktree root.".into());
    }
    parse_wsl_checkout(&output.stdout)
}

#[cfg(any(windows, test))]
fn parse_wsl_checkout(output: &[u8]) -> Result<String, String> {
    let path = String::from_utf8_lossy(output).trim().to_owned();
    if path.is_empty() || path.chars().any(char::is_control) {
        return Err("WSL returned an invalid Windows checkout path.".into());
    }
    Ok(path)
}

fn select_project(
    projects: Vec<LocalProject>,
    project_ref: &OpenProjectRef,
) -> Result<Option<LocalProject>, String> {
    if project_ref.profile.is_empty()
        || project_ref.profile.len() > 128
        || project_ref.profile.chars().any(char::is_control)
        || project_ref
            .node_id
            .as_deref()
            .is_none_or(|id| id.is_empty() || id.len() > 128 || id.chars().any(char::is_control))
    {
        return Err("The project identity is invalid.".into());
    }
    Ok(projects.into_iter().find(|candidate| {
        candidate.profile.name == project_ref.profile
            && project_ref.node_id.as_ref().map_or(true, |requested| {
                candidate.node_id.as_ref() == Some(requested)
            })
    }))
}

fn resolve_checkout(project_ref: &OpenProjectRef) -> Result<Option<Checkout>, String> {
    let settings = read_settings();
    let Some(project) = select_project(local_projects(&settings)?, project_ref)? else {
        return Ok(None);
    };
    let (candidate, base) = checkout_candidate(&project, project_ref.session_id.as_deref())?;
    let path = match project.environment {
        Environment::Native => validate_native_checkout(&candidate, base.as_deref())?,
        #[cfg(windows)]
        Environment::Wsl => resolve_wsl_checkout(&settings, &candidate, base.as_deref())?,
    };
    Ok(Some(Checkout { path }))
}

fn tool(id: &'static str, label: &'static str) -> OpenTool {
    OpenTool { id, label }
}

fn supported_tool(id: &str) -> bool {
    matches!(
        id,
        "file_manager"
            | "vscode"
            | "vim"
            | "neovim"
            | "idea"
            | "webstorm"
            | "pycharm"
            | "rustrover"
            | "clion"
            | "goland"
            | "rider"
            | "android_studio"
            | "xcode"
    )
}

#[cfg(target_os = "macos")]
fn which(name: &str) -> Option<String> {
    let path = command("which")
        .arg(name)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
        .filter(|path| !path.is_empty());
    path.or_else(|| {
        ["/opt/homebrew/bin", "/usr/local/bin", "/usr/bin"]
            .into_iter()
            .map(|dir| Path::new(dir).join(name))
            .find(|path| path.is_file())
            .map(|path| path.to_string_lossy().into_owned())
    })
}

#[cfg(target_os = "macos")]
fn has_app(name: &str) -> bool {
    command("open")
        .args(["-Ra", name])
        .status()
        .is_ok_and(|status| status.success())
}

#[cfg(any(target_os = "macos", test))]
fn contains_xcode_project(path: &Path, depth: usize) -> bool {
    if depth == 0 {
        return false;
    }
    let Ok(entries) = std::fs::read_dir(path) else {
        return false;
    };
    entries.filter_map(Result::ok).any(|entry| {
        let path = entry.path();
        let extension = path.extension().and_then(|value| value.to_str());
        matches!(extension, Some("xcodeproj" | "xcworkspace"))
            || entry
                .file_type()
                .is_ok_and(|kind| kind.is_dir() && !kind.is_symlink())
                && contains_xcode_project(&path, depth - 1)
    })
}

#[cfg(target_os = "macos")]
fn available_tools(checkout: &Checkout) -> Vec<OpenTool> {
    let mut tools = vec![tool("file_manager", "Finder")];
    if which("code").is_some() || has_app("Visual Studio Code") {
        tools.push(tool("vscode", "VS Code"));
    }
    if which("vim").is_some() {
        tools.push(tool("vim", "Vim"));
    }
    if which("nvim").is_some() {
        tools.push(tool("neovim", "Neovim"));
    }
    for (id, label, app) in [
        ("idea", "IntelliJ IDEA", "IntelliJ IDEA"),
        ("webstorm", "WebStorm", "WebStorm"),
        ("pycharm", "PyCharm", "PyCharm"),
        ("rustrover", "RustRover", "RustRover"),
        ("clion", "CLion", "CLion"),
        ("goland", "GoLand", "GoLand"),
        ("rider", "Rider", "Rider"),
        ("android_studio", "Android Studio", "Android Studio"),
    ] {
        if has_app(app) {
            tools.push(tool(id, label));
        }
    }
    if has_app("Xcode") && contains_xcode_project(Path::new(&checkout.path), 5) {
        tools.push(tool("xcode", "Xcode"));
    }
    tools
}

#[cfg(windows)]
fn where_program(names: &[&str]) -> Option<PathBuf> {
    for name in names {
        let output = command("where.exe").arg(name).output().ok()?;
        if output.status.success() {
            if let Some(path) = String::from_utf8_lossy(&output.stdout)
                .lines()
                .find(|line| !line.trim().is_empty())
            {
                return Some(PathBuf::from(path.trim()));
            }
        }
    }
    None
}

#[cfg(windows)]
fn find_program(root: &Path, names: &[&str], depth: usize) -> Option<PathBuf> {
    if depth == 0 {
        return None;
    }
    for entry in std::fs::read_dir(root).ok()?.filter_map(Result::ok) {
        let path = entry.path();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        if kind.is_file()
            && names.iter().any(|name| {
                path.file_name()
                    .and_then(|value| value.to_str())
                    .is_some_and(|value| value.eq_ignore_ascii_case(name))
            })
        {
            return Some(path);
        }
        if kind.is_dir() && !kind.is_symlink() {
            if let Some(found) = find_program(&path, names, depth - 1) {
                return Some(found);
            }
        }
    }
    None
}

#[cfg(windows)]
fn editor_program(id: &str) -> Option<PathBuf> {
    let names: &[&str] = match id {
        "vscode" => &["Code.exe"],
        "vim" => &["vim.exe"],
        "neovim" => &["nvim.exe"],
        "idea" => &["idea64.exe"],
        "webstorm" => &["webstorm64.exe"],
        "pycharm" => &["pycharm64.exe"],
        "rustrover" => &["rustrover64.exe"],
        "clion" => &["clion64.exe"],
        "goland" => &["goland64.exe"],
        "rider" => &["rider64.exe"],
        "android_studio" => &["studio64.exe"],
        _ => return None,
    };
    if let Some(path) = where_program(names) {
        return Some(path);
    }
    let local = std::env::var_os("LOCALAPPDATA").map(PathBuf::from);
    if id == "vscode" {
        return local
            .map(|root| root.join("Programs/Microsoft VS Code/Code.exe"))
            .filter(|path| path.is_file());
    }
    for root in [
        std::env::var_os("ProgramFiles")
            .map(PathBuf::from)
            .map(|path| path.join("JetBrains")),
        local.as_ref().map(|path| path.join("Programs")),
        local.map(|path| path.join("JetBrains/Toolbox/apps")),
    ]
    .into_iter()
    .flatten()
    {
        if let Some(path) = find_program(&root, names, 7) {
            return Some(path);
        }
    }
    None
}

#[cfg(windows)]
fn available_tools(_checkout: &Checkout) -> Vec<OpenTool> {
    let mut tools = vec![tool("file_manager", "File Explorer")];
    for (id, label) in [
        ("vscode", "VS Code"),
        ("vim", "Vim"),
        ("neovim", "Neovim"),
        ("idea", "IntelliJ IDEA"),
        ("webstorm", "WebStorm"),
        ("pycharm", "PyCharm"),
        ("rustrover", "RustRover"),
        ("clion", "CLion"),
        ("goland", "GoLand"),
        ("rider", "Rider"),
        ("android_studio", "Android Studio"),
    ] {
        if editor_program(id).is_some()
            && (!matches!(id, "vim" | "neovim") || where_program(&["wt.exe"]).is_some())
        {
            tools.push(tool(id, label));
        }
    }
    tools
}

#[cfg(not(any(target_os = "macos", windows)))]
fn available_tools(_checkout: &Checkout) -> Vec<OpenTool> {
    Vec::new()
}

fn spawn(mut process: Command) -> Result<(), String> {
    process
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Cannot open the local checkout: {error}"))
}

#[cfg(any(target_os = "macos", test))]
fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(any(target_os = "macos", test))]
fn apple_script_string(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

#[cfg(target_os = "macos")]
fn launch(tool_id: &str, checkout: &Checkout) -> Result<(), String> {
    let path = checkout.path.as_str();
    match tool_id {
        "file_manager" => {
            let mut process = command("open");
            process.arg(path);
            spawn(process)
        }
        "vscode" => {
            let mut process = if let Some(code) = which("code") {
                command(&code)
            } else {
                let mut command = command("open");
                command.args(["-a", "Visual Studio Code"]);
                command
            };
            process.arg(path);
            spawn(process)
        }
        "vim" | "neovim" => {
            let binary = which(if tool_id == "vim" { "vim" } else { "nvim" })
                .ok_or("The selected editor is no longer installed.")?;
            let shell = format!(
                "cd -- {} && exec {} .",
                shell_quote(path),
                shell_quote(&binary)
            );
            let script = format!(
                "tell application \"Terminal\" to do script {}",
                apple_script_string(&shell)
            );
            let mut process = command("osascript");
            process.args(["-e", &script]);
            spawn(process)
        }
        id => {
            let app = match id {
                "idea" => "IntelliJ IDEA",
                "webstorm" => "WebStorm",
                "pycharm" => "PyCharm",
                "rustrover" => "RustRover",
                "clion" => "CLion",
                "goland" => "GoLand",
                "rider" => "Rider",
                "android_studio" => "Android Studio",
                "xcode" => "Xcode",
                _ => return Err("Unsupported local open tool.".into()),
            };
            let mut process = command("open");
            process.args(["-a", app, path]);
            spawn(process)
        }
    }
}

#[cfg(windows)]
fn launch(tool_id: &str, checkout: &Checkout) -> Result<(), String> {
    if tool_id == "file_manager" {
        let mut process = command("explorer.exe");
        process.arg(&checkout.path);
        return spawn(process);
    }
    let editor = editor_program(tool_id).ok_or("The selected editor is no longer installed.")?;
    if matches!(tool_id, "vim" | "neovim") {
        let terminal =
            where_program(&["wt.exe"]).ok_or("Windows Terminal is required for Vim and Neovim.")?;
        let mut process = command(terminal.to_string_lossy().as_ref());
        process.args(["-d", &checkout.path]).arg(editor).arg(".");
        return spawn(process);
    }
    let mut process = command(editor.to_string_lossy().as_ref());
    process.arg(&checkout.path);
    spawn(process)
}

#[cfg(not(any(target_os = "macos", windows)))]
fn launch(_tool_id: &str, _checkout: &Checkout) -> Result<(), String> {
    Err("Opening a local checkout is supported on macOS and Windows.".into())
}

#[tauri::command]
pub async fn desktop_open_context(
    window: tauri::WebviewWindow,
    project: OpenProjectRef,
) -> Result<DesktopOpenContext, String> {
    configured_central(&window)?;
    let Some(checkout) = resolve_checkout(&project)? else {
        return Ok(DesktopOpenContext {
            available: false,
            preferred_tool: None,
            tools: Vec::new(),
            reason: Some("This checkout belongs to another device."),
        });
    };
    let tools = available_tools(&checkout);
    if tools.is_empty() {
        return Ok(DesktopOpenContext {
            available: false,
            preferred_tool: None,
            tools,
            reason: Some("No supported local opener is installed."),
        });
    }
    let preferred = read_settings().preferred_open_tool;
    let preferred_tool = tools
        .iter()
        .any(|tool| tool.id == preferred)
        .then_some(preferred)
        .or_else(|| Some("file_manager".into()));
    Ok(DesktopOpenContext {
        available: true,
        preferred_tool,
        tools,
        reason: None,
    })
}

#[tauri::command]
pub async fn open_local_checkout(
    window: tauri::WebviewWindow,
    project: OpenProjectRef,
    tool: String,
) -> Result<(), String> {
    configured_central(&window)?;
    if !supported_tool(&tool) {
        return Err("Unsupported local open tool.".into());
    }
    let checkout = resolve_checkout(&project)?.ok_or("This checkout belongs to another device.")?;
    let tools = available_tools(&checkout);
    if !tools.iter().any(|candidate| candidate.id == tool) {
        return Err("Unsupported or unavailable local open tool.".into());
    }
    launch(&tool, &checkout)?;
    let mut settings = read_settings();
    settings.preferred_open_tool = tool;
    write_settings(&settings).map_err(|error| {
        format!("Checkout opened, but the preferred app could not be saved: {error}")
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn origin_check_requires_the_configured_origin() {
        let matches = |current: &str, configured: &str| {
            let current: tauri::Url = current.parse().unwrap();
            same_central_origin(&current, configured)
        };
        assert!(matches(
            "https://central.example/chat",
            "https://central.example"
        ));
        assert!(!matches(
            "https://evil.example/chat",
            "https://central.example"
        ));
        assert!(!matches(
            "http://central.example/chat",
            "https://central.example"
        ));
    }

    #[test]
    fn native_session_resolution_cannot_escape_the_worktree_root() {
        let nonce = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let root =
            std::env::temp_dir().join(format!("gah-open-test-{}-{nonce}", std::process::id()));
        let checkout = root.join("checkout");
        let worktrees = root.join("worktrees");
        let target = worktrees.join("gah-chat-owner/repo-session1");
        fs::create_dir_all(&checkout).unwrap();
        fs::create_dir_all(&target).unwrap();
        command("git")
            .args(["init", "--quiet", checkout.to_string_lossy().as_ref()])
            .status()
            .unwrap();
        command("git")
            .args(["init", "--quiet", target.to_string_lossy().as_ref()])
            .status()
            .unwrap();
        assert!(validate_native_checkout(
            target.to_string_lossy().as_ref(),
            Some(worktrees.to_string_lossy().as_ref())
        )
        .is_ok());
        assert!(validate_native_checkout(
            checkout.to_string_lossy().as_ref(),
            Some(worktrees.to_string_lossy().as_ref())
        )
        .is_err());
        assert!(validate_native_checkout("/tmp/bad\npath", None).is_err());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn opaque_project_identity_selects_only_its_local_node() {
        let project = LocalProject {
            node_id: Some("mac-1".into()),
            profile: Profile {
                name: "gah".into(),
                local_path: "/repo".into(),
                repo_id: "owner/repo".into(),
                worktree_base: "/worktrees".into(),
            },
            environment: Environment::Native,
        };
        let selected = select_project(
            vec![project.clone()],
            &OpenProjectRef {
                profile: "gah".into(),
                node_id: Some("mac-1".into()),
                session_id: Some("session1".into()),
            },
        )
        .unwrap()
        .unwrap();
        assert_eq!(selected.profile.local_path, "/repo");
        assert_eq!(
            checkout_candidate(&selected, Some("session1")).unwrap().0,
            "/worktrees/gah-chat-owner/repo-session1"
        );
        assert!(select_project(
            vec![selected],
            &OpenProjectRef {
                profile: "gah".into(),
                node_id: Some("windows-1".into()),
                session_id: None,
            },
        )
        .unwrap()
        .is_none());
        assert!(select_project(
            vec![project],
            &OpenProjectRef {
                profile: "gah".into(),
                node_id: None,
                session_id: None,
            },
        )
        .is_err());
    }

    #[test]
    fn identifiers_and_shell_arguments_are_bounded() {
        assert!(safe_identifier("session_1-test"));
        assert!(!safe_identifier("../escape"));
        assert!(!safe_identifier("bad\nvalue"));
        assert_eq!(
            shell_quote("a'b; touch /tmp/nope"),
            "'a'\"'\"'b; touch /tmp/nope'"
        );
        assert!(apple_script_string("a\"b\\c").contains("\\\""));
    }

    #[test]
    fn unknown_tools_are_rejected() {
        assert!(supported_tool("file_manager"));
        assert!(!supported_tool("shell"));
    }

    #[test]
    fn wsl_conversion_passes_untrusted_paths_as_arguments() {
        let path = "/home/user/a folder/'quoted'; touch /tmp/nope";
        let args = wsl_resolve_args(path, Some("/home/user/worktrees"));
        assert_eq!(args[5], path);
        assert_eq!(args[6], "/home/user/worktrees");
        assert_eq!(args[7], "session");
        assert!(!args[3].contains(path));
        assert!(args[3].contains("wslpath -w"));
        assert_eq!(
            posix_join("/home/me/worktrees/", "gah-chat-owner/repo-session"),
            "/home/me/worktrees/gah-chat-owner/repo-session"
        );
        assert_eq!(
            parse_wsl_checkout(b"C:\\Users\\me\\repo\r\n").unwrap(),
            "C:\\Users\\me\\repo"
        );
        assert!(parse_wsl_checkout(b"C:\\bad\npath").is_err());
    }
}
