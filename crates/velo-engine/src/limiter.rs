//! A token bucket that keeps downloads under a speed limit.

use std::sync::{Arc, Mutex, PoisonError};
use std::time::Duration;

use tokio::time::Instant;

/// A speed limit shared by every connection it applies to. Cloning gives another handle to the
/// same limit.
///
/// Connections call [`Limiter::acquire`] for each chunk they receive and wait as long as it
/// says; delaying the reads slows the server down through TCP's own flow control. The bucket
/// holds up to one second of data, so a pause is followed by a short burst, and borrowing ahead
/// lets a chunk larger than the bucket through without stalling forever.
#[derive(Debug, Clone, Default)]
pub struct Limiter(Arc<Mutex<Bucket>>);

#[derive(Debug, Default)]
struct Bucket {
    /// Bytes per second; no limit if `None`.
    rate: Option<u64>,
    /// Bytes that may pass now; negative when borrowed ahead.
    tokens: f64,
    /// When `tokens` was last topped up; `None` until the first chunk.
    last: Option<Instant>,
}

impl Limiter {
    /// A limiter for `rate` bytes per second, or one that never waits for `None`.
    pub fn new(rate: Option<u64>) -> Self {
        let limiter = Self::default();
        limiter.set_rate(rate);
        limiter
    }

    /// Changes the limit; connections already waiting finish their current wait.
    pub fn set_rate(&self, rate: Option<u64>) {
        let mut bucket = self.lock();
        bucket.rate = rate.filter(|&r| r > 0);
        bucket.last = None;
    }

    /// The limit in bytes per second, if any.
    pub fn rate(&self) -> Option<u64> {
        self.lock().rate
    }

    /// Takes `bytes` from the bucket and waits until the limit allows them.
    pub async fn acquire(&self, bytes: u64) {
        let wait = {
            let mut bucket = self.lock();
            let Some(rate) = bucket.rate else { return };
            let rate = rate as f64;
            let now = Instant::now();
            bucket.tokens = match bucket.last {
                Some(last) => {
                    (bucket.tokens + now.duration_since(last).as_secs_f64() * rate).min(rate)
                }
                None => rate,
            };
            bucket.last = Some(now);
            bucket.tokens -= bytes as f64;
            if bucket.tokens >= 0.0 {
                return;
            }
            Duration::from_secs_f64(-bucket.tokens / rate)
        };
        tokio::time::sleep(wait).await;
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Bucket> {
        self.0.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// How long `bytes` take through `limiter`, in seconds of (paused) tokio time.
    async fn time_for(limiter: &Limiter, chunks: &[u64]) -> f64 {
        let start = Instant::now();
        for &bytes in chunks {
            limiter.acquire(bytes).await;
        }
        start.elapsed().as_secs_f64()
    }

    #[tokio::test(start_paused = true)]
    async fn one_second_of_data_passes_at_once_then_the_rate_holds() {
        let limiter = Limiter::new(Some(1000));
        assert_eq!(time_for(&limiter, &[1000]).await, 0.0);
        let elapsed = time_for(&limiter, &[500, 500, 500, 500]).await;
        assert!((elapsed - 2.0).abs() < 0.01, "{elapsed}");
    }

    #[tokio::test(start_paused = true)]
    async fn a_chunk_bigger_than_the_bucket_waits_for_its_share() {
        let limiter = Limiter::new(Some(1000));
        let elapsed = time_for(&limiter, &[3000]).await;
        assert!((elapsed - 2.0).abs() < 0.01, "{elapsed}");
    }

    #[tokio::test(start_paused = true)]
    async fn no_limit_never_waits_and_the_limit_can_change() {
        let limiter = Limiter::new(None);
        assert_eq!(time_for(&limiter, &[u64::MAX / 2]).await, 0.0);
        limiter.set_rate(Some(100));
        assert_eq!(limiter.rate(), Some(100));
        let elapsed = time_for(&limiter, &[100, 100]).await;
        assert!((elapsed - 1.0).abs() < 0.01, "{elapsed}");
        limiter.set_rate(Some(0));
        assert_eq!(limiter.rate(), None);
    }
}
