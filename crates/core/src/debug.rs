//! The "Copy Debug Info" bundle.
//!
//! One block of text a user can paste into a bug report without hunting for
//! version numbers. It lives here rather than in the GUI because it is only
//! gathering facts, and because the CLI wants the same block when someone is
//! being helped over a terminal.
//!
//! Nothing here fails: a fact that cannot be read is reported as unavailable
//! and the rest is still produced. A debug bundle that refuses to render
//! because one file is missing is useless exactly when it is needed.

use std::fmt::Write as _;
use std::path::{Path, PathBuf};

use crate::paths::Layout;
use crate::product::ProductCode;
use crate::proton::ProtonRelease;

/// Where crash dumps and error logs land inside a WoW install.
const ERROR_DIRS: &[&str] = &["Errors", "Logs", "Crashes"];

/// Everything the bundle reports about one machine.
#[derive(Debug, Clone, Default)]
pub struct DebugInfo {
    pub app_version: String,
    pub product: String,
    pub action_state: String,
    pub install_path: Option<PathBuf>,
    pub build_version: Option<String>,
    pub build_key: Option<String>,
    pub proton: Option<String>,
    pub proton_path: Option<PathBuf>,
    pub dxvk: Option<String>,
    pub vkd3d: Option<String>,
    pub prefix: PathBuf,
    /// Most recent crash/error log: path, modified time, and its tail.
    pub last_crash: Option<CrashLog>,
}

#[derive(Debug, Clone)]
pub struct CrashLog {
    pub path: PathBuf,
    pub modified: Option<std::time::SystemTime>,
    pub tail: String,
}

/// Lines of a log to keep. Enough to show the failure, short enough to paste
/// into a chat window without anyone complaining.
const TAIL_LINES: usize = 40;

impl DebugInfo {
    /// Gather what can be read. `proton` is the build the product would launch
    /// under, if one is resolvable.
    pub fn gather(
        layout: &Layout,
        code: &ProductCode,
        action_state: &str,
        proton: Option<&ProtonRelease>,
    ) -> Self {
        let install = layout.product_dir(code);
        let info = crate::buildinfo::BuildInfo::load(layout.build_info(code)).ok();
        let cell = |column: &str| {
            info.as_ref()
                .and_then(|i| i.get(code, column).ok())
                .map(str::to_owned)
                .filter(|v| !v.is_empty())
        };

        Self {
            app_version: env!("CARGO_PKG_VERSION").to_owned(),
            product: code.as_str().to_owned(),
            action_state: action_state.to_owned(),
            install_path: install.is_dir().then(|| install.clone()),
            build_version: cell(crate::buildinfo::COL_VERSION),
            build_key: cell(crate::buildinfo::COL_BUILD_KEY),
            proton: proton.map(|p| p.version.clone()),
            proton_path: proton.map(|p| p.path.clone()),
            dxvk: proton.and_then(|p| component_version(&p.path, "dxvk")),
            vkd3d: proton.and_then(|p| component_version(&p.path, "vkd3d-proton")),
            prefix: layout.prefix(),
            last_crash: newest_log(&install),
        }
    }

    /// Render the bundle as the block that gets copied.
    pub fn render(&self) -> String {
        let mut out = String::new();
        let row = |out: &mut String, key: &str, value: &str| {
            let _ = writeln!(out, "{key:<16}{value}");
        };
        let opt = |v: &Option<String>| v.clone().unwrap_or_else(|| "unavailable".to_owned());
        let opt_path =
            |v: &Option<PathBuf>| v.as_ref().map_or("unavailable".to_owned(), |p| p.display().to_string());

        let _ = writeln!(out, "--- Lightforge debug info ---");
        row(&mut out, "lightforge", &self.app_version);
        row(&mut out, "product", &self.product);
        row(&mut out, "state", &self.action_state);
        row(&mut out, "install", &opt_path(&self.install_path));
        row(&mut out, "version", &opt(&self.build_version));
        row(&mut out, "build key", &opt(&self.build_key));
        row(&mut out, "proton", &opt(&self.proton));
        row(&mut out, "proton path", &opt_path(&self.proton_path));
        row(&mut out, "dxvk", &opt(&self.dxvk));
        row(&mut out, "vkd3d", &opt(&self.vkd3d));
        row(&mut out, "prefix", &self.prefix.display().to_string());
        row(&mut out, "os", &os_release());
        row(&mut out, "kernel", &kernel());

        match &self.last_crash {
            Some(log) => {
                let _ = writeln!(out, "\n--- last crash log: {} ---", log.path.display());
                let _ = writeln!(out, "{}", log.tail);
            }
            None => {
                let _ = writeln!(out, "\nno crash log found");
            }
        }
        out
    }
}

/// Read a bundled component's version out of a Proton build.
///
/// Proton ships these as plain text next to its own `version` file; the exact
/// layout has moved between releases, so several known locations are tried
/// rather than assuming one.
fn component_version(proton: &Path, component: &str) -> Option<String> {
    let candidates = [
        proton.join(format!("{component}.version")),
        proton.join("files").join(format!("lib64/{component}/version")),
        proton.join("files").join(format!("share/{component}/version")),
    ];
    candidates
        .iter()
        .find_map(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.trim().to_owned())
        .filter(|s| !s.is_empty())
}

/// The newest file in any of the install's error/log directories.
fn newest_log(install: &Path) -> Option<CrashLog> {
    let mut newest: Option<(std::time::SystemTime, PathBuf)> = None;

    for dir in ERROR_DIRS {
        let Ok(entries) = std::fs::read_dir(install.join(dir)) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            if !meta.is_file() {
                continue;
            }
            let Ok(modified) = meta.modified() else { continue };
            if newest.as_ref().is_none_or(|(t, _)| modified > *t) {
                newest = Some((modified, entry.path()));
            }
        }
    }

    let (modified, path) = newest?;
    let text = std::fs::read_to_string(&path).unwrap_or_default();
    let lines: Vec<&str> = text.lines().collect();
    let tail = lines[lines.len().saturating_sub(TAIL_LINES)..].join("\n");

    Some(CrashLog { path, modified: Some(modified), tail })
}

fn os_release() -> String {
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|text| {
            text.lines()
                .find_map(|l| l.strip_prefix("PRETTY_NAME=").map(|v| v.trim_matches('"').to_owned()))
        })
        .unwrap_or_else(|| "unknown".to_owned())
}

fn kernel() -> String {
    std::fs::read_to_string("/proc/sys/kernel/osrelease")
        .map(|s| s.trim().to_owned())
        .unwrap_or_else(|_| "unknown".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bundle_renders_every_row_even_with_nothing_available() {
        let info = DebugInfo {
            app_version: "0.1.0".into(),
            product: "wow".into(),
            action_state: "Install".into(),
            prefix: PathBuf::from("/tmp/prefix"),
            ..DebugInfo::default()
        };
        let text = info.render();

        for key in ["lightforge", "product", "proton", "dxvk", "vkd3d", "prefix", "os", "kernel"] {
            assert!(text.contains(key), "missing row: {key}");
        }
        assert!(text.contains("unavailable"));
        assert!(text.contains("no crash log found"));
    }

    #[test]
    fn the_newest_log_across_directories_wins_and_is_tailed() {
        let dir = std::env::temp_dir().join("lightforge-debug-logs");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Errors")).unwrap();
        std::fs::create_dir_all(dir.join("Logs")).unwrap();

        std::fs::write(dir.join("Logs/old.log"), "old\n").unwrap();
        let body: String = (0..100).map(|i| format!("line {i}\n")).collect();
        std::fs::write(dir.join("Errors/crash.txt"), &body).unwrap();

        let log = newest_log(&dir).expect("a log");
        assert!(log.path.ends_with("crash.txt"));
        // Tailed, not whole: only the last TAIL_LINES survive.
        assert_eq!(log.tail.lines().count(), TAIL_LINES);
        assert!(log.tail.contains("line 99"));
        assert!(!log.tail.contains("line 10\n"));
    }
}
