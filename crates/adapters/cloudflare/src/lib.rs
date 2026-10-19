//! [`DecisionPort`] over Cloudflare Workers AI's decision models (Clef):
//! `POST {base}/accounts/{account}/ai/run/@cf/cloudflare/{model}` with a
//! bearer token, the body `{model, state, questions}` and the answers in
//! the `result` of Cloudflare's envelope.
//!
//! Question types map one to one: yes/no is `noul` (`criteria` says what
//! true and false mean), a choice is `choice` (`criteria` maps option ids
//! to their meaning) and a score is `score` (`criteria` lists the levels,
//! lowest first). One request, no retries: a decision that is late is not
//! worth waiting for, the caller falls back.

use std::collections::BTreeMap;
use std::time::Duration;

use application::decide::DecisionOptions;
use application::ports::decision::{DecisionError, DecisionPort};
use async_trait::async_trait;
use domain::decision::{Answer, DecisionRequest, DecisionResponse, Question};
use serde_json::{Map, Value, json};

/// Cloudflare's API.
pub const DEFAULT_BASE_URL: &str = "https://api.cloudflare.com/client/v4";
/// Environment variable and keyring entry (service `crest`) of the token.
pub const TOKEN_NAME: &str = "CLOUDFLARE_API_TOKEN";
/// Environment variable and keyring entry (service `crest`) of the
/// account id.
pub const ACCOUNT_NAME: &str = "CLOUDFLARE_ACCOUNT_ID";
/// Keyring service of the credentials.
const KEYRING_SERVICE: &str = "crest";
/// Longest error text kept from a response.
const MAX_ERROR_CHARS: usize = 300;

/// `name` from the environment, else from the keyring (service `crest`).
/// The keyring call may block (D-Bus, Keychain): call this through
/// `spawn_blocking`.
#[must_use]
pub fn secret_from_env_or_keyring(name: &str) -> Option<String> {
    if let Ok(value) = std::env::var(name)
        && !value.trim().is_empty()
    {
        return Some(value.trim().to_owned());
    }
    keyring::Entry::new(KEYRING_SERVICE, name)
        .inspect_err(|err| tracing::debug!(error = %err, "keyring unavailable"))
        .ok()?
        .get_password()
        .inspect_err(|err| tracing::debug!(error = %err, name, "not in the keyring"))
        .ok()
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
}

/// A Clef client.
pub struct ClefClient {
    http: reqwest::Client,
    url: String,
    token: String,
    model: String,
}

impl std::fmt::Debug for ClefClient {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ClefClient")
            .field("url", &self.url)
            .field("token", &"<redacted>")
            .field("model", &self.model)
            .finish()
    }
}

impl ClefClient {
    /// A client for `account` with `token`, using the model, base URL and
    /// timeout of `options`.
    ///
    /// # Errors
    ///
    /// [`DecisionError::Network`] when the HTTP client cannot be built.
    pub fn new(
        options: &DecisionOptions,
        account: &str,
        token: impl Into<String>,
    ) -> Result<Self, DecisionError> {
        let http = reqwest::Client::builder()
            .timeout(Duration::from_millis(options.timeout_ms))
            .build()
            .map_err(|err| DecisionError::Network(err.to_string()))?;
        let base = options.base_url.as_deref().unwrap_or(DEFAULT_BASE_URL);
        Ok(Self {
            http,
            url: format!(
                "{}/accounts/{account}/ai/run/@cf/cloudflare/{}",
                base.trim_end_matches('/'),
                options.model
            ),
            token: token.into(),
            model: options.model.clone(),
        })
    }
}

#[async_trait]
impl DecisionPort for ClefClient {
    fn model(&self) -> &str {
        &self.model
    }

    async fn decide(&self, request: &DecisionRequest) -> Result<DecisionResponse, DecisionError> {
        request.validate()?;
        let response = self
            .http
            .post(&self.url)
            .bearer_auth(&self.token)
            .json(&body(&self.model, request))
            .send()
            .await
            .map_err(|err| DecisionError::Network(err.to_string()))?;
        let status = response.status();
        let text = response
            .text()
            .await
            .map_err(|err| DecisionError::Network(err.to_string()))?;
        if !status.is_success() {
            let message = error_message(&text);
            return Err(match status.as_u16() {
                401 | 403 => DecisionError::Auth(message),
                code => DecisionError::Http {
                    status: code,
                    message,
                },
            });
        }
        let value: Value =
            serde_json::from_str(&text).map_err(|err| DecisionError::Decode(err.to_string()))?;
        parse(&value)
    }
}

/// The request body.
fn body(model: &str, request: &DecisionRequest) -> Value {
    let questions: Map<String, Value> = request
        .questions
        .iter()
        .map(|(id, question)| (id.clone(), question_json(question)))
        .collect();
    json!({ "model": model, "state": request.state, "questions": questions })
}

fn question_json(question: &Question) -> Value {
    match question {
        Question::YesNo {
            instructions,
            yes,
            no,
        } => {
            let mut q = json!({ "type": "noul", "instructions": instructions });
            let mut criteria = Map::new();
            if let Some(yes) = yes {
                criteria.insert("true".into(), json!(yes));
            }
            if let Some(no) = no {
                criteria.insert("false".into(), json!(no));
            }
            if !criteria.is_empty() {
                q["criteria"] = Value::Object(criteria);
            }
            q
        }
        Question::Choice {
            instructions,
            options,
        } => {
            let criteria: Map<String, Value> = options
                .iter()
                .map(|(id, meaning)| (id.clone(), json!(meaning)))
                .collect();
            json!({ "type": "choice", "instructions": instructions, "criteria": criteria })
        }
        Question::Score {
            instructions,
            levels,
        } => json!({ "type": "score", "instructions": instructions, "criteria": levels }),
    }
}

/// The answers of a response: inside Cloudflare's `result`, or bare (a
/// gateway that unwraps it).
fn parse(value: &Value) -> Result<DecisionResponse, DecisionError> {
    let decode = |what: &str| DecisionError::Decode(what.to_owned());
    if value.get("success") == Some(&Value::Bool(false)) {
        return Err(DecisionError::Http {
            status: 200,
            message: error_message(&value.to_string()),
        });
    }
    let result = value.get("result").unwrap_or(value);
    let answers = result
        .get("answers")
        .and_then(Value::as_object)
        .ok_or_else(|| decode("no answers"))?;
    let usage = result.get("usage");
    let tokens = |field: &str| {
        usage
            .and_then(|u| u.get(field))
            .and_then(Value::as_u64)
            .unwrap_or(0)
    };
    Ok(DecisionResponse {
        model: result
            .get("model")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_owned(),
        answers: answers
            .iter()
            .map(|(id, answer)| Ok((id.clone(), answer_of(id, answer)?)))
            .collect::<Result<_, DecisionError>>()?,
        input_tokens: tokens("input_tokens"),
        output_tokens: tokens("output_tokens"),
    })
}

fn answer_of(id: &str, answer: &Value) -> Result<Answer, DecisionError> {
    let bad = |what: &str| DecisionError::Decode(format!("answer {id:?}: {what}"));
    let number = |field: &str| {
        answer
            .get(field)
            .and_then(Value::as_f64)
            .map(|n| n as f32)
            .ok_or_else(|| bad(&format!("no {field}")))
    };
    let probabilities = || -> BTreeMap<String, f32> {
        answer
            .get("probabilities")
            .and_then(Value::as_object)
            .map(|p| {
                p.iter()
                    .filter_map(|(k, v)| Some((k.clone(), v.as_f64()? as f32)))
                    .collect()
            })
            .unwrap_or_default()
    };
    match answer.get("type").and_then(Value::as_str) {
        Some("noul") => Ok(Answer::YesNo {
            p_yes: number("noul")?,
        }),
        Some("choice") => Ok(Answer::Choice {
            choice: answer
                .get("choice")
                .and_then(Value::as_str)
                .ok_or_else(|| bad("no choice"))?
                .to_owned(),
            probabilities: probabilities(),
            confidence: number("confidence")?,
        }),
        Some("score") => Ok(Answer::Score {
            score: number("score")?,
            probabilities: probabilities()
                .into_iter()
                .filter_map(|(level, p)| Some((level.parse().ok()?, p)))
                .collect(),
            confidence: number("confidence")?,
        }),
        other => Err(bad(&format!("unknown type {other:?}"))),
    }
}

/// The first error message of Cloudflare's envelope, else the text.
fn error_message(text: &str) -> String {
    let parsed: Option<Value> = serde_json::from_str(text).ok();
    let message = parsed
        .as_ref()
        .and_then(|v| {
            v.get("errors")?
                .get(0)?
                .get("message")?
                .as_str()
                .map(str::to_owned)
        })
        .unwrap_or_else(|| text.trim().to_owned());
    message.chars().take(MAX_ERROR_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use wiremock::matchers::{body_partial_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::*;

    const TOKEN: &str = "cf-fake-token-never-real";

    fn client(server: &MockServer) -> ClefClient {
        let options = DecisionOptions {
            base_url: Some(format!("{}/client/v4/", server.uri())),
            timeout_ms: 500,
            ..DecisionOptions::default()
        };
        ClefClient::new(&options, "acct", TOKEN).unwrap()
    }

    fn request() -> DecisionRequest {
        DecisionRequest::new("my site is down")
            .ask(
                "urgent",
                Question::yes_no("Is this support request urgent?"),
            )
            .ask(
                "team",
                Question::choice(
                    "Which team?",
                    [("billing", "Payments"), ("technical", "Outages")],
                ),
            )
            .ask(
                "severity",
                Question::score("How severe?", ["No impact", "Minor", "Major"]),
            )
    }

    fn answers() -> Value {
        json!({
            "model": "clef-flash",
            "answers": {
                "urgent": { "type": "noul", "noul": 0.93 },
                "team": { "type": "choice", "choice": "technical",
                          "probabilities": { "billing": 0.04, "technical": 0.96 },
                          "confidence": 0.96 },
                "severity": { "type": "score", "score": 1.8,
                              "legend": { "0": "No impact", "1": "Minor", "2": "Major" },
                              "probabilities": { "0": 0.05, "1": 0.1, "2": 0.85 },
                              "confidence": 0.85 }
            },
            "usage": { "input_tokens": 120, "output_tokens": 0 }
        })
    }

    #[tokio::test]
    async fn questions_are_sent_typed_and_answers_read_back() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/client/v4/accounts/acct/ai/run/@cf/cloudflare/clef-flash",
            ))
            .and(header("authorization", format!("Bearer {TOKEN}").as_str()))
            .and(body_partial_json(json!({
                "model": "clef-flash",
                "state": "my site is down",
                "questions": {
                    "urgent": { "type": "noul", "instructions": "Is this support request urgent?" },
                    "team": { "type": "choice",
                              "criteria": { "billing": "Payments", "technical": "Outages" } },
                    "severity": { "type": "score",
                                  "criteria": ["No impact", "Minor", "Major"] }
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "result": answers(), "success": true, "errors": [], "messages": []
            })))
            .expect(1)
            .mount(&server)
            .await;
        let response = client(&server).decide(&request()).await.unwrap();
        assert_eq!(response.yes("urgent"), Some(0.93));
        assert_eq!(response.choice("team"), Some(("technical", 0.96)));
        assert!((response.score("severity").unwrap() - 1.8).abs() < 1e-6);
        assert_eq!(response.input_tokens, 120);
        assert_eq!(response.model, "clef-flash");
        let Some(Answer::Score { probabilities, .. }) = response.answers.get("severity") else {
            panic!("not a score");
        };
        assert!((probabilities[&2] - 0.85).abs() < 1e-6);
    }

    #[tokio::test]
    async fn a_bare_result_is_read_too() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_json(answers()))
            .mount(&server)
            .await;
        let response = client(&server).decide(&request()).await.unwrap();
        assert_eq!(response.yes("urgent"), Some(0.93));
    }

    #[tokio::test]
    async fn errors_are_typed_and_never_show_the_token() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path(
                "/client/v4/accounts/acct/ai/run/@cf/cloudflare/clef-flash",
            ))
            .respond_with(ResponseTemplate::new(401).set_body_json(json!({
                "success": false, "errors": [{ "code": 10000, "message": "Authentication error" }]
            })))
            .mount(&server)
            .await;
        let err = client(&server).decide(&request()).await.unwrap_err();
        assert_eq!(err, DecisionError::Auth("Authentication error".into()));
        assert!(!format!("{err} {:?}", client(&server)).contains(TOKEN));

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&server)
            .await;
        assert_eq!(
            client(&server).decide(&request()).await.unwrap_err(),
            DecisionError::Http {
                status: 500,
                message: "boom".into()
            }
        );

        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
            .mount(&server)
            .await;
        assert!(matches!(
            client(&server).decide(&request()).await,
            Err(DecisionError::Decode(_))
        ));
    }

    #[tokio::test]
    async fn a_late_answer_is_a_network_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_json(answers())
                    .set_delay(Duration::from_secs(2)),
            )
            .mount(&server)
            .await;
        assert!(matches!(
            client(&server).decide(&request()).await,
            Err(DecisionError::Network(_))
        ));
    }

    #[tokio::test]
    async fn invalid_requests_are_not_sent() {
        let server = MockServer::start().await;
        let err = client(&server)
            .decide(&DecisionRequest::new("x"))
            .await
            .unwrap_err();
        assert!(matches!(err, DecisionError::Invalid(_)));
        assert!(server.received_requests().await.unwrap().is_empty());
    }
}
