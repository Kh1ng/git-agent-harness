use anyhow::{bail, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

pub(crate) const SCRIPT: &str = include_str!("credential_guard.py");

pub(crate) fn bound(env: &[(String, String)]) -> bool {
    env.iter()
        .any(|(name, value)| name == "GAH_VIBE_CREDENTIAL_BOUND" && value == "1")
}

/// Use the explicitly selected launcher's Python environment, including venv
/// identity. A shell/Rust launcher cannot silently bypass the provider guard.
pub(crate) fn interpreter(executable: &Path) -> Result<PathBuf> {
    let launcher = std::fs::canonicalize(executable)
        .map_err(|_| anyhow::anyhow!("selected Vibe launcher is unavailable"))?;
    let text = std::fs::read_to_string(launcher).map_err(|_| {
        anyhow::anyhow!("named Vibe credentials require the supported Python launcher")
    })?;
    let shebang = text
        .lines()
        .next()
        .and_then(|line| line.strip_prefix("#!"))
        .ok_or_else(|| {
            anyhow::anyhow!("named Vibe credentials require the supported Python launcher")
        })?;
    let python = PathBuf::from(shebang.trim());
    if !python.is_absolute()
        || !crate::runner::is_executable_path(&python)
        || !python
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("python"))
    {
        bail!("named Vibe credentials require the supported Python launcher");
    }
    Ok(python)
}

pub(crate) fn command(executable: &Path, env: &[(String, String)]) -> Result<Command> {
    if !bound(env) {
        return Ok(Command::new(executable));
    }
    let mut cmd = Command::new(interpreter(executable)?);
    cmd.args(["-c", &format!("{SCRIPT}\nimport sys\nsys.argv = ['vibe', *sys.argv[1:]]\nfrom vibe.cli.entrypoint import main\nmain()\n")]);
    Ok(cmd)
}

/// Manager Chat's fixed stdin bridge runs in the same guarded interpreter.
/// Its provider resolution is guarded after any managed config is applied.
pub(crate) fn bridge(executable: &Path, requested: &Path, args: &mut [String]) -> Result<Command> {
    let python = interpreter(executable)?;
    if std::fs::canonicalize(requested).ok() != std::fs::canonicalize(&python).ok()
        || args.len() != 2
        || args[0] != "-c"
    {
        bail!("Vibe bridge does not match the selected Python launcher");
    }
    args[1] = format!("{SCRIPT}\n{}", args[1]);
    Ok(Command::new(python))
}
