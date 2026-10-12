//! Retry backoff: exponential, capped, jittered.
//!
//! Loco.rs shipped a 1.0 job queue whose own CLI documentation says *"There is
//! no automatic retry or backoff in any queue driver, so without `retry` a
//! failed job is terminal."* That is the shape of failure this module exists to
//! make impossible: [`Backoff`] is a `const` on the [`Job`] trait with a
//! working default, so a job that declares nothing still retries five times
//! with growing delays.
//!
//! [`Job`]: crate::Job

use std::time::Duration;

/// How long to wait before attempt `n + 1`, given that attempt `n` failed.
///
/// `delay(n) = min(base * factor^(n - 1), max)`, then spread by `jitter`.
///
/// Jitter is the half of this that gets skipped and then causes an incident.
/// Without it, a hundred jobs failing against the same downed service retry in
/// the same millisecond, forever, and the retry storm is what keeps the service
/// down. `jitter` is a fraction: `0.25` spreads each delay uniformly across
/// `[0.75 d, 1.25 d]`.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Backoff {
    base_ms: u64,
    factor: u32,
    max_ms: u64,
    jitter: f64,
}

impl Backoff {
    /// Ten seconds, doubling, capped at an hour, spread by a quarter.
    ///
    /// Five attempts on this schedule span roughly two and a half minutes,
    /// which outlasts a deploy and a brief network partition and does not
    /// outlast anybody's patience.
    pub const DEFAULT: Backoff = Backoff {
        base_ms: 10_000,
        factor: 2,
        max_ms: 3_600_000,
        jitter: 0.25,
    };

    /// A backoff of `base`, doubling, capped at `max`, with a quarter of
    /// jitter.
    #[must_use]
    pub const fn new(base: Duration, max: Duration) -> Self {
        Self {
            base_ms: base.as_millis() as u64,
            factor: 2,
            max_ms: max.as_millis() as u64,
            jitter: 0.25,
        }
    }

    /// The same delay before every attempt. Jitter still applies.
    #[must_use]
    pub const fn fixed(delay: Duration) -> Self {
        Self {
            base_ms: delay.as_millis() as u64,
            factor: 1,
            max_ms: delay.as_millis() as u64,
            jitter: 0.25,
        }
    }

    /// Replaces the multiplier. `1` makes the backoff constant.
    #[must_use]
    pub const fn factor(mut self, factor: u32) -> Self {
        self.factor = factor;
        self
    }

    /// Replaces the ceiling.
    #[must_use]
    pub const fn max(mut self, max: Duration) -> Self {
        self.max_ms = max.as_millis() as u64;
        self
    }

    /// Replaces the jitter fraction. `0.0` makes the delay exact, which is what
    /// a test wants and what production does not.
    #[must_use]
    pub const fn jitter(mut self, fraction: f64) -> Self {
        self.jitter = fraction;
        self
    }

    /// The un-jittered delay before attempt `attempt + 1`.
    ///
    /// `attempt` is the number of attempts already made, so the first retry
    /// asks with `1` and gets `base`.
    #[must_use]
    pub fn base_delay(&self, attempt: u32) -> Duration {
        let steps = attempt.saturating_sub(1);
        let mut delay = self.base_ms;
        for _ in 0..steps {
            delay = delay.saturating_mul(u64::from(self.factor));
            if delay >= self.max_ms {
                return Duration::from_millis(self.max_ms);
            }
        }
        Duration::from_millis(delay.min(self.max_ms))
    }

    /// The delay before attempt `attempt + 1`, jittered.
    #[must_use]
    pub fn delay(&self, attempt: u32, rng: &mut Prng) -> Duration {
        let base = self.base_delay(attempt).as_millis() as f64;
        if self.jitter <= 0.0 {
            return Duration::from_millis(base as u64);
        }
        // Uniform in [1 - j, 1 + j].
        let spread = 1.0 - self.jitter + 2.0 * self.jitter * rng.next_f64();
        Duration::from_millis((base * spread).max(0.0) as u64)
    }
}

impl Default for Backoff {
    fn default() -> Self {
        Self::DEFAULT
    }
}

/// A SplitMix64 generator.
///
/// The jitter needs a random number and nothing else does, so this crate has a
/// twelve-line generator instead of a dependency on `rand`. It is also seedable
/// from a test, which is why the jitter bounds below can be asserted rather
/// than hoped for.
#[derive(Clone, Debug)]
pub struct Prng(u64);

impl Prng {
    /// A generator seeded from `seed`.
    #[must_use]
    pub const fn seeded(seed: u64) -> Self {
        Self(seed)
    }

    /// A generator seeded from the system clock and this object's address.
    #[must_use]
    pub fn from_entropy() -> Self {
        use std::time::{SystemTime, UNIX_EPOCH};
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0x9e37_79b9_7f4a_7c15);
        let spice = &nanos as *const u64 as u64;
        Self(nanos ^ spice.rotate_left(17))
    }

    /// The next 64 bits.
    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// The next value in `[0, 1)`.
    pub fn next_f64(&mut self) -> f64 {
        // 53 bits is the mantissa, so this is every representable value in the
        // range and no bias.
        (self.next_u64() >> 11) as f64 / (1u64 << 53) as f64
    }
}

impl Default for Prng {
    fn default() -> Self {
        Self::from_entropy()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_delay_grows_and_then_stops_growing() {
        let b = Backoff::new(Duration::from_secs(10), Duration::from_secs(80));
        assert_eq!(b.base_delay(1), Duration::from_secs(10));
        assert_eq!(b.base_delay(2), Duration::from_secs(20));
        assert_eq!(b.base_delay(3), Duration::from_secs(40));
        assert_eq!(b.base_delay(4), Duration::from_secs(80));
        assert_eq!(b.base_delay(5), Duration::from_secs(80));
        assert_eq!(b.base_delay(40), Duration::from_secs(80));
    }

    #[test]
    fn a_fixed_backoff_does_not_grow() {
        let b = Backoff::fixed(Duration::from_secs(5));
        assert_eq!(b.base_delay(1), Duration::from_secs(5));
        assert_eq!(b.base_delay(9), Duration::from_secs(5));
    }

    #[test]
    fn jitter_stays_inside_its_fraction() {
        let b = Backoff::new(Duration::from_secs(100), Duration::from_secs(1_000)).jitter(0.25);
        let mut rng = Prng::seeded(42);
        let mut saw_below = false;
        let mut saw_above = false;
        for _ in 0..1_000 {
            let d = b.delay(1, &mut rng).as_millis() as i64;
            assert!((75_000..=125_000).contains(&d), "{d} outside [75s, 125s]");
            saw_below |= d < 100_000;
            saw_above |= d > 100_000;
        }
        assert!(
            saw_below && saw_above,
            "the jitter spread in one direction only"
        );
    }

    #[test]
    fn zero_jitter_is_exact() {
        let b = Backoff::DEFAULT.jitter(0.0);
        let mut rng = Prng::seeded(1);
        assert_eq!(b.delay(1, &mut rng), b.base_delay(1));
        assert_eq!(b.delay(3, &mut rng), b.base_delay(3));
    }

    #[test]
    fn the_generator_does_not_repeat_itself_immediately() {
        let mut rng = Prng::seeded(0);
        let a: Vec<u64> = (0..64).map(|_| rng.next_u64()).collect();
        let unique: std::collections::HashSet<_> = a.iter().collect();
        assert_eq!(unique.len(), a.len());
        let mut rng = Prng::seeded(0);
        for _ in 0..512 {
            let v = rng.next_f64();
            assert!((0.0..1.0).contains(&v));
        }
    }
}
