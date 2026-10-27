use super::{ManageError, Options};
use crate::addons::catalog::Request;
use crate::addons::install::{self, Ledger};

/// Remove an addon Lightforge installed.
pub fn remove(options: &Options, provider: &str, id: &str) -> Result<Vec<String>, ManageError> {
    let mut ledger = Ledger::load(&options.ledger_path)?;
    let removed = install::remove(&mut ledger, &options.addons_dir, provider, id)?;
    ledger.save(&options.ledger_path)?;
    Ok(removed)
}

/// The requests an import token implies.
pub fn requests_from(collection: &crate::addons::Collection) -> Vec<Request> {
    collection
        .addons
        .iter()
        .filter(|entry| entry.provider().is_supported())
        .map(Request::from)
        .collect()
}

/// Entries of a token Lightforge cannot resolve at all, for reporting.
pub fn unsupported(collection: &crate::addons::Collection) -> Vec<&crate::addons::Entry> {
    collection
        .addons
        .iter()
        .filter(|entry| !entry.provider().is_supported())
        .collect()
}

/// Set or clear one addon's channel override.
///
/// Clearing it (`None`) returns the addon to following the global default,
/// which is not the same as setting it to stable.
pub fn set_channel(
    options: &Options,
    provider: &str,
    id: &str,
    channel: Option<crate::addons::Channel>,
) -> Result<(), ManageError> {
    let mut ledger = Ledger::load(&options.ledger_path)?;
    if let Some(record) = ledger.find_mut(provider, id) {
        record.channel = channel;
        ledger.save(&options.ledger_path)?;
    }
    Ok(())
}

/// Leave one addon out of everything that updates in bulk, or stop doing so.
///
/// Deliberately not a toggle: the caller knows which state it is showing, and
/// a toggle would flip the wrong way against a ledger changed under it.
pub fn set_ignored(
    options: &Options,
    provider: &str,
    id: &str,
    ignored: bool,
) -> Result<(), ManageError> {
    let mut ledger = Ledger::load(&options.ledger_path)?;
    if let Some(record) = ledger.find_mut(provider, id) {
        record.ignored = ignored;
        ledger.save(&options.ledger_path)?;
    }
    Ok(())
}

/// Set or clear one addon's auto-update choice.
///
/// Clearing it (`None`) returns the addon to following the app-wide default,
/// which is not the same as turning auto-update off.
pub fn set_auto_update(
    options: &Options,
    provider: &str,
    id: &str,
    auto_update: Option<bool>,
) -> Result<(), ManageError> {
    let mut ledger = Ledger::load(&options.ledger_path)?;
    if let Some(record) = ledger.find_mut(provider, id) {
        record.auto_update = auto_update;
        ledger.save(&options.ledger_path)?;
    }
    Ok(())
}

/// Hold one addon at a file id, or let it track its channel again.
///
/// The caller passes the file to hold rather than a flag, because a pin is a
/// statement about a particular file: re-pinning after an update means
/// pinning to the *new* file, which a toggle could not express.
pub fn set_pinned(
    options: &Options,
    provider: &str,
    id: &str,
    file_id: Option<String>,
) -> Result<(), ManageError> {
    let mut ledger = Ledger::load(&options.ledger_path)?;
    if let Some(record) = ledger.find_mut(provider, id) {
        record.pinned = file_id;
        ledger.save(&options.ledger_path)?;
    }
    Ok(())
}
