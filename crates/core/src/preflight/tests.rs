use std::collections::HashSet;
use std::path::Path;

use sysinfo::{Signal, System};

use super::*;
use crate::paths::Layout;
use std::ffi::OsString;

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("lightforge-preflight-{name}"));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn environ(vars: &[&str]) -> Vec<OsString> {
    vars.iter().map(OsString::from).collect()
}

/// `argv` as Wine presents it, plus the `comm` Linux reports.
fn argv(parts: &[&str]) -> Vec<OsString> {
    parts.iter().map(OsString::from).collect()
}

const BNET: &str = r"C:\Program Files (x86)\Battle.net\Battle.net.exe";
const AGENT: &str = r"C:\ProgramData\Battle.net\Agent\Agent.9775\Agent.exe";

#[test]
fn a_pass_beside_the_client_leaves_it_running_and_touches_nothing() {
    use std::process::{Command, Stdio};

    let layout = Layout::with_root("/games/Blizzard");
    // A process that presents itself as the client, the way Wine does.
    let mut client = Command::new("bash")
        .args(["-c", r#"exec -a 'C:\Program Files (x86)\Battle.net\Battle.net.exe' sleep 30"#])
        .env("WINEPREFIX", layout.prefix())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    std::thread::sleep(Duration::from_millis(300));
    assert!(client_running(&layout), "the stand-in is recognised as the client");

    let report = Preflight::new("steamuser")
        .keeping_client(true)
        .run_beside_client(&layout, None, &mut NoDownloads, Vec::new())
        .unwrap();
    assert!(report.processes.is_empty() && report.configs.is_empty());
    assert!(report.agent_cache_removed.is_none() && report.staging_removed.is_none());
    assert!(client.try_wait().unwrap().is_none(), "the client is still up");
    let _ = client.kill();
    let _ = client.wait();
}

#[test]
fn a_process_that_ignores_sigterm_is_killed_and_a_stranger_is_left_alone() {
    use std::process::{Command, Stdio};

    let layout = Layout::with_root("/games/Blizzard");
    let prefix = layout.prefix();
    let stubborn = |env: Option<&Path>| {
        let mut cmd = Command::new("sh");
        cmd.args(["-c", "trap '' TERM; while :; do sleep 0.05; done"])
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if let Some(prefix) = env {
            cmd.env("WINEPREFIX", prefix);
        }
        cmd.spawn().unwrap()
    };
    let mut ours = stubborn(Some(&prefix));
    let mut stranger = stubborn(None);
    std::thread::sleep(Duration::from_millis(200));

    let mut system = System::new();
    refresh(&mut system);
    let grace = Duration::from_millis(400);
    let ignored = signal_all(&mut system, &layout, &[ours.id(), stranger.id()], Signal::Term, grace);
    assert_eq!(ignored, HashSet::from([ours.id()]), "SIGTERM is ignored, and the stranger was never signalled");
    let survivors = signal_all(&mut system, &layout, &[ours.id()], Signal::Kill, Duration::from_secs(2));
    assert!(survivors.is_empty());

    assert!(ours.wait().is_ok());
    // Still running: it was not ours to signal.
    assert!(stranger.try_wait().unwrap().is_none());
    let _ = stranger.kill();
    let _ = stranger.wait();
}

#[test]
fn a_handed_off_game_is_whatever_is_not_battle_net_or_wine() {
    // The point of the rule: a title nobody wrote a table entry for is
    // still recognised, which is what makes the hand-off work for
    // anything Blizzard ships.
    assert_eq!(
        names_a_game(&argv([r"C:\Program Files (x86)\Hearthstone\Hearthstone.exe"].as_ref()), "Hearthstone.e"),
        Some("Hearthstone.exe".to_owned())
    );
    assert_eq!(
        names_a_game(&argv(&[r"C:\...\_retail_\Wow.exe"]), "Wow.exe"),
        Some("Wow.exe".to_owned())
    );

    // Battle.net's own are somebody else's problem, including the CEF
    // children that report themselves as CrBrowserMain.
    assert_eq!(names_a_game(&argv(&[BNET, "--from-launcher"]), "CrBrowserMain"), None);
    assert_eq!(names_a_game(&argv(&[AGENT]), "Agent.exe"), None);

    // Wine runs these in every prefix, and one of them appearing a
    // second after the click would otherwise be "the game", played for
    // a quarter of a second.
    for service in ["services.exe", "explorer.exe", "winedevice.exe", "rpcss.exe"] {
        assert_eq!(
            names_a_game(&argv(&[&format!(r"C:\windows\system32\{service}")]), service),
            None,
            "{service} is Wine's, not a game"
        );
    }

    // What a GE-Proton launch leaves behind after the game has gone,
    // from a real report: none of it is a game, and all of it counted as
    // one refused every Play after the first.
    for (exe, comm) in [
        (r"C:\windows\system32\steam.exe", "steam.exe"),
        (r"C:\windows\system32\xalia.exe", "xalia.exe"),
        (r"C:\windows\system32\lsass.exe", "lsass.exe"),
        (r"C:\windows\system32\rundll32.exe", "rundll32.exe"),
        ("python3", "python3"),
        ("/usr/bin/gamemoderun", "gamemoderun"),
    ] {
        assert_eq!(names_a_game(&argv(&[exe, "run"]), comm), None, "{exe} is not a game");
    }

    // A browser child of the game is not a second game.
    assert_eq!(
        names_a_game(
            &argv(&[r"C:\Games\Something.exe", "--type=renderer"]),
            "CrRendererMain"
        ),
        None
    );
}

#[test]
fn only_the_processes_that_cause_the_deadlock_are_classified() {
    assert_eq!(
        ProcKind::classify(&argv(&[AGENT, "--session=4409"]), "Agent.exe"),
        Some(ProcKind::Agent)
    );
    assert_eq!(
        ProcKind::classify(&argv(&[BNET, "--from-launcher"]), "CrBrowserMain"),
        Some(ProcKind::BattleNet)
    );
    assert_eq!(
        ProcKind::classify(
            &argv(&[r"C:\Program Files (x86)\Battle.net\Battle.net Launcher.exe"]),
            "Battle.net Laun"
        ),
        Some(ProcKind::BattleNet)
    );
    assert_eq!(
        ProcKind::classify(
            &argv(&["/home/u/proton/files/bin/wineserver", "-p"]),
            "wineserver"
        ),
        Some(ProcKind::WineServer)
    );

    // The game itself is never touched. That is the legal guardrail, and
    // it is enforced here rather than by remembering not to.
    assert_eq!(
        ProcKind::classify(&argv(&[r"C:\...\_retail_\Wow.exe"]), "Wow.exe"),
        None
    );
    assert_eq!(
        ProcKind::classify(&argv(&["/usr/bin/firefox"]), "firefox"),
        None
    );
}

#[test]
fn the_client_is_recognised_despite_cef_renaming_its_threads() {
    // Live finding: Linux reports `comm`, and the Battle.net client is
    // CEF: every one of its processes is named after a Chromium thread,
    // never after the executable. Classifying on the name alone silently
    // misses the client entirely, which is the one process pre-flight
    // exists to clear.
    for comm in ["CrBrowserMain", "CrGpuMain", "CrRendererMain", "CrUtilityMain", "Agent"] {
        assert!(
            ProcKind::classify(&argv(&[BNET, "--from-launcher"]), comm).is_some(),
            "{comm} was not recognised as the client"
        );
    }
}

#[test]
fn cef_children_are_helpers_and_die_before_the_browser_process() {
    let child = ProcKind::classify(
        &argv(&[BNET, "--type=renderer", "--log-severity=error"]),
        "CrRendererMain",
    );
    assert_eq!(child, Some(ProcKind::Helper));
    assert!(ProcKind::Helper.kill_order() < ProcKind::BattleNet.kill_order());
}

#[test]
fn a_windows_argv0_splits_on_backslashes() {
    assert_eq!(executable_name(&argv(&[BNET]), "x"), "Battle.net.exe");
    assert_eq!(executable_name(&argv(&["/usr/bin/wineserver"]), "x"), "wineserver");
    // A process with no readable command line still has its name.
    assert_eq!(executable_name(&[], "wineserver"), "wineserver");
    assert_eq!(executable_name(&argv(&[""]), "wineserver"), "wineserver");
}

#[test]
fn wineserver_dies_after_everything_it_owns() {
    let mut kinds = [
        ProcKind::WineServer,
        ProcKind::BattleNet,
        ProcKind::Helper,
        ProcKind::Agent,
    ];
    kinds.sort_by_key(|k| k.kill_order());
    assert_eq!(
        kinds,
        [
            ProcKind::Helper,
            ProcKind::Agent,
            ProcKind::BattleNet,
            ProcKind::WineServer
        ]
    );
}

#[test]
fn a_process_in_another_prefix_is_never_ours() {
    let layout = Layout::with_root("/games/Blizzard");
    let prefixes = owned_prefixes(&layout);

    // Gabriel's Lutris Battle.net: same executable name, different prefix.
    assert!(!belongs_to(
        &environ(&["WINEPREFIX=/home/u/Games/battlenet"]),
        &prefixes
    ));
    assert!(belongs_to(
        &environ(&["WINEPREFIX=/games/Blizzard/prefix/pfx"]),
        &prefixes
    ));
    // Proton exports the compat-data dir rather than the prefix itself.
    assert!(belongs_to(
        &environ(&["STEAM_COMPAT_DATA_PATH=/games/Blizzard/prefix"]),
        &prefixes
    ));
}

#[test]
fn an_unreadable_environment_means_hands_off() {
    let prefixes = owned_prefixes(&Layout::with_root("/games/Blizzard"));
    assert!(!belongs_to(&[], &prefixes));
    assert!(!belongs_to(&environ(&["PATH=/usr/bin"]), &prefixes));
}

#[test]
fn a_variable_that_merely_starts_with_the_name_does_not_match() {
    let prefixes = owned_prefixes(&Layout::with_root("/games/Blizzard"));
    // `WINEPREFIXES` is not `WINEPREFIX`, and a value-only match would be
    // a way to get an unrelated process signalled.
    assert!(!belongs_to(
        &environ(&["WINEPREFIXES=/games/Blizzard/prefix/pfx"]),
        &prefixes
    ));
}

#[test]
fn scanning_a_prefix_nothing_runs_in_finds_nothing() {
    // Lightforge's own test process is alive and has neither variable set,
    // so this also asserts the scan doesn't match on name alone.
    assert!(scan(&Layout::with_root("/nonexistent/Blizzard")).is_empty());
}

#[test]
fn cleanup_reports_what_it_reclaimed_and_is_idempotent() {
    let root = scratch("cleanup");
    let layout = Layout::with_root(&root);

    let cache = layout.agent_cache_dir();
    std::fs::create_dir_all(cache.join("data")).unwrap();
    std::fs::write(cache.join("data").join("blob"), vec![0u8; 2048]).unwrap();

    assert_eq!(remove_dir(&cache).unwrap(), Some(2048));
    assert!(!cache.exists());
    // A second pass is the normal case on a healthy system.
    assert_eq!(remove_dir(&cache).unwrap(), None);
}

#[test]
fn clearing_the_cache_never_touches_agents_product_registry() {
    // The bug this guards against cost a real user their install
    // registration on every launch: `agent_cache_dir` pointed at Agent's
    // whole directory, so clearing it deleted `product.db`: the list of
    // games Battle.net knows are installed, and the client then asked to
    // locate the game again, every single time.
    let root = scratch("agent-registry");
    let layout = Layout::with_root(&root);

    let agent = layout.agent_dir();
    std::fs::create_dir_all(layout.agent_cache_dir()).unwrap();
    std::fs::write(layout.agent_cache_dir().join("blob"), vec![0u8; 512]).unwrap();
    std::fs::write(agent.join("product.db"), b"registry").unwrap();
    std::fs::write(agent.join("Agent.exe"), b"binary").unwrap();

    remove_dir(&layout.agent_cache_dir()).unwrap();

    assert!(!layout.agent_cache_dir().exists(), "the cache goes");
    assert!(agent.join("product.db").is_file(), "the registry stays");
    assert!(agent.join("Agent.exe").is_file(), "Agent itself stays");
}

#[test]
fn the_cache_is_inside_agents_directory_but_is_not_it() {
    let layout = Layout::with_root("/games/Blizzard");
    assert!(layout.agent_cache_dir().starts_with(layout.agent_dir()));
    assert_ne!(layout.agent_cache_dir(), layout.agent_dir());
    assert!(layout.agent_cache_dir().ends_with("Agent/data/cache"));
}

#[test]
fn the_installer_staging_directory_is_the_one_inside_the_install() {
    let layout = Layout::with_root("/games/Blizzard");
    assert!(layout
        .battle_net_staging()
        .starts_with(layout.battle_net_dir()));
    assert!(layout.battle_net_staging().ends_with(".battle.net/.extract"));
}

#[test]
fn a_surviving_process_names_itself_in_the_error() {
    let err = PreflightError::StillRunning(vec![PrefixProcess {
        pid: 4242,
        name: "Agent.exe".into(),
        kind: ProcKind::Agent,
        exe: None,
    }]);
    let text = err.to_string();
    assert!(text.contains("Agent.exe"), "{text}");
    assert!(text.contains("4242"), "{text}");
}

#[test]
fn the_no_downloads_guard_never_claims_a_pause() {
    let mut guard = NoDownloads;
    assert!(!guard.pause(&ProductCode::Retail).unwrap());
    assert!(!guard.resume(&ProductCode::Retail).unwrap());
}
