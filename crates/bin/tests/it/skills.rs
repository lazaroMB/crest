//! Claude Code's rules and skills through the real composition root: the
//! rules reach the prompt or the tool results they match, the model loads
//! a skill with `skill`, and the user's `/name` loads one before the model
//! answers.

// Helpers outside #[test] functions unwrap too: a failure is a test failure.
#![allow(clippy::unwrap_used)]

use std::io::{self, Cursor, Write};
use std::path::Path;
use std::sync::Arc;

use application::ports::llm::LlmRequest;
use application::ports::plugin::ConfigSources;
use application::usecases::cancel::CancelToken;
use application::usecases::commands::Command;
use application_testing::llm::{ScriptedLlm, ScriptedResponse};
use crest::gui::UiController;
use crest::headless::run::frontend;
use crest::headless::{StdinApproval, Terminal, run_turn};
use crest::root::{App, Config, Overrides, build_app};
use domain::conversation::{ContentPart, ToolResultContent};
use domain::permission::PermissionMode;
use serde_json::json;
use ui::session::{Reply, SessionController as _, TurnEnd, TurnRequest};

struct Sink;

impl Write for Sink {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

fn write(path: &Path, text: &str) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, text).unwrap();
}

/// A workspace set up for Claude Code: a rule for every file, one for the
/// API, a skill and a custom command.
fn workspace(dir: &Path) -> std::path::PathBuf {
    let ws = dir.join("ws");
    write(&ws.join(".claude/rules/style.md"), "Indent with tabs.");
    write(
        &ws.join(".claude/rules/api.md"),
        "---\npaths:\n  - \"src/api/**/*.rs\"\n---\nValidate every input.",
    );
    write(
        &ws.join(".claude/skills/hello/SKILL.md"),
        "---\nname: hello\ndescription: Greets someone\nargument-hint: <name>\n---\n\
         Say hello to $ARGUMENTS in Spanish.",
    );
    write(
        &ws.join(".claude/commands/standup.md"),
        "---\ndescription: Writes the standup\ndisable-model-invocation: true\n---\n\
         Summarize yesterday's commits.",
    );
    write(&ws.join("src/api/users.rs"), "pub fn create() {}\n");
    ws
}

async fn app(ws: &Path, dir: &Path, llm: &Arc<ScriptedLlm>) -> App {
    let terminal = Terminal::new(Box::new(Sink), false);
    let approval = StdinApproval::new(terminal.clone(), Box::new(Cursor::new(Vec::new())), false);
    let config = Config {
        workspace: ws.to_path_buf(),
        mode: PermissionMode::Ask,
        model: None,
        trust_mcp: Vec::new(),
        trust_plugins: Vec::new(),
        resume: false,
        session_id: None,
        project: None,
    };
    let overrides = Overrides {
        llm: Some(llm.clone()),
        config_sources: Some(ConfigSources::default()),
        db_path: Some(dir.join("sessions.db")),
        ..Overrides::default()
    };
    build_app(&config, frontend(&terminal, approval), overrides)
        .await
        .unwrap()
}

fn system(request: &LlmRequest) -> String {
    request.system.iter().map(|b| b.text.clone()).collect()
}

fn results(request: &LlmRequest) -> Vec<String> {
    request
        .messages
        .iter()
        .flat_map(|m| m.content.clone())
        .filter_map(|part| match part {
            ContentPart::ToolResult { content, .. } => Some(content),
            _ => None,
        })
        .flatten()
        .filter_map(|item| match item {
            ToolResultContent::Text { text } => Some(text),
            ToolResultContent::Image { .. } => None,
        })
        .collect()
}

#[tokio::test]
async fn rules_apply_and_the_model_loads_a_skill() {
    let dir = tempfile::tempdir().unwrap();
    let ws = workspace(dir.path());
    let llm = Arc::new(ScriptedLlm::new(vec![
        ScriptedResponse::tool_use(
            "toolu_1",
            "skill",
            json!({ "skill": "hello", "args": "Ana" }),
        ),
        ScriptedResponse::tool_use(
            "toolu_2",
            "read_file",
            json!({ "path": "src/api/users.rs" }),
        ),
        ScriptedResponse::text("¡Hola, Ana!"),
    ]));
    let mut app = app(&ws, dir.path(), &llm).await;
    run_turn(&mut app, "greet Ana", CancelToken::new()).await;

    let requests = llm.requests();
    let first = system(&requests[0]);
    assert!(
        first.contains("## .claude/rules/style.md\nIndent with tabs."),
        "{first}"
    );
    assert!(
        !first.contains("Validate every input"),
        "path rules wait: {first}"
    );

    let skill = requests[0]
        .tools
        .iter()
        .find(|tool| tool.name == "skill")
        .expect("the skill tool is offered");
    assert!(
        skill.description.contains("- hello: Greets someone"),
        "{}",
        skill.description
    );
    assert!(
        !skill.description.contains("standup"),
        "disable-model-invocation hides it"
    );

    let loaded = results(&requests[1]);
    assert!(
        loaded[0].contains("Say hello to Ana in Spanish."),
        "{loaded:?}"
    );

    let read = results(&requests[2]);
    let attached = read.last().unwrap();
    assert!(
        attached.contains("<instructions path=\".claude/rules/api.md\">"),
        "{read:?}"
    );
    assert!(attached.contains("Validate every input."));
}

#[tokio::test]
async fn the_users_command_runs_its_skill_first() {
    let dir = tempfile::tempdir().unwrap();
    let ws = workspace(dir.path());
    let llm = Arc::new(ScriptedLlm::new(vec![ScriptedResponse::text("Standup.")]));
    let app = app(&ws, dir.path(), &llm).await;
    let controller = UiController::new(app, tokio::runtime::Handle::current());

    // `/standup` is found among the skills and asks for a skill turn.
    let reply = controller
        .run_command(Command::parse("standup since monday"))
        .expect("a skill answers it")
        .await;
    let Reply::Actions(actions) = reply else {
        panic!("{reply:?}");
    };
    assert_eq!(
        actions,
        [application::ports::plugin::PluginAction::Skill {
            name: "standup".into(),
            args: "since monday".into()
        }]
    );
    assert!(controller.run_command(Command::parse("nosuch")).is_none());

    let end = controller
        .run_turn(TurnRequest {
            prompt: "/standup since monday".into(),
            images: Vec::new(),
            model_override: None,
            role: None,
            cancel: CancelToken::new(),
            skill: Some(application::usecases::run_turn::SkillCall {
                name: "standup".into(),
                args: "since monday".into(),
            }),
        })
        .await;
    assert!(matches!(end, TurnEnd::Finished(_)), "{end:?}");
    let requests = llm.requests();
    assert_eq!(requests.len(), 1);
    let loaded = results(&requests[0]);
    assert!(
        loaded[0].contains("Summarize yesterday's commits.\n\nARGUMENTS: since monday"),
        "{loaded:?}"
    );
}
