//! Time for the robot link. Everything in the link reads time through this trait, so tests run on
//! a clock they move by hand and never wait.

use std::time::{Duration, Instant};

pub trait Clock: Send + Sync {
    /// Milliseconds on a clock that only moves forward.
    fn now_ms(&self) -> u64;
    /// Waits, unless time is controlled by a test.
    fn pause(&self, duration: Duration);
}

pub struct SystemClock {
    start: Instant,
}

impl SystemClock {
    pub fn new() -> Self {
        Self {
            start: Instant::now(),
        }
    }
}

impl Clock for SystemClock {
    fn now_ms(&self) -> u64 {
        u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX)
    }

    fn pause(&self, duration: Duration) {
        std::thread::sleep(duration);
    }
}

#[cfg(test)]
pub use manual::ManualClock;

#[cfg(test)]
mod manual {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Duration;

    use super::Clock;

    /// A clock that moves only when a test says so. `pause` does nothing.
    pub struct ManualClock {
        now: AtomicU64,
    }

    impl ManualClock {
        pub fn new() -> Self {
            // Not zero, so "never happened" and "just now" differ.
            Self {
                now: AtomicU64::new(1_000),
            }
        }

        pub fn advance(&self, ms: u64) {
            self.now.fetch_add(ms, Ordering::SeqCst);
        }
    }

    impl Clock for ManualClock {
        fn now_ms(&self) -> u64 {
            self.now.load(Ordering::SeqCst)
        }

        fn pause(&self, _duration: Duration) {}
    }
}
