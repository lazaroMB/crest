//! Forms of `ask_user`: [`UiForm`] implements [`FormPort`] for the use
//! cases, which run off the UI thread, and hands each request to the
//! window through a channel; [`listen`] attaches it to the `Form` block it
//! names (see [`crate::blocks::form`]), which the user fills in place.
//!
//! `ask()` sends a [`PendingForm`] (the request, a oneshot to reply on and
//! a cancellation signal) and waits for the reply. Dropping that future
//! drops its end of the signal, so the block stops offering the form; a
//! block that drops the reply cancels the form.

use application::ports::form::{FormPort, FormRequest, FormResponse};
use async_trait::async_trait;
use futures::StreamExt as _;
use futures::channel::{mpsc, oneshot};
use gpui::{App, Task, Window};

/// One form waiting for the user, as received by the window.
#[derive(Debug)]
pub struct PendingForm {
    request: FormRequest,
    reply: oneshot::Sender<FormResponse>,
    cancelled: Option<oneshot::Receiver<()>>,
}

impl PendingForm {
    /// A pending form and the receiver of its answer, for tests.
    pub fn new(request: FormRequest) -> (Self, oneshot::Receiver<FormResponse>) {
        let (reply, answer) = oneshot::channel();
        let pending = Self {
            request,
            reply,
            cancelled: None,
        };
        (pending, answer)
    }

    /// The request to show.
    pub fn request(&self) -> &FormRequest {
        &self.request
    }

    /// Whether the requester stopped waiting (its future was dropped).
    pub fn is_cancelled(&self) -> bool {
        self.reply.is_canceled()
    }

    /// Sends the answer. Returns `false` when nobody is waiting any more.
    pub fn respond(self, response: FormResponse) -> bool {
        self.reply.send(response).is_ok()
    }

    /// A future that resolves when the requester stops waiting. Only the
    /// first call gets the signal; later ones never resolve.
    pub fn cancellation(&mut self) -> impl Future<Output = ()> + Send + 'static {
        let signal = self.cancelled.take();
        async move {
            match signal {
                // The sender is never used: the only outcome is `Canceled`.
                Some(signal) => {
                    let _ = signal.await;
                }
                None => futures::future::pending::<()>().await,
            }
        }
    }
}

/// The receiving end of [`UiForm`], consumed by [`listen`].
#[derive(Debug)]
pub struct FormInbox {
    rx: mpsc::UnboundedReceiver<PendingForm>,
}

impl FormInbox {
    /// The next request, or `None` when every [`UiForm`] is gone.
    pub async fn next(&mut self) -> Option<PendingForm> {
        self.rx.next().await
    }
}

/// [`FormPort`] answered by the UI. Cheap to clone; every clone feeds the
/// same [`FormInbox`].
#[derive(Debug, Clone)]
pub struct UiForm {
    tx: mpsc::UnboundedSender<PendingForm>,
}

impl UiForm {
    /// A port and the inbox the window reads its requests from.
    pub fn channel() -> (Self, FormInbox) {
        let (tx, rx) = mpsc::unbounded();
        (Self { tx }, FormInbox { rx })
    }
}

#[async_trait]
impl FormPort for UiForm {
    async fn ask(&self, req: FormRequest) -> FormResponse {
        let (reply, answer) = oneshot::channel();
        // Held until this future ends; dropping it signals cancellation.
        let (_alive, cancelled) = oneshot::channel::<()>();
        let pending = PendingForm {
            request: req,
            reply,
            cancelled: Some(cancelled),
        };
        if self.tx.unbounded_send(pending).is_err() {
            return FormResponse::cancelled("the window is closed");
        }
        answer
            .await
            .unwrap_or_else(|_| FormResponse::cancelled("the form was closed"))
    }
}

/// Attaches every request of `inbox` to its `Form` block, which takes the
/// focus, until the inbox closes or the window goes away.
pub fn listen(mut inbox: FormInbox, window: &mut Window, cx: &mut App) -> Task<()> {
    window.spawn(cx, async move |cx| {
        while let Some(pending) = inbox.next().await {
            if cx
                .update(|window, cx| crate::blocks::form::attach(pending, window, cx))
                .is_err()
            {
                break;
            }
        }
    })
}
