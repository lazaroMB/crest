//! What the model adapters share. Adapters may not depend on each other,
//! so the pieces that Anthropic and the OpenAI-style APIs (and whatever
//! speaks HTTP to a model next) have in common live here:
//!
//! - [`sse`]: the loop that runs a [`Decoder`] over the server-sent
//!   events of a streaming answer;
//! - [`http`]: the reqwest client, the network error, `retry-after` and
//!   the parts of a refused response;
//! - [`errors`]: the typed [`LlmError`](application_core::ports::llm::LlmError)
//!   of an HTTP status, for answers whose body says nothing more precise;
//! - [`TokioClock`]: the real clock of the retries.
//!
//! Only `application-core`, the HTTP stack and tokio; no other adapter.

pub mod clock;
pub mod errors;
pub mod http;
pub mod sse;

pub use clock::TokioClock;
pub use sse::{Decoder, llm_events};
