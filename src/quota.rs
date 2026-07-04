#![allow(dead_code)]

use serde::{Deserialize, Serialize};
use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PaceBand {
    AggressiveBurn,
    MildBurn,
    Normal,
    Conserve,
    HardConserve,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PacingConfig {
    #[serde(default = "default_aggressive")]
    pub aggressive: f64,
    #[serde(default = "default_mild")]
    pub mild: f64,
    #[serde(default = "default_conserve")]
    pub conserve: f64,
    #[serde(default = "default_hard_conserve")]
    pub hard_conserve: f64,
}

fn default_aggressive() -> f64 {
    20.0
}

fn default_mild() -> f64 {
    7.0
}

fn default_conserve() -> f64 {
    -7.0
}

fn default_hard_conserve() -> f64 {
    -20.0
}

impl Default for PacingConfig {
    fn default() -> Self {
        Self {
            aggressive: 20.0,
            mild: 7.0,
            conserve: -7.0,
            hard_conserve: -20.0,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum QuotaPacingError {
    InvalidUsage(f64),
    InvalidDays(f64),
}

impl fmt::Display for QuotaPacingError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidUsage(u) => write!(f, "Invalid usage percentage: {}", u),
            Self::InvalidDays(d) => write!(f, "Invalid days remaining: {}", d),
        }
    }
}

impl std::error::Error for QuotaPacingError {}

/// Pure deterministic pacing policy function that computes a pacing band from
/// quota usage, time remaining, and configured thresholds.
pub fn quota_pace(
    usage_pct: Option<f64>,
    days_remaining: Option<f64>,
    config: &PacingConfig,
) -> Result<PaceBand, QuotaPacingError> {
    // 6. Missing usage data handled honestly (returns Normal, not a fabricated aggressive result)
    let usage_pct = match usage_pct {
        Some(u) => u,
        None => return Ok(PaceBand::Normal),
    };
    let days_remaining = match days_remaining {
        Some(d) => d,
        None => return Ok(PaceBand::Normal),
    };

    // 7. Invalid inputs (negative usage, negative days, usage > 100) return explicit error
    if !(0.0..=100.0).contains(&usage_pct) {
        return Err(QuotaPacingError::InvalidUsage(usage_pct));
    }
    if days_remaining < 0.0 {
        return Err(QuotaPacingError::InvalidDays(days_remaining));
    }

    // 3. Target linear pace calculation: target_pct = 100 - (100 / 7) * days_remaining
    let target_pct = 100.0 - (100.0 / 7.0) * days_remaining;

    // 4. Pace delta: actual_pct - target_pct
    // where actual_pct is remaining quota percentage (100 - usage_pct)
    let actual_pct = 100.0 - usage_pct;
    let delta = actual_pct - target_pct;

    // 5. Threshold bands
    if delta >= config.aggressive {
        Ok(PaceBand::AggressiveBurn)
    } else if delta >= config.mild {
        Ok(PaceBand::MildBurn)
    } else if delta <= config.hard_conserve {
        Ok(PaceBand::HardConserve)
    } else if delta <= config.conserve {
        Ok(PaceBand::Conserve)
    } else {
        Ok(PaceBand::Normal)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_default_pacing() {
        let config = PacingConfig::default();

        // Start of week, target is 0% used, we used 0%. Delta is (100 - 0) - 0 = +100.
        // This is >= 20.0, so AggressiveBurn.
        assert_eq!(
            quota_pace(Some(0.0), Some(7.0), &config).unwrap(),
            PaceBand::AggressiveBurn
        );

        // Halfway through the week (3.5 days remaining). Target is 100 - (100/7)*3.5 = 50.0% used.
        // If we used 50%, delta is (100 - 50) - 50 = 0.
        // Between -7 and +7, so Normal.
        assert_eq!(
            quota_pace(Some(50.0), Some(3.5), &config).unwrap(),
            PaceBand::Normal
        );

        // Halfway, used 30% (remaining 70%). Target remaining 50%. Delta = 70 - 50 = +20.
        // >= 20.0, so AggressiveBurn.
        assert_eq!(
            quota_pace(Some(30.0), Some(3.5), &config).unwrap(),
            PaceBand::AggressiveBurn
        );

        // Halfway, used 40% (remaining 60%). Target remaining 50%. Delta = 60 - 50 = +10.
        // >= 7.0 but < 20.0, so MildBurn.
        assert_eq!(
            quota_pace(Some(40.0), Some(3.5), &config).unwrap(),
            PaceBand::MildBurn
        );

        // Halfway, used 60% (remaining 40%). Target remaining 50%. Delta = 40 - 50 = -10.
        // <= -7.0 but > -20.0, so Conserve.
        assert_eq!(
            quota_pace(Some(60.0), Some(3.5), &config).unwrap(),
            PaceBand::Conserve
        );

        // Halfway, used 70% (remaining 30%). Target remaining 50%. Delta = 30 - 50 = -20.
        // <= -20.0, so HardConserve.
        assert_eq!(
            quota_pace(Some(70.0), Some(3.5), &config).unwrap(),
            PaceBand::HardConserve
        );
    }

    #[test]
    fn test_missing_data() {
        let config = PacingConfig::default();
        assert_eq!(
            quota_pace(None, Some(3.5), &config).unwrap(),
            PaceBand::Normal
        );
        assert_eq!(
            quota_pace(Some(50.0), None, &config).unwrap(),
            PaceBand::Normal
        );
        assert_eq!(quota_pace(None, None, &config).unwrap(), PaceBand::Normal);
    }

    #[test]
    fn test_invalid_inputs() {
        let config = PacingConfig::default();
        assert_eq!(
            quota_pace(Some(-1.0), Some(3.5), &config),
            Err(QuotaPacingError::InvalidUsage(-1.0))
        );
        assert_eq!(
            quota_pace(Some(101.0), Some(3.5), &config),
            Err(QuotaPacingError::InvalidUsage(101.0))
        );
        assert_eq!(
            quota_pace(Some(50.0), Some(-0.5), &config),
            Err(QuotaPacingError::InvalidDays(-0.5))
        );
    }

    #[test]
    fn test_configurable_thresholds() {
        let config = PacingConfig {
            aggressive: 10.0,
            mild: 5.0,
            conserve: -5.0,
            hard_conserve: -10.0,
        };

        // Halfway, used 42% (remaining 58%). Target remaining 50%. Delta = 58 - 50 = +8.
        // Under custom config, >= 5.0 but < 10.0 is MildBurn.
        assert_eq!(
            quota_pace(Some(42.0), Some(3.5), &config).unwrap(),
            PaceBand::MildBurn
        );

        // Delta = +11, >= 10.0 is AggressiveBurn.
        assert_eq!(
            quota_pace(Some(39.0), Some(3.5), &config).unwrap(),
            PaceBand::AggressiveBurn
        );
    }
}
