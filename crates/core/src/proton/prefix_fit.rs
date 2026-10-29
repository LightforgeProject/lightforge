use std::path::Path;

use super::discover::{ge_version, parse_version_file};

/// The Proton build that last created or upgraded the prefix, from the tag
/// Proton writes to `<compat_data>/version`. `None` before a prefix exists.
pub fn prefix_build(compat_data: &Path) -> Option<String> {
    let contents = std::fs::read_to_string(compat_data.join("version")).ok()?;
    parse_version_file(&contents).map(|(_, tag)| tag)
}

/// How the build about to run compares with the one the prefix carries.
/// Proton only migrates a prefix forward, so an older build is the one case
/// worth stopping for: the prefix is every product's.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrefixFit {
    /// No prefix yet; whichever build runs first makes it.
    Fresh,
    /// The build that made it.
    Same,
    /// Newer than the prefix; Proton upgrades it on the way in.
    Newer,
    /// Older than the build named here, a downgrade of the shared prefix.
    Older { built_by: String },
    /// One of the two is not a GE tag: no ordering, so no warning.
    Unknown,
}

impl PrefixFit {
    /// Whether this is the case to warn about.
    pub fn is_downgrade(&self) -> bool {
        matches!(self, Self::Older { .. })
    }
}

/// Compare the build about to run against `built_by`, [`prefix_build`]'s
/// answer, read once by the caller for every product.
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
