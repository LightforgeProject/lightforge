//! Moving one game to another library without downloading it again.
//!
//! Every library has its own WoW install, whose store the games in it share,
//! so a game moves as data: the store entries its build needs, its configs,
//! its flavour folder and its `.build.info` row. In steps that each leave the
//! game launchable from somewhere:
//!
//! 1. [`copy`] fills the target and reads back every copied entry, then writes
//!    the game's row there, which is the commit. Cancelled or failed before
//!    that, the source is untouched and the copies are unused data for the
//!    next prune.
//! 2. The caller points the config at the target library. Only then may the
//!    source go: a crash in between leaves the game in two places, not none.
//! 3. [`release`] removes the game from the source as an uninstall does, and
//!    tells Battle.net and the prefix where it now is.
//!
//! A game alone in its install, going to a library without one, on the same
//! filesystem, is the whole folder renamed instead.

use std::collections::HashSet;
use std::os::unix::fs::MetadataExt;
use std::path::{Path, PathBuf};

use crate::buildinfo::{BuildInfo, BuildInfoError, COL_BUILD_KEY, COL_CDN_KEY};
use crate::library::Library;
use crate::net::Http;
use crate::paths::Layout;
use crate::product::ProductCode;
use crate::tact::download::Cancel;
use crate::tact::entry::Header;
use crate::tact::idx::{Entry, CASC_ENTRY_HEADER};
use crate::tact::productdb::{self, ProductDbError, ProductInstall};
use crate::tact::reclaim::{self, Build, ReclaimError};
use crate::tact::storage::{resolve_data_dir, Storage, StorageError};
use crate::tact::uninstall::{self, UninstallError};
use crate::tact::writer::{StoreLock, StoreWriter, WriteError};
use crate::tact::{config as tact_config, verify};

#[derive(Debug, thiserror::Error)]
pub enum MoveError {
    #[error("close {0} first; the game files are in use")]
    Busy(String),
    #[error("{0} is not installed where the config says")]
    NotInstalled(String),
    #[error("{library} already holds {product}")]
    AlreadyThere { product: String, library: String },
    #[error("{0} is not mounted")]
    Offline(String),
    #[error("{0} of the game's files are damaged where it is now; repair it there first")]
    Damaged(usize),
    #[error("{0} copied files did not read back intact; the game was not moved")]
    CopyFailed(usize),
    #[error("cancelled")]
    Cancelled,
    #[error(transparent)]
    BuildInfo(#[from] BuildInfoError),
    #[error(transparent)]
    Reclaim(#[from] ReclaimError),
    #[error(transparent)]
    Storage(#[from] StorageError),
    #[error(transparent)]
    Write(#[from] WriteError),
    #[error(transparent)]
    ProductDb(#[from] ProductDbError),
    #[error(transparent)]
    Uninstall(#[from] UninstallError),
    #[error(transparent)]
    Prefix(#[from] crate::prefix::PrefixError),
    #[error("i/o error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> MoveError + '_ {
    move |source| MoveError::Io { path: path.to_path_buf(), source }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Progress {
    pub bytes_done: u64,
    pub bytes_total: u64,
    /// Reading the copies back, after the last one was written.
    pub verifying: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Moved {
    /// The whole install folder, renamed.
    Renamed,
    /// Store entries copied; the source still holds the game until [`release`].
    Copied { files: usize, bytes: u64 },
}

/// Step 1: put `code` into `to`, checked, and commit it there.
pub async fn copy(
    http: &Http,
    layout: &Layout,
    code: &ProductCode,
    to: &Library,
    cancel: &Cancel,
    mut progress: impl FnMut(Progress),
) -> Result<Moved, MoveError> {
    if let Some(process) = crate::preflight::scan(layout).first() {
        return Err(MoveError::Busy(process.name.clone()));
    }
    if !to.state().is_present() {
        return Err(MoveError::Offline(to.label.clone()));
    }
    let from = layout.wow_install(code);
    let dest = layout.wow_install_in(to);
    let already_there = || MoveError::AlreadyThere { product: code.as_str().to_owned(), library: to.label.clone() };
    if from == dest {
        return Err(already_there());
    }

    let info = BuildInfo::load(from.join(".build.info"))?;
    let row = info.named_row(code).ok_or_else(|| MoveError::NotInstalled(code.as_str().to_owned()))?;
    let dest_info_path = dest.join(".build.info");
    let mut dest_info = match BuildInfo::load(&dest_info_path) {
        Ok(info) => info,
        Err(_) if !dest_info_path.exists() => BuildInfo::with_default_schema(),
        Err(e) => return Err(e.into()),
    };
    let flavor = layout.flavor(code);
    let (src_flavor, dst_flavor) = (from.join(&*flavor), dest.join(&*flavor));
    if dest_info.row(code).is_some() || !is_empty_dir(&dst_flavor) {
        return Err(already_there());
    }
    let windows_path = to.windows_wow_path();

    // Nothing may write the source while it is read, or renamed.
    let _source_lock = StoreLock::acquire(&resolve_data_dir(&from))?;
    let same_filesystem = device(&from)? == device(&nearest_existing(&dest))?;

    if same_filesystem && is_empty_dir(&dest) && uninstall::plan(&from, code)?.is_whole_install() {
        if dest.exists() {
            std::fs::remove_dir(&dest).map_err(io(&dest))?;
        }
        std::fs::rename(&from, &dest).map_err(io(&dest))?;
        // Its `.product.db` still names the old Windows path.
        let record = ProductInstall::from_build_info(&info, code, &flavor, &windows_path)?;
        productdb::upsert(&dest, &[record])?;
        tracing::info!("moved {} by renaming {} to {}", code.as_str(), from.display(), dest.display());
        return Ok(Moved::Renamed);
    }

    let (copied, bytes) = copy_store(http, &info, code, &from, &dest, cancel, &mut progress).await?;

    // The configs travel too: the client reads them before any CDN.
    for column in [COL_BUILD_KEY, COL_CDN_KEY] {
        let hash = info.get(code, column)?;
        let source = tact_config::config_path(&from.join("Data").join("config"), hash)
            .ok()
            .and_then(|path| std::fs::read(path).ok());
        if let Some(bytes) = source {
            if let Err(e) = tact_config::store(&dest.join("Data"), hash, &bytes) {
                tracing::warn!("could not carry config {hash} over: {e}");
            }
        }
    }

    // Executables, WTF, AddOns, screenshots: the folder as it is.
    let renamed = same_filesystem && src_flavor.is_dir();
    match renamed {
        true => std::fs::rename(&src_flavor, &dst_flavor).map_err(io(&dst_flavor))?,
        false => crate::fsutil::copy_tree(&src_flavor, &dst_flavor)
            .map_err(|(path, source)| MoveError::Io { path, source })?,
    }

    let pairs: Vec<(&str, &str)> = row.iter().map(|(column, value)| (column.as_str(), value.as_str())).collect();
    let mut commit = || -> Result<(), MoveError> {
        dest_info.set_product(code, &pairs)?;
        dest_info.save(&dest_info_path)?;
        let record = ProductInstall::from_build_info(&dest_info, code, &flavor, &windows_path)?;
        productdb::upsert(&dest, &[record])?;
        Ok(())
    };
    if let Err(e) = commit() {
        // The source is still the game; give it its folder back.
        if renamed {
            let _ = std::fs::rename(&dst_flavor, &src_flavor);
        }
        return Err(e);
    }
    tracing::info!("copied {} into {}: {} files, {bytes} B", code.as_str(), dest.display(), copied);
    Ok(Moved::Copied { files: copied, bytes })
}

/// The store half of [`copy`]: every entry `code`'s build needs that the
/// target lacks, copied as it is and read back.
async fn copy_store(
    http: &Http,
    info: &BuildInfo,
    code: &ProductCode,
    from: &Path,
    dest: &Path,
    cancel: &Cancel,
    progress: &mut impl FnMut(Progress),
) -> Result<(usize, u64), MoveError> {
    let build = Build::installed(info, code)?;
    let mut source = Storage::open(from)?;
    let wanted = reclaim::keep_keys(http, from, &mut source, &build).await?;
    let present: HashSet<[u8; 9]> =
        Storage::open(dest).map(|store| store.files().map(|e| e.key).collect()).unwrap_or_default();

    let mut entries: Vec<Entry> =
        source.files().filter(|e| wanted.contains(&e.key) && !present.contains(&e.key)).copied().collect();
    // In file order, so each source archive is read front to back.
    entries.sort_by_key(|e| (e.archive_id, e.offset));
    let total: u64 = entries.iter().map(|e| u64::from(e.size)).sum();

    let mut writer = StoreWriter::create(dest.join("Data").join("data"))?;
    let mut copied: HashSet<[u8; 9]> = HashSet::new();
    let mut done = 0u64;
    for entry in &entries {
        if cancel.is_cancelled() {
            writer.finish()?;
            return Err(MoveError::Cancelled);
        }
        if copied.contains(&entry.key) {
            continue;
        }
        let raw = source.read_raw(entry)?;
        match writer.relocate(entry, &raw) {
            Ok(_) => {
                copied.insert(entry.key);
            }
            // A headerless alias: the same key's headed copy is the one moved.
            Err(WriteError::BadHeader { .. }) => {}
            Err(e) => return Err(e.into()),
        }
        done += u64::from(entry.size);
        progress(Progress { bytes_done: done, bytes_total: total, verifying: false });
    }
    writer.finish()?;

    let uncopied: HashSet<[u8; 9]> = entries.iter().map(|e| e.key).filter(|k| !copied.contains(k)).collect();
    if !uncopied.is_empty() {
        return Err(MoveError::Damaged(uncopied.len()));
    }

    let mut target = Storage::open(dest)?;
    let (mut checked, mut bad) = (0u64, 0usize);
    for key in &copied {
        if cancel.is_cancelled() {
            return Err(MoveError::Cancelled);
        }
        let mut ekey = [0u8; 16];
        ekey[..9].copy_from_slice(key);
        let Some(entry) = target.locate(&ekey).copied() else {
            bad += 1;
            continue;
        };
        let raw = target.read_raw(&entry)?;
        let intact = Header::verify(&raw, entry.archive_id, entry.offset)
            && raw.get(CASC_ENTRY_HEADER..).is_some_and(|blte| verify::blte_fault(blte, key).is_none());
        if !intact {
            bad += 1;
        }
        checked += u64::from(entry.size);
        progress(Progress { bytes_done: checked, bytes_total: total, verifying: true });
    }
    if bad > 0 {
        return Err(MoveError::CopyFailed(bad));
    }
    Ok((copied.len(), total))
}

/// Step 3: take `code` out of `from`, the library it moved out of, and point
/// Battle.net and the prefix at where `layout` now says it is. Returns the
/// bytes freed. After [`Moved::Renamed`] there is nothing left to remove.
pub async fn release(http: &Http, layout: &Layout, code: &ProductCode, from: &Library) -> Result<u64, MoveError> {
    if let Some(process) = crate::preflight::scan(layout).first() {
        return Err(MoveError::Busy(process.name.clone()));
    }
    let source = layout.wow_install_in(from);
    let info_path = source.join(".build.info");
    let mut freed = 0;
    // Gone with the folder after a rename.
    let info = match info_path.exists() {
        true => Some(BuildInfo::load(&info_path)?).filter(|info| info.row(code).is_some()),
        false => None,
    };
    if let Some(info) = info {
        let departing = Build::installed(&info, code)?;
        let plan = uninstall::plan(&source, code)?;
        // Planned before the row goes: the row names the build.
        let reclaim = match plan.is_whole_install() {
            true => None,
            false => Some(reclaim::plan(http, &source, &departing).await?),
        };
        freed += uninstall::execute(&source, code, &plan, &from.windows_wow_path())?.bytes;
        if let Some(reclaim) = &reclaim {
            freed += reclaim::execute(&source, reclaim, |_| {})?;
        }
    }

    crate::prefix::link_libraries(layout)?;
    let dest = layout.wow_install(code);
    let windows_path = layout
        .library(code)
        .map(Library::windows_wow_path)
        .unwrap_or_else(|| crate::paths::WINDOWS_WOW_PATH.to_owned());
    let record = ProductInstall::from_build_info(
        &BuildInfo::load(dest.join(".build.info"))?,
        code,
        &crate::paths::flavor_dir_name(&dest, code),
        &windows_path,
    )?;
    productdb::register(&layout.agent_dir(), &[record])?;
    Ok(freed)
}

/// Libraries besides `code`'s own whose install still holds it: what a move
/// leaves when [`release`] failed. Releasing from one removes it. None while
/// the game's own library is not mounted, since release registers that one.
pub fn leftovers(layout: &Layout, code: &ProductCode) -> Vec<Library> {
    if layout.library(code).is_some_and(|own| !own.state().is_present()) {
        return Vec::new();
    }
    let own = layout.wow_install(code);
    layout
        .libraries()
        .all()
        .iter()
        .filter(|library| library.state().is_present())
        .filter(|library| layout.wow_install_in(library) != own)
        .filter(|library| {
            BuildInfo::load(layout.wow_install_in(library).join(".build.info"))
                .is_ok_and(|info| info.row(code).is_some())
        })
        .cloned()
        .collect()
}

fn device(path: &Path) -> Result<u64, MoveError> {
    Ok(std::fs::metadata(path).map_err(io(path))?.dev())
}

fn nearest_existing(path: &Path) -> PathBuf {
    path.ancestors().find(|p| p.exists()).unwrap_or(Path::new("/")).to_path_buf()
}

/// Missing counts as empty.
fn is_empty_dir(path: &Path) -> bool {
    std::fs::read_dir(path).map_or(true, |mut entries| entries.next().is_none())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::Libraries;

    /// Two claimed libraries under one temp dir, so one filesystem, and the
    /// layout that has Classic Era in the first.
    fn libraries(root: &Path) -> (Layout, Library, Library) {
        let mut from = Library::main_at(root.join("a/games"), "A");
        let mut to = Library::adopted(root.join("b/games"), "B");
        from.claim().unwrap();
        to.claim().unwrap();
        let layout =
            Layout::with_root(root).with_libraries(Libraries::new(vec![from.clone(), to.clone()]));
        (layout, from, to)
    }

    /// An install `copy` can read: a row, a store directory, a game folder.
    fn install(dir: &Path, code: &ProductCode) {
        std::fs::create_dir_all(dir.join("Data/data")).unwrap();
        let flavor = dir.join(&*code.install_dir());
        std::fs::create_dir_all(&flavor).unwrap();
        std::fs::write(flavor.join("WowClassic.exe"), b"MZ").unwrap();
        let mut info = BuildInfo::with_default_schema();
        info.set_product(
            code,
            &[
                ("Branch", "eu"),
                ("Active", "1"),
                (COL_BUILD_KEY, &"a".repeat(32)),
                (COL_CDN_KEY, &"c".repeat(32)),
                ("CDN Path", "tpr/wow"),
                ("Version", "1.15.9.69722"),
            ],
        )
        .unwrap();
        info.save(dir.join(".build.info")).unwrap();
    }

    #[tokio::test]
    async fn a_game_alone_on_the_same_filesystem_moves_by_renaming_its_install() {
        let root = tempfile::tempdir().unwrap();
        let (layout, from, to) = libraries(root.path());
        let code = ProductCode::ClassicEra;
        install(&layout.wow_install_in(&from), &code);

        let http = Http::new().unwrap();
        let moved = copy(&http, &layout, &code, &to, &Cancel::new(), |_| {}).await.unwrap();

        assert_eq!(moved, Moved::Renamed);
        let dest = layout.wow_install_in(&to);
        assert!(BuildInfo::load(dest.join(".build.info")).unwrap().row(&code).is_some());
        assert!(dest.join("_classic_era_/WowClassic.exe").is_file());
        assert!(!layout.wow_install_in(&from).exists());
        let records = productdb::read(&dest.join(".product.db")).unwrap();
        assert_eq!(records[0].install_path, to.windows_wow_path());
    }

    #[tokio::test]
    async fn a_library_that_already_holds_the_game_is_refused_untouched() {
        let root = tempfile::tempdir().unwrap();
        let (layout, from, to) = libraries(root.path());
        let code = ProductCode::ClassicEra;
        install(&layout.wow_install_in(&from), &code);
        install(&layout.wow_install_in(&to), &code);

        let http = Http::new().unwrap();
        let refused = copy(&http, &layout, &code, &to, &Cancel::new(), |_| {}).await;

        assert!(matches!(refused, Err(MoveError::AlreadyThere { .. })));
        assert!(layout.wow_install_in(&from).join("_classic_era_/WowClassic.exe").is_file());
    }

    #[test]
    fn a_copy_in_another_library_is_a_leftover_and_the_game_s_own_is_not() {
        let root = tempfile::tempdir().unwrap();
        let (layout, from, to) = libraries(root.path());
        let code = ProductCode::ClassicEra;
        install(&layout.wow_install_in(&from), &code);
        assert!(leftovers(&layout, &code).is_empty());

        install(&layout.wow_install_in(&to), &code);
        let found: Vec<String> = leftovers(&layout, &code).into_iter().map(|l| l.label).collect();
        assert_eq!(found, ["B"]);
        assert!(leftovers(&layout, &ProductCode::Retail).is_empty(), "another game's row is not this one");
    }
}
