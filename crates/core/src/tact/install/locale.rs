//! The locales a build ships, and which one an install gets.

use crate::tact::manifest::Tag;

/// The locales a WoW build ships, as the download manifest spells them, with
/// something to call each one in a picker.
///
/// Not every build ships all of them; the plan sorts that out.
pub const LOCALES: &[(&str, &str)] = &[
    ("enUS", "English (US)"),
    ("deDE", "Deutsch"),
    ("frFR", "Français"),
    ("esES", "Español (EU)"),
    ("esMX", "Español (AL)"),
    ("itIT", "Italiano"),
    ("ptBR", "Português (BR)"),
    ("ruRU", "Русский"),
    ("koKR", "한국어"),
    ("zhCN", "简体中文"),
    ("zhTW", "繁體中文"),
];

/// The locale to use for a saved language not on the list: `enGB` reads
/// `enUS` and `ptPT` reads `ptBR`, which is what those clients load.
pub fn current_locale(saved: &str) -> &str {
    match saved {
        "enGB" => "enUS",
        "ptPT" => "ptBR",
        other => other,
    }
}

/// What a locale code is called, or the code itself when it is not one we know.
pub fn locale_name(code: &str) -> &str {
    LOCALES
        .iter()
        .find(|(locale, _)| *locale == code)
        .map(|(_, name)| *name)
        .unwrap_or(code)
}

/// The locale whose files to fetch for a client language.
///
/// `enGB` and `ptPT` have no data pack: the English (EU) client reads `enUS`
/// and takes its language from `Config.wtf`. A tag the manifest lacks selects
/// nothing, and the download "finished" with zero files.
pub(super) fn shipped_locale(wanted: &str, tags: &[Tag]) -> String {
    let ships = |name: &str| tags.iter().any(|t| t.kind == LOCALE_TAG && t.name == name);
    // A product with no locale tags at all has no packs to choose between.
    if ships(wanted) || !tags.iter().any(|t| t.kind == LOCALE_TAG) {
        return wanted.to_owned();
    }
    let nearest = match wanted {
        "ptPT" => "ptBR",
        _ => "enUS",
    };
    match [nearest, "enUS"].into_iter().find(|name| ships(name)) {
        Some(name) => {
            tracing::warn!("this build ships no {wanted} data; fetching {name} instead");
            name.to_owned()
        }
        None => wanted.to_owned(),
    }
}

/// The manifest's category number for locale tags.
pub(super) const LOCALE_TAG: u16 = 3;
