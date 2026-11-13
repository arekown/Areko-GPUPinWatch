//! Two-stage protection per GPU, driven by the highest 12 V pin current.
//!
//! Stage 1: a pin >= warn for warn_hold      -> throttle the card (power limit down).
//! Stage 2: throttled, after settle a pin still >= crit for crit_hold -> trip (shutdown or alarm only).
//! Holds are measured in time, not samples, so the poll rate does not change the behaviour.
//! A missing/implausible sample resets the running holds (no decision on bad data).
use crate::config::Config;
use crate::model::PINS;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Phase {
    Ok,
    Throttled,
    Alarm,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Decision {
    Nothing,
    /// Lower the power limit of this GPU. pin is 0-based.
    Throttle { pin: usize, amps: f32 },
    /// Stage 2 reached: shut down (or alarm only, per config).
    Trip { pin: usize, amps: f32 },
}

#[derive(Debug, Clone, Default)]
pub struct PinGuard {
    phase: Option<Phase>,
    warn_since: Option<u64>,
    crit_since: Option<u64>,
    throttled_at: u64,
}

fn worst(amps: &[f32; PINS]) -> (usize, f32) {
    amps.iter().copied().enumerate().fold((0, f32::MIN), |b, (i, a)| if a > b.1 { (i, a) } else { b })
}

impl PinGuard {
    pub fn phase(&self) -> Phase {
        self.phase.unwrap_or(Phase::Ok)
    }

    /// Restores a phase after a service restart (e.g. still throttled).
    pub fn resume(phase: Phase, now_ms: u64) -> Self {
        PinGuard { phase: Some(phase), throttled_at: now_ms, ..Default::default() }
    }

    /// One sample. `amps` = None when the source could not be read.
    pub fn step(&mut self, cfg: &Config, now_ms: u64, amps: Option<&[f32; PINS]>) -> Decision {
        let Some(amps) = amps else {
            self.warn_since = None;
            self.crit_since = None;
            return Decision::Nothing;
        };
        let (pin, top) = worst(amps);
        match self.phase() {
            Phase::Ok => {
                if top >= cfg.stage1.warn_a {
                    let since = *self.warn_since.get_or_insert(now_ms);
                    if now_ms - since >= cfg.stage1.hold_ms {
                        self.phase = Some(Phase::Throttled);
                        self.throttled_at = now_ms;
                        self.warn_since = None;
                        return Decision::Throttle { pin, amps: top };
                    }
                } else {
                    self.warn_since = None;
                }
            }
            Phase::Throttled => {
                if now_ms - self.throttled_at < cfg.settle_ms {
                    return Decision::Nothing;
                }
                if top >= cfg.stage2.crit_a {
                    let since = *self.crit_since.get_or_insert(now_ms);
                    if now_ms - since >= cfg.stage2.hold_ms {
                        self.phase = Some(Phase::Alarm);
                        self.crit_since = None;
                        return Decision::Trip { pin, amps: top };
                    }
                } else {
                    self.crit_since = None;
                }
            }
            Phase::Alarm => {}
        }
        Decision::Nothing
    }

    /// User released the throttle (or alarm-only state): back to normal watching.
    pub fn release(&mut self) {
        *self = PinGuard::default();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn amps(top: f32) -> [f32; PINS] {
        [5.0, 5.1, 5.0, top, 4.9, 5.0]
    }

    #[test]
    fn throttles_after_hold_then_trips_after_settle_and_hold() {
        let cfg = Config::default(); // 8.5 A / 1 s, settle 2 s, 9.2 A / 1.5 s
        let mut g = PinGuard::default();
        let mut t = 0;
        let mut step = |g: &mut PinGuard, a: f32| {
            let d = g.step(&cfg, t, Some(&amps(a)));
            t += 500;
            d
        };
        assert_eq!(step(&mut g, 8.6), Decision::Nothing); // t=0
        assert_eq!(step(&mut g, 8.6), Decision::Nothing); // 500
        assert_eq!(step(&mut g, 8.7), Decision::Throttle { pin: 3, amps: 8.7 }); // 1000
        // still above crit right away: nothing during the 2 s settle time
        for _ in 0..4 {
            assert_eq!(step(&mut g, 9.5), Decision::Nothing); // 1500..3000
        }
        assert_eq!(step(&mut g, 9.5), Decision::Nothing); // 3500 hold 0.5 s
        assert_eq!(step(&mut g, 9.5), Decision::Nothing); // 4000 hold 1.0 s
        assert_eq!(step(&mut g, 9.6), Decision::Trip { pin: 3, amps: 9.6 }); // 4500 hold 1.5 s
        assert_eq!(g.phase(), Phase::Alarm);
    }

    #[test]
    fn short_spikes_and_bad_samples_do_not_act() {
        let cfg = Config::default();
        let mut g = PinGuard::default();
        assert_eq!(g.step(&cfg, 0, Some(&amps(8.9))), Decision::Nothing);
        assert_eq!(g.step(&cfg, 500, Some(&amps(7.0))), Decision::Nothing); // dropped below: hold resets
        assert_eq!(g.step(&cfg, 1000, Some(&amps(8.9))), Decision::Nothing);
        assert_eq!(g.step(&cfg, 1500, None), Decision::Nothing); // unreadable: hold resets
        assert_eq!(g.step(&cfg, 2000, Some(&amps(8.9))), Decision::Nothing);
        assert_eq!(g.step(&cfg, 2500, Some(&amps(8.9))), Decision::Nothing);
        assert_eq!(g.phase(), Phase::Ok);
    }
}
