//! Startup timing, printed only when `LIGHTFORGE_TRACE` is set.
//!
//! Stamps each startup moment with milliseconds since process start. Off by
//! default: one atomic read per call.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();
static ON: OnceLock<bool> = OnceLock::new();
static FRAMES: OnceLock<bool> = OnceLock::new();
static PAD: OnceLock<bool> = OnceLock::new();
/// Microseconds since [`begin`] at the end of the previous view build.
static LAST_FRAME: AtomicU64 = AtomicU64::new(0);

/// Start the clock. Called as early as `main` can manage.
pub fn begin() {
    let value = std::env::var("LIGHTFORGE_TRACE").unwrap_or_default();
    let _ = START.set(Instant::now());
    let _ = ON.set(std::env::var_os("LIGHTFORGE_TRACE").is_some());
    // Per-frame timing is noisy enough to want asking for by name.
    let _ = FRAMES.set(value.split(',').any(|v| v.trim() == "frames"));
    let _ = PAD.set(value.split(',').any(|v| v.trim() == "pad"));
    mark("process start");
}

#[allow(clippy::cast_possible_truncation, reason = "microseconds since start")]
fn micros() -> u64 {
    START.get().map(|s| s.elapsed().as_micros() as u64).unwrap_or(0)
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

/// Stamp a moment that can only be interesting the first time it happens,
/// the first frame drawn, the first frame drawn with branding.
pub fn once(slot: &'static std::sync::atomic::AtomicBool, what: impl std::fmt::Display) {
    use std::sync::atomic::Ordering;
    if !slot.swap(true, Ordering::Relaxed) {
        mark(what);
    }
}

/// Stamp one controller event under `LIGHTFORGE_TRACE=pad`, for a press that
/// moves the ring twice.
pub fn pad(what: impl std::fmt::Display) {
    if PAD.get().copied().unwrap_or(false) {
        mark(what);
    }
}

/// Whether per-event tracing was asked for.
pub fn frames() -> bool {
    FRAMES.get().copied().unwrap_or(false)
}

/// Stamp one rebuild of the view tree.
///
/// **Not** one frame: a resize redraws without calling `view`.
pub fn view_built(build: std::time::Duration) {
    if !FRAMES.get().copied().unwrap_or(false) {
        return;
    }
    let now = micros();
    let previous = LAST_FRAME.swap(now, Ordering::Relaxed);
    let gap = now.saturating_sub(previous) as f64 / 1000.0;
    let build = build.as_secs_f64() * 1000.0;
    eprintln!("[view]  {:>9.1} ms  build {build:>6.2} ms  gap {gap:>7.1} ms", now as f64 / 1000.0);
}
