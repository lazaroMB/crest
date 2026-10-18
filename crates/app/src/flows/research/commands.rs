//! The chat commands of the research flow: `/stage`, `/back <stage>`,
//! `/skip` and `/abort`.

use domain::research::Stage;

use super::gate::normalize;

/// A research command typed in the chat.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Command {
    /// `/stage`: show where the flow is.
    Stage,
    /// `/back <stage>`: go back to an earlier (or the current) stage.
    Back(Stage),
    /// `/skip`: jump over the current stage, if it is optional.
    Skip,
    /// `/abort`: stop the flow for good.
    Abort,
}

/// Why a research command could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommandError {
    /// `/back` without a stage.
    #[error("usage: /back <stage>, by number (1-14) or name, e.g. /back 5 or /back hypotheses")]
    MissingStage,
    /// `/back` with something that names no stage (or several).
    #[error("{0:?} names no single stage; use its number (1-14) or name")]
    UnknownStage(String),
    /// A command that takes no argument got one.
    #[error("/{0} takes no arguments")]
    UnexpectedArgument(&'static str),
}

/// Parses `line` if it is a research command.
///
/// Returns `None` for anything else (plain text, or other `/` commands
/// such as `/model`, which belong to the chat).
#[must_use]
pub fn parse_command(line: &str) -> Option<Result<Command, CommandError>> {
    let rest = line.trim().strip_prefix('/')?;
    let (name, arg) = rest
        .split_once(char::is_whitespace)
        .map_or((rest, ""), |(n, a)| (n, a.trim()));
    let no_arg = |name: &'static str, cmd: Command| {
        if arg.is_empty() {
            Ok(cmd)
        } else {
            Err(CommandError::UnexpectedArgument(name))
        }
    };
    Some(match name {
        "stage" => no_arg("stage", Command::Stage),
        "skip" => no_arg("skip", Command::Skip),
        "abort" => no_arg("abort", Command::Abort),
        "back" if arg.is_empty() => Err(CommandError::MissingStage),
        "back" => parse_stage(arg).map(Command::Back),
        _ => return None,
    })
}

/// A stage by number (`5`), name (`hypotheses`, or the Spanish
/// `hipótesis`; accents and case ignored), id (`critical_review`) or unique
/// name prefix of at least three letters (`defin`, `crit`, `revis`).
///
/// # Errors
///
/// [`CommandError::UnknownStage`] when it matches no stage or several.
pub fn parse_stage(text: &str) -> Result<Stage, CommandError> {
    let unknown = || CommandError::UnknownStage(text.to_owned());
    let wanted = normalize(text);
    if let Ok(n) = wanted.parse::<u32>() {
        return Stage::from_index(n).ok_or_else(unknown);
    }
    let id = |s: Stage| {
        serde_json::to_value(s)
            .ok()
            .and_then(|v| v.as_str().map(normalize))
    };
    let names = |s: Stage| [normalize(s.name()), normalize(s.name_es())];
    if let Some(stage) = Stage::ALL
        .into_iter()
        .find(|s| names(*s).contains(&wanted) || id(*s).as_deref() == Some(wanted.as_str()))
    {
        return Ok(stage);
    }
    if wanted.chars().count() < 3 {
        return Err(unknown());
    }
    let mut prefixed = Stage::ALL
        .into_iter()
        .filter(|s| names(*s).iter().any(|n| n.starts_with(&wanted)));
    match (prefixed.next(), prefixed.next()) {
        (Some(stage), None) => Ok(stage),
        _ => Err(unknown()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_four_commands() {
        assert_eq!(parse_command("/stage"), Some(Ok(Command::Stage)));
        assert_eq!(parse_command("  /skip "), Some(Ok(Command::Skip)));
        assert_eq!(parse_command("/abort"), Some(Ok(Command::Abort)));
        assert_eq!(
            parse_command("/back 1"),
            Some(Ok(Command::Back(Stage::Intake)))
        );
        assert_eq!(
            parse_command("/back   Hipótesis"),
            Some(Ok(Command::Back(Stage::Hypotheses)))
        );
        assert_eq!(
            parse_command("/back hypotheses"),
            Some(Ok(Command::Back(Stage::Hypotheses)))
        );
    }

    #[test]
    fn leaves_other_lines_to_the_chat() {
        assert_eq!(parse_command("hello"), None);
        assert_eq!(parse_command("/model opus"), None);
        assert_eq!(parse_command("/stages"), None);
    }

    #[test]
    fn reports_bad_arguments() {
        assert_eq!(
            parse_command("/back"),
            Some(Err(CommandError::MissingStage))
        );
        assert_eq!(
            parse_command("/skip 3"),
            Some(Err(CommandError::UnexpectedArgument("skip")))
        );
        assert!(matches!(
            parse_command("/back 15"),
            Some(Err(CommandError::UnknownStage(_)))
        ));
    }

    #[test]
    fn stages_by_number_name_id_or_prefix() {
        for stage in Stage::ALL {
            assert_eq!(parse_stage(&stage.index().to_string()), Ok(stage));
            assert_eq!(parse_stage(stage.name()), Ok(stage));
            assert_eq!(parse_stage(&stage.name().to_uppercase()), Ok(stage));
            assert_eq!(parse_stage(stage.name_es()), Ok(stage));
            assert_eq!(parse_stage(&stage.name_es().to_uppercase()), Ok(stage));
        }
        assert_eq!(parse_stage("observacion"), Ok(Stage::Observation));
        assert_eq!(parse_stage("critical_review"), Ok(Stage::CriticalReview));
        assert_eq!(parse_stage("defin"), Ok(Stage::Definition));
        assert_eq!(parse_stage("revisión"), Ok(Stage::CriticalReview));
        assert_eq!(parse_stage("lluvia"), Ok(Stage::Brainstorm));
        assert_eq!(parse_stage("crit"), Ok(Stage::CriticalReview));
        assert_eq!(parse_stage("brain"), Ok(Stage::Brainstorm));
        assert_eq!(parse_stage("data"), Ok(Stage::Data));
        // Prefixes need three letters.
        assert!(parse_stage("p").is_err());
        assert!(parse_stage("pr").is_err());
        assert!(parse_stage("0").is_err());
        assert!(parse_stage("zzz").is_err());
    }
}
