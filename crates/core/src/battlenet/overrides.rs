//! What the background mode overwrote in the client's settings, so switching
//! it off puts back the user's own values instead of forcing a default:
//! auto-update for Battle.net's games, notifications.
//!
//! Each key keeps the first value seen: after that pass the file holds
//! Lightforge's. Restored only with the client closed, since it rewrites its
//! files on exit. Kept beside those files because the GUI, its workers and
//! the CLI all run pre-flight, and the prefix is what they share.

use std::collections::BTreeMap;
use std::path::Path;

use serde_json::Value;

use super::tweaks::{account_configs, merge_file, remove_key};
use super::{io, BattleNetError, Tweak};
use crate::paths::Layout;

/// Not `*.config`, which would make it an account config.
const RECORD_FILE: &str = "lightforge-overrides.json";

/// Config file name, then dotted key, then the value before Lightforge's;
/// `None` where the key was absent.
type Record = BTreeMap<String, BTreeMap<String, Option<Value>>>;

/// Note what `global` (for `Battle.net.config`) and `account` (for every
/// account config) are about to replace.
pub fn record_overrides(layout: &Layout, user: &str, global: &[Tweak], account: &[Tweak]) -> Result<(), BattleNetError> {
    let global_path = layout.battle_net_config(user);
    let Some(dir) = global_path.parent() else {
        return Ok(());
    };
    let mut record = read_record(dir)?;
    let before = record.clone();
    note(&mut record, &global_path, global)?;
    for path in std::fs::read_dir(dir).map(account_configs).unwrap_or_default() {
        note(&mut record, &path, account)?;
    }
    if record == before {
        return Ok(());
    }
    let path = dir.join(RECORD_FILE);
    let mut text = serde_json::to_string_pretty(&record)
        .map_err(|e| BattleNetError::MalformedConfig { path: path.clone(), source: e })?;
    text.push('\n');
    crate::fsutil::write_atomic(&path, text.as_bytes()).map_err(io(&path))
}

/// Put back what [`record_overrides`] noted, then forget it. `false` when
/// there was nothing to put back.
pub fn restore_overrides(layout: &Layout, user: &str) -> Result<bool, BattleNetError> {
    let global_path = layout.battle_net_config(user);
    let Some(dir) = global_path.parent() else {
        return Ok(false);
    };
    let record = read_record(dir)?;
    if record.is_empty() {
        return Ok(false);
    }
    for (file, keys) in &record {
        let path = dir.join(file);
        let back: Vec<Tweak> = keys
            .iter()
            .filter_map(|(key, value)| value.clone().map(|value| Tweak::new(key, value, "the value before the background mode")))
            .collect();
        merge_file(&path, &back, false)?;
        for (key, _) in keys.iter().filter(|(_, value)| value.is_none()) {
            remove_key(&path, key)?;
        }
    }
    let path = dir.join(RECORD_FILE);
    std::fs::remove_file(&path).map_err(io(&path))?;
    Ok(true)
}

fn read_record(dir: &Path) -> Result<Record, BattleNetError> {
    let path = dir.join(RECORD_FILE);
    match std::fs::read_to_string(&path) {
        Ok(text) => serde_json::from_str(&text).map_err(|e| BattleNetError::MalformedConfig { path, source: e }),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Record::new()),
        Err(e) => Err(io(&path)(e)),
    }
}

fn note(record: &mut Record, path: &Path, tweaks: &[Tweak]) -> Result<(), BattleNetError> {
    let config: Value = match std::fs::read_to_string(path) {
        Ok(text) => serde_json::from_str(&text)
            .map_err(|e| BattleNetError::MalformedConfig { path: path.to_path_buf(), source: e })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Value::Null,
        Err(e) => return Err(io(path)(e)),
    };
    let Some(file) = path.file_name().map(|name| name.to_string_lossy().into_owned()) else {
        return Ok(());
    };
    let noted = record.entry(file).or_default();
    for tweak in tweaks {
        noted
            .entry(tweak.key())
            .or_insert_with(|| tweak.path.iter().try_fold(&config, |node, key| node.get(key)).cloned());
    }
    Ok(())
}
