//! Routing of each chat prompt with the decision model, in one call: which
//! optional tool groups the turn is offered and which model answers it.
//!
//! - **Tools.** The file tools are always offered. The others form groups
//!   (the `show_*` components, the web, `simulate_solution`, the `task`
//!   subagent, each MCP server, each plugin tool) and one yes/no question
//!   per group asks whether the prompt needs it; a group stays from
//!   [`KEEP_THRESHOLD`] (leaving out a needed tool costs more than offering
//!   an idle one), and so does every group the session already used, so
//!   the history never names a tool the request lacks. The set is fixed
//!   for the whole turn, so the prompt cache holds within its tool loop.
//! - **Model.** A score question rates the prompt trivial, normal or hard;
//!   a trivial one goes to `models.trivial`, a hard one to `models.hard`,
//!   a normal one keeps the `main` role.
//!
//! Without a decision model, or when it fails, the turn gets every tool
//! and the `main` role, as before.

use std::collections::BTreeSet;

use domain::conversation::{ContentPart, Session};
use domain::decision::{DecisionRequest, Question};

use crate::decide::Decider;
use crate::ports::llm::ToolDefinition;

/// Probability of "needed" from which a tool group is offered.
pub const KEEP_THRESHOLD: f32 = 0.3;
/// Below this weighted level a prompt is trivial (levels 0, 1, 2).
const TRIVIAL_BELOW: f32 = 0.5;
/// Above this weighted level a prompt is hard.
const HARD_ABOVE: f32 = 1.5;
/// Characters of the prompt put in the state.
const MAX_PROMPT_CHARS: usize = 4_000;
/// Characters of a tool description put in a question.
const MAX_DESCRIPTION_CHARS: usize = 120;
/// Tools offered in every turn.
const CORE_TOOLS: [&str; 8] = [
    "list_files",
    "find_file",
    "tree",
    "grep",
    "read_file",
    "edit_file",
    "write_file",
    "run_command",
];
/// Question id of the difficulty.
const DIFFICULTY: &str = "difficulty";

/// How hard a prompt is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Difficulty {
    /// A greeting, a short answer, a one-line change.
    Trivial,
    /// Ordinary work.
    Normal,
    /// Design, large or risky changes, subtle debugging.
    Hard,
}

/// What the decision model chose for a prompt.
#[derive(Debug, Clone, PartialEq)]
pub struct Routing {
    /// The model (alias or id) the turn uses instead of the `main` role.
    pub model: Option<String>,
    /// How hard the prompt is, when it was asked.
    pub difficulty: Option<Difficulty>,
    /// The tools offered, when the groups were asked.
    pub tools: Option<BTreeSet<String>>,
    /// Labels of the groups left out.
    pub omitted: Vec<String>,
    /// The decision model that answered.
    pub decision_model: String,
    /// Its input tokens.
    pub input_tokens: u64,
    /// Its output tokens.
    pub output_tokens: u64,
}

impl Routing {
    /// One line for the user when the routing changed something.
    #[must_use]
    pub fn summary(&self) -> Option<String> {
        let mut parts = Vec::new();
        if let (Some(model), Some(difficulty)) = (&self.model, self.difficulty) {
            let how = match difficulty {
                Difficulty::Trivial => "a simple",
                Difficulty::Normal => "a normal",
                Difficulty::Hard => "a hard",
            };
            parts.push(format!("{model} for {how} prompt"));
        }
        if !self.omitted.is_empty() {
            parts.push(format!("tools left out: {}", self.omitted.join(", ")));
        }
        (!parts.is_empty()).then(|| format!("Decision model: {}.", parts.join("; ")))
    }
}

/// An optional group of tools.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Group {
    /// Question id.
    id: String,
    /// Shown to the user.
    label: String,
    /// What the group lets the model do.
    purpose: String,
    /// Its tools.
    tools: Vec<String>,
}

/// The optional groups of `definitions` (the core tools are in none).
fn groups(definitions: &[ToolDefinition]) -> Vec<Group> {
    let mut out: Vec<Group> = Vec::new();
    let mut add = |id: String, label: String, purpose: String, tool: &str| match out
        .iter_mut()
        .find(|g| g.id == id)
    {
        Some(group) => group.tools.push(tool.to_owned()),
        None => out.push(Group {
            id,
            label,
            purpose,
            tools: vec![tool.to_owned()],
        }),
    };
    for def in definitions {
        let name = def.name.as_str();
        if CORE_TOOLS.contains(&name) {
            continue;
        }
        let (id, label, purpose) = if name.starts_with("show_") {
            (
                "need.components".to_owned(),
                "charts and tables".to_owned(),
                "draw a chart, a table, a Mermaid diagram or rendered markdown in the chat"
                    .to_owned(),
            )
        } else if matches!(name, "web_search" | "web_fetch") {
            (
                "need.web".to_owned(),
                "web".to_owned(),
                "search the web or read a web page".to_owned(),
            )
        } else if name == "simulate_solution" {
            (
                "need.simulate".to_owned(),
                "simulation".to_owned(),
                "simulate the solution of a Spec".to_owned(),
            )
        } else if name == "task" {
            (
                "need.subagent".to_owned(),
                "subagent".to_owned(),
                "hand a broad search of the codebase to a subagent".to_owned(),
            )
        } else if let Some(server) = name
            .strip_prefix("mcp__")
            .and_then(|rest| rest.split("__").next())
        {
            (
                format!("need.mcp.{}", sanitize(server)),
                format!("MCP {server}"),
                format!("use the {server} MCP server ({})", short(&def.description)),
            )
        } else {
            (
                format!("need.tool.{}", sanitize(name)),
                name.to_owned(),
                format!("use the {name} tool ({})", short(&def.description)),
            )
        };
        add(id, label, purpose, name);
    }
    out
}

/// `text` with every character a question id cannot have replaced.
fn sanitize(text: &str) -> String {
    text.chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .take(80)
        .collect()
}

fn short(text: &str) -> String {
    let line = text.lines().next().unwrap_or_default().trim();
    let mut out: String = line.chars().take(MAX_DESCRIPTION_CHARS).collect();
    if line.chars().count() > MAX_DESCRIPTION_CHARS {
        out.push('…');
    }
    out
}

/// The tools `session` already called.
#[must_use]
pub fn used_tools(session: &Session) -> BTreeSet<String> {
    session
        .history()
        .flat_map(|m| m.content.iter())
        .filter_map(|part| match part {
            ContentPart::ToolUse { name, .. } => Some(name.clone()),
            _ => None,
        })
        .collect()
}

/// Asks the decision model about `prompt`: the groups of `definitions` it
/// needs and how hard it is, as `decider`'s options allow. `None` without
/// a decision model, with nothing to ask, or when it fails.
pub async fn route(
    decider: &Decider,
    prompt: &str,
    definitions: &[ToolDefinition],
    used: &BTreeSet<String>,
) -> Option<Routing> {
    let options = decider.options();
    if !decider.is_available() || prompt.trim().is_empty() {
        return None;
    }
    // Groups the session used are kept without asking.
    let groups: Vec<Group> = if options.tools {
        groups(definitions)
    } else {
        Vec::new()
    };
    let asked: Vec<&Group> = groups
        .iter()
        .filter(|g| !g.tools.iter().any(|t| used.contains(t)))
        .take(domain::decision::MAX_QUESTIONS - 1)
        .collect();
    let state: String = prompt.chars().take(MAX_PROMPT_CHARS).collect();
    let mut request = DecisionRequest::new(state);
    for group in &asked {
        request = request.ask(
            group.id.clone(),
            Question::yes_no(format!(
                "Does answering this request of a user to a coding assistant need to {}?",
                group.purpose
            )),
        );
    }
    if options.model_routing {
        request = request.ask(
            DIFFICULTY,
            Question::score(
                "How hard is this request to a coding assistant?",
                [
                    "trivial: a greeting, a short factual answer or a one-line change",
                    "normal: ordinary coding work in a few files",
                    "hard: design, a large or risky change across many files, or subtle \
                     debugging",
                ],
            ),
        );
    }
    if request.questions.is_empty() {
        return None;
    }
    let response = decider.decide(&request).await?;
    let difficulty = response.score(DIFFICULTY).map(|level| {
        if level < TRIVIAL_BELOW {
            Difficulty::Trivial
        } else if level > HARD_ABOVE {
            Difficulty::Hard
        } else {
            Difficulty::Normal
        }
    });
    let model = match difficulty {
        Some(Difficulty::Trivial) => Some(options.models.trivial.clone()),
        Some(Difficulty::Hard) => Some(options.models.hard.clone()),
        Some(Difficulty::Normal) | None => None,
    };
    let (tools, omitted) = if options.tools {
        let mut offered: BTreeSet<String> = definitions
            .iter()
            .map(|d| d.name.clone())
            .filter(|n| CORE_TOOLS.contains(&n.as_str()))
            .collect();
        let mut omitted = Vec::new();
        for group in &groups {
            let keep = response.yes(&group.id).is_none_or(|p| p >= KEEP_THRESHOLD);
            if keep {
                offered.extend(group.tools.iter().cloned());
            } else {
                omitted.push(group.label.clone());
            }
        }
        (Some(offered), omitted)
    } else {
        (None, Vec::new())
    };
    Some(Routing {
        model,
        difficulty,
        tools,
        omitted,
        decision_model: response.model.clone(),
        input_tokens: response.input_tokens,
        output_tokens: response.output_tokens,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use domain::decision::Answer;
    use serde_json::json;

    use super::*;
    use crate::decide::DecisionOptions;
    use crate::testing::decision::{ScriptedDecider, score};

    fn def(name: &str, description: &str) -> ToolDefinition {
        ToolDefinition::new(name, description, json!({ "type": "object" }))
    }

    fn definitions() -> Vec<ToolDefinition> {
        vec![
            def("read_file", "Read a file"),
            def("grep", "Search"),
            def("show_chart", "Draw a chart"),
            def("show_table", "Draw a table"),
            def("web_search", "Search the web"),
            def("web_fetch", "Read a page"),
            def("task", "Subagent"),
            def(
                "mcp__github__list_issues",
                "List the issues of a repository",
            ),
            def("mcp__github__create_issue", "Create an issue"),
            def("weather", "Weather of a city\nmore details"),
        ]
    }

    fn scripted(
        port: ScriptedDecider,
        options: DecisionOptions,
    ) -> (Decider, Arc<ScriptedDecider>) {
        let port = Arc::new(port);
        (Decider::new(Arc::clone(&port) as _, options), port)
    }

    #[test]
    fn tools_form_groups_and_the_file_tools_none() {
        let groups = groups(&definitions());
        let ids: Vec<&str> = groups.iter().map(|g| g.id.as_str()).collect();
        assert_eq!(
            ids,
            [
                "need.components",
                "need.web",
                "need.subagent",
                "need.mcp.github",
                "need.tool.weather"
            ]
        );
        assert_eq!(groups[3].tools.len(), 2);
        assert!(
            groups[4].purpose.contains("Weather of a city") && !groups[4].purpose.contains("more")
        );
    }

    #[tokio::test]
    async fn unneeded_groups_are_left_out_and_the_difficulty_picks_the_model() {
        let (decider, port) = scripted(
            ScriptedDecider::answering(|id, _| match id {
                "difficulty" => score(0.1),
                "need.components" => Answer::YesNo { p_yes: 0.8 },
                _ => Answer::YesNo { p_yes: 0.05 },
            }),
            DecisionOptions::default(),
        );
        let used = BTreeSet::from(["mcp__github__list_issues".to_owned()]);
        let routing = route(&decider, "grafica las ventas", &definitions(), &used)
            .await
            .unwrap();
        assert_eq!(routing.model.as_deref(), Some("haiku"));
        assert_eq!(routing.difficulty, Some(Difficulty::Trivial));
        let tools = routing.tools.clone().unwrap();
        for kept in [
            "read_file",
            "grep",
            "show_chart",
            "show_table",
            "mcp__github__create_issue",
        ] {
            assert!(tools.contains(kept), "{kept}");
        }
        for left in ["web_search", "task", "weather"] {
            assert!(!tools.contains(left), "{left}");
        }
        assert_eq!(routing.omitted, ["web", "subagent", "weather"]);
        let asked = &port.requests()[0];
        assert!(
            !asked.questions.contains_key("need.mcp.github"),
            "used groups are not asked"
        );
        assert_eq!(asked.state, json!("grafica las ventas"));
        let summary = routing.summary().unwrap();
        assert!(
            summary.contains("haiku for a simple prompt") && summary.contains("web"),
            "{summary}"
        );
    }

    #[tokio::test]
    async fn the_options_limit_what_is_asked() {
        let only_models = DecisionOptions {
            tools: false,
            ..DecisionOptions::default()
        };
        let (decider, port) = scripted(ScriptedDecider::answering(|_, _| score(2.0)), only_models);
        let routing = route(
            &decider,
            "rediseña la arquitectura",
            &definitions(),
            &BTreeSet::new(),
        )
        .await
        .unwrap();
        assert_eq!(routing.model.as_deref(), Some("opus"));
        assert!(routing.tools.is_none());
        assert_eq!(port.requests()[0].questions.len(), 1);

        let nothing = DecisionOptions {
            tools: false,
            model_routing: false,
            ..DecisionOptions::default()
        };
        let (decider, _) = scripted(ScriptedDecider::yes(1.0), nothing);
        assert!(
            route(&decider, "hola", &definitions(), &BTreeSet::new())
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn without_a_decision_model_nothing_is_routed() {
        assert!(
            route(&Decider::none(), "hola", &definitions(), &BTreeSet::new())
                .await
                .is_none()
        );
        let (failing, _) = scripted(ScriptedDecider::failing(), DecisionOptions::default());
        assert!(
            route(&failing, "hola", &definitions(), &BTreeSet::new())
                .await
                .is_none()
        );
        let (yes, _) = scripted(ScriptedDecider::yes(1.0), DecisionOptions::default());
        assert!(
            route(&yes, "  ", &definitions(), &BTreeSet::new())
                .await
                .is_none()
        );
    }

    #[tokio::test]
    async fn a_normal_prompt_keeps_the_main_role() {
        let (decider, _) = scripted(
            ScriptedDecider::answering(|id, _| match id {
                "difficulty" => score(1.0),
                _ => Answer::YesNo { p_yes: 0.9 },
            }),
            DecisionOptions::default(),
        );
        let routing = route(
            &decider,
            "arregla el test",
            &definitions(),
            &BTreeSet::new(),
        )
        .await
        .unwrap();
        assert_eq!(routing.model, None);
        assert!(routing.omitted.is_empty());
        assert_eq!(routing.summary(), None);
        assert_eq!(routing.tools.unwrap().len(), definitions().len());
    }
}
