use std::path::{Path, PathBuf};

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
    builds_in(proton_dir, ProtonSource::Managed)
}

/// Every Proton build available: Lightforge's own, then everything in the
/// user's Steam libraries and the system's compatibility tool folders. Own
/// builds win the dedup against the same version.
pub fn discover_all(
    steam: Option<&crate::steam::SteamRoot>,
    proton_dir: &Path,
) -> Vec<ProtonRelease> {
    discover_with(steam, proton_dir, &system_compat_dirs())
}

/// [`discover_all`] with the system's folders given, so a test sees none of
/// the machine's own.
pub(super) fn discover_with(
    steam: Option<&crate::steam::SteamRoot>,
    proton_dir: &Path,
    system_dirs: &[PathBuf],
) -> Vec<ProtonRelease> {
    let mut found = discover_managed(proton_dir);
    if let Some(steam) = steam {
        found.extend(discover_installed(steam));
    }
    for dir in system_dirs {
        found.extend(builds_in(dir, ProtonSource::SystemCompatTools));
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

/// Newer than [`TESTED`] and known to break setup, so having one installed is
/// no reason to stay off [`TESTED`]. GE-Proton11-9: `Battle.net-Setup.exe`
/// dies on start, `c00000fd`, exit 253.
const KNOWN_BAD: [&str; 1] = ["GE-Proton11-9"];

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
/// before setup, not when a newer build is installed (one in [`KNOWN_BAD`]
/// does not count), and not when the prefix was made by a newer build.
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
    if supported.any(|b| ge_version(&b.version) > tested && !KNOWN_BAD.contains(&b.version.as_str())) {
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
            found.extend(builds_in(&dir, source));
        }
    }

    found.sort_by(|a, b| a.version.cmp(&b.version));
    found.dedup_by(|a, b| a.version == b.version);
    found
}

/// Where distribution packages put Proton (CachyOS's `proton-cachyos`, the
/// AUR's `proton-ge-custom-bin`), with or without Steam, and the extra
/// folders Steam itself is told to read.
fn system_compat_dirs() -> Vec<PathBuf> {
    let extra = std::env::var_os("STEAM_EXTRA_COMPAT_TOOLS_PATHS").unwrap_or_default();
    std::env::split_paths(&extra)
        .filter(|dir| !dir.as_os_str().is_empty())
        .chain(
            ["/usr/share/steam/compatibilitytools.d", "/usr/local/share/steam/compatibilitytools.d"]
                .map(PathBuf::from),
        )
        .collect()
}

fn builds_in(dir: &Path, source: ProtonSource) -> Vec<ProtonRelease> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries.flatten().filter_map(|e| ProtonRelease::read_from_dir(&e.path(), source)).collect()
}
