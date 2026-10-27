use std::path::Path;

use super::discover::{ge_version, parse_version_file};

/// The Proton build that last ran `wineboot` in a compat-data directory.
///
/// Proton writes its own tag to `<compat_data>/version` whenever it creates or
/// upgrades the prefix, so this is the build the prefix *is*: not whatever is
/// pinned now. `None` when no prefix has been built there yet.
pub fn prefix_build(compat_data: &Path) -> Option<String> {
    let contents = std::fs::read_to_string(compat_data.join("version")).ok()?;
    parse_version_file(&contents).map(|(_, tag)| tag)
}

/// How the build about to run compares with the one the prefix carries.
///
/// The prefix is shared by every product, one Battle.net install, one login,
/// and Proton only ever migrates a prefix *forward*. Opening it with an older
/// Wine than the one that last rewrote its registry and DLLs is the way that
/// arrangement breaks, so that is the single case worth stopping for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrefixFit {
    /// No prefix yet; whichever build runs first makes it.
    Fresh,
    /// The build that made it.
    Same,
    /// Newer than the prefix. Proton upgrades it on the way in, which is
    /// routine and is what every update does.
    Newer,
    /// Older than the build named here, a downgrade of the shared prefix.
    Older { built_by: String },
    /// One of the two is not a GE tag, so there is no ordering to trust.
    /// Never warned about: a guess costs more here than silence.
    Unknown,
}

impl PrefixFit {
    /// Whether this is the case to warn about.
    pub fn is_downgrade(&self) -> bool {
        matches!(self, Self::Older { .. })
    }
}

/// Compare the build that is about to run against the one in the prefix.
///
/// `built_by` is [`prefix_build`]'s answer, passed in rather than read here so
/// the UI can read the prefix once and judge every product against it.
pub fn prefix_fit(pick: &str, built_by: Option<&str>) -> PrefixFit {
    let Some(built_by) = built_by else {
        return PrefixFit::Fresh;
    };
    if pick == built_by {
        return PrefixFit::Same;
    }
    match (ge_version(pick), ge_version(built_by)) {
        (Some(pick), Some(prefix)) if pick < prefix => PrefixFit::Older {
            built_by: built_by.to_owned(),
        },
        (Some(_), Some(_)) => PrefixFit::Newer,
        _ => PrefixFit::Unknown,
    }
}
