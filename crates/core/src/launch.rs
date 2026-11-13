//! Starting a game directly, the same way from the GUI and the CLI: a setting
//! one of them honoured and the other did not would make every launch
//! measured from the terminal unreproducible.

use std::path::PathBuf;
use std::process::Command;

use crate::config::Config;
use crate::paths::{AppDirs, Layout};
use crate::prefix::{PrefixError, ProtonRunner};
use crate::product::ProductCode;
use crate::proton::{self, ProtonRelease};
use crate::steam::SteamRoot;
use crate::tweaks::{HostTools, Tweaks};

#[derive(Debug, thiserror::Error)]
pub enum LaunchError {
    #[error(transparent)]
    Prefix(#[from] PrefixError),
    #[error("no installed Proton build tagged `{0}`")]
    NotInstalled(String),
    #[error("no supported Proton-GE build installed")]
    NoProton,
    #[error("no game at {}", .0.display())]
    NoGame(PathBuf),
}

/// The runner `code` starts under: the build `pin` names (see
/// [`proton::select`]), the product's own shader cache, and `tweaks`.
pub fn runner(
    layout: &Layout,
    dirs: &AppDirs,
    code: &ProductCode,
    pin: Option<&str>,
    tweaks: &Tweaks,
    tools: &HostTools,
) -> Result<(ProtonRunner, ProtonRelease), LaunchError> {
    if layout.prefix_away() {
        return Err(PrefixError::Away(layout.compat_data()).into());
    }
    let steam = SteamRoot::discover();
    let builds = proton::discover_all(steam.as_ref(), &dirs.proton_dir());
    let build = proton::select(&builds, pin).ok_or_else(|| match pin {
        Some(tag) => LaunchError::NotInstalled(tag.to_owned()),
        None => LaunchError::NoProton,
    })?;

    // Whatever the symlink resolves to has to be visible inside the prefix,
    // and it is not necessarily under the games root.
    let target = std::fs::read_link(layout.wow_symlink()).unwrap_or_else(|_| layout.wow_install(code));
    let mut runner = ProtonRunner::new(build, layout, ProtonRunner::steam_client_path(steam.as_ref(), dirs))
        .with_compat_mount(&target)
        // The prefix is shared, so pooling the shader cache would mean one
        // product's Proton swap discarding another's shaders.
        .with_shader_cache(dirs.shader_cache_dir(code));
    for (key, value) in tweaks.environment(tools) {
        runner = runner.with_env(key, value);
    }
    Ok((runner, build.clone()))
}

/// A game Lightforge starts itself, under the product's own settings.
pub struct DirectLaunch {
    pub build: ProtonRelease,
    pub runner: ProtonRunner,
    /// The native path, not the `C:` symlink, so Wine puts the game on its
    /// own drive ([`Layout::wow_drive`]) and the shop's browser keeps its GPU
    /// child. Verified 2026-10-02, a full checkout.
    pub exe: PathBuf,
    /// Our own wrappers, outermost first.
    pub wrappers: Vec<PathBuf>,
    pub launch_options: String,
}

impl DirectLaunch {
    /// `proton` overrides the product's pin.
    pub fn plan(
        config: &Config,
        layout: &Layout,
        dirs: &AppDirs,
        code: &ProductCode,
        proton: Option<&str>,
    ) -> Result<Self, LaunchError> {
        let exe = layout.wow_exe(code);
        if !exe.is_file() {
            return Err(LaunchError::NoGame(exe));
        }
        let profile = config.product(code).map(|p| p.profile.clone()).unwrap_or_default();
        let tools = HostTools::detect();
        let pin = proton.or_else(|| config.effective_proton(code));
        let (runner, build) = runner(layout, dirs, code, pin, &profile.tweaks, &tools)?;
        Ok(Self {
            build,
            runner,
            exe,
            wrappers: profile.tweaks.wrappers(&tools),
            launch_options: profile.launch_options,
        })
    }

    /// The launch options outermost, as Steam puts them: around every wrapper
    /// of our own.
    pub fn command(&self) -> Command {
        crate::launch_options::wrap(
            crate::tweaks::wrapped(self.runner.command("run", &self.exe, Vec::<String>::new()), &self.wrappers),
            &self.launch_options,
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_game_that_is_not_installed_is_refused_before_proton_is_looked_for() {
        let dir = tempfile::tempdir().unwrap();
        let layout = Layout::with_root(dir.path());
        let dirs = AppDirs::with_dirs(dir.path().join("config"), dir.path().join("cache"), dir.path().join("data"));
        let err = DirectLaunch::plan(&Config::default(), &layout, &dirs, &ProductCode::Retail, None).err().unwrap();
        assert!(matches!(err, LaunchError::NoGame(path) if path == layout.wow_exe(&ProductCode::Retail)));
    }
}
