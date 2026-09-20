//! Startup timing, printed only when `LIGHTFORGE_TRACE` is set.
//!
//! How fast the dashboard *feels* is a sequence of moments — the window
//! appearing, the cache answering, the network answering, the first frame that
//! actually has art in it — and which of those is slow is not something a user
//! can report from the outside. This stamps each one with milliseconds since
//! process start.
//!
//! Off by default and free when off: one atomic read per call.

use std::sync::OnceLock;
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();
static ON: OnceLock<bool> = OnceLock::new();

/// Start the clock. Called as early as `main` can manage.
pub fn begin() {
    let _ = START.set(Instant::now());
    let _ = ON.set(std::env::var_os("LIGHTFORGE_TRACE").is_some());
    mark("process start");
}

/// Stamp one moment.
pub fn mark(what: impl std::fmt::Display) {
    if !ON.get().copied().unwrap_or(false) {
        return;
    }
    let ms = START
        .get()
        .map(|s| s.elapsed().as_secs_f64() * 1000.0)
        .unwrap_or(0.0);
    eprintln!("[trace] {ms:>9.1} ms  {what}");
}

/// Stamp a moment that can only be interesting the first time it happens —
/// the first frame drawn, the first frame drawn with branding.
pub fn once(slot: &'static std::sync::atomic::AtomicBool, what: impl std::fmt::Display) {
    use std::sync::atomic::Ordering;
    if !slot.swap(true, Ordering::Relaxed) {
        mark(what);
    }
}
