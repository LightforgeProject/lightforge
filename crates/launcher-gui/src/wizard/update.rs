//! What the wizard does with each of its messages.

use super::adopt::{adopt_taken, link_hand_offs, mark_known};
use super::*;

/// Handle one wizard message. Returns the task it wants run.
pub fn update(app: &mut Lightforge, msg: Msg) -> iced::Task<Message> {
    match msg {
        Msg::Open | Msg::Scan | Msg::Finish | Msg::Next | Msg::Back | Msg::Prefetched(_) => {
            flow(app, msg)
        }
        Msg::Region(_) | Msg::Locale(_) | Msg::SteamOsUse(_) | Msg::WantBattleNet(_) => {
            answer(app, msg)
        }
        Msg::Scanned(_)
        | Msg::Take(..)
        | Msg::PointInput(_)
        | Msg::Browse
        | Msg::Browsed(_)
        | Msg::Point
        | Msg::Pointed(_) => found_games(app, msg),
        Msg::AddDrive(_) | Msg::RemoveDrive(_) => drives(app, msg),
    }
}

/// Opening and closing the wizard, moving between steps, and the artwork
/// prefetch it starts on opening.
fn flow(app: &mut Lightforge, msg: Msg) -> iced::Task<Message> {
    match msg {
        Msg::Open => {
            app.wizard = Some(Wizard::default());
            prefetch(app)
        }

        Msg::Scan => {
            app.wizard = Some(Wizard { step: Step::Found, scanning: true, scan_only: true, ..Wizard::default() });
            scan(app)
        }

        Msg::Finish => {
            let pending = app.wizard.as_ref().map(|w| w.pending.clone()).unwrap_or_default();
            app.wizard = None;
            // Marks the first run as seen; Settings keeps the button for a
            // second one.
            app.config.setup_complete = true;
            // A fresh install has nothing to be told what is new about: the
            // notes describe the version it has just been set up with. Marked
            // seen here so the *second* start does not open a popup about the
            // build the wizard was running.
            app.config.patch_notes_seen = Some(env!("CARGO_PKG_VERSION").to_owned());
            app.save_config();
            // Played from Game Mode, so it belongs in the Steam library
            // there. A failure is said in Settings, under the button that
            // tries again; setup is done either way.
            if app.console() {
                app.add_to_steam();
            }
            if !pending.is_empty() {
                link_hand_offs(app, &pending);
            }
            app.go_to(crate::Screen::Dashboard);
            // The band and the panel both change shape when a game is adopted
            // or Battle.net is declined, so the product on screen is re-read.
            app.reload(app.selected.code.clone())
        }

        Msg::Next => advance(app),

        // There is nothing behind the one slide a scan shows: its Back is
        // the way out, and Escape with it.
        Msg::Back if app.wizard.as_ref().is_some_and(|w| w.scan_only) => {
            app.wizard = None;
            app.reload(app.selected.code.clone())
        }

        Msg::Back => {
            if let Some(wizard) = app.wizard.as_mut() {
                if let Some(back) = wizard.step.back() {
                    wizard.step = back;
                }
            }
            iced::Task::none()
        }

        Msg::Prefetched(count) => {
            if let Some(wizard) = app.wizard.as_mut() {
                wizard.prefetched = Some(count);
            }
            iced::Task::none()
        }

        other => unreachable!("{other:?} is not a step change"),
    }
}

/// The choices the steps ask for, each saved as soon as it is made.
fn answer(app: &mut Lightforge, msg: Msg) -> iced::Task<Message> {
    match msg {
        Msg::Region(region) => {
            app.config.region = region;
            app.save_config();
            // Everything already fetched was fetched against the old region's
            // version service, so the panel has to ask again.
            app.reload(app.selected.code.clone())
        }

        Msg::Locale(locale) => {
            app.config.locale = locale;
            app.save_config();
            iced::Task::none()
        }

        Msg::SteamOsUse(answer) => {
            app.config.steamos_use = Some(answer);
            app.save_config();
            // The window changes with the answer, fullscreen for a console,
            // so whoever answered sees what they chose straight away.
            app.window_mode()
        }

        Msg::WantBattleNet(want) => {
            if let Some(wizard) = app.wizard.as_mut() {
                wizard.battle_net = want;
            }
            app.config.battle_net_declined = !want;
            app.save_config();
            // What is outstanding changed, so what the Install step draws has
            // to be read again.
            app.setup_status()
        }

        other => unreachable!("{other:?} is not an answer"),
    }
}

/// The Found step: the scan's rows, and a folder the user points at.
fn found_games(app: &mut Lightforge, msg: Msg) -> iced::Task<Message> {
    match msg {
        Msg::Scanned(found) => {
            if let Some(wizard) = app.wizard.as_mut() {
                wizard.scanning = false;
                wizard.scanned = true;
                wizard.rows = rows_of(found);
            }
            mark_known(app);
            iced::Task::none()
        }

        Msg::Take(at, take) => {
            if let Some(wizard) = app.wizard.as_mut() {
                if let Some(row) = wizard.rows.get_mut(at) {
                    row.take = take && row.blocked.is_none();
                }
            }
            iced::Task::none()
        }

        Msg::PointInput(path) => {
            if let Some(wizard) = app.wizard.as_mut() {
                wizard.pointed = path;
                wizard.point_error = None;
            }
            iced::Task::none()
        }

        Msg::Browse => iced::Task::perform(
            // The XDG portal through `ashpd`, as the screenshot picker does;
            // see `update/report.rs` for why not `rfd`.
            async {
                let request = ashpd::desktop::file_chooser::OpenFileRequest::default()
                    .title("Choose a Wine prefix or a game folder")
                    .modal(true)
                    .directory(true)
                    .multiple(false);
                let files = request.send().await.ok()?.response().ok()?;
                files.uris().first().and_then(|uri| uri.to_file_path().ok())
            },
            |path| Message::Wizard(Msg::Browsed(path)),
        ),

        Msg::Browsed(path) => {
            let Some(path) = path else { return iced::Task::none() };
            if let Some(wizard) = app.wizard.as_mut() {
                wizard.pointed = path.display().to_string();
            }
            update(app, Msg::Point)
        }

        Msg::Point => {
            let Some(wizard) = app.wizard.as_mut() else {
                return iced::Task::none();
            };
            if wizard.pointing {
                return iced::Task::none();
            }
            let path = match crate::parse_path(&wizard.pointed) {
                Ok(path) => path,
                Err(e) => {
                    wizard.point_error = Some(e);
                    return iced::Task::none();
                }
            };
            wizard.pointing = true;
            wizard.point_error = None;
            iced::Task::perform(
                async move {
                    tokio::task::spawn_blocking(move || discover::inspect_pointed(&path)).await.ok().flatten()
                },
                |found| Message::Wizard(Msg::Pointed(found)),
            )
        }

        Msg::Pointed(found) => {
            let Some(wizard) = app.wizard.as_mut() else {
                return iced::Task::none();
            };
            wizard.pointing = false;
            match found {
                Some(found) if !found.is_empty() => {
                    let fresh = rows_of(vec![found]);
                    // Only what is not already listed: pointing at a prefix the
                    // scan found would otherwise double every row in it.
                    for row in fresh {
                        let already = wizard.rows.iter().any(|have| match (&have.kind, &row.kind) {
                            (RowKind::Wow(a), RowKind::Wow(b)) => a.path == b.path,
                            (RowKind::Game(a), RowKind::Game(b)) => a.path == b.path,
                            _ => false,
                        });
                        if !already {
                            wizard.rows.push(row);
                        }
                    }
                    wizard.pointed.clear();
                    wizard.point_error = None;
                    mark_known(app);
                }
                _ => {
                    wizard.point_error = Some(
                        "nothing of Blizzard's in there — point at a Wine prefix, or at a \
                         World of Warcraft folder with a .build.info in it"
                            .to_owned(),
                    )
                }
            }
            iced::Task::none()
        }

        other => unreachable!("{other:?} is not about found games"),
    }
}

/// The Place step's drive list.
fn drives(app: &mut Lightforge, msg: Msg) -> iced::Task<Message> {
    match msg {
        Msg::AddDrive(index) => {
            let outcome = app.add_drive(index);
            if outcome.is_ok() {
                app.refresh_drives();
            }
            if let Some(wizard) = app.wizard.as_mut() {
                wizard.drive_error = outcome.err();
            }
            iced::Task::none()
        }

        Msg::RemoveDrive(id) => {
            // `refresh_drives` puts the partition back in the list below, so
            // a drive added by mistake can be added again straight away.
            let outcome = app.remove_drive(&id);
            if outcome.is_ok() {
                app.refresh_drives();
            }
            if let Some(wizard) = app.wizard.as_mut() {
                wizard.drive_error = outcome.err();
            }
            iced::Task::none()
        }

        other => unreachable!("{other:?} is not about drives"),
    }
}

/// Leave the current step, doing whatever that step's answers asked for.
fn advance(app: &mut Lightforge) -> iced::Task<Message> {
    let Some(step) = app.wizard.as_ref().map(|w| w.step) else {
        return iced::Task::none();
    };
    match step {
        // Entering the scan: it reads two files per prefix and measures what it
        // finds, so it runs off the UI thread like any other walk of a disk.
        Step::Place => {
            let fresh = app.wizard.as_ref().is_some_and(|w| !w.scanned);
            if let Some(wizard) = app.wizard.as_mut() {
                wizard.step = Step::Found;
                wizard.scanning = fresh;
            }
            match fresh {
                true => scan(app),
                false => iced::Task::none(),
            }
        }

        // After setup the prefix and Battle.net are there already, so a
        // found game is linked as it is added, and the slide stays up to say
        // how that went.
        Step::Found if app.wizard.as_ref().is_some_and(|w| w.scan_only) => {
            adopt_taken(app);
            let pending = app.wizard.as_mut().map(|w| std::mem::take(&mut w.pending)).unwrap_or_default();
            if !pending.is_empty() {
                link_hand_offs(app, &pending);
            }
            iced::Task::none()
        }

        Step::Found => {
            adopt_taken(app);
            if let Some(wizard) = app.wizard.as_mut() {
                wizard.step = Step::Install;
            }
            app.setup_status()
        }

        other => {
            if let Some(wizard) = app.wizard.as_mut() {
                if let Some(next) = other.next() {
                    wizard.step = next;
                }
            }
            iced::Task::none()
        }
    }
}

/// Look through the usual Wine prefixes, off the UI thread: it reads two
/// files per prefix and measures what it finds, like any other walk of a disk.
fn scan(app: &Lightforge) -> iced::Task<Message> {
    let prefix = app.layout.prefix();
    iced::Task::perform(
        async move {
            tokio::task::spawn_blocking(move || discover::scan(&prefix)).await.unwrap_or_default()
        },
        |found| Message::Wizard(Msg::Scanned(found)),
    )
}
