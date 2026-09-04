#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum GithubCiStatus {
    Missing,
    Pending,
    Passed,
    Failed,
}

impl GithubCiStatus {
    pub(crate) fn as_str(self) -> Option<&'static str> {
        match self {
            Self::Missing => None,
            Self::Pending => Some("pending"),
            Self::Passed => Some("passed"),
            Self::Failed => Some("failed"),
        }
    }
}

/// Classifies the complete GitHub check rollup using one policy for sync and review gates.
pub(crate) fn classify<'a>(
    conclusions: impl IntoIterator<Item = Option<&'a str>>,
) -> GithubCiStatus {
    let mut found = false;
    let mut passed = true;
    for conclusion in conclusions {
        found = true;
        if conclusion == Some("FAILURE") {
            return GithubCiStatus::Failed;
        }
        passed &= matches!(conclusion, Some("SUCCESS" | "NEUTRAL" | "SKIPPED"));
    }
    match (found, passed) {
        (false, _) => GithubCiStatus::Missing,
        (true, true) => GithubCiStatus::Passed,
        (true, false) => GithubCiStatus::Pending,
    }
}
