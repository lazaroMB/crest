//! The timeline of a scenario: the current simulated time, play/pause,
//! speed, stepping from event to event and scrubbing, plus the red
//! markers of the failed assertions. Pure, so it is tested without a
//! window; the view only feeds it real time.
//!
//! Playing is **paced by events** ([`Pace`]): simulated time is not mapped
//! linearly to real time, because the events of a scenario come in bursts
//! (a request fans out in microseconds, then nothing happens for a
//! second). At `1×` every stretch between two consecutive moments where
//! something happens lasts between [`MIN_STEP_S`] and [`MAX_STEP_S`] real
//! seconds, so each message can be followed with the eye.

use std::time::Duration;

use domain::sysmodel::trace::EventKind;
use domain::sysmodel::{ScenarioRun, Trace};

/// Playback speeds; `1×` is the paced speed.
pub const SPEEDS: [f32; 7] = [0.1, 0.25, 0.5, 1.0, 2.0, 4.0, 8.0];
/// Real seconds a scenario would last at `1×` if its time ran linearly;
/// each stretch between events gets its share of it, within
/// [`MIN_STEP_S`]..[`MAX_STEP_S`].
pub const PLAY_SECONDS: f32 = 10.0;
/// Shortest real time a stretch between two events lasts at `1×`.
pub const MIN_STEP_S: f64 = 0.8;
/// Longest real time a stretch between two events lasts at `1×`.
pub const MAX_STEP_S: f64 = 3.0;

/// How simulated time maps to real playing time: the moments where
/// something happens (`stops_us`) and the real second each one is reached
/// at `1×` (`stops_s`); linear in between.
#[derive(Debug, Clone, PartialEq)]
pub struct Pace {
    stops_us: Vec<u64>,
    stops_s: Vec<f64>,
}

impl Pace {
    /// Linear: the whole of `until_us` in [`PLAY_SECONDS`].
    #[must_use]
    pub fn linear(until_us: u64) -> Self {
        Self {
            stops_us: vec![0, until_us.max(1)],
            stops_s: vec![0.0, f64::from(PLAY_SECONDS)],
        }
    }

    /// Paced by the events of `trace`: sends, arrivals, handling starts
    /// and ends, failures.
    #[must_use]
    pub fn of(trace: &Trace) -> Self {
        let until = trace.until_us.max(1);
        let mut stops = vec![0, until];
        for event in &trace.events {
            stops.push(event.at_us);
            match &event.kind {
                EventKind::Sent { arrive_us, .. } => stops.push(*arrive_us),
                EventKind::Started { done_us, .. } => stops.push(*done_us),
                _ => {}
            }
        }
        stops.retain(|t| *t <= until);
        stops.sort_unstable();
        stops.dedup();
        let mut real = Vec::with_capacity(stops.len());
        let mut acc = 0.0;
        real.push(acc);
        for pair in stops.windows(2) {
            let share = (pair[1] - pair[0]) as f64 / until as f64 * f64::from(PLAY_SECONDS);
            acc += share.clamp(MIN_STEP_S, MAX_STEP_S);
            real.push(acc);
        }
        Self {
            stops_us: stops,
            stops_s: real,
        }
    }

    /// Real seconds the scenario lasts at `1×`.
    #[must_use]
    pub fn duration_s(&self) -> f64 {
        self.stops_s.last().copied().unwrap_or(0.0)
    }

    /// Simulated time reached after `real_s` seconds at `1×`.
    #[must_use]
    pub fn time_at(&self, real_s: f64) -> u64 {
        let i = self.stops_s.partition_point(|s| *s <= real_s);
        if i == 0 {
            return 0;
        }
        if i >= self.stops_s.len() {
            return self.stops_us.last().copied().unwrap_or(0);
        }
        let (s0, s1) = (self.stops_s[i - 1], self.stops_s[i]);
        let (t0, t1) = (self.stops_us[i - 1], self.stops_us[i]);
        let f = ((real_s - s0) / (s1 - s0)).clamp(0.0, 1.0);
        #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
        let dt = ((t1 - t0) as f64 * f).floor() as u64;
        t0 + dt
    }

    /// Real seconds at `1×` at which simulated time `t_us` is reached.
    #[must_use]
    pub fn real_at(&self, t_us: u64) -> f64 {
        let i = self.stops_us.partition_point(|t| *t <= t_us);
        if i == 0 {
            return 0.0;
        }
        if i >= self.stops_us.len() {
            return self.duration_s();
        }
        let (t0, t1) = (self.stops_us[i - 1], self.stops_us[i]);
        let (s0, s1) = (self.stops_s[i - 1], self.stops_s[i]);
        s0 + (s1 - s0) * (t_us - t0) as f64 / (t1 - t0) as f64
    }
}

/// Where the timeline is.
#[derive(Debug, Clone, PartialEq)]
pub struct Playback {
    /// Simulated time shown.
    pub t_us: u64,
    /// End of the scenario.
    pub until_us: u64,
    /// Whether time advances.
    pub playing: bool,
    speed: usize,
    pace: Pace,
    /// Real seconds played at `1×`.
    real_s: f64,
}

impl Playback {
    /// At the start, paused, at `1×`, with time running linearly.
    #[must_use]
    pub fn new(until_us: u64) -> Self {
        Self::with_pace(until_us, Pace::linear(until_us))
    }

    /// At the start, paused, at `1×`, paced by the events of `trace`.
    #[must_use]
    pub fn for_trace(trace: &Trace) -> Self {
        Self::with_pace(trace.until_us, Pace::of(trace))
    }

    fn with_pace(until_us: u64, pace: Pace) -> Self {
        Self {
            t_us: 0,
            until_us: until_us.max(1),
            playing: false,
            speed: 3,
            pace,
            real_s: 0.0,
        }
    }

    /// Real seconds the scenario lasts at the current speed.
    #[must_use]
    pub fn duration_s(&self) -> f64 {
        self.pace.duration_s() / f64::from(self.speed())
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
            self.seek(0);
        }
        self.playing = !self.playing;
    }

    /// Moves time on by `real` elapsed time; returns whether it moved.
    /// Pauses at the end.
    pub fn advance(&mut self, real: Duration) -> bool {
        if !self.playing {
            return false;
        }
        let end = self.pace.duration_s();
        self.real_s = (self.real_s + real.as_secs_f64() * f64::from(self.speed())).min(end);
        let before = self.t_us;
        self.t_us = self.pace.time_at(self.real_s).min(self.until_us);
        if self.real_s >= end {
            self.t_us = self.until_us;
            self.playing = false;
        }
        self.t_us != before
    }

    /// Jumps to `t_us` (clamped).
    pub fn seek(&mut self, t_us: u64) {
        self.t_us = t_us.min(self.until_us);
        self.real_s = self.pace.real_at(self.t_us);
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
    fn events_get_time_to_be_followed() {
        // A burst at 100..103 µs and a lone event at 9 000 µs.
        let t = trace(&[100, 101, 103, 9_000]);
        let pace = Pace::of(&t);
        // Stops: 0, 100, 101, 103, 9 000, 10 000: five stretches.
        let d = pace.duration_s();
        assert!(d >= 5.0 * MIN_STEP_S - 1e-9, "{d}");
        assert!(d <= 5.0 * MAX_STEP_S + 1e-9, "{d}");
        // The 1 µs stretch of the burst lasts a visible while.
        let a = pace.real_at(100);
        let b = pace.real_at(101);
        assert!(b - a >= MIN_STEP_S - 1e-9, "{a} {b}");
        // time_at and real_at invert each other at the stops.
        for t_us in [0, 100, 101, 103, 9_000, 10_000] {
            assert_eq!(pace.time_at(pace.real_at(t_us)), t_us);
        }
        let mut p = Playback::for_trace(&t);
        p.toggle();
        p.advance(Duration::from_secs_f64(a + 0.01));
        assert_eq!(
            p.t_us, 100,
            "the first second reaches the burst, not past it"
        );
        p.advance(Duration::from_secs(1_000));
        assert_eq!(p.t_us, 10_000);
        assert!(!p.playing);
    }

    #[test]
    fn labels_switch_units() {
        assert_eq!(time_label(1_500), "1.500 ms");
        assert_eq!(time_label(12_000_000), "12.00 s");
    }
}
