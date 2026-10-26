//! `show_card`: the model shows a card in another host's format (A2UI or
//! an Adaptive Card)
//! with crest's own blocks.
//!
//! The card is mapped by [`crate::interop`]. When it has inputs and
//! buttons it becomes a form (static content above the fields, one button
//! per action) and the call waits for the user like `ask_user`, through
//! [`crate::tools::form::ask`]; the model gets the answer in the card's own
//! response format (A2UI's `action` message, an Adaptive Card's
//! `Action.Execute`/`Action.Submit` payload) plus the values by field. A
//! card without buttons is shown as a block and the model gets a one-line
//! summary saying what was not drawn. [`crate::tools::form::FORM_TOOL_PROMPT`]
//! tells the model when to use it. The call only reads: it needs no
//! approval.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use async_trait::async_trait;
use domain::block::{BlockId, BlockKind};
use domain::permission::ToolEffect;
use futures::FutureExt as _;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};

use super::form::{Asked, ask, cancelled_text, rejected_text};
use super::registry::{PreparedCall, ToolContext, ToolError, ToolOutput, TypedTool};
use crate::interop::{self, CardKind};
use crate::ports::form::FormPort;

/// Name of the tool.
pub const SHOW_CARD: &str = "show_card";

const DESCRIPTION: &str = "Shows a declarative card in the user's timeline with crest's own \
blocks. `format` is `a2ui` (A2UI v0.9 messages: `createSurface`, `updateComponents` with a \
component `root`, `updateDataModel`; a list of messages or JSON lines; v0.8 is read too) or \
`adaptive_card` (an Adaptive Card object, schema 1.0 to 1.6). Supported A2UI components: Text, \
Image, Row, Column, List, Card, Tabs, Divider, Button, TextField, CheckBox, ChoicePicker, \
Slider, DateTimeInput. Supported Adaptive Card elements: TextBlock, RichTextBlock, Image, \
ImageSet, Container, ColumnSet, FactSet, Table, ActionSet, Input.Text/Number/Date/Time/Toggle/\
ChoiceSet, and actions Action.Submit, Action.Execute, Action.OpenUrl, Action.ShowCard. Others \
show as placeholders (or their `fallback`). Images only from data: URIs; remote ones become \
links. If the card has submit buttons, the call waits until the user presses one and returns \
the card's own response (A2UI `action` message, or `{type, verb, data}` with the inputs merged \
into `data`) and the values by field; otherwise it returns a summary.";

/// Input of `show_card`.
#[derive(Debug, Clone, PartialEq, Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct ShowCard {
    /// The card's format.
    pub format: CardKind,
    /// The card: a list of A2UI messages (or JSON lines as text), or the
    /// Adaptive Card object.
    pub card: Value,
    /// Heading of the block; the card's own or the format's name when
    /// absent.
    #[serde(default)]
    pub title: Option<String>,
}

/// The `show_card` tool.
pub struct ShowCardTool {
    port: Arc<dyn FormPort>,
    next_id: Arc<AtomicU64>,
}

impl std::fmt::Debug for ShowCardTool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShowCardTool").finish_non_exhaustive()
    }
}

impl ShowCardTool {
    /// A tool asking through `port`.
    #[must_use]
    pub fn new(port: Arc<dyn FormPort>) -> Self {
        Self {
            port,
            next_id: Arc::new(AtomicU64::new(1)),
        }
    }
}

#[async_trait]
impl TypedTool for ShowCardTool {
    type Input = ShowCard;

    fn name(&self) -> &str {
        SHOW_CARD
    }

    fn description(&self) -> &str {
        DESCRIPTION
    }

    fn effect(&self, _input: &ShowCard) -> ToolEffect {
        ToolEffect::Read
    }

    async fn prepare(&self, input: ShowCard, ctx: &ToolContext) -> Result<PreparedCall, ToolError> {
        let mapped = interop::map_card(input.format, &input.card, true).map_err(|err| {
            ToolError::InvalidInput {
                tool: SHOW_CARD.to_owned(),
                message: err.to_string(),
            }
        })?;
        let card = mapped.card();
        let title = input.title.filter(|t| !t.trim().is_empty());
        let label = card.format.label();
        let subject = title.clone().unwrap_or_else(|| label.to_owned());
        if !card.is_interactive() {
            let output = ToolOutput::text(card.summary())
                .with_block_update(interop::block(subject.clone(), card.tree()));
            return Ok(PreparedCall::new(
                subject,
                futures::future::ready(Ok(output)).boxed(),
            ));
        }
        let spec = card.form(title.as_deref());
        let summary = card.summary();
        let block_id = BlockId::new();
        let shown = BlockKind::Form(spec.clone());
        let port = Arc::clone(&self.port);
        let next_id = Arc::clone(&self.next_id);
        let tool_use_id = ctx.tool_use_id.clone();
        let run = async move {
            let check = {
                let mapped = mapped.clone();
                move |answer: &BTreeMap<String, Value>, action: Option<&str>| {
                    mapped.check(answer, action)
                }
            };
            let asked = ask(
                port.as_ref(),
                &next_id,
                block_id,
                &tool_use_id,
                spec,
                &check,
            )
            .await;
            match asked {
                Asked::Submitted {
                    form,
                    answer,
                    action,
                } => {
                    let pressed = action
                        .as_deref()
                        .and_then(|id| form.actions.iter().find(|a| a.id == id))
                        .map(|a| a.label.clone());
                    let reply = json!({
                        "pressed": pressed,
                        "response": mapped.response(&answer, action.as_deref()),
                        "values": answer,
                    });
                    let json = serde_json::to_string_pretty(&reply)
                        .map_err(|err| ToolError::Execution(err.to_string()))?;
                    let text = format!("{summary}\nThe user answered the card:\n{json}");
                    Ok(ToolOutput::text(text).with_block_update(BlockKind::Form(form)))
                }
                Asked::Cancelled { form, reason } => {
                    Ok(ToolOutput::error(cancelled_text(reason.as_deref()))
                        .with_block_update(BlockKind::Form(form)))
                }
                Asked::Rejected { form, error } => Ok(ToolOutput::error(rejected_text(&error))
                    .with_block_update(BlockKind::Form(form))),
            }
        };
        Ok(PreparedCall::new(subject, run.boxed()).with_block_as(block_id, shown))
    }
}

#[cfg(test)]
mod tests;
