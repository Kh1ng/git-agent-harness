//! Issue #116: per-attempt process-tree resource observations with
//! explicit provenance. Split from `entry.rs` to keep that file under the
//! source-size guard; the ledger types are re-exported from `ledger::mod`.

use serde::{Deserialize, Serialize};

/// Issue #116: provenance of one per-attempt resource observation (CPU time,
/// peak RSS). Mirrors `BehaviorMetricQuality`: "unavailable" is never silently
/// treated as a real zero.
#[derive(Debug, Serialize, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ResourceMetricQuality {
    /// Observed on the live backend process tree during the attempt.
    Measured,
    /// The platform cannot measure process trees at all (e.g. no /proc).
    Unsupported,
    /// The platform supports measurement but none was obtained for this
    /// attempt (e.g. the tree exited before the first sample).
    Unknown,
}

/// Issue #116: one resource metric with explicit provenance. `value` is
/// `None` when the metric is unknown; unknown stays unknown and is never
/// coerced to zero (same discipline as `BehaviorMetric`).
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ResourceMetric {
    /// Measured value. CPU time is seconds; peak RSS is bytes. Integral
    /// byte counts stay exact below 2^53.
    #[serde(default)]
    pub value: Option<f64>,
    pub quality: ResourceMetricQuality,
    /// Why the value is unknown. Present whenever `value` is `None`.
    #[serde(default)]
    pub unknown_reason: Option<String>,
}

impl ResourceMetric {
    pub fn measured(value: f64) -> Self {
        ResourceMetric {
            value: Some(value),
            quality: ResourceMetricQuality::Measured,
            unknown_reason: None,
        }
    }

    pub fn unsupported(reason: &str) -> Self {
        ResourceMetric {
            value: None,
            quality: ResourceMetricQuality::Unsupported,
            unknown_reason: Some(reason.to_string()),
        }
    }

    pub fn unknown(reason: &str) -> Self {
        ResourceMetric {
            value: None,
            quality: ResourceMetricQuality::Unknown,
            unknown_reason: Some(reason.to_string()),
        }
    }
}

/// Issue #116: per-attempt process-tree resource usage. Kept separate from
/// `usage` (token/cost/quota attribution) — never mixed into token or cost
/// accounting. `#[serde(default)]` on the holder field keeps historical
/// ledger entries (written before this existed) deserializing.
#[derive(Debug, Serialize, Deserialize, Default, Clone, PartialEq)]
pub struct AttemptResourceUsage {
    #[serde(default)]
    pub cpu_time_seconds: Option<ResourceMetric>,
    #[serde(default)]
    pub peak_rss_bytes: Option<ResourceMetric>,
}

impl AttemptResourceUsage {
    /// Provenance for an attempt that never spawned a backend process.
    pub fn never_launched() -> Self {
        AttemptResourceUsage {
            cpu_time_seconds: Some(ResourceMetric::unknown("backend never launched")),
            peak_rss_bytes: Some(ResourceMetric::unknown("backend never launched")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ledger::test_util::profile;
    use crate::ledger::{AttemptRecord, LedgerEntry, LedgerUsage};

    /// Issue #116: a pre-resource-capture attempt record deserializes with
    /// `resources: None` (unknown, never zero), and a fully-populated record
    /// round-trips through JSON with its provenance intact.
    #[test]
    fn attempt_resources_deserialize_compatibly_and_round_trip() {
        let legacy: AttemptRecord =
            serde_json::from_str(r#"{"attempt_number": 1, "backend": "codex", "usage": {}}"#)
                .unwrap();
        assert!(
            legacy.resources.is_none(),
            "pre-capture attempts must stay unknown, not zero"
        );

        let mut entry = LedgerEntry::new(
            "test-session",
            &profile(),
            "fix",
            "improve",
            "target",
            None,
            None,
        );
        entry.attempts.push(AttemptRecord {
            attempt_number: 1,
            backend: "codex".into(),
            usage: LedgerUsage::default(),
            resources: Some(AttemptResourceUsage {
                cpu_time_seconds: Some(ResourceMetric::measured(12.5)),
                peak_rss_bytes: Some(ResourceMetric::measured(48_000_000.0)),
            }),
            ..AttemptRecord::default()
        });
        entry.attempts.push(AttemptRecord {
            attempt_number: 2,
            backend: "codex".into(),
            usage: LedgerUsage::default(),
            resources: Some(AttemptResourceUsage::never_launched()),
            ..AttemptRecord::default()
        });
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: LedgerEntry = serde_json::from_str(&json).unwrap();
        let first = &parsed.attempts[0];
        let resources = first.resources.as_ref().unwrap();
        let cpu = resources.cpu_time_seconds.as_ref().unwrap();
        assert_eq!(cpu.value, Some(12.5));
        assert_eq!(cpu.quality, ResourceMetricQuality::Measured);
        let rss = resources.peak_rss_bytes.as_ref().unwrap();
        assert_eq!(rss.value, Some(48_000_000.0));
        let second = parsed.attempts[1].resources.as_ref().unwrap();
        let unknown = second.cpu_time_seconds.as_ref().unwrap();
        assert_eq!(unknown.quality, ResourceMetricQuality::Unknown);
        assert_eq!(
            unknown.unknown_reason.as_deref(),
            Some("backend never launched")
        );
        assert!(unknown.value.is_none());
    }
}
