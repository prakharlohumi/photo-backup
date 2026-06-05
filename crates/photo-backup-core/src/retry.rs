use rand::Rng;
use std::time::Duration;

#[derive(Debug, Clone, Copy)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

impl Default for RetryPolicy {
    fn default() -> Self {
        Self {
            max_attempts: 6,
            base_delay: Duration::from_secs(2),
            max_delay: Duration::from_secs(120),
        }
    }
}

impl RetryPolicy {
    pub fn should_retry(&self, attempt: u32) -> bool {
        attempt < self.max_attempts
    }

    pub fn delay_for_attempt(&self, attempt: u32) -> Duration {
        let exponent = attempt.min(20);
        let factor = 1u128.checked_shl(exponent).unwrap_or(u128::MAX);
        let millis = self
            .base_delay
            .as_millis()
            .saturating_mul(factor)
            .min(self.max_delay.as_millis()) as u64;
        let delay = Duration::from_millis(millis);

        let jitter = rand::thread_rng().gen_range(0.75_f64..1.25_f64);
        let millis = ((delay.as_millis() as f64) * jitter).round() as u64;
        Duration::from_millis(millis.max(250))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backoff_grows_and_caps() {
        let policy = RetryPolicy {
            max_attempts: 5,
            base_delay: Duration::from_secs(1),
            max_delay: Duration::from_secs(8),
        };

        assert!(policy.delay_for_attempt(0) >= Duration::from_millis(750));
        assert!(policy.delay_for_attempt(10) <= Duration::from_secs(10));
    }
}
