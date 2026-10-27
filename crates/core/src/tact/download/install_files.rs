use super::{Cancel, DownloadError, Source};
use crate::tact::config::to_hex;

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

/// Write the install manifest's files into the flavor directory.
///
/// These exist as real files, not CASC content (`Wow.exe`, `wow_loader.dll`,
/// CEF and its `.pak`s), and are not written into the store: only 12 of the
/// 36 are in Agent's store, because the download manifest lists them too.
/// `dest` is the flavor directory (`<install>/_retail_`). Each file is
/// verified against its content key and written through a temporary file.
pub async fn extract_install(
    source: Source<'_>,
    entries: &[&crate::tact::manifest::InstallEntry],
    catalog: Catalog<'_>,
    dest: &std::path::Path,
    cancel: &Cancel,
    mut progress: impl FnMut(&Extracted),
) -> Result<InstallReport, DownloadError> {
    let Source { http, client, keys } = source;
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
            fail(&mut report, format!("encoding does not list content key {}", to_hex(&entry.ckey)));
            continue;
        };

        // The same dispatch the plan makes: archived keys come out of a range
        // request, loose ones whole.
        let encoded = if let Some((archive, located)) = locations.get(&ekey) {
            let Some(range) = located.range() else {
                fail(&mut report, format!("encoding key {} has size 0 in archive {archive}", to_hex(&ekey)));
                continue;
            };
            client.fetch_range(http, archive, &range).await
        } else if loose.contains(&ekey) {
            client.fetch(http, crate::tact::cdn::Kind::Data, &to_hex(&ekey)).await
        } else {
            fail(&mut report, format!("encoding key {} is in no index", to_hex(&ekey)));
            continue;
        };

        let encoded = match encoded {
            Ok(bytes) => bytes,
            Err(e) => {
                fail(&mut report, e.to_string());
                continue;
            }
        };

        let decoded = match blte::decompress_blte(encoded, keys) {
            Ok(bytes) => bytes,
            Err(e) => {
                fail(&mut report, e.to_string());
                continue;
            }
        };

        // Unlike store content, an install file is never stored unverified:
        // these are executables, and one that does not match its content key
        // is not something to write to disk and hope.
        let actual = *md5::compute(&decoded);
        if actual != entry.ckey {
            fail(
                &mut report,
                format!("decoded to {} but the build says {}", to_hex(&actual), to_hex(&entry.ckey)),
            );
            continue;
        }

        let Some(relative) = entry.relative_path() else {
            fail(&mut report, "the install manifest names a path outside the install".into());
            continue;
        };
        let path = resolve_windows_path(dest, &relative);
        match write_file(&path, &decoded) {
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

/// Write `<flavor dir>/.flavor.info`, which nothing else creates.
///
/// Agent writes this, the install manifest does not list it, and the client
/// uses it to work out which `.build.info` row belongs to it. A flavor
/// directory without one is not an install any Blizzard tool produces.
///
/// Verbatim shape, from a real install's `_classic_beta_`:
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
    write_file(&path, format!("{FLAVOR_HEADER}\n{}\n", product.as_str()).as_bytes())?;
    Ok(path)
}

/// Join `relative` onto `root`, reusing directories that differ only by case.
///
/// The install manifest mixes `Utils\cef.pak` with `UTILS\LOCALES\EN-US.PAK`;
/// Windows lands both in one directory, Linux would make two, and a real
/// install then had `LIBCEF.DLL` in `UTILS/` where Wine could miss it. So a
/// component reuses an existing case-insensitive match and otherwise keeps the
/// manifest's spelling (first writer decides, as on Windows). File-name case
/// is kept either way.
pub(super) fn resolve_windows_path(root: &std::path::Path, relative: &std::path::Path) -> std::path::PathBuf {
    let mut at = root.to_path_buf();

    for component in relative.components() {
        let name = component.as_os_str();
        let candidate = at.join(name);

        // An exact match wins without a directory scan, which is the common
        // case and keeps this off the hot path for deep trees.
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

/// Write a file and its parent directories, via a temporary file alongside.
pub(super) fn write_file(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;

    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let dir = path.parent().unwrap_or(std::path::Path::new("."));
    let mut temp = tempfile::NamedTempFile::new_in(dir)?;
    temp.write_all(bytes)?;
    temp.as_file().sync_all()?;

    // tempfile creates at 0600; these are files Wine and the client read, and
    // a launcher that silently tightens permissions on Wow.exe is a bug.
    temp.persist(path).map_err(|e| e.error)?;
    std::fs::set_permissions(path, std::os::unix::fs::PermissionsExt::from_mode(0o644))
}
