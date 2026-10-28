use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{ProtonError, ProtonFlavor, ProtonRelease, ProtonSource, UpdateNotice};

/// A line of Proton builds Lightforge can download.
///
/// Valve's Proton is only distributed through Steam and TKG publishes no
/// release builds, so neither is a family here; the installer window says so.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ProtonFamily {
    GloriousEggroll,
    CachyOs,
}

impl ProtonFamily {
    pub const ALL: [Self; 2] = [Self::GloriousEggroll, Self::CachyOs];

    /// The family a release tag belongs to, if one can be downloaded.
    pub fn of_tag(tag: &str) -> Option<Self> {
        match ProtonFlavor::classify(tag) {
            ProtonFlavor::GloriousEggroll => Some(Self::GloriousEggroll),
            ProtonFlavor::CachyOs => Some(Self::CachyOs),
            _ => None,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::GloriousEggroll => "Proton-GE",
            Self::CachyOs => "Proton-CachyOS",
        }
    }

    fn releases_url(self) -> &'static str {
        match self {
            Self::GloriousEggroll => GE_RELEASES_URL,
            Self::CachyOs => CACHYOS_RELEASES_URL,
        }
    }

    /// The file the release list is cached in, beside the other asset caches.
    pub fn cache_file(self) -> &'static str {
        match self {
            Self::GloriousEggroll => "proton-releases.json",
            Self::CachyOs => "proton-cachyos-releases.json",
        }
    }
}

/// GitHub Releases endpoint for Proton-GE.
const GE_RELEASES_URL: &str =
    "https://api.github.com/repos/GloriousEggroll/proton-ge-custom/releases";

/// And for Proton-CachyOS, whose assets are `.tar.xz`.
const CACHYOS_RELEASES_URL: &str = "https://api.github.com/repos/CachyOS/proton-cachyos/releases";

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

/// Archive formats a release can use, GE's first.
const ARCHIVE_EXTENSIONS: [&str; 2] = [".tar.gz", ".tar.xz"];

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
        // CachyOS calls aarch64 `arm64`, and compresses with xz.
        let names: &[&str] = match arch {
            "aarch64" => &["aarch64", "arm64"],
            "x86_64" => &["x86_64"],
            other => &[other],
        };
        for name in names {
            for ext in ARCHIVE_EXTENSIONS {
                let suffix = format!("-{name}{ext}");
                if let Some(exact) = self.assets.iter().find(|a| a.name.ends_with(&suffix)) {
                    return Some(exact);
                }
            }
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
        let stem = ARCHIVE_EXTENSIONS
            .iter()
            .find_map(|ext| tarball.name.strip_suffix(ext))?;
        let expected = format!("{stem}.sha512sum");
        self.assets.iter().find(|a| a.name == expected)
    }
}

/// Fetch a family's release list, revalidating with `cache`.
///
/// Unauthenticated GitHub API calls are capped at 60/hour per IP, so the cached
/// copy is returned on `304` and the budget only moves when something actually
/// changed upstream.
pub async fn fetch_releases(
    http: &crate::net::Http,
    cache: &crate::net::ETagCache,
    family: ProtonFamily,
) -> Result<Vec<UpstreamRelease>, ProtonError> {
    match http.get_json_cached(family.releases_url(), cache).await? {
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
    crate::fsutil::dir_size(path)
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
    tracing::info!(
        "installing {} from {} into {}",
        release.tag_name,
        tarball.browser_download_url,
        dest_root.display()
    );
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
    match &installed {
        Ok(build) => tracing::info!("installed {} at {}", build.version, build.path.display()),
        Err(e) => tracing::error!("installing {} failed: {}", release.tag_name, crate::error::chain(e)),
    }
    installed
}

/// Extract a `.tar.gz` or `.tar.xz` into `dest_root`, returning the build's directory.
///
/// Unpacked into a staging folder inside `dest_root` and renamed into place
/// only once it is complete and reads as a Proton build. An interrupted or
/// failed unpack used to leave a half-extracted GE-Proton folder that
/// discovery listed and a launch then failed on; now it leaves nothing.
pub(super) fn unpack(archive: &Path, dest_root: &Path) -> Result<PathBuf, ProtonError> {
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

    let decoder: Box<dyn std::io::Read> = match archive.extension().and_then(|e| e.to_str()) {
        Some("xz") => Box::new(liblzma::read::XzDecoder::new(file)),
        _ => Box::new(flate2::read::GzDecoder::new(file)),
    };
    let mut tar = tar::Archive::new(decoder);
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
pub(super) fn parse_sha512sum(contents: &str) -> Option<String> {
    let digest = contents.split_whitespace().next()?;
    (digest.len() == 128 && digest.chars().all(|c| c.is_ascii_hexdigit()))
        .then(|| digest.to_ascii_lowercase())
}
