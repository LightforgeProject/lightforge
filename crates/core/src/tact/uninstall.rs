//! Removing an installed product.
//!
//! Deleting a game is the one operation where being slightly wrong is
//! unrecoverable, so this is split in two: [`plan`] reads and decides, and
//! [`execute`] does exactly what the plan says. A caller can show the plan and
//! ask before anything is touched.
//!
//! The rule that shapes everything here: **products share one install.** The
//! CASC store under `Data/` holds every product's files interleaved in the same
//! archives, and `.build.info` holds a row per product. So removing one product
//! means its flavor directory, its row, and its `.product.db` record, and the
//! store survives untouched *unless it was the last product*, at which point
//! the store is orphaned and goes too.

use std::path::{Path, PathBuf};

use crate::buildinfo::BuildInfo;
use crate::product::ProductCode;
use crate::tact::productdb;

/// What removing a product would delete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The product's flavor directory, e.g. `_classic_era_`.
    pub flavor_dir: Option<PathBuf>,
    /// The shared CASC store, present only when this is the last product and
    /// the store would otherwise be left behind with nothing to read it.
    pub store: Option<PathBuf>,
    /// `.build.info` row to drop, and whether the file itself would go.
    pub has_row: bool,
    pub remove_build_info: bool,
    /// Products that remain installed afterwards.
    pub remaining: Vec<ProductCode>,
    /// Bytes on disk the deletions would free.
    pub bytes: u64,
}

impl Plan {
    /// Whether there is anything to do at all.
    pub fn is_empty(&self) -> bool {
        self.flavor_dir.is_none() && self.store.is_none() && !self.has_row
    }

    /// Whether this removes the entire install rather than one product.
    pub fn is_whole_install(&self) -> bool {
        self.store.is_some()
    }
}

/// Decide what removing `code` from `install` would delete. Touches nothing.
pub fn plan(install: &Path, code: &ProductCode) -> Result<Plan, UninstallError> {
    // A file that is there but cannot be read says nothing about which
    // products remain, and guessing "none" would delete the shared store.
    // Only a file that does not exist at all means no product finished.
    let info_path = install.join(".build.info");
    let info = match BuildInfo::load(&info_path) {
        Ok(info) => Some(info),
        Err(_) if !info_path.exists() => None,
        Err(e) => return Err(UninstallError::Unreadable(Box::new(e))),
    };
    let remaining: Vec<ProductCode> = info
        .as_ref()
        .map(|info| info.products().filter(|p| p != code).collect())
        .unwrap_or_default();

    let flavor = install.join(&*code.install_dir());
    if !is_plain_child(install, &flavor) {
        return Err(UninstallError::Outside(flavor));
    }
    let flavor_dir = flavor.is_dir().then_some(flavor);
    let store = install.join("Data");

    // The store only goes when nothing is left to read it. A second product
    // sharing these archives would be destroyed by deleting them. Without a
    // `.build.info`, another product's flavor folder is the only sign one is
    // still there, a first install that has not finished yet for example.
    let others_on_disk = info.is_none() && has_other_flavor(install, flavor_dir.as_deref());
    let store = (remaining.is_empty() && !others_on_disk && store.is_dir()).then_some(store);

    let bytes = flavor_dir.iter().chain(store.iter()).map(|p| dir_size(p)).sum();

    Ok(Plan {
        has_row: info.as_ref().is_some_and(|i| i.row(code).is_some()),
        remove_build_info: remaining.is_empty(),
        flavor_dir,
        store,
        remaining,
        bytes,
    })
}

/// What was actually removed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Removed {
    pub paths: Vec<PathBuf>,
    pub bytes: u64,
}

/// Carry out a plan.
///
/// Every path is checked to be inside `install` before it is deleted, a plan
/// is data, and data that escaped the install root would be a very expensive
/// bug.
///
/// `windows_path` is how Battle.net addresses this install, which differs per
/// library; see [`crate::library::Library::windows_wow_path`]. It is only
/// used to rewrite `.product.db` for the products that remain, and getting it
/// wrong there is what makes the client reject an install it was happy with.
pub fn execute(
    install: &Path,
    code: &ProductCode,
    plan: &Plan,
    windows_path: &str,
) -> Result<Removed, UninstallError> {
    let mut removed = Removed { bytes: plan.bytes, ..Removed::default() };

    for path in plan.flavor_dir.iter().chain(plan.store.iter()) {
        if !is_plain_child(install, path) {
            return Err(UninstallError::Outside(path.clone()));
        }
        std::fs::remove_dir_all(path).map_err(|source| UninstallError::Io {
            path: path.clone(),
            source,
        })?;
        removed.paths.push(path.clone());
    }

    let info_path = install.join(".build.info");
    match plan.remove_build_info {
        // Nothing left to describe: the file goes with the last product.
        true => {
            if info_path.exists() {
                std::fs::remove_file(&info_path).map_err(|source| UninstallError::Io {
                    path: info_path.clone(),
                    source,
                })?;
                removed.paths.push(info_path);
            }
        }
        false => {
            if let Ok(mut info) = BuildInfo::load(&info_path) {
                if info.remove_product(code) {
                    info.save(&info_path)?;
                }
            }
        }
    }

    // `.product.db` is rewritten for whatever is left, or removed with the
    // last product. A record naming a product that is gone is exactly what
    // makes Battle.net reject an install.
    let db_path = install.join(".product.db");
    if db_path.exists() {
        match plan.remaining.is_empty() {
            true => {
                std::fs::remove_file(&db_path).map_err(|source| UninstallError::Io {
                    path: db_path.clone(),
                    source,
                })?;
                removed.paths.push(db_path);
            }
            false => {
                if let Ok(info) = BuildInfo::load(install.join(".build.info")) {
                    let records: Vec<_> = plan
                        .remaining
                        .iter()
                        .filter_map(|code| {
                            productdb::ProductInstall::from_build_info(&info, code, windows_path)
                            .ok()
                        })
                        .collect();
                    productdb::write(install, &records)?;
                }
            }
        }
    }

    Ok(removed)
}

/// Whether `path` is `install` plus one ordinary folder name, never a path
/// that climbs out of it through `..`. `starts_with` alone is textual and
/// would accept `install/../elsewhere`.
fn is_plain_child(install: &Path, path: &Path) -> bool {
    path.strip_prefix(install).is_ok_and(|rest| {
        let mut parts = rest.components();
        matches!(parts.next(), Some(std::path::Component::Normal(_))) && parts.next().is_none()
    })
}

/// Whether a flavor folder other than `own` sits in the install, `_retail_`,
/// `_classic_era_` and the like.
fn has_other_flavor(install: &Path, own: Option<&Path>) -> bool {
    let Ok(entries) = std::fs::read_dir(install) else {
        return false;
    };
    entries.flatten().any(|entry| {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        name.len() > 2
            && name.starts_with('_')
            && name.ends_with('_')
            && entry.file_type().is_ok_and(|t| t.is_dir())
            && own != Some(entry.path().as_path())
    })
}

/// Recursive size of a directory, ignoring anything unreadable.
fn dir_size(path: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(path) else {
        return 0;
    };
    entries
        .flatten()
        .map(|entry| match entry.file_type() {
            Ok(t) if t.is_dir() => dir_size(&entry.path()),
            Ok(t) if t.is_file() => entry.metadata().map(|m| m.len()).unwrap_or(0),
            _ => 0,
        })
        .sum()
}

#[derive(Debug, thiserror::Error)]
pub enum UninstallError {
    #[error("refusing to delete {0}: it is outside the install directory")]
    Outside(PathBuf),

    #[error(transparent)]
    BuildInfo(#[from] crate::buildinfo::BuildInfoError),

    /// Refused rather than guessed: which products share the store is
    /// written nowhere else.
    #[error("refusing to plan an uninstall: .build.info cannot be read, so it is unknown which products still use the shared game data")]
    Unreadable(#[source] Box<crate::buildinfo::BuildInfoError>),

    #[error(transparent)]
    ProductDb(#[from] productdb::ProductDbError),

    #[error("i/o error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `name` must be unique per test: `scratch` wipes the directory, so two
    /// tests sharing one race under the parallel runner.
    fn install_with(name: &str, products: &[(&str, &str)]) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("lightforge-uninstall-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Data/data")).unwrap();
        std::fs::write(dir.join("Data/data/data.000"), vec![0u8; 2048]).unwrap();

        let mut info = BuildInfo::with_default_schema();
        for (code, flavor) in products {
            let code: ProductCode = code.parse().unwrap();
            std::fs::create_dir_all(dir.join(flavor)).unwrap();
            std::fs::write(dir.join(flavor).join("game.exe"), vec![0u8; 1024]).unwrap();
            info.set_product(
                &code,
                &[("Branch", "eu"), ("Active", "1"), ("Version", "1.0.0.1")],
            )
            .unwrap();
        }
        info.save(dir.join(".build.info")).unwrap();
        dir
    }

    #[test]
    fn removing_one_of_two_products_keeps_the_shared_store() {
        // The store holds both products' files in the same archives, so
        // deleting it would destroy the product that stays.
        let dir = install_with("two", &[("wow", "_retail_"), ("wow_classic_era", "_classic_era_")]);
        let plan = plan(&dir, &ProductCode::ClassicEra).unwrap();

        assert!(plan.store.is_none(), "the store must survive");
        assert!(!plan.remove_build_info);
        assert_eq!(plan.remaining, vec![ProductCode::Retail]);

        execute(&dir, &ProductCode::ClassicEra, &plan, crate::paths::WINDOWS_WOW_PATH).unwrap();
        assert!(!dir.join("_classic_era_").exists());
        assert!(dir.join("_retail_/game.exe").is_file());
        assert!(dir.join("Data/data/data.000").is_file());

        // The row is gone, the sibling's row is not.
        let info = BuildInfo::load(dir.join(".build.info")).unwrap();
        assert!(info.row(&ProductCode::ClassicEra).is_none());
        assert!(info.row(&ProductCode::Retail).is_some());
    }

    #[test]
    fn removing_the_last_product_takes_the_store_and_the_build_info() {
        let dir = install_with("last", &[("wow_classic_era", "_classic_era_")]);
        let plan = plan(&dir, &ProductCode::ClassicEra).unwrap();

        assert!(plan.is_whole_install());
        assert!(plan.bytes >= 3072, "counts the flavor dir and the store");

        execute(&dir, &ProductCode::ClassicEra, &plan, crate::paths::WINDOWS_WOW_PATH).unwrap();
        assert!(!dir.join("Data").exists());
        assert!(!dir.join(".build.info").exists());
        assert!(!dir.join("_classic_era_").exists());
    }

    #[test]
    fn a_product_that_is_not_installed_plans_nothing() {
        let dir = install_with("absent", &[("wow", "_retail_")]);
        let plan = plan(&dir, &ProductCode::Beta).unwrap();

        assert!(!plan.has_row);
        assert!(plan.flavor_dir.is_none());
        // Retail is still there, so the store stays regardless.
        assert!(plan.store.is_none());
        assert!(plan.is_empty());
    }

    #[test]
    fn an_unreadable_build_info_refuses_instead_of_taking_the_store() {
        // A ragged row used to read as "no other products", and the plan
        // then deleted the store Retail was still using.
        let dir = install_with("ragged", &[("wow", "_retail_"), ("wow_classic_era", "_classic_era_")]);
        std::fs::write(
            dir.join(".build.info"),
            "Branch!STRING:0|Active!DEC:1|Product!STRING:0\neu|1|wow\neu|1\n",
        )
        .unwrap();

        assert!(matches!(
            plan(&dir, &ProductCode::ClassicEra),
            Err(UninstallError::Unreadable(_))
        ));
        assert!(dir.join("Data/data/data.000").is_file());
    }

    #[test]
    fn without_a_build_info_another_flavor_keeps_the_store() {
        // Two first installs into one store, neither finished: discarding
        // one must not take the archives the other is writing.
        let dir = install_with("unfinished", &[("wow", "_retail_"), ("wow_classic_era", "_classic_era_")]);
        std::fs::remove_file(dir.join(".build.info")).unwrap();

        let plan = plan(&dir, &ProductCode::ClassicEra).unwrap();
        assert!(plan.store.is_none(), "_retail_ is still there");
        assert!(plan.flavor_dir.is_some());
    }

    #[test]
    fn without_a_build_info_a_lone_first_install_still_goes_whole() {
        let dir = install_with("lone", &[("wow_classic_era", "_classic_era_")]);
        std::fs::remove_file(dir.join(".build.info")).unwrap();

        assert!(plan(&dir, &ProductCode::ClassicEra).unwrap().is_whole_install());
    }

    #[test]
    fn a_climbing_path_is_refused_even_though_it_starts_with_the_install() {
        let dir = install_with("climb", &[("wow", "_retail_")]);
        let escaped = Plan {
            flavor_dir: Some(dir.join("..").join("lightforge-uninstall-climb-victim")),
            store: None,
            has_row: false,
            remove_build_info: false,
            remaining: vec![ProductCode::Retail],
            bytes: 0,
        };
        assert!(matches!(
            execute(&dir, &ProductCode::Retail, &escaped, crate::paths::WINDOWS_WOW_PATH),
            Err(UninstallError::Outside(_))
        ));
    }

    #[test]
    fn a_path_outside_the_install_is_refused() {
        let dir = install_with("outside", &[("wow", "_retail_")]);
        let escaped = Plan {
            flavor_dir: Some(PathBuf::from("/etc")),
            store: None,
            has_row: false,
            remove_build_info: false,
            remaining: Vec::new(),
            bytes: 0,
        };
        assert!(matches!(
            execute(&dir, &ProductCode::Retail, &escaped, crate::paths::WINDOWS_WOW_PATH),
            Err(UninstallError::Outside(_))
        ));
        assert!(Path::new("/etc").is_dir(), "nothing was deleted");
    }
}
