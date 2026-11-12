use super::{Cancel, DownloadError, Source};
use crate::text::hex;

/// The three lookups that turn a content key into a fetch: what it encodes
/// to, which archive holds it, and whether it is loose instead.
#[derive(Debug, Clone, Copy)]
pub struct Catalog<'a> {
    pub encoding: &'a crate::tact::encoding::Encoding,
    pub locations: &'a crate::tact::indices::Locations,
    pub loose: &'a crate::tact::archive::FileIndex,
}

/// One install-manifest file, written to disk.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Extracted {
    pub path: std::path::PathBuf,
    pub bytes: u64,
}

/// What extracting the install-manifest files achieved.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct InstallReport {
    pub written: Vec<Extracted>,
    /// Already on disk with the right content, so left as they were.
    pub unchanged: usize,
    pub failed: Vec<(String, String)>,
}

impl InstallReport {
    pub fn bytes(&self) -> u64 {
        self.written.iter().map(|e| e.bytes).sum()
    }

    pub fn is_complete(&self) -> bool {
        self.failed.is_empty()
    }
}

/// An install-manifest file and where the CDN has its bytes, worked out
/// before the download: the encoding table and the indices it takes are
/// hundreds of MB, not to be held through an hours-long run.
pub struct InstallFile {
    entry: crate::tact::manifest::InstallEntry,
    /// Why it cannot be fetched, when it cannot.
    from: Result<OnCdn, String>,
}

enum OnCdn {
    /// Range-requested out of an archive, as the plan does.
    Archive { archive: String, range: String },
    /// Fetched whole.
    Loose(crate::tact::storage::EKey),
}

/// Where each of `entries` comes from, by `catalog`.
pub fn locate_install(entries: &[&crate::tact::manifest::InstallEntry], catalog: Catalog<'_>) -> Vec<InstallFile> {
    let Catalog { encoding, locations, loose } = catalog;
    let locate = |entry: &crate::tact::manifest::InstallEntry| {
        let ekey = encoding
            .ekey_for(&entry.ckey)
            .ok_or_else(|| format!("encoding does not list content key {}", hex(&entry.ckey)))?;
        if let Some((archive, located)) = locations.get(&ekey) {
            let range = located
                .range()
                .ok_or_else(|| format!("encoding key {} has size 0 in archive {archive}", hex(&ekey)))?;
            return Ok(OnCdn::Archive { archive: archive.to_owned(), range });
        }
        match loose.contains(&ekey) {
            true => Ok(OnCdn::Loose(ekey)),
            false => Err(format!("encoding key {} is in no index", hex(&ekey))),
        }
    };
    entries.iter().map(|entry| InstallFile { entry: (*entry).clone(), from: locate(entry) }).collect()
}

/// Write the install manifest's files (`Wow.exe`, `wow_loader.dll`, CEF) as
/// plain files into `dest`, the flavor directory (`<install>/_retail_`),
/// each verified against its content key.
pub async fn extract_install(
    source: Source<'_>,
    files: &[InstallFile],
    dest: &std::path::Path,
    cancel: &Cancel,
    mut progress: impl FnMut(&Extracted),
) -> Result<InstallReport, DownloadError> {
    let Source { http, client } = source;
    let mut report = InstallReport::default();

    for InstallFile { entry, from } in files {
        if cancel.is_cancelled() {
            break;
        }

        let fail = |report: &mut InstallReport, reason: String| {
            report.failed.push((entry.path.clone(), reason));
        };

        let Some(relative) = entry.relative_path() else {
            fail(&mut report, "the install manifest names a path outside the install".into());
            continue;
        };
        let path = resolve_windows_path(dest, &relative);
        // Otherwise every repair and update fetches them all again, one at a
        // time: 275 files, 340 MB, three minutes for wowf (2026-11-10).
        if std::fs::read(&path).is_ok_and(|on_disk| *md5::compute(&on_disk) == entry.ckey) {
            report.unchanged += 1;
            continue;
        }

        let encoded = match from {
            Ok(OnCdn::Archive { archive, range }) => client.fetch_range(http, archive, range).await,
            Ok(OnCdn::Loose(ekey)) => client.fetch(http, crate::tact::cdn::Kind::Data, &hex(ekey)).await,
            Err(reason) => {
                fail(&mut report, reason.clone());
                continue;
            }
        };

        let encoded = match encoded {
            Ok(bytes) => bytes,
            Err(e) => {
                fail(&mut report, e.to_string());
                continue;
            }
        };

        let decoded = match crate::tact::blte::decode(&encoded) {
            Ok(bytes) => bytes,
            Err(e) => {
                fail(&mut report, e.to_string());
                continue;
            }
        };

        // Never written unverified: these are executables.
        let actual = *md5::compute(&decoded);
        if actual != entry.ckey {
            fail(
                &mut report,
                format!("decoded to {} but the build says {}", hex(&actual), hex(&entry.ckey)),
            );
            continue;
        }

        match crate::fsutil::write_atomic_with_mode(&path, &decoded, 0o644) {
            Ok(()) => {
                let written = Extracted { path, bytes: decoded.len() as u64 };
                progress(&written);
                report.written.push(written);
            }
            Err(e) => fail(&mut report, e.to_string()),
        }
    }

    Ok(report)
}

/// The `.flavor.info` line a client reads to know which product it is.
const FLAVOR_HEADER: &str = "Product Flavor!STRING:0";

/// Write `<flavor dir>/.flavor.info`. Agent writes it, the install manifest
/// does not list it, and the client finds its `.build.info` row by it. From
/// a real `_classic_beta_`:
///
/// ```text
/// Product Flavor!STRING:0
/// wow_classic_beta
/// ```
pub fn write_flavor_info(
    flavor_dir: &std::path::Path,
    product: &crate::ProductCode,
) -> std::io::Result<std::path::PathBuf> {
    let path = flavor_dir.join(".flavor.info");
    let text = format!("{FLAVOR_HEADER}\n{}\n", product.as_str());
    crate::fsutil::write_atomic_with_mode(&path, text.as_bytes(), 0o644)?;
    Ok(path)
}

/// Join `relative` onto `root`, reusing directories that differ only by case.
///
/// The manifest mixes `Utils\cef.pak` with `UTILS\LOCALES\EN-US.PAK`; on Linux
/// that makes two directories, and Wine missed `LIBCEF.DLL`. The first
/// spelling decides, as on Windows. File-name case is kept.
pub(super) fn resolve_windows_path(root: &std::path::Path, relative: &std::path::Path) -> std::path::PathBuf {
    let mut at = root.to_path_buf();

    for component in relative.components() {
        let name = component.as_os_str();
        let candidate = at.join(name);

        // An exact match needs no directory scan.
        if candidate.exists() {
            at = candidate;
            continue;
        }

        let existing = std::fs::read_dir(&at).ok().and_then(|entries| {
            entries.flatten().map(|e| e.file_name()).find(|existing| {
                existing.as_encoded_bytes().eq_ignore_ascii_case(name.as_encoded_bytes())
            })
        });

        at = match existing {
            Some(existing) => at.join(existing),
            None => candidate,
        };
    }

    at
}
