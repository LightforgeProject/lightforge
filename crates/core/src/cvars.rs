//! Lines of the client's `WTF/Config.wtf`: `SET <name> "<value>"`.
//!
//! Only the named lines are touched. The client rewrites the file when it
//! exits, so a write while it runs is lost.

use std::io;
use std::path::Path;

/// A cvar's value as the file holds it. Names match case-insensitively, as
/// the client reads them.
pub fn get(config_wtf: &Path, name: &str) -> Option<String> {
    let text = std::fs::read_to_string(config_wtf).ok()?;
    text.lines().find_map(|line| {
        let (key, value) = line.trim().strip_prefix("SET ")?.split_once(' ')?;
        key.eq_ignore_ascii_case(name).then(|| value.trim().trim_matches('"').to_owned())
    })
}

/// Set each `(name, value)` atomically, keeping every other line. A missing
/// file, or `WTF` folder, is created.
pub fn set(config_wtf: &Path, values: &[(&str, &str)]) -> io::Result<()> {
    let old = match std::fs::read_to_string(config_wtf) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => String::new(),
        Err(e) => return Err(e),
    };
    let named = |line: &str| {
        let key = line.trim().strip_prefix("SET ").and_then(|rest| rest.split_whitespace().next());
        key.is_some_and(|key| values.iter().any(|(name, _)| key.eq_ignore_ascii_case(name)))
    };
    let mut out: Vec<String> = old.lines().filter(|line| !named(line)).map(str::to_owned).collect();
    out.extend(values.iter().map(|(name, value)| format!("SET {name} \"{value}\"")));
    crate::fsutil::write_atomic(config_wtf, (out.join("\n") + "\n").as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_is_read_whatever_the_case_of_its_name() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Config.wtf");
        // From wowf's Config.wtf, 2026-11-01.
        std::fs::write(&file, "SET textLocale \"enUS\"\nSET portal \"EU\"\n").unwrap();
        assert_eq!(get(&file, "portal").as_deref(), Some("EU"));
        assert_eq!(get(&file, "PORTAL").as_deref(), Some("EU"));
        assert_eq!(get(&file, "realmList"), None);
    }

    #[test]
    fn setting_replaces_the_line_and_keeps_the_rest() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Config.wtf");
        std::fs::write(&file, "SET textLocale \"enUS\"\nSET Portal \"EU\"\n").unwrap();
        set(&file, &[("portal", "US")]).unwrap();
        let text = std::fs::read_to_string(&file).unwrap();
        assert_eq!(text, "SET textLocale \"enUS\"\nSET portal \"US\"\n");
    }
}
