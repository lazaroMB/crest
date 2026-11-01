//! Cooperative cancellation of a turn, without depending on a runtime.
//!
//! The caller keeps a [`CancelToken`] and calls [`CancelToken::cancel`]
//! (e.g. on `Ctrl+C`); the use case races every await point against
//! [`CancelToken::cancelled`] and drops the losing future, which is how
//! ports are cancelled (the LLM stream closes its connection, the shell
//! kills its process).

use std::fmt;
use std::future::Future;
use std::pin::{Pin, pin};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::task::{Context, Poll, Waker};

use futures::future::{Either, select};

#[derive(Default)]
struct Inner {
    cancelled: AtomicBool,
    wakers: Mutex<Vec<Waker>>,
}

/// A cancellation flag shared by every clone.
#[derive(Clone, Default)]
pub struct CancelToken {
    inner: Arc<Inner>,
}

impl CancelToken {
    /// A token not cancelled yet.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Cancels the token and wakes everything waiting on it. Idempotent.
    pub fn cancel(&self) {
        self.inner.cancelled.store(true, Ordering::SeqCst);
        let wakers = std::mem::take(
            &mut *self
                .inner
                .wakers
                .lock()
                .unwrap_or_else(PoisonError::into_inner),
        );
        for waker in wakers {
            waker.wake();
        }
    }

    /// Whether [`cancel`](Self::cancel) was called.
    #[must_use]
    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    /// A future that completes once the token is cancelled.
    #[must_use]
    pub fn cancelled(&self) -> Cancelled {
        Cancelled {
            token: self.clone(),
        }
    }

    /// Runs `fut` unless the token is cancelled first; on cancellation
    /// `fut` is dropped and [`Interrupted`] returned. A token cancelled
    /// before the call never polls `fut`.
    ///
    /// # Errors
    ///
    /// [`Interrupted`] when the token was cancelled before `fut` finished.
    pub async fn run<F: Future>(&self, fut: F) -> Result<F::Output, Interrupted> {
        if self.is_cancelled() {
            return Err(Interrupted);
        }
        match select(pin!(fut), self.cancelled()).await {
            Either::Left((output, _)) => Ok(output),
            Either::Right(((), _)) => Err(Interrupted),
        }
    }
}

impl fmt::Debug for CancelToken {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("CancelToken")
            .field("cancelled", &self.is_cancelled())
            .finish()
    }
}

/// Two tokens are equal when they are clones of each other.
impl PartialEq for CancelToken {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.inner, &other.inner)
    }
}

impl Eq for CancelToken {}

/// The work was interrupted by a [`CancelToken`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
#[error("cancelled by user")]
pub struct Interrupted;

/// Future returned by [`CancelToken::cancelled`].
#[derive(Debug)]
pub struct Cancelled {
    token: CancelToken,
}

impl Future for Cancelled {
    type Output = ();

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<()> {
        if self.token.is_cancelled() {
            return Poll::Ready(());
        }
        {
            let mut wakers = self
                .token
                .inner
                .wakers
                .lock()
                .unwrap_or_else(PoisonError::into_inner);
            if !wakers.iter().any(|w| w.will_wake(cx.waker())) {
                wakers.push(cx.waker().clone());
            }
        }
        // `cancel` may have run between the first check and the push.
        if self.token.is_cancelled() {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::AtomicUsize;

    use super::*;

    #[tokio::test]
    async fn run_returns_the_output_when_not_cancelled() {
        let token = CancelToken::new();
        assert_eq!(token.run(async { 7 }).await, Ok(7));
    }

    #[tokio::test]
    async fn cancel_drops_the_pending_future() {
        struct Guard(Arc<AtomicUsize>);
        impl Drop for Guard {
            fn drop(&mut self) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }
        let dropped = Arc::new(AtomicUsize::new(0));
        let token = CancelToken::new();
        let guard = Guard(Arc::clone(&dropped));
        let work = async move {
            let _guard = guard;
            futures::future::pending::<()>().await;
        };
        let canceller = async {
            tokio::task::yield_now().await;
            token.cancel();
        };
        let (result, ()) = futures::join!(token.run(work), canceller);
        assert_eq!(result, Err(Interrupted));
        assert_eq!(dropped.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn a_cancelled_token_never_polls_the_future() {
        let token = CancelToken::new();
        token.clone().cancel();
        assert!(token.is_cancelled());
        let polled = AtomicUsize::new(0);
        let result = token
            .run(async {
                polled.fetch_add(1, Ordering::SeqCst);
            })
            .await;
        assert_eq!(result, Err(Interrupted));
        assert_eq!(polled.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn clones_are_equal_and_fresh_tokens_are_not() {
        let token = CancelToken::new();
        assert_eq!(token, token.clone());
        assert_ne!(token, CancelToken::new());
    }
}
