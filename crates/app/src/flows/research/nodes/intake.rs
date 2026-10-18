//! Stage 1, Intake: restates the request, classifies it (feature, bug,
//! performance, refactor) and asks what is missing to investigate it.
//!
//! The result is an [`IntakeBrief`] stored under [`BRIEF_KEY`] and shown as
//! a `Text` block. The stage has a gate: an answer other than an approval
//! reruns the node with it, and the answer is added to the brief before it
//! is restated again.
//!
//! The JSON helpers at the end ([`parse_json`], [`ask_json`]) are shared
//! with the other discovery nodes.

use std::fmt::Write as _;

use async_trait::async_trait;
use domain::block::BlockKind;
use domain::conversation::{ContentPart, Message};
use domain::research::Stage;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::flows::research::node::{NodeContext, NodeError, NodeOutcome, ResearchNode};

/// Flow data key of the [`IntakeBrief`].
pub const BRIEF_KEY: &str = "intake.brief";

const SYSTEM: &str = "Eres el primer paso de una investigación sobre un proyecto de software. \
Reformula el pedido del usuario en una o dos oraciones precisas, clasifícalo y pregunta solo \
lo que falte para investigarlo (pasos para reproducir, comportamiento esperado, alcance, \
métricas, entorno). No inventes datos ni propongas soluciones.
Responde solo con un objeto JSON, sin texto alrededor:
{\"kind\": \"feature\" | \"bug\" | \"performance\" | \"refactor\", \"restated\": \"…\", \
\"questions\": [\"…\"]}
Si no falta nada, `questions` es [].";

const RETRY: &str = "Tu respuesta anterior no es válida";

/// What the user asked for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestKind {
    /// New functionality.
    #[serde(alias = "funcionalidad")]
    Feature,
    /// Something that does not work as expected.
    #[serde(alias = "error")]
    Bug,
    /// Something too slow or too costly.
    #[serde(alias = "rendimiento")]
    Performance,
    /// Restructuring without changing behaviour.
    Refactor,
}

impl RequestKind {
    /// Name shown to the user.
    #[must_use]
    pub const fn name_es(self) -> &'static str {
        match self {
            Self::Feature => "Funcionalidad nueva",
            Self::Bug => "Bug",
            Self::Performance => "Rendimiento",
            Self::Refactor => "Refactor",
        }
    }
}

/// The request as Intake understood it, read by the later stages.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IntakeBrief {
    /// Classification of the request.
    pub kind: RequestKind,
    /// The request restated in one or two sentences.
    pub restated: String,
    /// What is still missing, asked to the user.
    pub questions: Vec<String>,
    /// The user's answers at the gate, oldest first.
    #[serde(default)]
    pub answers: Vec<String>,
}

/// What the model answers.
#[derive(Debug, Deserialize)]
struct Answer {
    kind: RequestKind,
    restated: String,
    #[serde(default)]
    questions: Vec<String>,
}

impl Answer {
    fn validate(&self) -> Result<(), String> {
        if self.restated.trim().is_empty() {
            return Err("`restated` está vacío".into());
        }
        if self.questions.iter().any(|q| q.trim().is_empty()) {
            return Err("hay una pregunta vacía en `questions`".into());
        }
        Ok(())
    }
}

/// The Intake node.
#[derive(Debug, Default)]
pub struct Intake;

impl Intake {
    /// The node.
    #[must_use]
    pub fn new() -> Self {
        Self
    }
}

/// The user prompt: the request, the previous brief and the answers.
fn prompt(problem: &str, previous: Option<&IntakeBrief>, answers: &[String]) -> String {
    let mut out = format!("Pedido original:\n{problem}\n");
    if let Some(brief) = previous {
        let _ = write!(
            out,
            "\nReformulación anterior ({}): {}\n",
            brief.kind.name_es(),
            brief.restated
        );
        if !brief.questions.is_empty() {
            out.push_str("Preguntas hechas:\n");
            for q in &brief.questions {
                let _ = writeln!(out, "- {q}");
            }
        }
    }
    if !answers.is_empty() {
        out.push_str("\nRespuestas del usuario:\n");
        for a in answers {
            let _ = writeln!(out, "- {a}");
        }
        out.push_str(
            "\nIncorpora las respuestas, vuelve a reformular y pregunta solo lo que siga faltando.\n",
        );
    }
    out
}

/// The `Text` block shown to the user.
fn render(brief: &IntakeBrief) -> String {
    let mut out = format!(
        "**Tipo:** {}\n\n**Pedido reformulado:** {}\n",
        brief.kind.name_es(),
        brief.restated
    );
    if brief.questions.is_empty() {
        out.push_str("\nNo hay preguntas pendientes.\n");
    } else {
        out.push_str("\n**Preguntas:**\n\n");
        for (i, q) in brief.questions.iter().enumerate() {
            let _ = writeln!(out, "{}. {q}", i + 1);
        }
    }
    out
}

#[async_trait]
impl ResearchNode for Intake {
    fn stage(&self) -> Stage {
        Stage::Intake
    }

    async fn run(&self, ctx: &mut NodeContext) -> Result<NodeOutcome, NodeError> {
        let previous: Option<IntakeBrief> = ctx.data(BRIEF_KEY);
        let mut answers = previous
            .as_ref()
            .map(|b| b.answers.clone())
            .unwrap_or_default();
        if let Some(reply) = ctx.user_reply().map(str::trim).filter(|r| !r.is_empty()) {
            answers.push(reply.to_owned());
        }
        let prompt = prompt(ctx.problem(), previous.as_ref(), &answers);
        let answer: Answer = ask_json(ctx, SYSTEM, &prompt, Answer::validate).await?;
        let brief = IntakeBrief {
            kind: answer.kind,
            restated: answer.restated.trim().to_owned(),
            questions: answer
                .questions
                .iter()
                .map(|q| q.trim().to_owned())
                .collect(),
            answers,
        };
        ctx.set_data(BRIEF_KEY, &brief)?;
        ctx.emit(BlockKind::Text {
            markdown: render(&brief),
        })?;
        let action = if ctx.user_reply().is_some() {
            "Pedido reformulado con la respuesta del usuario"
        } else {
            "Pedido reformulado"
        };
        ctx.log(format!(
            "{action} y clasificado como {}: {} ({} preguntas pendientes).",
            brief.kind.name_es(),
            brief.restated,
            brief.questions.len()
        ));
        Ok(NodeOutcome::Done)
    }
}

/// The JSON object in a model answer: from its first `{` to its last `}`,
/// so code fences and stray text around it are ignored.
fn json_body(answer: &str) -> &str {
    match (answer.find('{'), answer.rfind('}')) {
        (Some(start), Some(end)) if start < end => &answer[start..=end],
        _ => answer.trim(),
    }
}

/// Parses the JSON object of a model answer.
///
/// # Errors
///
/// The serde error, as text to send back to the model.
pub(crate) fn parse_json<T: DeserializeOwned>(answer: &str) -> Result<T, String> {
    serde_json::from_str(json_body(answer)).map_err(|e| e.to_string())
}

/// Parses and validates `answer`.
pub(crate) fn parse_valid<T: DeserializeOwned>(
    answer: &str,
    validate: impl Fn(&T) -> Result<(), String>,
) -> Result<T, String> {
    let value = parse_json(answer)?;
    validate(&value)?;
    Ok(value)
}

/// The message sent back to the model when its answer is invalid.
pub(crate) fn retry_message(error: &str) -> String {
    format!("{RETRY}: {error}. Responde de nuevo solo con el objeto JSON pedido.")
}

/// The error of a node whose model answered invalid JSON twice.
pub(crate) fn invalid_answer(error: &str) -> NodeError {
    NodeError::Failed(format!(
        "el modelo respondió dos veces un JSON inválido: {error}"
    ))
}

/// Asks the stage's model for a JSON object of type `T`, without tools.
///
/// An answer that does not parse or `validate` is sent back once with the
/// error; a second failure is a [`NodeError::Failed`].
///
/// # Errors
///
/// The model's errors, or the invalid answer.
pub(crate) async fn ask_json<T: DeserializeOwned>(
    ctx: &NodeContext,
    system: &str,
    prompt: &str,
    validate: impl Fn(&T) -> Result<(), String>,
) -> Result<T, NodeError> {
    let first = ctx.ask(system, prompt).await?;
    let error = match parse_valid(&first, &validate) {
        Ok(value) => return Ok(value),
        Err(error) => error,
    };
    let messages = vec![
        Message::user_text(prompt),
        Message::assistant(vec![ContentPart::text(first)]),
        Message::user_text(retry_message(&error)),
    ];
    let second = ctx.complete(ctx.request(system, messages)).await?;
    parse_valid(&second, &validate).map_err(|e| invalid_answer(&e))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use domain::model::{ModelPolicy, ModelRole};
    use domain::research::ResearchState;

    use super::*;
    use crate::flows::research::node::NodeDeps;
    use crate::ports::events::DomainEvent;
    use crate::testing::llm::{ScriptedLlm, ScriptedResponse};
    use crate::testing::system::RecordingSink;

    fn context(llm: &Arc<ScriptedLlm>, sink: &Arc<RecordingSink>) -> NodeContext {
        let deps =
            NodeDeps::new(llm.clone(), ModelPolicy::default(), sink.clone()).with_clock(|| 5);
        NodeContext::new(
            Stage::Intake,
            deps,
            "el login tarda mucho",
            ResearchState::default(),
        )
    }

    fn texts(sink: &RecordingSink) -> Vec<String> {
        sink.events()
            .into_iter()
            .filter_map(|e| match e {
                DomainEvent::BlockAdded { block } => match block.kind {
                    BlockKind::Text { markdown } => Some(markdown),
                    _ => None,
                },
                _ => None,
            })
            .collect()
    }

    const ANSWER: &str = r#"```json
{"kind": "performance", "restated": "El login tarda más de 5 s.",
 "questions": ["¿Desde qué versión?", "¿En qué entorno?"]}
```"#;

    #[tokio::test]
    async fn classifies_restates_and_asks() {
        let llm = Arc::new(ScriptedLlm::new([ScriptedResponse::text(ANSWER)]));
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(&llm, &sink);

        assert_eq!(
            Intake::new().run(&mut ctx).await.unwrap(),
            NodeOutcome::Done
        );

        let brief: IntakeBrief = ctx.data(BRIEF_KEY).unwrap();
        assert_eq!(brief.kind, RequestKind::Performance);
        assert_eq!(brief.restated, "El login tarda más de 5 s.");
        assert_eq!(brief.questions.len(), 2);
        assert!(brief.answers.is_empty());
        let shown = texts(&sink);
        assert_eq!(shown.len(), 1);
        assert!(shown[0].contains("**Tipo:** Rendimiento"));
        assert!(shown[0].contains("1. ¿Desde qué versión?"));
        let req = llm.last_request().unwrap();
        assert_eq!(req.model.role, ModelRole::Main);
        let journal = &ctx.state().journal;
        assert_eq!((journal.len(), journal[0].stage), (1, Stage::Intake));
        assert!(journal[0].text.contains("2 preguntas"));
    }

    #[tokio::test]
    async fn a_gate_reply_is_incorporated_and_restated() {
        let llm = Arc::new(ScriptedLlm::new([ScriptedResponse::text(
            r#"{"kind": "bug", "restated": "Desde la v2 el login en Linux tarda 5 s.", "questions": []}"#,
        )]));
        let sink = Arc::new(RecordingSink::new());
        let ctx = context(&llm, &sink);
        let previous = IntakeBrief {
            kind: RequestKind::Performance,
            restated: "El login tarda más de 5 s.".into(),
            questions: vec!["¿Desde qué versión?".into()],
            answers: vec![],
        };
        ctx.set_data(BRIEF_KEY, &previous).unwrap();
        let mut ctx = ctx.with_reply("desde la v2, en Linux");

        Intake::new().run(&mut ctx).await.unwrap();

        let brief: IntakeBrief = ctx.data(BRIEF_KEY).unwrap();
        assert_eq!(brief.kind, RequestKind::Bug);
        assert_eq!(brief.answers, ["desde la v2, en Linux"]);
        assert!(brief.questions.is_empty());
        let sent = format!("{:?}", llm.last_request().unwrap().messages);
        assert!(sent.contains("desde la v2, en Linux"));
        assert!(sent.contains("¿Desde qué versión?"));
        assert!(texts(&sink)[0].contains("No hay preguntas pendientes"));
        assert!(
            ctx.state().journal[0]
                .text
                .contains("respuesta del usuario")
        );
    }

    #[tokio::test]
    async fn an_invalid_answer_is_retried_once() {
        let llm = Arc::new(ScriptedLlm::new([
            ScriptedResponse::text("es un bug"),
            ScriptedResponse::text(r#"{"kind": "refactor", "restated": "Separar el módulo."}"#),
        ]));
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(&llm, &sink);
        Intake::new().run(&mut ctx).await.unwrap();
        let brief: IntakeBrief = ctx.data(BRIEF_KEY).unwrap();
        assert_eq!(brief.kind, RequestKind::Refactor);
        let retry = llm.last_request().unwrap();
        assert_eq!(retry.messages.len(), 3);
        assert!(format!("{:?}", retry.messages[2]).contains(RETRY));
    }

    #[tokio::test]
    async fn two_invalid_answers_fail_without_state() {
        let llm = Arc::new(ScriptedLlm::new([
            ScriptedResponse::text(r#"{"kind": "bug", "restated": ""}"#),
            ScriptedResponse::text(r#"{"kind": "otro", "restated": "x"}"#),
        ]));
        let sink = Arc::new(RecordingSink::new());
        let mut ctx = context(&llm, &sink);
        let err = Intake::new().run(&mut ctx).await.unwrap_err();
        assert!(matches!(err, NodeError::Failed(_)), "{err}");
        assert!(ctx.data::<IntakeBrief>(BRIEF_KEY).is_none());
        assert!(texts(&sink).is_empty());
    }
}
