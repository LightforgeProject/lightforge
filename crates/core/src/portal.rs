//! Which login region a directly launched client signs in to: `SET portal`
//! in its `WTF/Config.wtf`. A launch through Battle.net follows Battle.net's
//! own region picker instead.

use std::fmt;
use std::io;
use std::path::Path;

use crate::paths::Layout;
use crate::product::ProductCode;
use crate::tact::version::Region;

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

/// Whose build a game is checked and updated against: the region it logs
/// into, else `configured`. Blizzard rolls a build out region by region, and
/// a client behind its login region's build cannot log in: wowf 1.60.1.70888
/// went to NA on 2026-11-10 and to EU a day later (GlitchTip #46).
pub fn build_region(layout: &Layout, code: &ProductCode, configured: Region) -> Region {
    match read(&layout.wtf_dir(code).join("Config.wtf")) {
        Some(Portal::Eu) => Region::Eu,
        Some(Portal::Us) => Region::Us,
        None => configured,
    }
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

    #[test]
    fn a_game_that_logs_into_na_is_checked_against_na() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::with_root(dir.path());
        set(&layout.wtf_dir(&ProductCode::Forever).join("Config.wtf"), Portal::Us).unwrap();
        assert_eq!(build_region(&layout, &ProductCode::Forever, Region::Eu), Region::Us);
    }

    #[test]
    fn a_game_without_a_login_region_is_checked_against_the_configured_one() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::with_root(dir.path());
        assert_eq!(build_region(&layout, &ProductCode::Forever, Region::Kr), Region::Kr);
    }
}
