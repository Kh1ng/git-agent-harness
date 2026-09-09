//! Host process measurements are independent of provider token, price and quota accounting.
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Best-effort observations, never an assertion of complete lifetime consumption.
/// Linux samples include the root, process group and discovered descendants. CPU
/// is accumulated per process identity; RSS is the maximum simultaneous sum.
/// Sampling can miss short-lived processes and peaks between observations.
#[derive(Debug, Serialize, Deserialize, Clone, PartialEq)]
pub struct ProcessResources {
    pub cpu_seconds: Option<f64>,
    pub peak_rss_bytes: Option<u64>,
    pub source: String,
    pub unknown_reason: Option<String>,
}

impl ProcessResources {
    pub fn unknown(reason: &str) -> Self {
        Self {
            cpu_seconds: None,
            peak_rss_bytes: None,
            source: "unavailable".into(),
            unknown_reason: Some(reason.into()),
        }
    }
}

impl Default for ProcessResources {
    fn default() -> Self {
        Self::unknown("not_recorded")
    }
}

/// CPU sums known attempt observations; RSS takes the largest attempt peak,
/// never a sum of unrelated peaks. Null totals mean no known observations.
#[derive(Debug, Default, Serialize, Clone)]
pub struct AggregatedResources {
    pub cpu_seconds: Option<f64>,
    pub peak_rss_bytes: Option<u64>,
    pub cpu_known_attempts: u64,
    pub rss_known_attempts: u64,
    pub unknown_attempts: u64,
    pub sources: BTreeMap<String, u64>,
    pub unknown_reasons: BTreeMap<String, u64>,
}

impl AggregatedResources {
    pub(crate) fn add(&mut self, resources: &ProcessResources) {
        if let Some(cpu) = resources.cpu_seconds {
            self.cpu_seconds = Some(self.cpu_seconds.unwrap_or(0.0) + cpu);
            self.cpu_known_attempts += 1;
        }
        if let Some(rss) = resources.peak_rss_bytes {
            self.peak_rss_bytes = Some(self.peak_rss_bytes.unwrap_or(0).max(rss));
            self.rss_known_attempts += 1;
        }
        if resources.cpu_seconds.is_none() || resources.peak_rss_bytes.is_none() {
            self.unknown_attempts += 1;
        }
        *self.sources.entry(resources.source.clone()).or_default() += 1;
        if let Some(reason) = &resources.unknown_reason {
            *self.unknown_reasons.entry(reason.clone()).or_default() += 1;
        }
    }
}
