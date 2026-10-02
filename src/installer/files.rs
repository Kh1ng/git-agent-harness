//! Private, atomic writes for the files installers own: environment files
//! that shells and systemd both read, JSON settings, and scripts.

use anyhow::{bail, Context, Result};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;

/// Replaces `path` with `contents` at `mode`. The new file is complete on
/// disk before it takes the old one's name, so a reader never sees half.
pub fn replace(path: &Path, contents: &[u8], mode: u32) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    std::fs::create_dir_all(parent).with_context(|| format!("creating {}", parent.display()))?;
    let mut temporary = tempfile::NamedTempFile::new_in(parent)
        .with_context(|| format!("staging a file in {}", parent.display()))?;
    temporary.write_all(contents)?;
    temporary.as_file().sync_all()?;
    std::fs::set_permissions(temporary.path(), std::fs::Permissions::from_mode(mode))?;
    temporary
        .persist(path)
        .with_context(|| format!("replacing {}", path.display()))?;
    Ok(())
}

/// Rejects values a line-based file cannot hold literally.
pub fn check_value(key: &str, value: &str) -> Result<()> {
    if value.chars().any(|c| c.is_control()) {
        bail!("{key} must not contain control characters.");
    }
    Ok(())
}

/// `KEY="value"`, escaped so that both `source` in bash and systemd's
/// `EnvironmentFile=` read the value back literally.
pub fn env_line(key: &str, value: &str) -> String {
    let mut quoted = String::with_capacity(value.len() + 2);
    for c in value.chars() {
        if matches!(c, '\\' | '"' | '$' | '`') {
            quoted.push('\\');
        }
        quoted.push(c);
    }
    format!("{key}=\"{quoted}\"")
}

/// Sets `key` in an environment file, keeping its other lines. The file is
/// written with mode 0600: these files hold credentials.
pub fn env_set(path: &Path, key: &str, value: &str) -> Result<()> {
    env_set_all(path, &[(key, value)])
}

pub fn env_set_all(path: &Path, values: &[(&str, &str)]) -> Result<()> {
    for (key, value) in values {
        if key.is_empty() || !key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            bail!("{key:?} is not an environment variable name.");
        }
        check_value(key, value)?;
    }
    let existing = match std::fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => String::new(),
        Err(error) => return Err(error).with_context(|| format!("reading {}", path.display())),
    };
    let mut lines: Vec<String> = existing
        .lines()
        .filter(|line| {
            !values
                .iter()
                .any(|(key, _)| line.starts_with(&format!("{key}=")))
        })
        .map(str::to_owned)
        .collect();
    lines.extend(values.iter().map(|(key, value)| env_line(key, value)));
    replace(path, format!("{}\n", lines.join("\n")).as_bytes(), 0o600)
}

/// The value the file sets `key` to: `env_line`'s quoting undone, or a
/// single-quoted or bare value as written by hand.
pub fn env_get(path: &Path, key: &str) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    let raw = text
        .lines()
        .filter_map(|line| line.strip_prefix(&format!("{key}=")))
        .next_back()?
        .trim();
    if let Some(inner) = raw
        .strip_prefix('"')
        .and_then(|rest| rest.strip_suffix('"'))
    {
        let mut value = String::with_capacity(inner.len());
        let mut chars = inner.chars();
        while let Some(c) = chars.next() {
            value.push(if c == '\\' {
                chars.next().unwrap_or('\\')
            } else {
                c
            });
        }
        return Some(value);
    }
    Some(
        raw.strip_prefix('\'')
            .and_then(|rest| rest.strip_suffix('\''))
            .unwrap_or(raw)
            .to_string(),
    )
}

/// Whether the file sets `key` to a non-empty value.
pub fn env_has(path: &Path, key: &str) -> bool {
    env_get(path, key).is_some_and(|value| !value.is_empty())
}

/// Single-quotes a value for a bash script.
pub fn shell_quote(value: &str) -> String {
    if !value.is_empty()
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "@%+=:,./-_".contains(c))
    {
        return value.to_string();
    }
    format!("'{}'", value.replace('\'', "'\"'\"'"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    #[test]
    fn values_survive_sourcing_literally_and_unrelated_lines_stay() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("gateway.env");
        let marker = dir.path().join("must-not-exist");
        let token = format!(
            "a&b|c\"d\\e $VALUE $(touch {0}) `touch {0}` 'q'",
            marker.display()
        );
        std::fs::write(&path, "HOST=127.0.0.1\nTDAI_GATEWAY_API_KEY=old\n").unwrap();
        env_set(&path, "TDAI_GATEWAY_API_KEY", &token).unwrap();
        let read = Command::new("bash")
            .args([
                "-euc",
                "source \"$1\"; printf %s \"$TDAI_GATEWAY_API_KEY\"",
                "t",
            ])
            .arg(&path)
            .output()
            .unwrap();
        assert_eq!(String::from_utf8(read.stdout).unwrap(), token);
        assert!(!marker.exists(), "sourcing must not run the value");
        let text = std::fs::read_to_string(&path).unwrap();
        assert!(text.starts_with("HOST=127.0.0.1\n"));
        assert_eq!(text.matches("TDAI_GATEWAY_API_KEY=").count(), 1);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert!(env_has(&path, "TDAI_GATEWAY_API_KEY"));
        assert_eq!(
            env_get(&path, "TDAI_GATEWAY_API_KEY").as_deref(),
            Some(token.as_str()),
            "read back as written"
        );
        assert!(!env_has(&path, "MISSING"));
    }

    #[test]
    fn invalid_values_leave_the_old_file_untouched() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("worker.env");
        std::fs::write(&path, "COORDINATOR_TOKEN=\"old\"\n").unwrap();
        assert!(env_set(&path, "COORDINATOR_TOKEN", "bad\nvalue").is_err());
        assert!(env_set(&path, "BAD KEY", "x").is_err());
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "COORDINATOR_TOKEN=\"old\"\n"
        );
        assert!(
            !env_has(&path, "EMPTY") && {
                std::fs::write(&path, "EMPTY=\"\"\n").unwrap();
                !env_has(&path, "EMPTY")
            }
        );
    }

    #[test]
    fn shell_quoting_round_trips_through_bash() {
        for value in ["plain/path", "home with 'quotes' %", "", "$(x)`y`"] {
            let script = format!("printf %s {}", shell_quote(value));
            let output = Command::new("bash").args(["-c", &script]).output().unwrap();
            assert_eq!(String::from_utf8(output.stdout).unwrap(), value);
        }
    }
}
