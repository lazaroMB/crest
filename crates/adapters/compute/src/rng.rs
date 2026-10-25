//! The seeded random stream of every resampling and simulation: ChaCha20
//! (`rand_chacha`), so a seed gives the same draws on every platform and a
//! recompute reproduces the values exactly.

use rand_chacha::ChaCha20Rng;
use rand_chacha::rand_core::{Rng as _, SeedableRng as _};

use crate::math::qnorm;

/// Name recorded in the envelope's `rng`.
pub const RNG_NAME: &str = "chacha20";

/// A seeded stream of draws.
#[derive(Debug, Clone)]
pub struct Rng(ChaCha20Rng);

impl Rng {
    /// The stream of `seed`.
    #[must_use]
    pub fn new(seed: u64) -> Self {
        Self(ChaCha20Rng::seed_from_u64(seed))
    }

    /// A uniform draw in the open interval (0, 1), with 53 random bits.
    pub fn uniform(&mut self) -> f64 {
        loop {
            #[allow(clippy::cast_precision_loss)]
            let u = (self.0.next_u64() >> 11) as f64 / (1u64 << 53) as f64;
            if u > 0.0 {
                return u;
            }
        }
    }

    /// A uniform index in `0..n` (`n > 0`), without modulo bias.
    pub fn below(&mut self, n: usize) -> usize {
        let n = n as u64;
        let zone = u64::MAX - u64::MAX % n;
        loop {
            let x = self.0.next_u64();
            if x < zone {
                // `x % n` is below `n`, which came from a usize.
                #[allow(clippy::cast_possible_truncation)]
                return (x % n) as usize;
            }
        }
    }

    /// A standard normal draw, by inversion of a uniform draw.
    pub fn normal(&mut self) -> f64 {
        qnorm(self.uniform())
    }

    /// Shuffles `items` in place (Fisher-Yates).
    pub fn shuffle<T>(&mut self, items: &mut [T]) {
        for i in (1..items.len()).rev() {
            let j = self.below(i + 1);
            items.swap(i, j);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_seed_gives_the_same_stream() {
        let mut a = Rng::new(7);
        let mut b = Rng::new(7);
        let xs: Vec<f64> = (0..5).map(|_| a.uniform()).collect();
        let ys: Vec<f64> = (0..5).map(|_| b.uniform()).collect();
        assert_eq!(xs, ys);
        assert!(xs.iter().all(|u| *u > 0.0 && *u < 1.0));
        assert_ne!(Rng::new(8).uniform(), xs[0]);
        let mut c = Rng::new(1);
        let mean = (0..20_000).map(|_| c.normal()).sum::<f64>() / 20_000.0;
        assert!(mean.abs() < 0.03, "{mean}");
        let mut counts = [0; 3];
        for _ in 0..3000 {
            counts[c.below(3)] += 1;
        }
        assert!(counts.iter().all(|k| (900..1100).contains(k)), "{counts:?}");
    }
}
