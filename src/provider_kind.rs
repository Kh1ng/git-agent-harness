//! Canonical git-hosting provider identity.
//!
//! Before this module, "which provider is this profile talking to" was
//! independently hardcoded as a bare `&str` match on `Profile::provider`
//! (`"github"`/`"gitlab"`) in ~25 places across `config.rs`, `provider.rs`,
//! `doctor.rs`, `sync.rs`, `status.rs`, and `init.rs`, each with its own
//! `other => bail!(...)`/`_ => None` fallback -- exactly the pre-`BackendKind`
//! state backends were in (see `backend_kind.rs`). This is the same recipe
//! applied to a different axis: `parse`/`as_str`/`all`/`Display`, an
//! `UnknownProviderKind` error, round-trip tests.
//!
//! Unlike backends, provider doesn't get a trait: the per-provider functions
//! in `provider.rs` (`gitlab_mr`/`github_mr`, `gitlab_merge_mr`/
//! `github_merge_mr`, etc.) already share identical signatures pairwise --
//! the existing one-line match-to-function-call at each site already *is*
//! the thin adapter. A trait would add methods for zero normalization
//! benefit, unlike `BackendRunner`'s `RunContext`, which existed specifically
//! to paper over genuinely divergent per-backend parameter shapes.
//!
//! Only github/gitlab are implemented anywhere in this codebase. `parse` is
//! still a `Result`, not an infallible mapping, since `Profile.provider` is
//! free-text config -- a typo or an unimplemented provider must fail
//! loudly, not silently coerce to one of the two real ones.

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ProviderKind {
    Github,
    Gitlab,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UnknownProviderKind(pub String);

impl fmt::Display for UnknownProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unknown provider '{}'", self.0)
    }
}

impl std::error::Error for UnknownProviderKind {}

impl ProviderKind {
    pub fn parse(s: &str) -> Result<Self, UnknownProviderKind> {
        match s {
            "github" => Ok(Self::Github),
            "gitlab" => Ok(Self::Gitlab),
            other => Err(UnknownProviderKind(other.to_string())),
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Github => "github",
            Self::Gitlab => "gitlab",
        }
    }

    pub fn all() -> [ProviderKind; 2] {
        [Self::Github, Self::Gitlab]
    }
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_canonical_name_round_trips() {
        for kind in ProviderKind::all() {
            assert_eq!(ProviderKind::parse(kind.as_str()).unwrap(), kind);
        }
    }

    #[test]
    fn unknown_strings_are_rejected() {
        assert!(ProviderKind::parse("bitbucket").is_err());
        assert!(ProviderKind::parse("gitea").is_err());
        assert!(ProviderKind::parse("").is_err());
    }

    #[test]
    fn all_returns_exactly_two_kinds_with_no_duplicates() {
        let all = ProviderKind::all();
        assert_eq!(all.len(), 2);
        let mut seen = std::collections::HashSet::new();
        for kind in all {
            assert!(seen.insert(kind), "duplicate kind in all(): {kind:?}");
        }
    }

    #[test]
    fn display_matches_as_str() {
        for kind in ProviderKind::all() {
            assert_eq!(kind.to_string(), kind.as_str());
        }
    }
}
