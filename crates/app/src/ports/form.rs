//! Asking the user to fill a form (`ask_user`).
//!
//! The tool adds a `Form` block to the timeline, then awaits
//! [`FormPort::ask`] with the same block id, so the UI answers in that
//! block; the headless mode asks each field on stdin. The answer comes
//! back as values by field name, as the user's words when they answered
//! in the chat instead of the form, or as a cancellation.

use std::collections::BTreeMap;
use std::fmt;

use async_trait::async_trait;
use domain::block::BlockId;
use domain::conversation::ToolUseId;
use domain::form::FormSpec;
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Identifies one [`FormRequest`]; a per-tool counter is enough.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FormRequestId(pub u64);

impl fmt::Display for FormRequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(&self.0, f)
    }
}

/// A form to show and wait on.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FormRequest {
    /// Id of this request.
    pub id: FormRequestId,
    /// The `Form` block that shows it.
    pub block_id: BlockId,
    /// The `ask_user` call that waits.
    pub tool_use_id: ToolUseId,
    /// The fields, in the pending state.
    pub spec: FormSpec,
    /// Why the previous answer was rejected, when the form is asked again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// The user's answer to a [`FormRequest`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum FormResponse {
    /// The values by field name; fields left empty are absent.
    Submitted {
        /// The values.
        answer: BTreeMap<String, Value>,
    },
    /// The user typed an answer in the chat instead of filling the form.
    Replied {
        /// What they wrote.
        text: String,
    },
    /// The user dismissed the form, or nobody can answer it.
    Cancelled {
        /// Why, when known (e.g. "stdin is not a terminal").
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
    },
}

impl FormResponse {
    /// A cancellation with `reason`.
    #[must_use]
    pub fn cancelled(reason: impl Into<String>) -> Self {
        Self::Cancelled {
            reason: Some(reason.into()),
        }
    }
}

/// Shows forms to the user and waits for the answers.
#[async_trait]
pub trait FormPort: Send + Sync {
    /// Shows `req` and waits for the answer.
    ///
    /// The wait may be arbitrarily long; dropping the future withdraws the
    /// form, and the adapter must stop offering it. A front end that
    /// cannot ask answers [`FormResponse::Cancelled`] with the reason.
    async fn ask(&self, req: FormRequest) -> FormResponse;
}

/// A [`FormPort`] with nobody to ask: every form is cancelled with the
/// reason given.
#[derive(Debug, Clone)]
pub struct NoForms(pub String);

#[async_trait]
impl FormPort for NoForms {
    async fn ask(&self, _req: FormRequest) -> FormResponse {
        FormResponse::cancelled(self.0.clone())
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn response_round_trips_through_json() {
        let submitted = FormResponse::Submitted {
            answer: BTreeMap::from([("n".to_owned(), json!(3))]),
        };
        let json = serde_json::to_value(&submitted).unwrap();
        assert_eq!(json, json!({"outcome": "submitted", "answer": {"n": 3}}));
        assert_eq!(
            serde_json::from_value::<FormResponse>(json).unwrap(),
            submitted
        );
        let cancelled = FormResponse::Cancelled { reason: None };
        let back: FormResponse =
            serde_json::from_str(&serde_json::to_string(&cancelled).unwrap()).unwrap();
        assert_eq!(back, cancelled);
        let replied = FormResponse::Replied {
            text: "mejor en Python".into(),
        };
        let json = serde_json::to_value(&replied).unwrap();
        assert_eq!(json["outcome"], "replied");
        assert_eq!(
            serde_json::from_value::<FormResponse>(json).unwrap(),
            replied
        );
    }

    #[tokio::test]
    async fn no_forms_cancels_with_its_reason() {
        let port: std::sync::Arc<dyn FormPort> = std::sync::Arc::new(NoForms("no UI".into()));
        let spec: FormSpec = serde_json::from_value(json!({"title": "t", "fields": []})).unwrap();
        let req = FormRequest {
            id: FormRequestId(1),
            block_id: BlockId::new(),
            tool_use_id: ToolUseId::new("toolu_1"),
            spec,
            error: None,
        };
        assert_eq!(port.ask(req).await, FormResponse::cancelled("no UI"));
    }
}
