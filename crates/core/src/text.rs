//! Text formatting several modules share.

/// Lower-case hex, two digits per byte.
pub fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The last `n` non-empty lines of `text`, in order.
pub(crate) fn tail_lines(text: &str, n: usize) -> String {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

/// `secs` since the epoch, broken down in the machine's time zone.
pub(crate) fn local_time(secs: i64) -> Option<libc::tm> {
    let secs = secs as libc::time_t;
    // SAFETY: `localtime_r` is the reentrant form; it reads `secs` and
    // writes only into `tm`, which outlives the call.
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&secs, &mut tm) }.is_null() {
        return None;
    }
    Some(tm)
}
