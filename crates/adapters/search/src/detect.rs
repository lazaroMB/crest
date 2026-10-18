//! Detection of the external tools (`rg`, `fzf`) and their versions.
//!
//! At startup the app runs `rg --version` and `fzf --version`, compares the
//! result with the minimum versions crest needs (ripgrep 13.0, fzf 0.36,
//! the first one with `--scheme=path`) and, when a tool is missing or too
//! old, shows [`ToolStatus::problem`]: what is wrong and how to install it
//! on the current platform.

use std::ffi::OsStr;
use std::fmt;
use std::time::Duration;

use crate::process::{self, ProcessError, ProcessSpec};

/// How long `--version` may take before the tool is considered unusable.
pub const DETECT_TIMEOUT: Duration = Duration::from_secs(5);

/// A `major.minor.patch` version. Missing components are 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Version {
    /// Major component.
    pub major: u32,
    /// Minor component.
    pub minor: u32,
    /// Patch component.
    pub patch: u32,
}

impl Version {
    /// A version from its components.
    pub const fn new(major: u32, minor: u32, patch: u32) -> Self {
        Self {
            major,
            minor,
            patch,
        }
    }

    /// Parses the first token of `text` that starts with a digit, such as
    /// `15.2.0` in `ripgrep 15.2.0 (rev abc)` or `0.74.5` in
    /// `0.74.5 (c0a193e0)`. Suffixes like `-dev` are ignored.
    pub fn find_in(text: &str) -> Option<Self> {
        let token = text
            .split_whitespace()
            .find(|t| t.starts_with(|c: char| c.is_ascii_digit()))?;
        let mut parts = token.split('.').map(|part| {
            let digits: String = part.chars().take_while(char::is_ascii_digit).collect();
            digits.parse::<u32>().ok()
        });
        let major = parts.next().flatten()?;
        let minor = parts.next().flatten().unwrap_or(0);
        let patch = parts.next().flatten().unwrap_or(0);
        Some(Self::new(major, minor, patch))
    }
}

impl fmt::Display for Version {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}.{}.{}", self.major, self.minor, self.patch)
    }
}

/// An external tool crest depends on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Tool {
    /// ripgrep, used by `list_files`, `tree`, `grep` and `find_file`.
    Ripgrep,
    /// fzf, used by `find_file` (`--filter --scheme=path`).
    Fzf,
}

impl Tool {
    /// The default executable name.
    pub const fn program(self) -> &'static str {
        match self {
            Self::Ripgrep => "rg",
            Self::Fzf => "fzf",
        }
    }

    /// The oldest supported version.
    pub const fn min_version(self) -> Version {
        match self {
            Self::Ripgrep => Version::new(13, 0, 0),
            Self::Fzf => Version::new(0, 36, 0),
        }
    }

    /// Name shown to the user.
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Ripgrep => "ripgrep (`rg`)",
            Self::Fzf => "fzf",
        }
    }

    /// What crest loses without the tool.
    const fn impact(self) -> &'static str {
        match self {
            Self::Ripgrep => {
                "`grep` is unavailable and file listing falls back to a slower built-in walker"
            }
            Self::Fzf => "`find_file` falls back to plain substring matching",
        }
    }

    /// How to install the tool on the current platform.
    pub fn install_hint(self) -> String {
        install_hint(self, std::env::consts::OS)
    }
}

/// How to install `tool` on `os` (a value of [`std::env::consts::OS`]).
pub fn install_hint(tool: Tool, os: &str) -> String {
    let (package, releases) = match tool {
        Tool::Ripgrep => ("ripgrep", "https://github.com/BurntSushi/ripgrep/releases"),
        Tool::Fzf => ("fzf", "https://github.com/junegunn/fzf/releases"),
    };
    let min = tool.min_version();
    match os {
        "macos" => format!(
            "install it with `brew install {package}` (version {min} or newer), \
             or download a binary from {releases}"
        ),
        "linux" => format!(
            "run `scripts/install-deps.sh`, or install the `{package}` package \
             (`apt install {package}`, `dnf install {package}`, `pacman -S {package}`); \
             if your distribution ships a version older than {min}, put the \
             official binary from {releases} in ~/.local/bin"
        ),
        _ => format!("install version {min} or newer from {releases} and put it in PATH"),
    }
}

/// Result of checking one tool.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ToolStatus {
    /// Installed and recent enough.
    Ok {
        /// The detected version.
        version: Version,
    },
    /// Not found (or not executable).
    Missing,
    /// Installed but older than the minimum.
    TooOld {
        /// The detected version.
        found: Version,
        /// The minimum supported version.
        min: Version,
    },
    /// It ran, but `--version` printed nothing that looks like a version
    /// (or exited with an error).
    Unrecognized {
        /// The first line of what it printed.
        output: String,
    },
}

impl ToolStatus {
    /// Classifies the output of `<tool> --version`. ripgrep prints
    /// `ripgrep <version> ...` and fzf `<version> (<rev>)` on the first
    /// line; anything else (another program with the same name) is
    /// [`ToolStatus::Unrecognized`].
    pub fn from_version_output(tool: Tool, output: &str) -> Self {
        let first_line = output.lines().next().unwrap_or_default().trim();
        let version_text = match tool {
            Tool::Ripgrep => first_line.strip_prefix("ripgrep "),
            Tool::Fzf => first_line.split_whitespace().next(),
        };
        match version_text.and_then(Version::find_in) {
            Some(found) if found < tool.min_version() => Self::TooOld {
                found,
                min: tool.min_version(),
            },
            Some(version) => Self::Ok { version },
            None => Self::Unrecognized {
                output: first_line.to_owned(),
            },
        }
    }

    /// Whether the tool can be used.
    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Ok { .. })
    }

    /// A message for the user explaining what is wrong with `tool`, what
    /// stops working and how to fix it; `None` when it is fine.
    pub fn problem(&self, tool: Tool) -> Option<String> {
        let name = tool.display_name();
        let what = match self {
            Self::Ok { .. } => return None,
            Self::Missing => format!("{name} was not found in PATH"),
            Self::TooOld { found, min } => {
                format!("{name} {found} is too old (crest needs {min} or newer)")
            }
            Self::Unrecognized { output } => {
                format!("could not read the version of {name} (it printed {output:?})")
            }
        };
        Some(format!(
            "{what}; {}. To fix it, {}.",
            tool.impact(),
            tool.install_hint()
        ))
    }
}

/// Runs `<program> --version` and classifies the result for `tool`.
/// `program` is usually [`Tool::program`], or a configured path.
pub async fn detect(tool: Tool, program: &OsStr) -> ToolStatus {
    let spec = ProcessSpec::new(program, std::env::temp_dir()).arg("--version");
    match process::run(&spec, None, DETECT_TIMEOUT).await {
        Ok(out) if out.status.success() && !out.timed_out => {
            ToolStatus::from_version_output(tool, &String::from_utf8_lossy(&out.stdout))
        }
        Ok(out) => {
            let text = if out.stderr.is_empty() {
                out.stdout
            } else {
                out.stderr
            };
            let text = String::from_utf8_lossy(&text);
            ToolStatus::Unrecognized {
                output: text.lines().next().unwrap_or_default().trim().to_owned(),
            }
        }
        Err(ProcessError::NotFound { .. }) => ToolStatus::Missing,
        Err(err) => {
            tracing::debug!(%err, ?tool, "could not run --version");
            ToolStatus::Missing
        }
    }
}

/// Status of both tools.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ToolReport {
    /// ripgrep.
    pub rg: ToolStatus,
    /// fzf.
    pub fzf: ToolStatus,
}

impl ToolReport {
    /// One message per tool with a problem (see [`ToolStatus::problem`]).
    pub fn problems(&self) -> Vec<String> {
        [(Tool::Ripgrep, &self.rg), (Tool::Fzf, &self.fzf)]
            .into_iter()
            .filter_map(|(tool, status)| status.problem(tool))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ripgrep_versions() {
        let v = |s| Version::find_in(s);
        assert_eq!(
            v("ripgrep 15.2.0 (rev 1234abcd)"),
            Some(Version::new(15, 2, 0))
        );
        assert_eq!(v("ripgrep 13.0.0"), Some(Version::new(13, 0, 0)));
        assert_eq!(v("ripgrep 14.1.1-dev"), Some(Version::new(14, 1, 1)));
        assert_eq!(v("ripgrep 14"), Some(Version::new(14, 0, 0)));
        assert_eq!(v("ripgrep"), None);
    }

    #[test]
    fn parses_fzf_versions() {
        let v = |s| Version::find_in(s);
        assert_eq!(v("0.74.5 (c0a193e0)"), Some(Version::new(0, 74, 5)));
        assert_eq!(v("0.29.0 (devel)"), Some(Version::new(0, 29, 0)));
        assert_eq!(v("0.36.0 (brew)"), Some(Version::new(0, 36, 0)));
    }

    #[test]
    fn classifies_version_output() {
        let full = "ripgrep 15.2.0 (rev x)\n\nfeatures:+pcre2\n";
        assert_eq!(
            ToolStatus::from_version_output(Tool::Ripgrep, full),
            ToolStatus::Ok {
                version: Version::new(15, 2, 0)
            }
        );
        assert_eq!(
            ToolStatus::from_version_output(Tool::Fzf, "0.29.0 (devel)\n"),
            ToolStatus::TooOld {
                found: Version::new(0, 29, 0),
                min: Version::new(0, 36, 0)
            }
        );
        assert_eq!(
            ToolStatus::from_version_output(Tool::Fzf, "0.36.0\n"),
            ToolStatus::Ok {
                version: Version::new(0, 36, 0)
            }
        );
        assert!(matches!(
            ToolStatus::from_version_output(Tool::Ripgrep, "hello 1.0\n"),
            ToolStatus::Unrecognized { output } if output == "hello 1.0"
        ));
        assert!(matches!(
            ToolStatus::from_version_output(Tool::Fzf, "true (GNU coreutils) 9.1\n"),
            ToolStatus::Unrecognized { .. }
        ));
    }

    #[test]
    fn problems_explain_how_to_install() {
        assert_eq!(
            ToolStatus::Ok {
                version: Version::new(15, 0, 0)
            }
            .problem(Tool::Ripgrep),
            None
        );
        let missing = ToolStatus::Missing.problem(Tool::Ripgrep).unwrap();
        assert!(
            missing.contains("ripgrep (`rg`) was not found"),
            "{missing}"
        );
        assert!(missing.contains("install"), "{missing}");

        let old = ToolStatus::TooOld {
            found: Version::new(0, 29, 0),
            min: Version::new(0, 36, 0),
        }
        .problem(Tool::Fzf)
        .unwrap();
        assert!(old.contains("fzf 0.29.0 is too old"), "{old}");
        assert!(old.contains("0.36.0 or newer"), "{old}");
        assert!(old.contains("substring"), "{old}");
    }

    #[test]
    fn install_hints_depend_on_the_platform() {
        assert!(install_hint(Tool::Fzf, "macos").contains("brew install fzf"));
        let linux = install_hint(Tool::Ripgrep, "linux");
        assert!(linux.contains("apt install ripgrep"), "{linux}");
        assert!(linux.contains("~/.local/bin"), "{linux}");
        assert!(install_hint(Tool::Ripgrep, "freebsd").contains("releases"));
    }

    #[tokio::test]
    async fn missing_program_is_reported_with_a_clear_message() {
        let report = ToolReport {
            rg: detect(Tool::Ripgrep, OsStr::new("crest-no-such-rg")).await,
            fzf: detect(Tool::Fzf, OsStr::new("/nonexistent/fzf")).await,
        };
        assert_eq!(report.rg, ToolStatus::Missing);
        assert_eq!(report.fzf, ToolStatus::Missing);
        let problems = report.problems();
        assert_eq!(problems.len(), 2);
        assert!(problems[0].contains("ripgrep"), "{}", problems[0]);
        assert!(problems[1].contains("fzf"), "{}", problems[1]);
    }

    #[tokio::test]
    async fn non_version_output_is_unrecognized() {
        // GNU `true --version` prints "true (GNU coreutils) 9.x"; BSD
        // `true` prints nothing. Neither is ripgrep or fzf.
        for tool in [Tool::Ripgrep, Tool::Fzf] {
            let status = detect(tool, OsStr::new("true")).await;
            assert!(
                matches!(status, ToolStatus::Unrecognized { .. }),
                "{tool:?}: {status:?}"
            );
        }
    }

    #[tokio::test]
    async fn detects_installed_tools() {
        // Whatever is installed here must at least report a version.
        for tool in [Tool::Ripgrep, Tool::Fzf] {
            let status = detect(tool, OsStr::new(tool.program())).await;
            assert!(
                !matches!(status, ToolStatus::Unrecognized { .. }),
                "{tool:?}: {status:?}"
            );
        }
    }
}
