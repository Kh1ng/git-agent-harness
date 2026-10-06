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
    /// Seconds the recurring loop rests after a completed pass before it
    /// reads the provider again. Every pass costs provider API calls in
    /// proportion to the open issues and pull requests, so raise this when
    /// the loop shares one API allowance with other loops or tools.
    #[serde(default = "default_loop_interval_seconds")]
    pub loop_interval_seconds: u64,
}

impl PacingConfig {
    /// The rest between loop passes, kept within 5 seconds to 1 hour so a
    /// typo can neither hammer the provider nor park the loop for a day.
    pub fn loop_interval(&self) -> std::time::Duration {
        std::time::Duration::from_secs(self.loop_interval_seconds.clamp(5, 3600))
    }
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

fn default_loop_interval_seconds() -> u64 {
    30
}

impl Default for PacingConfig {
    fn default() -> Self {
        Self {
            aggressive: 20.0,
            mild: 7.0,
            conserve: -7.0,
            hard_conserve: -20.0,
            loop_interval_seconds: default_loop_interval_seconds(),
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

    // A weekly quota starts with 100% remaining and targets 0% at reset.
    // Compare remaining to remaining: a positive delta is unused headroom.
    let target_remaining_pct = ((100.0 / 7.0) * days_remaining).min(100.0);
    let actual_remaining_pct = 100.0 - usage_pct;
    let delta = actual_remaining_pct - target_remaining_pct;

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

        // At the start of the week, all quota remaining is on pace.
        assert_eq!(
            quota_pace(Some(0.0), Some(7.0), &config).unwrap(),
            PaceBand::Normal
        );

        // Halfway through the week, 50% remaining matches the target.
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
    fn unused_weekly_quota_becomes_more_urgent_as_reset_approaches() {
        let config = PacingConfig::default();
        assert_eq!(
            quota_pace(Some(50.0), Some(7.0), &config).unwrap(),
            PaceBand::HardConserve
        );
        assert_eq!(
            quota_pace(Some(50.0), Some(1.0 / 24.0), &config).unwrap(),
            PaceBand::AggressiveBurn
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
            ..Default::default()
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

#[cfg(test)]
mod loop_interval_tests {
    use super::PacingConfig;
    use std::time::Duration;

    #[test]
    fn an_unconfigured_profile_keeps_the_thirty_second_pass_interval() {
        let pacing: PacingConfig = toml::from_str("").unwrap();
        assert_eq!(pacing.loop_interval(), Duration::from_secs(30));
        assert_eq!(
            PacingConfig::default().loop_interval(),
            Duration::from_secs(30)
        );
    }

    #[test]
    fn the_pass_interval_follows_the_profile_and_stays_within_bounds() {
        let interval = |seconds: u64| {
            toml::from_str::<PacingConfig>(&format!("loop_interval_seconds = {seconds}"))
                .unwrap()
                .loop_interval()
        };
        assert_eq!(interval(120), Duration::from_secs(120));
        assert_eq!(interval(0), Duration::from_secs(5));
        assert_eq!(interval(1_000_000), Duration::from_secs(3600));
    }
}
