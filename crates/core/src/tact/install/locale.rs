//! The locales a build ships, and which one an install gets.

use crate::tact::manifest::Tag;

/// The locales a WoW build ships, as the download manifest spells them, with
/// something to call each one in a picker.
///
/// Not every product has every one, a Classic Era build ships fewer, but the
/// manifest decides that at plan time: a tag nothing defines selects nothing
/// rather than failing, so offering the full list is safe and the alternative
/// is reading a manifest before the user can pick a language.
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

/// The locale to use for a saved language that is not on the list.
///
/// `enGB` and `ptPT` used to be offered, but no build ships data for them (the
/// English (EU) client reads `enUS`, Portuguese reads `ptBR`), so a download in
/// either selected nothing. A config that still says so is read as the
/// language it meant.
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
/// A build ships a data pack per locale tag, and `enGB` (English (EU)) and
/// `ptPT` are client languages with none: the English (EU) client reads the
/// `enUS` files and takes its language from `Config.wtf`. Asking the manifest
/// for a tag it does not define selects *nothing*, so the download "finished"
/// with zero files and no error, which is how it looked like a hang.
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
