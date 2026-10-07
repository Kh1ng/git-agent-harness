//! Operator settings for the loop's node capacity admission (issue #1380).
//! Split from `config.rs` to stay under the source-size guard.

use anyhow::Result;
use serde::{Deserialize, Serialize};

/// MiB per byte unit used by every node-capacity conversion.
pub(crate) const MIB: u64 = 1024 * 1024;
/// Lower bound of the adaptive free-memory floor when `memory_floor_mib`
/// is zero. Lives here so the admission path, the deferral hints, and the
/// Settings copy all read one formula (review of #1383).
pub const ADAPTIVE_FLOOR_MIN_MIB: u64 = 2048;

#[derive(Debug, Deserialize, Serialize, Clone, Copy, PartialEq, Eq)]
#[serde(default)]
pub struct NodeCapacitySettings {
    pub worker_memory_mib: u64,
    /// Zero retains the adaptive floor of max(2048 MiB, total memory / 6).
    pub memory_floor_mib: u64,
    /// CPU cores one implementation, fix, retry, or escalation worker
    /// reserves. Lighter work keeps its fixed fractions.
    pub worker_cpu_cores: u32,
    /// Share of the node's logical CPUs that load plus reservations may
    /// reach before another worker is deferred. Above 100 admits more
    /// workers than cores, which suits agents that mostly wait on a model.
    pub cpu_ceiling_percent: u32,
}

impl Default for NodeCapacitySettings {
    fn default() -> Self {
        Self {
            worker_memory_mib: 4096,
            memory_floor_mib: 0,
            worker_cpu_cores: 2,
            cpu_ceiling_percent: 90,
        }
    }
}

impl NodeCapacitySettings {
    /// Load plus reservations admission tolerates on a node with
    /// `logical_cpus` CPUs; never below one core.
    pub fn cpu_ceiling(self, logical_cpus: usize) -> f64 {
        (logical_cpus.max(1) as f64 * f64::from(self.cpu_ceiling_percent) / 100.0).max(1.0)
    }

    pub fn floor_description(self) -> String {
        if self.memory_floor_mib == 0 {
            format!("max({ADAPTIVE_FLOOR_MIN_MIB} MiB, total / 6)")
        } else {
            format!("{} MiB", self.memory_floor_mib)
        }
    }

    /// Free-memory floor enforced by admission against a node with
    /// `total_memory_bytes` of RAM. The single definition of the adaptive
    /// formula; the controller must not restate it (review of #1383).
    pub fn memory_reserve_bytes(self, total_memory_bytes: u64) -> u64 {
        if self.memory_floor_mib == 0 {
            (ADAPTIVE_FLOOR_MIN_MIB * MIB).max(total_memory_bytes / 6)
        } else {
            self.memory_floor_mib * MIB
        }
    }

    pub fn validate(self) -> Result<()> {
        anyhow::ensure!(
            self.worker_memory_mib >= 512,
            "node_capacity.worker_memory_mib must be at least 512"
        );
        anyhow::ensure!(
            self.memory_floor_mib == 0 || self.memory_floor_mib >= 512,
            "node_capacity.memory_floor_mib must be zero or at least 512"
        );
        anyhow::ensure!(
            self.worker_memory_mib <= u64::MAX / MIB && self.memory_floor_mib <= u64::MAX / MIB,
            "node_capacity memory value is too large"
        );
        anyhow::ensure!(
            (1..=64).contains(&self.worker_cpu_cores),
            "node_capacity.worker_cpu_cores must be between 1 and 64"
        );
        anyhow::ensure!(
            (10..=400).contains(&self.cpu_ceiling_percent),
            "node_capacity.cpu_ceiling_percent must be between 10 and 400"
        );
        Ok(())
    }

    /// Load-time policy (review of #1383): an invalid value must not take
    /// the whole CLI down, because every repair path -- `gah config set`,
    /// `gah status`, the Settings page -- loads the config first, and a
    /// hard-failing load locks the operator out of every way to fix it.
    /// Warn and fall back to the defaults instead; `config::save` and the
    /// `config set` flags still hard-validate new values.
    pub fn sanitized(self) -> Self {
        match self.validate() {
            Ok(()) => self,
            Err(error) => {
                eprintln!(
                    "gah config: ignoring invalid defaults.node_capacity {self:?} ({error:#}); using defaults until the config is fixed"
                );
                Self::default()
            }
        }
    }

    /// Rejects values this node can never satisfy: on an otherwise idle
    /// node, an implementation-class worker needs its reservation plus
    /// the free-memory floor to fit in total memory (review of #1383).
    /// Callers skip the check when the platform does not expose total
    /// memory, so a config written elsewhere still loads.
    pub fn validate_against_node_total(self, total_memory_bytes: u64) -> Result<()> {
        let worker_bytes = self.worker_memory_mib * MIB;
        let floor_bytes = self.memory_reserve_bytes(total_memory_bytes);
        anyhow::ensure!(
            worker_bytes.saturating_add(floor_bytes) <= total_memory_bytes,
            "node_capacity needs {} MiB ({} worker reservation + {} memory floor) but this node only has {} MiB total; implementation workers would be deferred forever",
            self.worker_memory_mib + floor_bytes / MIB,
            self.worker_memory_mib,
            floor_bytes / MIB,
            total_memory_bytes / MIB
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::load;

    #[test]
    fn defaults_and_validation() {
        let defaults = NodeCapacitySettings::default();
        assert_eq!(defaults.worker_memory_mib, 4096);
        assert_eq!(defaults.memory_floor_mib, 0);
        assert!(defaults.validate().is_ok());
        for settings in [
            NodeCapacitySettings {
                worker_memory_mib: 511,
                ..defaults
            },
            NodeCapacitySettings {
                memory_floor_mib: 511,
                ..defaults
            },
        ] {
            assert!(settings.validate().is_err());
        }
    }

    #[test]
    fn cpu_defaults_and_ranges() {
        let defaults = NodeCapacitySettings::default();
        assert_eq!(defaults.worker_cpu_cores, 2);
        assert_eq!(defaults.cpu_ceiling_percent, 90);
        for (cores, percent, valid) in [
            (1, 10, true),
            (64, 400, true),
            (0, 90, false),
            (65, 90, false),
            (2, 9, false),
            (2, 401, false),
        ] {
            let settings = NodeCapacitySettings {
                worker_cpu_cores: cores,
                cpu_ceiling_percent: percent,
                ..defaults
            };
            assert_eq!(
                settings.validate().is_ok(),
                valid,
                "{cores} cores, {percent}%"
            );
        }
    }

    #[test]
    fn cpu_keys_round_trip_and_default_when_absent() {
        let old_config: NodeCapacitySettings = toml::from_str("worker_memory_mib = 1536").unwrap();
        assert_eq!(old_config.worker_cpu_cores, 2);
        assert_eq!(old_config.cpu_ceiling_percent, 90);
        let settings: NodeCapacitySettings =
            toml::from_str("worker_cpu_cores = 4\ncpu_ceiling_percent = 150").unwrap();
        let restored: NodeCapacitySettings =
            toml::from_str(&toml::to_string(&settings).unwrap()).unwrap();
        assert_eq!(restored.worker_cpu_cores, 4);
        assert_eq!(restored.cpu_ceiling_percent, 150);
    }

    #[test]
    fn toml_round_trip() {
        let settings: NodeCapacitySettings =
            toml::from_str("worker_memory_mib = 1536\nmemory_floor_mib = 768").unwrap();
        settings.validate().unwrap();
        let encoded = toml::to_string(&settings).unwrap();
        let restored: NodeCapacitySettings = toml::from_str(&encoded).unwrap();
        assert_eq!(restored.worker_memory_mib, 1536);
        assert_eq!(restored.memory_floor_mib, 768);
    }

    #[test]
    fn load_falls_back_to_defaults_on_invalid_values() {
        let file = tempfile::NamedTempFile::new().unwrap();
        for body in [
            "[defaults.node_capacity]\nworker_memory_mib = 511\n",
            "[defaults.node_capacity]\nmemory_floor_mib = 511\n",
        ] {
            std::fs::write(file.path(), body).unwrap();
            let cfg = load(file.path().to_str()).unwrap();
            assert_eq!(cfg.defaults.node_capacity, NodeCapacitySettings::default());
        }
    }

    #[test]
    fn adaptive_floor_formula_lives_in_one_place() {
        let total = 96 * MIB;
        assert_eq!(
            NodeCapacitySettings::default().memory_reserve_bytes(total),
            ADAPTIVE_FLOOR_MIN_MIB * MIB,
            "small nodes keep the 2048 MiB minimum"
        );
        let big_total = 96 * 1024 * MIB;
        assert_eq!(
            NodeCapacitySettings::default().memory_reserve_bytes(big_total),
            big_total / 6,
            "large nodes keep the total / 6 share"
        );
        assert_eq!(
            NodeCapacitySettings {
                memory_floor_mib: 768,
                ..NodeCapacitySettings::default()
            }
            .memory_reserve_bytes(big_total),
            768 * MIB
        );
    }

    #[test]
    fn node_total_rejects_never_admittable_values() {
        let total = 16 * 1024 * MIB;
        NodeCapacitySettings::default()
            .validate_against_node_total(total)
            .unwrap();
        let oversized = NodeCapacitySettings {
            worker_memory_mib: 40960,
            ..NodeCapacitySettings::default()
        };
        let error = oversized.validate_against_node_total(total).unwrap_err();
        assert!(error.to_string().contains("deferred forever"));
    }
}
