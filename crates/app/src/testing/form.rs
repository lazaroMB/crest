//! [`ScriptedForm`]: a [`FormPort`] that answers with scripted responses
//! and records every request, for tests of `ask_user`.

use std::collections::VecDeque;
use std::sync::{Mutex, PoisonError};

use async_trait::async_trait;

use crate::ports::form::{FormPort, FormRequest, FormResponse};

/// Answers each request with the next scripted response; once they run
/// out, every form is cancelled.
#[derive(Debug, Default)]
pub struct ScriptedForm {
    responses: Mutex<VecDeque<FormResponse>>,
    requests: Mutex<Vec<FormRequest>>,
}

impl ScriptedForm {
    /// A port answering `responses` in order.
    #[must_use]
    pub fn new(responses: impl IntoIterator<Item = FormResponse>) -> Self {
        Self {
            responses: Mutex::new(responses.into_iter().collect()),
            requests: Mutex::new(Vec::new()),
        }
    }

    /// The requests received so far.
    pub fn requests(&self) -> Vec<FormRequest> {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }
}

#[async_trait]
impl FormPort for ScriptedForm {
    async fn ask(&self, req: FormRequest) -> FormResponse {
        self.requests
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(req);
        self.responses
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .pop_front()
            .unwrap_or_else(|| FormResponse::cancelled("no scripted answer left"))
    }
}
