//! TICKET-109: a single place that knows how to (a) verify a named reviewer
//! capability is actually installed, and (b) activate it for a review turn.
//! Adding a new capability means adding an entry to `activation_prefix`, not
//! touching call sites -- "no hardcoded /ponytail scattered across call
//! sites" per the ticket's own constraint.
//!
//! Availability is checked generically via the Claude Code plugin cache
//! convention (`~/.claude/plugins/cache/<capability>/`), not a Claude-
//! specific hack: any future capability that ships as a Claude Code plugin
//! is checkable the same way, by capability name == plugin directory name.

use std::path::PathBuf;

/// The exact turn-opening text that activates a capability for a Claude
/// Code `-p` invocation. Returns `None` for a capability this module
/// doesn't know how to activate -- callers must treat that as a hard
/// failure (a required-but-unactivatable capability must not silently
/// degrade to an ordinary review).
pub fn activation_prefix(capability: &str) -> Option<&'static str> {
    match capability {
        "ponytail" => Some("/ponytail full\n\n"),
        _ => None,
    }
}

/// Whether `capability` is installed, checked via the Claude Code plugin
/// cache directory. `home_override` lets tests point this at a fake HOME
/// instead of reading the real environment.
pub fn is_capability_available(capability: &str, home_override: Option<&str>) -> bool {
    plugin_cache_dir(home_override).join(capability).is_dir()
}

fn plugin_cache_dir(home_override: Option<&str>) -> PathBuf {
    let home = home_override
        .map(str::to_string)
        .or_else(|| std::env::var("HOME").ok())
        .unwrap_or_else(|| "/root".into());
    PathBuf::from(home).join(".claude/plugins/cache")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ponytail_has_a_known_activation_prefix() {
        assert_eq!(activation_prefix("ponytail"), Some("/ponytail full\n\n"));
    }

    #[test]
    fn unknown_capability_has_no_activation_prefix() {
        assert_eq!(activation_prefix("some-future-skill"), None);
    }

    #[test]
    fn capability_available_when_plugin_dir_exists() {
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".claude/plugins/cache/ponytail")).unwrap();
        assert!(is_capability_available(
            "ponytail",
            Some(tmp.path().to_str().unwrap())
        ));
    }

    #[test]
    fn capability_unavailable_when_plugin_dir_missing() {
        let tmp = tempfile::tempdir().unwrap();
        assert!(!is_capability_available(
            "ponytail",
            Some(tmp.path().to_str().unwrap())
        ));
    }

    #[test]
    fn capability_unavailable_when_path_is_a_file_not_a_dir() {
        // A stray file at the expected plugin path must not count as
        // "installed" -- is_dir() specifically, not just exists().
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(".claude/plugins/cache")).unwrap();
        std::fs::write(tmp.path().join(".claude/plugins/cache/ponytail"), "oops").unwrap();
        assert!(!is_capability_available(
            "ponytail",
            Some(tmp.path().to_str().unwrap())
        ));
    }
}
