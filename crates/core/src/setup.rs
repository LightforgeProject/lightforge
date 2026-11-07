//! First-run provisioning: Proton, the prefix, and Battle.net.
//!
//! In the only possible order. Every step is idempotent and reports itself
//! skipped, so a rerun resumes. Reports through [`Event`]s.

use std::path::{Path, PathBuf};

use crate::battlenet::{self, SetupReport};
use crate::config::Config;
use crate::net::Http;
use crate::paths::{AppDirs, Layout};
use crate::prefix::{self, PrefixStatus, ProtonRunner};
use crate::proton::{self, ProtonRelease};
use crate::runtime;
use crate::tact::productdb;

/// What provisioning still has to do on this machine.
#[derive(Debug, Clone)]
pub struct Status {
    /// The build a launch would use, if any is installed.
    pub proton: Option<ProtonRelease>,
    /// Every build Lightforge can see, managed and Steam alike.
    pub installed: Vec<ProtonRelease>,
    pub prefix: PrefixStatus,
    pub battle_net: bool,
    /// Whether Battle.net is wanted at all; declining it still finishes setup.
    pub battle_net_wanted: bool,
    /// Whether this machine has a 32-bit runtime: the cheap
    /// [`runtime::check_loader`]. [`run`] does the deeper check.
    pub thirty_two_bit: bool,
}

impl Status {
    /// Read the machine. Touches nothing.
    pub fn inspect(layout: &Layout, dirs: &AppDirs, config: &Config) -> Self {
        let steam = crate::steam::SteamRoot::discover();
        let installed = proton::discover_all(steam.as_ref(), &dirs.proton_dir());
        let proton = proton::select(&installed, config.default_proton.version()).cloned();

        Self {
            proton,
            installed,
            prefix: PrefixStatus::inspect(layout),
            battle_net: battlenet::Installer::new(layout, PathBuf::new()).is_installed(),
            battle_net_wanted: !config.battle_net_declined,
            thirty_two_bit: runtime::has_32bit_loader(),
        }
    }

    /// Nothing left to provision.
    pub fn is_ready(&self) -> bool {
        self.proton.is_some() && self.prefix.is_ready() && (self.battle_net || !self.battle_net_wanted)
    }

    /// The steps still outstanding, in the order they must run.
    pub fn outstanding(&self) -> Vec<Step> {
        let mut steps = Vec::new();
        if self.proton.is_none() {
            steps.push(Step::Proton);
        }
        if !self.prefix.is_ready() {
            steps.push(Step::Prefix);
        }
        if !self.battle_net && self.battle_net_wanted {
            steps.push(Step::BattleNet);
        }
        steps
    }
}

/// One provisioning step.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Step {
    Proton,
    Prefix,
    BattleNet,
}

impl Step {
    pub fn label(&self) -> &'static str {
        match self {
            Self::Proton => "Proton-GE",
            Self::Prefix => "Wine prefix",
            Self::BattleNet => "Battle.net",
        }
    }

    /// Roughly what this step is about to do.
    pub fn detail(&self) -> &'static str {
        match self {
            Self::Proton => "downloading and verifying a Proton-GE build (~400 MB)",
            Self::Prefix => "building the Wine prefix and linking the game directory",
            Self::BattleNet => "installing Battle.net into the prefix",
        }
    }
}

#[derive(Debug, Clone)]
pub enum Event {
    Started(Step),
    /// Bytes of a download within the current step.
    Progress { done: u64, total: Option<u64> },
    /// The step finished; `skipped` means it was already done.
    Finished { step: Step, skipped: bool, note: String },
}

/// Provision whatever is missing. Safe to call when nothing is.
///
/// `proton_version` pins which release to install when one is needed; `None`
/// takes [`proton::TESTED`], installing it even beside a newer build.
pub async fn run(
    layout: &Layout,
    dirs: &AppDirs,
    config: &Config,
    proton_version: Option<&str>,
    on_event: impl FnMut(Event),
) -> Result<Status, SetupError> {
    let mut on_event = logged(on_event);
    let http = Http::new()?;
    let mut status = Status::inspect(layout, dirs, config);
    tracing::info!(
        "setup: proton={:?} prefix_ready={} battle_net={} (wanted {}), pinned {proton_version:?}",
        status.proton.as_ref().map(|b| b.version.as_str()),
        status.prefix.is_ready(),
        status.battle_net,
        status.battle_net_wanted,
    );

    // Before any download: the installer is the only 32-bit step. Only when
    // Battle.net is wanted.
    if !status.battle_net && status.battle_net_wanted {
        runtime::check_loader()?;
    }

    // 1. Proton: the tested build, not just any (11-9 could not install Battle.net).
    let wanted = proton_version.unwrap_or(proton::TESTED);
    let build = match &status.proton {
        Some(build) if build.version == wanted => {
            on_event(Event::Finished {
                step: Step::Proton,
                skipped: true,
                note: build.version.clone(),
            });
            build.clone()
        }
        _ => {
            on_event(Event::Started(Step::Proton));
            let build = install_proton(&http, dirs, Some(wanted), &mut on_event).await?;
            on_event(Event::Finished {
                step: Step::Proton,
                skipped: false,
                note: build.version.clone(),
            });
            build
        }
    };

    let steam = crate::steam::SteamRoot::discover();
    let runner = ProtonRunner::new(
        &build,
        layout,
        ProtonRunner::steam_client_path(steam.as_ref(), dirs),
    );

    // 2. The prefix, plus the symlink that gives the game its Windows path.
    if status.prefix.is_ready() {
        on_event(Event::Finished {
            step: Step::Prefix,
            skipped: true,
            note: status.prefix.prefix.display().to_string(),
        });
    } else {
        on_event(Event::Started(Step::Prefix));
        let prefix = prefix::provision(&runner, layout)?;
        on_event(Event::Finished {
            step: Step::Prefix,
            skipped: false,
            note: prefix.prefix.display().to_string(),
        });
        status.prefix = prefix;
    }

    // 3. Battle.net, unless declined. Needs a working 32-bit wine; this check
    //    names a missing library the loader check cannot see.
    if !status.battle_net && !status.battle_net_wanted {
        status.proton = Some(build);
        return Ok(status);
    }

    //    Wine's HTTPS needs a 32-bit gnutls too, which neither check sees;
    //    without it the installer runs inside Steam's runtime, which brings
    //    its own (SteamOS ships none).
    let container = match status.battle_net {
        true => None,
        false => {
            runtime::check_wine(runner.dist())?;
            runtime::tls_container(&build.path, steam.as_ref())?
        }
    };

    if status.battle_net {
        on_event(Event::Finished {
            step: Step::BattleNet,
            skipped: true,
            note: layout.battle_net_dir().display().to_string(),
        });
    } else {
        on_event(Event::Started(Step::BattleNet));
        let report =
            install_battle_net(&http, layout, dirs, config, &runner, container.as_deref(), &mut on_event)
                .await?;
        on_event(Event::Finished {
            step: Step::BattleNet,
            skipped: false,
            note: match report.install.skipped {
                true => "already present".to_owned(),
                false => format!("installed in {:.0}s", report.install.waited.as_secs_f64()),
            },
        });
        status.battle_net = true;
    }

    status.proton = Some(build);
    Ok(status)
}

/// What a Battle.net repair carried over.
#[derive(Debug, Clone)]
pub struct Repaired {
    pub status: Status,
    /// Games put back into the new registry, by Battle.net's names for them.
    pub games: Vec<String>,
}

/// Reinstall Battle.net for a client that no longer comes back, Agent
/// "sleeping" after a Locate that never finished (GitHub #23), without losing
/// what it knew.
///
/// Every game's record in Agent's registry is lifted out first and grafted
/// into the new one, byte for byte, so nothing has to be found again. The
/// settings and sign-in under `AppData` are not touched, and no game's files
/// are in the folders that go. Those are set aside rather than deleted until
/// the new client is in; a failed install puts them back.
pub async fn repair_battle_net(
    layout: &Layout,
    dirs: &AppDirs,
    config: &Config,
    proton_version: Option<&str>,
    on_event: impl FnMut(Event),
) -> Result<Repaired, SetupError> {
    if config.battle_net_declined {
        return Err(SetupError::BattleNetDeclined);
    }
    let running = crate::preflight::scan(layout);
    if !running.is_empty() {
        return Err(SetupError::Busy(running.into_iter().map(|p| p.name).collect()));
    }

    let agent_dir = layout.agent_dir();
    let games = std::fs::read(agent_dir.join("product.db")).map(|bytes| game_records(&bytes)).unwrap_or_default();
    let program_data = layout.drive_c().join("ProgramData");
    let program = [
        layout.battle_net_dir(),
        agent_dir.clone(),
        program_data.join("Battle.net").join("Setup"),
        program_data.join("Battle.net").join("bdk"),
        program_data.join("Battle.net_components"),
    ];
    // In `drive_c`, so every move is a rename.
    let drive_c = layout.drive_c();
    let aside = crate::staging::create_in(&drive_c).map_err(io(&drive_c))?;
    let moved = set_aside(&program, aside.path())?;
    tracing::info!(
        "repairing Battle.net: {} folders set aside, {} games to keep",
        moved.len(),
        games.len()
    );

    let status = match run(layout, dirs, config, proton_version, on_event).await {
        Ok(status) if status.battle_net => status,
        outcome => {
            if !put_back(&moved) {
                let kept = aside.keep();
                tracing::error!("the old Battle.net is left at {}", kept.display());
            }
            return match outcome {
                Err(e) => Err(e),
                Ok(_) => Err(SetupError::BattleNetDeclined),
            };
        }
    };

    let names: Vec<String> = games.iter().map(|record| record.product.clone()).collect();
    if !games.is_empty() && !productdb::graft(&agent_dir, &games)? {
        tracing::warn!("the new Battle.net registry could not be read, so {names:?} need a Locate");
    }
    Ok(Repaired { status, games: names })
}

/// The records of every game in a registry, not Battle.net's own.
fn game_records(registry: &[u8]) -> Vec<productdb::Record> {
    productdb::decode(registry)
        .into_iter()
        .filter(|game| !game.is_infrastructure())
        .filter_map(|game| productdb::lift(registry, &game.product, &game.install_path))
        .collect()
}

/// Move each of `dirs` that exists into `aside`, numbered. A move that fails
/// takes the ones before it back.
fn set_aside(dirs: &[PathBuf], aside: &Path) -> Result<Vec<(PathBuf, PathBuf)>, SetupError> {
    let mut moved = Vec::new();
    for (n, dir) in dirs.iter().enumerate() {
        if !dir.exists() {
            continue;
        }
        let to = aside.join(n.to_string());
        if let Err(e) = std::fs::rename(dir, &to) {
            put_back(&moved);
            return Err(io(dir)(e));
        }
        moved.push((dir.clone(), to));
    }
    Ok(moved)
}

/// Undo [`set_aside`] over whatever a failed install left in its place.
/// `false` when something could not go back.
fn put_back(moved: &[(PathBuf, PathBuf)]) -> bool {
    let mut all = true;
    for (dir, aside) in moved.iter().rev() {
        let _ = std::fs::remove_dir_all(dir);
        if let Err(e) = std::fs::rename(aside, dir) {
            tracing::error!("could not put {} back from {}: {e}", dir.display(), aside.display());
            all = false;
        }
    }
    all
}

/// Write each step to the log on its way to the caller. Download progress is
/// left out; the downloader reports its own.
fn logged(mut on_event: impl FnMut(Event)) -> impl FnMut(Event) {
    move |event| {
        match &event {
            Event::Progress { .. } => {}
            Event::Started(step) => tracing::info!("setup: {} started", step.label()),
            Event::Finished { step, skipped, note } => {
                tracing::info!("setup: {} finished (skipped: {skipped}): {note}", step.label());
            }
        }
        on_event(event);
    }
}

/// Install one Proton build and nothing else.
///
/// Unlike [`run`], creates no prefix as a side effect.
pub async fn install_build(
    dirs: &AppDirs,
    version: &str,
    on_event: impl FnMut(Event),
) -> Result<ProtonRelease, SetupError> {
    let mut on_event = logged(on_event);
    let http = Http::new()?;
    on_event(Event::Started(Step::Proton));
    let build = install_proton(&http, dirs, Some(version), &mut on_event).await?;
    on_event(Event::Finished {
        step: Step::Proton,
        skipped: false,
        note: build.version.clone(),
    });
    Ok(build)
}

async fn install_proton(
    http: &Http,
    dirs: &AppDirs,
    version: Option<&str>,
    on_event: &mut impl FnMut(Event),
) -> Result<ProtonRelease, SetupError> {
    // A tag names its family; with none asked for, the newest GE is the
    // default, as it always was.
    let family = version
        .and_then(proton::ProtonFamily::of_tag)
        .unwrap_or(proton::ProtonFamily::GloriousEggroll);
    let cache = crate::net::ETagCache::new(dirs.asset_cache_dir().join(family.cache_file()));
    let releases = proton::fetch_releases(http, &cache, family).await?;

    let release = match version {
        Some(tag) => releases
            .iter()
            .find(|r| r.tag_name == tag)
            .ok_or_else(|| SetupError::NoSuchRelease(tag.to_owned()))?,
        None => proton::latest_stable(&releases).ok_or(SetupError::NoStableRelease)?,
    };

    let scratch = dirs.cache_dir().join("downloads");
    std::fs::create_dir_all(&scratch).map_err(io(&scratch))?;

    Ok(proton::install_release(http, release, &dirs.proton_dir(), &scratch, |done, total| {
        on_event(Event::Progress { done, total })
    })
    .await?)
}

async fn install_battle_net(
    http: &Http,
    layout: &Layout,
    dirs: &AppDirs,
    config: &Config,
    runner: &ProtonRunner,
    container: Option<&std::path::Path>,
    on_event: &mut impl FnMut(Event),
) -> Result<SetupReport, SetupError> {
    let installer = dirs.cache_dir().join(battlenet::INSTALLER_FILE);
    if !installer.is_file() {
        battlenet::fetch_installer(http, dirs.cache_dir(), |done, total| {
            on_event(Event::Progress { done, total })
        })
        .await?;
    }
    Ok(battlenet::provision(
        runner,
        container,
        layout,
        &config.prefix_user,
        &config.locale,
        &installer,
        &config.managed_products(),
    )?)
}

/// Where a path came from, for the settings UI.
pub fn describe_source(build: &ProtonRelease) -> &'static str {
    match build.source {
        proton::ProtonSource::Managed => "managed by Lightforge",
        proton::ProtonSource::SystemCompatTools => "in compatibilitytools.d",
        proton::ProtonSource::SteamApp => "installed as a Steam app",
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    #[error(transparent)]
    Net(#[from] crate::net::NetError),

    #[error(transparent)]
    Proton(#[from] proton::ProtonError),

    #[error(transparent)]
    Prefix(#[from] prefix::PrefixError),

    #[error(transparent)]
    Runtime(#[from] runtime::RuntimeError),

    #[error(transparent)]
    BattleNet(#[from] battlenet::BattleNetError),

    #[error("no Proton-GE release is tagged `{0}`")]
    NoSuchRelease(String),

    #[error("upstream has published only prereleases")]
    NoStableRelease,

    #[error("Battle.net is switched off in setup")]
    BattleNetDeclined,

    #[error("close these first, they are running in the prefix: {}", .0.join(", "))]
    Busy(Vec<String>),

    #[error(transparent)]
    ProductDb(#[from] productdb::ProductDbError),

    #[error("i/o error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> SetupError + '_ {
    move |source| SetupError::Io { path: path.to_path_buf(), source }
}


/// Convenience for a caller that only wants to know whether to show setup.
pub fn is_ready(layout: &Layout, dirs: &AppDirs, config: &Config) -> bool {
    Status::inspect(layout, dirs, config).is_ready()
}

/// The directory a first run would install Proton into, for display.
pub fn proton_dir(dirs: &AppDirs) -> PathBuf {
    dirs.proton_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(proton: bool, prefix_ready: bool, battle_net: bool) -> Status {
        let layout = Layout::with_root("/tmp/lightforge-setup-test");
        let mut prefix = PrefixStatus::inspect(&layout);
        prefix.initialized = prefix_ready;
        if prefix_ready {
            prefix.link = prefix::LinkState::Correct;
        }

        Status {
            proton: proton.then(|| ProtonRelease {
                version: "GE-Proton11-7".into(),
                flavor: proton::ProtonFlavor::GloriousEggroll,
                source: proton::ProtonSource::Managed,
                path: PathBuf::from("/tmp/ge"),
                built_at: None,
            }),
            installed: Vec::new(),
            prefix,
            battle_net,
            battle_net_wanted: true,
            // These tests are about which steps are outstanding, which is a
            // question about the disk, not about the host's libraries.
            thirty_two_bit: true,
        }
    }

    #[test]
    fn outstanding_steps_come_back_in_dependency_order() {
        let steps = status(false, false, false).outstanding();
        assert_eq!(steps, vec![Step::Proton, Step::Prefix, Step::BattleNet]);
    }

    #[test]
    fn a_provisioned_machine_has_nothing_outstanding() {
        let ready = status(true, true, true);
        assert!(ready.is_ready());
        assert!(ready.outstanding().is_empty());
    }

    #[test]
    fn a_machine_that_declined_battle_net_is_still_finished() {
        let mut status = status(true, true, false);
        status.battle_net_wanted = false;
        assert!(status.is_ready(), "WoW alone is a supported setup");
        assert!(status.outstanding().is_empty());

        // And the same machine with Battle.net wanted is not.
        status.battle_net_wanted = true;
        assert!(!status.is_ready());
        assert_eq!(status.outstanding(), vec![Step::BattleNet]);
    }

    #[test]
    fn a_missing_middle_step_does_not_hide_the_later_one() {
        // Proton present, prefix not built, Battle.net obviously absent: both
        // remaining steps must be reported, in order.
        let steps = status(true, false, false).outstanding();
        assert_eq!(steps, vec![Step::Prefix, Step::BattleNet]);
    }

    fn registry(products: &[&str]) -> Vec<u8> {
        let install = |product: &str| productdb::ProductInstall {
            product: product.into(),
            install_path: "E:/Hearthstone".into(),
            region: "eu".into(),
            locale: "enUS".into(),
            account_country: String::new(),
            geo_country: String::new(),
            flavor_dir: String::new(),
            version: "1.0".into(),
            build_key: "k".repeat(32),
            tags: String::new(),
            key_ring: None,
            family: String::new(),
            installed_at_ms: 0,
        };
        productdb::encode(&products.iter().map(|p| install(p)).collect::<Vec<_>>())
    }

    #[test]
    fn a_repair_keeps_every_game_and_none_of_battle_nets_own_records() {
        let old = registry(&["agent", "battle.net", "hs_beta", "wowf"]);
        let kept: Vec<String> = game_records(&old).into_iter().map(|r| r.product).collect();
        assert_eq!(kept, ["hs_beta", "wowf"]);
    }

    #[test]
    fn the_kept_games_land_in_the_new_registry_beside_its_own() {
        let dir = tempfile::tempdir().unwrap();
        let games = game_records(&registry(&["agent", "hs_beta", "wowf"]));
        // What a fresh install's Agent writes: only itself.
        std::fs::write(dir.path().join("product.db"), registry(&["agent", "battle.net"])).unwrap();

        assert!(productdb::graft(dir.path(), &games).unwrap());
        let now: Vec<String> = productdb::read(&dir.path().join("product.db")).unwrap().into_iter().map(|i| i.product).collect();
        assert_eq!(now, ["agent", "battle.net", "hs_beta", "wowf"]);
    }

    #[test]
    fn set_aside_folders_go_back_over_what_a_failed_install_left() {
        let dir = tempfile::tempdir().unwrap();
        let (client, agent, missing) = (dir.path().join("Battle.net"), dir.path().join("Agent"), dir.path().join("Setup"));
        std::fs::create_dir_all(&client).unwrap();
        std::fs::write(client.join("Battle.net.exe"), "old").unwrap();
        std::fs::create_dir_all(&agent).unwrap();
        std::fs::write(agent.join("product.db"), "registry").unwrap();
        let aside = dir.path().join("aside");
        std::fs::create_dir(&aside).unwrap();

        let moved = set_aside(&[client.clone(), agent.clone(), missing.clone()], &aside).unwrap();
        assert_eq!(moved.len(), 2, "a folder that was never there is skipped");
        assert!(!client.exists() && !agent.exists());

        // A half-finished install in the client's place.
        std::fs::create_dir_all(&client).unwrap();
        std::fs::write(client.join("partial"), "new").unwrap();
        assert!(put_back(&moved));

        assert_eq!(std::fs::read_to_string(client.join("Battle.net.exe")).unwrap(), "old");
        assert!(!client.join("partial").exists());
        assert_eq!(std::fs::read_to_string(agent.join("product.db")).unwrap(), "registry");
        assert!(!missing.exists());
    }

    #[tokio::test]
    async fn a_declined_battle_net_is_not_repaired() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::with_root(dir.path().join("Blizzard"));
        let dirs = AppDirs::with_dirs(dir.path().join("c"), dir.path().join("k"), dir.path().join("d"));
        let config = Config { battle_net_declined: true, ..Config::with_default_products() };

        let result = repair_battle_net(&layout, &dirs, &config, None, |_| {}).await;
        assert!(matches!(result, Err(SetupError::BattleNetDeclined)), "{result:?}");
    }
}
