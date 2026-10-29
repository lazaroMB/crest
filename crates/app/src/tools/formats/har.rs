//! `show_har`: an HTTP archive as a waterfall, a request list and an
//! inspector.
//!
//! The model gets the counts, bytes, page timings, time per phase, the
//! slowest and the failed requests, with URLs redacted
//! ([`redact_url`]) and no header values or bodies: cookies and
//! authorization headers stay with the user.

use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use domain::block::BlockKind;
use domain::devops::har::{HarSpec, format_bytes, redact_url};
use domain::permission::ToolEffect;
use domain::widget::timeline::{TimeUnit, format_duration};
use domain::workspace::WorkspacePath;
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;

use super::devops::MAX_TOP;
use super::{FormatToolPorts, MAX_LISTED, invalid, one_line};
use crate::ports::fs::FileSystemPort;
use crate::tools::registry::{PreparedCall, ToolContext, ToolError, ToolOutput, TypedTool};
use crate::usecases::compute::offloaded;

/// Largest HAR file read (they carry bodies), in bytes.
pub const MAX_HAR_BYTES: u64 = 128 * 1024 * 1024;

/// Input of `show_har`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowHar {
    /// A HAR file of the workspace, as exported by browser DevTools
    /// ("Save all as HAR"), mitmproxy, Charles, Fiddler or an HTTP client.
    pub path: String,
    /// Title shown above the waterfall; defaults to the file name.
    #[serde(default)]
    pub title: Option<String>,
}

/// `show_har`: a HAR file as a waterfall with a request inspector.
#[derive(Clone)]
pub struct ShowHarTool {
    ports: FormatToolPorts,
}

impl ShowHarTool {
    /// The tool over `ports`.
    #[must_use]
    pub fn new(ports: FormatToolPorts) -> Self {
        Self { ports }
    }
}

/// Name of `show_har`.
pub const SHOW_HAR: &str = "show_har";

/// Reads a HAR file of the workspace (up to [`MAX_HAR_BYTES`]) through
/// the guard.
///
/// # Errors
///
/// A message for the caller: a bad path, a file the guard refuses, one
/// too large or one that cannot be read.
pub async fn read_har(fs: &dyn FileSystemPort, path: &str) -> Result<Vec<u8>, String> {
    let parsed = WorkspacePath::parse(path).map_err(|e| format!("HAR path `{path}`: {e}"))?;
    fs.read_bytes(&parsed, MAX_HAR_BYTES)
        .await
        .map_err(|e| format!("cannot read `{path}`: {e}"))
}

#[async_trait]
impl TypedTool for ShowHarTool {
    type Input = ShowHar;

    fn name(&self) -> &str {
        SHOW_HAR
    }

    fn description(&self) -> &str {
        "Shows an HTTP archive (HAR) of the workspace in the user's \
         timeline: a waterfall with each request's phases (blocked, DNS, \
         connect, TLS, send, wait, receive), a request list with status, \
         type, method and host chips and a text filter, and an inspector \
         for headers, query, bodies and timing (secrets masked until the \
         user reveals them, copy as curl). Returns the counts, bytes, \
         page timings, time per phase and the slowest and failed requests \
         (URLs redacted, no header values): do not repeat the list in \
         your reply."
    }

    fn effect(&self, _input: &ShowHar) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(&self, input: ShowHar, _ctx: &ToolContext) -> Result<PreparedCall, ToolError> {
        if input.path.trim().is_empty() {
            return Err(invalid(SHOW_HAR, "`path` is empty"));
        }
        let ports = self.ports.clone();
        let subject = input.path.clone();
        let run = async move {
            let bytes = match read_har(ports.fs.as_ref(), &input.path).await {
                Ok(bytes) => bytes,
                Err(message) => return Ok(ToolOutput::error(message)),
            };
            let formats = Arc::clone(&ports.formats);
            let path = input.path.clone();
            let parsed = offloaded(&ports.offload, move || formats.parse_har(&bytes, &path)).await;
            let mut spec = match parsed {
                Some(Ok(spec)) => spec,
                Some(Err(err)) => return Ok(ToolOutput::error(err.to_string())),
                None => return Ok(ToolOutput::error("the HAR parser did not run")),
            };
            spec.title = input.title.filter(|t| !t.trim().is_empty());
            if let Err(err) = spec.validate() {
                return Ok(ToolOutput::error(format!("`{}`: {err}", input.path)));
            }
            let summary = har_summary(&input.path, &spec);
            Ok(ToolOutput::text(summary).with_block_update(BlockKind::Har(Box::new(spec))))
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

fn ms(value: f64) -> String {
    format_duration(value, TimeUnit::Ms)
}

/// What the model gets back from `show_har`.
#[must_use]
pub fn har_summary(path: &str, spec: &HarSpec) -> String {
    let failed = spec.failed();
    let mut out = format!("HAR `{path}`");
    if let Some(creator) = &spec.creator {
        let _ = write!(out, " ({})", one_line(creator, 60));
    }
    let _ = write!(out, " shown to the user: {} requests", spec.entries.len());
    if spec.total_entries > spec.entries.len() {
        let _ = write!(out, " (of {} in the file)", spec.total_entries);
    }
    let _ = write!(
        out,
        ", {} failed, {} transferred, {} from the first request to the last response.",
        failed.len(),
        format_bytes(spec.transferred()),
        ms(spec.duration())
    );
    for page in &spec.pages {
        let mut timings = Vec::new();
        if let Some(t) = page.on_content_load {
            timings.push(format!("DOMContentLoaded {}", ms(t)));
        }
        if let Some(t) = page.on_load {
            timings.push(format!("load {}", ms(t)));
        }
        if !timings.is_empty() {
            let name = page.title.as_deref().unwrap_or(&page.id);
            let _ = write!(
                out,
                "\nPage {}: {}",
                one_line(&redact_url(name), 120),
                timings.join(", ")
            );
        }
    }
    let types: Vec<String> = spec
        .by_type()
        .iter()
        .map(|(t, n, bytes)| format!("{t} {n} ({})", format_bytes(*bytes)))
        .collect();
    if !types.is_empty() {
        let _ = write!(out, "\nBy type: {}", types.join(", "));
    }
    let hosts: Vec<String> = spec
        .by_host()
        .iter()
        .take(MAX_TOP)
        .map(|(h, n)| format!("{h} {n}"))
        .collect();
    if !hosts.is_empty() {
        let _ = write!(out, "\nHosts: {}", hosts.join(", "));
    }
    let phases: Vec<String> = spec
        .phase_totals()
        .iter()
        .filter(|(_, t)| *t > 0.0)
        .map(|(name, t)| format!("{name} {}", ms(*t)))
        .collect();
    if !phases.is_empty() {
        let _ = write!(
            out,
            "\nTime by phase, summed over requests: {}",
            phases.join(", ")
        );
    }
    let request = |ix: usize| {
        let e = &spec.entries[ix];
        format!("{} {}", e.method, one_line(&redact_url(&e.url), 160))
    };
    let slowest = spec.slowest(MAX_TOP);
    if !slowest.is_empty() {
        out.push_str("\nSlowest:");
        for ix in slowest {
            let e = &spec.entries[ix];
            let status = if e.status == 0 {
                "no response".to_owned()
            } else {
                e.status.to_string()
            };
            let _ = write!(out, "\n- {} → {status} in {}", request(ix), ms(e.time));
            let longest = e
                .timings
                .phases()
                .into_iter()
                .filter_map(|(name, t)| Some((name, t?)))
                .max_by(|a, b| a.1.total_cmp(&b.1));
            if let Some((name, t)) = longest.filter(|(_, t)| *t > 0.0) {
                let _ = write!(out, " (mostly {name}, {})", ms(t));
            }
        }
    }
    if !failed.is_empty() {
        out.push_str("\nFailed:");
        for &ix in failed.iter().take(MAX_LISTED) {
            let e = &spec.entries[ix];
            let _ = write!(out, "\n- {} → ", request(ix));
            if e.status == 0 {
                out.push_str("no response");
            } else {
                let _ = write!(out, "{}", e.status);
                if let Some(text) = &e.status_text {
                    let _ = write!(out, " {}", one_line(text, 60));
                }
            }
            if let Some(error) = &e.error {
                let _ = write!(out, " ({})", one_line(error, 80));
            }
        }
        if failed.len() > MAX_LISTED {
            let _ = write!(out, "\n… and {} more", failed.len() - MAX_LISTED);
        }
    }
    let credentials = spec.with_credentials();
    if credentials > 0 {
        let who = if credentials == 1 {
            "1 request carries".to_owned()
        } else {
            format!("{credentials} requests carry")
        };
        let _ = write!(
            out,
            "\n{who} cookies, authorization or tokens; their values are not shown here, and \
             the user sees them masked."
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

    fn ports(formats: Arc<FakeFormats>) -> FormatToolPorts {
        let fs = InMemoryFs::new(test_root()).with_file("net/app.har", "{}");
        FormatToolPorts {
            formats,
            fs: Arc::new(fs),
            offload: inline_offload(),
        }
    }

    async fn call(tool: &dyn Tool, input: Value) -> Result<ToolOutput, ToolError> {
        let prepared = tool.prepare(input, &ctx()).await?;
        prepared.run.await
    }

    #[test]
    fn definition() {
        let tool = Typed(ShowHarTool::new(ports(Arc::new(FakeFormats::new()))));
        insta::assert_json_snapshot!(
            "show_har_definition",
            json!({
                "name": tool.name(),
                "description": tool.description(),
                "input_schema": tool.input_schema(),
            })
        );
    }

    #[tokio::test]
    async fn the_model_gets_a_redacted_summary() {
        let formats = Arc::new(FakeFormats::new());
        let tool = Typed(ShowHarTool::new(ports(Arc::clone(&formats))));
        let out = call(&tool, json!({ "path": "net/app.har" })).await.unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        let text = out.text_content();
        assert_eq!(
            text,
            "HAR `net/app.har` (WebInspector 537.36) shown to the user: 2 requests, 1 failed, \
             4.0 kB transferred, 450 ms from the first request to the last response.\n\
             By type: document 1 (2.0 kB), fetch 1 (2.0 kB)\n\
             Hosts: app.test 2\n\
             Time by phase, summed over requests: connect 20 ms, send 20 ms, wait 380 ms\n\
             Slowest:\n\
             - POST https://app.test/api/pay?token=<redacted> → 503 in 300 ms (mostly wait, 280 ms)\n\
             - GET https://app.test/ → 200 in 120 ms (mostly wait, 100 ms)\n\
             Failed:\n\
             - POST https://app.test/api/pay?token=<redacted> → 503\n\
             1 request carries cookies, authorization or tokens; their values are not shown \
             here, and the user sees them masked."
        );
        assert!(!text.contains("s3cret") && !text.contains("abc"));
        assert!(matches!(out.block_update, Some(BlockKind::Har(_))));
        let empty = tool.prepare(json!({ "path": "" }), &ctx()).await;
        assert!(matches!(empty, Err(ToolError::InvalidInput { .. })));
        let missing = call(&tool, json!({ "path": "nope.har" })).await.unwrap();
        assert!(missing.is_error);
    }
}
