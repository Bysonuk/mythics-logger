//! The upload speed limit from Settings: a simple pacer. Each send "costs"
//! its size divided by the rate; the next send waits until the cost is paid.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::Mutex;
use tokio::time::Instant;

pub struct Throttle {
    /// Bytes a second; 0 means no limit.
    rate: AtomicU64,
    next: Mutex<Option<Instant>>,
}

impl Throttle {
    pub fn new(bytes_per_sec: u64) -> Self {
        Self {
            rate: AtomicU64::new(bytes_per_sec),
            next: Mutex::new(None),
        }
    }

    pub fn set_rate(&self, bytes_per_sec: u64) {
        self.rate.store(bytes_per_sec, Ordering::Relaxed);
    }

    pub fn rate(&self) -> u64 {
        self.rate.load(Ordering::Relaxed)
    }

    /// Waits until `bytes` may be sent.
    pub async fn take(&self, bytes: u64) {
        let rate = self.rate();
        if rate == 0 {
            return;
        }
        let start = {
            let mut next = self.next.lock().await;
            let now = Instant::now();
            let start = next.map_or(now, |n| n.max(now));
            *next = Some(start + Duration::from_secs_f64(bytes as f64 / rate as f64));
            start
        };
        tokio::time::sleep_until(start).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test(start_paused = true)]
    async fn paces_sends_to_the_rate() {
        let t = Throttle::new(1_000_000);
        let begin = Instant::now();
        for _ in 0..4 {
            t.take(500_000).await;
        }
        // Four half-megabyte sends at 1 MB/s: the fourth starts at 1.5 s.
        assert_eq!(begin.elapsed().as_millis(), 1500);
    }

    #[tokio::test(start_paused = true)]
    async fn no_limit_never_waits() {
        let t = Throttle::new(0);
        let begin = Instant::now();
        t.take(u64::MAX).await;
        assert_eq!(begin.elapsed().as_millis(), 0);
    }
}
