use super::adopt::hand_off_entry;
use super::steps::welcome_answered;
use super::*;

fn install(path: &str, blocked: Option<&str>) -> WowInstall {
    WowInstall {
        path: PathBuf::from(path),
        products: vec![ProductCode::Retail],
        bytes: 1024,
        blocked: blocked.map(str::to_owned),
    }
}

fn game(label: &str) -> HandOff {
    HandOff {
        code: "hs_beta".to_owned(),
        label: label.to_owned(),
        path: PathBuf::from("/games").join(label),
        version: None,
        bytes: 2048,
    }
}

/// Only SteamOS is asked, and there the first slide waits for an answer.
#[test]
fn steamos_has_to_say_what_it_is_for_before_setup_goes_on() {
    let (mut app, _) = Lightforge::new();
    app.dirs = None;
    app.config.steamos_use = None;

    app.steamos = false;
    assert!(welcome_answered(&app), "a desktop distro was held on a SteamOS question");

    app.steamos = true;
    assert!(!welcome_answered(&app));
    let _ = update(&mut app, Msg::SteamOsUse(SteamOsUse::Console));
    assert_eq!(app.config.steamos_use, Some(SteamOsUse::Console));
    assert!(welcome_answered(&app));
}

/// Setup by keys: the first key lands on the slide's own button, Enter
/// walks the slides, and Escape steps back and never leaves.
#[test]
fn setup_walks_by_enter_and_steps_back_by_escape() {
    let (mut app, _) = Lightforge::new();
    app.dirs = None;
    app.splash = None;
    app.steamos = false;
    app.wizard = Some(Wizard::default());
    let at = |x, y| iced::Rectangle { x, y, width: 120.0, height: 30.0 };
    let send = |message| focus::Press::Send(message);

    let slide = |app: &Lightforge| {
        focus::reset();
        let wizard = app.wizard.as_ref().unwrap();
        focus::put(focus::Target::Key("picker wizard region".to_owned()), at(200.0, 200.0), send(Message::MenuClick), 0);
        focus::put(forward(wizard), at(600.0, 600.0), send(Message::Wizard(match wizard.step {
            Step::Done => Msg::Finish,
            _ => Msg::Next,
        })), 0);
    };

    slide(&app);
    let _ = app.update(Message::FocusStep(focus::Step::Down));
    assert_eq!(app.focus, Some(forward(app.wizard.as_ref().unwrap())));
    let _ = app.update(Message::FocusPress);
    assert_eq!(app.wizard.as_ref().unwrap().step, Step::Place);

    let _ = app.update(Message::FocusBack);
    assert_eq!(app.wizard.as_ref().unwrap().step, Step::Welcome);
    let _ = app.update(Message::FocusBack);
    assert!(app.wizard.is_some(), "Escape on the first slide left setup");

    // Opened from Settings, where Escape otherwise means "back to Play".
    app.screen = crate::Screen::Settings;
    let _ = app.update(Message::FocusBack);
    assert!(app.wizard.is_some(), "Escape in setup over Settings left setup");
    assert_eq!(app.screen, crate::Screen::Settings);
}

/// A second look lists what is new, and greys out what Lightforge
/// already plays: the WoW install in one of its libraries, and the game
/// already linked into the default one.
#[test]
fn a_scan_after_setup_does_not_offer_what_is_already_here() {
    let (mut app, _) = Lightforge::new();
    app.dirs = None;
    let root = std::env::temp_dir().join(format!("lightforge-wizard-known-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let games = root.join("games");
    std::fs::create_dir_all(&games).unwrap();
    std::os::unix::fs::symlink(game("Hearthstone").path, games.join("Hearthstone")).unwrap();

    let library = lightforge_core::library::Library::main_at(games.clone(), "Main");
    let mut retail = lightforge_core::Product::new(ProductCode::Retail, "Retail");
    retail.profile.library = Some(library.id.clone());
    app.config.libraries = vec![library];
    app.config.products = vec![retail];
    app.wizard = Some(Wizard { step: Step::Found, scanning: true, scan_only: true, ..Wizard::default() });

    let found = Found {
        prefix: PathBuf::from("/lutris/pfx"),
        origin: Origin::Pointed,
        battle_net: None,
        wow: vec![
            install(games.join("World of Warcraft").to_str().unwrap(), None),
            install("/elsewhere/World of Warcraft", None),
        ],
        hand_offs: vec![game("Hearthstone"), game("Diablo IV")],
    };
    let _ = update(&mut app, Msg::Scanned(vec![found]));

    let rows = &app.wizard.as_ref().unwrap().rows;
    let had: Vec<bool> = rows.iter().map(|row| row.blocked.as_deref() == Some("already in Lightforge")).collect();
    assert_eq!(had, [true, false, true, false]);
    assert!(rows[1].take && rows[3].take, "what is new is offered ticked");
    let _ = std::fs::remove_dir_all(&root);
}

#[test]
fn a_typed_folder_is_looked_through_once_and_off_the_ui_thread() {
    let (mut app, _) = Lightforge::new();
    app.dirs = None;
    app.wizard = Some(Wizard { pointed: "/nowhere/World of Warcraft".to_owned(), ..Wizard::default() });

    let _ = update(&mut app, Msg::Point);
    assert!(app.wizard.as_ref().unwrap().pointing, "running in the background");
    let _ = update(&mut app, Msg::Point);
    assert!(app.wizard.as_ref().unwrap().pointing, "a second click starts nothing new");

    let _ = update(&mut app, Msg::Pointed(None));
    let wizard = app.wizard.as_ref().unwrap();
    assert!(!wizard.pointing);
    assert!(wizard.point_error.as_deref().is_some_and(|e| e.starts_with("nothing of Blizzard's")));
}

/// Opened from Settings, the slide closes on Back, and closing it is not
/// finishing setup: the release notes stay owed.
#[test]
fn closing_a_scan_leaves_the_first_run_state_alone() {
    let (mut app, _) = Lightforge::new();
    app.dirs = None;
    app.config.patch_notes_seen = Some("0.0.1".to_owned());
    let _ = update(&mut app, Msg::Scan);
    let wizard = app.wizard.as_ref().unwrap();
    assert!(wizard.scan_only && wizard.scanning);
    assert_eq!(wizard.step, Step::Found);

    let _ = update(&mut app, Msg::Back);

    assert!(app.wizard.is_none());
    assert_eq!(app.config.patch_notes_seen.as_deref(), Some("0.0.1"));
}

#[test]
fn the_steps_run_from_welcome_to_done_and_stop_there() {
    assert_eq!(Step::ALL.first(), Some(&Step::Welcome));
    assert_eq!(Step::ALL.last(), Some(&Step::Done));
    assert!(Step::Welcome.back().is_none(), "nothing before the first slide");
    assert!(Step::Done.next().is_none(), "nothing after the last");

    // Every step is reachable from the one before it, in order.
    let mut step = Step::Welcome;
    for expected in &Step::ALL[1..] {
        step = step.next().expect("another step");
        assert_eq!(&step, expected);
        assert_eq!(step.back().unwrap().next().unwrap(), step);
    }
}

/// Battle.net calls Hearthstone `hs_beta` on a machine where it is
/// installed; the catalog ships it as `hsb`. Adding it under the code on
/// disk when the catalog holds the other one is a tile with no artwork
/// that never matches what is installed.
#[test]
fn a_found_game_is_matched_to_the_catalog_even_under_another_code() {
    let catalog = lightforge_core::product::Catalog::builtin();
    let hearthstone = catalog
        .hand_offs()
        .find(|e| e.label == "Hearthstone")
        .expect("the catalog ships Hearthstone");

    // The code the catalog itself uses resolves to itself.
    let exact = game_named(hearthstone.code.as_str(), "Hearthstone");
    assert_eq!(hand_off_entry(&catalog, &exact), (hearthstone.code.clone(), "Hearthstone".to_owned()));

    // And the client's own spelling lands on the same entry, by family.
    for other in ["hs_beta", "hsb"] {
        let found = game_named(other, "Hearthstone");
        let (code, label) = hand_off_entry(&catalog, &found);
        assert_eq!(label, "Hearthstone", "{other} should be Hearthstone");
        assert_eq!(
            lightforge_core::battlenet::game_uid(&code),
            lightforge_core::battlenet::game_uid(&hearthstone.code),
            "{other} should resolve to the same game family"
        );
    }
}

/// A game nothing knows about is still the user's game, so it goes in
/// under the folder name they would recognise.
#[test]
fn a_game_the_catalog_has_never_heard_of_keeps_its_folder_name() {
    let catalog = lightforge_core::product::Catalog::builtin();
    let found = game_named("something_new", "Some New Game");
    let (code, label) = hand_off_entry(&catalog, &found);
    assert_eq!(code.as_str(), "something_new");
    assert_eq!(label, "Some New Game");
}

fn game_named(code: &str, label: &str) -> HandOff {
    HandOff {
        code: code.to_owned(),
        label: label.to_owned(),
        path: PathBuf::from("/games").join(label),
        version: None,
        bytes: 0,
    }
}

#[test]
fn a_scan_becomes_one_ticked_row_per_thing_found() {
    let found = Found {
        prefix: PathBuf::from("/home/u/Games/battlenet/pfx"),
        origin: Origin::Lutris,
        battle_net: None,
        wow: vec![install("/mnt/nvme2/Games/World of Warcraft", None)],
        hand_offs: vec![game("Hearthstone")],
    };
    let rows = rows_of(vec![found]);
    assert_eq!(rows.len(), 2);
    assert!(rows.iter().all(|row| row.take), "what is here is taken by default");
    assert_eq!(rows[0].bytes(), 1024);
    assert_eq!(rows[1].bytes(), 2048);
    // The prefix rides along on every row: a hand-off's record is lifted
    // out of the registry in the prefix it was found in.
    assert!(rows.iter().all(|row| row.prefix.ends_with("pfx")));
}

#[test]
fn an_install_that_cannot_be_adopted_is_shown_but_not_ticked() {
    let found = Found {
        prefix: PathBuf::new(),
        origin: Origin::Pointed,
        battle_net: None,
        wow: vec![install("/mnt/x/World of Warcraft", Some("already added"))],
        hand_offs: Vec::new(),
    };
    let rows = rows_of(vec![found]);
    assert_eq!(rows.len(), 1, "it is still listed, so the user can see why");
    assert!(!rows[0].take);
    assert_eq!(rows[0].blocked.as_deref(), Some("already added"));
}
