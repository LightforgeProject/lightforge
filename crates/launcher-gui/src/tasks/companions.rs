//! Companion apps: an installer run in a game's prefix, and starting what
//! it put there; see [`lightforge_core::companion`].

use std::collections::BTreeSet;

use super::*;
use lightforge_core::companion::{self, Companion};
use lightforge_core::product::Kind;

/// A Windows program to run, an installer or the program itself, picked
/// through the desktop portal; see `update/report.rs` for why not `rfd`.
pub async fn pick_program() -> Option<PathBuf> {
    let request = ashpd::desktop::file_chooser::OpenFileRequest::default()
        .title("Choose a Windows installer or program")
        .modal(true)
        .multiple(false)
        .filter(ashpd::desktop::file_chooser::FileFilter::new("Windows programs").glob("*.exe"));
    let files = request.send().await.ok()?.response().ok()?;
    files.uris().first().and_then(|uri| uri.to_file_path().ok())
}

/// Start `installer` in `game`'s prefix and return the programs that were
/// there before it, for [`installed_programs`] to compare against.
///
/// Not waited on: Proton's `run` returns only once the whole prefix is
/// empty, and an installer often starts what it installed. The user says
/// when it is done.
pub async fn start_installer(
    layout: Layout,
    game: ProductCode,
    installer: PathBuf,
) -> Result<BTreeSet<PathBuf>, Problem> {
    tokio::task::spawn_blocking(move || {
        let runner = companion_runner(&layout, &game)?;
        let before = companion::programs(&drive_c(&layout));
        tracing::info!("running a companion installer for {}: {}", game.as_str(), installer.display());
        let child = runner.command("run", &installer, Vec::<String>::new()).spawn().map_err(|e| Problem::of(&e))?;
        reap_later(child);
        Ok(before)
    })
    .await
    .map_err(|e| Problem::of(&e))?
}

/// What an installer added since `before`, as paths on this machine.
pub async fn installed_programs(layout: Layout, before: BTreeSet<PathBuf>) -> Vec<PathBuf> {
    tokio::task::spawn_blocking(move || {
        let drive_c = drive_c(&layout);
        let after = companion::programs(&drive_c);
        companion::installed(&before, &after).into_iter().map(|exe| drive_c.join(exe)).collect()
    })
    .await
    .unwrap_or_default()
}

/// Start a companion beside whatever runs in the prefix; not waited on.
pub async fn launch_companion(layout: Layout, companion: Companion) -> Result<String, Problem> {
    tokio::task::spawn_blocking(move || {
        let exe = companion.path(&drive_c(&layout));
        if !exe.is_file() {
            return Err(format!("{} is no longer at {}", companion.name, exe.display()).into());
        }
        let runner = companion_runner(&layout, &companion.game)?;
        tracing::info!("starting companion {} for {}", companion.name, companion.game.as_str());
        let child = runner.command("run", &exe, Vec::<String>::new()).spawn().map_err(|e| Problem::of(&e))?;
        reap_later(child);
        Ok(format!("{} started", companion.name))
    })
    .await
    .map_err(|e| Problem::of(&e))?
}

fn drive_c(layout: &Layout) -> PathBuf {
    layout.prefix().join("drive_c")
}

/// The game's own Proton build: a companion shares the prefix's wineserver
/// with the game, and two builds cannot share one.
fn companion_runner(layout: &Layout, game: &ProductCode) -> Result<lightforge_core::prefix::ProtonRunner, Problem> {
    let runs = match Kind::of(game) {
        Kind::Managed => Runs::Game,
        Kind::HandOff => Runs::BattleNet,
    };
    // The game's MangoHud setting is for the game, not a companion's window.
    let runner = runner_for(layout, game, runs)?.with_env("MANGOHUD", "0");
    if !lightforge_core::preflight::served_by(&runner, layout) {
        return Err("the prefix is running under another Proton build; close what runs there first".to_owned().into());
    }
    Ok(runner)
}
