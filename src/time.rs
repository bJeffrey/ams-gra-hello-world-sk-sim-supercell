//! Scenario-time primitives and pacing helpers.

use std::time::{Duration, Instant};

use anyhow::{Result, bail};
use time::OffsetDateTime;

/// Runtime mode for the scenario clock.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum TimeMode {
    /// Scenario time advances at the same rate as wall time.
    Realtime,
    /// Scenario time advances at `rate` scenario seconds per wall second.
    Scaled {
        /// Scenario seconds per wall second.
        rate: f64,
    },
    /// Scenario time advances by fixed ticks without wall pacing.
    Unpaced,
    /// Scenario time advances only when explicitly stepped.
    Stepped,
}

impl TimeMode {
    /// Return the configured scenario-to-wall rate for modes that have one.
    pub fn rate(self) -> Option<f64> {
        match self {
            Self::Realtime => Some(1.0),
            Self::Scaled { rate } => Some(rate),
            Self::Unpaced | Self::Stepped => None,
        }
    }
}

/// Scenario clock advanced explicitly by the simulation loop.
#[derive(Debug, Clone)]
pub struct ScenarioClock {
    mode: TimeMode,
    epoch: OffsetDateTime,
    scenario_elapsed: Duration,
    monotonic_anchor: Instant,
}

impl ScenarioClock {
    /// Create a scenario clock at `epoch` with zero scenario elapsed time.
    pub fn new(mode: TimeMode, epoch: OffsetDateTime) -> Self {
        Self {
            mode,
            epoch,
            scenario_elapsed: Duration::ZERO,
            monotonic_anchor: Instant::now(),
        }
    }

    /// Return the current authoritative scenario timestamp.
    pub fn now(&self) -> OffsetDateTime {
        self.epoch + self.scenario_elapsed
    }

    /// Return scenario elapsed time since the configured epoch.
    pub fn elapsed(&self) -> Duration {
        self.scenario_elapsed
    }

    /// Advance scenario time by an exact simulation duration.
    pub fn advance(&mut self, dt: Duration) {
        self.scenario_elapsed += dt;
    }

    /// Reset the scenario epoch and elapsed time.
    pub fn reset(&mut self, epoch: OffsetDateTime) {
        self.epoch = epoch;
        self.scenario_elapsed = Duration::ZERO;
        self.monotonic_anchor = Instant::now();
    }

    /// Return the wall pacing period for one simulation step.
    pub fn wall_period_for(&self, simulation_dt: Duration) -> Option<Duration> {
        match self.mode {
            TimeMode::Realtime => Some(simulation_dt),
            TimeMode::Scaled { rate } => {
                Some(Duration::from_secs_f64(simulation_dt.as_secs_f64() / rate))
            }
            TimeMode::Unpaced | TimeMode::Stepped => None,
        }
    }

    /// Return whether the clock is configured for unpaced execution.
    pub fn is_unpaced(&self) -> bool {
        self.mode == TimeMode::Unpaced
    }

    /// Return whether the clock is configured for externally stepped execution.
    pub fn is_stepped(&self) -> bool {
        self.mode == TimeMode::Stepped
    }

    /// Return the monotonic instant at which this clock was created or reset.
    pub fn monotonic_anchor(&self) -> Instant {
        self.monotonic_anchor
    }
}

/// Validate a scenario-time rate.
pub fn validate_time_rate(rate: f64) -> Result<()> {
    if rate > 0.0 && rate.is_finite() {
        return Ok(());
    }

    bail!("invalid time.rate={rate}; expected a positive finite value")
}

/// Validate a simulation integration rate.
pub fn validate_simulation_hz(simulation_hz: f64) -> Result<()> {
    if simulation_hz > 0.0 && simulation_hz.is_finite() {
        return Ok(());
    }

    bail!("invalid time.simulation_hz={simulation_hz}; expected a positive finite value")
}

#[cfg(test)]
mod tests {
    use super::*;
    use time::macros::datetime;

    #[test]
    fn advance_moves_scenario_time_exactly() {
        let epoch = datetime!(2026-01-01 0:00 UTC);
        let mut clock = ScenarioClock::new(TimeMode::Realtime, epoch);

        clock.advance(Duration::from_millis(100));

        assert_eq!(clock.elapsed(), Duration::from_millis(100));
        assert_eq!(clock.now(), epoch + Duration::from_millis(100));
    }

    #[test]
    fn scaled_wall_period_divides_by_rate() {
        let clock = ScenarioClock::new(
            TimeMode::Scaled { rate: 10.0 },
            datetime!(2026-01-01 0:00 UTC),
        );

        assert_eq!(
            clock.wall_period_for(Duration::from_millis(100)),
            Some(Duration::from_millis(10))
        );
    }

    #[test]
    fn unpaced_and_stepped_have_no_wall_period() {
        let dt = Duration::from_millis(100);
        let unpaced = ScenarioClock::new(TimeMode::Unpaced, datetime!(2026-01-01 0:00 UTC));
        let stepped = ScenarioClock::new(TimeMode::Stepped, datetime!(2026-01-01 0:00 UTC));

        assert_eq!(unpaced.wall_period_for(dt), None);
        assert_eq!(stepped.wall_period_for(dt), None);
    }

    #[test]
    fn validation_rejects_non_positive_and_non_finite_values() {
        assert!(validate_time_rate(1.0).is_ok());
        assert!(validate_simulation_hz(100.0).is_ok());
        assert!(validate_time_rate(0.0).is_err());
        assert!(validate_time_rate(f64::INFINITY).is_err());
        assert!(validate_simulation_hz(-1.0).is_err());
        assert!(validate_simulation_hz(f64::NAN).is_err());
    }
}
