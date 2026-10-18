//! Command line of `crest`.

use std::path::PathBuf;

use clap::Parser;
use domain::permission::PermissionMode;

/// Agente de código en la terminal y en una ventana GPUI.
#[derive(Debug, Clone, PartialEq, Eq, Parser)]
#[command(name = "crest", version, about)]
pub struct Args {
    /// Workspace a abrir (por defecto, el directorio actual).
    #[arg(value_name = "DIR", conflicts_with = "workspace")]
    pub dir: Option<PathBuf>,

    /// Workspace a abrir; equivalente al argumento posicional.
    #[arg(long, value_name = "DIR")]
    pub workspace: Option<PathBuf>,

    /// Corre un solo turno con este prompt, sin UI, imprimiendo los eventos.
    /// Sin prompt, junto a `--implement`.
    #[arg(long, value_name = "PROMPT", num_args = 0..=1, default_missing_value = "")]
    pub headless: Option<String>,

    /// En headless, implementa el plan de un Spec: su carpeta (o su nombre
    /// en `.crest/specs/`) o la ruta a su `plan.json`. Las compuertas se
    /// responden por stdin.
    #[arg(
        long,
        value_name = "RUTA",
        requires = "headless",
        conflicts_with = "research"
    )]
    pub implement: Option<String>,

    /// En headless, corre el flujo investigativo sobre este problema hasta
    /// escribir su Spec (`resume` retoma el flujo pendiente). Las compuertas
    /// se responden por stdin.
    #[arg(long, value_name = "PROBLEMA", requires = "headless")]
    pub research: Option<String>,

    /// Modelo del turno: alias (`opus`), rol (`plan`) o id completo.
    #[arg(long, value_name = "ALIAS|ID")]
    pub model: Option<String>,

    /// Modo de permisos inicial: ask, auto-edit o plan.
    #[arg(long, value_name = "MODE")]
    pub mode: Option<PermissionMode>,

    /// En headless, aprueba todas las acciones sin preguntar.
    #[arg(long, requires = "headless")]
    pub yes: bool,

    /// Abre la ventana con una sesión nueva en vez de reanudar la última
    /// del workspace.
    #[arg(long, conflicts_with = "headless")]
    pub new: bool,

    /// En headless, continúa la última sesión del workspace (con su
    /// historial) en vez de empezar una nueva.
    #[arg(long = "continue", requires = "headless")]
    pub continue_session: bool,

    /// Confía en el servidor MCP NAME del `.crest/init.lua` del workspace y
    /// lo arranca; se recuerda para este workspace. Repetible.
    #[arg(long = "trust-mcp", value_name = "NAME")]
    pub trust_mcp: Vec<String>,
}

/// What a headless run does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HeadlessRun<'a> {
    /// One turn answering the prompt.
    Prompt(&'a str),
    /// The implementation flow of the plan named by the target.
    Implement(&'a str),
    /// The research flow of the problem (`resume`: the pending one).
    Research(&'a str),
}

impl Args {
    /// What `--headless` runs, if given.
    ///
    /// # Errors
    ///
    /// A message when `--headless` gets neither a prompt nor `--implement`,
    /// or both.
    pub fn headless_run(&self) -> Option<Result<HeadlessRun<'_>, &'static str>> {
        let prompt = self.headless.as_deref()?;
        let flow = match (self.implement.as_deref(), self.research.as_deref()) {
            (Some(target), _) => Some(HeadlessRun::Implement(target)),
            (None, Some(problem)) => Some(HeadlessRun::Research(problem)),
            (None, None) => None,
        };
        Some(match (prompt.trim().is_empty(), flow) {
            (false, None) => Ok(HeadlessRun::Prompt(prompt)),
            (true, Some(flow)) => Ok(flow),
            (true, None) => {
                Err("--headless necesita un prompt, --implement <ruta> o --research <problema>")
            }
            (false, Some(_)) => {
                Err("--headless lleva un prompt o un flujo (--implement, --research), no ambos")
            }
        })
    }

    /// The workspace given on the command line, if any.
    #[must_use]
    pub fn workspace_dir(&self) -> Option<&PathBuf> {
        self.workspace.as_ref().or(self.dir.as_ref())
    }
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    use super::*;

    fn parse(args: &[&str]) -> Result<Args, clap::Error> {
        Args::try_parse_from(std::iter::once("crest").chain(args.iter().copied()))
    }

    #[test]
    fn the_definition_is_consistent() {
        Args::command().debug_assert();
    }

    #[test]
    fn no_arguments_opens_the_current_directory() {
        let args = parse(&[]).unwrap();
        assert_eq!(args.workspace_dir(), None);
        assert_eq!(args.headless, None);
        assert!(!args.yes);
    }

    #[test]
    fn workspace_comes_from_the_flag_or_the_positional() {
        let flag = parse(&["--workspace", "/w"]).unwrap();
        assert_eq!(flag.workspace_dir(), Some(&PathBuf::from("/w")));
        let positional = parse(&["/p"]).unwrap();
        assert_eq!(positional.workspace_dir(), Some(&PathBuf::from("/p")));
        assert!(parse(&["/p", "--workspace", "/w"]).is_err());
    }

    #[test]
    fn headless_options_are_parsed() {
        let args = parse(&[
            "--headless",
            "explica main",
            "--model",
            "opus",
            "--mode",
            "auto-edit",
            "--yes",
        ])
        .unwrap();
        assert_eq!(args.headless.as_deref(), Some("explica main"));
        assert_eq!(args.model.as_deref(), Some("opus"));
        assert_eq!(args.mode, Some(PermissionMode::AutoEdit));
        assert!(args.yes);
    }

    #[test]
    fn implement_runs_headless_without_a_prompt() {
        let args = parse(&["--headless", "--implement", "0001-x"]).unwrap();
        assert_eq!(
            args.headless_run(),
            Some(Ok(HeadlessRun::Implement("0001-x")))
        );
        let prompt = parse(&["--headless", "hola"]).unwrap();
        assert_eq!(prompt.headless_run(), Some(Ok(HeadlessRun::Prompt("hola"))));
        assert!(
            parse(&["--headless"])
                .unwrap()
                .headless_run()
                .unwrap()
                .is_err()
        );
        let both = parse(&["--headless", "hola", "--implement", "x"]).unwrap();
        assert!(both.headless_run().unwrap().is_err());
        assert!(parse(&["--implement", "x"]).is_err());
        assert_eq!(parse(&[]).unwrap().headless_run(), None);
    }

    #[test]
    fn research_runs_headless_without_a_prompt() {
        let args = parse(&["--headless", "--research", "add suma mal"]).unwrap();
        assert_eq!(
            args.headless_run(),
            Some(Ok(HeadlessRun::Research("add suma mal")))
        );
        assert!(parse(&["--research", "x"]).is_err());
        assert!(parse(&["--headless", "--research", "x", "--implement", "y"]).is_err());
        let both = parse(&["--headless", "hola", "--research", "x"]).unwrap();
        assert!(both.headless_run().unwrap().is_err());
    }

    #[test]
    fn trust_mcp_can_repeat() {
        assert!(parse(&[]).unwrap().trust_mcp.is_empty());
        let args = parse(&["--trust-mcp", "git", "--trust-mcp", "docs"]).unwrap();
        assert_eq!(args.trust_mcp, ["git", "docs"]);
    }

    #[test]
    fn new_is_for_the_window_and_continue_for_headless() {
        let args = parse(&[]).unwrap();
        assert!(!args.new && !args.continue_session);
        assert!(parse(&["--new"]).unwrap().new);
        assert!(
            parse(&["--headless", "hola", "--continue"])
                .unwrap()
                .continue_session
        );
        assert!(parse(&["--continue"]).is_err());
        assert!(parse(&["--new", "--headless", "hola"]).is_err());
    }

    #[test]
    fn invalid_mode_and_yes_without_headless_are_rejected() {
        assert!(parse(&["--mode", "yolo"]).is_err());
        assert!(parse(&["--yes"]).is_err());
    }
}
