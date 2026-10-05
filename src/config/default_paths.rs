use std::path::PathBuf;

fn home_dir() -> PathBuf {
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));
    home.map_or_else(|| PathBuf::from("/root"), PathBuf::from)
}

pub fn default_config_dir() -> PathBuf {
    home_dir().join(".config/gah")
}

/// Worktree root used when `defaults.worktree_base` is empty, so dispatch
/// never plans worktrees at the filesystem root.
pub fn default_worktree_base() -> PathBuf {
    home_dir().join(".local/share/gah/worktrees")
}

/// Fills an empty `defaults.worktree_base` with `default_worktree_base()` at
/// load time, so a config written with `worktree_base = ""` still resolves to
/// a usable worktree root (issue #1366).
pub(crate) fn resolve_empty_worktree_base(cfg: &mut super::GahConfig) {
    if cfg.defaults.worktree_base.trim().is_empty() {
        cfg.defaults.worktree_base = default_worktree_base().display().to_string();
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn load_resolves_empty_worktree_base_to_default() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.toml");
        std::fs::write(&path, "[defaults]\nworktree_base = \"\"\n").unwrap();
        let cfg = crate::config::load(Some(path.to_str().unwrap())).unwrap();
        assert_eq!(
            std::path::PathBuf::from(&cfg.defaults.worktree_base),
            super::default_worktree_base()
        );
        assert!(cfg
            .defaults
            .worktree_base
            .ends_with(".local/share/gah/worktrees"));
    }
}
