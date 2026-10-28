//! Turning numbers, dates and paths into the words the UI shows.

use crate::*;

/// The caption under a pre-installed game's button, from Battle.net's
/// `Available October 19, 2026`: about the servers (it is login that waits).
pub(crate) fn servers_open(release_date: &str) -> String {
    let Some(date) = release_date.strip_prefix("Available ") else {
        return release_date.to_owned();
    };
    // The year is noise for a date days away.
    let date = match date.rsplit_once(", ") {
        Some((day, year)) if year.len() == 4 && year.bytes().all(|b| b.is_ascii_digit()) => day,
        _ => date,
    };
    format!("Servers will be available on {date}")
}

/// One "name, size, maybe a button" line, as the Storage page draws them.
pub(crate) fn usage_row<'a>(
    name: Element<'a, Message>,
    bytes: u64,
    action: Option<Element<'a, Message>>,
) -> Element<'a, Message> {
    row![
        container(name).width(Length::Fill),
        container(mono(size_of(bytes)).align_x(Alignment::End)).width(Length::Fixed(SIZE_COLUMN)),
        container(action.unwrap_or_else(|| Space::new().width(Length::Shrink).into()))
            .width(Length::Fixed(ACTION_COLUMN))
            .align_x(Alignment::End),
    ]
    .spacing(theme::MD)
    .align_y(Alignment::Center)
    .into()
}

/// The optional downloads Lightforge offers to switch on.
///
/// What is *shown*, not what is gated: every optional tag stays off unless
/// opted in. `Alternate` and `Talebound` get no switch.
pub(crate) const OFFERED_OPTIONAL: &[&str] = &[HIGH_RES];

/// The high-resolution texture pack's manifest tag.
pub(crate) const HIGH_RES: &str = "HighRes";

/// The tags in `tags` that [`OFFERED_OPTIONAL`] names.
pub(crate) fn offered_only(tags: &[String]) -> Vec<String> {
    tags.iter().filter(|tag| OFFERED_OPTIONAL.contains(&tag.as_str())).cloned().collect()
}

/// A human label for an optional-download tag.
///
/// The manifest's own name is the fallback, for anything added to
/// [`OFFERED_OPTIONAL`] before it gets a phrase.
pub(crate) fn optional_label(tag: &str) -> String {
    match tag {
        "HighRes" => "High-resolution textures".to_owned(),
        other => other.to_owned(),
    }
}

/// `~/Games/Blizzard`, not `/home/ann/Games/Blizzard`. Only the home
/// prefix: `/run/media/ann/ssd` names the drive.
pub(crate) fn home_relative(path: &Path) -> String {
    let Some(home) = std::env::var_os("HOME").map(PathBuf::from) else {
        return path.display().to_string();
    };
    match path.strip_prefix(&home) {
        Ok(rest) => format!("~/{}", rest.display()),
        Err(_) => path.display().to_string(),
    }
}

/// Whether a key press is the chord that reveals the hints.
///
/// All three modifiers, so no slip triggers it.
pub(crate) fn hints_chord(key: &iced::keyboard::Key, modifiers: iced::keyboard::Modifiers) -> bool {
    matches!(key.as_ref(), iced::keyboard::Key::Character(HINTS_CHORD))
        && modifiers.control()
        && modifiers.alt()
        && modifiers.shift()
}

/// Height of the game band's own row.
///
/// Pinned: filling the cross axis would take the shorter buttons' height.
pub(crate) fn strip_height() -> f32 {
    theme::tile() + theme::LG
}

/// Whether Steam started this launcher (`SteamGameId` is set). Otherwise
/// [`STEAM_KEYBOARD`] starts the Steam client (measured 2026-10-17).
pub(crate) fn launched_by_steam() -> bool {
    std::env::var_os("SteamGameId").is_some_and(|id| !id.is_empty())
}

/// The handful of facts worth filtering a pile of issues by.
///
/// All already in the bundle; tags let GlitchTip group on them.
pub(crate) fn report_tags(info: &DebugInfo) -> Vec<(String, String)> {
    let mut tags = vec![
        ("lightforge".to_owned(), info.app_version.clone()),
        ("product".to_owned(), info.product.clone()),
        ("state".to_owned(), info.action_state.clone()),
        ("os".to_owned(), info.os.clone()),
        ("kernel".to_owned(), info.kernel.clone()),
    ];
    for (key, value) in [
        ("version", &info.build_version),
        ("proton", &info.proton),
        ("session", &info.session),
        ("gpu", &info.graphics),
        ("gpu_driver", &info.gpu_driver),
    ] {
        if let Some(value) = value {
            tags.push((key.to_owned(), value.clone()));
        }
    }
    tags
}

/// Which tiles the band shows, and which go behind the overflow button.
///
/// Pure, so the rules can be tested without a window: `widths` is what each
/// tile needs, `room` what the band has for tiles, and the answer is indices
/// into `widths`: shown first, hidden second, both in the order given.
///
/// Nothing is hidden while everything fits; the selected line never is.
pub(crate) fn divide_strip(widths: &[f32], selected: usize, room: f32) -> (Vec<usize>, Vec<usize>) {
    let gap = theme::SM;
    let span = |ids: &[usize]| -> f32 {
        ids.iter().map(|id| widths[*id]).sum::<f32>()
            + gap * ids.len().saturating_sub(1) as f32
    };

    let all: Vec<usize> = (0..widths.len()).collect();
    if all.is_empty() || span(&all) <= room {
        return (all, Vec::new());
    }

    let budget = room - BAND_BUTTON - gap;
    let mut shown: Vec<usize> = Vec::new();
    for id in 0..widths.len() {
        let mut next = shown.clone();
        next.push(id);
        if span(&next) > budget {
            break;
        }
        shown = next;
    }

    // The selected line replaces the last tiles that fit.
    if !shown.contains(&selected) {
        shown.pop();
        while !shown.is_empty() {
            let mut with_selected = shown.clone();
            with_selected.push(selected);
            if span(&with_selected) <= budget {
                break;
            }
            shown.pop();
        }
        shown.push(selected);
    }

    let hidden = all.iter().copied().filter(|id| !shown.contains(id)).collect();
    (shown, hidden)
}

/// `2026-10-04T20:10:34.237Z` -> `4 Oct`.
///
/// Day and month only, to save the column.
pub(crate) fn short_day(timestamp: &str) -> String {
    pub(crate) const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let date = timestamp.split('T').next().unwrap_or(timestamp);
    let mut parts = date.split('-');
    let (Some(_year), Some(month), Some(day)) = (parts.next(), parts.next(), parts.next()) else {
        // Not a shape we know. Showing it raw is more use than hiding it.
        return timestamp.to_owned();
    };
    match (month.parse::<usize>(), day.parse::<u32>()) {
        (Ok(m), Ok(d)) if (1..=12).contains(&m) => format!("{d} {}", MONTHS[m - 1]),
        _ => timestamp.to_owned(),
    }
}

/// The day out of an RFC 3339 timestamp, which is all a build list needs.
///
/// Not parsed: a malformed one shows as itself.
pub(crate) fn day_of(timestamp: &str) -> String {
    timestamp.split('T').next().unwrap_or(timestamp).to_owned()
}

/// A path as people say it: under the home directory it starts with `~`.
pub(crate) fn tilde(path: &std::path::Path) -> String {
    match std::env::var_os("HOME").map(PathBuf::from) {
        Some(home) if path.starts_with(&home) && home.as_os_str().len() > 1 => {
            format!("~/{}", path.strip_prefix(&home).unwrap_or(path).display())
        }
        _ => path.display().to_string(),
    }
}

/// Place a transfer sample within its job; see [`Job`].
pub(crate) fn job_progress(job: Job, sample: &TransferState, bytes_per_second: u64) -> state::Progress {
    let stored = sample.progress.bytes_stored;
    let (downloaded, total) = match (job.total, sample.remaining) {
        // Whatever the job planned that this run no longer has to fetch is
        // already done.
        (Some(total), Some(remaining)) => {
            ((total.saturating_sub(remaining) + stored).min(total), total)
        }
        // Resumed and still planning.
        (Some(total), None) => (job.reached.min(total), total),
        // A first run before its plan: nothing to measure against yet.
        (None, _) => (stored, sample.progress.bytes_total),
    };
    state::Progress { downloaded_bytes: downloaded, total_bytes: total, bytes_per_second }
}

/// A repeating 0 to 1 sweep for work of unknown length. iced's progress bar
/// has no indeterminate mode; a fill that restarts says "something is
/// happening". Off the clock, which the 100 ms tick redraws by anyway.
pub(crate) fn sweep() -> f32 {
    pub(crate) const PERIOD: f32 = 1.4;
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();
    let elapsed = START.get_or_init(Instant::now).elapsed().as_secs_f32();
    (elapsed % PERIOD) / PERIOD
}

/// Progress carried by the two transfer states, if this is one of them.
pub(crate) fn progress_of(action: &ActionState) -> Option<state::Progress> {
    match action {
        ActionState::Downloading(p) | ActionState::Paused(p) => Some(*p),
        _ => None,
    }
}

/// A download count, short enough to sit in a caption beside the author.
///
/// "98.4M", "142M", "847": one decimal unless it would read ".0".
pub(crate) fn downloads_of(n: u64) -> String {
    let scaled = |n: u64, unit: f64, suffix: &str| {
        let value = n as f64 / unit;
        // Rounded to choose the decimals, raw when printed: no "100.0M" or "1000M".
        let rounded = (value * 10.0).round() / 10.0;
        match rounded.fract() < 0.05 || rounded >= 100.0 {
            true => format!("{value:.0}{suffix}"),
            false => format!("{rounded:.1}{suffix}"),
        }
    };
    // The thresholds promote half a unit early, where the smaller unit would
    // round to four digits: 999,999,999 is "1B", never "1000M".
    match n {
        n if n >= 999_500_000 => scaled(n, 1e9, "B"),
        n if n >= 999_500 => scaled(n, 1e6, "M"),
        n if n >= 1_000 => scaled(n, 1e3, "K"),
        n => n.to_string(),
    }
}

pub(crate) fn bytes_of(done: u64, total: u64) -> String {
    let gib = |b: u64| b as f64 / 1024f64.powi(3);
    match total {
        0 => format!("{:.2} GiB", gib(done)),
        _ => format!("{:.2} of {:.2} GiB", gib(done), gib(total)),
    }
}

/// Describe a config that would not load, keeping a copy of it aside.
///
/// The whole error chain, down to the line and column.
pub(crate) fn broken_config(error: &lightforge_core::config::ConfigError, path: &Path) -> String {
    let mut message = error.to_string();
    let mut source = std::error::Error::source(error);
    while let Some(cause) = source {
        message.push_str(&format!("\n{cause}"));
        source = cause.source();
    }
    let backup = path.with_extension("toml.broken");
    if std::fs::copy(path, &backup).is_err() {
        message.push_str("\n(could not save a copy to config.toml.broken)");
    }
    format!("{}\n\n{message}", path.display())
}

/// A typed folder as a full path, `~/` expanded.
pub(crate) fn parse_path(typed: &str) -> Result<PathBuf, String> {
    let typed = typed.trim();
    let path = match typed.strip_prefix("~/") {
        Some(rest) => {
            PathBuf::from(std::env::var_os("HOME").ok_or("cannot find your home directory")?)
                .join(rest)
        }
        None => PathBuf::from(typed),
    };
    match path.is_absolute() {
        true => Ok(path),
        false => Err(format!("{typed} is not a full path — start it with / or ~/")),
    }
}

/// A size in the unit it reads best in: a 16 MB cache is not "0.02 GiB".
pub(crate) fn size_of(bytes: u64) -> String {
    pub(crate) const MIB: f64 = 1024.0 * 1024.0;
    let b = bytes as f64;
    match b {
        b if b >= MIB * 1024.0 * 1024.0 => format!("{:.2} TiB", b / (MIB * 1024.0 * 1024.0)),
        b if b >= MIB * 1024.0 => format!("{:.1} GiB", b / (MIB * 1024.0)),
        // A screenshot is the only thing here smaller than a megabyte, and
        // "0 MiB" beside a thumbnail reads as a broken file.
        b if b >= MIB => format!("{:.0} MiB", b / MIB),
        b => format!("{:.0} KiB", b / 1024.0),
    }
}

pub(crate) fn rate_of(per_second: u64) -> String {
    let mb = per_second as f64 / 1e6;
    match per_second {
        0 => "—".to_owned(),
        // Tenths tell a slow link apart from a stalled one; at speed they are
        // only jitter, and a digit that moves every second draws the eye.
        _ if mb >= 10.0 => format!("{mb:.0} MB/s"),
        _ => format!("{mb:.1} MB/s"),
    }
}

pub(crate) fn eta_of(progress: state::Progress) -> String {
    match progress.eta() {
        None => "ETA —".to_owned(),
        Some(eta) => {
            let secs = eta.as_secs();
            match secs {
                0..=59 => format!("ETA {secs}s"),
                60..=3599 => format!("ETA {}m", secs / 60),
                _ => format!("ETA {}h {}m", secs / 3600, (secs % 3600) / 60),
            }
        }
    }
}
