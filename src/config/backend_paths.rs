use super::Profile;

/// Per-backend executable path overrides and idle-timeout accessors. Split
/// out of config.rs (which has a tracked, forbidden-to-raise line-count
/// baseline -- see tests/source_structure.rs) into its own module, same
/// pattern as backend_instances.rs.
impl Profile {
    /// An explicit executable path override for `backend`, if this profile
    /// sets one. `resolve_backend_executable` (in `runner::resolve`) treats a
    /// `Some` return as a literal file path to check with `is_executable_path`
    /// -- this must ONLY ever return a real path override, never a marker
    /// string, or backend launch silently breaks (see `is_backend_configured`
    /// below for the "is this set up at all" signal, which is a different
    /// question with a different answer for openhands).
    pub fn configured_backend_path(&self, backend: &str) -> Option<&str> {
        match backend {
            "codex" => self.codex_path.as_deref(),
            "claude" => self.claude_path.as_deref(),
            "agy" | "agy-main" | "agy-second" => self.agy_path.as_deref(),
            "vibe" => self.vibe_path.as_deref(),
            "opencode" => self.opencode_path.as_deref(),
            "hermes" => self.hermes_path.as_deref(),
            _ => None,
        }
    }

    pub fn review_timeout_seconds(&self) -> u64 {
        self.review_timeout_seconds.unwrap_or(300).max(1)
    }

    pub fn validation_timeout_seconds(&self) -> u64 {
        self.validation_timeout_seconds.unwrap_or(300).max(1)
    }

    pub fn agy_idle_timeout_seconds(&self) -> u64 {
        self.agy_idle_timeout_seconds.unwrap_or(120).max(1)
    }

    pub fn opencode_idle_timeout_seconds(&self) -> u64 {
        self.opencode_idle_timeout_seconds.unwrap_or(300).max(1)
    }

    pub fn openhands_idle_timeout_seconds(&self) -> u64 {
        self.openhands_idle_timeout_seconds.unwrap_or(300).max(1)
    }

    pub fn vibe_idle_timeout_seconds(&self) -> u64 {
        self.vibe_idle_timeout_seconds.unwrap_or(300).max(1)
    }

    pub fn codex_idle_timeout_seconds(&self) -> u64 {
        self.codex_idle_timeout_seconds.unwrap_or(300).max(1)
    }

    pub fn claude_idle_timeout_seconds(&self) -> u64 {
        self.claude_idle_timeout_seconds.unwrap_or(300).max(1)
    }

    pub fn hermes_idle_timeout_seconds(&self) -> u64 {
        self.hermes_idle_timeout_seconds.unwrap_or(300).max(1)
    }
}
