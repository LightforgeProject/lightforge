use super::servers_open;

#[test]
fn the_servers_caption_is_built_from_blizzards_date() {
    assert_eq!(servers_open("Available October 19, 2026"), "Servers will be available on October 19");
    // Anything else is Blizzard's words, untouched.
    assert_eq!(servers_open("Coming soon"), "Coming soon");
}

use super::*;

fn chord(text: &str, modifiers: iced::keyboard::Modifiers) -> bool {
    hints_chord(&iced::keyboard::Key::Character(text.into()), modifiers)
}

#[test]
fn a_date_column_is_a_day_and_a_month_or_the_string_itself() {
    // What the proxy sends, straight from CurseForge.
    assert_eq!(short_day("2026-10-04T20:10:34.237Z"), "4 Oct");
    // What our own ledger stamps.
    assert_eq!(short_day("2026-01-09"), "9 Jan");
    assert_eq!(short_day("2026-12-31"), "31 Dec");
    // A shape we do not know shows as itself. A column that silently
    // blanks is a column that looks broken, and the raw value at least
    // says what the provider sent.
    assert_eq!(short_day("whenever"), "whenever");
    assert_eq!(short_day("2026-13-01"), "2026-13-01", "no such month");
}

#[test]
fn the_handoff_button_only_promises_what_is_known() {
    let act = |family: bool, installed: bool| match (family, installed) {
        (false, _) => HandoffAct::OpenClient,
        (true, true) => HandoffAct::Play,
        (true, false) => HandoffAct::Install,
    };

    assert_eq!(act(true, true), HandoffAct::Play);
    assert_eq!(act(true, false), HandoffAct::Install);
    // No family means no tile can be named, installed or not: the client
    // resolves an unknown one to Warcraft III, so neither Play nor
    // Install would be true.
    assert_eq!(act(false, true), HandoffAct::OpenClient);
    assert_eq!(act(false, false), HandoffAct::OpenClient);

    // Label, colour and message agree, which is the reason for the type.
    assert_eq!(HandoffAct::Play.label(), "Play via Battle.net");
    assert_eq!(HandoffAct::Install.label(), "Install via Battle.net");
    assert_eq!(HandoffAct::OpenClient.label(), "Open Battle.net");
    assert_eq!(HandoffAct::Play.colour(), Some(theme::ACCENT));
    assert_eq!(
        HandoffAct::Install.colour(),
        Some(theme::PAUSE),
        "blue is reserved for a game that can actually be played"
    );
    assert_eq!(
        HandoffAct::OpenClient.colour(),
        None,
        "opening the client is not an act on this game, so it draws outlined"
    );
    assert!(matches!(HandoffAct::Install.message(), Message::InstallViaBattleNet));
}

#[test]
fn a_download_count_is_short_without_lying() {
    assert_eq!(downloads_of(0), "0");
    assert_eq!(downloads_of(847), "847", "small counts stay exact");
    assert_eq!(downloads_of(999), "999");
    assert_eq!(downloads_of(1_000), "1K", "a whole value drops the decimal");
    assert_eq!(downloads_of(24_138), "24.1K");
    assert_eq!(downloads_of(98_400_000), "98.4M");
    assert_eq!(downloads_of(142_000_000), "142M", "no decimal past a hundred");
    assert_eq!(downloads_of(300_000_000), "300M");
    assert_eq!(downloads_of(1_500_000_000), "1.5B");
    // Rounding up a magnitude must not leave a ".0" behind.
    assert_eq!(downloads_of(99_950_000), "100M");
    assert_eq!(downloads_of(999_999_999), "1B", "promoted, not four digits");
    assert_eq!(downloads_of(999_499_999), "999M", "and not promoted early");
    assert_eq!(downloads_of(999_499), "999K");
    assert_eq!(downloads_of(999_500), "1M");
}

#[test]
fn the_hints_need_all_three_modifiers() {
    use iced::keyboard::Modifiers;
    let all = Modifiers::CTRL | Modifiers::ALT | Modifiers::SHIFT;
    assert!(chord("h", all));
    assert!(!chord("h", Modifiers::CTRL | Modifiers::SHIFT), "Ctrl+Shift+H is a near miss");
    assert!(!chord("h", Modifiers::CTRL), "and plain Ctrl+H is someone else's shortcut");
    assert!(!chord("h", Modifiers::default()), "typing an h is just typing");
    assert!(!chord("g", all), "another letter is not the chord");
}

/// A band wide enough for four 54pt tiles and their gaps.
fn room_for(tiles: usize) -> f32 {
    54.0 * tiles as f32 + theme::SM * tiles.saturating_sub(1) as f32
}

#[test]
fn a_band_with_room_to_spare_hides_nothing() {
    let widths = [54.0; 3];
    let (shown, hidden) = divide_strip(&widths, 0, room_for(3));
    assert_eq!(shown, vec![0, 1, 2]);
    assert!(hidden.is_empty(), "nothing overflows when everything fits");
}

#[test]
fn the_overflow_button_is_paid_for_out_of_the_band() {
    // Room for four tiles exactly, five to place: the fifth does not fit,
    // and making space for the button costs a fourth tile as well.
    let widths = [54.0; 5];
    let (shown, hidden) = divide_strip(&widths, 0, room_for(4));
    assert_eq!(shown, vec![0, 1, 2]);
    assert_eq!(hidden, vec![3, 4]);
}

#[test]
fn the_selected_game_is_never_the_one_that_disappears() {
    let widths = [54.0; 5];
    let (shown, hidden) = divide_strip(&widths, 4, room_for(4));
    assert!(shown.contains(&4), "the game on screen stays in the band");
    assert_eq!(shown.last(), Some(&4), "it takes the place nearest the menu");
    assert!(!hidden.contains(&4));
    assert_eq!(hidden, vec![2, 3], "the tiles it displaced are the ones behind the menu");
}

#[test]
fn a_band_with_almost_no_room_still_shows_the_selected_game() {
    let widths = [54.0; 4];
    let (shown, hidden) = divide_strip(&widths, 2, 10.0);
    assert_eq!(shown, vec![2], "something is always lit");
    assert_eq!(hidden, vec![0, 1, 3]);
}

#[test]
fn a_wide_name_tile_costs_more_of_the_band_than_an_icon() {
    // Not a layout assertion, just that the estimate scales with the
    // name, since that is what keeps a long label from being cut off.
    let widths = [54.0, 200.0, 54.0];
    let (shown, hidden) = divide_strip(&widths, 0, room_for(4));
    assert_eq!(shown, vec![0], "the wide tile does not fit beside the first");
    assert_eq!(hidden, vec![1, 2]);
}

fn planned(remaining: u64, stored: u64) -> TransferState {
    TransferState {
        remaining: Some(remaining),
        progress: lightforge_core::tact::download::Progress {
            bytes_stored: stored,
            ..Default::default()
        },
        ..TransferState::default()
    }
}

#[test]
fn an_update_measures_what_it_fetches_not_the_install() {
    // The symptom this guards: a 10 GiB update to a 90 GiB install opened
    // at "80 of 90 GiB", counting the untouched install as progress.
    let gib = 1024u64 * 1024 * 1024;
    let job = Job { total: Some(10 * gib), reached: 0 };

    let start = job_progress(job, &planned(10 * gib, 0), 0);
    assert_eq!((start.downloaded_bytes, start.total_bytes), (0, 10 * gib));

    let later = job_progress(job, &planned(10 * gib, 4 * gib), 0);
    assert_eq!(later.downloaded_bytes, 4 * gib);
}

#[test]
fn a_resumed_run_continues_its_job_rather_than_restarting() {
    // Pause 3 GiB into a 10 GiB job: the next run plans only the 7 GiB
    // still missing, and the bar must carry on from 3, not from 0.
    let gib = 1024u64 * 1024 * 1024;
    let paused = Download {
        code: ProductCode::Retail,
        label: String::new(),
        stage: Stage::Paused(state::Progress {
            downloaded_bytes: 3 * gib,
            total_bytes: 10 * gib,
            bytes_per_second: 0,
        }),
        sample: TransferState::default(),
        rate: Rate::default(),
        job: Job { total: Some(10 * gib), reached: 0 },
    };
    let job = paused.resumed();

    // Still planning: hold where the pause left it.
    let planning = job_progress(job, &TransferState::default(), 0);
    assert_eq!((planning.downloaded_bytes, planning.total_bytes), (3 * gib, 10 * gib));

    // Planned: a run byte moves the job readout by one byte.
    let running = job_progress(job, &planned(7 * gib, gib), 0);
    assert_eq!((running.downloaded_bytes, running.total_bytes), (4 * gib, 10 * gib));
}

#[test]
fn progress_stays_run_relative_until_the_plan_is_known() {
    let sample = TransferState {
        progress: lightforge_core::tact::download::Progress {
            bytes_stored: 500,
            bytes_total: 1000,
            ..Default::default()
        },
        ..TransferState::default()
    };
    let p = job_progress(Job::default(), &sample, 0);
    assert_eq!((p.downloaded_bytes, p.total_bytes), (500, 1000));
}

/// Scrolled to the end, the index followed whichever section reached
/// the top last, Storage, with Setup and About on screen below it.
#[test]
fn the_index_follows_the_page_to_its_last_section() {
    let at = |y: f32, h: f32| iced::Rectangle { x: 0.0, y, width: 800.0, height: h };
    let layout = SettingsLayout {
        top: 0.0,
        sections: vec![
            (SettingsTab::General, at(0.0, 600.0)),
            (SettingsTab::Storage, at(600.0, 900.0)),
            (SettingsTab::Setup, at(1500.0, 300.0)),
            (SettingsTab::About, at(1800.0, 100.0)),
        ],
    };
    // 1900 tall, 700 shown: the page ends at 1200.
    let read = |y| layout.reading_at(y, 700.0, 1900.0, SettingsTab::General);
    assert_eq!(read(0.0), Some(SettingsTab::General));
    assert_eq!(read(650.0), Some(SettingsTab::Storage));
    assert_eq!(read(1200.0), Some(SettingsTab::About), "the bottom marks the last section");
    // On the way there Setup has its turn.
    assert!((1000..1200).any(|y| read(y as f32) == Some(SettingsTab::Setup)));
    // A click on Setup lands at the bottom and keeps its mark.
    assert_eq!(layout.reading_at(1200.0, 700.0, 1900.0, SettingsTab::Setup), Some(SettingsTab::Setup));
}

/// A game installed before Lightforge reached Battle.net's registry only
/// once the client had run, and the tile kept saying Install until a
/// restart. The watch re-reads on a changed timestamp, and stands down
/// once the client it saw has gone.
#[test]
fn the_registry_is_read_again_when_battle_net_changes_it() {
    let (mut app, _) = Lightforge::new();
    assert!(app.registry_watch.is_some(), "watched from the start");
    let t0 = std::time::SystemTime::UNIX_EPOCH + Duration::from_secs(1);
    let t1 = t0 + Duration::from_secs(5);

    let _ = app.update(Message::RegistryPolled(Some(t0), Some(true)));
    assert_eq!(app.registry_watch.as_ref().unwrap().stamp, Some(t0));
    assert!(app.registry_watch.as_ref().unwrap().seen_client);

    // Battle.net wrote the file: the stamp moves, the watch keeps going.
    let _ = app.update(Message::RegistryPolled(Some(t1), None));
    assert_eq!(app.registry_watch.as_ref().unwrap().stamp, Some(t1));

    // The client it saw has closed: nothing left to watch for.
    let _ = app.update(Message::RegistryPolled(Some(t1), Some(false)));
    assert!(app.registry_watch.is_none());

    // A client not up *yet* is not a reason to stop: Wine takes seconds.
    app.registry_watch = Some(RegistryWatch::new());
    let _ = app.update(Message::RegistryPolled(None, Some(false)));
    assert!(app.registry_watch.is_some());
}

/// A game whose drive is away is held, whatever the last refresh thought
/// of it, and held in a way that cannot turn into a download. Resolving
/// a missing drive by re-fetching 150 GB onto the system disk is the one
/// outcome this whole mechanism exists to prevent.
#[test]
fn a_game_on_an_absent_drive_is_held_rather_than_played_or_downloaded() {
    let (mut app, _) = Lightforge::new();
    app.dirs = None;
    app.queue.clear();
    let code = app.selected.code.clone();

    // An install that is complete and current, so core would say Play.
    app.local = Local {
        build_key: Some("abc".to_owned()),
        version: Some("11.2.7.65000".to_owned()),
        executable: Some(PathBuf::from("/nowhere/Wow.exe")),
        store_files: 12,
    };
    assert!(matches!(app.action(), ActionState::Play));

    // The same product, on a library that was set up once and is not here.
    let mut away =
        library::Library::on_partition(std::path::Path::new("/nowhere"), "nvme2", None);
    away.claimed = true;
    let id = away.id.clone();
    app.config.libraries = vec![away];
    if let Some(product) = app.config.product_mut(&code) {
        product.profile.library = Some(id);
    }
    app.check_drives();

    match app.action() {
        ActionState::Offline { drive } => assert_eq!(drive, "nvme2"),
        other => panic!("expected the drive to hold the game, got {other:?}"),
    }
    // And the download path refuses on its own, because the queue is
    // restored and resumed without anyone pressing the button.
    let _ = app.enqueue();
    assert!(app.queue.is_empty(), "a download was queued onto a drive that is not there");
    assert_eq!(app.error.as_ref().map(|e| e.message.as_str()), Some("nvme2 is not mounted"));
}

/// Dragging a tile moves its whole line, branches and all, and only in
/// the direction the cursor is heading; a press and release without
/// travel is still a click.
#[test]
fn dragging_a_tile_reorders_the_band() {
    let (mut app, _) = Lightforge::new();
    app.dirs = None;
    app.queue.clear();

    let retail = Choice { code: ProductCode::Retail, label: "Retail".to_owned() };
    app.choices = vec![
        retail.clone(),
        Choice { code: ProductCode::Ptr, label: "PTR".to_owned() },
        Choice { code: ProductCode::ClassicEra, label: "Classic Era".to_owned() },
        Choice { code: ProductCode::EraPtr, label: "Era PTR".to_owned() },
        Choice { code: ProductCode::Classic, label: "Classic".to_owned() },
    ];
    app.config.products = app
        .choices
        .iter()
        .map(|c| Product::new(c.code.clone(), c.label.clone()))
        .collect();
    app.selected = retail;

    let classic = app.catalog.line_of(&ProductCode::Classic);
    let era = app.catalog.line_of(&ProductCode::ClassicEra);
    let retail_line = app.catalog.line_of(&ProductCode::Retail);

    // Held over Classic, nudged inside the slop, then dragged left over
    // Era (heading right: no move) and on over Retail (heading left).
    let _ = app.update(Message::TileEntered(classic.clone()));
    let _ = app.update(Message::TileGrabbed(classic.clone()));
    let _ = app.update(Message::TileDragged(300.0));
    let _ = app.update(Message::TileDragged(297.0));
    assert!(!app.tile_drag.as_ref().unwrap().moving, "a twitch started a drag");
    let _ = app.update(Message::TileEntered(era.clone()));
    let _ = app.update(Message::TileDragged(305.0));
    assert_eq!(app.lines().last(), Some(&classic), "moved against the direction of travel");
    let _ = app.update(Message::TileDragged(250.0));
    let _ = app.update(Message::TileEntered(retail_line.clone()));
    let _ = app.update(Message::TileDragged(200.0));
    let _ = app.update(Message::TileDropped);

    let order: Vec<&ProductCode> = app.config.products.iter().map(|p| &p.code).collect();
    assert_eq!(order, vec![
        &ProductCode::Classic,
        &ProductCode::Retail,
        &ProductCode::Ptr,
        &ProductCode::ClassicEra,
        &ProductCode::EraPtr,
    ]);
    assert_eq!(app.lines(), vec![classic.clone(), retail_line, era.clone()]);
    assert!(app.tile_drag.is_none());

    // A click: press and release on Era with no travel selects it.
    let _ = app.update(Message::TileEntered(era.clone()));
    let _ = app.update(Message::TileGrabbed(era.clone()));
    let _ = app.update(Message::TileDropped);
    assert_eq!(app.catalog.line_of(&app.selected.code), era);
}

/// The mouse moving takes the ring away, and the next key brings it back
/// where it was rather than to the start of the page: Steam's keyboard
/// moves the pointer as it closes, and the ring went to the logo.
#[test]
fn the_ring_comes_back_where_the_mouse_took_it() {
    let (mut app, _) = Lightforge::new();
    app.dirs = None;
    app.splash = None;
    let at = |x, y| iced::Rectangle { x, y, width: 80.0, height: 30.0 };
    let key = |name: &str| focus::Target::Key(name.to_owned());
    focus::reset();
    focus::put(focus::Target::AppMenu, at(10.0, 10.0), focus::Press::Send(Message::MenuClick), 0);
    focus::put(key("frame cap"), at(400.0, 600.0), focus::Press::Click, 0);
    app.focus = Some(key("frame cap"));

    let _ = app.update(Message::FocusHidden);
    assert_eq!(app.focus, None);
    let _ = app.update(Message::FocusStep(focus::Step::Down));
    assert_eq!(app.focus, Some(key("frame cap")));

    // Gone from the page by then: the start of the page, as before.
    let _ = app.update(Message::FocusHidden);
    focus::reset();
    focus::put(focus::Target::AppMenu, at(10.0, 10.0), focus::Press::Send(Message::MenuClick), 0);
    let _ = app.update(Message::FocusStep(focus::Step::Down));
    assert_eq!(app.focus, Some(focus::Target::AppMenu));
}

/// The Deck's controls arrive as keys: the D-pad as arrows, A as Enter,
/// B as Escape. They have to reach a game and back out of a dialog with
/// no mouse at all.
/// Buttons that are gone once pressed. Install inside the addon details:
/// the ring moves on inside the dialog, and closing the dialog still takes
/// it back to the Details button that opened it. Keep on an addon row:
/// the ring lands on the Remove beside it, not at the top of the page.
#[test]
fn a_button_gone_once_pressed_leaves_the_ring_nearby() {
    let (mut app, _) = Lightforge::new();
    app.dirs = None;
    app.splash = None;
    let at = |x, y| iced::Rectangle { x, y, width: 80.0, height: 30.0 };
    let key = |name: &str| focus::Target::Key(name.to_owned());
    let send = || focus::Press::Send(Message::MenuClick);
    let settle = |app: &mut Lightforge| {
        app.focus_entering = FOCUS_ENTER_FRAMES;
        let _ = app.update(Message::FocusSettle);
    };

    // The dialog opens over the page; the ring goes into it.
    focus::reset();
    focus::put(focus::Target::AppMenu, at(10.0, 10.0), send(), 0);
    focus::put(key("details"), at(600.0, 300.0), send(), 0);
    focus::put(key("install"), at(500.0, 500.0), send(), 1);
    focus::put(key("close"), at(600.0, 500.0), send(), 1);
    app.focus = Some(key("details"));
    settle(&mut app);
    assert_eq!(app.focus, Some(key("install")));

    // Install pressed and gone: on to Close, still in the dialog.
    focus::reset();
    focus::put(focus::Target::AppMenu, at(10.0, 10.0), send(), 0);
    focus::put(key("details"), at(600.0, 300.0), send(), 0);
    focus::put(key("close"), at(600.0, 500.0), send(), 1);
    settle(&mut app);
    assert_eq!(app.focus, Some(key("close")));

    // The dialog closed: back on Details.
    focus::reset();
    focus::put(focus::Target::AppMenu, at(10.0, 10.0), send(), 0);
    focus::put(key("details"), at(600.0, 300.0), send(), 0);
    settle(&mut app);
    assert_eq!(app.focus, Some(key("details")));
    // Keep pressed on a row, and gone with the question: the ring lands
    // on the Remove that comes back beside it, not at the top of the page.
    app.focus_return = None;
    focus::reset();
    focus::put(focus::Target::AppMenu, at(10.0, 10.0), send(), 0);
    focus::put(key("delete"), at(600.0, 400.0), send(), 0);
    focus::put(key("keep"), at(690.0, 400.0), send(), 0);
    app.focus = Some(key("keep"));
    let _ = app.update(Message::FocusPress);
    focus::reset();
    focus::put(focus::Target::AppMenu, at(10.0, 10.0), send(), 0);
    focus::put(key("remove"), at(600.0, 400.0), send(), 0);
    settle(&mut app);
    assert_eq!(app.focus, Some(key("remove")));
}

#[test]
fn right_from_the_settings_index_goes_in_and_left_comes_back_out() {
    let (mut app, _) = Lightforge::new();
    app.dirs = None;
    app.splash = None;
    app.wizard = None;
    app.screen = Screen::Settings;
    let at = |x, y| iced::Rectangle { x, y, width: 80.0, height: 30.0 };
    let send = || focus::Press::Send(Message::MenuClick);
    let tab = focus::Target::SettingsTab(SettingsTab::ALL[0]);
    let switch = focus::Target::Key("switch".to_owned());

    focus::reset();
    focus::put(tab.clone(), at(20.0, 100.0), focus::Press::Send(Message::SettingsTab(SettingsTab::ALL[0])), 0);
    focus::put_in(SETTINGS_PAGE, switch.clone(), at(400.0, 100.0), send());
    app.focus = Some(tab.clone());

    // Right is A here: into the section, once it has been drawn.
    let _ = app.update(Message::FocusStep(focus::Step::Right));
    app.focus_entering = FOCUS_ENTER_FRAMES;
    let _ = app.update(Message::FocusSettle);
    assert_eq!(app.focus, Some(switch));

    // Nothing further left inside the section: back onto its entry, as B.
    let _ = app.update(Message::FocusStep(focus::Step::Left));
    assert_eq!(app.focus, Some(tab));
}

#[test]
fn the_keyboard_alone_picks_a_game_and_closes_the_release_notes() {
    let (mut app, _) = Lightforge::new();
    app.dirs = None;
    app.queue.clear();
    app.wizard = None;
    app.splash = None;
    app.update_notice = None;
    app.screen = Screen::Dashboard;

    let retail = Choice { code: ProductCode::Retail, label: "Retail".to_owned() };
    app.choices = vec![
        retail.clone(),
        Choice { code: ProductCode::ClassicEra, label: "Classic Era".to_owned() },
    ];
    app.config.products = app
        .choices
        .iter()
        .map(|c| Product::new(c.code.clone(), c.label.clone()))
        .collect();
    app.selected = retail;
    let retail_line = app.catalog.line_of(&ProductCode::Retail);
    let era = app.catalog.line_of(&ProductCode::ClassicEra);

    // What a frame would have recorded: where each control was drawn,
    // and in which layer.
    let at = |x: f32, y: f32| iced::Rectangle { x, y, width: 56.0, height: 40.0 };
    let send = focus::Press::Send;
    let page = || {
        focus::reset();
        focus::put(focus::Target::AppMenu, at(40.0, 10.0), send(Message::ToggleAppMenu), 0);
        focus::put(
            focus::Target::Tile(retail_line.clone()),
            at(110.0, 60.0),
            send(Message::LineSelected(retail_line.clone())),
            0,
        );
        focus::put(focus::Target::Tile(era.clone()), at(180.0, 60.0), send(Message::LineSelected(era.clone())), 0);
        focus::put(focus::Target::Primary, at(980.0, 660.0), send(Message::Primary), 0);
    };

    // Release notes up, over the page: the first key lands on Got it,
    // the page beneath is out of reach, and B closes them.
    page();
    focus::put(focus::Target::NotesDismiss, at(700.0, 500.0), send(Message::DismissPatchNotes), 1);
    app.patch_notes = Some(PatchNotes {
        release: lightforge_core::releases::Release { version: "9.9.9".to_owned(), intro: None, notes: vec![] },
        mute: false,
    });
    let _ = app.update(Message::FocusStep(focus::Step::Down));
    assert_eq!(app.focus, Some(focus::Target::NotesDismiss));
    let _ = app.update(Message::FocusStep(focus::Step::Up));
    assert_eq!(app.focus, Some(focus::Target::NotesDismiss), "the ring left the dialog");
    let _ = app.update(Message::FocusBack);
    assert!(app.patch_notes.is_none(), "Escape left the notes up");

    // Gone with the dialog, the ring lands on PLAY once the page is drawn.
    page();
    let _ = app.update(Message::FocusSettle);
    assert_eq!(app.focus, Some(focus::Target::Primary));

    // Up from the first game is the wordmark above it.
    app.focus = Some(focus::Target::Tile(retail_line.clone()));
    let _ = app.update(Message::FocusStep(focus::Step::Up));
    assert_eq!(app.focus, Some(focus::Target::AppMenu));

    // Enter on the wordmark opens its menu, the ring moves into it once
    // it is drawn, and Escape brings it back to the wordmark.
    let _ = app.update(Message::FocusPress);
    assert!(app.app_menu_open);
    focus::put(focus::Target::Menu("Settings"), at(40.0, 50.0), send(Message::Show(Screen::Settings)), 1);
    let _ = app.update(Message::FocusSettle);
    assert_eq!(app.focus, Some(focus::Target::Menu("Settings")));
    let _ = app.update(Message::FocusBack);
    assert!(!app.app_menu_open);
    page();
    let _ = app.update(Message::FocusSettle);
    assert_eq!(app.focus, Some(focus::Target::AppMenu));

    app.focus = Some(focus::Target::Tile(retail_line));
    let _ = app.update(Message::FocusStep(focus::Step::Right));
    let _ = app.update(Message::FocusPress);
    assert_eq!(app.catalog.line_of(&app.selected.code), era);

    // The mouse moving takes the ring away.
    let _ = app.update(Message::FocusHidden);
    assert!(app.focus.is_none());
}

/// A tile is a line. Removing Classic Era with its PTR tracked used to
/// leave the PTR behind, so the game stayed in the band under a branch
/// name, the bug Gabriel reported as "it will still be here but only the
/// ptr".
#[test]
fn removing_a_game_takes_its_ptr_and_beta_with_it() {
    let (mut app, _) = Lightforge::new();
    app.dirs = None;
    app.queue.clear();

    let keep = Choice { code: ProductCode::Retail, label: "Retail".to_owned() };
    app.choices = vec![
        keep.clone(),
        Choice { code: ProductCode::ClassicEra, label: "Classic Era".to_owned() },
        Choice { code: ProductCode::EraPtr, label: "Era PTR".to_owned() },
    ];
    app.config.products = app
        .choices
        .iter()
        .map(|c| Product::new(c.code.clone(), c.label.clone()))
        .collect();
    app.selected = keep;
    app.error = None;

    let _ = app.update(Message::RemoveProduct(ProductCode::ClassicEra));

    let left: Vec<&ProductCode> = app.choices.iter().map(|c| &c.code).collect();
    assert_eq!(left, vec![&ProductCode::Retail], "the PTR stayed behind");
    assert!(
        app.config.products.iter().all(|p| p.code == ProductCode::Retail),
        "the config still tracks a branch of the removed line",
    );
    // Only the save complains, and only because this harness has no
    // config directory, the removal itself was not refused.
    assert_ne!(app.error.as_ref().map(|e| e.message.as_str()), Some("that is the only game left — add another first"));
}

/// The header cannot be emptied, and a line is now the unit, so the last
/// *line* is held even when it has branches that would keep `choices`
/// from going to zero.
#[test]
fn the_last_line_cannot_be_removed_even_with_its_branches_tracked() {
    let (mut app, _) = Lightforge::new();
    app.dirs = None;
    app.queue.clear();

    app.choices = vec![
        Choice { code: ProductCode::ClassicEra, label: "Classic Era".to_owned() },
        Choice { code: ProductCode::EraPtr, label: "Era PTR".to_owned() },
    ];
    app.config.products = app
        .choices
        .iter()
        .map(|c| Product::new(c.code.clone(), c.label.clone()))
        .collect();
    app.selected = app.choices[0].clone();
    app.error = None;

    let _ = app.update(Message::RemoveProduct(ProductCode::ClassicEra));

    assert_eq!(app.choices.len(), 2, "the only line was removed");
    assert_eq!(app.error.as_ref().map(|e| e.message.as_str()), Some("that is the only game left — add another first"));
}

/// A game opened from Add a game is shown, not tracked, and survives the
/// band being rebuilt; the first action on it puts it in the band.
#[test]
fn a_game_opened_from_add_a_game_joins_the_band_only_when_acted_on() {
    let (mut app, _) = Lightforge::new();
    app.dirs = None;
    app.queue.clear();

    let retail = Choice { code: ProductCode::Retail, label: "Retail".to_owned() };
    app.choices = vec![retail.clone()];
    app.config.products = vec![Product::new(ProductCode::Retail, "Retail")];
    app.selected = retail;

    let _ = app.update(Message::OpenProduct(ProductCode::ClassicEra));
    assert_eq!(app.selected.code, ProductCode::ClassicEra);
    assert_eq!(app.choices.len(), 1, "looking at a game added it to the band");
    assert!(app.config.product(&ProductCode::ClassicEra).is_none());

    // Adoption and the wizard rebuild the band; the page stays put.
    app.sync_choices();
    assert_eq!(app.selected.code, ProductCode::ClassicEra, "the preview was dropped");

    app.track_selected();
    assert!(app.choices.iter().any(|c| c.code == ProductCode::ClassicEra));
    assert!(app.config.product(&ProductCode::ClassicEra).is_some());
    app.track_selected();
    assert_eq!(app.choices.len(), 2, "tracked twice");
}

/// Remove from a tile's menu takes the line out of the band and closes
/// the menu; on the last tile it is held.
#[test]
fn a_tiles_menu_removes_its_line_and_closes() {
    let (mut app, _) = Lightforge::new();
    app.dirs = None;
    app.queue.clear();

    let retail = Choice { code: ProductCode::Retail, label: "Retail".to_owned() };
    app.choices = vec![
        retail.clone(),
        Choice { code: ProductCode::ClassicEra, label: "Classic Era".to_owned() },
    ];
    app.config.products = app
        .choices
        .iter()
        .map(|c| Product::new(c.code.clone(), c.label.clone()))
        .collect();
    app.selected = retail;

    let _ = app.update(Message::TileMenu(ProductLine::ClassicEra));
    assert_eq!(app.tile_menu, Some(ProductLine::ClassicEra));
    assert!(app.panel_open());
    // A second right-click on the same tile closes it again.
    let _ = app.update(Message::TileMenu(ProductLine::ClassicEra));
    assert!(app.tile_menu.is_none());

    let _ = app.update(Message::TileMenu(ProductLine::ClassicEra));
    let _ = app.update(Message::RemoveProduct(ProductLine::ClassicEra.head()));
    assert_eq!(app.choices.len(), 1);
    assert!(app.tile_menu.is_none(), "the menu outlived its tile");
}

/// A bug report goes nowhere without a category and a way back; feedback
/// only needs the category.
#[test]
fn a_bug_report_needs_a_category_and_a_contact() {
    let dialog = |kind, category, contact: &str| ReportDialog {
        kind,
        note: iced::widget::text_editor::Content::with_text("it broke"),
        category,
        contact: contact.to_owned(),
        tags: Vec::new(),
        shot: None,
        base: String::new(),
        debug: String::new(),
        history: Vec::new(),
        stage: ReportStage::Writing,
    };
    let addons = Some(report::Category::Addons);
    assert!(!dialog(ReportKind::Bug, None, "me@example.org").can_send());
    assert!(!dialog(ReportKind::Bug, addons, "  ").can_send());
    assert!(dialog(ReportKind::Bug, addons, "someone#1234").can_send());
    assert!(dialog(ReportKind::Feedback, addons, "").can_send());
    assert!(!dialog(ReportKind::Feedback, None, "").can_send());
}

/// The popup opens once per version and then not again, and every way of
/// saying "not now" is honoured. Each `return` in `open_patch_notes` is a
/// case that should see nothing.
#[test]
fn the_release_notes_open_once_after_an_update_and_never_uninvited() {
    let version = env!("CARGO_PKG_VERSION");
    let fresh = || {
        let (mut app, _) = Lightforge::new();
        app.dirs = None;
        app.queue.clear();
        app.config.setup_complete = true;
        app.config.patch_notes_muted = false;
        app.config.patch_notes_seen = Some("0.0.1".to_owned());
        app
    };

    // Updated from an older version: this is the one start that shows them.
    let mut app = fresh();
    app.open_patch_notes();
    assert_eq!(
        app.patch_notes.as_ref().map(|n| n.release.version.clone()),
        Some(version.to_owned()),
    );

    // Dismissed, and the version recorded, so a second start is quiet.
    let _ = app.update(Message::DismissPatchNotes);
    assert!(app.patch_notes.is_none());
    assert_eq!(app.config.patch_notes_seen.as_deref(), Some(version));
    assert!(!app.config.patch_notes_muted, "closing it is not muting it");
    app.open_patch_notes();
    assert!(app.patch_notes.is_none(), "shown twice for one version");

    // The checkbox means every future release, not just this one.
    let mut app = fresh();
    app.open_patch_notes();
    let _ = app.update(Message::MutePatchNotes(true));
    let _ = app.update(Message::DismissPatchNotes);
    assert!(app.config.patch_notes_muted);
    app.config.patch_notes_seen = Some("0.0.1".to_owned());
    app.open_patch_notes();
    assert!(app.patch_notes.is_none(), "muted and still opened");

    // A machine still in the wizard has a first run to get through, and
    // notes about a build nothing has been done with yet say nothing.
    let mut app = fresh();
    app.config.setup_complete = false;
    app.open_patch_notes();
    assert!(app.patch_notes.is_none());
}

#[test]
fn a_resume_during_the_wind_down_is_not_swallowed() {
    let (mut app, _) = Lightforge::new();
    // Nothing read from or written to the real queue file.
    app.dirs = None;
    app.queue.clear();
    let code = app.selected.code.clone();
    // Paused, then resumed before the paused run reported back: the entry
    // is already a fresh, queued one and the line is still winding down.
    app.queue.push(Download {
        code: code.clone(),
        label: String::new(),
        stage: Stage::Queued,
        sample: TransferState::default(),
        rate: Rate::default(),
        job: Job::default(),
    });
    app.winding_down = Some(code.clone());

    let stopped = Complete { complete: false, cancelled: true, files: 0, version: None };
    let _ = app.update(Message::Downloaded(code, Ok(stopped)));

    assert!(app.winding_down.is_none());
    assert!(matches!(app.queue[0].stage, Stage::Running { .. }), "the resume must start");
    assert_eq!(app.status, "downloading…");
}

#[test]
fn a_library_under_home_is_shown_short_and_one_elsewhere_is_not() {
    let home = PathBuf::from(std::env::var_os("HOME").expect("a home directory"));
    assert_eq!(
        home_relative(&home.join("Games/Blizzard/games/World of Warcraft")),
        "~/Games/Blizzard/games/World of Warcraft"
    );
    // Every part of a path on another drive says which drive it is.
    let external = PathBuf::from("/run/media/someone/ssd/games");
    assert_eq!(home_relative(&external), "/run/media/someone/ssd/games");
}

/// A queued or paused entry has no run to wind down, so its files can go
/// at once. A running one must not: the downloader still has the store
/// open, and deleting it under its own writer is the thing this defers.
#[test]
fn deleting_a_cancelled_install_waits_for_the_run_to_stop() {
    let (mut app, _) = Lightforge::new();
    app.dirs = None;
    app.queue.clear();
    let code = app.selected.code.clone();
    app.queue.push(Download {
        code: code.clone(),
        label: "Retail".to_owned(),
        stage: Stage::Queued,
        sample: TransferState::default(),
        rate: Rate::default(),
        job: Job::default(),
    });

    app.cancel_dialog = Some(CancelDialog {
        code: code.clone(),
        label: "Retail".to_owned(),
        installed: None,
    });
    let _ = app.update(Message::CancelDownloadConfirm { delete: true });

    assert!(app.queue.is_empty(), "the entry goes either way");
    assert!(
        app.delete_after_cancel.is_empty(),
        "a queued entry had no run open, so nothing is deferred"
    );
    assert_eq!(app.discarding, Some(code), "removed at once");
}

fn running(app: &mut Lightforge, code: &ProductCode) {
    let (_tx, rx) = tokio::sync::watch::channel(TransferState::default());
    app.queue.push(Download {
        code: code.clone(),
        label: "Retail".to_owned(),
        stage: Stage::Running { rx, cancel: Cancel::new() },
        sample: TransferState::default(),
        rate: Rate::default(),
        job: Job::default(),
    });
}

fn stopped(cancelled: bool) -> Result<Complete, Problem> {
    Ok(Complete { complete: false, cancelled, files: 0, version: None })
}

#[test]
fn a_running_download_is_removed_only_once_its_run_has_stopped() {
    let (mut app, _) = Lightforge::new();
    app.dirs = None;
    app.queue.clear();
    let code = app.selected.code.clone();
    running(&mut app, &code);
    app.cancel_dialog =
        Some(CancelDialog { code: code.clone(), label: "Retail".to_owned(), installed: None });

    let _ = app.update(Message::CancelDownloadConfirm { delete: true });
    assert_eq!(app.delete_after_cancel, std::slice::from_ref(&code));
    assert_eq!(app.discarding, None, "the run is still winding down");

    let _ = app.update(Message::Downloaded(code.clone(), stopped(true)));
    assert!(app.delete_after_cancel.is_empty());
    assert_eq!(app.discarding, Some(code));
}

#[test]
fn a_cancel_confirmed_after_the_install_finished_deletes_nothing() {
    let (mut app, _) = Lightforge::new();
    app.dirs = None;
    app.queue.clear();
    let code = app.selected.code.clone();
    app.cancel_dialog =
        Some(CancelDialog { code: code.clone(), label: "Retail".to_owned(), installed: None });

    let _ = app.update(Message::CancelDownloadConfirm { delete: true });
    assert!(app.delete_after_cancel.is_empty());
    assert_eq!(app.discarding, None);
}

#[test]
fn scan_and_repair_waits_for_a_running_download() {
    let (mut app, _) = Lightforge::new();
    app.dirs = None;
    app.queue.clear();
    let code = app.selected.code.clone();
    running(&mut app, &code);

    let _ = app.update(Message::ScanAndRepair);
    assert!(!app.scanning, "repair rewrites the indices the download is writing");
    assert!(app.error.is_some());
}

#[test]
fn queueing_a_product_that_is_downloading_leaves_its_run_alone() {
    let (mut app, _) = Lightforge::new();
    app.dirs = None;
    app.queue.clear();
    let code = app.selected.code.clone();
    running(&mut app, &code);

    let _ = app.enqueue_for(code.clone());
    assert_eq!(app.queue.len(), 1);
    assert!(matches!(app.queue[0].stage, Stage::Running { .. }));
}

#[test]
fn a_launch_waits_for_its_paused_download_to_come_to_rest() {
    let (mut app, _) = Lightforge::new();
    app.dirs = None;
    app.queue.clear();
    let code = app.selected.code.clone();
    running(&mut app, &code);
    app.busy = true;
    app.launching = Some(code.clone());

    // Pre-flight tripped the token and returned; the run has not stopped.
    let _ = app.update(Message::PreflightDone(Launch::Game, Ok(())));
    assert_eq!(app.launch_when_quiet, Some(Launch::Game));

    let _ = app.update(Message::Downloaded(code.clone(), stopped(true)));
    assert_eq!(app.launch_when_quiet, None, "started once the run stopped");
    assert!(app.status.starts_with("running"), "{}", app.status);
    assert_eq!(app.launching, Some(code), "still this product's session");
}

#[test]
fn a_download_a_launch_paused_is_restarted_when_the_game_exits() {
    use lightforge_core::preflight::DownloadGuard;

    let (mut app, _) = Lightforge::new();
    app.dirs = None;
    app.queue.clear();
    app.config.pause_downloads_while_playing = false;
    let code = app.selected.code.clone();
    running(&mut app, &code);
    app.downloads.register(code.clone(), Cancel::new());
    app.busy = true;
    app.launching = Some(code.clone());

    // Pre-flight pauses it, and the run reports back as cancelled.
    let mut guard = app.downloads.clone();
    assert!(guard.pause(&code).unwrap());
    let _ = app.update(Message::Downloaded(code.clone(), stopped(true)));
    assert!(matches!(app.queue[0].stage, Stage::Paused(_)));

    let _ = app.update(Message::Launched(Ok("exited".to_owned())));
    assert!(
        !matches!(app.queue[0].stage, Stage::Paused(_)),
        "queued again, or already running"
    );
}

#[test]
fn closing_waits_for_a_repair_to_finish() {
    let (mut app, _) = Lightforge::new();
    app.dirs = None;
    app.queue.clear();
    app.repairing = Some(app.selected.code.clone());

    let _ = app.update(Message::CloseRequested(iced::window::Id::unique()));
    assert!(app.closing.is_some());
}

#[test]
fn cancelling_an_update_never_deletes_anything() {
    let (mut app, _) = Lightforge::new();
    app.dirs = None;
    app.queue.clear();
    let code = app.selected.code.clone();
    app.queue.push(Download {
        code: code.clone(),
        label: "Retail".to_owned(),
        stage: Stage::Queued,
        sample: TransferState::default(),
        rate: Rate::default(),
        job: Job::default(),
    });
    app.cancel_dialog = Some(CancelDialog {
        code: code.clone(),
        label: "Retail".to_owned(),
        // Installed, so this is an update and the game plays right now.
        installed: Some("1.15.9.70003".to_owned()),
    });

    let _ = app.update(Message::CancelDownloadConfirm { delete: false });

    assert!(app.queue.is_empty());
    assert!(app.delete_after_cancel.is_empty(), "an update's files are never removed");
    assert_eq!(app.status, "cancelled");
}

/// Asking about a product with nothing in the queue must not open a dialog
/// offering to delete its install.
#[test]
fn there_is_nothing_to_cancel_when_nothing_is_downloading() {
    let (mut app, _) = Lightforge::new();
    app.dirs = None;
    app.queue.clear();
    let code = app.selected.code.clone();

    let _ = app.update(Message::AskCancelDownload(code));

    assert!(app.cancel_dialog.is_none());
}

#[test]
fn a_download_running_at_close_is_saved_as_running() {
    let (mut app, _) = Lightforge::new();
    app.dirs = None;
    app.queue.clear();
    let code = app.selected.code.clone();
    app.queue.push(Download {
        code: code.clone(),
        label: String::new(),
        stage: Stage::Queued,
        sample: TransferState::default(),
        rate: Rate::default(),
        job: Job { total: Some(12 << 30), reached: 0 },
    });
    let _ = app.pump();
    let running = app.queue_snapshot();
    assert_eq!(running.entries[0].standing, Standing::Running);
    app.saved_queue = running.clone();

    // Closing pauses the transfer to checkpoint it; that pause must not
    // be what the next start restores.
    let id = iced::window::Id::unique();
    let _ = app.update(Message::CloseRequested(id));
    assert!(matches!(app.queue[0].stage, Stage::Paused(_)));
    assert_eq!(app.saved_queue, running);
}

#[test]
fn a_config_that_will_not_load_is_described_kept_and_never_overwritten() {
    let dir = std::env::temp_dir().join(format!("lightforge-badcfg-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let dirs = AppDirs::with_dirs(dir.join("config"), dir.join("cache"), dir.join("data"));
    std::fs::create_dir_all(dirs.config_dir()).unwrap();
    let broken = "default_proton = \"GE-Proton10-34\"\nprefix_user = [oops\n";
    std::fs::write(dirs.config_file(), broken).unwrap();

    let error = Config::load_or_default(&dirs.config_file()).unwrap_err();
    let message = broken_config(&error, &dirs.config_file());
    assert!(message.contains("line 2"), "names where: {message}");
    let backup = dirs.config_file().with_extension("toml.broken");
    assert_eq!(std::fs::read_to_string(backup).unwrap(), broken);

    let (mut app, _) = Lightforge::new();
    app.dirs = Some(dirs.clone());
    app.queue.clear();
    app.config_error = Some(message);
    app.config.download_limit_mbps = Some(5);
    app.save_config();
    app.persist_queue();
    assert_eq!(std::fs::read_to_string(dirs.config_file()).unwrap(), broken);
    assert!(!dirs.queue_file().exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn only_offered_optional_content_is_shown_or_fetched() {
    let tags = |names: &[&str]| names.iter().map(|n| n.to_string()).collect::<Vec<_>>();
    // What the Forever beta's manifest carries.
    assert_eq!(offered_only(&tags(&["Alternate", "HighRes", "Talebound"])), tags(&["HighRes"]));
    // A stale opt-in from before the list existed is dropped, not fetched.
    assert!(offered_only(&tags(&["Talebound"])).is_empty());
}

#[test]
fn the_rate_meter_smooths_rather_than_tracking_one_tick() {
    let mut rate = Rate::default();
    rate.sample(0);
    // First real sample seeds the average outright.
    std::thread::sleep(Duration::from_millis(300));
    rate.sample(30_000_000);
    let first = rate.per_second;
    assert!(first > 0);

    // A tick with nothing stored must not drop the readout to zero: the
    // average gives up a fraction of the way, not all of it.
    std::thread::sleep(Duration::from_millis(300));
    rate.sample(30_000_000);
    assert!(rate.average > 0.0 && rate.average < first as f64);
}

#[test]
fn the_shown_rate_is_held_a_second_however_often_it_is_sampled() {
    let mut rate = Rate::default();
    rate.sample(0);
    std::thread::sleep(Duration::from_millis(300));
    rate.sample(30_000_000);
    let shown = rate.per_second;

    // Three more samples inside the hold: the average follows them, the
    // number on screen does not move.
    for _ in 0..3 {
        std::thread::sleep(Duration::from_millis(260));
        rate.sample(30_000_000);
    }
    assert_eq!(rate.per_second, shown);
    assert!(rate.average < shown as f64);

    // Past the hold, the next sample publishes where the average got to.
    std::thread::sleep(Duration::from_millis(300));
    rate.sample(30_000_000);
    assert_eq!(rate.per_second, rate.average as u64);
    assert!(rate.per_second < shown);
}

#[test]
fn the_rate_readout_drops_the_decimal_once_tenths_are_only_jitter() {
    assert_eq!(rate_of(0), "—");
    assert_eq!(rate_of(6_400_000), "6.4 MB/s");
    assert_eq!(rate_of(9_900_000), "9.9 MB/s");
    assert_eq!(rate_of(31_400_000), "31 MB/s");
}

#[test]
fn eta_is_rendered_in_the_largest_useful_unit() {
    let p = |done: u64, total: u64, rate: u64| state::Progress {
        downloaded_bytes: done,
        total_bytes: total,
        bytes_per_second: rate,
    };
    assert_eq!(eta_of(p(0, 30, 1)), "ETA 30s");
    assert_eq!(eta_of(p(0, 600, 1)), "ETA 10m");
    assert_eq!(eta_of(p(0, 7200, 1)), "ETA 2h 0m");
    // Stalled: no rate means no honest estimate.
    assert_eq!(eta_of(p(0, 100, 0)), "ETA —");
}

#[test]
fn byte_readouts_survive_an_unknown_total() {
    assert_eq!(bytes_of(1024 * 1024 * 1024, 0), "1.00 GiB");
    assert!(bytes_of(0, 1024 * 1024 * 1024).ends_with("of 1.00 GiB"));
}

fn update_notice_app() -> Lightforge {
    let (mut app, _) = Lightforge::new();
    app.dirs = None;
    app.update_notice = Some(lightforge_core::update::Available {
        current: "1.0.0-beta.6".to_owned(),
        latest: "1.0.0-beta.7".to_owned(),
        url: lightforge_core::update::RELEASES_PAGE.to_owned(),
    });
    app
}

#[test]
fn the_update_card_draws_for_every_route_and_stage() {
    use lightforge_core::upgrade::Origin;
    let mut app = update_notice_app();
    let available = app.update_notice.clone().unwrap();
    let origins = [
        Origin::Installer { bin_dir: "/home/u/.local/bin".into() },
        Origin::Pacman { package: "lightforge".into() },
        Origin::Deb,
        Origin::Copr { atomic: true },
        Origin::Checkout { repo: "/home/u/lightforge".into() },
        Origin::Unknown,
    ];
    let stages = [
        None,
        Some(UpgradeStage::Fetching),
        Some(UpgradeStage::InTerminal { command: "makepkg -si --noconfirm".into(), stamp: None }),
        Some(UpgradeStage::NoTerminal { command: "sh update.sh".into() }),
        Some(UpgradeStage::Installed),
        Some(UpgradeStage::Failed("no connection".into())),
    ];
    for origin in origins {
        app.origin = origin;
        for stage in stages.clone() {
            app.upgrade = stage.map(|stage| Upgrade { stage, rx: None, sample: (5, Some(10)) });
            let _ = app.update_card(&available);
        }
    }
}

#[test]
fn the_update_notice_stays_while_files_are_being_replaced() {
    let mut app = update_notice_app();
    app.upgrade = Some(Upgrade { stage: UpgradeStage::Fetching, rx: None, sample: (0, None) });
    let _ = app.update(Message::DismissUpdate);
    assert!(app.update_notice.is_some(), "closed halfway through an update");

    app.upgrade.as_mut().unwrap().stage = UpgradeStage::Installed;
    let _ = app.update(Message::DismissUpdate);
    assert!(app.update_notice.is_none() && app.upgrade.is_none());
}

#[test]
fn a_package_manager_replacing_the_binary_ends_the_wait() {
    let dir = std::env::temp_dir().join(format!("lightforge-upgrade-watch-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let exe = dir.join("lightforge-gui");
    std::fs::write(&exe, "old").unwrap();

    let mut app = update_notice_app();
    app.exe = Some(exe.clone());
    let stamp = tasks::upgrade::stamp(&exe);
    app.upgrade = Some(Upgrade {
        stage: UpgradeStage::InTerminal { command: "makepkg -si".into(), stamp },
        rx: None,
        sample: (0, None),
    });

    let _ = app.update(Message::UpgradeWatch);
    assert!(matches!(app.upgrade.as_ref().unwrap().stage, UpgradeStage::InTerminal { .. }));

    // What pacman does: a new file renamed over the old one.
    std::fs::write(dir.join("new"), "new").unwrap();
    std::fs::rename(dir.join("new"), &exe).unwrap();
    let _ = app.update(Message::UpgradeWatch);
    assert_eq!(app.upgrade.as_ref().unwrap().stage, UpgradeStage::Installed);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_proton_installer_opens_on_proton_ge_switches_tabs_and_closes_on_escape() {
    let (mut app, _) = Lightforge::new();
    app.dirs = None;
    app.queue.clear();
    app.wizard = None;
    app.splash = None;
    app.update_notice = None;
    app.patch_notes = None;
    // A list already fetched, so opening it asks GitHub for nothing.
    app.proton_available = vec![(proton::ProtonFamily::GloriousEggroll, vec!["GE-Proton11-7".to_owned()])];

    let _ = app.update(Message::OpenProtonInstaller);
    assert_eq!(app.proton_installer, Some(ProtonCategory::GloriousEggroll));

    for tab in ProtonCategory::ALL {
        let _ = app.update(Message::ProtonInstallerTab(tab));
        assert_eq!(app.proton_installer, Some(tab));
        // Every tab draws, including the two that only explain.
        let _ = app.proton_installer_card(tab);
    }

    assert!(matches!(app.dialog_dismissal(), Some(Message::CloseProtonInstaller)));
    let _ = app.update(Message::CloseProtonInstaller);
    assert_eq!(app.proton_installer, None);
}

#[test]
fn stop_asks_first_and_play_comes_back_when_the_game_is_gone() {
    let (mut app, _) = Lightforge::new();
    app.dirs = None;
    app.queue.clear();
    app.wizard = None;
    app.splash = None;
    app.update_notice = None;
    app.patch_notes = None;
    app.screen = Screen::Dashboard;

    // Nothing is running: STOP has nothing to ask about.
    let _ = app.update(Message::AskStopGame);
    assert!(app.stop_dialog.is_none());

    // A launch of ours, and a game seen in the prefix.
    app.busy = true;
    app.launching = Some(app.selected.code.clone());
    let _ = app.update(Message::GameSeen(true));
    assert!(app.game_up);
    let _ = app.update(Message::AskStopGame);
    assert!(app.stop_dialog.is_some());
    assert!(matches!(app.dialog_dismissal(), Some(Message::StopGameDismiss)));
    // Drawn, on the managed panel and as the dialog.
    let _ = app.view();

    // Keeping the game going changes nothing.
    let _ = app.update(Message::StopGameDismiss);
    assert!(app.stop_dialog.is_none() && app.game_up && !app.stopping);

    // Confirmed: the game is told to go, once.
    let _ = app.update(Message::AskStopGame);
    let _ = app.update(Message::StopGameConfirm);
    assert!(app.stopping && app.stop_dialog.is_none());
    let _ = app.update(Message::StopGameConfirm);

    // The watcher reports the exit: PLAY is back and nothing is left over.
    let _ = app.update(Message::Launched(Ok("game closed".to_owned())));
    assert!(!app.busy && !app.game_up && !app.stopping);

    // A scan that was out when the session ended must not bring STOP back.
    let _ = app.update(Message::GameSeen(true));
    assert!(!app.game_up);
}
