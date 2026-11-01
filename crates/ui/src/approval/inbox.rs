//! The channel between [`UiApproval`], which the use cases call off the UI
//! thread, and the dialog that answers its requests.

use application_core::ports::approval::{
    ApprovalError, ApprovalPort, ApprovalRequest, ApprovalResponse,
};
use async_trait::async_trait;
use futures::StreamExt as _;
use futures::channel::{mpsc, oneshot};

/// One request waiting for the user, as received by the dialog.
#[derive(Debug)]
pub struct PendingApproval {
    request: ApprovalRequest,
    reply: oneshot::Sender<ApprovalResponse>,
    cancelled: Option<oneshot::Receiver<()>>,
}

impl PendingApproval {
    /// The request to show.
    pub fn request(&self) -> &ApprovalRequest {
        &self.request
    }

    /// Whether the requester stopped waiting (its future was dropped).
    pub fn is_cancelled(&self) -> bool {
        self.reply.is_canceled()
    }

    /// Sends the answer. Returns `false` when nobody is waiting any more.
    pub fn respond(self, response: ApprovalResponse) -> bool {
        self.reply.send(response).is_ok()
    }

    /// A future that resolves when the requester stops waiting, either
    /// because it was answered or because its future was dropped. Only the
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

/// The receiving end of [`UiApproval`], consumed by the dialog view.
#[derive(Debug)]
pub struct ApprovalInbox {
    rx: mpsc::UnboundedReceiver<PendingApproval>,
}

impl ApprovalInbox {
    /// The next request, or `None` when every [`UiApproval`] is gone.
    pub async fn next(&mut self) -> Option<PendingApproval> {
        self.rx.next().await
    }
}

/// [`ApprovalPort`] answered by the UI. Cheap to clone; every clone feeds the
/// same [`ApprovalInbox`].
#[derive(Debug, Clone)]
pub struct UiApproval {
    tx: mpsc::UnboundedSender<PendingApproval>,
}

impl UiApproval {
    /// A port and the inbox the dialog reads its requests from.
    pub fn channel() -> (Self, ApprovalInbox) {
        let (tx, rx) = mpsc::unbounded();
        (Self { tx }, ApprovalInbox { rx })
    }
}

#[async_trait]
impl ApprovalPort for UiApproval {
    async fn request(&self, req: ApprovalRequest) -> Result<ApprovalResponse, ApprovalError> {
        let (reply, answer) = oneshot::channel();
        // Held until this future ends; dropping it signals cancellation.
        let (_alive, cancelled) = oneshot::channel::<()>();
        let pending = PendingApproval {
            request: req,
            reply,
            cancelled: Some(cancelled),
        };
        self.tx
            .unbounded_send(pending)
            .map_err(|_| ApprovalError::Unavailable("the approval dialog is closed".into()))?;
        answer.await.map_err(|_| ApprovalError::Cancelled)
    }
}
