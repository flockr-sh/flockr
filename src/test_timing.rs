//! Test overrides for fixed waits.
//!
//! Integration tests run the real binary, so `cfg(test)` can't shorten its
//! waits. Debug builds read a `FLOCKR_TEST_*_MS` env var instead, so a test can
//! shorten the one wait it exercises. Release builds always use the default.

use std::sync::OnceLock;
use std::time::Duration;

pub(crate) struct TestTiming {
    env_var: &'static str,
    default: Duration,
    value: OnceLock<Duration>,
}

impl TestTiming {
    pub(crate) const fn new(env_var: &'static str, default: Duration) -> Self {
        Self {
            env_var,
            default,
            value: OnceLock::new(),
        }
    }

    /// The wait for this process. Read once, so hot paths pay no env lookup.
    pub(crate) fn get(&self) -> Duration {
        *self
            .value
            .get_or_init(|| override_from_env(self.env_var).unwrap_or(self.default))
    }
}

fn override_from_env(env_var: &str) -> Option<Duration> {
    if !cfg!(debug_assertions) {
        return None;
    }
    parse_millis(&std::env::var(env_var).ok()?)
}

fn parse_millis(value: &str) -> Option<Duration> {
    value.trim().parse().ok().map(Duration::from_millis)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_whole_milliseconds() {
        assert_eq!(parse_millis(" 250 "), Some(Duration::from_millis(250)));
        assert_eq!(parse_millis("0"), Some(Duration::ZERO));
    }

    #[test]
    fn rejects_values_that_are_not_whole_milliseconds() {
        assert_eq!(parse_millis(""), None);
        assert_eq!(parse_millis("1.5"), None);
        assert_eq!(parse_millis("-1"), None);
        assert_eq!(parse_millis("5s"), None);
    }

    #[test]
    fn unset_override_uses_default() {
        let timing = TestTiming::new(
            "FLOCKR_TEST_TIMING_UNIT_TEST_UNSET_MS",
            Duration::from_secs(3),
        );
        assert_eq!(timing.get(), Duration::from_secs(3));
    }
}
