//! The [`Clock`] of the retries in a real process.

use std::collections::hash_map::RandomState;
use std::hash::{BuildHasher, Hasher};
use std::time::Duration;

use application_core::llm_retry::Clock;
use futures::future::BoxFuture;

/// [`Clock`] backed by `tokio::time::sleep` and std's random hasher seed.
#[derive(Debug, Clone, Copy, Default)]
pub struct TokioClock;

impl Clock for TokioClock {
    fn sleep(&self, duration: Duration) -> BoxFuture<'static, ()> {
        Box::pin(tokio::time::sleep(duration))
    }

    fn jitter(&self) -> f64 {
        // Each `RandomState` is seeded randomly; good enough for jitter.
        let bits = RandomState::new().build_hasher().finish();
        (bits >> 11) as f64 / (1u64 << 53) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn jitter_is_a_fraction() {
        for _ in 0..100 {
            assert!((0.0..1.0).contains(&TokioClock.jitter()));
        }
    }

    #[tokio::test]
    async fn sleep_waits_on_tokio_time() {
        let start = tokio::time::Instant::now();
        TokioClock.sleep(Duration::from_millis(20)).await;
        assert!(start.elapsed() >= Duration::from_millis(20));
    }
}
