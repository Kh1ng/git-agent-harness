use serde::{Deserialize, Serialize};

/// How far `gah loop` may grow past the profile's baseline worker count
/// (`max_parallel_workers`) and per-model caps (`max_concurrent_per_model`).
///
/// Two independent sources add workers:
///
/// * Automatic (`enabled`): a capped model gets `extra_per_model` more
///   concurrent runs while every fresh quota window of its subscription,
///   the five-hour one included, still has `min_remaining_percent` left. A
///   subscription with no fresh reading never scales. The total stays at or
///   under `max_workers`.
/// * Manual (`boost_workers`): the operator adds workers outright, for one
///   model (`boost_model`) or for every capped model, optionally until
///   `boost_until`. A boost is explicit, so `max_workers` does not limit it.
///
/// Neither source bypasses node capacity admission: memory and CPU still
/// decide whether an extra worker actually starts.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WorkerScaling {
    #[serde(default)]
    pub enabled: bool,
    /// Ceiling on the total worker count reached by automatic scaling.
    /// Unset means twice the baseline.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_workers: Option<u32>,
    #[serde(default = "default_extra_per_model")]
    pub extra_per_model: u32,
    #[serde(default = "default_min_remaining_percent")]
    pub min_remaining_percent: f64,
    #[serde(default, skip_serializing_if = "is_zero")]
    pub boost_workers: u32,
    /// `"{backend}/{model}"`, the `max_concurrent_per_model` key spelling.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boost_model: Option<String>,
    /// RFC 3339 expiry. Unset keeps the boost until it is cleared.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub boost_until: Option<String>,
}

fn default_extra_per_model() -> u32 {
    1
}

fn default_min_remaining_percent() -> f64 {
    50.0
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

impl Default for WorkerScaling {
    fn default() -> Self {
        Self {
            enabled: false,
            max_workers: None,
            extra_per_model: default_extra_per_model(),
            min_remaining_percent: default_min_remaining_percent(),
            boost_workers: 0,
            boost_model: None,
            boost_until: None,
        }
    }
}

impl WorkerScaling {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }

    pub fn clear_boost(&mut self) {
        self.boost_workers = 0;
        self.boost_model = None;
        self.boost_until = None;
    }
}
