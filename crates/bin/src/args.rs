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
    #[arg(long, value_name = "PROMPT")]
    pub headless: Option<String>,

    /// Modelo del turno: alias (`opus`), rol (`plan`) o id completo.
    #[arg(long, value_name = "ALIAS|ID")]
    pub model: Option<String>,

    /// Modo de permisos inicial: ask, auto-edit o plan.
    #[arg(long, value_name = "MODE")]
    pub mode: Option<PermissionMode>,

    /// En headless, aprueba todas las acciones sin preguntar.
    #[arg(long, requires = "headless")]
    pub yes: bool,
}

impl Args {
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
    fn invalid_mode_and_yes_without_headless_are_rejected() {
        assert!(parse(&["--mode", "yolo"]).is_err());
        assert!(parse(&["--yes"]).is_err());
    }
}
