//! The console of the GUI binary on Windows.
//!
//! On Windows crest is linked as a GUI program (`windows_subsystem =
//! "windows"`), so double-clicking it opens no console window; the price is
//! that it starts without one even when typed in a terminal. The command
//! line modes call [`attach_parent_console`] to write to the terminal they
//! were started from. Both functions do nothing on other platforms.

/// Attaches to the console of the parent process (the terminal crest was
/// started from) when stdout and stderr are not already connected, so a
/// redirection like `crest --headless > out.txt` keeps its file. Returns
/// whether a console was attached. Text written afterwards reaches the
/// terminal, but the shell does not wait for a GUI program: its prompt may
/// come back before crest's output.
#[must_use]
pub fn attach_parent_console() -> bool {
    imp::attach_parent_console()
}

/// Turns on ANSI escape sequences (colours) in the attached console, which
/// older Windows consoles show as garbage otherwise. Returns whether they
/// are on; `true` off Windows.
#[must_use]
pub fn enable_ansi() -> bool {
    imp::enable_ansi()
}

#[cfg(windows)]
#[allow(unsafe_code)]
mod imp {
    use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
    use windows_sys::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, CONSOLE_MODE, ENABLE_VIRTUAL_TERMINAL_PROCESSING,
        GetConsoleMode, GetStdHandle, STD_ERROR_HANDLE, STD_HANDLE, STD_OUTPUT_HANDLE,
        SetConsoleMode,
    };

    fn handle(which: STD_HANDLE) -> Option<HANDLE> {
        // SAFETY: GetStdHandle takes no pointers and returns a handle owned
        // by the process (or null / INVALID_HANDLE_VALUE).
        let handle = unsafe { GetStdHandle(which) };
        (!handle.is_null() && handle != INVALID_HANDLE_VALUE).then_some(handle)
    }

    pub(super) fn attach_parent_console() -> bool {
        if handle(STD_OUTPUT_HANDLE).is_some() || handle(STD_ERROR_HANDLE).is_some() {
            // Redirected, or already a console program.
            return false;
        }
        // SAFETY: AttachConsole takes a process id and no pointers.
        unsafe { AttachConsole(ATTACH_PARENT_PROCESS) != 0 }
    }

    pub(super) fn enable_ansi() -> bool {
        let mut enabled = false;
        for which in [STD_OUTPUT_HANDLE, STD_ERROR_HANDLE] {
            let Some(handle) = handle(which) else {
                continue;
            };
            let mut mode: CONSOLE_MODE = 0;
            // SAFETY: `handle` is a live standard handle and `mode` a valid
            // out pointer for the duration of the call. A handle that is not
            // a console (a file, a pipe) makes the calls fail harmlessly.
            enabled |= unsafe {
                GetConsoleMode(handle, &raw mut mode) != 0
                    && SetConsoleMode(handle, mode | ENABLE_VIRTUAL_TERMINAL_PROCESSING) != 0
            };
        }
        enabled
    }
}

#[cfg(not(windows))]
mod imp {
    pub(super) fn attach_parent_console() -> bool {
        false
    }

    pub(super) fn enable_ansi() -> bool {
        true
    }
}
