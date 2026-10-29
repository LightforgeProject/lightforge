use std::path::Path;

use super::{prefix_fit, ProtonRelease, ProtonSource};

/// Parse a Proton `version` file: `<unix timestamp> <tag>`.
pub(super) fn parse_version_file(contents: &str) -> Option<(Option<u64>, String)> {
    let line = contents.lines().next()?.trim();
    match line.split_once(char::is_whitespace) {
        Some((ts, tag)) => Some((ts.parse().ok(), tag.trim().to_owned())),
        // Some builds ship only the tag.
        None if !line.is_empty() => Some((None, line.to_owned())),
        None => None,
    }
}

/// Builds Lightforge downloaded and manages itself.
pub fn discover_managed(proton_dir: &Path) -> Vec<ProtonRelease> {
    let Ok(entries) = std::fs::read_dir(proton_dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .filter_map(|e| ProtonRelease::read_from_dir(&e.path(), ProtonSource::Managed))
        .collect()
}

/// Every Proton build available: Lightforge's own, then everything in the
/// user's Steam libraries. Own builds win the dedup against the same version.
pub fn discover_all(
    steam: Option<&crate::steam::SteamRoot>,
    proton_dir: &Path,
) -> Vec<ProtonRelease> {
    let mut found = discover_managed(proton_dir);
    if let Some(steam) = steam {
        found.extend(discover_installed(steam));
    }
    found.sort_by(|a, b| {
        // GE tags numerically (9-20, 10-34, 11-7); equal versions stay
        // adjacent for the dedup.
        (ge_version(&a.version), &a.version)
            .cmp(&(ge_version(&b.version), &b.version))
            // Managed sorts before SystemCompatTools/SteamApp, so dedup keeps it.
            .then_with(|| {
                (a.source != ProtonSource::Managed).cmp(&(b.source != ProtonSource::Managed))
            })
    });
    found.dedup_by(|a, b| a.version == b.version);
    found
}

/// The build Lightforge is tested against, used whenever nothing is pinned.
/// Newest is not safest: GE-Proton11-9 crashes Battle.net's installer
/// (`c00000fd`). Moved by hand once a build has passed setup, a Battle.net
/// sign-in and a game launch.
pub const TESTED: &str = "GE-Proton11-7";

/// Choose the build to launch with. A pin is honoured exactly, never swapped
/// for another build. Without one, [`TESTED`] when installed, else the newest
/// supported build by its `version` file date (tags do not sort as strings).
pub fn select<'a>(builds: &'a [ProtonRelease], pin: Option<&str>) -> Option<&'a ProtonRelease> {
    match pin {
        Some(version) => builds.iter().find(|b| b.version == version),
        None => builds.iter().find(|b| b.version == TESTED).or_else(|| {
            builds
                .iter()
                .filter(|b| b.flavor.is_default_candidate())
                .max_by_key(|b| (b.built_at, b.version.clone()))
        }),
    }
}

/// Whether to fetch [`TESTED`] for a machine set up before it was the
/// default (GE-Proton11-5 could not start Warcraft III's web UI; 11-7 can).
/// Only ever a step forward: not under a pin, not when it is installed, not
/// before setup, not when a newer build is installed, and not when the prefix
/// was made by a newer build.
pub fn wants_tested(
    installed: &[ProtonRelease],
    pin: Option<&str>,
    prefix_built_by: Option<&str>,
) -> bool {
    if pin.is_some() || installed.iter().any(|b| b.version == TESTED) {
        return false;
    }
    let mut supported = installed.iter().filter(|b| b.flavor.is_default_candidate()).peekable();
    if supported.peek().is_none() {
        return false;
    }
    let tested = ge_version(TESTED);
    if supported.any(|b| ge_version(&b.version) > tested) {
        return false;
    }
    !prefix_fit(TESTED, prefix_built_by).is_downgrade()
}

/// A GE release tag as numbers for ordering: `GE-Proton11-7` → `(11, 7)`.
/// As strings `GE-Proton11-7` < `GE-Proton9-20`, and a prefix records only
/// the tag.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub(super) struct GeVersion {
    major: u32,
    minor: u32,
}

/// Parse a GE tag, or `None` for anything else (Valve's, umu's builds).
pub(super) fn ge_version(tag: &str) -> Option<GeVersion> {
    let (major, rest) = tag.strip_prefix("GE-Proton")?.split_once('-')?;
    Some(GeVersion {
        major: major.parse().ok()?,
        // `GE-Proton11-7-x86_64` never appears in a `version` file, but a
        // directory name read in its place carries the suffix.
        minor: rest.split('-').next()?.parse().ok()?,
    })
}

/// Every Proton build installed across the user's Steam libraries.
///
/// Unsupported flavors are included; filter with [`super::ProtonFlavor::is_supported`]
/// at the point of use.
pub fn discover_installed(steam: &crate::steam::SteamRoot) -> Vec<ProtonRelease> {
    let mut found = Vec::new();

    for library in steam.library_paths() {
        let sources = [
            (
                library.join("compatibilitytools.d"),
                ProtonSource::SystemCompatTools,
            ),
            (
                library.join("steamapps").join("common"),
                ProtonSource::SteamApp,
            ),
        ];

        for (dir, source) in sources {
            let Ok(entries) = std::fs::read_dir(&dir) else {
                continue;
            };
            for entry in entries.flatten() {
                if let Some(release) = ProtonRelease::read_from_dir(&entry.path(), source) {
                    found.push(release);
                }
            }
        }
    }

    found.sort_by(|a, b| a.version.cmp(&b.version));
    found.dedup_by(|a, b| a.version == b.version);
    found
}
