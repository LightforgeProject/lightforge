use super::{ProtonRelease, ProtonSource};

/// A Valve Proton that Steam can install as a tool.
///
/// Valve publishes no tarballs: these are Steam apps, so "download" means
/// asking Steam to install one (`steam://install/<id>`). The ids are from
/// SteamDB; `install_dir` is the folder Steam puts it in, which is how an
/// installed one is recognised.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SteamTool {
    pub name: &'static str,
    pub app_id: u32,
    pub install_dir: &'static str,
}

impl SteamTool {
    pub fn install_url(&self) -> String {
        format!("steam://install/{}", self.app_id)
    }

    /// Whether `build` is this tool, by the folder it was found in.
    pub fn is_installed_as(&self, build: &ProtonRelease) -> bool {
        build.source == ProtonSource::SteamApp
            && build.path.file_name().is_some_and(|n| n.eq_ignore_ascii_case(self.install_dir))
    }
}

/// Every Valve Proton still on Steam, newest first.
pub const STEAM_TOOLS: &[SteamTool] = &[
    SteamTool { name: "Proton 11.0", app_id: 4628710, install_dir: "Proton 11.0" },
    SteamTool { name: "Proton 10.0", app_id: 3658110, install_dir: "Proton 10.0" },
    SteamTool { name: "Proton 9.0", app_id: 2805730, install_dir: "Proton 9.0" },
    SteamTool { name: "Proton 8.0", app_id: 2348590, install_dir: "Proton 8.0" },
    SteamTool { name: "Proton 7.0", app_id: 1887720, install_dir: "Proton 7.0" },
    SteamTool { name: "Proton 6.3", app_id: 1580130, install_dir: "Proton 6.3" },
    SteamTool { name: "Proton 5.13", app_id: 1420170, install_dir: "Proton 5.13" },
    SteamTool { name: "Proton 5.0", app_id: 1245040, install_dir: "Proton 5.0" },
    SteamTool { name: "Proton 4.11", app_id: 1113280, install_dir: "Proton 4.11" },
    SteamTool { name: "Proton 4.2", app_id: 1054830, install_dir: "Proton 4.2" },
    SteamTool { name: "Proton 3.16", app_id: 961940, install_dir: "Proton 3.16" },
    SteamTool { name: "Proton 3.7", app_id: 858280, install_dir: "Proton 3.7" },
    SteamTool { name: "Proton Experimental", app_id: 1493710, install_dir: "Proton - Experimental" },
    SteamTool { name: "Proton Hotfix", app_id: 2180100, install_dir: "Proton Hotfix" },
];
