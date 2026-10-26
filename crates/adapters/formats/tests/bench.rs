//! Criterion estimates on a fixture `target/criterion` tree.

use adapter_formats::Formats;
use application::ports::formats::{BenchFiles, BenchRequest, FormatsPort};
use domain::devops::bench::Verdict;

fn read(rel: &str) -> Option<Vec<u8>> {
    let path = format!(
        "{}/tests/fixtures/devops/criterion/{rel}",
        env!("CARGO_MANIFEST_DIR")
    );
    std::fs::read(path).ok()
}

fn files(dir: &str) -> BenchFiles {
    BenchFiles {
        dir: dir.to_owned(),
        benchmark: read(&format!("{dir}/new/benchmark.json")),
        new: read(&format!("{dir}/new/estimates.json")),
        base: read(&format!("{dir}/base/estimates.json")),
        change: read(&format!("{dir}/change/estimates.json")),
    }
}

#[test]
fn estimates_are_copied_and_judged_against_the_noise_band() {
    let request = BenchRequest {
        path: "target/criterion".into(),
        baseline: "base".into(),
        benches: vec![
            files("sort/1k"),
            files("fib_20"),
            files("parse"),
            files("gone"),
        ],
    };
    let spec = Formats
        .parse_benchmarks(&request)
        .unwrap_or_else(|e| panic!("{e}"));
    spec.validate().unwrap();
    let ids: Vec<_> = spec.benches.iter().map(|b| b.id.as_str()).collect();
    assert_eq!(ids, ["fib/20", "parse", "sort/1k"]);
    assert_eq!(spec.skipped, ["gone"]);
    let fib = &spec.benches[0];
    assert_eq!(fib.dir, "fib_20");
    assert_eq!(fib.new.mean.point, 25_100.0);
    assert_eq!((fib.new.mean.lo, fib.new.mean.hi), (24_900.0, 25_300.0));
    assert_eq!(fib.new.mean.standard_error, Some(100.0));
    assert_eq!(fib.base.map(|b| b.mean.point), Some(22_000.0));
    let change = fib.change.unwrap_or_else(|| panic!("no change"));
    assert_eq!((change.point, change.lo, change.hi), (0.1409, 0.129, 0.156));
    assert_eq!(fib.median_change.map(|c| c.point), Some(0.1397));
    assert_eq!(fib.verdict, Verdict::Regressed);
    assert_eq!(spec.benches[2].verdict, Verdict::NoChange);
    // A baseline without a change file: the ratio of the point estimates.
    let parse = &spec.benches[1];
    assert_eq!(parse.verdict, Verdict::Unknown);
    assert!(parse.ratio.is_some_and(|r| (r + 0.1695).abs() < 1e-9));
}

#[test]
fn no_results_is_an_error() {
    let request = BenchRequest {
        path: "target/criterion".into(),
        baseline: "base".into(),
        benches: vec![files("report")],
    };
    let err = Formats.parse_benchmarks(&request).unwrap_err().to_string();
    assert!(err.contains("cargo bench"), "{err}");
}
