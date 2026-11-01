//! `/login` and `/logout`: the API key dialog and its check.

use application::ports::events::NoticeLevel;
use domain::model::Account;
use gpui::{Context, Entity, Window};

use super::{LOGIN_TIMEOUT, Session, SessionEvent};
use crate::login::{LoginEvent, LoginView};

impl Session {
    /// `/login [account]`: opens the key dialog, once nothing runs.
    pub(super) fn open_login(&mut self, arg: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(account) = self.account_arg("/login", arg, cx) else {
            return;
        };
        if !self.idle_for("/login", cx) {
            return;
        }
        let account = account.unwrap_or(Account::Anthropic);
        self.login
            .update(cx, |login, cx| login.show(account, window, cx));
    }

    /// The account named by the argument of `command` (`None` when there
    /// is none); warns and returns `None` for an unknown one.
    fn account_arg(
        &mut self,
        command: &str,
        arg: &str,
        cx: &mut Context<Self>,
    ) -> Option<Option<Account>> {
        if arg.is_empty() {
            return Some(None);
        }
        match arg.parse::<Account>() {
            Ok(account) => Some(Some(account)),
            Err(_) => {
                let known = Account::ALL.map(Account::as_str).join(", ");
                let message = format!("{command}: unknown account `{arg}` (known: {known})");
                self.notice(NoticeLevel::Warning, message, cx);
                None
            }
        }
    }

    /// What the key dialog asks.
    pub(super) fn on_login(
        &mut self,
        _: &Entity<LoginView>,
        event: &LoginEvent,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        match event {
            LoginEvent::Cancelled => {
                // A check in flight is dropped, which cancels it.
                self.login_check = None;
                self.focus_input(window, cx);
            }
            LoginEvent::Submit { account, key } => {
                let login = self.controller.login(*account, key.clone());
                let timeout = cx.background_executor().timer(LOGIN_TIMEOUT);
                let name = account.display_name();
                self.login_check = Some(cx.spawn_in(window, async move |this, cx| {
                    let result = match futures::future::select(login, timeout).await {
                        futures::future::Either::Left((result, _)) => result,
                        futures::future::Either::Right(_) => Err(format!(
                            "{name} did not answer in {} s; check the network and try again",
                            LOGIN_TIMEOUT.as_secs()
                        )),
                    };
                    this.update_in(cx, |this, window, cx| {
                        this.login_check = None;
                        match result {
                            Ok(message) => {
                                this.login.update(cx, |login, cx| login.close(window, cx));
                                cx.emit(SessionEvent::Reopen {
                                    notice: Some((NoticeLevel::Info, message)),
                                });
                            }
                            Err(error) => {
                                this.login.update(cx, |login, cx| login.fail(error, cx));
                            }
                        }
                    })
                    .ok();
                }));
            }
        }
    }

    /// `/logout [account]`: deletes the stored key (of every account
    /// without an argument) and reopens the workspace.
    pub(super) fn logout(&mut self, arg: &str, window: &mut Window, cx: &mut Context<Self>) {
        let Some(account) = self.account_arg("/logout", arg, cx) else {
            return;
        };
        if !self.idle_for("/logout", cx) {
            return;
        }
        let logout = self.controller.logout(account);
        cx.spawn_in(window, async move |this, cx| {
            let result = logout.await;
            this.update(cx, |this, cx| match result {
                Ok(message) => cx.emit(SessionEvent::Reopen {
                    notice: Some((NoticeLevel::Info, message)),
                }),
                Err(error) => this.notice(NoticeLevel::Warning, error, cx),
            })
            .ok();
        })
        .detach();
    }
}
