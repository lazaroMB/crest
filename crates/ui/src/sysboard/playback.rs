//! The timeline of a scenario: the current simulated time, play/pause,
//! speed, stepping from event to event and scrubbing, plus the red
//! markers of the failed assertions. Pure, so it is tested without a
//! window; the view only feeds it real time.

use std::time::Duration;

use domain::sysmodel::{ScenarioRun, Trace};

/// Playback speeds; `1×` plays the whole scenario in [`PLAY_SECONDS`].
pub const SPEEDS: [f32; 6] = [0.1, 0.25, 0.5, 1.0, 2.0, 4.0];
/// Real seconds a scenario lasts at `1×`.
pub const PLAY_SECONDS: f32 = 10.0;

/// Where the timeline is.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Playback {
    /// Simulated time shown.
    pub t_us: u64,
    /// End of the scenario.
    pub until_us: u64,
    /// Whether time advances.
    pub playing: bool,
    speed: usize,
    /// Fraction of a microsecond carried between ticks.
    carry: f64,
}

impl Playback {
    /// At the start, paused, at `1×`.
    #[must_use]
    pub fn new(until_us: u64) -> Self {
        Self {
            t_us: 0,
            until_us: until_us.max(1),
            playing: false,
            speed: 3,
            carry: 0.0,
        }
    }

    /// The speed factor.
    #[must_use]
    pub fn speed(&self) -> f32 {
        SPEEDS[self.speed]
    }

    /// One speed up (stops at the fastest).
    pub fn faster(&mut self) {
        self.speed = (self.speed + 1).min(SPEEDS.len() - 1);
    }

    /// One speed down (stops at the slowest).
    pub fn slower(&mut self) {
        self.speed = self.speed.saturating_sub(1);
    }

    /// Play or pause; playing at the end starts over.
    pub fn toggle(&mut self) {
        if !self.playing && self.t_us >= self.until_us {
            self.t_us = 0;
        }
        self.playing = !self.playing;
        self.carry = 0.0;
    }

    /// Moves time on by `real` elapsed time; returns whether it moved.
    /// Pauses at the end.
    pub fn advance(&mut self, real: Duration) -> bool {
        if !self.playing {
            return false;
        }
        let rate = self.until_us as f64 / f64::from(PLAY_SECONDS) * f64::from(self.speed());
        let step = rate * real.as_secs_f64() + self.carry;
        let whole = (step + 1e-6).floor();
        self.carry = (step - whole).max(0.0);
        let before = self.t_us;
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let whole = whole as u64;
        self.t_us = self.t_us.saturating_add(whole).min(self.until_us);
        if self.t_us >= self.until_us {
            self.playing = false;
        }
        self.t_us != before
    }

    /// Jumps to `t_us` (clamped).
    pub fn seek(&mut self, t_us: u64) {
        self.t_us = t_us.min(self.until_us);
        self.carry = 0.0;
    }

    /// Jumps to `fraction` of the scenario.
    pub fn scrub(&mut self, fraction: f32) {
        let f = if fraction.is_finite() {
            f64::from(fraction.clamp(0.0, 1.0))
        } else {
            0.0
        };
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        self.seek((self.until_us as f64 * f).round() as u64);
    }

    /// Jumps to the next event of `trace` (or the end); pauses.
    pub fn step_forward(&mut self, trace: &Trace) {
        self.playing = false;
        self.seek(trace.next_event_after(self.t_us).unwrap_or(self.until_us));
    }

    /// Jumps to the previous event of `trace` (or the start); pauses.
    pub fn step_back(&mut self, trace: &Trace) {
        self.playing = false;
        self.seek(trace.prev_event_before(self.t_us).unwrap_or(0));
    }

    /// How far along, `0..=1`.
    #[must_use]
    pub fn fraction(&self) -> f32 {
        (self.t_us as f64 / self.until_us as f64) as f32
    }
}

/// A failed assertion on the timeline.
#[derive(Debug, Clone, PartialEq)]
pub struct Marker {
    /// Where, `0..=1`.
    pub fraction: f32,
    /// When.
    pub at_us: u64,
    /// The assertion.
    pub label: String,
}

/// One marker per failed verdict with a counterexample.
#[must_use]
pub fn failure_markers(run: &ScenarioRun) -> Vec<Marker> {
    let until = run.trace.until_us.max(1);
    run.failures()
        .filter_map(|v| {
            let cx = v.counterexample.as_ref()?;
            Some(Marker {
                fraction: (cx.at_us.min(until) as f64 / until as f64) as f32,
                at_us: cx.at_us,
                label: v.property.clone(),
            })
        })
        .collect()
}

/// `12.345 ms` / `1.2 s`.
#[must_use]
pub fn time_label(t_us: u64) -> String {
    if t_us >= 10_000_000 {
        format!("{:.2} s", t_us as f64 / 1e6)
    } else {
        format!("{:.3} ms", t_us as f64 / 1000.0)
    }
}

#[cfg(test)]
mod tests {
    use domain::sysmodel::trace::{EventKind, Snapshot, TraceEvent};

    use super::*;

    fn trace(times: &[u64]) -> Trace {
        Trace {
            scenario: "s".into(),
            seed: 0,
            until_us: 10_000,
            model_hash: String::new(),
            initial: Snapshot::default(),
            events: times
                .iter()
                .map(|&t| TraceEvent {
                    at_us: t,
                    kind: EventKind::Killed {
                        comp: "x".into(),
                        lost: 0,
                    },
                })
                .collect(),
            checkpoints: Vec::new(),
            truncated: false,
            error: None,
        }
    }

    #[test]
    fn playing_advances_at_the_speed_and_stops_at_the_end() {
        let mut p = Playback::new(10_000);
        assert!(!p.advance(Duration::from_secs(1)), "paused");
        p.toggle();
        assert!(p.advance(Duration::from_secs(1)));
        assert_eq!(p.t_us, 1_000);
        p.faster();
        assert_eq!(p.speed(), 2.0);
        p.advance(Duration::from_secs(1));
        assert_eq!(p.t_us, 3_000);
        p.advance(Duration::from_secs(100));
        assert_eq!(p.t_us, 10_000);
        assert!(!p.playing);
        p.toggle();
        assert_eq!(p.t_us, 0, "playing again starts over");
        for _ in 0..10 {
            p.slower();
        }
        assert_eq!(p.speed(), SPEEDS[0]);
    }

    #[test]
    fn small_ticks_accumulate() {
        let mut p = Playback::new(1_000);
        p.toggle();
        for _ in 0..100 {
            p.advance(Duration::from_micros(1_000));
        }
        // 1 000 µs over 10 s at 1×: 100 ms of real time is 10 µs.
        assert_eq!(p.t_us, 10);
    }

    #[test]
    fn scrub_and_step_between_events() {
        let t = trace(&[100, 100, 2_500, 7_000]);
        let mut p = Playback::new(10_000);
        p.scrub(0.5);
        assert_eq!(p.t_us, 5_000);
        assert!((p.fraction() - 0.5).abs() < 1e-6);
        p.step_forward(&t);
        assert_eq!(p.t_us, 7_000);
        p.step_forward(&t);
        assert_eq!(p.t_us, 10_000);
        p.step_back(&t);
        assert_eq!(p.t_us, 7_000);
        p.step_back(&t);
        assert_eq!(p.t_us, 2_500);
        p.step_back(&t);
        p.step_back(&t);
        assert_eq!(p.t_us, 0);
        p.scrub(f32::NAN);
        assert_eq!(p.t_us, 0);
        p.scrub(7.0);
        assert_eq!(p.t_us, 10_000);
    }

    #[test]
    fn labels_switch_units() {
        assert_eq!(time_label(1_500), "1.500 ms");
        assert_eq!(time_label(12_000_000), "12.00 s");
    }
}
