//! The environment of the user's login shell for crest.app on macOS.
//!
//! An app opened from the Dock, Launchpad or Spotlight does not inherit
//! the terminal's environment: its `PATH` is /usr/bin:/bin:/usr/sbin:/sbin,
//! without Homebrew, so crest would not find rg, fzf or git. The login
//! shell reads ~/.zprofile, ~/.profile or ~/.bash_profile (where
//! `brew shellenv` and ~/.cargo/bin usually live).
//!
//! [`reexec_from_app`] runs first thing in `main`: when LaunchServices
//! started the app, it replaces the process with
//! `$SHELL -l -c 'exec "$0" "$@"' <crest> <args>`, the same thing the
//! shell launcher inside crest.app used to do (the app's executable is
//! now the binary itself, so it can be signed and notarized). The shell is
//! not interactive (`-i`): an ~/.zshrc that prompts or execs tmux would
//! keep the app from starting. On Linux the desktop entry still goes
//! through `crest-launcher`.

use std::ffi::OsStr;
use std::path::PathBuf;

/// The `CFBundleIdentifier` of crest.app (assets/macos/Info.plist.in).
pub const BUNDLE_ID: &str = "mx.twocrest.crest";

/// Set on the re-executed process so it does not go through the shell
/// again (and inherited by what it starts).
pub const GUARD_VAR: &str = "CREST_LOGIN_SHELL";

/// Whether this process must re-execute through the login shell, given
/// `__CFBundleIdentifier` (which LaunchServices sets to the bundle it
/// opens; a terminal sets its own) and [`GUARD_VAR`].
pub fn should_reexec(bundle_id: Option<&OsStr>, guard: Option<&OsStr>) -> bool {
    guard.is_none() && bundle_id == Some(OsStr::new(BUNDLE_ID))
}

/// The login shell for `SHELL`: a POSIX-like one as is, `/bin/zsh` (the
/// macOS default) when unset, `/bin/sh` for fish, nu, xonsh... (other
/// syntax; a POSIX login shell still reads ~/.profile).
pub fn login_shell(shell: Option<&OsStr>) -> PathBuf {
    let Some(shell) = shell.filter(|shell| !shell.is_empty()) else {
        return PathBuf::from("/bin/zsh");
    };
    let path = PathBuf::from(shell);
    let posix = path
        .file_name()
        .and_then(OsStr::to_str)
        .is_some_and(|name| {
            matches!(
                name,
                "sh" | "bash" | "zsh" | "dash" | "ksh" | "mksh" | "yash"
            )
        });
    if posix {
        path
    } else {
        PathBuf::from("/bin/sh")
    }
}

/// Replaces the process with this binary run through the login shell when
/// LaunchServices started it (see the module docs). Returns, doing
/// nothing, from a terminal or when the guard is set; also when the exec
/// fails (logged to stderr), so crest still starts with the app's
/// environment.
#[cfg(target_os = "macos")]
pub fn reexec_from_app() {
    use std::os::unix::process::CommandExt as _;

    if !should_reexec(
        std::env::var_os("__CFBundleIdentifier").as_deref(),
        std::env::var_os(GUARD_VAR).as_deref(),
    ) {
        return;
    }
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(err) => {
            eprintln!("crest: cannot find its own binary: {err}");
            return;
        }
    };
    let shell = login_shell(std::env::var_os("SHELL").as_deref());
    let err = std::process::Command::new(&shell)
        .arg("-l")
        .arg("-c")
        .arg(r#"exec "$0" "$@""#)
        .arg(&exe)
        .args(std::env::args_os().skip(1))
        .env(GUARD_VAR, "1")
        .exec();
    eprintln!(
        "crest: cannot start through the login shell {}: {err}",
        shell.display()
    );
}

/// Nothing outside macOS (see the module docs).
#[cfg(not(target_os = "macos"))]
pub fn reexec_from_app() {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reexecs_only_when_launch_services_opened_the_app() {
        let ours = Some(OsStr::new(BUNDLE_ID));
        assert!(should_reexec(ours, None));
        assert!(!should_reexec(ours, Some(OsStr::new("1"))));
        assert!(!should_reexec(Some(OsStr::new("com.apple.Terminal")), None));
        assert!(!should_reexec(None, None));
    }

    #[test]
    fn picks_a_posix_login_shell() {
        assert_eq!(login_shell(None), PathBuf::from("/bin/zsh"));
        assert_eq!(login_shell(Some(OsStr::new(""))), PathBuf::from("/bin/zsh"));
        assert_eq!(
            login_shell(Some(OsStr::new("/opt/homebrew/bin/bash"))),
            PathBuf::from("/opt/homebrew/bin/bash")
        );
        assert_eq!(
            login_shell(Some(OsStr::new("/opt/homebrew/bin/fish"))),
            PathBuf::from("/bin/sh")
        );
    }
}
