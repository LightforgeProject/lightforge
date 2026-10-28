use std::collections::HashSet;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};

use sysinfo::{ProcessRefreshKind, ProcessesToUpdate, System, UpdateKind};

use crate::paths::Layout;

/// Environment variables that identify which prefix a process belongs to.
const PREFIX_VARS: [&str; 2] = ["WINEPREFIX", "STEAM_COMPAT_DATA_PATH"];

/// What a process found inside the prefix is. [`ProcKind::Agent`] causes the
/// repair scan; [`ProcKind::WineServer`] dies last, taking the rest with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ProcKind {
    /// `Battle.net.exe` or `Battle.net Launcher.exe`.
    BattleNet,
    /// `Agent.exe`: the update/repair service.
    Agent,
    /// A CEF or crash-handler child of the client.
    Helper,
    /// The prefix's own `wineserver`.
    WineServer,
}

impl ProcKind {
    /// Classify a process from `argv[0]`, the Windows path Wine was handed.
    /// Not `comm`: that is the thread name, and the CEF client shows up as
    /// `CrBrowserMain` or `CrRendererMain`. `None` for anything not ours to touch.
    pub(super) fn classify(argv: &[std::ffi::OsString], name: &str) -> Option<Self> {
        // Case-insensitive: Wine's spelling varies across builds.
        let exe = executable_name(argv, name).to_ascii_lowercase();
        let kind = match exe.trim_end_matches(".exe") {
            "battle.net" | "battle.net launcher" => Self::BattleNet,
            "agent" => Self::Agent,
            "blizzarderror" | "bnetcrashhandler" => Self::Helper,
            "wineserver" => Self::WineServer,
            _ => return None,
        };

        // CEF children re-exec the same binary with `--type=`; they go first.
        if kind == Self::BattleNet && argv.iter().skip(1).any(is_cef_child_arg) {
            return Some(Self::Helper);
        }
        Some(kind)
    }

    /// Termination order: children before the server that owns them.
    pub(super) fn kill_order(self) -> u8 {
        match self {
            Self::Helper => 0,
            Self::Agent => 1,
            Self::BattleNet => 2,
            Self::WineServer => 3,
        }
    }
}

/// The executable's file name from `argv[0]`: a Windows path under Wine, a
/// POSIX one for `wineserver`.
pub(super) fn executable_name(argv: &[std::ffi::OsString], fallback: &str) -> String {
    let Some(argv0) = argv.first().map(|a| a.to_string_lossy()) else {
        return fallback.to_owned();
    };
    if argv0.is_empty() {
        return fallback.to_owned();
    }
    argv0
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(&argv0)
        .to_owned()
}

/// `C:\...` or anything ending `.exe`: what Wine puts in `argv[0]`.
pub(super) fn is_windows_exe(argv0: &std::ffi::OsString) -> bool {
    let argv0 = argv0.to_string_lossy();
    let bytes = argv0.as_bytes();
    let drive = bytes.len() > 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':';
    drive || argv0.to_ascii_lowercase().ends_with(".exe")
}

pub(super) fn is_cef_child_arg(arg: &std::ffi::OsString) -> bool {
    arg.to_string_lossy().starts_with("--type=")
}

/// A process proven to belong to Lightforge's prefix.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrefixProcess {
    pub pid: u32,
    pub name: String,
    pub kind: ProcKind,
    pub exe: Option<PathBuf>,
}

/// Every process belonging to the prefix, in termination order.
pub fn scan(layout: &Layout) -> Vec<PrefixProcess> {
    let mut system = System::new();
    refresh(&mut system);
    collect(&system, layout)
}

/// Refresh only what ownership and classification need; the default also
/// walks disks and networks, a measurable stall.
pub(super) fn refresh(system: &mut System) {
    system.refresh_processes_specifics(
        ProcessesToUpdate::All,
        true,
        ProcessRefreshKind::nothing()
            .with_cmd(UpdateKind::Always)
            .with_exe(UpdateKind::Always)
            .with_environ(UpdateKind::Always),
    );
}

pub(super) fn collect(system: &System, layout: &Layout) -> Vec<PrefixProcess> {
    let prefixes = owned_prefixes(layout);

    let mut found: Vec<PrefixProcess> = system
        .processes()
        .iter()
        .filter_map(|(pid, proc)| {
            // Threads match every test below; only thread-group leaders count.
            if proc.thread_kind().is_some() {
                return None;
            }
            let fallback = proc.name().to_string_lossy();
            let kind = ProcKind::classify(proc.cmd(), &fallback)?;
            if !belongs_to(proc.environ(), &prefixes) {
                return None;
            }
            Some(PrefixProcess {
                pid: pid.as_u32(),
                name: executable_name(proc.cmd(), &fallback),
                kind,
                exe: proc.exe().map(Path::to_path_buf),
            })
        })
        .collect();

    found.sort_by_key(|p| (p.kind.kill_order(), p.pid));
    found
}

/// Every spelling of "our prefix" a process's environment might carry: the
/// compat-data directory (Proton's) and `pfx` (Wine's), raw and canonical.
pub(super) fn owned_prefixes(layout: &Layout) -> HashSet<PathBuf> {
    let mut set = HashSet::new();
    for path in [layout.compat_data(), layout.prefix()] {
        if let Ok(real) = path.canonicalize() {
            set.insert(real);
        }
        set.insert(path);
    }
    set
}

/// Whether a process's environment names one of `prefixes`. Unreadable means
/// not ours: never signalling what cannot be attributed is this module's safety rule.
pub(super) fn belongs_to(environ: &[std::ffi::OsString], prefixes: &HashSet<PathBuf>) -> bool {
    environ.iter().any(|entry| {
        let Some(value) = PREFIX_VARS.iter().find_map(|var| strip_var(entry, var)) else {
            return false;
        };
        let path = PathBuf::from(value);
        prefixes.contains(&path)
            || matches!(path.canonicalize(), Ok(real) if prefixes.contains(&real))
    })
}

/// `KEY=value` -> `value`, for one key.
fn strip_var<'a>(entry: &'a OsStr, var: &str) -> Option<&'a OsStr> {
    use std::os::unix::ffi::OsStrExt;

    let bytes = entry.as_bytes();
    let prefix = var.as_bytes();
    let rest = bytes.strip_prefix(prefix)?;
    let rest = rest.strip_prefix(b"=")?;
    Some(OsStr::from_bytes(rest))
}
