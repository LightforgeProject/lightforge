//! Proton-GE version model.
//!
//! Policy is **pinned + notify-only**: Lightforge never swaps a working Proton
//! build out from under the user.
//!
//! All products share one prefix, so a pin selects which Proton-GE binary runs
//! at launch rather than isolating a prefix. The stale-shader safety step on a
//! version swap therefore clears only that product's
//! [`crate::paths::AppDirs::shader_cache_dir`].

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// GitHub repository Proton-GE releases are polled from.
pub const GE_RELEASES_REPO: &str = "GloriousEggroll/proton-ge-custom";

/// The build WoW's in-game shop is known to work under.
///
/// Newer GE builds start the shop's browser (`Utils/BlizzardBrowser.exe`) but
/// lose its GPU child, the grey box instead of a page. The other half of the
/// fix is the install's own drive ([`crate::paths::Layout::wow_drive`]).
/// Measured 2026-10-02. A recommendation only: other builds play fine
/// without a shop.
pub const SHOP_BUILD: &str = "GE-Proton11-7";

/// Where a given Proton build comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtonSource {
    /// Downloaded and unpacked by Lightforge into its own data directory.
    Managed,
    /// An existing install in the user's Steam `compatibilitytools.d`.
    SystemCompatTools,
    /// A Valve Proton installed as a regular Steam app.
    SteamApp,
}

/// Which Proton lineage a build belongs to.
///
/// Only [`ProtonFlavor::GloriousEggroll`] is a supported launch target;
/// everything else is detected so the UI can show what is installed and
/// explain why it isn't offered, rather than silently omitting it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProtonFlavor {
    /// Proton-GE, the supported target.
    GloriousEggroll,
    /// Valve's official Proton, including Experimental.
    Valve,
    /// umu's Proton build. Out of scope, umu-launcher is not a runtime target.
    Umu,
    /// Anything else found in `compatibilitytools.d`.
    Other,
}

impl ProtonFlavor {
    /// Classify a build by its version tag.
    ///
    /// Matching is case-insensitive: Valve tags their builds in lowercase
    /// (`proton-11.0-2-x86_64`, `experimental-11.0-20260910b-x86_64`) while
    /// GE and umu use mixed case.
    pub fn classify(version: &str) -> Self {
        let v = version.to_ascii_lowercase();

        // Order matters: umu builds are Proton derivatives and would otherwise
        // match the lineage checks below.
        if v.starts_with("umu-") {
            Self::Umu
        } else if v.starts_with("ge-proton") || v.starts_with("proton-ge") {
            Self::GloriousEggroll
        } else if v.starts_with("proton") || v.starts_with("experimental") {
            Self::Valve
        } else {
            Self::Other
        }
    }

    /// Whether Lightforge will launch games with this flavor.
    pub fn is_supported(&self) -> bool {
        matches!(self, Self::GloriousEggroll)
    }
}

/// An installed Proton build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProtonRelease {
    /// Upstream release tag, e.g. `GE-Proton11-6`.
    ///
    /// Read from the build's `version` file rather than its directory name,
    /// they disagree in practice (`GE-Proton11-6-x86_64/` ships
    /// `version` = `GE-Proton11-6`), and only the tag matches what the GitHub
    /// Releases API reports.
    pub version: String,
    pub flavor: ProtonFlavor,
    pub source: ProtonSource,
    /// Directory containing the `proton` entry-point script.
    pub path: PathBuf,
    /// Build timestamp from the `version` file, seconds since the epoch.
    pub built_at: Option<u64>,
}

impl ProtonRelease {
    /// Read a build from a compatibility-tool directory.
    ///
    /// Returns `None` when the directory has no `proton` entry point, partial
    /// extractions and stray directories are common in `compatibilitytools.d`.
    pub fn read_from_dir(dir: &Path, source: ProtonSource) -> Option<Self> {
        if !dir.join("proton").is_file() {
            return None;
        }

        let from_file = std::fs::read_to_string(dir.join("version"))
            .ok()
            .and_then(|s| parse_version_file(&s));

        // Fall back to the directory name when `version` is absent, which is
        // the case for Valve's `Proton - Experimental`.
        let (built_at, version) = match from_file {
            Some(parsed) => parsed,
            None => (None, dir.file_name()?.to_string_lossy().into_owned()),
        };

        Some(Self {
            flavor: ProtonFlavor::classify(&version),
            version,
            source,
            path: dir.to_path_buf(),
            built_at,
        })
    }

    /// The `proton` entry-point script invoked to run anything in the prefix.
    pub fn entry_point(&self) -> PathBuf {
        self.path.join("proton")
    }
}

/// Parse a Proton `version` file: `<unix timestamp> <tag>`.
fn parse_version_file(contents: &str) -> Option<(Option<u64>, String)> {
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

/// Every Proton build available to Lightforge: its own managed installs plus
/// everything found across the user's Steam libraries.
///
/// Managed builds are listed first so they win the dedup against an identical
/// version installed under Steam, Lightforge controls its own copy's integrity.
pub fn discover_all(
    steam: Option<&crate::steam::SteamRoot>,
    proton_dir: &Path,
) -> Vec<ProtonRelease> {
    let mut found = discover_managed(proton_dir);
    if let Some(steam) = steam {
        found.extend(discover_installed(steam));
    }
    found.sort_by(|a, b| {
        // Numerically where both are GE tags, so the list reads 9-20, 10-34,
        // 11-7 rather than in the lexical order those strings have. Equal
        // versions stay adjacent either way, which is what the dedup needs.
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
///
/// Newest is not safest. GE-Proton11-9 hangs `wineboot` when given a game id
/// and crashes Battle.net's installer on start (`c00000fd`), and it was the
/// newest build on the machine it broke. Moved by hand, after a release has
/// been through setup, a Battle.net sign-in and a game launch.
pub const TESTED: &str = "GE-Proton11-7";

/// Choose the build to launch with.
///
/// A pin is honored exactly, silently falling back to another build is how a
/// user ends up debugging a regression against a Proton version they never
/// selected. Without a pin, [`TESTED`] wins when it is installed. Only when it
/// is not does the newest supported build stand in, dated by the `version`
/// file rather than by tag, since tags don't sort numerically
/// (`GE-Proton11-10` < `GE-Proton11-7` lexically). Setup installs [`TESTED`]
/// when it is missing, so the stand-in is for machines set up before it.
pub fn select<'a>(builds: &'a [ProtonRelease], pin: Option<&str>) -> Option<&'a ProtonRelease> {
    match pin {
        Some(version) => builds.iter().find(|b| b.version == version),
        None => builds.iter().find(|b| b.version == TESTED).or_else(|| {
            builds
                .iter()
                .filter(|b| b.flavor.is_supported())
                .max_by_key(|b| (b.built_at, b.version.clone()))
        }),
    }
}

/// Whether to fetch [`TESTED`] on the user's behalf.
///
/// For a machine set up before [`TESTED`] was the default, which otherwise
/// stays on whatever build it had: a GE-Proton11-5 install could not start
/// Warcraft III's web UI, and 11-7 could. Only ever a step *forward*, so:
/// nothing is fetched under a pin (a pin is a choice), when it is already
/// installed, when no supported build is installed yet (setup installs one),
/// when a newer build is installed (someone who has one and plays on it keeps
/// it), or when the prefix was made by a newer build than [`TESTED`], which
/// would turn the switch into a downgrade of the shared prefix.
pub fn wants_tested(
    installed: &[ProtonRelease],
    pin: Option<&str>,
    prefix_built_by: Option<&str>,
) -> bool {
    if pin.is_some() || installed.iter().any(|b| b.version == TESTED) {
        return false;
    }
    let mut supported = installed.iter().filter(|b| b.flavor.is_supported()).peekable();
    if supported.peek().is_none() {
        return false;
    }
    let tested = ge_version(TESTED);
    if supported.any(|b| ge_version(&b.version) > tested) {
        return false;
    }
    !prefix_fit(TESTED, prefix_built_by).is_downgrade()
}

/// A GE release tag as the pair of numbers it is, for ordering.
///
/// `GE-Proton11-7` → `(11, 7)`. Needed because these tags do not sort as
/// strings, `GE-Proton11-7` < `GE-Proton9-20` lexically, and because the
/// build that made a prefix is recorded as a bare tag with no build timestamp
/// beside it, so there is nothing else to compare on.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
struct GeVersion {
    major: u32,
    minor: u32,
}

/// Parse a GE tag, or `None` for anything else, Valve's builds, UMU's, a
/// directory name read as a fallback. Incomparable is the honest answer there.
fn ge_version(tag: &str) -> Option<GeVersion> {
    let (major, rest) = tag.strip_prefix("GE-Proton")?.split_once('-')?;
    Some(GeVersion {
        major: major.parse().ok()?,
        // `GE-Proton11-7-x86_64` never appears in a `version` file, but a
        // directory name read in its place carries the suffix.
        minor: rest.split('-').next()?.parse().ok()?,
    })
}

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

/// Every Proton build installed across the user's Steam libraries.
///
/// Unsupported flavors are included; filter with [`ProtonFlavor::is_supported`]
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

/// Which Proton build a product is pinned to.
///
/// The Battle.net prefix carries its own pin, independent of any WoW product,
/// "latest" is not reliably best for Agent.exe and CEF.
///
/// Serialized as an optional string rather than a tagged enum, so TOML stays a
/// flat `default_proton = "GE-Proton9-20"` line instead of a nested table that
/// would have to be ordered against the `[[products]]` array.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(from = "Option<String>", into = "Option<String>")]
pub enum ProtonPin {
    /// Follow the app-wide default.
    #[default]
    Default,
    /// Stay on exactly this release until the user changes it.
    Pinned(String),
}

impl ProtonPin {
    /// The pinned version, or `None` when following the app default.
    pub fn version(&self) -> Option<&str> {
        match self {
            Self::Default => None,
            Self::Pinned(v) => Some(v),
        }
    }
}

impl From<Option<String>> for ProtonPin {
    fn from(value: Option<String>) -> Self {
        match value {
            Some(v) => Self::Pinned(v),
            None => Self::Default,
        }
    }
}

impl From<ProtonPin> for Option<String> {
    fn from(pin: ProtonPin) -> Self {
        match pin {
            ProtonPin::Default => None,
            ProtonPin::Pinned(v) => Some(v),
        }
    }
}

/// A newer upstream release, surfaced passively, never applied automatically.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpdateNotice {
    pub current: String,
    pub available: String,
    pub release_url: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A `.tar.gz` holding `files`, written into `dir`.
    fn tarball(dir: &Path, files: &[(&str, &[u8])]) -> PathBuf {
        let path = dir.join("release.tar.gz");
        let gz = flate2::write::GzEncoder::new(std::fs::File::create(&path).unwrap(), Default::default());
        let mut tar = tar::Builder::new(gz);
        for (name, bytes) in files {
            let mut header = tar::Header::new_gnu();
            header.set_size(bytes.len() as u64);
            header.set_mode(0o755);
            header.set_cksum();
            tar.append_data(&mut header, name, *bytes).unwrap();
        }
        tar.into_inner().unwrap().finish().unwrap();
        path
    }

    fn staging_left(dir: &Path) -> bool {
        std::fs::read_dir(dir)
            .unwrap()
            .filter_map(Result::ok)
            .any(|e| e.file_name().to_string_lossy().starts_with(crate::staging::PREFIX))
    }

    const BUILD: &[(&str, &[u8])] = &[
        ("GE-Proton11-7/proton", b"#!/usr/bin/env python3"),
        ("GE-Proton11-7/version", b"1760000000 GE-Proton11-7"),
        ("GE-Proton11-7/files/bin/wine", b"elf"),
    ];

    #[test]
    fn an_unpacked_build_lands_whole_and_leaves_no_staging() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("proton");
        let built = unpack(&tarball(tmp.path(), BUILD), &root).unwrap();

        assert_eq!(built, root.join("GE-Proton11-7"));
        assert!(built.join("files/bin/wine").is_file());
        assert!(!staging_left(&root));
    }

    #[test]
    fn a_truncated_download_leaves_nothing_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("proton");
        let archive = tarball(tmp.path(), BUILD);
        let bytes = std::fs::read(&archive).unwrap();
        std::fs::write(&archive, &bytes[..bytes.len() / 2]).unwrap();

        assert!(unpack(&archive, &root).is_err());
        assert!(!root.join("GE-Proton11-7").exists());
        assert!(!staging_left(&root));
        assert!(discover_managed(&root).is_empty());
    }

    #[test]
    fn a_tarball_that_is_not_a_proton_build_is_not_installed() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("proton");
        let archive = tarball(tmp.path(), &[("Something/readme", b"hi")]);

        assert!(matches!(unpack(&archive, &root), Err(ProtonError::NotAProtonBuild(_))));
        assert!(!root.join("Something").exists());
        assert!(!staging_left(&root));
    }

    #[test]
    fn unpacking_a_release_again_replaces_it_rather_than_merging() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path().join("proton");
        let stale = root.join("GE-Proton11-7/files/stale.dll");
        std::fs::create_dir_all(stale.parent().unwrap()).unwrap();
        std::fs::write(&stale, b"old").unwrap();

        let built = unpack(&tarball(tmp.path(), BUILD), &root).unwrap();
        assert!(built.join("proton").is_file());
        assert!(!stale.exists());
        assert!(!staging_left(&root));
    }

    #[test]
    fn classifies_real_world_tags() {
        // Tags observed on a live system.
        assert_eq!(
            ProtonFlavor::classify("GE-Proton11-6"),
            ProtonFlavor::GloriousEggroll
        );
        assert_eq!(
            ProtonFlavor::classify("GE-Proton10-34"),
            ProtonFlavor::GloriousEggroll
        );
        assert_eq!(
            ProtonFlavor::classify("UMU-Proton-10.0-4"),
            ProtonFlavor::Umu
        );
        assert_eq!(
            ProtonFlavor::classify("proton-11.0-2-x86_64"),
            ProtonFlavor::Valve
        );
        assert_eq!(
            ProtonFlavor::classify("experimental-11.0-20260910b-x86_64"),
            ProtonFlavor::Valve
        );
    }

    #[test]
    fn only_ge_is_a_launch_target() {
        assert!(ProtonFlavor::GloriousEggroll.is_supported());
        assert!(!ProtonFlavor::Umu.is_supported());
        assert!(!ProtonFlavor::Valve.is_supported());
        assert!(!ProtonFlavor::Other.is_supported());
    }

    /// A real build directory under `root`, complete enough for discovery.
    fn installed(root: &Path, version: &str) -> ProtonRelease {
        let dir = root.join(format!("{version}-x86_64"));
        std::fs::create_dir_all(dir.join("files")).unwrap();
        std::fs::write(dir.join("proton"), b"#!/usr/bin/env python3\n").unwrap();
        std::fs::write(dir.join("version"), format!("1787951532 {version}\n")).unwrap();
        std::fs::write(dir.join("files/blob"), vec![0u8; 4096]).unwrap();
        ProtonRelease::read_from_dir(&dir, ProtonSource::Managed).unwrap()
    }

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lightforge-proton-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn an_unused_managed_build_can_be_removed() {
        let root = scratch("remove-ok");
        let build = installed(&root, "GE-Proton11-7");
        assert!(build.path.is_dir());

        remove_build(&build, &root, &[]).unwrap();
        assert!(!build.path.exists());
    }

    #[test]
    fn a_build_something_is_pinned_to_is_refused_by_name() {
        let root = scratch("remove-pinned");
        let build = installed(&root, "GE-Proton11-7");

        let err = remove_build(&build, &root, &["the app default".into(), "WoW PTR".into()])
            .unwrap_err();
        // The message has to say what to unpin, or "no" is unactionable.
        let message = err.to_string();
        assert!(message.contains("the app default"), "{message}");
        assert!(message.contains("WoW PTR"), "{message}");
        assert!(build.path.is_dir(), "a refused removal must delete nothing");
    }

    #[test]
    fn a_build_we_did_not_install_is_never_deleted() {
        let root = scratch("remove-theirs");
        let elsewhere = scratch("remove-steam");
        let build = installed(&elsewhere, "GE-Proton11-7");

        // Right shape, wrong directory: this is Steam's, or hand-placed.
        let err = remove_build(&build, &root, &[]).unwrap_err();
        assert!(matches!(err, ProtonError::NotOurs(_)));
        assert!(build.path.is_dir());

        // And the same build claiming a non-managed source, even in our dir.
        let mut theirs = installed(&root, "GE-Proton11-6");
        theirs.source = ProtonSource::SystemCompatTools;
        assert!(matches!(
            remove_build(&theirs, &root, &[]).unwrap_err(),
            ProtonError::NotOurs(_)
        ));
        assert!(theirs.path.is_dir());
    }

    #[test]
    fn size_counts_the_whole_tree() {
        let root = scratch("size");
        let build = installed(&root, "GE-Proton11-7");
        // The 4 KiB blob plus the two small text files.
        assert!(size_on_disk(&build.path) > 4096);
        // A path that is not there is zero, not a panic.
        assert_eq!(size_on_disk(&root.join("nothing")), 0);
    }

    fn build(version: &str, built_at: Option<u64>, flavor: ProtonFlavor) -> ProtonRelease {
        ProtonRelease {
            version: version.to_owned(),
            flavor,
            source: ProtonSource::Managed,
            path: PathBuf::from("/tmp").join(version),
            built_at,
        }
    }

    #[test]
    fn without_the_tested_build_the_newest_supported_one_stands_in() {
        let builds = vec![
            build(
                "GE-Proton10-25",
                Some(1_787_951_532),
                ProtonFlavor::GloriousEggroll,
            ),
            build(
                "GE-Proton11-10",
                Some(1_780_000_000),
                ProtonFlavor::GloriousEggroll,
            ),
            build(
                "proton-11.0-2-x86_64",
                Some(1_799_999_999),
                ProtonFlavor::Valve,
            ),
        ];
        assert_eq!(select(&builds, None).unwrap().version, "GE-Proton10-25");
    }

    #[test]
    fn the_tested_build_beats_a_newer_one() {
        // The machine GE-Proton11-9 broke had both, and newest-wins took 11-9.
        let builds = vec![
            build(TESTED, Some(1_787_951_532), ProtonFlavor::GloriousEggroll),
            build("GE-Proton11-9", Some(1_791_936_695), ProtonFlavor::GloriousEggroll),
        ];
        assert_eq!(select(&builds, None).unwrap().version, TESTED);
        // A pin still means exactly what it says.
        assert_eq!(select(&builds, Some("GE-Proton11-9")).unwrap().version, "GE-Proton11-9");
    }

    #[test]
    fn the_tested_build_is_fetched_only_as_a_step_forward() {
        let have = |tags: &[&str]| -> Vec<ProtonRelease> {
            tags.iter().map(|t| build(t, None, ProtonFlavor::GloriousEggroll)).collect()
        };
        let old = have(&["GE-Proton11-5"]);
        assert!(wants_tested(&old, None, Some("GE-Proton11-5")));
        assert!(wants_tested(&old, None, None), "no prefix yet");
        assert!(!wants_tested(&old, Some("GE-Proton11-5"), None), "a pin is a choice");
        assert!(!wants_tested(&have(&["GE-Proton11-5", TESTED]), None, None), "already there");
        assert!(!wants_tested(&[], None, None), "setup installs the first build");
        assert!(!wants_tested(&have(&["GE-Proton11-11"]), None, None), "a newer build is kept");
        assert!(!wants_tested(&old, None, Some("GE-Proton11-9")), "would downgrade the prefix");
        let valve = [build("proton-11.0-2-x86_64", None, ProtonFlavor::Valve)];
        assert!(!wants_tested(&valve, None, None), "nothing supported installed");
    }

    #[test]
    fn a_pin_is_never_silently_substituted() {
        let builds = vec![build("GE-Proton11-7", None, ProtonFlavor::GloriousEggroll)];
        assert_eq!(
            select(&builds, Some("GE-Proton11-7")).unwrap().version,
            "GE-Proton11-7"
        );
        assert!(select(&builds, Some("GE-Proton9-20")).is_none());
    }

    #[test]
    fn version_file_splits_timestamp_from_tag() {
        assert_eq!(
            parse_version_file("1787951532 GE-Proton11-6\n"),
            Some((Some(1787951532), "GE-Proton11-6".to_owned()))
        );
    }

    #[test]
    fn version_file_tolerates_a_bare_tag() {
        assert_eq!(
            parse_version_file("GE-Proton9-20"),
            Some((None, "GE-Proton9-20".to_owned()))
        );
        assert_eq!(parse_version_file(""), None);
    }

    #[test]
    fn ge_tags_order_numerically_not_lexically() {
        // The whole reason this parse exists: as strings, 11-7 sorts below 9-20.
        assert!(ge_version("GE-Proton11-7") > ge_version("GE-Proton9-20"));
        assert!(ge_version("GE-Proton11-10") > ge_version("GE-Proton11-7"));
        assert_eq!(ge_version("GE-Proton11-7-x86_64"), ge_version("GE-Proton11-7"));
        assert_eq!(ge_version("proton-11.0-2-x86_64"), None);
        assert_eq!(ge_version("Proton - Experimental"), None);
    }

    #[test]
    fn a_prefix_reports_the_build_that_made_it() {
        let dir = scratch("prefix-build");
        assert_eq!(prefix_build(&dir), None);
        // What Proton actually writes there: the tag alone, no timestamp.
        std::fs::write(dir.join("version"), "GE-Proton11-7\n").unwrap();
        assert_eq!(prefix_build(&dir), Some("GE-Proton11-7".to_owned()));
    }

    #[test]
    fn only_an_older_build_counts_as_a_downgrade() {
        assert_eq!(prefix_fit("GE-Proton11-7", None), PrefixFit::Fresh);
        assert_eq!(
            prefix_fit("GE-Proton11-7", Some("GE-Proton11-7")),
            PrefixFit::Same
        );
        assert_eq!(
            prefix_fit("GE-Proton11-8", Some("GE-Proton11-7")),
            PrefixFit::Newer
        );
        // Across a major, where a string comparison would get it backwards.
        assert_eq!(
            prefix_fit("GE-Proton11-7", Some("GE-Proton9-20")),
            PrefixFit::Newer
        );
        let fit = prefix_fit("GE-Proton10-34", Some("GE-Proton11-7"));
        assert_eq!(
            fit,
            PrefixFit::Older {
                built_by: "GE-Proton11-7".to_owned()
            }
        );
        assert!(fit.is_downgrade());
    }

    #[test]
    fn an_unparseable_tag_is_never_warned_about() {
        // A prefix built by Valve's Proton, or by a hand-placed build: there
        // is no ordering between the two tags, so nothing is claimed.
        assert_eq!(
            prefix_fit("GE-Proton11-7", Some("proton-11.0-2")),
            PrefixFit::Unknown
        );
        assert!(!prefix_fit("GE-Proton11-7", Some("proton-11.0-2")).is_downgrade());
    }

    #[test]
    fn discovery_lists_builds_in_numeric_order() {
        let dir = scratch("discovery-order");
        for version in ["GE-Proton9-20", "GE-Proton11-7", "GE-Proton10-34"] {
            installed(&dir, version);
        }
        let versions: Vec<String> =
            discover_all(None, &dir).into_iter().map(|b| b.version).collect();
        assert_eq!(versions, ["GE-Proton9-20", "GE-Proton10-34", "GE-Proton11-7"]);
    }

    fn release(tag: &str, prerelease: bool) -> UpstreamRelease {
        UpstreamRelease {
            tag_name: tag.to_owned(),
            prerelease,
            html_url: format!("https://example.invalid/{tag}"),
            assets: vec![
                ReleaseAsset {
                    name: format!("{tag}.tar.gz"),
                    browser_download_url: format!("https://example.invalid/{tag}.tar.gz"),
                    size: 400_000_000,
                },
                ReleaseAsset {
                    name: format!("{tag}.sha512sum"),
                    browser_download_url: format!("https://example.invalid/{tag}.sha512sum"),
                    size: 150,
                },
            ],
        }
    }

    #[test]
    fn latest_stable_skips_prereleases() {
        // GitHub returns newest-first.
        let releases = vec![
            release("GE-Proton12-1", true),
            release("GE-Proton11-6", false),
        ];
        assert_eq!(latest_stable(&releases).unwrap().tag_name, "GE-Proton11-6");
    }

    #[test]
    fn update_notice_only_when_behind() {
        let releases = vec![release("GE-Proton11-6", false)];
        assert!(check_for_update("GE-Proton11-6", &releases).is_none());

        let notice = check_for_update("GE-Proton10-34", &releases).unwrap();
        assert_eq!(notice.current, "GE-Proton10-34");
        assert_eq!(notice.available, "GE-Proton11-6");
    }

    #[test]
    fn no_notice_when_upstream_has_only_prereleases() {
        let releases = vec![release("GE-Proton12-1", true)];
        assert!(check_for_update("GE-Proton11-6", &releases).is_none());
    }

    fn with_assets(tag: &str, names: &[&str]) -> UpstreamRelease {
        UpstreamRelease {
            tag_name: tag.to_owned(),
            prerelease: false,
            html_url: String::new(),
            assets: names
                .iter()
                .map(|n| ReleaseAsset {
                    name: (*n).to_owned(),
                    browser_download_url: format!("https://example.invalid/{n}"),
                    size: 0,
                })
                .collect(),
        }
    }

    #[test]
    fn dual_arch_release_does_not_hand_x86_the_arm_build() {
        // Real asset list for GE-Proton11-7; aarch64 sorts first.
        let r = with_assets(
            "GE-Proton11-7",
            &[
                "GE-Proton11-7-aarch64.sha512sum",
                "GE-Proton11-7-aarch64.tar.gz",
                "GE-Proton11-7-x86_64.sha512sum",
                "GE-Proton11-7-x86_64.tar.gz",
            ],
        );
        let tarball = r.tarball_for("x86_64").unwrap();
        assert_eq!(tarball.name, "GE-Proton11-7-x86_64.tar.gz");
        assert_eq!(
            r.checksum_for(tarball).unwrap().name,
            "GE-Proton11-7-x86_64.sha512sum"
        );
        assert_eq!(
            r.tarball_for("aarch64").unwrap().name,
            "GE-Proton11-7-aarch64.tar.gz"
        );
    }

    #[test]
    fn mixed_era_treats_the_unsuffixed_asset_as_x86() {
        // GE-Proton11-3 shipped aarch64 alongside an unsuffixed x86_64 build.
        let r = with_assets(
            "GE-Proton11-3",
            &[
                "GE-Proton11-3-aarch64.sha512sum",
                "GE-Proton11-3-aarch64.tar.gz",
                "GE-Proton11-3.sha512sum",
                "GE-Proton11-3.tar.gz",
            ],
        );
        let tarball = r.tarball_for("x86_64").unwrap();
        assert_eq!(tarball.name, "GE-Proton11-3.tar.gz");
        assert_eq!(
            r.checksum_for(tarball).unwrap().name,
            "GE-Proton11-3.sha512sum"
        );
    }

    #[test]
    fn legacy_single_asset_release_resolves() {
        let r = with_assets(
            "GE-Proton10-34",
            &["GE-Proton10-34.sha512sum", "GE-Proton10-34.tar.gz"],
        );
        let tarball = r.tarball_for("x86_64").unwrap();
        assert_eq!(tarball.name, "GE-Proton10-34.tar.gz");
        assert_eq!(
            r.checksum_for(tarball).unwrap().name,
            "GE-Proton10-34.sha512sum"
        );
        // No ARM build in this era.
        assert!(r.tarball_for("aarch64").is_none());
    }

    #[test]
    fn sha512sum_line_yields_the_digest() {
        let digest = "a".repeat(128);
        assert_eq!(
            parse_sha512sum(&format!("{digest}  GE-Proton11-6.tar.gz\n")),
            Some(digest.clone())
        );
        // Uppercase digests normalize.
        assert_eq!(
            parse_sha512sum(&format!("{}  f", "A".repeat(128))),
            Some("a".repeat(128))
        );
    }

    #[test]
    fn malformed_checksums_are_rejected() {
        assert_eq!(parse_sha512sum(""), None);
        assert_eq!(parse_sha512sum("deadbeef  short.tar.gz"), None);
        assert_eq!(parse_sha512sum(&format!("{}  f", "z".repeat(128))), None);
    }

    #[test]
    fn pin_round_trips_through_an_optional_string() {
        assert_eq!(ProtonPin::from(None), ProtonPin::Default);
        assert_eq!(ProtonPin::Default.version(), None);
        assert_eq!(
            ProtonPin::from(Some("GE-Proton11-6".to_owned())).version(),
            Some("GE-Proton11-6")
        );
    }
}

// ---------------------------------------------------------------------------
// Upstream releases (async edge)
// ---------------------------------------------------------------------------

/// GitHub Releases endpoint for Proton-GE.
const GE_RELEASES_URL: &str =
    "https://api.github.com/repos/GloriousEggroll/proton-ge-custom/releases";

/// One release as published upstream.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UpstreamRelease {
    /// Release tag, e.g. `GE-Proton11-6`. Matches the `version` file of an
    /// installed build, which is what makes the two comparable.
    pub tag_name: String,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub html_url: String,
    #[serde(default)]
    pub assets: Vec<ReleaseAsset>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReleaseAsset {
    pub name: String,
    pub browser_download_url: String,
    #[serde(default)]
    pub size: u64,
}

/// Architecture suffix used by GE's release assets.
const AARCH64: &str = "-aarch64";

impl UpstreamRelease {
    /// The `.tar.gz` build artifact for `arch`.
    ///
    /// GE's asset naming has three eras, all still present in the release list:
    ///
    /// | era | assets |
    /// |---|---|
    /// | ≤ `GE-Proton11-2` | `GE-ProtonX-Y.tar.gz`: unsuffixed, x86_64 |
    /// | `GE-Proton11-1`, `11-3` | unsuffixed x86_64 **plus** `-aarch64` |
    /// | ≥ `GE-Proton11-4` | explicit `-x86_64` **and** `-aarch64` |
    ///
    /// So an x86_64 host takes an explicit `-x86_64` when offered and otherwise
    /// any tarball that isn't `-aarch64`; picking the first `.tar.gz` would
    /// select the ARM build, since `aarch64` sorts first.
    pub fn tarball_for(&self, arch: &str) -> Option<&ReleaseAsset> {
        let suffix = format!("-{arch}.tar.gz");
        if let Some(exact) = self.assets.iter().find(|a| a.name.ends_with(&suffix)) {
            return Some(exact);
        }
        // Only x86_64 has an unsuffixed spelling; ARM is always explicit.
        if arch != "x86_64" {
            return None;
        }
        self.assets
            .iter()
            .find(|a| a.name.ends_with(".tar.gz") && !a.name.contains(AARCH64))
    }

    /// The build artifact for the architecture Lightforge is running on.
    pub fn tarball(&self) -> Option<&ReleaseAsset> {
        self.tarball_for(std::env::consts::ARCH)
    }

    /// The checksum published alongside a specific tarball.
    ///
    /// Derived from the tarball's own name rather than searched for, so a
    /// dual-arch release can never pair an x86_64 download with the ARM digest.
    pub fn checksum_for(&self, tarball: &ReleaseAsset) -> Option<&ReleaseAsset> {
        let expected = tarball.name.replace(".tar.gz", ".sha512sum");
        self.assets.iter().find(|a| a.name == expected)
    }
}

/// Fetch the Proton-GE release list, revalidating with `cache`.
///
/// Unauthenticated GitHub API calls are capped at 60/hour per IP, so the cached
/// copy is returned on `304` and the budget only moves when something actually
/// changed upstream.
pub async fn fetch_releases(
    http: &crate::net::Http,
    cache: &crate::net::ETagCache,
) -> Result<Vec<UpstreamRelease>, ProtonError> {
    match http.get_json_cached(GE_RELEASES_URL, cache).await? {
        Some(releases) => Ok(releases),
        None => Ok(cache.read_json()?),
    }
}

/// The newest stable release, or `None` if upstream published only prereleases.
pub fn latest_stable(releases: &[UpstreamRelease]) -> Option<&UpstreamRelease> {
    // GitHub returns newest-first, so the first stable entry is the latest.
    releases.iter().find(|r| !r.prerelease)
}

/// Compare an installed version against upstream, producing a passive notice.
///
/// Returns `None` when already current. Nothing here applies an update, the
/// policy is pinned + notify-only.
pub fn check_for_update(installed: &str, releases: &[UpstreamRelease]) -> Option<UpdateNotice> {
    let latest = latest_stable(releases)?;
    (latest.tag_name != installed).then(|| UpdateNotice {
        current: installed.to_owned(),
        available: latest.tag_name.clone(),
        release_url: latest.html_url.clone(),
    })
}

/// Delete a Proton build Lightforge installed.
///
/// Refuses builds outside `proton_dir` (Steam's or hand-placed) and builds
/// something is pinned to, naming the pins. It does not unpin them itself: a
/// pin is the user's decision about how a game launches, and freeing disk
/// space should not quietly rewrite it.
pub fn remove_build(
    build: &ProtonRelease,
    proton_dir: &Path,
    pinned_by: &[String],
) -> Result<(), ProtonError> {
    if build.source != ProtonSource::Managed || !build.path.starts_with(proton_dir) {
        return Err(ProtonError::NotOurs(build.path.clone()));
    }
    if !pinned_by.is_empty() {
        return Err(ProtonError::StillPinned {
            version: build.version.clone(),
            users: pinned_by.join(", "),
        });
    }

    std::fs::remove_dir_all(&build.path).map_err(|source| ProtonError::Io {
        path: build.path.clone(),
        source,
    })
}

/// How much disk a build occupies, following no symlinks.
///
/// Best effort: an unreadable entry contributes nothing rather than failing
/// the total, since this only ever labels a button.
pub fn size_on_disk(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(kind) if kind.is_dir() => size_on_disk(&entry.path()),
            Ok(kind) if kind.is_file() => entry.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

/// Download, verify, and unpack a release into `dest_root`.
///
/// The tarball unpacks to a single top-level directory named for the release,
/// so the installed build lands at `dest_root/<dir>`. The download is verified
/// against the published SHA-512 before anything is extracted, a truncated
/// Proton fails much later and far more confusingly than a checksum mismatch.
pub async fn install_release(
    http: &crate::net::Http,
    release: &UpstreamRelease,
    dest_root: &Path,
    scratch: &Path,
    on_progress: impl FnMut(u64, Option<u64>),
) -> Result<ProtonRelease, ProtonError> {
    let tarball = release
        .tarball()
        .ok_or_else(|| ProtonError::NoTarball(release.tag_name.clone()))?;

    let archive = scratch.join(&tarball.name);
    let digest = http
        .download(&tarball.browser_download_url, &archive, on_progress)
        .await?;

    // Whatever happens from here, the tarball goes: it is ~400 MB, and a
    // failed checksum fetch, a missing checksum or a bad unpack used to leave
    // it in the cache for good.
    let installed: Result<ProtonRelease, ProtonError> = async {
        // GE publishes a checksum file for every release; treat its absence
        // as a reason to stop rather than a reason to trust the download.
        let expected = match release.checksum_for(tarball) {
            Some(asset) => parse_sha512sum(&http.get_text(&asset.browser_download_url).await?)
                .ok_or_else(|| ProtonError::MalformedChecksum(asset.name.clone()))?,
            None => return Err(ProtonError::NoChecksum(release.tag_name.clone())),
        };

        if !digest.eq_ignore_ascii_case(&expected) {
            return Err(ProtonError::ChecksumMismatch {
                version: release.tag_name.clone(),
                expected,
                actual: digest,
            });
        }

        let dest_root = dest_root.to_path_buf();
        let archive_for_task = archive.clone();
        // Decompression is CPU-bound and blocking; keep it off the async runtime.
        let unpacked = tokio::task::spawn_blocking(move || unpack(&archive_for_task, &dest_root))
            .await
            .map_err(|e| ProtonError::Join(e.to_string()))??;

        ProtonRelease::read_from_dir(&unpacked, ProtonSource::Managed)
            .ok_or(ProtonError::NotAProtonBuild(unpacked))
    }
    .await;
    let _ = std::fs::remove_file(&archive);
    installed
}

/// Extract a `.tar.gz` into `dest_root`, returning the build's directory.
///
/// Unpacked into a staging folder inside `dest_root` and renamed into place
/// only once it is complete and reads as a Proton build. An interrupted or
/// failed unpack used to leave a half-extracted GE-Proton folder that
/// discovery listed and a launch then failed on; now it leaves nothing.
fn unpack(archive: &Path, dest_root: &Path) -> Result<PathBuf, ProtonError> {
    let io = |path: &Path| {
        let path = path.to_path_buf();
        move |source| ProtonError::Io { path, source }
    };
    let file = std::fs::File::open(archive).map_err(io(archive))?;
    std::fs::create_dir_all(dest_root).map_err(io(dest_root))?;

    let staging = crate::staging::create_in(dest_root).map_err(io(dest_root))?;
    let fresh = staging.path().join("new");
    let old = staging.path().join("old");
    std::fs::create_dir(&fresh).map_err(io(&fresh))?;

    let mut tar = tar::Archive::new(flate2::read::GzDecoder::new(file));
    for entry in tar.entries().map_err(io(archive))? {
        let mut entry = entry.map_err(io(archive))?;
        let path = entry.path().map_err(io(archive))?.into_owned();

        // Reject absolute paths and `..` traversal rather than trusting the
        // archive to stay inside its own directory.
        if path.is_absolute()
            || path
                .components()
                .any(|c| c == std::path::Component::ParentDir)
        {
            return Err(ProtonError::UnsafeArchivePath(path));
        }
        entry.unpack_in(&fresh).map_err(io(&fresh))?;
    }

    // A release is one directory named for it. Anything else is not a
    // GE-Proton tarball, and guessing which part is the build is how a stray
    // folder would end up beside the real ones.
    let mut tops = std::fs::read_dir(&fresh)
        .map_err(io(&fresh))?
        .filter_map(Result::ok)
        .map(|e| e.path());
    let (Some(built), None) = (tops.next(), tops.next()) else {
        return Err(match std::fs::read_dir(&fresh).map(|mut d| d.next().is_none()) {
            Ok(true) => ProtonError::EmptyArchive(archive.to_path_buf()),
            _ => ProtonError::NotAProtonBuild(archive.to_path_buf()),
        });
    };
    if ProtonRelease::read_from_dir(&built, ProtonSource::Managed).is_none() {
        return Err(ProtonError::NotAProtonBuild(archive.to_path_buf()));
    }

    let target = dest_root.join(built.file_name().unwrap_or_default());
    // The same release again, a repair: the old copy only goes once the new
    // one is ready to take its place, and comes back if that fails.
    let replacing = target.exists();
    if replacing {
        std::fs::rename(&target, &old).map_err(io(&target))?;
    }
    if let Err(source) = std::fs::rename(&built, &target) {
        if replacing {
            let _ = std::fs::rename(&old, &target);
        }
        return Err(ProtonError::Io { path: target, source });
    }
    Ok(target)
}

/// Pull the digest out of a `sha512sum` line: `<hex>  <filename>`.
fn parse_sha512sum(contents: &str) -> Option<String> {
    let digest = contents.split_whitespace().next()?;
    (digest.len() == 128 && digest.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| digest.to_ascii_lowercase())
}

#[derive(Debug, thiserror::Error)]
pub enum ProtonError {
    #[error("{0} is not a build Lightforge installed")]
    NotOurs(PathBuf),

    #[error("{version} is still pinned by {users}")]
    StillPinned { version: String, users: String },

    #[error(transparent)]
    Net(#[from] crate::net::NetError),

    #[error("release {0} publishes no .tar.gz asset for {arch}", arch = std::env::consts::ARCH)]
    NoTarball(String),

    #[error("release {0} publishes no checksum; refusing to install unverified")]
    NoChecksum(String),

    #[error("checksum file {0} is malformed")]
    MalformedChecksum(String),

    #[error("checksum mismatch for {version}: expected {expected}, got {actual}")]
    ChecksumMismatch {
        version: String,
        expected: String,
        actual: String,
    },

    #[error("archive contains an unsafe path: {0}")]
    UnsafeArchivePath(PathBuf),

    #[error("archive {0} is empty")]
    EmptyArchive(PathBuf),

    #[error("{0} is not a Proton build")]
    NotAProtonBuild(PathBuf),

    #[error("extraction task failed: {0}")]
    Join(String),

    #[error("i/o error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}
