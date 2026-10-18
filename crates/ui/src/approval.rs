//! The approval dialog: [`UiApproval`] implements [`ApprovalPort`] for the
//! use cases, which run off the UI thread, and hands each request to the
//! window through a channel.
//!
//! `request()` sends a [`PendingApproval`] (the request, a oneshot to reply
//! on and a cancellation signal) and waits for the reply. Dropping that
//! future drops its end of the signal, so the dialog showing the request
//! closes; a closed dialog drops the reply, so the request fails with
//! [`ApprovalError::Cancelled`].

use application::ports::approval::{
    ApprovalError, ApprovalPort, ApprovalRequest, ApprovalResponse,
};
use async_trait::async_trait;
use domain::permission::{PermissionRule, RuleDecision, RuleMatcher};
use futures::StreamExt as _;
use futures::channel::{mpsc, oneshot};

/// The "allow always" rule for `request`: same tool, exactly this subject.
pub fn allow_always_rule(request: &ApprovalRequest) -> PermissionRule {
    PermissionRule::new(
        request.tool.clone(),
        RuleMatcher::Exact(request.subject.clone()),
        RuleDecision::AllowAlways,
    )
}

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

#[cfg(test)]
mod tests {
    use application::ports::approval::{ApprovalPreview, ApprovalRequestId};
    use domain::conversation::ToolUseId;
    use domain::permission::ToolEffect;
    use futures::FutureExt as _;
    use futures::executor::block_on;

    use super::*;

    pub(super) fn command_request(id: u64) -> ApprovalRequest {
        ApprovalRequest {
            id: ApprovalRequestId(id),
            tool_use_id: ToolUseId::new(format!("toolu_{id}")),
            tool: "run_command".into(),
            effect: ToolEffect::Execute,
            subject: "cargo test".into(),
            preview: ApprovalPreview::Command {
                command: "cargo".into(),
                args: vec!["test".into()],
                cwd: ".".into(),
            },
        }
    }

    #[test]
    fn reply_reaches_the_requester() {
        let (port, mut inbox) = UiApproval::channel();
        let mut request = port.request(command_request(1)).boxed();
        assert!(
            request.as_mut().now_or_never().is_none(),
            "waits for the UI"
        );
        let pending = block_on(inbox.next()).unwrap();
        assert_eq!(pending.request().id, ApprovalRequestId(1));
        assert!(pending.respond(ApprovalResponse::Approve));
        assert_eq!(block_on(request), Ok(ApprovalResponse::Approve));
    }

    #[test]
    fn closed_inbox_is_unavailable() {
        let (port, inbox) = UiApproval::channel();
        drop(inbox);
        let result = block_on(port.request(command_request(1)));
        assert!(matches!(result, Err(ApprovalError::Unavailable(_))));
    }

    #[test]
    fn dropped_dialog_cancels_the_request() {
        let (port, mut inbox) = UiApproval::channel();
        let request = port.request(command_request(1));
        let (result, ()) = block_on(futures::future::join(request, async {
            drop(inbox.next().await);
        }));
        assert_eq!(result, Err(ApprovalError::Cancelled));
    }

    #[test]
    fn dropped_request_signals_the_dialog() {
        let (port, mut inbox) = UiApproval::channel();
        let mut request = port.request(command_request(1)).boxed();
        assert!(request.as_mut().now_or_never().is_none());
        let mut pending = block_on(inbox.next()).unwrap();
        let cancellation = pending.cancellation();
        assert!(!pending.is_cancelled());
        drop(request);
        assert!(pending.is_cancelled());
        block_on(cancellation);
        assert!(!pending.respond(ApprovalResponse::Approve));
    }

    #[test]
    fn allow_always_matches_exactly_the_subject() {
        let rule = allow_always_rule(&command_request(1));
        assert_eq!(rule.tool, "run_command");
        assert_eq!(rule.matcher, RuleMatcher::Exact("cargo test".into()));
        assert_eq!(rule.decision, RuleDecision::AllowAlways);
    }
}
