//! Engineering viewers on fixture files: a VCD dump, a SPICE deck and a
//! KiCad netlist.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use adapter_formats::Formats;
use application::ports::formats::{FormatsPort, WaveRequest};
use domain::engineering::netlist::NetlistFormat;
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

#[test]
fn spice_decks_read_elements_pins_and_subcircuits() {
    let text = String::from_utf8(fixture("amp.cir")).unwrap();
    let spec = Formats::new()
        .parse_netlist(&text, None, Some("amp.cir"))
        .unwrap();
    spec.validate().unwrap();
    assert_eq!(spec.format, NetlistFormat::Spice);
    assert_eq!(
        spec.design.as_deref(),
        Some("Common-emitter amplifier with an RC filter")
    );
    let refs: Vec<&str> = spec
        .components
        .iter()
        .map(|c| c.reference.as_str())
        .collect();
    assert_eq!(
        refs,
        [
            "V1", "Vin", "C1", "R1", "R2", "Q1", "RC", "RE", "CE", "X1", "M1", "Rg", "Rd", "Rx"
        ]
    );
    let q1 = &spec.components[5];
    assert_eq!(
        (q1.kind.as_str(), q1.model.as_deref()),
        ("bjt", Some("2N3904"))
    );
    let pins: Vec<(&str, &str)> = q1
        .pins
        .iter()
        .map(|p| (p.pin.as_str(), p.net.as_str()))
        .collect();
    assert_eq!(pins, [("C", "c"), ("B", "b"), ("E", "e")]);
    // The continuation line carries CE's value.
    assert_eq!(spec.components[8].value.as_deref(), Some("100u"));
    assert_eq!(spec.components[1].value.as_deref(), Some("SIN(0 10m 1k)"));
    // X1 names its pins after the subcircuit's ports.
    let x1 = &spec.components[9];
    assert_eq!(x1.model.as_deref(), Some("rcfilt"));
    let ports: Vec<&str> = x1.pins.iter().map(|p| p.pin.as_str()).collect();
    assert_eq!(ports, ["a", "y", "gnd"]);
    assert_eq!(x1.value.as_deref(), Some("R=1k"));
    let m1 = &spec.components[10];
    assert_eq!(m1.pins.len(), 4);
    assert_eq!(m1.value.as_deref(), Some("W=1u L=180n"));
    assert_eq!(spec.subcircuits.len(), 1);
    assert_eq!(spec.subcircuits[0].ports, ["a", "y", "gnd"]);
    assert_eq!(spec.subcircuits[0].elements, 2);
    let single: Vec<String> = spec.single_pin_nets().into_iter().map(|n| n.name).collect();
    assert_eq!(single, ["float", "out"]);
    assert!(spec.warnings.iter().any(|w| w.contains(".include")));
    assert_eq!(spec.nets()[0].name, "0");
}

#[test]
fn kicad_netlists_read_components_and_nets() {
    let text = String::from_utf8(fixture("board.net")).unwrap();
    let spec = Formats::new()
        .parse_netlist(&text, None, Some("board.net"))
        .unwrap();
    spec.validate().unwrap();
    assert_eq!(spec.format, NetlistFormat::Kicad);
    assert_eq!(spec.design.as_deref(), Some("/home/u/led/led.kicad_sch"));
    assert_eq!(spec.components.len(), 3);
    let d1 = &spec.components[0];
    assert_eq!(d1.kind, "Device:LED");
    assert_eq!(d1.model.as_deref(), Some("LED_SMD:LED_0603_1608Metric"));
    let pins: Vec<(&str, &str, Option<&str>)> = d1
        .pins
        .iter()
        .map(|p| (p.pin.as_str(), p.net.as_str(), p.function.as_deref()))
        .collect();
    assert_eq!(
        pins,
        [("1", "GND", Some("K")), ("2", "Net-(D1-A)", Some("A"))]
    );
    let single: Vec<String> = spec.single_pin_nets().into_iter().map(|n| n.name).collect();
    assert_eq!(single, ["unconnected-(J1-Pin_3-Pad3)"]);
    assert_eq!(spec.nets()[0].name, "GND");
    let graph = spec.to_graph();
    assert_eq!(graph.edges.len(), 7);
    assert!(
        Formats::new()
            .parse_netlist("(export (a", None, None)
            .is_err()
    );
    assert!(
        Formats::new()
            .parse_netlist("title only\n.end\n", None, None)
            .is_err()
    );
}
