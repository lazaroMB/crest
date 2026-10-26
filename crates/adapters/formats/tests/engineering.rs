//! Engineering viewers on fixture files: a VCD dump.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use adapter_formats::Formats;
use application::ports::formats::{FormatsPort, WaveRequest};
use domain::engineering::waveform::{Radix, SignalKind, WaveFormat};

fn fixture(name: &str) -> Vec<u8> {
    let path = format!(
        "{}/tests/fixtures/engineering/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn wave(signals: &[&str]) -> WaveRequest {
    WaveRequest {
        path: "sim/counter.vcd".into(),
        signals: signals.iter().map(|s| (*s).to_owned()).collect(),
        radix: Radix::Hex,
    }
}

#[test]
fn vcd_signals_come_in_scope_order_with_their_changes() {
    let spec = Formats::new()
        .parse_waveform(&fixture("counter.vcd"), &wave(&[]))
        .unwrap();
    spec.validate().unwrap();
    assert_eq!(spec.format, WaveFormat::Vcd);
    assert_eq!(spec.timescale.label(), "1 ps");
    assert_eq!((spec.start, spec.end), (0, 40_000));
    assert_eq!(spec.total_signals, 5);
    let paths: Vec<String> = spec.signals.iter().map(|s| s.path()).collect();
    assert_eq!(
        paths,
        [
            "tb.clk",
            "tb.rst",
            "tb.dut.count[3:0]",
            "tb.dut.en",
            "tb.dut.vout"
        ]
    );
    let clk = &spec.signals[0];
    assert_eq!(clk.kind, SignalKind::Bit);
    assert_eq!(clk.clock_period(), Some(10_000));
    assert_eq!(spec.time_text(10_000), "10 ns");
    let count = &spec.signals[2];
    assert_eq!((count.kind, count.width), (SignalKind::Vector, 4));
    let values: Vec<&str> = count.changes.iter().map(|c| c.v.as_str()).collect();
    assert_eq!(values, ["0000", "0001", "0010", "1z10"]);
    assert_eq!(count.formatted_at(26_000, Radix::Hex).as_deref(), Some("2"));
    assert_eq!(count.formatted_at(36_000, Radix::Hex).as_deref(), Some("x"));
    assert!(count.has_unknown());
    // `x` then 1 at 10 ns.
    assert_eq!(spec.signals[3].value_at(0), Some("x"));
    assert_eq!(spec.signals[3].value_at(10_000), Some("1"));
    let vout = &spec.signals[4];
    assert_eq!(vout.kind, SignalKind::Real);
    assert_eq!(vout.value_at(15_000), Some("1.25"));
    assert!(spec.generator.as_deref().unwrap_or("").contains("Icarus"));
}

#[test]
fn vcd_patterns_filter_and_bad_input_is_an_error() {
    let formats = Formats::new();
    let spec = formats
        .parse_waveform(&fixture("counter.vcd"), &wave(&["tb.dut"]))
        .unwrap();
    assert_eq!(spec.signals.len(), 3);
    let none = formats.parse_waveform(&fixture("counter.vcd"), &wave(&["nope"]));
    assert!(none.unwrap_err().to_string().contains("no signal matches"));
    let junk = formats.parse_waveform(b"hello", &wave(&[]));
    assert!(junk.is_err());
}
