//! Clients beside the first, for multi-boxing.
//!
//! Every client of a product reads `WTF/Config.wtf` and writes it back on
//! exit, so each extra client starts from its own copy through `-config`,
//! windowed. The main client's settings are never written by another.

use std::io;
use std::path::Path;

pub const MAX_CLIENTS: u8 = 5;

/// Write the config client `slot` (2 and up) starts with, and return the
/// game arguments that point it there.
///
/// Copied fresh each launch, so account, portal and graphics follow the
/// main client's.
pub fn prepare(wtf_dir: &Path, slot: u8) -> io::Result<Vec<String>> {
    let name = format!("Config-{slot}.wtf");
    let own = wtf_dir.join(&name);
    let with_path = |e: io::Error| io::Error::new(e.kind(), format!("failed to write {}: {e}", own.display()));
    match std::fs::copy(wtf_dir.join("Config.wtf"), &own) {
        Ok(_) => {}
        // A client that never ran has no config yet; `cvars::set` creates one.
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(with_path(e)),
    }
    crate::cvars::set(&own, &[("gxWindow", "1"), ("gxMaximize", "0")]).map_err(with_path)?;
    Ok(vec!["-config".to_owned(), name])
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extra_client_gets_a_windowed_copy_and_leaves_the_main_config_alone() {
        let dir = tempfile::tempdir().unwrap();
        let main = dir.path().join("Config.wtf");
        let original = "SET portal \"EU\"\nSET gxMaximize \"1\"\n";
        std::fs::write(&main, original).unwrap();

        let args = prepare(dir.path(), 2).unwrap();

        assert_eq!(args, ["-config", "Config-2.wtf"]);
        assert_eq!(std::fs::read_to_string(&main).unwrap(), original);
        let own = dir.path().join("Config-2.wtf");
        assert_eq!(crate::cvars::get(&own, "portal").as_deref(), Some("EU"));
        assert_eq!(crate::cvars::get(&own, "gxWindow").as_deref(), Some("1"));
        assert_eq!(crate::cvars::get(&own, "gxMaximize").as_deref(), Some("0"));
    }

    #[test]
    fn extra_client_starts_without_a_main_config() {
        let dir = tempfile::tempdir().unwrap();
        prepare(dir.path(), 3).unwrap();
        assert_eq!(crate::cvars::get(&dir.path().join("Config-3.wtf"), "gxWindow").as_deref(), Some("1"));
    }
}
