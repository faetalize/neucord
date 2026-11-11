//! How long to wait before trying again after repeated failures.

use std::time::Duration;

/// Each failure waits longer than the one before, up to a ceiling, until a
/// success starts over. The client's `Backoff` (158390) adds up to twice the
/// current wait at random, so clients that failed together don't come back
/// together.
#[derive(Clone, Debug)]
pub struct Backoff {
    min: Duration,
    max: Duration,
    current: Duration,
}

impl Backoff {
    pub fn new(min: Duration, max: Duration) -> Self {
        Backoff { min, max, current: min }
    }

    /// The wait after a failure. `random` is in `0.0..1.0`.
    pub fn fail(&mut self, random: f64) -> Duration {
        self.current = (self.current + self.current.mul_f64(2.0 * random)).min(self.max);
        self.current
    }

    pub fn succeed(&mut self) {
        self.current = self.min;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn backoff() -> Backoff {
        Backoff::new(Duration::from_millis(1500), Duration::from_secs(30))
    }

    #[test]
    fn waits_grow_by_a_random_share_of_double_and_stop_at_the_ceiling() {
        // The client's `Backoff` (158390): current += 2 * current * random, capped.
        let mut b = backoff();
        assert_eq!(b.fail(0.0), Duration::from_millis(1500), "no jitter: the first wait is the minimum");
        let mut b = backoff();
        assert_eq!(b.fail(0.5), Duration::from_millis(3000));
        assert_eq!(b.fail(0.5), Duration::from_millis(6000));
        let mut b = backoff();
        assert_eq!(b.fail(0.999), Duration::from_millis(4497));
        for _ in 0..10 {
            b.fail(0.9);
        }
        assert_eq!(b.fail(0.9), Duration::from_secs(30));
    }

    #[test]
    fn success_starts_over() {
        let mut b = backoff();
        b.fail(0.5);
        b.fail(0.5);
        b.succeed();
        assert_eq!(b.fail(0.5), Duration::from_millis(3000));
    }
}
