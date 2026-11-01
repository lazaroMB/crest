//! The arguments of `/research`. The flow behind them lives in the binary
//! (it needs the research and discovery engines); only the parse is here,
//! so every front end reads the command the same way.

/// How `/research <problem>` runs: the flag before the problem.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StartMode {
    /// No flag: the fast pipeline, at the depth its triage picks.
    Auto,
    /// `--quick` (`-q`): the fast pipeline at quick depth.
    Quick,
    /// `--deep` (`-d`): the 14-stage pipeline.
    Deep,
}

impl StartMode {
    /// The mode named by a leading flag, if `word` is one.
    fn from_flag(word: &str) -> Option<Self> {
        match word {
            "--quick" | "-q" | "--rapido" | "--rápido" => Some(Self::Quick),
            "--deep" | "-d" | "--profundo" => Some(Self::Deep),
            _ => None,
        }
    }
}

/// What `/research <args>` asks.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResearchCommand {
    /// `/research [--quick|--deep] <problem>`.
    Start {
        /// The problem to research.
        problem: String,
        /// The pipeline and depth.
        mode: StartMode,
    },
    /// `/research resume [n|id]` (or `retry`): continues the last flow, or
    /// the one numbered `n` in `/research list`.
    Resume(Option<String>),
    /// `/research list [all]`: the open flows, or every one with `all`.
    List(bool),
    /// `/research cancel [n|id|all]`: aborts the last flow, the one
    /// numbered `n` in `/research list`, or every open one.
    Cancel(Option<String>),
    /// `/research stage|back <stage>|skip|abort|reopen <section>`: a
    /// command for the last flow.
    Line(String),
    /// `/research accept`: freezes an approved discovery and derives its
    /// Spec (discovery engine only).
    Accept,
    /// `/research`.
    Status,
}

impl ResearchCommand {
    /// Parses the arguments of `/research`.
    #[must_use]
    pub fn parse(args: &str) -> Self {
        let args = args.trim();
        let (name, rest) = args
            .split_once(char::is_whitespace)
            .map_or((args, ""), |(n, r)| (n, r.trim()));
        let which = (!rest.is_empty()).then(|| rest.to_owned());
        match name {
            "" => Self::Status,
            "resume" | "retomar" | "reanudar" | "continue" | "continuar" => Self::Resume(which),
            "retry" | "reintentar" if rest.is_empty() => Self::Resume(None),
            "list" | "ls" | "listar" if rest.is_empty() => Self::List(false),
            "list" | "ls" | "listar" if matches!(rest, "all" | "todos") => Self::List(true),
            "cancel" | "cancelar" | "descartar" | "drop" => Self::Cancel(which),
            "stage" | "skip" | "abort" | "back" | "reopen" => Self::Line(format!("/{args}")),
            "accept" | "aceptar" if rest.is_empty() => Self::Accept,
            flag => match StartMode::from_flag(flag) {
                Some(mode) => Self::Start {
                    problem: rest.to_owned(),
                    mode,
                },
                None => Self::Start {
                    problem: args.to_owned(),
                    mode: StartMode::Auto,
                },
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn research_arguments() {
        assert_eq!(ResearchCommand::parse(""), ResearchCommand::Status);
        assert_eq!(
            ResearchCommand::parse(" resume "),
            ResearchCommand::Resume(None)
        );
        assert_eq!(
            ResearchCommand::parse("resume 2"),
            ResearchCommand::Resume(Some("2".into()))
        );
        assert_eq!(
            ResearchCommand::parse("retry"),
            ResearchCommand::Resume(None)
        );
        assert_eq!(ResearchCommand::parse("list"), ResearchCommand::List(false));
        assert_eq!(
            ResearchCommand::parse("list all"),
            ResearchCommand::List(true)
        );
        assert_eq!(
            ResearchCommand::parse("cancel 2"),
            ResearchCommand::Cancel(Some("2".into()))
        );
        assert_eq!(
            ResearchCommand::parse("cancelar"),
            ResearchCommand::Cancel(None)
        );
        assert_eq!(
            ResearchCommand::parse("cancel all"),
            ResearchCommand::Cancel(Some("all".into()))
        );
        assert_eq!(
            ResearchCommand::parse("back 3"),
            ResearchCommand::Line("/back 3".into())
        );
        assert_eq!(
            ResearchCommand::parse("reopen scope too narrow"),
            ResearchCommand::Line("/reopen scope too narrow".into())
        );
        assert_eq!(ResearchCommand::parse("accept"), ResearchCommand::Accept);
        let start = |problem: &str, mode| ResearchCommand::Start {
            problem: problem.into(),
            mode,
        };
        assert_eq!(
            ResearchCommand::parse("add suma mal"),
            start("add suma mal", StartMode::Auto)
        );
        assert_eq!(
            ResearchCommand::parse("--quick the button does nothing"),
            start("the button does nothing", StartMode::Quick)
        );
        assert_eq!(ResearchCommand::parse("-q x"), start("x", StartMode::Quick));
        assert_eq!(
            ResearchCommand::parse("--deep  login is slow "),
            start("login is slow", StartMode::Deep)
        );
        assert_eq!(ResearchCommand::parse("-d"), start("", StartMode::Deep));
        // A flag later in the text is part of the problem.
        assert_eq!(
            ResearchCommand::parse("why does -q fail"),
            start("why does -q fail", StartMode::Auto)
        );
    }
}
