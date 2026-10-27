use super::{ManageError, Options};
use crate::addons::catalog::Catalog;
use crate::addons::install::{Ledger, Record};
use crate::net::Http;

/// One addon that was recognised and is now managed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Adopted {
    pub name: String,
    pub folders: Vec<String>,
}

/// Recognise hand-installed addons and take them into the ledger.
///
/// An unmanaged folder can only be listed; fingerprinting it the way
/// CurseForge does (see [`crate::addons::fingerprint`]) and asking the catalog
/// makes it an ordinary managed addon. A folder another record owns is left
/// alone, and only folders actually present are recorded, so a later removal
/// never promises to delete one that is not there.
pub async fn adopt(
    http: &Http,
    catalog: Option<&Catalog>,
    options: &Options,
) -> Result<Vec<Adopted>, ManageError> {
    use crate::addons::fingerprint;

    let Some(catalog) = catalog else {
        return Ok(Vec::new());
    };
    let mut ledger = Ledger::load(&options.ledger_path)?;

    // Folders a record already claims. Not just the ones on disk: a record
    // whose folders are missing is still that addon's, and adopting over it
    // would leave two records fighting for one name.
    let mut claimed: std::collections::BTreeSet<String> = ledger
        .addons
        .iter()
        .flat_map(|record| record.folders.iter().cloned())
        .collect();

    let present: Vec<String> = crate::addons::scan(&options.addons_dir)
        .unwrap_or_default()
        .into_iter()
        .map(|addon| addon.folder)
        .collect();

    // Fingerprint to folder, so the catalog's answer, which is keyed by the
    // hashes we sent, can be read back as "this folder is that addon".
    let mut by_print: std::collections::BTreeMap<u32, String> = Default::default();
    for folder in present.iter().filter(|f| !claimed.contains(*f)) {
        if let Some(print) = fingerprint::of_folder(&options.addons_dir.join(folder)) {
            by_print.insert(print, folder.clone());
        }
    }
    if by_print.is_empty() {
        return Ok(Vec::new());
    }

    let prints: Vec<u32> = by_print.keys().copied().collect();
    let matches = catalog.match_fingerprints(http, &prints).await?;

    let on_disk: std::collections::BTreeSet<&str> =
        present.iter().map(String::as_str).collect();

    let by_project = group_by_project(&matches, &by_print);
    let mut adopted = Vec::new();
    for (id, (found, matched)) in by_project {
        // The package's folders, limited to what is here and not already
        // somebody else's, plus every folder that actually matched, which
        // is the stronger evidence of the two.
        let mut folders: Vec<String> = found
            .folders
            .iter()
            .filter(|f| on_disk.contains(f.as_str()) && !claimed.contains(*f))
            .cloned()
            .chain(matched)
            .filter(|f| !claimed.contains(f))
            .collect();
        folders.sort();
        folders.dedup();
        if folders.is_empty() {
            continue;
        }
        // Taken now, so a second project in the same answer cannot claim it.
        claimed.extend(folders.iter().cloned());

        // Already managed under this id: a folder an older build left behind,
        // most likely. It joins that record, which keeps everything the user
        // chose about the addon; a fresh record would have replaced it.
        if let Some(existing) = ledger.find_mut("curse", &id) {
            existing.folders.extend(folders.iter().cloned());
            existing.folders.sort();
            existing.folders.dedup();
            adopted.push(Adopted { name: existing.name.clone(), folders });
            continue;
        }

        let name = found
            .name
            .clone()
            .or_else(|| folders.first().cloned())
            .unwrap_or_else(|| id.clone());
        ledger.record(Record {
            provider: "curse".to_owned(),
            id,
            name: name.clone(),
            file_id: found.file_id.clone(),
            folders: folders.clone(),
            // Adopted, not installed: the build on disk is whatever the user
            // put there, and the day it arrived is not ours to claim.
            ..Record::default()
        });
        adopted.push(Adopted { name, folders });
    }

    if !adopted.is_empty() {
        ledger.save(&options.ledger_path)?;
    }
    adopted.sort_by_key(|a| a.name.to_lowercase());
    Ok(adopted)
}

/// Gather fingerprint matches into one entry per project.
///
/// Every folder of a package matches separately, BigWigs answers twelve
/// times, EllesmereUI twenty-two, and each answer names the same project.
/// Writing one record per match records one addon a dozen times over, and
/// reports it a dozen times to the user, which is how this was noticed.
pub(super) fn group_by_project(
    matches: &std::collections::BTreeMap<String, crate::addons::catalog::Match>,
    by_print: &std::collections::BTreeMap<u32, String>,
) -> std::collections::BTreeMap<String, (crate::addons::catalog::Match, Vec<String>)> {
    let mut out: std::collections::BTreeMap<String, (crate::addons::catalog::Match, Vec<String>)> =
        Default::default();
    for (print, found) in matches {
        let Ok(print) = print.parse::<u32>() else { continue };
        let Some(folder) = by_print.get(&print) else { continue };

        out.entry(found.id.clone())
            .or_insert_with(|| (found.clone(), Vec::new()))
            .1
            .push(folder.clone());
    }
    out
}
