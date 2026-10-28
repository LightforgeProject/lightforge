//! `lightforge doctor` and the Diagnostics block in settings: what this
//! machine has, held against what Battle.net and WoW under Proton need.
//!
//! Read-only, and nothing fails: a fact that cannot be read is reported as
//! such.

use std::fmt::Write as _;
use std::path::Path;

use serde::Serialize;

use crate::config::Config;
use crate::paths::Layout;
use crate::proton::{self, ProtonSource};
use crate::runtime;

/// The hard `nofile` limit esync needs; systemd 240 and newer set it.
const NOFILE_MIN: u64 = 524_288;

/// Free space a library should keep: a retail patch day can stage 15 GiB.
const FREE_MIN: u64 = 20 << 30;

/// Where Vulkan loaders look for drivers.
const ICD_DIRS: &[&str] = &["/usr/share/vulkan/icd.d", "/etc/vulkan/icd.d"];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Status {
    Ok,
    Warn,
    Fail,
}

impl Status {
    pub fn label(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Warn => "warn",
            Self::Fail => "fail",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Check {
    pub name: String,
    pub status: Status,
    pub detail: String,
    /// What to do about it, when there is something to do.
    pub hint: Option<String>,
}

fn check(name: impl Into<String>, status: Status, detail: impl Into<String>, hint: Option<String>) -> Check {
    Check { name: name.into(), status, detail: detail.into(), hint }
}

pub fn run(config: &Config, layout: &Layout, proton_dir: &Path) -> Vec<Check> {
    let mut checks = vec![open_files(), ntsync(), vulkan(), loader(), gnutls()];
    checks.extend(libraries(layout));
    checks.push(proton(config, proton_dir));
    checks.push(prefix(layout));
    if let Some(version) = crate::debug::nvidia_version() {
        checks.extend(nvidia(&version));
    }
    checks
}

/// The plain-text report, for a terminal or a pasted bug report.
pub fn render(checks: &[Check]) -> String {
    let mut out = String::new();
    for c in checks {
        let _ = writeln!(out, "{:<6}{:<20}{}", c.status.label(), c.name, c.detail);
        if let Some(hint) = &c.hint {
            let _ = writeln!(out, "{:<26}{hint}", "");
        }
    }
    crate::debug::scrub(&out)
}

fn open_files() -> Check {
    let Some((soft, hard)) = std::fs::read_to_string("/proc/self/limits").ok().as_deref().and_then(nofile) else {
        return check("open files", Status::Warn, "could not read /proc/self/limits", None);
    };
    let show = |n: u64| if n == u64::MAX { "unlimited".to_owned() } else { n.to_string() };
    let detail = format!("soft {}, hard {}", show(soft), show(hard));
    match hard < NOFILE_MIN {
        // Proton raises the soft limit to the hard one itself.
        true => check(
            "open files",
            Status::Warn,
            detail,
            Some(format!(
                "esync needs a hard limit of {NOFILE_MIN}: set DefaultLimitNOFILE in systemd's \
                 system.conf and user.conf"
            )),
        ),
        false => check("open files", Status::Ok, detail, None),
    }
}

/// Soft and hard `Max open files` from `/proc/<pid>/limits`.
fn nofile(limits: &str) -> Option<(u64, u64)> {
    let line = limits.lines().find(|l| l.starts_with("Max open files"))?;
    let mut values = line["Max open files".len()..].split_whitespace().map(|v| match v {
        "unlimited" => Some(u64::MAX),
        n => n.parse().ok(),
    });
    Some((values.next()??, values.next()??))
}

fn ntsync() -> Check {
    match Path::new("/dev/ntsync").exists() {
        true => check("ntsync", Status::Ok, "available", None),
        false => check(
            "ntsync",
            Status::Ok,
            "not available, Proton uses fsync",
            Some("ntsync needs kernel 6.14 or newer with the ntsync module loaded".to_owned()),
        ),
    }
}

fn vulkan() -> Check {
    let mut drivers: Vec<String> = ICD_DIRS
        .iter()
        .filter_map(|dir| std::fs::read_dir(dir).ok())
        .flatten()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .filter(|name| name.ends_with(".json"))
        .collect();
    drivers.sort();
    match drivers.is_empty() {
        true => check(
            "vulkan",
            Status::Fail,
            "no Vulkan driver found",
            Some("install your GPU's Vulkan driver; DXVK and VKD3D cannot run without one".to_owned()),
        ),
        false => check("vulkan", Status::Ok, drivers.join(", "), None),
    }
}

fn loader() -> Check {
    if runtime::has_32bit_loader() {
        return check("32-bit runtime", Status::Ok, "present", None);
    }
    let remedy = runtime::remedy();
    let hint = match remedy.command {
        Some(command) => format!("Battle.net is 32-bit, the games are not; on {}: {command}", remedy.distro),
        None => "Battle.net is 32-bit, the games are not; install your distribution's 32-bit libraries"
            .to_owned(),
    };
    check("32-bit runtime", Status::Warn, "no 32-bit loader", Some(hint))
}

fn gnutls() -> Check {
    match runtime::has_32bit_tls() {
        Some(true) => check("32-bit gnutls", Status::Ok, "present", None),
        Some(false) => check(
            "32-bit gnutls",
            Status::Warn,
            "missing",
            Some("Battle.net then runs inside the Steam Linux Runtime, which Steam has to have installed".to_owned()),
        ),
        None => check("32-bit gnutls", Status::Ok, "unknown, ldconfig could not be read", None),
    }
}

fn libraries(layout: &Layout) -> Vec<Check> {
    let gib = |bytes: u64| bytes as f64 / f64::from(1u32 << 30);
    layout
        .libraries()
        .all()
        .iter()
        .map(|library| {
            let name = format!("space on {}", library.label);
            let Some(disk) = library.disk() else {
                let detail = library
                    .state()
                    .reason(&library.label)
                    .unwrap_or_else(|| format!("{} could not be read", library.games.display()));
                return check(name, Status::Warn, detail, None);
            };
            let detail = format!(
                "{:.1} GiB free of {:.0} GiB at {}",
                gib(disk.free),
                gib(disk.total),
                library.games.display()
            );
            match disk.free < FREE_MIN {
                true => check(name, Status::Warn, detail, Some("a patch can need 15 GiB of room".to_owned())),
                false => check(name, Status::Ok, detail, None),
            }
        })
        .collect()
}

fn proton(config: &Config, proton_dir: &Path) -> Check {
    let steam = crate::steam::SteamRoot::discover();
    let builds = proton::discover_all(steam.as_ref(), proton_dir);
    let managed = builds.iter().filter(|b| b.source == ProtonSource::Managed).count();
    let pin = config.default_proton.version();

    let Some(build) = proton::select(&builds, pin) else {
        return match pin {
            Some(pin) => check(
                "proton",
                Status::Fail,
                format!("pinned {pin} is not installed"),
                Some("install it under Settings, Proton, or remove the pin".to_owned()),
            ),
            None => check(
                "proton",
                Status::Fail,
                "no supported Proton build",
                Some(format!("setup installs {}, or run `lightforge proton-install`", proton::TESTED)),
            ),
        };
    };
    let detail = |why: &str| format!("{} ({why}), {managed} managed by Lightforge", build.version);
    match (pin, build.version == proton::TESTED) {
        (Some(_), _) => check("proton", Status::Ok, detail("pinned"), None),
        (None, true) => check("proton", Status::Ok, detail("tested default"), None),
        (None, false) => check(
            "proton",
            Status::Warn,
            detail("newest installed"),
            Some(format!("Lightforge is tested with {}", proton::TESTED)),
        ),
    }
}

fn prefix(layout: &Layout) -> Check {
    let status = crate::prefix::PrefixStatus::inspect(layout);
    if !status.initialized {
        return check(
            "prefix",
            Status::Warn,
            "not set up yet",
            Some("run setup from the app, or `lightforge setup`".to_owned()),
        );
    }
    let detail = format!(
        "built by {}, game link {}",
        status.built_by.as_deref().unwrap_or("an unknown Proton"),
        status.link.describe()
    );
    match status.link.is_usable() {
        true => check("prefix", Status::Ok, detail, None),
        false => check("prefix", Status::Warn, detail, Some("`lightforge prefix-link` repairs the link".to_owned())),
    }
}

fn nvidia(version: &str) -> Vec<Check> {
    let host = crate::tweaks::HostTools::detect();
    let driver = match host.wayland && host.nvidia_lacks_explicit_sync() {
        true => check(
            "nvidia driver",
            Status::Warn,
            version,
            Some("Wayland needs 555 or newer for explicit sync; see TROUBLESHOOTING.md".to_owned()),
        ),
        false => check("nvidia driver", Status::Ok, version, None),
    };

    let modeset = match std::fs::read_to_string("/sys/module/nvidia_drm/parameters/modeset") {
        Ok(value) if value.trim() == "Y" => check("nvidia modeset", Status::Ok, "on", None),
        Ok(_) => check(
            "nvidia modeset",
            Status::Warn,
            "off",
            Some("Wayland and gamescope need nvidia_drm.modeset=1 on the kernel command line".to_owned()),
        ),
        Err(_) => check("nvidia modeset", Status::Warn, "nvidia_drm is not loaded", None),
    };

    let gl32 = match runtime::has_32bit_lib("libGLX_nvidia.so.0") {
        Some(true) => check("nvidia 32-bit gl", Status::Ok, "present", None),
        Some(false) => {
            let remedy = runtime::nvidia_remedy(version);
            let fix = match (remedy.command, remedy.note) {
                (Some(command), Some(note)) => format!("; on {}: {command} ({note})", remedy.distro),
                (Some(command), None) => format!("; on {}: {command}", remedy.distro),
                (None, Some(note)) => format!("; {note}"),
                (None, None) => String::new(),
            };
            check(
                "nvidia 32-bit gl",
                Status::Warn,
                "missing",
                Some(format!("Battle.net is 32-bit and needs the driver's 32-bit libraries{fix}")),
            )
        }
        None => check("nvidia 32-bit gl", Status::Ok, "unknown, ldconfig could not be read", None),
    };

    vec![driver, modeset, gl32]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `/proc/self/limits` from a CachyOS desktop, 2026-10-28, trailing
    /// spaces and all.
    const LIMITS: &str = "\
Limit                     Soft Limit           Hard Limit           Units     
Max cpu time              unlimited            unlimited            seconds   
Max open files            1048576              2097152              files     
Max locked memory         8388608              8388608              bytes     
";

    #[test]
    fn the_open_file_limit_is_read_from_proc() {
        assert_eq!(nofile(LIMITS), Some((1_048_576, 2_097_152)));
    }

    #[test]
    fn an_unlimited_open_file_limit_counts_as_enough() {
        let limits = LIMITS.replace("1048576              2097152", "unlimited            unlimited");
        assert_eq!(nofile(&limits), Some((u64::MAX, u64::MAX)));
    }

    #[test]
    fn a_report_puts_hints_under_their_check() {
        let report = render(&[
            check("ntsync", Status::Ok, "available", None),
            check("open files", Status::Warn, "soft 1024, hard 4096", Some("raise it".to_owned())),
        ]);
        let lines: Vec<&str> = report.lines().collect();
        assert_eq!(lines.len(), 3);
        assert!(lines[1].starts_with("warn  open files"));
        assert_eq!(lines[2].trim(), "raise it");
    }
}
