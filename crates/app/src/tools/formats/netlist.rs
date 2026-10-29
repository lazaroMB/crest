//! `show_netlist`: a SPICE deck or KiCad netlist as connectivity, a
//! component–net graph and a component table.
//!
//! The model gets what it cannot trace reliably by reading the text: the
//! component counts per kind, the nets with the most pins, the nets with
//! one pin (open wires, pins marked unconnected), each component's nets
//! (up to a limit) and the subcircuits, all read from the file.

use std::fmt::Write as _;
use std::sync::Arc;

use async_trait::async_trait;
use domain::block::BlockKind;
use domain::engineering::netlist::{NetlistFormat, NetlistSpec, is_ground};
use domain::permission::ToolEffect;
use futures::FutureExt;
use schemars::JsonSchema;
use serde::Deserialize;

use super::diagram::{one_of, source_text};
use super::{FormatToolPorts, MAX_LISTED, one_line};
use crate::tools::registry::{PreparedCall, ToolContext, ToolError, ToolOutput, TypedTool};
use crate::usecases::compute::offloaded;

/// Components whose nets are listed to the model.
pub const MAX_COMPONENTS_LISTED: usize = 60;

/// Input of `show_netlist`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowNetlist {
    /// A netlist of the workspace: a SPICE deck (`.cir`, `.sp`, `.spice`,
    /// `.net` from LTspice or ngspice) or a KiCad netlist export
    /// (`.net`, s-expressions). Give this or `source`.
    #[serde(default)]
    pub path: Option<String>,
    /// Netlist text inline (a SPICE deck you wrote). Give this or `path`.
    #[serde(default)]
    pub source: Option<String>,
    /// `spice` or `kicad`; guessed from the text when absent.
    #[serde(default)]
    pub format: Option<NetlistFormat>,
    /// Title shown above; defaults to the file name or the deck's title.
    #[serde(default)]
    pub title: Option<String>,
}

/// `show_netlist`: a netlist as a graph and a component table.
#[derive(Clone)]
pub struct ShowNetlistTool {
    ports: FormatToolPorts,
}

impl ShowNetlistTool {
    /// The tool over `ports`.
    #[must_use]
    pub fn new(ports: FormatToolPorts) -> Self {
        Self { ports }
    }
}

/// Name of `show_netlist`.
pub const SHOW_NETLIST: &str = "show_netlist";

#[async_trait]
impl TypedTool for ShowNetlistTool {
    type Input = ShowNetlist;

    fn name(&self) -> &str {
        SHOW_NETLIST
    }

    fn description(&self) -> &str {
        "Shows a circuit netlist (a SPICE deck or a KiCad `.net` export, in \
         `path` or `source`) as connectivity in the user's timeline: a \
         component–net graph (pins as edges, ground and single-pin nets \
         marked) and a component table with each pin's net. Returns the \
         counts per component kind, the largest nets, the nets with only \
         one pin, each component's nets and the subcircuits, read from the \
         file: trust these over your own reading of the text."
    }

    fn effect(&self, _input: &ShowNetlist) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(
        &self,
        input: ShowNetlist,
        _ctx: &ToolContext,
    ) -> Result<PreparedCall, ToolError> {
        let (path, inline) = one_of(SHOW_NETLIST, input.path, input.source, "source")?;
        let subject = path.clone().unwrap_or_else(|| "netlist".to_owned());
        let ports = self.ports.clone();
        let title = input.title.filter(|t| !t.trim().is_empty());
        let format = input.format;
        let run = async move {
            let text = match source_text(&ports, path.as_deref(), inline).await {
                Ok(text) => text,
                Err(message) => return Ok(ToolOutput::error(message)),
            };
            let formats = Arc::clone(&ports.formats);
            let at = path.clone();
            let parsed = offloaded(&ports.offload, move || {
                formats.parse_netlist(&text, format, at.as_deref())
            })
            .await;
            let mut spec = match parsed {
                Some(Ok(spec)) => spec,
                Some(Err(err)) => return Ok(ToolOutput::error(err.to_string())),
                None => return Ok(ToolOutput::error("the netlist reader did not run")),
            };
            spec.title = title;
            if let Err(err) = spec.validate() {
                return Ok(ToolOutput::error(err.to_string()));
            }
            let summary = netlist_summary(&spec);
            Ok(ToolOutput::text(summary).with_block_update(BlockKind::Netlist(Box::new(spec))))
        };
        Ok(PreparedCall::new(subject, run.boxed()))
    }
}

/// What the model gets back from `show_netlist`.
#[must_use]
pub fn netlist_summary(spec: &NetlistSpec) -> String {
    let nets = spec.nets();
    let mut out = spec.path.as_ref().map_or_else(
        || format!("{} netlist", spec.format.label()),
        |p| format!("{} netlist `{p}`", spec.format.label()),
    );
    if let Some(design) = &spec.design {
        let _ = write!(out, " ({})", one_line(design, 80));
    }
    let kinds: Vec<String> = spec
        .kind_counts()
        .iter()
        .map(|(k, n)| format!("{n} {k}"))
        .collect();
    let _ = write!(
        out,
        " shown to the user: {} components ({}), {} nets, {} pins.",
        spec.components.len(),
        kinds.join(", "),
        nets.len(),
        spec.pin_count()
    );
    let grounds: Vec<&str> = nets
        .iter()
        .filter(|n| is_ground(&n.name))
        .map(|n| n.name.as_str())
        .collect();
    if grounds.is_empty() {
        out.push_str("\nNo ground net (0 or GND) found.");
    } else {
        let _ = write!(out, "\nGround: {}", grounds.join(", "));
    }
    let mut largest: Vec<_> = nets.iter().filter(|n| n.pins.len() > 2).collect();
    largest.sort_by(|a, b| b.pins.len().cmp(&a.pins.len()).then(a.name.cmp(&b.name)));
    if !largest.is_empty() {
        let listed: Vec<String> = largest
            .iter()
            .take(MAX_LISTED)
            .map(|n| format!("{} ({} pins)", n.name, n.pins.len()))
            .collect();
        let _ = write!(out, "\nLargest nets: {}", listed.join(", "));
    }
    let single = spec.single_pin_nets();
    if single.is_empty() {
        out.push_str("\nEvery net connects at least two pins.");
    } else {
        let listed: Vec<String> = single
            .iter()
            .take(MAX_LISTED)
            .map(|n| {
                let pins: Vec<String> = n
                    .pins
                    .iter()
                    .map(|(ix, pin)| format!("{}.{pin}", spec.components[*ix].reference))
                    .collect();
                format!("{} ({})", n.name, pins.join(", "))
            })
            .collect();
        let _ = write!(
            out,
            "\nNets with a single pin (open, or marked unconnected): {}",
            listed.join("; ")
        );
    }
    out.push_str("\nComponents:");
    for c in spec.components.iter().take(MAX_COMPONENTS_LISTED) {
        let pins: Vec<String> = c
            .pins
            .iter()
            .map(|p| format!("{}={}", p.pin, p.net))
            .collect();
        let mut line = format!("\n- {} {}", c.reference, c.kind);
        if let Some(v) = &c.value {
            let _ = write!(line, " {}", one_line(v, 40));
        }
        if let Some(m) = &c.model {
            let _ = write!(line, " [{}]", one_line(m, 60));
        }
        if !pins.is_empty() {
            let _ = write!(line, ": {}", pins.join(" "));
        }
        out.push_str(&line);
    }
    if spec.components.len() > MAX_COMPONENTS_LISTED {
        let _ = write!(
            out,
            "\n… {} more components (the user sees them all)",
            spec.components.len() - MAX_COMPONENTS_LISTED
        );
    }
    if !spec.subcircuits.is_empty() {
        let subs: Vec<String> = spec
            .subcircuits
            .iter()
            .map(|s| {
                format!(
                    "{} ({}; {} elements)",
                    s.name,
                    s.ports.join(" "),
                    s.elements
                )
            })
            .collect();
        let _ = write!(out, "\nSubcircuits (not flattened): {}", subs.join(", "));
    }
    for w in spec.warnings.iter().take(5) {
        let _ = write!(out, "\nNote: {}", one_line(w, 120));
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

    fn ports() -> FormatToolPorts {
        let fs = InMemoryFs::new(test_root()).with_file("hw/div.cir", "divider\nR1 a 0 1k\n");
        FormatToolPorts {
            formats: Arc::new(FakeFormats::new()),
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
        let tool = Typed(ShowNetlistTool::new(ports()));
        insta::assert_json_snapshot!(
            "show_netlist_definition",
            json!({
                "name": tool.name(),
                "description": tool.description(),
                "input_schema": tool.input_schema(),
            })
        );
    }

    #[tokio::test]
    async fn the_model_gets_nets_and_open_pins() {
        let tool = Typed(ShowNetlistTool::new(ports()));
        let out = call(&tool, json!({ "path": "hw/div.cir" })).await.unwrap();
        assert!(!out.is_error, "{}", out.text_content());
        assert_eq!(
            out.text_content(),
            "SPICE netlist `hw/div.cir` (divider) shown to the user: 4 components (3 resistor, \
             1 voltage source), 4 nets, 8 pins.\n\
             Ground: 0\n\
             Largest nets: out (3 pins)\n\
             Nets with a single pin (open, or marked unconnected): nc (R3.2)\n\
             Components:\n\
             - V1 voltage source DC 5: 1=in 2=0\n\
             - R1 resistor 10k: 1=in 2=out\n\
             - R2 resistor 10k: 1=out 2=0\n\
             - R3 resistor 1k: 1=out 2=nc"
        );
        assert!(matches!(out.block_update, Some(BlockKind::Netlist(_))));
        let both = tool
            .prepare(json!({ "path": "a.cir", "source": "x" }), &ctx())
            .await;
        assert!(matches!(both, Err(ToolError::InvalidInput { .. })));
    }
}
