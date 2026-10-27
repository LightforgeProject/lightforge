use std::sync::Arc;

/// A shared cap on how fast the loop may pull bytes.
///
/// A token bucket in front of each request rather than a throttle on the byte
/// stream: the plan already knows how large every fetch is, so the cost can be
/// paid before the request goes out, and the bytes themselves never pass
/// through an extra layer. The average converges on the limit; the wire is
/// bursty within one request, which is the trade for leaving the hot path
/// untouched.
///
/// Cloneable and shared by every task in one run. `None` is the ordinary case
/// and costs a single `Option` check per request.
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
    /// Most tokens the bucket may hold, one second of the limit, so a quiet
    /// moment buys a short burst and no more.
    capacity: f64,
    /// Read from tokio's clock, not the system one: it is the clock
    /// [`tokio::time::sleep`] obeys, so the two cannot drift apart, and it
    /// makes the limiter testable without spending real seconds.
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

    /// Wait until `bytes` may be fetched.
    ///
    /// A request larger than the bucket holds is allowed through once the
    /// bucket is full rather than deadlocking against a capacity it can never
    /// reach, it simply overdraws, and the negative balance is repaid before
    /// the next request starts.
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
