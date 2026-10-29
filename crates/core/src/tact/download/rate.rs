use std::sync::Arc;

/// A shared cap on how fast the loop may pull bytes: a token bucket paid per
/// request, since the plan knows every fetch's size, so the byte stream is
/// untouched. The average converges on the limit; one request is bursty.
#[derive(Debug, Clone)]
pub struct RateLimit {
    inner: Arc<tokio::sync::Mutex<Bucket>>,
    /// Bytes per second.
    rate: u64,
}

#[derive(Debug)]
struct Bucket {
    /// Tokens available now, in bytes.
    tokens: f64,
    /// Most tokens the bucket may hold: one second of the limit.
    capacity: f64,
    /// Tokio's clock, which [`tokio::time::sleep`] obeys and tests can pause.
    last: tokio::time::Instant,
}

impl RateLimit {
    /// A limit of `bytes_per_second`, or `None` for no limit at all.
    pub fn new(bytes_per_second: u64) -> Option<Self> {
        (bytes_per_second > 0).then(|| Self {
            inner: Arc::new(tokio::sync::Mutex::new(Bucket {
                tokens: bytes_per_second as f64,
                capacity: bytes_per_second as f64,
                last: tokio::time::Instant::now(),
            })),
            rate: bytes_per_second,
        })
    }

    /// Wait until `bytes` may be fetched. A request larger than the bucket
    /// goes once it is full and overdraws; the next request repays it.
    pub async fn acquire(&self, bytes: u64) {
        let want = bytes as f64;
        loop {
            let wait = {
                let mut bucket = self.inner.lock().await;
                let now = tokio::time::Instant::now();
                let elapsed = now.duration_since(bucket.last).as_secs_f64();
                bucket.last = now;
                bucket.tokens = (bucket.tokens + elapsed * self.rate as f64).min(bucket.capacity);

                let affordable = want.min(bucket.capacity);
                if bucket.tokens >= affordable {
                    bucket.tokens -= want;
                    return;
                }
                (affordable - bucket.tokens) / self.rate as f64
            };
            tokio::time::sleep(std::time::Duration::from_secs_f64(wait.max(0.001))).await;
        }
    }
}
