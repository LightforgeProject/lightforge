//! Storage: the drives, what is on each, and freeing space.

use crate::*;
use iced::widget::column;

impl Lightforge {
    /// Where the library lives, how full its disk is, and what is using it.
    pub(crate) fn storage_section(&self) -> Element<'_, Message> {
        let report = self.storage.as_ref();

        // ---- One card per drive.
        let mut drives = column![].spacing(theme::MD);
        match report {
            None => drives = drives.push(muted("measuring…".to_owned(), theme::LABEL)),
            Some(report) => {
                for (index, library) in report.libraries.iter().enumerate() {
                    // The first library on a disk speaks for its free space.
                    let same_disk_as = report.libraries[..index]
                        .iter()
                        .find(|earlier| earlier.mount == library.mount)
                        .map(|earlier| earlier.label.as_str());
                    drives = drives.push(self.drive_card(library, same_disk_as));
                }
                drives = drives.spacing(theme::SM);
                drives = drives.extend(self.hint("The default drive is where a game goes unless you pick another one, and \
                     the folder Battle.net proposes for its own titles. It is only that: \
                     Battle.net itself and the Wine prefix stay where they were installed, \
                     above. Removing a drive takes it off this list and deletes nothing — \
                     add it again and Lightforge finds the games on it.").map(Element::from));
            }
        }

        // ---- Partitions that could become one.
        let offered: Vec<(usize, &library::Partition)> = self
            .partitions
            .iter()
            .enumerate()
            .filter(|(_, partition)| !partition.in_use)
            .collect();
        let add: Element<'_, Message> = match offered.is_empty() {
            true => setting_group([setting(
                "Nothing to add",
                "No other mounted partition Lightforge could write to.",
                None,
                Space::new(),
            )]),
            false => setting_group(offered.into_iter().map(|(index, partition)| {
                let free = match partition.disk {
                    Some(disk) => format!("{} free of {}", size_of(disk.free), size_of(disk.total)),
                    None => "size unknown".to_owned(),
                };
                setting(
                    partition.label.clone(),
                    format!(
                        "{} · {} at {}",
                        partition.fstype,
                        partition.device.display(),
                        partition.point.display()
                    ),
                    None,
                    row![
                        muted(free, theme::LABEL),
                        secondary("Add", Message::AddDrive(index)).enabled(true),
                    ]
                    .spacing(theme::MD)
                    .align_y(Alignment::Center),
                )
            })),
        };
        let add_hint = self.hint(
            "Lightforge creates a \"Lightforge\" folder on the partition and installs \
             games under it. Only mounted partitions you can write to are listed: mount the \
             drive the way you normally do and rescan. A partition is added, not a disk: a \
             drive with two partitions can hold two drives here.",
        );

        // ---- Lightforge's own directories, which follow XDG, not the
        // drives, so they are listed apart, not attributed to one.
        let mut usage = column![].spacing(theme::SM);
        match report {
            None => usage = usage.push(muted("measuring…".to_owned(), theme::LABEL)),
            Some(report) => {
                let clearing = self.cache_notice.as_deref() == Some("clearing…");
                usage = usage.push(setting_group(report.app.iter().map(|item| {
                    let action = item.clearable.then(|| {
                        secondary("Clear", Message::ClearCache).enabled(item.bytes > 0 && !clearing)
                    });
                    setting(
                        item.label.clone(),
                        tilde(&item.path),
                        None,
                        // The button's slot is kept on every row, so the
                        // sizes line up whether or not there is one.
                        row![
                            text(size_of(item.bytes)).size(theme::BODY).font(theme::MONO),
                            container(action.unwrap_or_else(|| Space::new().into()))
                                .width(Length::Fixed(ACTION_COLUMN))
                                .align_x(Alignment::End),
                        ]
                        .spacing(theme::MD)
                        .align_y(Alignment::Center),
                    )
                })));
                usage = usage.extend(self.hint("Clearing the cache removes downloaded art, installers and lists; each is \
                     fetched again when needed. Shader caches are kept: rebuilding them is \
                     what makes a game stutter.").map(Element::from));
                if let Some(notice) = &self.cache_notice {
                    usage = usage.push(muted(notice.clone(), theme::LABEL));
                }
                if self.storage_measuring {
                    usage = usage.push(muted("updating…".to_owned(), theme::CAPTION));
                }
            }
        }

        column![
            group_label("Drives", None),
            drives,
            group_label(
                "Add a drive",
                Some(secondary("Rescan drives", Message::RefreshDrives).enabled(true)),
            ),
            add,
        ]
        .extend(add_hint.map(Element::from))
        .push(group_label("Lightforge's own files", None))
        .push(usage)
        .spacing(theme::SM)
        .into()
    }

    /// One drive's card: how full it is, what Lightforge put there, and the
    /// three things you can do to it.
    pub(crate) fn drive_card<'a>(
        &'a self,
        library: &'a storage::LibraryUsage,
        same_disk_as: Option<&'a str>,
    ) -> Element<'a, Message> {
        let mut card = column![drive_title(library), muted(tilde(&library.path), theme::LABEL)]
            .spacing(theme::SM)
            .extend(drive_space(library, same_disk_as));
        // Only when true: an unclaimed library can still hold games.
        if library.state == library::State::New && library.total() == 0 {
            card = card.push(muted(
                "Nothing installed here yet. The folder is created with the first game.".to_owned(),
                theme::CAPTION,
            ));
        }

        card = card.extend(self.drive_usage(library));
        card = card.extend(self.unused_rows(library));
        card = card.push(self.drive_actions(library));
        setting_card(card)
    }

    /// One row per kind of thing on the drive, with a Clear button where it
    /// can be cleared, and what the last clear said.
    fn drive_usage<'a>(&'a self, library: &'a storage::LibraryUsage) -> Vec<Element<'a, Message>> {
        let mut rows = Vec::new();
        for item in &library.usage {
            // Nothing is a fact better told by absence than by a row of
            // zeroes with a dead button beside it.
            if item.bytes == 0 {
                continue;
            }
            let notice = self
                .drive_notice
                .as_ref()
                .filter(|(which, _)| *which == library.id)
                .map(|(_, note)| note.clone());
            let clearing = notice.as_deref() == Some("clearing…");
            let action = item.clearable.then(|| {
                secondary("Clear", Message::ClearDrive(library.id.clone()))
                    .enabled(item.bytes > 0 && !clearing)
            });
            rows.push(usage_row(
                text(item.label.clone()).size(theme::BODY).into(),
                item.bytes,
                action,
            ));
        }
        if let Some((which, note)) = &self.drive_notice {
            if *which == library.id {
                rows.push(muted(note.clone(), theme::LABEL).into());
            }
        }
        rows
    }

    /// Open, make default, remove.
    fn drive_actions<'a>(&'a self, library: &'a storage::LibraryUsage) -> Element<'a, Message> {
        let id = &library.id;
        let mut actions = row![Space::new().width(Length::Fill)]
            .spacing(theme::SM)
            .align_y(Alignment::Center);
        // `is_present`: only a drive that is away cannot be opened or made default.
        actions = actions.push(
            secondary("Open folder", Message::OpenDrive(id.clone()))
                .enabled(library.state.is_present()),
        );
        actions = actions.push(
            secondary("Make default", Message::SetDefaultDrive(id.clone()))
                .enabled(!library.is_default && library.state.is_present()),
        );
        // Not the default while another exists, never the last.
        actions = actions.push(
            secondary("Remove", Message::RemoveDrive(id.clone()))
                .enabled(!library.is_default && self.config.libraries.len() > 1),
        );
        actions.into()
    }

    /// A drive card's "Unused game data" row and the line under it.
    pub(crate) fn unused_rows<'a>(&'a self, library: &'a storage::LibraryUsage) -> Vec<Element<'a, Message>> {
        let label = || text("Unused game data").size(theme::BODY).into();
        let Some(state) = self.unused.get(&library.id) else {
            return Vec::new();
        };
        match state {
            UnusedData::Checking => vec![muted("checking for unused game data…".to_owned(), theme::CAPTION).into()],
            // Nothing worth a button: told by absence, like an empty cache.
            UnusedData::Ready(plan) if plan.freeable < UNUSED_WORTH_SHOWING => Vec::new(),
            UnusedData::Ready(plan) => {
                let busy = self
                    .config
                    .libraries
                    .iter()
                    .find(|l| l.id == library.id)
                    .is_some_and(|l| self.downloading_into(&self.layout.wow_install_in(l)));
                let held = self.store_writer().is_some() || self.launching.is_some();
                let action =
                    secondary("Free up", Message::FreeUnused(library.id.clone())).enabled(!busy && !held);
                let note = match busy {
                    true => "Finish or cancel the download on this drive first.".to_owned(),
                    false if plan.moved > 0 => format!(
                        "Left behind by patches and versions that are gone. {} of game data \
                         still in use is copied first. The game and Battle.net have to be closed.",
                        size_of(plan.moved)
                    ),
                    false => "Left behind by patches and versions that are gone. The game and \
                              Battle.net have to be closed."
                        .to_owned(),
                };
                vec![
                    usage_row(label(), plan.freeable, Some(action)),
                    muted(note, theme::CAPTION).into(),
                ]
            }
            UnusedData::Freeing { copied, total } => {
                let done = copied.load(std::sync::atomic::Ordering::Relaxed).min(*total);
                vec![muted(
                    format!("freeing unused game data… {} of {} copied", size_of(done), size_of(*total)),
                    theme::LABEL,
                )
                .into()]
            }
            UnusedData::Said(note) => vec![muted(note.clone(), theme::LABEL).into()],
        }
    }
}

/// The drive's name, its signs, and the two letters Wine knows it by.
fn drive_title(library: &storage::LibraryUsage) -> Element<'_, Message> {
    let mut title = row![text(library.label.clone()).size(theme::HEADING)]
        .spacing(theme::SM)
        .align_y(Alignment::Center);
    if library.is_default {
        title = title.push(
            container(text("Default").size(theme::CAPTION).color(theme::ACCENT))
                .padding([2.0, theme::SM])
                .style(theme::tag(theme::ACCENT)),
        );
    }
    if let Some(reason) = library.state.reason(&library.label) {
        title = title.push(
            container(text(reason).size(theme::CAPTION).color(theme::ERROR))
                .padding([2.0, theme::SM])
                .style(theme::tag(theme::ERROR)),
        );
    }
    title = title.push(Space::new().width(Length::Fill));
    title = title.push(mono(format!(
        "{} → games   {} → wow",
        library.letters.0.to_uppercase(),
        library.letters.1.to_uppercase(),
    )));
    title.into()
}

/// How full the disk under the drive is, or why that is not shown.
fn drive_space<'a>(
    library: &'a storage::LibraryUsage,
    same_disk_as: Option<&'a str>,
) -> Vec<Element<'a, Message>> {
    match library.disk {
        // Already drawn on the card above: one bar per disk.
        Some(_) if same_disk_as.is_some() => vec![muted(
            format!(
                "Same disk as {} — {} is Lightforge's here",
                same_disk_as.unwrap_or_default(),
                size_of(library.total()),
            ),
            theme::CAPTION,
        )
        .into()],
        Some(disk) => {
            let used = disk.total.saturating_sub(disk.free);
            let fraction = match disk.total {
                0 => 0.0,
                total => used as f32 / total as f32,
            };
            vec![
                progress_bar(0.0..=1.0, fraction).girth(6).into(),
                muted(
                    format!(
                        "{} free of {} — {} is Lightforge's",
                        size_of(disk.free),
                        size_of(disk.total),
                        size_of(library.total()),
                    ),
                    theme::CAPTION,
                )
                .into(),
            ]
        }
        // An absent drive is not measured: its mount point shows what is beneath.
        None => vec![muted(
            "Nothing is measured while the drive is away. The games on it are \
             untouched, and come back with it."
                .to_owned(),
            theme::CAPTION,
        )
        .into()],
    }
}
