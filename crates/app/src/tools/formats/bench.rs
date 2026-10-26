//! `show_benchmarks`: criterion results, baseline against new, as a
//! table with a forest plot of the changes.
//!
//! The tool walks the criterion directory through the guarded file port
//! ([`collect_benchmarks`]): a directory with a `new/` child is a
//! benchmark, others (except `report/`) are groups to descend into. The
//! model gets the counts per verdict and the regressed and improved
//! benchmarks with criterion's own numbers, marked as read from the
//! files.

use std::fmt::Write as _;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use async_trait::async_trait;
use domain::block::BlockKind;
use domain::devops::bench::{BenchSpec, Estimate, MAX_BENCHES, Verdict, format_change, format_ns};
use domain::permission::ToolEffect;
use domain::workspace::WorkspacePath;
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;

use super::{FormatToolPorts, MAX_LISTED, invalid, one_line};
use crate::ports::formats::{BenchFiles, BenchRequest};
use crate::ports::fs::FileSystemPort;
use crate::tools::registry::{PreparedCall, ToolContext, ToolError, ToolOutput, TypedTool};
use crate::usecases::compute::offloaded;

/// Where `cargo bench` writes criterion's results.
pub const DEFAULT_CRITERION_DIR: &str = "target/criterion";
/// Criterion's default baseline (the previous run).
pub const DEFAULT_BASELINE: &str = "base";
/// Deepest group nesting searched for benchmarks.
pub const MAX_BENCH_DEPTH: usize = 6;
/// Largest estimates file read, in bytes.
const MAX_ESTIMATES_BYTES: u64 = 1024 * 1024;
/// Directories listed at most while searching.
const MAX_LISTINGS: usize = 5_000;

/// Input of `show_benchmarks`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowBenchmarks {
    /// The criterion output directory; `target/criterion` by default (or
    /// one benchmark group inside it, e.g. `target/criterion/parse`).
    #[serde(default)]
    pub path: Option<String>,
    /// The baseline to compare against: `base` (the previous run, the
    /// default) or a name saved with `--save-baseline`. Criterion's change
    /// estimates (with confidence intervals) exist for `base`; for another
    /// name only the ratio of the means is shown.
    #[serde(default)]
    pub baseline: Option<String>,
    /// Keep only benchmarks whose directory contains this text.
    #[serde(default)]
    pub filter: Option<String>,
    /// Title shown above the table; defaults to the directory.
    #[serde(default)]
    pub title: Option<String>,
}

/// `show_benchmarks`: criterion results as a comparison table.
#[derive(Clone)]
pub struct ShowBenchmarksTool {
    ports: FormatToolPorts,
}

impl ShowBenchmarksTool {
    /// The tool over `ports`.
    #[must_use]
    pub fn new(ports: FormatToolPorts) -> Self {
        Self { ports }
    }
}

/// Name of `show_benchmarks`.
pub const SHOW_BENCHMARKS: &str = "show_benchmarks";

fn join(base: &str, name: &str) -> String {
    if base.is_empty() {
        name.to_owned()
    } else {
        format!("{base}/{name}")
    }
}

async fn read_optional(fs: &dyn FileSystemPort, path: &str) -> Option<Vec<u8>> {
    let path = WorkspacePath::parse(path).ok()?;
    fs.read_bytes(&path, MAX_ESTIMATES_BYTES).await.ok()
}

struct Walk<'a> {
    fs: &'a dyn FileSystemPort,
    root: String,
    baseline: String,
    filter: Option<String>,
    listings: usize,
    benches: Vec<BenchFiles>,
}

impl Walk<'_> {
    /// Searches `rel` (relative to the root) for benchmark directories.
    fn visit<'b>(
        &'b mut self,
        rel: String,
        depth: usize,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + 'b>> {
        Box::pin(async move {
            if self.listings >= MAX_LISTINGS || self.benches.len() >= MAX_BENCHES {
                return Ok(());
            }
            self.listings += 1;
            let dir = join(&self.root, &rel);
            let parsed =
                WorkspacePath::parse(&dir).map_err(|e| format!("criterion path `{dir}`: {e}"))?;
            let entries = match self.fs.list_dir(&parsed).await {
                Ok(entries) => entries,
                // The root must be there; a vanished subdirectory is skipped.
                Err(e) if depth == 0 => return Err(format!("cannot list `{dir}`: {e}")),
                Err(_) => return Ok(()),
            };
            if entries.iter().any(|e| e.is_dir && e.name == "new") {
                if self
                    .filter
                    .as_ref()
                    .is_none_or(|f| rel.contains(f.as_str()))
                {
                    let at = |file: &str| join(&dir, file);
                    let change = if self.baseline == DEFAULT_BASELINE {
                        read_optional(self.fs, &at("change/estimates.json")).await
                    } else {
                        None
                    };
                    self.benches.push(BenchFiles {
                        dir: if rel.is_empty() {
                            dir.rsplit('/').next().unwrap_or(&dir).to_owned()
                        } else {
                            rel.clone()
                        },
                        benchmark: read_optional(self.fs, &at("new/benchmark.json")).await,
                        new: read_optional(self.fs, &at("new/estimates.json")).await,
                        base: read_optional(
                            self.fs,
                            &at(&format!("{}/estimates.json", self.baseline)),
                        )
                        .await,
                        change,
                    });
                }
                return Ok(());
            }
            if depth >= MAX_BENCH_DEPTH {
                return Ok(());
            }
            for entry in entries.iter().filter(|e| e.is_dir && e.name != "report") {
                self.visit(join(&rel, &entry.name), depth + 1).await?;
            }
            Ok(())
        })
    }
}

/// Finds the benchmarks under criterion directory `path` and reads their
/// estimate files (baseline `baseline`; criterion's change estimates
/// only for `base`), keeping those whose directory contains `filter`.
///
/// # Errors
///
/// A message for a bad path or a directory that cannot be listed.
pub async fn collect_benchmarks(
    fs: &dyn FileSystemPort,
    path: &str,
    baseline: &str,
    filter: Option<&str>,
) -> Result<BenchRequest, String> {
    let root = path.trim().trim_end_matches('/').to_owned();
    let mut walk = Walk {
        fs,
        root: root.clone(),
        baseline: baseline.to_owned(),
        filter: filter.map(str::to_owned),
        listings: 0,
        benches: Vec::new(),
    };
    walk.visit(String::new(), 0).await?;
    Ok(BenchRequest {
        path: root,
        baseline: baseline.to_owned(),
        benches: walk.benches,
    })
}

#[async_trait]
impl TypedTool for ShowBenchmarksTool {
    type Input = ShowBenchmarks;

    fn name(&self) -> &str {
        SHOW_BENCHMARKS
    }

    fn description(&self) -> &str {
        "Shows criterion benchmark results (`target/criterion` after \
         `cargo bench`) in the user's timeline: per benchmark the new and \
         baseline mean with confidence intervals, the change in % with its \
         interval and a regressed / improved / no change verdict, plus a \
         forest plot of the changes. The numbers are criterion's own \
         estimates read from its files. Returns the counts per verdict and \
         the regressed and improved benchmarks with those numbers: quote \
         them, do not compute your own."
    }

    fn effect(&self, _input: &ShowBenchmarks) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: ShowBenchmarks,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let path = input
            .path
            .filter(|p| !p.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_CRITERION_DIR.to_owned());
        let baseline = input
            .baseline
            .filter(|b| !b.trim().is_empty())
            .unwrap_or_else(|| DEFAULT_BASELINE.to_owned());
        if baseline.contains(['/', '\\']) || baseline == "new" || baseline == "change" {
            return Err(invalid(
                SHOW_BENCHMARKS,
                "`baseline` is a baseline name such as `base`, not a path",
            ));
        }
        let ports = self.ports.clone();
        let subject = path.clone();
        let filter = input.filter.filter(|f| !f.trim().is_empty());
        let title = input.title.filter(|t| !t.trim().is_empty());
        let run = async move {
            let request =
                match collect_benchmarks(ports.fs.as_ref(), &path, &baseline, filter.as_deref())
                    .await
                {
                    Ok(request) => request,
                    Err(message) => return Ok(ToolOutput::error(message)),
                };
            let formats = Arc::clone(&ports.formats);
            let parsed =
                offloaded(&ports.offload, move || formats.parse_benchmarks(&request)).await;
            let mut spec = match parsed {
                Some(Ok(spec)) => spec,
                Some(Err(err)) => return Ok(ToolOutput::error(err.to_string())),
                None => return Ok(ToolOutput::error("the benchmark reader did not run")),
            };
            spec.title = title;
            if let Err(err) = spec.validate() {
                return Ok(ToolOutput::error(format!("`{path}`: {err}")));
            }
            let summary = bench_summary(&spec);
            Ok(ToolOutput::text(summary).with_block_update(BlockKind::Benchmarks(Box::new(spec))))
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

fn interval_ns(e: &Estimate) -> String {
    format!(
        "{} [{}, {}]",
        format_ns(e.point),
        format_ns(e.lo),
        format_ns(e.hi)
    )
}

fn interval_change(e: &Estimate) -> String {
    format!(
        "{} [{}, {}]",
        format_change(e.point),
        format_change(e.lo),
        format_change(e.hi)
    )
}

/// What the model gets back from `show_benchmarks`.
#[must_use]
pub fn bench_summary(spec: &BenchSpec) -> String {
    let count = |v| spec.with_verdict(v).len();
    let mut out = format!(
        "Criterion results in `{}` (baseline `{}` vs `new`) shown to the user: {} benchmarks, \
         {} regressed, {} improved, {} within noise, {} no change",
        spec.path,
        spec.baseline,
        spec.benches.len(),
        count(Verdict::Regressed),
        count(Verdict::Improved),
        count(Verdict::WithinNoise),
        count(Verdict::NoChange),
    );
    let unknown = count(Verdict::Unknown);
    if unknown > 0 {
        let _ = write!(out, ", {unknown} without a change estimate");
    }
    let _ = write!(
        out,
        ". The times and changes are criterion's own estimates read from its estimates.json \
         files (95% intervals), not computed here; the verdict compares the interval of the \
         mean change with criterion's ±{}% noise threshold (its p-value is not stored).",
        spec.noise_threshold * 100.0
    );
    for (verdict, head) in [
        (Verdict::Regressed, "Regressed"),
        (Verdict::Improved, "Improved"),
    ] {
        let ixs = spec.with_verdict(verdict);
        if ixs.is_empty() {
            continue;
        }
        let _ = write!(out, "\n{head}:");
        for &ix in ixs.iter().take(MAX_LISTED) {
            let b = &spec.benches[ix];
            let _ = write!(
                out,
                "\n- {}: mean {}",
                one_line(&b.id, 120),
                interval_ns(&b.new.mean)
            );
            if let Some(base) = &b.base {
                let _ = write!(out, " vs {}", interval_ns(&base.mean));
            }
            if let Some(change) = &b.change {
                let _ = write!(out, ", change {}", interval_change(change));
            }
        }
        if ixs.len() > MAX_LISTED {
            let _ = write!(out, "\n… and {} more", ixs.len() - MAX_LISTED);
        }
    }
    let ratios: Vec<String> = spec
        .benches
        .iter()
        .filter_map(|b| {
            let ratio = b.ratio?;
            Some(format!(
                "{}: mean {} vs {} ({} from the point estimates, no interval)",
                one_line(&b.id, 120),
                format_ns(b.new.mean.point),
                format_ns(b.base.as_ref()?.mean.point),
                format_change(ratio)
            ))
        })
        .take(MAX_LISTED)
        .collect();
    if !ratios.is_empty() {
        let _ = write!(
            out,
            "\nWithout change estimates:\n- {}",
            ratios.join("\n- ")
        );
    }
    if !spec.skipped.is_empty() {
        let _ = write!(
            out,
            "\nSkipped (no readable new/estimates.json): {}",
            spec.skipped
                .iter()
                .take(MAX_LISTED)
                .cloned()
                .collect::<Vec<_>>()
                .join(", ")
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use domain::conversation::{SessionId, ToolUseId, TurnId};
    use serde_json::{Value, json};

    use super::*;
    use crate::testing::formats::FakeFormats;
    use crate::testing::fs::{InMemoryFs, test_root};
    use crate::tools::registry::{Tool, Typed};
    use crate::usecases::compute::inline_offload;

    fn ctx() -> ToolContext {
        ToolContext::new(SessionId::new(), TurnId::new(), ToolUseId::new("toolu_1"))
    }

    fn fs() -> InMemoryFs {
        InMemoryFs::new(test_root())
            .with_file("target/criterion/fib_20/new/estimates.json", "new")
            .with_file("target/criterion/fib_20/new/benchmark.json", "id")
            .with_file("target/criterion/fib_20/base/estimates.json", "base")
            .with_file("target/criterion/fib_20/change/estimates.json", "change")
            .with_file("target/criterion/sort/1k/new/estimates.json", "new")
            .with_file("target/criterion/sort/1k/main/estimates.json", "main")
            .with_file("target/criterion/sort/report/index.html", "")
            .with_file("target/criterion/report/index.html", "")
    }

    fn ports(formats: Arc<FakeFormats>) -> FormatToolPorts {
        FormatToolPorts {
            formats,
            fs: Arc::new(fs()),
            offload: inline_offload(),
        }
    }

    async fn call(tool: &dyn Tool, input: Value) -> Result<ToolOutput, ToolError> {
        let prepared = tool.prepare(input, &ctx()).await?;
        prepared.run.await
    }

    #[tokio::test]
    async fn the_walk_finds_benchmarks_and_their_files() {
        let fs = fs();
        let request = collect_benchmarks(&fs, "target/criterion/", "base", None)
            .await
            .unwrap();
        assert_eq!(request.path, "target/criterion");
        let found: Vec<_> = request
            .benches
            .iter()
            .map(|b| {
                (
                    b.dir.as_str(),
                    b.new.is_some(),
                    b.benchmark.is_some(),
                    b.base.is_some(),
                    b.change.is_some(),
                )
            })
            .collect();
        assert_eq!(
            found,
            [
                ("fib_20", true, true, true, true),
                ("sort/1k", true, false, false, false)
            ]
        );
        let main = collect_benchmarks(&fs, "target/criterion", "main", Some("sort"))
            .await
            .unwrap();
        assert_eq!(main.benches.len(), 1);
        assert_eq!(main.benches[0].base.as_deref(), Some(&b"main"[..]));
        assert!(main.benches[0].change.is_none());
        let missing = collect_benchmarks(&fs, "target/nope", "base", None).await;
        assert!(missing.is_err());
    }

    #[test]
    fn definition() {
        let tool = Typed(ShowBenchmarksTool::new(ports(Arc::new(FakeFormats::new()))));
        insta::assert_json_snapshot!(
            "show_benchmarks_definition",
            json!({
                "name": tool.name(),
                "description": tool.description(),
                "input_schema": tool.input_schema(),
            })
        );
    }

    #[tokio::test]
    async fn the_model_gets_criterions_numbers_marked_as_read() {
        let formats = Arc::new(FakeFormats::new());
        let tool = Typed(ShowBenchmarksTool::new(ports(Arc::clone(&formats))));
        let out = call(&tool, json!({})).await.unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        assert_eq!(
            out.text_content(),
            "Criterion results in `target/criterion` (baseline `base` vs `new`) shown to the \
             user: 2 benchmarks, 1 regressed, 0 improved, 0 within noise, 1 no change. The \
             times and changes are criterion's own estimates read from its estimates.json files \
             (95% intervals), not computed here; the verdict compares the interval of the mean \
             change with criterion's ±1% noise threshold (its p-value is not stored).\n\
             Regressed:\n\
             - fib/20: mean 25.1 µs [24.9 µs, 25.3 µs] vs 22 µs [21.8 µs, 22.2 µs], change \
             +14.2% [+12.9%, +15.6%]"
        );
        assert!(matches!(out.block_update, Some(BlockKind::Benchmarks(_))));
        let bad = tool.prepare(json!({ "baseline": "../x" }), &ctx()).await;
        assert!(matches!(bad, Err(ToolError::InvalidInput { .. })));
        let missing = call(&tool, json!({ "path": "nowhere" })).await.unwrap();
        assert!(missing.is_error);
    }
}
