//! Settings backups: listing, taking, restoring, deleting.

use super::*;

/// Where this game's settings backups go, and how many it keeps.
pub fn settings_backup(layout: &Layout, dirs: &AppDirs, code: &ProductCode) -> backup::Plan {
    let keep = Config::load_or_default(&dirs.config_file())
        .map_or(backup::DEFAULT_KEEP, |config| config.backup_keep);
    backup::Plan::for_product(layout, dirs, code, keep)
}

/// A game's settings backups, newest first.
pub async fn list_backups(layout: Layout, code: ProductCode) -> Result<Vec<backup::Backup>, String> {
    tokio::task::spawn_blocking(move || {
        let dirs = AppDirs::discover().map_err(|e| chain(&e))?;
        settings_backup(&layout, &dirs, &code).list().map_err(|e| chain(&e))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Back up a game's settings now, answering with the sentence to show.
pub async fn create_backup(layout: Layout, code: ProductCode) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let dirs = AppDirs::discover().map_err(|e| chain(&e))?;
        let plan = settings_backup(&layout, &dirs, &code);
        Ok(match plan.create(backup::Reason::Manual).map_err(|e| chain(&e))? {
            Some(_) => "Backed up.".to_owned(),
            None if plan.account_dir.is_dir() => "Nothing changed since the last backup.".to_owned(),
            None => "Nothing to back up yet: the game has not been started.".to_owned(),
        })
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Put one backup back. Refused while a game runs: the client writes
/// SavedVariables when it exits and would overwrite the restore.
pub async fn restore_backup(layout: Layout, code: ProductCode, archive: PathBuf) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        if !lightforge_core::preflight::games_running(&layout).is_empty() {
            return Err("Close the game first, it would overwrite the restore when it exits.".to_owned());
        }
        let dirs = AppDirs::discover().map_err(|e| chain(&e))?;
        let plan = settings_backup(&layout, &dirs, &code);
        let backups = plan.list().map_err(|e| chain(&e))?;
        let chosen = backups
            .iter()
            .find(|b| b.path == archive)
            .ok_or("That backup is no longer there.")?;
        plan.restore(chosen).map_err(|e| chain(&e))?;
        Ok(format!("Restored the backup from {}.", chosen.when()))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Delete one backup, answering with the sentence to show.
pub async fn delete_backup(layout: Layout, code: ProductCode, archive: PathBuf) -> Result<String, String> {
    tokio::task::spawn_blocking(move || {
        let dirs = AppDirs::discover().map_err(|e| chain(&e))?;
        let plan = settings_backup(&layout, &dirs, &code);
        let backups = plan.list().map_err(|e| chain(&e))?;
        let chosen = backups
            .iter()
            .find(|b| b.path == archive)
            .ok_or("That backup is no longer there.")?;
        plan.delete(chosen).map_err(|e| chain(&e))?;
        Ok(format!("Deleted the backup from {}.", chosen.when()))
    })
    .await
    .map_err(|e| e.to_string())?
}
