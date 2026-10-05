use std::path::PathBuf;

fn home_dir() -> PathBuf {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    home.map_or_else(|| PathBuf::from("/root"), PathBuf::from)
}

pub fn default_config_dir() -> PathBuf {
    home_dir().join(".config/gah")
}

/// Single source for GAH's per-user data root (`~/.local/share/gah`), used by
/// init/setup defaults, the WSL installer's initial config, and the
/// worktree-base default below. Never resolved against a different cwd: all
/// callers treat it as an absolute path.
pub fn default_data_root() -> PathBuf {
    home_dir().join(".local/share/gah")
}

/// Worktree root used when `defaults.worktree_base` is empty, so dispatch
/// never plans worktrees at the filesystem root (issue #1366). Only resolved
/// at the point of use (dispatch planning, prune, doctor); `load()` keeps the
/// stored value untouched so load→save round trips never write a `$HOME`
/// path the user did not set, and chat sessions keep their pre-#1366
/// checkout-mode behavior for unconfigured profiles.
pub fn default_worktree_base() -> PathBuf {
    default_data_root().join("worktrees")
}

/// Effective worktree root for work that GAH itself plans (dispatch
/// worktrees, validation-gate worktrees, prune). An explicitly configured
/// value wins; an empty value resolves to `default_worktree_base()` so
/// dispatch never plans at the filesystem root. The stored config value is
/// never rewritten by this resolution.
pub fn effective_worktree_base(defaults: &super::Defaults) -> PathBuf {
    let raw = defaults.worktree_base.trim();
    if raw.is_empty() {
        default_worktree_base()
    } else {
        PathBuf::from(raw)
    }
}

#[cfg(test)]
mod tests {
    use super::{default_worktree_base, effective_worktree_base};

    #[test]
    fn load_preserves_empty_worktree_base() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, "[defaults]\nworktree_base = \"\"\n").unwrap();
        let cfg = crate::config::load(Some(path.to_str().unwrap())).unwrap();
        // The stored value stays exactly what the user wrote: resolving the
        // default in memory here would leak into every load→save command and
        // flip chat sessions from checkout mode into worktree mode.
        assert!(cfg.defaults.worktree_base.is_empty());
    }

    #[test]
    fn effective_worktree_base_resolves_empty_to_default() {
        let defaults = crate::config::Defaults::default();
        assert_eq!(effective_worktree_base(&defaults), default_worktree_base());
        let whitespace = crate::config::Defaults {
            worktree_base: "   ".to_string(),
            ..Default::default()
        };
        assert_eq!(
            effective_worktree_base(&whitespace),
            default_worktree_base()
        );
    }

    #[test]
    fn effective_worktree_base_uses_configured_value() {
        let defaults = crate::config::Defaults {
            worktree_base: "/srv/gah/worktrees".to_string(),
            ..Default::default()
        };
        assert_eq!(
            effective_worktree_base(&defaults),
            std::path::PathBuf::from("/srv/gah/worktrees")
        );
    }

    #[test]
    fn default_worktree_base_lives_under_default_data_root() {
        assert!(default_worktree_base().ends_with(".local/share/gah/worktrees"));
        assert!(default_worktree_base().is_absolute());
    }
}
