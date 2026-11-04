#[test]
fn starting_in_the_tray_is_a_global_key_written_as_a_string() {
    // The client stores every one of these as a string, and a real
    // boolean here is a value it reads back as false.
    let minimized = window_state_tweaks(true);
    assert_eq!(minimized.len(), 1);
    assert_eq!(minimized[0].path, ["Client", "AutoStartMinimized"]);
    assert_eq!(minimized[0].value, json!("true"));
    assert_eq!(window_state_tweaks(false)[0].value, json!("false"));
}

#[test]
fn an_autostart_is_the_plain_client_start_plus_the_flag_that_unlocks_it() {
    let dir = PathBuf::from("/bnet");
    let (plain, no_args) = Launch::Client.command(&dir);
    let (exe, args) = Launch::Autostart.command(&dir);

    // Same binary: the launcher keeps the client's self-update working.
    assert_eq!(exe, plain);
    assert!(no_args.is_empty());
    assert_eq!(args, ["--autostarted"]);
    // It opens the client; it hands nothing off.
    assert!(!Launch::Autostart.hands_off());
}

#[test]
fn minimizing_is_only_safe_where_the_client_signs_itself_in() {
    let dir = scratch("auto-login");
    let layout = Layout::with_root(&dir);
    let path = layout.battle_net_config("steamuser");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();

    let write = |value: Value| std::fs::write(&path, value.to_string()).unwrap();

    // Both halves are needed: a remembered account with auto-login off
    // still stops at the sign-in form, and auto-login with nothing
    // remembered has nobody to sign in as.
    write(json!({"Client": {"AutoLogin": "true", "SavedAccountNames": "a@b.c"}}));
    assert!(signs_in_by_itself(&layout, "steamuser"));

    write(json!({"Client": {"AutoLogin": "false", "SavedAccountNames": "a@b.c"}}));
    assert!(!signs_in_by_itself(&layout, "steamuser"));

    write(json!({"Client": {"AutoLogin": "true", "SavedAccountNames": ""}}));
    assert!(!signs_in_by_itself(&layout, "steamuser"));

    // A prefix with no client installed yet answers no, not a panic.
    let _ = std::fs::remove_file(&path);
    assert!(!signs_in_by_itself(&layout, "steamuser"));

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_uri_is_the_command_the_system_protocol_handler_sends() {
    let dir = PathBuf::from("/bn");
    let (exe, args) = Launch::Uri(game_uri("W1R")).command(&dir);

    // Exactly what Wine registered for battlenet:// at install time, so
    // the client treats it as a protocol link and queues it for its
    // UriController instead of applying it mid-startup and losing it.
    assert_eq!(exe, dir.join("Battle.net.exe"));
    assert_eq!(args, vec!["--uri=battlenet://W1R"]);
    assert!(Launch::Uri(game_uri("W1R")).hands_off());
}

#[test]
fn readiness_is_read_from_the_log_this_start_wrote() {
    let dir = scratch("signed-in");
    let layout = Layout::with_root(&dir);
    let logs = layout.battle_net_logs("steamuser");
    std::fs::create_dir_all(&logs).unwrap();

    // A previous session's log, saying it signed in, must not answer for
    // this start. That is how a hand-off fires at a client that is
    // still booting.
    let old = logs.join("battle.net-20260101T000000.000000.log");
    std::fs::write(&old, format!("I [BNLogin] {SIGNED_IN}. |bnet=1:0:x\n{GAMES_READY}\n"))
        .unwrap();

    let since = SystemTime::now() + Duration::from_secs(60);
    assert_eq!(
        wait_until_ready(&layout, "steamuser", since, Duration::from_millis(1)),
        Readiness::NeedsAttention,
        "a log older than this start says nothing about it"
    );

    // And one written now does answer.
    assert_eq!(
        wait_until_ready(
            &layout,
            "steamuser",
            SystemTime::UNIX_EPOCH,
            Duration::from_millis(1),
        ),
        Readiness::SignedIn
    );

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_launch_waits_for_the_game_list_not_just_the_sign_in() {
    let signed = format!("I [BNLogin] {SIGNED_IN}. |bnet=1:0:x\n");
    let listed = format!("{signed}I [GameController] {GAMES_READY}\n");
    let none = Duration::ZERO;

    assert!(!client_ready("", none));
    assert!(!client_ready(&signed, none), "signed in, list not built: the command is lost");
    assert!(client_ready(&listed, none));
    // The list's line is in the log before the sign-in's on some starts,
    // which is no reason to wait for it twice.
    assert!(client_ready(&format!("{GAMES_READY}\n{signed}"), none));
    assert!(!client_ready(&listed.replace(SIGNED_IN, "x"), GAMES_GRACE), "not signed in");
    assert!(client_ready(&signed, GAMES_GRACE), "a line that never comes costs the grace, not 60 s");
}

// From the client's log, 2026-11-04: Hearthstone ran, exited, and a launch
// sent before Agent reported the exit only selected its tile.
const HS_STARTED: &str = "D 2026-11-04 00:53:36.388350 [InstallManager] {Main} Game is running: hs_beta\r\n";
const HS_GONE: &str = "D 2026-11-04 00:54:03.478889 [InstallManager] {Main} Game is no longer running: hs_beta\r\n";
const W3_STARTED: &str = "D 2026-11-04 00:53:03.307605 [InstallManager] {Main} Game is running: w3\r\n";

#[test]
fn a_game_counts_as_running_until_the_client_hears_it_exit() {
    assert!(!counts_as_running("", "hs_beta"));
    assert!(counts_as_running(HS_STARTED, "hs_beta"));
    assert!(!counts_as_running(&format!("{HS_STARTED}{HS_GONE}"), "hs_beta"));
    assert!(counts_as_running(&format!("{HS_STARTED}{HS_GONE}{HS_STARTED}"), "hs_beta"));
}

#[test]
fn another_game_running_does_not_hold_a_launch() {
    assert!(!counts_as_running(W3_STARTED, "hs_beta"));
    assert!(!counts_as_running(&format!("{HS_STARTED}{HS_GONE}{W3_STARTED}"), "hs_beta"));
}

#[test]
fn the_client_says_when_it_holds_a_game_back_for_an_update() {
    // Forever, and Hearthstone beside it, at the client's start, 2026-11-04.
    let waiting = "D 2026-11-04 07:51:18.920525 [ProductState] {Main} InstallState (wowf): currentOperation=Update currentOperationStatus=New installed=1 installPath=H:/World of Warcraft\r\n";
    let fine = "D 2026-11-04 07:51:18.920568 [ProductState] {Main} InstallState (hs_beta): currentOperation=UpToDate currentOperationStatus=Paused installed=1 playable=1\r\n";
    // Its own lines in between carry other fields, and say nothing about this.
    let progress = "D 2026-11-03 23:36:12.000000 [ProductState] {Main} InstallState (wowf): currentOperationStatus=Running progress=0.3\r\n";
    let finished = "D 2026-11-03 23:36:16.271653 [ProductState] {Main} InstallState (wowf): currentOperation=UpToDate currentOperationStatus=Paused\r\n";

    assert!(pending_update(waiting, "wowf"));
    assert!(pending_update(&format!("{waiting}{fine}{progress}"), "wowf"));
    assert!(!pending_update(&format!("{waiting}{finished}"), "wowf"));
    assert!(!pending_update(fine, "hs_beta"));
    assert!(!pending_update(waiting, "hs_beta"), "another game's update");
    assert!(!pending_update("", "wowf"));
}

#[test]
fn an_update_is_followed_through_the_clients_log() {
    // From a Hearthstone update, 2026-10-27.
    let started = "D 2026-10-27 16:51:45.651795 [InstallManager] {Main} Update started: hs_beta\r\n";
    let running = "D 2026-10-27 16:51:48.211613 [ProductState] {Main} InstallState (hs_beta): currentOperationStatus=Running bytesToDownload=52458135 updatesToApplySize=511408526 downloadRate=1807604 downloadRemaining=20000000 progress=0.3\r\n";
    let applying = "D 2026-10-27 16:51:49.721426 [ProductState] {Main} InstallState (hs_beta): currentOperationStatus=Waiting bytesToDownload=0 updatesToApplySize=0 downloadRate=0 downloadRemaining=0 progress=1\r\n";
    let completed = "D 2026-10-27 16:51:49.722062 [InstallManager] {Main} Update completed: hs_beta\r\n";

    assert_eq!(update_state("", "hs_beta"), UpdateState::NotStarted);
    assert_eq!(
        update_state(started, "hs_beta"),
        UpdateState::Running(crate::state::Progress::default()),
        "no sizes yet: no number"
    );
    assert_eq!(
        update_state(&format!("{started}{running}"), "hs_beta"),
        UpdateState::Running(crate::state::Progress {
            downloaded_bytes: 32458135,
            total_bytes: 52458135,
            bytes_per_second: 1807604,
        })
    );
    let UpdateState::Running(done) = update_state(&format!("{started}{running}{applying}"), "hs_beta") else {
        panic!("still running");
    };
    assert_eq!((done.downloaded_bytes, done.total_bytes), (52458135, 52458135));
    assert_eq!(update_state(&format!("{started}{running}{completed}"), "hs_beta"), UpdateState::Completed);
}

#[test]
fn another_games_update_is_not_this_ones() {
    let other = "D 2026-11-04 08:09:52.148886 [InstallManager] {Main} Update completed: wow_classic\r\n";
    assert_eq!(update_state(other, "wow"), UpdateState::NotStarted);
}

#[test]
fn the_session_log_is_the_one_that_signed_in_not_the_second_invocation() {
    let dir = scratch("session-log");
    let layout = Layout::with_root(&dir);
    let logs = layout.battle_net_logs("steamuser");
    std::fs::create_dir_all(&logs).unwrap();
    let client = logs.join("battle.net-20261104T005254.117078.log");
    std::fs::write(&client, format!("I [BNLogin] {SIGNED_IN}. |bnet=1:0:x\r\n{HS_STARTED}")).unwrap();
    // Written after the client's, as every hand-off's own invocation is.
    std::thread::sleep(Duration::from_millis(20));
    std::fs::write(
        logs.join("battle.net-20261104T005403.063011.log"),
        "I [Main] {Main} Command line arguments: {[0]=--productcode=hs_beta, [1]=--exec=launch WTCG}\r\n",
    )
    .unwrap();

    let session = SessionLog::find(&layout, "steamuser").unwrap();
    assert!(!session.wait_until_closed("hs_beta", Duration::ZERO), "the client's log says it still runs");

    let mut text = std::fs::read_to_string(&client).unwrap();
    text.push_str(HS_GONE);
    std::fs::write(&client, text).unwrap();
    assert!(session.wait_until_closed("hs_beta", Duration::ZERO));
    assert!(!session.launch_taken(Duration::ZERO));

    let mut text = std::fs::read_to_string(&client).unwrap();
    text.push_str("I 2026-11-04 00:53:34.342537 [GameLaunchController] {Main} LaunchBinary: uid=hs_beta selectedRegion=EU binaryType=game\r\n");
    std::fs::write(&client, text).unwrap();
    assert!(session.launch_taken(Duration::ZERO));

    assert!(!session.updated_itself());
    let mut text = std::fs::read_to_string(&client).unwrap();
    text.push_str("D 2026-11-04 01:03:09.027560 [InstallManager] {Main} Update completed: battle.net\r\n");
    std::fs::write(&client, text).unwrap();
    assert!(session.updated_itself());

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn game_uids_are_client_family_ids_not_tact_codes() {
    use super::*;

    // Verified against the client's log: these ids select these tiles.
    let uid = |code| game_uid(&code).unwrap();
    assert_eq!(uid(ProductCode::Retail), "WoW");
    assert_eq!(uid(ProductCode::ClassicEra), "WoWC");
    assert_eq!(uid(ProductCode::Classic), "WoWC");
    assert_eq!(uid(ProductCode::ClassicBeta), "WoWF");
    assert_eq!(uid(ProductCode::Forever), "WoWF");

    // The bug this prevents: a TACT code is unresolvable and the client
    // falls back to Warcraft III.
    assert_ne!(uid(ProductCode::ClassicEra), "wow_classic_era");
}

use std::process::Stdio;
use std::time::{Instant, SystemTime};

use serde_json::{json, Value};

use super::*;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lightforge-bnet-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

#[test]
fn the_installer_is_told_the_windows_install_path() {
    let layout = Layout::with_root("/games/Blizzard");
    let args = Installer::new(&layout, "/cache/Battle.net-Setup.exe")
        .with_locale("deDE")
        .args();
    assert_eq!(
        args,
        [
            "--lang=deDE",
            r"--installpath=C:\Program Files (x86)\Battle.net",
        ]
    );
}

#[test]
fn an_install_needs_both_the_launcher_and_a_versioned_client() {
    let dir = scratch("detect");
    assert!(!is_installed(&dir));

    std::fs::write(dir.join("Battle.net Launcher.exe"), b"MZ").unwrap();
    // The launcher alone is what a half-finished bootstrap leaves behind.
    assert!(!is_installed(&dir));

    std::fs::create_dir_all(dir.join("Battle.net.15000")).unwrap();
    assert!(is_installed(&dir));
}

/// The Debian regression: with no 32-bit runtime the installer dies in under
/// a second, and that must not surface as a ten-minute timeout.
#[test]
fn an_installer_that_dies_is_reported_as_exited_not_timed_out() {
    let dir = scratch("exited");
    let layout = Layout::with_root(&dir);
    let installer = Installer::new(&layout, dir.join("setup.exe"))
        // Generous: it must not wait this long once the process is gone.
        .with_timeout(Duration::from_secs(60))
        .with_poll_interval(Duration::from_millis(10))
        .with_exit_grace(Duration::from_millis(50), Duration::from_millis(20));

    let mut child = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg("exit 3")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");

    let started = Instant::now();
    let err = installer
        .wait_for_install(started, Some(&mut child))
        .unwrap_err();

    assert_eq!(err, Wait::Exited(Some(3)));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "a dead installer must not be waited out: {:?}",
        started.elapsed()
    );
}

/// Exiting is not failing. Blizzard's setup bootstraps a second process,
/// so a clean exit with the files still landing has to stay a success.
#[test]
fn files_landing_after_a_clean_exit_still_count() {
    let dir = scratch("late-files");
    let layout = Layout::with_root(&dir);
    let install_dir = layout.battle_net_dir();
    let installer = Installer::new(&layout, dir.join("setup.exe"))
        .with_timeout(Duration::from_secs(60))
        .with_poll_interval(Duration::from_millis(10))
        .with_exit_grace(Duration::from_secs(10), Duration::from_millis(20));

    let mut child = std::process::Command::new("/bin/sh")
        .arg("-c")
        .arg("exit 0")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("spawn");

    let writer = {
        let install_dir = install_dir.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(80));
            std::fs::create_dir_all(install_dir.join("Battle.net.15000")).unwrap();
            std::fs::write(install_dir.join("Battle.net Launcher.exe"), b"MZ").unwrap();
        })
    };

    let outcome = installer
        .wait_for_install(Instant::now(), Some(&mut child))
        .expect("files arriving within the grace period is a success");
    assert!(!outcome.skipped);
    writer.join().unwrap();
}

#[test]
fn the_message_shows_the_end_of_stderr_not_all_of_it() {
    let noise: String = (0..200).map(|i| format!("warn {i}\n")).collect();
    let err = BattleNetError::InstallerExited {
        code: Some(127),
        dir: PathBuf::from("/tmp/bn"),
        stderr: format!("{noise}libfreetype.so.6: cannot open shared object file"),
    };

    let rendered = err.to_string();
    assert!(rendered.contains("libfreetype.so.6"), "the real error must survive");
    assert!(rendered.contains("code 127"));
    assert!(
        rendered.lines().count() <= STDERR_LINES_SHOWN + 1,
        "a banner cannot show 200 lines of Wine chatter:\n{rendered}"
    );
    assert!(!rendered.contains("warn 0\n"), "the start is the least useful part");
}

#[test]
fn stderr_is_kept_from_the_end_and_bounded() {
    let noise = "x".repeat(STDERR_TAIL * 3);
    let text = format!("{noise}libfreetype.so.6: cannot open shared object file");
    let kept = tail_of(text.as_bytes());

    assert!(kept.len() <= STDERR_TAIL, "kept {} bytes", kept.len());
    assert!(
        kept.ends_with("cannot open shared object file"),
        "the end is where the error is"
    );
}

#[test]
fn a_stalled_installer_times_out_instead_of_hanging() {
    let dir = scratch("timeout");
    let layout = Layout::with_root(&dir);
    let installer = Installer::new(&layout, dir.join("setup.exe"))
        .with_timeout(Duration::from_millis(60))
        .with_poll_interval(Duration::from_millis(10));

    let err = installer.wait_for_install(Instant::now(), None).unwrap_err();
    assert_eq!(err, Wait::TimedOut);
}

#[test]
fn tweaks_create_missing_objects_without_touching_anything_else() {
    let mut config = json!({
        "Client": { "Sound": { "Enabled": "true" } },
        "Games": { "wow": { "LastPlayed": "1" } }
    });
    apply_tweaks(&mut config, &default_tweaks(&Layout::with_root("/games/Blizzard"), false));

    assert_eq!(config["Client"]["GameLaunchWindowBehavior"], json!("2"));
    assert_eq!(config["Client"]["HardwareAcceleration"], json!("false"));
    // The default library's `games/` drive, so a title Battle.net installs
    // itself lands on the drive the user picked, not inside the
    // prefix.
    assert_eq!(config["Client"]["Install"]["DefaultInstallPath"], json!(r"E:\"));
    // Login state and per-game settings share this file; losing them would
    // log the user out.
    assert_eq!(config["Client"]["Sound"]["Enabled"], json!("true"));
    assert_eq!(config["Games"]["wow"]["LastPlayed"], json!("1"));
}

#[test]
fn an_existing_value_is_overwritten_not_appended() {
    let mut config = json!({ "Client": { "GameLaunchWindowBehavior": "0" } });
    apply_tweaks(&mut config, &default_tweaks(&Layout::with_root("/games/Blizzard"), false));
    assert_eq!(config["Client"]["GameLaunchWindowBehavior"], json!("2"));
}

#[test]
fn a_background_client_stays_after_a_hand_off() {
    let mut config = json!({ "Client": { "GameLaunchWindowBehavior": "2" } });
    apply_tweaks(&mut config, &default_tweaks(&Layout::with_root("/games/Blizzard"), true));
    assert_eq!(config["Client"]["GameLaunchWindowBehavior"], json!("3"));
    assert_eq!(config["Client"]["RestoreWindowOnGameEnd"], json!("false"));
}

#[test]
fn a_client_that_exits_after_a_hand_off_keeps_its_own_restore_setting() {
    let mut config = json!({ "Client": { "RestoreWindowOnGameEnd": "true" } });
    apply_tweaks(&mut config, &default_tweaks(&Layout::with_root("/games/Blizzard"), false));
    assert_eq!(config["Client"]["RestoreWindowOnGameEnd"], json!("true"));
}

#[test]
fn a_background_client_hides_on_close() {
    let mut config = json!({ "User": { "Client": { "HideOnClose": "false" } } });
    apply_tweaks(&mut config, &account_tweaks(true));
    assert_eq!(config["User"]["Client"]["HideOnClose"], json!("true"));
    assert_eq!(config["User"]["Client"]["DisplayExitPrompt"], json!("false"));
}

#[test]
fn only_a_background_client_has_its_notifications_switched_off() {
    let mut config = json!({ "User": { "Client": { "ToastsEnabled": "false" } } });
    apply_tweaks(&mut config, &account_tweaks(false));
    assert_eq!(config["User"]["Client"]["ToastsEnabled"], json!("false"), "the user's own choice stands");

    let mut config = json!({ "User": { "Client": {} } });
    apply_tweaks(&mut config, &account_tweaks(true));
    assert_eq!(config["User"]["Client"]["ToastsEnabled"], json!("false"));
}

#[test]
fn a_scalar_where_an_object_belongs_is_replaced() {
    // Battle.net has shipped schema changes before; a stale scalar must not
    // abort setup.
    let mut config = json!({ "Client": { "Install": "C:\\somewhere" } });
    apply_tweaks(&mut config, &default_tweaks(&Layout::with_root("/games/Blizzard"), false));
    assert_eq!(config["Client"]["Install"]["DefaultInstallPath"], json!(r"E:\"));
}

#[test]
fn hide_on_close_is_written_to_every_account_config_but_not_the_global_one() {
    let dir = scratch("accounts");
    let layout = Layout::with_root(&dir);
    let bnet = layout.battle_net_config("steamuser").parent().unwrap().to_path_buf();
    std::fs::create_dir_all(&bnet).unwrap();
    std::fs::write(bnet.join("1c858370.config"), r#"{"User":{"Client":{"HideOnClose":"true"}}}"#)
        .unwrap();
    std::fs::write(bnet.join("beef0001.config"), "{}").unwrap();

    configure(&layout, "steamuser", &default_tweaks(&layout, false)).unwrap();
    let changed = configure_accounts(&layout, "steamuser", &account_tweaks(false)).unwrap();
    assert_eq!(changed.len(), 2);

    for name in ["1c858370.config", "beef0001.config"] {
        let v: Value =
            serde_json::from_str(&std::fs::read_to_string(bnet.join(name)).unwrap()).unwrap();
        assert_eq!(v["User"]["Client"]["HideOnClose"], json!("false"));
        assert_eq!(v["User"]["Client"]["DisplayExitPrompt"], json!("false"));
    }

    // The global config is the client's own; account keys must not leak in.
    let global: Value = serde_json::from_str(
        &std::fs::read_to_string(layout.battle_net_config("steamuser")).unwrap(),
    )
    .unwrap();
    assert!(global.get("User").is_none());
    assert!(global["Client"].get("MinimizeToTray").is_none());
}

#[test]
fn a_prefix_with_no_account_yet_is_a_no_op() {
    let dir = scratch("noaccount");
    let layout = Layout::with_root(&dir);
    configure(&layout, "steamuser", &default_tweaks(&layout, false)).unwrap();

    // Only Battle.net.config exists before the first login.
    assert!(configure_accounts(&layout, "steamuser", &account_tweaks(false)).unwrap().is_empty());
}

#[test]
fn configure_seeds_a_config_that_does_not_exist_yet() {
    let dir = scratch("seed");
    let layout = Layout::with_root(&dir);

    let path = configure(&layout, "steamuser", &default_tweaks(&layout, false)).unwrap();
    assert_eq!(path, layout.battle_net_config("steamuser"));

    let written: Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(written["Client"]["GameLaunchWindowBehavior"], json!("2"));
}

#[test]
fn configure_is_idempotent_and_leaves_no_temp_files() {
    let dir = scratch("idempotent");
    let layout = Layout::with_root(&dir);

    let path = configure(&layout, "steamuser", &default_tweaks(&layout, false)).unwrap();
    let first = std::fs::read_to_string(&path).unwrap();
    configure(&layout, "steamuser", &default_tweaks(&layout, false)).unwrap();
    assert_eq!(first, std::fs::read_to_string(&path).unwrap());

    let leftovers = std::fs::read_dir(path.parent().unwrap()).unwrap().count();
    assert_eq!(leftovers, 1);
}

#[test]
fn a_corrupt_config_is_reported_rather_than_silently_replaced() {
    let dir = scratch("corrupt");
    let layout = Layout::with_root(&dir);
    let path = layout.battle_net_config("steamuser");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{ not json").unwrap();

    assert!(matches!(
        configure(&layout, "steamuser", &default_tweaks(&layout, false)),
        Err(BattleNetError::MalformedConfig { .. })
    ));
}

#[test]
fn dxva2_is_set_through_reg_add() {
    let dxva2 = &registry_tweaks()[0];
    assert_eq!(
        dxva2.args(),
        [
            "add",
            r"HKEY_CURRENT_USER\Software\Wine\DXVA2",
            "/v",
            "backend",
            "/t",
            "REG_SZ",
            "/d",
            "va",
            "/f",
        ]
    );
}

#[test]
fn a_plain_open_runs_the_launcher_with_no_arguments() {
    let dir = Path::new("/prefix/Battle.net");
    let (exe, args) = Launch::Client.command(dir);
    assert_eq!(exe, dir.join("Battle.net Launcher.exe"));
    assert!(args.is_empty());
}

/// An unknown uid makes the client fall back to its first family, Warcraft
/// III, so only opening the client is safe.
#[test]
fn registry_tweaks_are_well_formed_reg_add_arguments() {
    for value in registry_tweaks() {
        let args = value.args();
        assert_eq!(args[0], "add", "{}", value.name);
        assert!(args.contains(&"/f".to_owned()), "{} is not forced", value.name);
        assert!(!value.reason.is_empty(), "{} has no reason", value.name);

        // `reg add /t REG_DWORD` takes its data as digits; anything else
        // is rejected by reg.exe at runtime, where nobody is watching.
        if value.kind == "REG_DWORD" {
            assert!(
                value.data.chars().all(|c| c.is_ascii_digit()),
                "{} is a DWORD but its data is {:?}",
                value.name,
                value.data
            );
        }
    }
}

#[test]
fn font_smoothing_is_left_at_protons_default() {
    let tweaks = registry_tweaks();
    assert!(tweaks.iter().all(|v| !v.name.starts_with("FontSmoothing")));

    // DPI is not written; see the comment beside the list.
    assert!(tweaks.iter().all(|v| v.name != "LogPixels"));
}

#[test]
fn only_managed_products_have_their_updates_switched_off() {
    let tweaks = product_tweaks(&[ProductCode::Retail, ProductCode::ClassicEra]);
    let keys: Vec<String> = tweaks.iter().map(Tweak::key).collect();

    assert!(keys.contains(&"Games.wow.AutoUpdate".to_owned()));
    assert!(keys.contains(&"Games.wow.DownloadAllUpdates".to_owned()));
    assert!(keys.contains(&"Games.wow_classic_era.AutoUpdate".to_owned()));
    assert!(tweaks.iter().all(|t| t.value == json!("false")));

    // Nothing for a product nobody asked about; hand-off titles never get here.
    assert!(product_tweaks(&[]).is_empty());
}

#[test]
fn the_download_limit_round_trips_and_clears() {
    let dir = scratch("download-limit");
    let layout = Layout::with_root(&dir);
    let path = layout.battle_net_config("steamuser");
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();

    // A file the client wrote, with settings that must survive.
    std::fs::write(
        &path,
        r#"{"Client":{"Install":{"LastUserDownloadLimitNextPatchInBps":"1000000"},
           "AutoLogin":"true"}}"#,
    )
    .unwrap();
    assert_eq!(download_limit(&layout, "steamuser"), Some(1_000_000));

    set_download_limit(&layout, "steamuser", Some(5_000_000)).unwrap();
    assert_eq!(download_limit(&layout, "steamuser"), Some(5_000_000));

    // Cleared means the key is gone, not zero, and the neighbours stay.
    set_download_limit(&layout, "steamuser", None).unwrap();
    assert_eq!(download_limit(&layout, "steamuser"), None);
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(!text.contains("LastUserDownloadLimitNextPatchInBps"), "{text}");
    assert!(text.contains("AutoLogin"), "unrelated settings were lost: {text}");

    // Clearing again is success, not an error.
    set_download_limit(&layout, "steamuser", None).unwrap();
}

#[test]
fn a_limit_on_a_machine_with_no_config_is_simply_absent() {
    let layout = Layout::with_root(scratch("no-config"));
    assert_eq!(download_limit(&layout, "steamuser"), None);
    // And clearing one that was never there does not fail.
    set_download_limit(&layout, "steamuser", None).unwrap();
}

#[test]
fn a_product_with_no_known_family_opens_the_client_instead_of_guessing() {
    let dir = Path::new("/prefix/Battle.net");
    let unknown = Launch::product(ProductCode::Custom("turtle_wow".into()));

    assert!(!unknown.hands_off());
    assert_eq!(unknown.command(dir), Launch::Client.command(dir));
}

#[test]
fn the_families_read_from_a_real_client_log_hand_off() {
    let dir = Path::new("/prefix/Battle.net");
    for (code, uid) in [
        ("hs_beta", "WTCG"),
        ("hsb", "WTCG"),
        ("osi", "OSI"),
    ] {
        let launch = Launch::product(ProductCode::Custom(code.into()));
        assert!(launch.hands_off(), "{code}");
        let (exe, args) = launch.command(dir);
        assert_eq!(exe, dir.join("Battle.net.exe"), "{code}");
        assert!(args.contains(&format!("--exec=launch {uid}")), "{code}: {args:?}");
        assert!(args.contains(&format!("--productcode={code}")), "{code}: {args:?}");
    }
}

#[test]
fn a_product_hand_off_goes_to_battle_net_exe_with_productcode_and_exec() {
    let dir = Path::new("/prefix/Battle.net");
    let (exe, args) = Launch::product(ProductCode::Retail).command(dir);
    assert_eq!(exe, dir.join("Battle.net.exe"));
    // The exec uid is the client's family id, not the TACT code.
    assert_eq!(args, ["--productcode=wow", "--exec=launch WoW"]);

    let (_, classic) = Launch::product(ProductCode::Classic).command(dir);
    // Every Classic-line product shares one tile.
    assert_eq!(classic, ["--productcode=wow_classic", "--exec=launch WoWC"]);
}

#[test]
fn an_explicit_exec_verb_overrides_the_derived_uid() {
    let (_, args) = Launch::Product { code: ProductCode::Retail, exec: Some("WoW".into()) }
        .command(Path::new("/prefix/Battle.net"));
    assert_eq!(args, ["--productcode=wow", "--exec=launch WoW"]);
}

#[test]
fn launching_without_an_install_is_an_error_not_a_spawn_attempt() {
    let dir = scratch("nolaunch");
    let layout = Layout::with_root(&dir);
    let release = crate::proton::ProtonRelease {
        version: "GE-Proton11-7".into(),
        flavor: crate::proton::ProtonFlavor::GloriousEggroll,
        source: crate::proton::ProtonSource::Managed,
        path: dir.join("GE-Proton11-7"),
        built_at: None,
    };
    let runner = ProtonRunner::new(&release, &layout, dir.join("Steam"));

    assert!(matches!(
        launch(&runner, &layout, &Launch::Client),
        Err(BattleNetError::NotInstalled(_))
    ));
}

#[test]
fn provisioning_refuses_to_run_before_the_prefix_exists() {
    let dir = scratch("noprefix");
    let layout = Layout::with_root(&dir);
    let release = crate::proton::ProtonRelease {
        version: "GE-Proton11-7".into(),
        flavor: crate::proton::ProtonFlavor::GloriousEggroll,
        source: crate::proton::ProtonSource::Managed,
        path: dir.join("GE-Proton11-7"),
        built_at: None,
    };
    let runner = ProtonRunner::new(&release, &layout, dir.join("Steam"));

    assert!(matches!(
        provision(&runner, None, &layout, "steamuser", "enUS", &dir.join("setup.exe"), &[]),
        Err(BattleNetError::PrefixNotReady(_))
    ));
}
