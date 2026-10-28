//! The language the WoW client starts in.
//!
//! The client reads `textLocale` and `audioLocale` from `WTF/Config.wtf`
//! (asking when absent), not `.build.info`; only those two lines are edited.

use std::io;
use std::path::Path;

const KEYS: [&str; 2] = ["textLocale", "audioLocale"];

/// Point `config_wtf` at `locale` atomically, keeping every other line.
pub fn set(config_wtf: &Path, locale: &str) -> io::Result<()> {
    let old = match std::fs::read_to_string(config_wtf) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let mut out: Vec<String> = old
        .lines()
        .filter(|line| !KEYS.iter().any(|key| line.trim_start().starts_with(&format!("SET {key} "))))
        .map(str::to_owned)
        .collect();
    out.extend(KEYS.iter().map(|key| format!("SET {key} \"{locale}\"")));
    crate::fsutil::write_atomic(config_wtf, (out.join("\n") + "\n").as_bytes())
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
    fn creates_the_file_when_there_is_none() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("WTF").join("Config.wtf");
        set(&file, "frFR").unwrap();
        assert!(std::fs::read_to_string(&file).unwrap().contains("SET textLocale \"frFR\""));
    }
}
