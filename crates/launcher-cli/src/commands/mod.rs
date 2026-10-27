//! One function per command, grouped the way `--help` reads.

use crate::*;

pub(crate) mod info;
pub(crate) mod proton;
pub(crate) mod battlenet;
pub(crate) mod play;
pub(crate) mod download;
pub(crate) mod addons;
pub(crate) mod storage;
pub(crate) mod inspect;

/// Clap's check on every product code argument: a custom code becomes a
/// folder name, so it is held to the same rule as one in the config.
pub(crate) fn product_code(code: &str) -> Result<String, String> {
    match lightforge_core::product::is_well_formed(code) {
        true => Ok(code.to_owned()),
        false => Err("a product code is letters, digits and `_` only, e.g. `wow_classic`".to_owned()),
    }
}

/// The install a store-level command (`verify`, `reindex`, `product-db`)
/// works on without `--path`: the default library's.
pub(crate) fn default_install(layout: &Layout) -> std::path::PathBuf {
    layout
        .default_library()
        .map(|library| layout.wow_install_in(library))
        .unwrap_or_else(|| layout.games_root().join(lightforge_core::paths::WOW_DIR))
}

/// How Battle.net addresses a product's install, which depends on the library
/// it is in; see [`lightforge_core::library::Library::windows_wow_path`].
pub(crate) fn windows_wow_path(layout: &Layout, code: &ProductCode) -> String {
    layout
        .library(code)
        .map(|library| library.windows_wow_path())
        .unwrap_or_else(|| lightforge_core::paths::WINDOWS_WOW_PATH.to_owned())
}

/// Render what a pre-flight pass did, one aligned line per fact.
pub(crate) fn print_preflight(report: &lightforge_core::preflight::Report) {
    use lightforge_core::preflight::Disposition;

    if report.processes.is_empty() {
        println!("processes       none running");
    }
    for outcome in &report.processes {
        let how = match outcome.disposition {
            Disposition::Wineserver => "wineserver -k",
            Disposition::Terminated => "SIGTERM",
            Disposition::Killed => "SIGKILL",
            Disposition::Survived => "SURVIVED",
        };
        println!(
            "processes       {:<24} pid {:<8} {how}",
            outcome.process.name, outcome.process.pid
        );
    }

    for (label, bytes) in [
        ("agent cache", report.agent_cache_removed),
        ("staging", report.staging_removed),
    ] {
        match bytes {
            Some(n) => println!("{label:<15} removed, {:.0} MB reclaimed", n as f64 / 1_048_576.0),
            None => println!("{label:<15} absent"),
        }
    }

    for config in &report.configs {
        println!("config          re-applied {}", config.display());
    }
    if report.download_paused {
        println!("download        paused for the session");
    }

    // Last, because it is the answer to a different question: not what the
    // pass did, but where it spent the time. A launch that feels stuck is one
    // of these stages, and a signal grace is five seconds where a scan is
    // sixty milliseconds.
    for timing in &report.timings {
        println!("timing          {:<18} {:>6} ms", timing.stage, timing.took.as_millis());
    }
}

/// Products Lightforge patches, read fresh from `config.toml`.
///
/// Read per call rather than threaded through: pre-flight runs once per
/// launch, and a stale list here would leave a product's auto-update on.
pub(crate) fn managed_products() -> Vec<lightforge_core::product::ProductCode> {
    AppDirs::discover()
        .ok()
        .and_then(|dirs| Config::load_or_default(&dirs.config_file()).ok())
        .map(|config| config.managed_products())
        .unwrap_or_default()
}

/// Resolve the Proton build to run against the shared prefix, and a runner for
/// it. `None` selects the tested build, see `proton::select`.
pub(crate) fn proton_runner(
    layout: &Layout,
    version: Option<&str>,
) -> Result<(
    lightforge_core::prefix::ProtonRunner,
    lightforge_core::proton::ProtonRelease,
)> {
    use lightforge_core::prefix::ProtonRunner;
    use lightforge_core::proton;

    let dirs = AppDirs::discover().context("resolving XDG directories")?;
    let steam = lightforge_core::steam::SteamRoot::discover();
    let builds = proton::discover_all(steam.as_ref(), &dirs.proton_dir());
    let build = proton::select(&builds, version).with_context(|| match version {
        Some(tag) => format!("no installed Proton build tagged `{tag}`"),
        None => {
            "no supported Proton-GE build installed — run `lightforge proton-install`".to_owned()
        }
    })?;

    let runner = ProtonRunner::new(
        build,
        layout,
        ProtonRunner::steam_client_path(steam.as_ref(), &dirs),
    );
    Ok((runner, build.clone()))
}

pub(crate) fn truncate(text: &str, width: usize) -> String {
    match text.chars().count() > width {
        true => format!("{}…", text.chars().take(width - 1).collect::<String>()),
        false => text.to_owned(),
    }
}
