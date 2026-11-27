//! The language the WoW client starts in.
//!
//! The client reads `textLocale` and `audioLocale` from `WTF/Config.wtf`
//! (asking when absent), not `.build.info`; only those two lines are edited.

use std::io;
use std::path::Path;

/// Point `config_wtf` at `locale` atomically, keeping every other line.
pub fn set(config_wtf: &Path, locale: &str) -> io::Result<()> {
    crate::cvars::set(config_wtf, &[("textLocale", locale), ("audioLocale", locale)])
}

/// [`set`], unless the client already has a language: a first start then
/// skips the client's own picker, and a language chosen in game stays.
pub fn seed(config_wtf: &Path, locale: &str) -> io::Result<()> {
    if crate::cvars::get(config_wtf, "textLocale").is_some() {
        return Ok(());
    }
    set(config_wtf, locale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_both_keys_and_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Config.wtf");
        std::fs::write(&file, "SET portal \"EU\"\nSET textLocale \"deDE\"\nSET audioLocale \"deDE\"\n").unwrap();
        set(&file, "enUS").unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("SET portal \"EU\""));
        assert!(text.contains("SET textLocale \"enUS\"") && text.contains("SET audioLocale \"enUS\""));
        assert!(!text.contains("deDE"));
    }

    #[test]
    fn seeding_keeps_a_language_the_client_already_has() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Config.wtf");
        std::fs::write(&file, "SET textLocale \"deDE\"\nSET audioLocale \"deDE\"\n").unwrap();
        seed(&file, "enUS").unwrap();
        assert!(!std::fs::read_to_string(&file).unwrap().contains("enUS"));
    }

    #[test]
    fn seeding_a_fresh_install_writes_both_keys() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("WTF").join("Config.wtf");
        seed(&file, "enGB").unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("SET textLocale \"enGB\"") && text.contains("SET audioLocale \"enGB\""));
    }

    #[test]
    fn creates_the_file_when_there_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("WTF").join("Config.wtf");
        set(&file, "frFR").unwrap();
        assert!(std::fs::read_to_string(&file).unwrap().contains("SET textLocale \"frFR\""));
    }
}
