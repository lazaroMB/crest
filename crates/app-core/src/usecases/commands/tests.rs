use std::sync::Mutex;

use super::*;

#[test]
fn a_line_splits_in_name_and_trimmed_argument() {
    let line = |name, arg| CommandLine { name, arg };
    assert_eq!(parse("model"), line("model", ""));
    assert_eq!(parse("model  set opus "), line("model", "set opus"));
    assert_eq!(parse("!git status"), line("!", "git status"));
    assert_eq!(parse("! git status"), line("!", "git status"));
    assert_eq!(parse("!"), line("!", ""));
    assert_eq!(parse(""), line("", ""));
}

#[test]
fn aliases_name_the_same_command() {
    for quit in ["q", "quit", "qa"] {
        assert_eq!(Command::parse(quit), Command::Quit);
    }
    assert_eq!(Command::parse("h keys"), Command::Help("keys".into()));
    assert_eq!(Command::parse("aparte why"), Command::Aside("why".into()));
    assert_eq!(Command::parse("volver yes"), Command::Flow("yes".into()));
}

#[test]
fn the_session_commands_read_their_arguments() {
    assert_eq!(Command::parse("model"), Command::Model(ModelCommand::Popup));
    assert_eq!(
        Command::parse("model clear"),
        Command::Model(ModelCommand::Clear)
    );
    assert_eq!(
        Command::parse("model set"),
        Command::Model(ModelCommand::Invalid("usage: /model set <id>"))
    );
    assert_eq!(
        Command::parse("model set  claude-x "),
        Command::Model(ModelCommand::Set("claude-x".into()))
    );
    assert_eq!(
        Command::parse("model opus"),
        Command::Model(ModelCommand::Override("opus".into()))
    );
    assert_eq!(Command::parse("theme"), Command::Theme(ThemeCommand::Popup));
    assert_eq!(
        Command::parse("theme dark"),
        Command::Theme(ThemeCommand::Save("dark".into()))
    );
    assert_eq!(
        Command::parse("theme set dark"),
        Command::Theme(ThemeCommand::Save("dark".into()))
    );
    assert_eq!(
        Command::parse("theme set"),
        Command::Theme(ThemeCommand::Invalid("usage: /theme set <name>"))
    );
    assert_eq!(
        Command::parse("theme a b"),
        Command::Theme(ThemeCommand::Invalid("usage: /theme [set] <name>"))
    );
    assert_eq!(
        Command::parse("mode plan"),
        Command::Mode(Ok(PermissionMode::Plan))
    );
    assert!(matches!(Command::parse("mode nope"), Command::Mode(Err(_))));
    assert_eq!(Command::parse("modal close"), Command::ModalClose);
    assert_eq!(Command::parse("modal open"), Command::Modal("open".into()));
    assert_eq!(Command::parse("login"), Command::Login(String::new()));
}

#[test]
fn the_controller_commands_read_their_arguments() {
    assert_eq!(
        Command::parse("implement"),
        Command::Implement(ImplementCommand::Resume)
    );
    assert_eq!(
        Command::parse("implement retomar"),
        Command::Implement(ImplementCommand::Resume)
    );
    assert_eq!(
        Command::parse("implement cancelar"),
        Command::Implement(ImplementCommand::Cancel)
    );
    assert_eq!(
        Command::parse("implement 0001"),
        Command::Implement(ImplementCommand::Start("0001".into()))
    );
    assert_eq!(
        Command::parse("research list"),
        Command::Research(ResearchCommand::List(false))
    );
    assert_eq!(
        Command::parse("back 3"),
        Command::FlowLine("/back 3".into())
    );
    assert_eq!(Command::parse("skip"), Command::FlowLine("/skip".into()));
    assert_eq!(Command::parse("mcp"), Command::Mcp(McpCommand::List));
    assert_eq!(
        Command::parse("mcp restart git"),
        Command::Mcp(McpCommand::Manage {
            verb: McpVerb::Restart,
            name: "git".into()
        })
    );
    assert_eq!(
        Command::parse("mcp trust git"),
        Command::Mcp(McpCommand::Trust("git".into()))
    );
    assert_eq!(
        Command::parse("mcp frobnicate git"),
        Command::Mcp(McpCommand::Invalid)
    );
    assert_eq!(
        Command::parse("plugin trust x"),
        Command::Plugin(PluginCommand::Trust("x".into()))
    );
    assert_eq!(
        Command::parse("plugin approve x"),
        Command::Plugin(PluginCommand::Invalid)
    );
    assert_eq!(Command::parse("open @a.rs"), Command::Open("@a.rs".into()));
    assert_eq!(
        Command::parse("picker.files"),
        Command::Other {
            name: "picker.files".into(),
            arg: String::new()
        }
    );
}

#[test]
fn every_catalog_name_and_alias_is_a_command() {
    for info in CATALOG {
        // The shell command is typed as a bare `!` (`! ls`), not `/!`.
        let typed = if info.name == "!" {
            "!".to_owned()
        } else {
            format!("/{}", info.name)
        };
        assert!(info.usage.starts_with(&typed), "{info:?}");
        assert!(!info.description.is_empty(), "{info:?}");
        for name in std::iter::once(&info.name).chain(info.aliases) {
            let command = Command::parse(name);
            // Builtin actions with a dot (`/picker.files`) run as actions.
            let action = name.contains('.');
            assert_eq!(
                matches!(command, Command::Other { .. }),
                action,
                "/{name} is {command:?}"
            );
            let ours = dispatch(&Fake::default(), command).is_none();
            assert_eq!(
                ours,
                info.handler == Handler::Ui,
                "/{name} goes to the wrong handler"
            );
        }
    }
}

#[test]
fn the_unknown_command_hint_names_every_session_command() {
    let hint = unknown_command_hint("frobnicate");
    assert!(hint.starts_with("unknown command /frobnicate"), "{hint}");
    for info in commands_of(Handler::Ui) {
        assert!(hint.contains(&format!("/{}", info.name)), "{hint}");
    }
    assert!(!hint.contains("/research"), "{hint}");
    assert!(hint.contains("/help lists every command"), "{hint}");
}

/// Records what it was asked to run; `/picker.files` is not a command of
/// it, any other name is.
#[derive(Default)]
struct Fake {
    calls: Mutex<Vec<String>>,
}

impl Fake {
    fn record(&self, call: String) -> ReplyFuture {
        self.calls.lock().unwrap().push(call);
        futures::future::ready(Reply::Done).boxed()
    }
}

impl CommandHandler for Fake {
    fn implement(&self, command: ImplementCommand) -> ReplyFuture {
        self.record(format!("implement {command:?}"))
    }
    fn research(&self, command: ResearchCommand) -> ReplyFuture {
        self.record(format!("research {command:?}"))
    }
    fn aside(&self, question: String) -> ReplyFuture {
        self.record(format!("aside {question}"))
    }
    fn flow(&self, answer: String) -> ReplyFuture {
        self.record(format!("flow {answer}"))
    }
    fn flow_line(&self, line: String) -> ReplyFuture {
        self.record(format!("line {line}"))
    }
    fn shell(&self, command: String) -> ReplyFuture {
        self.record(format!("shell {command}"))
    }
    fn simulate(&self, spec: String) -> ReplyFuture {
        self.record(format!("simulate {spec}"))
    }
    fn mcp(&self, command: McpCommand) -> ReplyFuture {
        self.record(format!("mcp {command:?}"))
    }
    fn plugin(&self, command: PluginCommand) -> ReplyFuture {
        self.record(format!("plugin {command:?}"))
    }
    fn open(&self, arg: String) -> ReplyFuture {
        self.record(format!("open {arg}"))
    }
    fn callback(&self, name: &str, arg: String) -> Option<ReplyFuture> {
        (!name.contains('.')).then(|| self.record(format!("callback {name} {arg}")))
    }
}

fn run(fake: &Fake, line: &str) -> Option<Reply> {
    dispatch(fake, Command::parse(line)).map(|reply| reply.now_or_never().unwrap())
}

#[test]
fn dispatch_sends_each_command_to_its_handler() {
    let fake = Fake::default();
    for line in [
        "implement cancel",
        "research --deep x",
        "aparte why",
        "flow",
        "abort",
        "!ls -a",
        "simulate  0001 ",
        "mcp tools git",
        "plugin list",
        "open @a.rs",
        "review src",
    ] {
        assert_eq!(run(&fake, line), Some(Reply::Done), "{line}");
    }
    assert_eq!(
        *fake.calls.lock().unwrap(),
        [
            "implement Cancel",
            "research Start { problem: \"x\", mode: Deep }",
            "aside why",
            "flow ",
            "line /abort",
            "shell ls -a",
            "simulate 0001",
            "mcp Tools(\"git\")",
            "plugin List",
            "open @a.rs",
            "callback review src",
        ]
    );
}

#[test]
fn dispatch_answers_the_usage_of_an_empty_shell_or_simulate() {
    let fake = Fake::default();
    let warning = |text: &str| Some(Reply::Notice(NoticeLevel::Warning, text.into()));
    assert_eq!(run(&fake, "!"), warning(SHELL_USAGE));
    assert_eq!(run(&fake, "! "), warning(SHELL_USAGE));
    assert_eq!(run(&fake, "simulate"), warning(SIMULATE_USAGE));
    assert!(fake.calls.lock().unwrap().is_empty());
}

#[test]
fn dispatch_leaves_the_session_commands_and_unknown_actions() {
    let fake = Fake::default();
    for line in ["model opus", "clear", "q", "modal close", "picker.files"] {
        assert!(run(&fake, line).is_none(), "{line}");
    }
    assert!(fake.calls.lock().unwrap().is_empty());
}
