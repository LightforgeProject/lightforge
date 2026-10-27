//! Drives: adding, removing, the default, and the space on each.

use crate::*;

impl Lightforge {
    pub(super) fn update_drives(&mut self, message: Message) -> Result<Task<Message>, Message> {
        Ok(match message {
            Message::StorageMeasured(report) => {
                self.storage_measuring = false;
                self.storage = Some(*report);
                Task::none()
            }
            Message::RefreshDrives => {
                self.refresh_drives();
                self.measure_storage()
            }
            Message::AddDrive(index) => {
                match self.add_drive(index) {
                    Ok(()) => {
                        self.refresh_drives();
                        self.measure_storage()
                    }
                    Err(e) => {
                        self.error = Some(e.into());
                        Task::none()
                    }
                }
            }
            Message::SetDefaultDrive(id) => {
                // The button is held for a drive that is away, but this also
                // decides where the *next* install goes and what Battle.net is
                // told, neither is worth pointing at a drive that is not here.
                if let Some(drive) = self
                    .config
                    .libraries
                    .iter()
                    .find(|l| l.id == id)
                    .filter(|l| self.drive_states.get(&l.id) == Some(&library::State::Missing))
                    .map(|l| l.label.clone())
                {
                    self.error = Some(format!("{drive} is not mounted").into());
                    return Ok(Task::none());
                }
                for library in &mut self.config.libraries {
                    library.is_default = library.id == id;
                }
                self.save_config();
                self.rebuild_layout();
                // Battle.net's `DefaultInstallPath` follows the default drive,
                // and it is only written where nothing in the prefix is running,
                // so it is re-asserted by the next pre-flight, not here. The
                // client itself does not move: it lives in the prefix.
                self.status = "new installs will go to this drive".to_owned();
                self.measure_storage()
            }
            Message::RemoveDrive(id) => {
                match self.remove_drive(&id) {
                    // `refresh_drives` as well as the measurement: a drive
                    // that is no longer ours goes back to being a partition
                    // the page can offer, and that list is only rebuilt here.
                    // Without it the row stays gone until you leave Settings
                    // and come back.
                    Ok(()) => {
                        self.refresh_drives();
                        self.measure_storage()
                    }
                    Err(e) => {
                        self.error = Some(e.into());
                        Task::none()
                    }
                }
            }
            Message::ClearDrive(id) => {
                let Some(library) = self.config.libraries.iter().find(|l| l.id == id).cloned()
                else {
                    return Ok(Task::none());
                };
                self.drive_notice = Some((id.clone(), "clearing…".to_owned()));
                let layout = self.layout.clone();
                Task::perform(
                    tasks::clear_library(layout, library),
                    move |result| Message::DriveCleared(id.clone(), result),
                )
            }
            Message::UnusedPlanned(id, result) => {
                if !matches!(self.unused.get(&id), Some(UnusedData::Freeing { .. })) {
                    let state = match result {
                        Ok(plan) => UnusedData::Ready(plan),
                        Err(e) => UnusedData::Said(e),
                    };
                    self.unused.insert(id, state);
                }
                Task::none()
            }
            Message::FreeUnused(id) => {
                let Some(library) = self.config.libraries.iter().find(|l| l.id == id) else {
                    return Ok(Task::none());
                };
                let install = self.layout.wow_install_in(library);
                if self.downloading_into(&install) {
                    return Ok(Task::none());
                }
                // Compaction moves and deletes archives for minutes; nothing
                // else may write the store meanwhile, and the client may not
                // have it open. `pump` and pre-flight both hold while it runs.
                if let Some(what) = self.store_writer() {
                    self.error = Some(format!("wait for {what} to finish first").into());
                    return Ok(Task::none());
                }
                if self.launching.is_some() {
                    self.error = Some("close the game first".into());
                    return Ok(Task::none());
                }
                let total = match self.unused.get(&id) {
                    Some(UnusedData::Ready(plan)) => plan.moved,
                    _ => return Ok(Task::none()),
                };
                let copied = Arc::<std::sync::atomic::AtomicU64>::default();
                self.unused.insert(id.clone(), UnusedData::Freeing { copied: copied.clone(), total });
                Task::perform(
                    tasks::free_unused(self.http.clone(), self.layout.clone(), install, copied),
                    move |result| Message::UnusedFreed(id.clone(), result),
                )
            }
            Message::UnusedFreed(id, result) => {
                let note = match result {
                    Ok(bytes) => format!("freed {}", size_of(bytes)),
                    Err(e) => e,
                };
                self.unused.insert(id, UnusedData::Said(note));
                Task::batch([self.measure_storage(), self.pump()])
            }
            Message::DriveCleared(id, result) => {
                self.drive_notice = Some(match result {
                    Ok(0) => (id, "nothing to clear".to_owned()),
                    Ok(bytes) => (id, format!("freed {}", size_of(bytes))),
                    Err(e) => (id, e),
                });
                self.measure_storage()
            }
            Message::OpenDrive(id) => {
                if let Some(library) = self.config.libraries.iter().find(|l| l.id == id) {
                    // A drive with nothing installed on it yet has no `games/`
                    // folder, so this opens the nearest thing that does exist,
                    // the place the folder would appear.
                    let path = library
                        .games()
                        .ancestors()
                        .find(|p| p.is_dir())
                        .map(Path::to_path_buf)
                        .unwrap_or_else(|| library.games().to_path_buf());
                    if let Err(e) = tasks::open_path(path) {
                        self.error = Some(e.into());
                    }
                }
                Task::none()
            }
            other => return Err(other),
        })
    }
}
