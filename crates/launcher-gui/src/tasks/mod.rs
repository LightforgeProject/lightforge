//! The work the dashboard hands off: disk inspection, the version service, the
//! asset pipeline, downloads, and launching.
//!
//! Everything here returns owned data and never borrows application state, so
//! each one can be driven by `Task::perform` without the update loop holding a
//! lock across an await.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use iced::widget::image::Handle;
use lightforge_core::backup;
use lightforge_core::advice::{advise, Advice};
use lightforge_core::error::chain;
use lightforge_core::assets::{Manifest, Pipeline};
use lightforge_core::config::Config;
use lightforge_core::net::Http;
use lightforge_core::news;
use lightforge_core::families::Families;
use lightforge_core::paths::{AppDirs, Layout};
use lightforge_core::preflight::{DownloadGuard, GameProcess, Preflight, PreflightError};
use lightforge_core::product::ProductCode;
use lightforge_core::state::Local;
use lightforge_core::tact::download::{Cancel, Downloads, Progress};
use lightforge_core::tact::install::{self, Event, Options as InstallOptions, Summary};
use lightforge_core::tact::version::{self, Region, Version};
use lightforge_core::tweaks::{HostTools, Tweaks};

use crate::art::Art;

mod branding;
mod catalog;
mod transfer;
mod store;
mod launch;
mod backups;
mod headlines;
pub(crate) mod upgrade;

pub(crate) use branding::*;
pub(crate) use catalog::*;
pub(crate) use transfer::*;
pub(crate) use store::*;
pub(crate) use launch::*;
pub(crate) use backups::*;
pub(crate) use headlines::*;
pub(crate) use upgrade::{Prepared, Stamp};


/// An error on its way to the screen: what went wrong, and what to do about
/// it when that is known.
///
/// Built with [`Problem::of`] while the error still has its type, because
/// that is the only point at which [`advise`](lightforge_core::advice::advise)
/// can tell a full drive from a dropped connection. A plain `String` converts
/// into one with no advice, so a path that has not been taught any yet still
/// reports exactly what it did before.
#[derive(Debug, Clone, PartialEq)]
pub struct Problem {
    pub message: String,
    pub advice: Option<Advice>,
}

impl Problem {
    pub fn of(error: &(dyn std::error::Error + 'static)) -> Self {
        Self { message: chain(error), advice: advise(error) }
    }
}

impl From<String> for Problem {
    fn from(message: String) -> Self {
        Self { message, advice: None }
    }
}

impl From<&str> for Problem {
    fn from(message: &str) -> Self {
        message.to_owned().into()
    }
}

/// The message alone. The advice is for the screen; the status history and
/// a bug report want what actually went wrong.
impl std::fmt::Display for Problem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// The Proton build a product launches under, with its per-product caches.
/// Which workload a runner is being built for.
///
/// They pin separately: `SPEC.md` asks for a Proton build chosen for
/// Battle.net and Agent specifically, because "latest" is not always best for
/// the client even when it is right for the game. A product's own pin still
/// wins on both paths; see [`Config::handoff_proton`], so a hand-off title
/// is not stuck with whatever the client wants.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Runs {
    /// The game itself, under the product's own pin.
    Game,
    /// Battle.net or Agent, starting something for this product.
    BattleNet,
}

impl Runs {
    /// The build this workload resolves to for `code`.
    fn pin<'a>(self, config: &'a Config, code: &ProductCode) -> Option<&'a str> {
        match self {
            Self::Game => config.effective_proton(code),
            Self::BattleNet => config.handoff_proton(code),
        }
    }
}

/// The build a launch would use, and how it sits against the prefix.
///
/// Read before a launch rather than during it: the prefix is shared, Proton
/// only migrates it forward, and an older build opening a prefix a newer one
/// rewrote is the one way this arrangement breaks. The caller decides what to
/// do about it. Nothing here refuses a launch.
pub fn prefix_fit(
    layout: &Layout,
    code: &ProductCode,
    runs: Runs,
) -> (Option<String>, lightforge_core::proton::PrefixFit) {
    use lightforge_core::proton;

    let Ok(dirs) = AppDirs::discover() else {
        return (None, proton::PrefixFit::Unknown);
    };
    let config = Config::load_or_default(&dirs.config_file()).unwrap_or_default();
    let steam = lightforge_core::steam::SteamRoot::discover();
    let builds = proton::discover_all(steam.as_ref(), &dirs.proton_dir());
    let Some(build) = proton::select(&builds, runs.pin(&config, code)) else {
        return (None, proton::PrefixFit::Unknown);
    };
    let built_by = proton::prefix_build(&layout.compat_data());
    let fit = proton::prefix_fit(&build.version, built_by.as_deref());
    (Some(build.version.clone()), fit)
}

fn runner_for(
    layout: &Layout,
    code: &ProductCode,
    runs: Runs,
) -> Result<lightforge_core::prefix::ProtonRunner, String> {
    use lightforge_core::prefix::ProtonRunner;
    use lightforge_core::proton;

    let dirs = AppDirs::discover().map_err(|e| chain(&e))?;
    let config = Config::load_or_default(&dirs.config_file()).unwrap_or_default();
    let steam = lightforge_core::steam::SteamRoot::discover();
    let builds = proton::discover_all(steam.as_ref(), &dirs.proton_dir());
    let build = proton::select(&builds, runs.pin(&config, code))
        .ok_or_else(|| "no supported Proton-GE build installed".to_owned())?;

    let target =
        std::fs::read_link(layout.wow_symlink()).unwrap_or_else(|_| layout.wow_install(code));
    let mut runner =
        ProtonRunner::new(build, layout, ProtonRunner::steam_client_path(steam.as_ref(), &dirs))
            .with_compat_mount(&target)
            // The prefix is shared, so pooling the shader cache would mean one
            // product's Proton swap discarding another's shaders.
            .with_shader_cache(dirs.shader_cache_dir(code));

    // Two profiles, because they are two different questions. A direct
    // launch is this product's own; a Battle.net launch is the client's and
    // therefore every game the client starts, the one environment all of
    // them inherit. See `Config::battle_net_tweaks` for why that cannot be
    // per-game.
    let tweaks = match runs {
        Runs::Game => config.product(code).map(|p| p.profile.tweaks.clone()).unwrap_or_default(),
        Runs::BattleNet => config.battle_net_tweaks.clone(),
    };
    for (key, value) in tweaks.environment(&HostTools::detect()) {
        runner = runner.with_env(key, value);
    }
    Ok(runner)
}

/// The shared profile Battle.net and its games launch under.
///
/// Not per-product on purpose; see `Config::battle_net_tweaks`.
fn config_tweaks() -> Tweaks {
    AppDirs::discover()
        .ok()
        .and_then(|dirs| Config::load_or_default(&dirs.config_file()).ok())
        .map(|config| config.battle_net_tweaks)
        .unwrap_or_default()
}

/// One product's performance settings, and what this machine can honour.
fn tweaks_for(code: &ProductCode) -> (Tweaks, HostTools) {
    let tweaks = AppDirs::discover()
        .ok()
        .and_then(|dirs| Config::load_or_default(&dirs.config_file()).ok())
        .and_then(|config| config.product(code).map(|p| p.profile.tweaks.clone()))
        .unwrap_or_default();
    (tweaks, HostTools::detect())
}

/// The Proton build that would be used, for the debug bundle.
pub fn proton_for(code: &ProductCode) -> Option<lightforge_core::proton::ProtonRelease> {
    use lightforge_core::proton;

    let dirs = AppDirs::discover().ok()?;
    let config = Config::load_or_default(&dirs.config_file()).unwrap_or_default();
    let steam = lightforge_core::steam::SteamRoot::discover();
    let builds = proton::discover_all(steam.as_ref(), &dirs.proton_dir());
    proton::select(&builds, config.effective_proton(code)).cloned()
}

/// Open a path in the desktop's file manager.
pub fn open_path(path: PathBuf) -> Result<(), String> {
    open::that_detached(&path).map_err(|e| format!("{}: {e}", path.display()))
}

/// Open a URL in the default browser.
pub fn open_url(url: &str) -> Result<(), String> {
    open::that_detached(url).map_err(|e| format!("{url}: {e}"))
}
