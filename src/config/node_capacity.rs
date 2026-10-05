//! Operator settings for the loop's node capacity admission (issue #1380).
//! Split from `config.rs` to stay under the source-size guard.

use anyhow::Result;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, Clone, Copy)]
#[serde(default)]
pub struct NodeCapacitySettings {
    pub worker_memory_mib: u64,
    /// Zero retains the adaptive floor of max(2048 MiB, total memory / 6).
    pub memory_floor_mib: u64,
}

impl Default for NodeCapacitySettings {
    fn default() -> Self {
        Self {
            worker_memory_mib: 4096,
            memory_floor_mib: 0,
        }
    }
}

impl NodeCapacitySettings {
    pub fn floor_description(self) -> String {
        if self.memory_floor_mib == 0 {
            "max(2048 MiB, total / 6)".into()
        } else {
            format!("{} MiB", self.memory_floor_mib)
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
            self.worker_memory_mib <= u64::MAX / (1024 * 1024)
                && self.memory_floor_mib <= u64::MAX / (1024 * 1024),
            "node_capacity memory value is too large"
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
    fn load_rejects_below_minimum_values() {
        let file = tempfile::NamedTempFile::new().unwrap();
        for body in [
            "[defaults.node_capacity]\nworker_memory_mib = 511\n",
            "[defaults.node_capacity]\nmemory_floor_mib = 511\n",
        ] {
            std::fs::write(file.path(), body).unwrap();
            assert!(load(file.path().to_str()).is_err());
        }
    }
}
