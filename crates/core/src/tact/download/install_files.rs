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

/// Write the install manifest's files (`Wow.exe`, `wow_loader.dll`, CEF) as
/// plain files into `dest`, the flavor directory (`<install>/_retail_`),
/// each verified against its content key.
pub async fn extract_install(
    source: Source<'_>,
    entries: &[&crate::tact::manifest::InstallEntry],
    catalog: Catalog<'_>,
    dest: &std::path::Path,
    cancel: &Cancel,
    mut progress: impl FnMut(&Extracted),
) -> Result<InstallReport, DownloadError> {
    let Source { http, client } = source;
    let Catalog { encoding, locations, loose } = catalog;
    let mut report = InstallReport::default();

    for entry in entries {
        if cancel.is_cancelled() {
            break;
        }

        let fail = |report: &mut InstallReport, reason: String| {
            report.failed.push((entry.path.clone(), reason));
        };

        let Some(ekey) = encoding.ekey_for(&entry.ckey) else {
            fail(&mut report, format!("encoding does not list content key {}", hex(&entry.ckey)));
            continue;
        };

        // As the plan does: archived keys by range request, loose ones whole.
        let encoded = if let Some((archive, located)) = locations.get(&ekey) {
            let Some(range) = located.range() else {
                fail(&mut report, format!("encoding key {} has size 0 in archive {archive}", hex(&ekey)));
                continue;
            };
            client.fetch_range(http, archive, &range).await
        } else if loose.contains(&ekey) {
            client.fetch(http, crate::tact::cdn::Kind::Data, &hex(&ekey)).await
        } else {
            fail(&mut report, format!("encoding key {} is in no index", hex(&ekey)));
            continue;
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

        let Some(relative) = entry.relative_path() else {
            fail(&mut report, "the install manifest names a path outside the install".into());
            continue;
        };
        let path = resolve_windows_path(dest, &relative);
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
