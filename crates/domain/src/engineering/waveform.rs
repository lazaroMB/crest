//! Digital waveforms from simulation dumps (`show_waveform`): VCD, FST
//! and GHW files read by the adapter into signals with their value
//! changes.
//!
//! Times are integer ticks of the file's [`Timescale`]; values are kept
//! as the file wrote them: bit strings most significant bit first (`0`,
//! `1`, `x`, `z` and the nine-valued VHDL states), real numbers and text.
//! Formatting a bus in hex, binary or decimal ([`format_value`]) and the
//! value at a cursor ([`WaveSignal::value_at`]) are pure helpers, so the
//! card and the model's summary show exactly the file's values.

use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::invalid;
use crate::block::BlockError;

/// Signals a block keeps.
pub const MAX_SIGNALS: usize = 2_000;
/// Value changes a block keeps over all its signals.
pub const MAX_CHANGES: usize = 400_000;

const BLOCK: &str = "waveform";

/// The dump format.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum WaveFormat {
    /// Value change dump (IEEE 1364).
    #[default]
    Vcd,
    /// GTKWave's fast signal trace.
    Fst,
    /// GHDL's waveform dump.
    Ghw,
}

impl WaveFormat {
    /// Upper-case name.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Vcd => "VCD",
            Self::Fst => "FST",
            Self::Ghw => "GHW",
        }
    }
}

/// What a signal carries.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum SignalKind {
    /// One bit.
    #[default]
    Bit,
    /// A bus of `width` bits.
    Vector,
    /// A real number.
    Real,
    /// Text (VHDL enumerations, strings).
    Text,
    /// A named event (no value).
    Event,
}

/// How a bus value is written.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Hash, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Radix {
    /// Hexadecimal digits (`3f`); a digit with an unknown bit is `x`.
    #[default]
    Hex,
    /// The bits as written.
    Bin,
    /// Unsigned decimal; unknown when any bit is not 0 or 1.
    Dec,
}

impl Radix {
    /// Short name.
    #[must_use]
    pub fn label(self) -> &'static str {
        match self {
            Self::Hex => "hex",
            Self::Bin => "bin",
            Self::Dec => "dec",
        }
    }
}

/// Length of a tick: `factor` × 10^`exponent` seconds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct Timescale {
    /// 1, 10 or 100 in VCD files.
    pub factor: u32,
    /// Power of ten of the unit: −9 for ns, −12 for ps.
    pub exponent: i8,
}

impl Default for Timescale {
    fn default() -> Self {
        Self {
            factor: 1,
            exponent: -9,
        }
    }
}

impl Timescale {
    /// Seconds per tick.
    #[must_use]
    pub fn seconds(self) -> f64 {
        f64::from(self.factor) * 10f64.powi(i32::from(self.exponent))
    }

    /// The timescale as a VCD writes it: `1 ns`, `10 ps`.
    #[must_use]
    pub fn label(self) -> String {
        let (scale, suffix) = unit_for(10f64.powi(i32::from(self.exponent)));
        let value = f64::from(self.factor) * 10f64.powi(i32::from(self.exponent)) / scale;
        format!("{} {suffix}", trimmed(value, 3))
    }

    /// `ticks` as a time with a unit that suits it (`12.5 ns`, `3 µs`).
    #[must_use]
    pub fn format(self, ticks: f64) -> String {
        let seconds = ticks * self.seconds();
        if seconds == 0.0 {
            return "0".to_owned();
        }
        let (scale, suffix) = unit_for(seconds.abs());
        format!("{} {suffix}", trimmed(seconds / scale, 3))
    }
}

/// The SI unit (scale in seconds and suffix) for a time of `seconds`.
#[must_use]
pub fn unit_for(seconds: f64) -> (f64, &'static str) {
    const UNITS: [(f64, &str); 6] = [
        (1.0, "s"),
        (1e-3, "ms"),
        (1e-6, "µs"),
        (1e-9, "ns"),
        (1e-12, "ps"),
        (1e-15, "fs"),
    ];
    UNITS
        .into_iter()
        .find(|(scale, _)| seconds >= *scale * (1.0 - 1e-9))
        .unwrap_or((1e-15, "fs"))
}

/// `value` with at most `decimals` decimals, trailing zeros dropped.
fn trimmed(value: f64, decimals: usize) -> String {
    let text = format!("{value:.decimals$}");
    if text.contains('.') {
        text.trim_end_matches('0').trim_end_matches('.').to_owned()
    } else {
        text
    }
}

/// One value change.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
pub struct WaveChange {
    /// Time in ticks.
    pub t: u64,
    /// The value: bits (msb first), a real number or text.
    pub v: String,
}

/// One signal with its changes in time order.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WaveSignal {
    /// Name within its scope, with its bit range (`data[7:0]`).
    pub name: String,
    /// Dotted path of its scope (`tb.cpu`), empty at the top.
    #[serde(default)]
    pub scope: String,
    /// What it carries.
    pub kind: SignalKind,
    /// Width in bits (1 for a bit, 0 for real and text).
    pub width: u32,
    /// The changes, ascending in time.
    #[serde(default)]
    pub changes: Vec<WaveChange>,
    /// Whether later changes were dropped to stay under [`MAX_CHANGES`].
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub truncated: bool,
}

impl WaveSignal {
    /// Full dotted path (`tb.cpu.clk`).
    #[must_use]
    pub fn path(&self) -> String {
        if self.scope.is_empty() {
            self.name.clone()
        } else {
            format!("{}.{}", self.scope, self.name)
        }
    }

    /// Index of the change in force at `t` (the last at or before it).
    #[must_use]
    pub fn index_at(&self, t: u64) -> Option<usize> {
        self.changes.partition_point(|c| c.t <= t).checked_sub(1)
    }

    /// The raw value at `t`, if the signal has one yet.
    #[must_use]
    pub fn value_at(&self, t: u64) -> Option<&str> {
        self.index_at(t).map(|ix| self.changes[ix].v.as_str())
    }

    /// The value at `t` written in `radix`.
    #[must_use]
    pub fn formatted_at(&self, t: u64, radix: Radix) -> Option<String> {
        self.value_at(t).map(|v| format_value(v, self.kind, radix))
    }

    /// Number of value changes after the first value.
    #[must_use]
    pub fn toggles(&self) -> usize {
        self.changes.len().saturating_sub(1)
    }

    /// The period of a one-bit clock: the time between rising edges, when
    /// there are at least three and they are evenly spaced.
    #[must_use]
    pub fn clock_period(&self) -> Option<u64> {
        if self.kind != SignalKind::Bit {
            return None;
        }
        let rises: Vec<u64> = self
            .changes
            .windows(2)
            .filter(|w| w[0].v == "0" && w[1].v == "1")
            .map(|w| w[1].t)
            .collect();
        if rises.len() < 3 {
            return None;
        }
        let period = rises[1] - rises[0];
        (period > 0 && rises.windows(2).all(|w| w[1] - w[0] == period)).then_some(period)
    }

    /// Whether any value has a bit that is not 0 or 1.
    #[must_use]
    pub fn has_unknown(&self) -> bool {
        matches!(self.kind, SignalKind::Bit | SignalKind::Vector)
            && self
                .changes
                .iter()
                .any(|c| c.v.chars().any(|b| b != '0' && b != '1'))
    }
}

/// Whether `bits` are all 0 or 1.
fn known(bits: &str) -> bool {
    !bits.is_empty() && bits.chars().all(|b| b == '0' || b == '1')
}

/// `v` (as the file wrote it) in `radix`: bits stay as they are for
/// one-bit signals, text and reals; a bus is grouped into hex digits
/// (`x` for a digit with an unknown bit, `z` when all its bits are high
/// impedance) or written in decimal (`x` when any bit is unknown).
#[must_use]
pub fn format_value(v: &str, kind: SignalKind, radix: Radix) -> String {
    if kind != SignalKind::Vector || radix == Radix::Bin {
        return v.to_owned();
    }
    match radix {
        Radix::Dec => {
            if !known(v) {
                return "x".to_owned();
            }
            if v.len() <= 128 {
                u128::from_str_radix(v, 2).map_or_else(|_| v.to_owned(), |n| n.to_string())
            } else {
                // Too wide for an integer: hex keeps it exact.
                format_value(v, kind, Radix::Hex)
            }
        }
        _ => {
            let pad = (4 - v.len() % 4) % 4;
            let padded: String = std::iter::repeat_n('0', pad).chain(v.chars()).collect();
            let bytes = padded.as_bytes();
            let digits: String = bytes
                .chunks(4)
                .map(|nibble| {
                    let text = std::str::from_utf8(nibble).unwrap_or("x");
                    if known(text) {
                        u8::from_str_radix(text, 2)
                            .ok()
                            .and_then(|d| char::from_digit(u32::from(d), 16))
                            .unwrap_or('x')
                    } else if nibble
                        .iter()
                        .enumerate()
                        .all(|(i, b)| *b == b'z' || *b == b'Z' || (i < pad && *b == b'0'))
                    {
                        'z'
                    } else {
                        'x'
                    }
                })
                .collect();
            let digits = digits.trim_start_matches('0');
            if digits.is_empty() {
                "0".to_owned()
            } else {
                digits.to_owned()
            }
        }
    }
}

/// A waveform block.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, JsonSchema)]
pub struct WaveformSpec {
    /// Title shown above the waves; defaults to the file name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Workspace path of the dump.
    pub path: String,
    /// Its format.
    pub format: WaveFormat,
    /// Length of one tick.
    #[serde(default)]
    pub timescale: Timescale,
    /// First time of the dump, in ticks.
    #[serde(default)]
    pub start: u64,
    /// Last time of the dump, in ticks.
    #[serde(default)]
    pub end: u64,
    /// Signals shown, in hierarchy order.
    #[serde(default)]
    pub signals: Vec<WaveSignal>,
    /// Signals in the file (before any filter or cap).
    #[serde(default)]
    pub total_signals: usize,
    /// How buses are written at first.
    #[serde(default)]
    pub radix: Radix,
    /// Simulator and date from the header, when present.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub generator: Option<String>,
}

impl WaveformSpec {
    /// Checks the time range, the change order and the size caps.
    ///
    /// # Errors
    ///
    /// [`BlockError::Invalid`] naming the first problem.
    pub fn validate(&self) -> Result<(), BlockError> {
        if self.signals.len() > MAX_SIGNALS {
            return Err(invalid(
                BLOCK,
                format!("{} signals; at most {MAX_SIGNALS}", self.signals.len()),
            ));
        }
        if self.start > self.end {
            return Err(invalid(BLOCK, "start is after end"));
        }
        if self.change_count() > MAX_CHANGES {
            return Err(invalid(
                BLOCK,
                format!("more than {MAX_CHANGES} value changes"),
            ));
        }
        if let Some(s) = self
            .signals
            .iter()
            .find(|s| s.changes.windows(2).any(|w| w[0].t > w[1].t))
        {
            return Err(invalid(
                BLOCK,
                format!("changes of {} are out of order", s.path()),
            ));
        }
        Ok(())
    }

    /// Value changes over all signals.
    #[must_use]
    pub fn change_count(&self) -> usize {
        self.signals.iter().map(|s| s.changes.len()).sum()
    }

    /// Whether any signal lost changes to the cap.
    #[must_use]
    pub fn truncated(&self) -> bool {
        self.signals.iter().any(|s| s.truncated)
    }

    /// The distinct scopes in order of first appearance.
    #[must_use]
    pub fn scopes(&self) -> Vec<&str> {
        let mut out: Vec<&str> = Vec::new();
        for s in &self.signals {
            if !out.contains(&s.scope.as_str()) {
                out.push(&s.scope);
            }
        }
        out
    }

    /// `ticks` as a time (`12.5 ns`).
    #[must_use]
    pub fn time_text(&self, ticks: u64) -> String {
        // Tick counts beyond 2^53 lose precision only in the display.
        #[allow(clippy::cast_precision_loss)]
        self.timescale.format(ticks as f64)
    }

    /// Every signal's value at `t` in `radix`: `path = value` lines.
    #[must_use]
    pub fn values_at(&self, t: u64, radix: Radix) -> String {
        let mut out = format!("t = {}\n", self.time_text(t));
        for s in &self.signals {
            let value = s.formatted_at(t, radix).unwrap_or_else(|| "—".to_owned());
            out.push_str(&format!("{} = {value}\n", s.path()));
        }
        out
    }

    /// The changes as CSV: `signal,time,ticks,value` (values as written).
    #[must_use]
    pub fn to_csv(&self) -> String {
        let mut out = String::from("signal,ticks,time,value\n");
        for s in &self.signals {
            for c in &s.changes {
                out.push_str(&format!(
                    "{},{},{},{}\n",
                    csv(&s.path()),
                    c.t,
                    self.time_text(c.t),
                    csv(&c.v)
                ));
            }
        }
        out
    }
}

fn csv(field: &str) -> String {
    if field.contains([',', '"', '\n']) {
        format!("\"{}\"", field.replace('"', "\"\""))
    } else {
        field.to_owned()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bit(name: &str, changes: &[(u64, &str)]) -> WaveSignal {
        WaveSignal {
            name: name.into(),
            scope: "tb".into(),
            kind: SignalKind::Bit,
            width: 1,
            changes: changes
                .iter()
                .map(|(t, v)| WaveChange {
                    t: *t,
                    v: (*v).into(),
                })
                .collect(),
            truncated: false,
        }
    }

    #[test]
    fn values_format_in_each_radix() {
        let v = SignalKind::Vector;
        assert_eq!(format_value("00111111", v, Radix::Hex), "3f");
        assert_eq!(format_value("00111111", v, Radix::Dec), "63");
        assert_eq!(format_value("00111111", v, Radix::Bin), "00111111");
        assert_eq!(format_value("101", v, Radix::Hex), "5");
        assert_eq!(format_value("0000", v, Radix::Hex), "0");
        assert_eq!(format_value("1x0011", v, Radix::Hex), "x3");
        assert_eq!(format_value("zzzz0001", v, Radix::Hex), "z1");
        assert_eq!(format_value("zz", v, Radix::Hex), "z");
        assert_eq!(format_value("1x", v, Radix::Dec), "x");
        assert_eq!(format_value("x", SignalKind::Bit, Radix::Hex), "x");
    }

    #[test]
    fn values_at_a_time_and_clock_period() {
        let clk = bit(
            "clk",
            &[
                (0, "0"),
                (5, "1"),
                (10, "0"),
                (15, "1"),
                (20, "0"),
                (25, "1"),
            ],
        );
        assert_eq!(clk.value_at(0), Some("0"));
        assert_eq!(clk.value_at(7), Some("1"));
        assert_eq!(clk.value_at(25), Some("1"));
        assert_eq!(clk.clock_period(), Some(10));
        assert_eq!(clk.toggles(), 5);
        let late = bit("rst", &[(3, "1")]);
        assert_eq!(late.value_at(2), None);
        assert_eq!(late.clock_period(), None);
    }

    #[test]
    fn timescales_label_and_format_times() {
        let ps = Timescale {
            factor: 10,
            exponent: -12,
        };
        assert_eq!(ps.label(), "10 ps");
        assert_eq!(ps.format(1500.0), "15 ns");
        assert_eq!(Timescale::default().format(2500.0), "2.5 µs");
        assert_eq!(Timescale::default().format(0.0), "0");
    }

    #[test]
    fn validate_rejects_out_of_order_changes() {
        let mut spec = WaveformSpec {
            title: None,
            path: "sim.vcd".into(),
            format: WaveFormat::Vcd,
            timescale: Timescale::default(),
            start: 0,
            end: 30,
            signals: vec![bit("clk", &[(0, "0"), (5, "1")])],
            total_signals: 1,
            radix: Radix::Hex,
            generator: None,
        };
        assert!(spec.validate().is_ok());
        assert_eq!(spec.values_at(6, Radix::Hex), "t = 6 ns\ntb.clk = 1\n");
        spec.signals[0].changes.reverse();
        assert!(spec.validate().is_err());
        assert!(
            spec.to_csv()
                .starts_with("signal,ticks,time,value\ntb.clk,5,5 ns,1\n")
        );
    }
}
