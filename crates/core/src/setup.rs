//! First-run provisioning: Proton, the prefix, and Battle.net.
//!
//! The three steps a user otherwise has to run from the CLI before anything in
//! the dashboard works, in the only order they can be done in, Proton has to
//! exist before a prefix can be built with it, and the prefix has to exist
//! before Battle.net can be installed into it.
//!
//! Every step is idempotent and reports itself skipped when there is nothing to
//! do, so running setup again after a partial failure resumes rather than
//! redoes. Like [`crate::tact::install`], it reports through an [`Event`]
//! callback so both front-ends can drive it.

use std::path::PathBuf;

use crate::battlenet::{self, SetupReport};
use crate::config::Config;
use crate::net::Http;
use crate::paths::{AppDirs, Layout};
use crate::prefix::{self, PrefixStatus, ProtonRunner};
use crate::proton::{self, ProtonRelease};
use crate::runtime;

/// What provisioning still has to do on this machine.
#[derive(Debug, Clone)]
pub struct Status {
    /// The build a launch would use, if any is installed.
    pub proton: Option<ProtonRelease>,
    /// Every build Lightforge can see, managed and Steam alike.
    pub installed: Vec<ProtonRelease>,
    pub prefix: PrefixStatus,
    pub battle_net: bool,
    /// Whether Battle.net is wanted at all.
    ///
    /// Declining it during setup is a supported choice, WoW is downloaded and
    /// launched by Lightforge alone, so a machine without it is not an
    /// unfinished one. Everything that asks "is setup done" has to know the
    /// difference, or the dashboard spends forever offering to finish a step
    /// the user said no to.
    pub battle_net_wanted: bool,
    /// Whether this machine has a 32-bit runtime at all.
    ///
    /// Only the cheap loader check ([`runtime::check_loader`]), one `stat`,
    /// so inspecting is still free enough to do on every reload. The deeper
    /// check that actually starts Proton's 32-bit wine runs in [`run`], where
    /// Proton is guaranteed to exist and the cost is paid once.
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

    /// Nothing left to provision: the dashboard can install, launch and hand
    /// off to Battle.net without help from the CLI.
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

    /// Roughly what this step is about to do, for a UI that wants to say so
    /// before it starts rather than after.
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
    mut on_event: impl FnMut(Event),
) -> Result<Status, SetupError> {
    let http = Http::new()?;
    let mut status = Status::inspect(layout, dirs, config);

    // Before anything is downloaded. Battle.net's installer is 32-bit, and it
    // is the only 32-bit thing in this sequence, so a machine with no 32-bit
    // userspace sails through Proton and prefix creation and dies on the last
    // step. Checking here costs one `stat` and saves a 400 MB download the
    // user could not have used. Only when Battle.net is still to come: WoW
    // alone is 64-bit, and a setup that declined Battle.net needs none of it.
    if !status.battle_net && status.battle_net_wanted {
        runtime::check_loader()?;
    }

    // 1. Proton. Everything else runs *through* a Proton build, so this is
    //    first whether the user cares about it or not.
    //    Not "any build will do": a newer one found in Steam's folders is not
    //    one Lightforge was tested on, and 11-9 could not install Battle.net.
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

    // 3. Battle.net, which needs a built prefix to install into, and, unlike
    //    every step before it, a working 32-bit wine. The loader check above
    //    catches a system with no 32-bit userspace at all; this catches one
    //    where it exists but a library Proton's wine needs does not, and the
    //    dynamic loader says which.
    //
    //    Skipped entirely when it was declined: the user is told at that point
    //    that only WoW will work, and Settings can install it later.
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

/// Install one Proton build and nothing else.
///
/// [`run`] also installs a build when one is missing, but it goes on to the
/// prefix and Battle.net afterwards, which is right for "set this machine
/// up" and wrong for "add this build to the list". Pressing Install beside a
/// version on the Proton page should not create a Wine prefix as a side
/// effect, and on a machine that is not provisioned yet it would.
pub async fn install_build(
    dirs: &AppDirs,
    version: &str,
    mut on_event: impl FnMut(Event),
) -> Result<ProtonRelease, SetupError> {
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
    std::fs::create_dir_all(&scratch).map_err(|source| SetupError::Io {
        path: scratch.clone(),
        source,
    })?;

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
        proton::ProtonSource::SystemCompatTools => "Steam compatibilitytools.d",
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

    #[error("i/o error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
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
}
