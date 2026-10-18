//! `Ctrl+C` in headless mode: the first one cancels the turn, the second
//! one exits right away with code 130.

use std::future::Future;

use application::usecases::cancel::CancelToken;

use super::Exit;
use super::terminal::{Style, Terminal};

/// Waits for interrupts from `signals`: the first cancels `cancel` and
/// says so on `terminal`; the second calls `exit`. Ends when `signals`
/// stops producing them.
pub async fn on_interrupt<S, F>(
    mut signals: S,
    cancel: CancelToken,
    terminal: Terminal,
    exit: fn(i32),
) where
    S: FnMut() -> F,
    F: Future<Output = bool>,
{
    if !signals().await {
        return;
    }
    cancel.cancel();
    terminal.lock().line(
        Some(Style::Yellow),
        "^C interrupting the turn (again to quit)",
    );
    if signals().await {
        exit(i32::from(Exit::Interrupted.code()));
    }
}

/// Watches the real `Ctrl+C` (SIGINT) in the background.
pub fn watch_ctrl_c(cancel: CancelToken, terminal: Terminal) -> tokio::task::JoinHandle<()> {
    tokio::spawn(on_interrupt(
        || async { tokio::signal::ctrl_c().await.is_ok() },
        cancel,
        terminal,
        |code| {
            std::process::exit(code);
        },
    ))
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicI32, Ordering};

    use super::*;
    use crate::headless::terminal::tests::terminal;

    static EXITED: AtomicI32 = AtomicI32::new(-1);

    fn record_exit(code: i32) {
        EXITED.store(code, Ordering::SeqCst);
    }

    #[tokio::test]
    async fn first_interrupt_cancels_and_second_exits() {
        let cancel = CancelToken::new();
        let (term, buf) = terminal(false);
        on_interrupt(|| async { true }, cancel.clone(), term, record_exit).await;
        assert!(cancel.is_cancelled());
        assert!(buf.text().contains("interrupting"));
        assert_eq!(EXITED.load(Ordering::SeqCst), 130);
    }

    #[tokio::test]
    async fn no_signal_leaves_the_turn_alone() {
        let cancel = CancelToken::new();
        let (term, buf) = terminal(false);
        on_interrupt(
            || async { false },
            cancel.clone(),
            term,
            |_| panic!("exited"),
        )
        .await;
        assert!(!cancel.is_cancelled());
        assert_eq!(buf.text(), "");
    }
}
