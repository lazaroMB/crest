//! The chat commands of the research flow: `/stage`, `/back <stage>`,
//! `/skip`, `/retry` and `/abort`.

use domain::research::{Pipeline, Stage};

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
    /// `/retry`: run a failed (or interrupted) stage again.
    Retry,
}

/// Why a research command could not be parsed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CommandError {
    /// `/back` without a stage; carries the number of stages of the flow.
    #[error("usage: /back <stage>, by number (1-{0}) or name, e.g. /back 2 or /back hypotheses")]
    MissingStage(u32),
    /// `/back` with something that names no stage (or several); carries the
    /// text and the number of stages of the flow.
    #[error("{0:?} names no single stage; use its number (1-{1}) or name")]
    UnknownStage(String, u32),
    /// A command that takes no argument got one.
    #[error("/{0} takes no arguments")]
    UnexpectedArgument(&'static str),
}

/// Number of stages of `pipeline`.
fn total(pipeline: Pipeline) -> u32 {
    u32::try_from(pipeline.stages().len()).unwrap_or(u32::MAX)
}

/// Parses `line` if it is a research command of a `pipeline` flow.
///
/// Returns `None` for anything else (plain text, or other `/` commands
/// such as `/model`, which belong to the chat).
#[must_use]
pub fn parse_command(line: &str, pipeline: Pipeline) -> Option<Result<Command, CommandError>> {
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
        "retry" => no_arg("retry", Command::Retry),
        "back" if arg.is_empty() => Err(CommandError::MissingStage(total(pipeline))),
        "back" => parse_stage(arg, pipeline).map(Command::Back),
        _ => return None,
    })
}

/// A stage of `pipeline` by number (`5`), name (`hypotheses`, or the
/// Spanish `hipótesis`; accents and case ignored), id (`critical_review`)
/// or unique name prefix of at least three letters (`defin`, `crit`,
/// `revis`).
///
/// Numbers count within the pipeline. A name of a deep stage that the
/// pipeline lacks resolves to the stage that owns it
/// ([`Stage::owner_in`]), so `/back hypotheses` is Framing in the fast
/// pipeline.
///
/// # Errors
///
/// [`CommandError::UnknownStage`] when it matches no stage or several.
pub fn parse_stage(text: &str, pipeline: Pipeline) -> Result<Stage, CommandError> {
    let unknown = || CommandError::UnknownStage(text.to_owned(), total(pipeline));
    let wanted = normalize(text);
    if let Ok(n) = wanted.parse::<u32>() {
        return Stage::from_index(pipeline, n).ok_or_else(unknown);
    }
    let id = |s: Stage| {
        serde_json::to_value(s)
            .ok()
            .and_then(|v| v.as_str().map(normalize))
    };
    let names = |s: Stage| [normalize(s.name()), normalize(s.name_es())];
    // The pipeline's own stages first, then every deep stage.
    let pools = [pipeline.stages(), Stage::ALL.as_slice()];
    for pool in pools {
        if let Some(stage) = pool
            .iter()
            .copied()
            .find(|s| names(*s).contains(&wanted) || id(*s).as_deref() == Some(wanted.as_str()))
        {
            return Ok(stage.owner_in(pipeline));
        }
    }
    if wanted.chars().count() < 3 {
        return Err(unknown());
    }
    for pool in pools {
        let mut prefixed = pool
            .iter()
            .copied()
            .filter(|s| names(*s).iter().any(|n| n.starts_with(&wanted)));
        match (prefixed.next(), prefixed.next()) {
            (Some(stage), None) => return Ok(stage.owner_in(pipeline)),
            (Some(_), Some(_)) => return Err(unknown()),
            _ => {}
        }
    }
    Err(unknown())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_the_four_commands() {
        assert_eq!(
            parse_command("/stage", Pipeline::Deep),
            Some(Ok(Command::Stage))
        );
        assert_eq!(
            parse_command("  /skip ", Pipeline::Deep),
            Some(Ok(Command::Skip))
        );
        assert_eq!(
            parse_command("/abort", Pipeline::Deep),
            Some(Ok(Command::Abort))
        );
        assert_eq!(
            parse_command("/retry", Pipeline::Deep),
            Some(Ok(Command::Retry))
        );
        assert_eq!(
            parse_command("/back 1", Pipeline::Deep),
            Some(Ok(Command::Back(Stage::Intake)))
        );
        assert_eq!(
            parse_command("/back   Hipótesis", Pipeline::Deep),
            Some(Ok(Command::Back(Stage::Hypotheses)))
        );
        assert_eq!(
            parse_command("/back hypotheses", Pipeline::Deep),
            Some(Ok(Command::Back(Stage::Hypotheses)))
        );
    }

    #[test]
    fn leaves_other_lines_to_the_chat() {
        assert_eq!(parse_command("hello", Pipeline::Deep), None);
        assert_eq!(parse_command("/model opus", Pipeline::Deep), None);
        assert_eq!(parse_command("/stages", Pipeline::Deep), None);
    }

    #[test]
    fn reports_bad_arguments() {
        assert_eq!(
            parse_command("/back", Pipeline::Deep),
            Some(Err(CommandError::MissingStage(14)))
        );
        assert_eq!(
            parse_command("/skip 3", Pipeline::Deep),
            Some(Err(CommandError::UnexpectedArgument("skip")))
        );
        assert!(matches!(
            parse_command("/back 15", Pipeline::Deep),
            Some(Err(CommandError::UnknownStage(..)))
        ));
    }

    #[test]
    fn stages_by_number_name_id_or_prefix() {
        for stage in Stage::ALL {
            assert_eq!(
                parse_stage(&stage.index().to_string(), Pipeline::Deep),
                Ok(stage)
            );
            assert_eq!(parse_stage(stage.name(), Pipeline::Deep), Ok(stage));
            assert_eq!(
                parse_stage(&stage.name().to_uppercase(), Pipeline::Deep),
                Ok(stage)
            );
            assert_eq!(parse_stage(stage.name_es(), Pipeline::Deep), Ok(stage));
            assert_eq!(
                parse_stage(&stage.name_es().to_uppercase(), Pipeline::Deep),
                Ok(stage)
            );
        }
        assert_eq!(
            parse_stage("observacion", Pipeline::Deep),
            Ok(Stage::Observation)
        );
        assert_eq!(
            parse_stage("critical_review", Pipeline::Deep),
            Ok(Stage::CriticalReview)
        );
        assert_eq!(parse_stage("defin", Pipeline::Deep), Ok(Stage::Definition));
        assert_eq!(
            parse_stage("revisión", Pipeline::Deep),
            Ok(Stage::CriticalReview)
        );
        assert_eq!(parse_stage("lluvia", Pipeline::Deep), Ok(Stage::Brainstorm));
        assert_eq!(
            parse_stage("crit", Pipeline::Deep),
            Ok(Stage::CriticalReview)
        );
        assert_eq!(parse_stage("brain", Pipeline::Deep), Ok(Stage::Brainstorm));
        assert_eq!(parse_stage("data", Pipeline::Deep), Ok(Stage::Data));
        // Prefixes need three letters.
        assert!(parse_stage("p", Pipeline::Deep).is_err());
        assert!(parse_stage("pr", Pipeline::Deep).is_err());
        assert!(parse_stage("0", Pipeline::Deep).is_err());
        assert!(parse_stage("zzz", Pipeline::Deep).is_err());
    }

    #[test]
    fn fast_stages_count_within_the_pipeline_and_absorb_deep_names() {
        let fast = |text| parse_stage(text, Pipeline::Fast);
        assert_eq!(fast("1"), Ok(Stage::Triage));
        assert_eq!(fast("2"), Ok(Stage::Discovery));
        assert_eq!(fast("5"), Ok(Stage::Delivery));
        assert!(fast("6").is_err());
        assert_eq!(fast("framing"), Ok(Stage::Framing));
        assert_eq!(fast("descubrimiento"), Ok(Stage::Discovery));
        assert_eq!(fast("hypotheses"), Ok(Stage::Framing));
        assert_eq!(fast("data"), Ok(Stage::Discovery));
        assert_eq!(fast("definition"), Ok(Stage::Framing));
        assert_eq!(fast("critical_review"), Ok(Stage::Delivery));
        assert_eq!(fast("brain"), Ok(Stage::Solution));
        assert_eq!(fast("intake"), Ok(Stage::Triage));
        assert_eq!(
            parse_command("/back 9", Pipeline::Fast),
            Some(Err(CommandError::UnknownStage("9".into(), 5)))
        );
        assert_eq!(
            CommandError::MissingStage(5).to_string(),
            "usage: /back <stage>, by number (1-5) or name, e.g. /back 2 or /back hypotheses"
        );
        // Deep keeps resolving deep names to themselves.
        assert_eq!(
            parse_stage("triage", Pipeline::Deep),
            Err(CommandError::UnknownStage("triage".into(), 14))
        );
    }
}
