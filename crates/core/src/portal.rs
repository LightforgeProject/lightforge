//! Which login region a directly launched client signs in to: `SET portal`
//! in its `WTF/Config.wtf`. A launch through Battle.net follows Battle.net's
//! own region picker instead.

use std::fmt;
use std::io;
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Portal {
    Eu,
    /// The Americas and Oceania; the cvar calls it `US`, players NA.
    Us,
}

impl Portal {
    pub const ALL: [Portal; 2] = [Portal::Eu, Portal::Us];

    fn cvar(self) -> &'static str {
        match self {
            Self::Eu => "EU",
            Self::Us => "US",
        }
    }
}

impl fmt::Display for Portal {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Eu => "EU",
            Self::Us => "NA",
        })
    }
}

/// The portal `config_wtf` names; `None` for none, or one of the other
/// regions (`KR`, `TW`, `CN`).
pub fn read(config_wtf: &Path) -> Option<Portal> {
    let value = crate::cvars::get(config_wtf, "portal")?;
    Portal::ALL.into_iter().find(|portal| value.eq_ignore_ascii_case(portal.cvar()))
}

pub fn set(config_wtf: &Path, portal: Portal) -> io::Result<()> {
    crate::cvars::set(config_wtf, &[("portal", portal.cvar())])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn switching_to_na_writes_us_and_reads_back() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("WTF").join("Config.wtf");
        set(&file, Portal::Us).unwrap();
        assert!(std::fs::read_to_string(&file).unwrap().contains("SET portal \"US\""));
        assert_eq!(read(&file), Some(Portal::Us));
    }

    #[test]
    fn a_region_without_a_choice_reads_as_none() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("Config.wtf");
        std::fs::write(&file, "SET portal \"KR\"\n").unwrap();
        assert_eq!(read(&file), None);
    }
}
