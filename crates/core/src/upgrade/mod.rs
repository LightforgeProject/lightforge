//! Getting a newer Lightforge onto this machine, once [`crate::update`] has
//! found one.
//!
//! Who may replace the binary depends on who put it there, and that is read
//! from where it lives: the deb, the rpm, the `-bin` package and the tarball
//! all ship the same binary, so nothing could be decided at build time.
//!
//! - The installer's copy in `~/.local/bin` is the user's own, and Lightforge
//!   replaces it itself ([`tarball`]).
//! - A package manager's copy is replaced by that package manager, run in a
//!   terminal where it asks for the password itself ([`terminal`]).
//!   Lightforge only fetches and checks what it will install.
//! - Everything else gets words: COPR and Fedora Atomic update with the
//!   system, and a build from source is rebuilt the way it was built.
//!
//! Lightforge never runs anything as root.

use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::net::{Http, NetError};

pub mod tarball;
pub mod terminal;

/// Where every release's files are published.
const DOWNLOADS: &str = "https://github.com/LightforgeProject/lightforge/releases/download";

/// The Arch packages, as the release left them on `main`: the tag still has
/// `sha256sums=('SKIP')`, the checksums are committed after it.
const PKGBUILDS: &str =
    "https://raw.githubusercontent.com/LightforgeProject/lightforge/main/packaging/aur";

/// The one-line install for everyone the routes below cannot help.
pub const INSTALLER_COMMAND: &str = "curl -fsSL https://raw.githubusercontent.com/LightforgeProject/lightforge/main/packaging/installer/install.sh | bash";

/// The COPR repository's file, as `dnf copr enable` writes it.
const COPR_REPO: &str = "etc/yum.repos.d/_copr:copr.fedorainfracloud.org:simplykenny:lightforge.repo";

/// How this copy of Lightforge was installed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Origin {
    /// `install.sh`, the double-click installer, or the tarball by hand into
    /// the same place: `~/.local/bin`.
    Installer { bin_dir: PathBuf },
    /// One of the PKGBUILDs, `lightforge` or `lightforge-bin`.
    Pacman { package: String },
    /// The `.deb` from the release page.
    Deb,
    /// The COPR repository; `atomic` on Bazzite and other Fedora Atomic
    /// systems, where it is layered onto the image.
    Copr { atomic: bool },
    /// The `.rpm` from the release page.
    Rpm { atomic: bool },
    /// Built from a checkout, and run from its `target/`.
    Checkout { repo: PathBuf },
    /// `cargo install`.
    Cargo,
    /// None of these.
    Unknown,
}

impl Origin {
    /// This process's own.
    ///
    /// Read once, at start: after a package manager replaces the binary, the
    /// running one's path reads `… (deleted)`.
    pub fn detect() -> Self {
        let Ok(exe) = std::env::current_exe() else { return Self::Unknown };
        let home = directories::BaseDirs::new().map(|d| d.home_dir().to_path_buf()).unwrap_or_default();
        let cargo_home = std::env::var_os("CARGO_HOME").map(PathBuf::from);
        Self::detect_at(&exe, &home, cargo_home.as_deref(), Path::new("/"))
    }

    /// The same question, with every place it looks passed in. `root` is
    /// where the system's own directories are read from.
    pub fn detect_at(exe: &Path, home: &Path, cargo_home: Option<&Path>, root: &Path) -> Self {
        let bin_dir = exe.parent().unwrap_or(Path::new("/"));
        if bin_dir == home.join(".local/bin") {
            return Self::Installer { bin_dir: bin_dir.to_path_buf() };
        }
        let cargo_bin = cargo_home.map(|c| c.join("bin")).unwrap_or_else(|| home.join(".cargo/bin"));
        if bin_dir == cargo_bin {
            return Self::Cargo;
        }
        // `<repo>/target/<profile>/`, or deeper for a custom target dir.
        if let Some(repo) = exe
            .ancestors()
            .skip(2)
            .take(4)
            .find(|dir| dir.join("Cargo.toml").is_file() && dir.join("crates/launcher-gui").is_dir())
        {
            return Self::Checkout { repo: repo.to_path_buf() };
        }
        if !bin_dir.starts_with("/usr") {
            return Self::Unknown;
        }

        if let Some(package) = pacman_package(&root.join("var/lib/pacman/local")) {
            return Self::Pacman { package };
        }
        if root.join("var/lib/dpkg/info/lightforge.list").is_file() {
            return Self::Deb;
        }
        let atomic = root.join("run/ostree-booted").exists();
        if root.join(COPR_REPO).is_file() {
            return Self::Copr { atomic };
        }
        if root.join("usr/lib/sysimage/rpm").is_dir() || root.join("var/lib/rpm").is_dir() {
            return Self::Rpm { atomic };
        }
        Self::Unknown
    }

    /// What the update dialog offers for `version`.
    pub fn route(&self, version: &str) -> Route {
        let say = |commands: &[&str], note: &str| Route::Instructions {
            commands: commands.iter().map(|c| (*c).to_owned()).collect(),
            note: note.to_owned(),
        };
        match self {
            Self::Installer { .. } => Route::InPlace,
            Self::Pacman { package } => Route::Terminal(Package::Pacman { package: package.clone() }),
            Self::Deb => Route::Terminal(Package::Deb),
            Self::Rpm { atomic: false } => Route::Terminal(Package::Rpm),
            Self::Copr { atomic: false } => say(
                &["sudo dnf upgrade --refresh lightforge"],
                "Lightforge comes from its COPR repository, so it also arrives with your \
                 normal system updates. COPR builds it a few minutes after a release.",
            ),
            Self::Copr { atomic: true } => say(
                &["rpm-ostree upgrade", "systemctl reboot"],
                "On Bazzite and other Fedora Atomic systems Lightforge updates with the \
                 system image: run these, or let the system update itself, and the new \
                 version is there after the restart. COPR builds it a few minutes after \
                 a release.",
            ),
            Self::Rpm { atomic: true } => say(
                &["sudo dnf5 copr enable simplykenny/lightforge", "rpm-ostree upgrade"],
                "This system boots from an image, and a downloaded .rpm does not update \
                 itself there. The COPR repository does: enable it once, and Lightforge \
                 updates with the system from then on.",
            ),
            Self::Checkout { repo } => Route::Instructions {
                commands: vec![format!(
                    "cd '{}' && git pull && cargo build --release",
                    repo.display()
                )],
                note: "This Lightforge was built from source, so the new version is built \
                       the same way."
                    .to_owned(),
            },
            Self::Cargo => Route::Instructions {
                commands: vec![format!(
                    "cargo install --locked --git https://github.com/LightforgeProject/lightforge \
                     --tag v{version} lightforge-gui"
                )],
                note: "This Lightforge was installed with cargo.".to_owned(),
            },
            Self::Unknown => say(
                &[INSTALLER_COMMAND],
                "Lightforge cannot tell how it was installed here. The installer puts it \
                 in your home folder, and from then on an update is one click.",
            ),
        }
    }
}

/// What the update dialog can do about a new version.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// Lightforge replaces its own files.
    InPlace,
    /// A package manager does, in a terminal.
    Terminal(Package),
    /// Nothing to press: commands to copy, and why.
    Instructions { commands: Vec<String>, note: String },
}

/// A package manager that owns the binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Package {
    Pacman { package: String },
    Deb,
    Rpm,
}

/// `lightforge` or `lightforge-bin`, from pacman's database folders, which
/// are named `<package>-<version>-<release>`.
fn pacman_package(local: &Path) -> Option<String> {
    let entries = std::fs::read_dir(local).ok()?;
    entries.filter_map(Result::ok).find_map(|entry| {
        let name = entry.file_name().to_string_lossy().into_owned();
        let rest = name.strip_prefix("lightforge-")?;
        match rest.strip_prefix("bin-") {
            Some(version) if version.starts_with(|c: char| c.is_ascii_digit()) => Some("lightforge-bin".to_owned()),
            _ if rest.starts_with(|c: char| c.is_ascii_digit()) => Some("lightforge".to_owned()),
            _ => None,
        }
    })
}

/// Download the release asset whose name ends in `suffix` into `dir`, and
/// check it against the release's `SHA256SUMS`, the file `install.sh`
/// checks against too.
pub async fn fetch_asset(
    http: &Http,
    version: &str,
    suffix: &str,
    dir: &Path,
    on_progress: impl FnMut(u64, Option<u64>),
) -> Result<PathBuf, UpgradeError> {
    let base = format!("{DOWNLOADS}/v{version}");
    let sums = http.get_text(&format!("{base}/SHA256SUMS")).await?;
    let (expected, name) = listed(&sums, suffix).ok_or_else(|| UpgradeError::NotPublished {
        version: version.to_owned(),
        what: suffix.to_owned(),
    })?;
    // A name is a single file name, or it is not one of ours.
    if name.contains('/') || name.starts_with('.') {
        return Err(UpgradeError::NotPublished { version: version.to_owned(), what: suffix.to_owned() });
    }

    let path = dir.join(name);
    http.download(&format!("{base}/{name}"), &path, on_progress).await?;
    let actual = sha256_of(&path)?;
    if !actual.eq_ignore_ascii_case(expected) {
        let _ = std::fs::remove_file(&path);
        return Err(UpgradeError::ChecksumMismatch { file: name.to_owned() });
    }
    Ok(path)
}

/// The digest and name of the one `SHA256SUMS` line naming a file that ends
/// in `suffix`.
fn listed<'a>(sums: &'a str, suffix: &str) -> Option<(&'a str, &'a str)> {
    sums.lines().find_map(|line| {
        let (digest, name) = line.split_once(char::is_whitespace)?;
        let name = name.trim_start().trim_start_matches('*');
        name.ends_with(suffix).then_some((digest, name))
    })
}

fn sha256_of(path: &Path) -> Result<String, UpgradeError> {
    let mut file = std::fs::File::open(path).map_err(|source| UpgradeError::io(path, source))?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).map_err(|source| UpgradeError::io(path, source))?;
    Ok(hasher.finalize().iter().map(|b| format!("{b:02x}")).collect())
}

/// The PKGBUILD for `package`, as it stands on `main`, once it is the one for
/// `version`.
///
/// `main` gets the new PKGBUILD a little after the release is published, and
/// its checksums a commit after that. Until both are there, building it would
/// build the old version again or one nothing checks, so it waits instead.
pub async fn fetch_pkgbuild(http: &Http, package: &str, version: &str) -> Result<String, UpgradeError> {
    let text = http.get_text(&format!("{PKGBUILDS}/{package}/PKGBUILD")).await?;
    let pkgver = text.lines().find_map(|line| line.strip_prefix("pkgver=")).map(str::trim);
    // `1.0.0-beta.7` is `1.0.0beta.7` to pacman, which allows no hyphen.
    let current = pkgver == Some(version.replace('-', "").as_str());
    let checked = !text.lines().any(|line| line.starts_with("sha256sums=") && line.contains("SKIP"));
    match current && checked {
        true => Ok(text),
        false => Err(UpgradeError::NotPublished {
            version: version.to_owned(),
            what: format!("the {package} PKGBUILD"),
        }),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum UpgradeError {
    #[error(transparent)]
    Net(#[from] NetError),

    #[error("{what} for Lightforge {version} is not published yet; try again in a while")]
    NotPublished { version: String, what: String },

    #[error("{file} does not match its published checksum, so nothing was installed")]
    ChecksumMismatch { file: String },

    #[error("{0} does not look like a Lightforge release")]
    NotARelease(PathBuf),

    #[error("no terminal to run the update in was found")]
    NoTerminal,

    #[error("i/o error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

impl UpgradeError {
    fn io(path: &Path, source: std::io::Error) -> Self {
        Self::Io { path: path.to_path_buf(), source }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway root with the given files and folders in it.
    fn root(name: &str, files: &[&str], dirs: &[&str]) -> PathBuf {
        let root = std::env::temp_dir().join(format!("lightforge-origin-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for dir in dirs {
            std::fs::create_dir_all(root.join(dir)).unwrap();
        }
        for file in files {
            let path = root.join(file);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, "").unwrap();
        }
        root
    }

    fn detect(exe: &str, root: &Path) -> Origin {
        Origin::detect_at(Path::new(exe), Path::new("/home/u"), None, root)
    }

    #[test]
    fn the_installers_copy_is_the_one_in_the_home_folder() {
        let empty = root("installer", &[], &[]);
        assert_eq!(
            detect("/home/u/.local/bin/lightforge-gui", &empty),
            Origin::Installer { bin_dir: "/home/u/.local/bin".into() }
        );
        assert_eq!(detect("/home/u/.cargo/bin/lightforge-gui", &empty), Origin::Cargo);
        assert_eq!(detect("/opt/lightforge/lightforge-gui", &empty), Origin::Unknown);
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[test]
    fn a_package_is_named_by_its_package_managers_own_records() {
        let arch = root("arch", &[], &["var/lib/pacman/local/lightforge-bin-1.0.0beta.6-1", "var/lib/pacman/local/lightforged-2-1"]);
        assert_eq!(
            detect("/usr/bin/lightforge-gui", &arch),
            Origin::Pacman { package: "lightforge-bin".into() }
        );
        let source = root("arch-src", &[], &["var/lib/pacman/local/lightforge-1.0.0beta.6-1"]);
        assert_eq!(
            detect("/usr/bin/lightforge-gui", &source),
            Origin::Pacman { package: "lightforge".into() }
        );

        let debian = root("deb", &["var/lib/dpkg/info/lightforge.list"], &[]);
        assert_eq!(detect("/usr/bin/lightforge-gui", &debian), Origin::Deb);

        let bazzite = root("copr", &[COPR_REPO, "run/ostree-booted"], &["usr/lib/sysimage/rpm"]);
        assert_eq!(detect("/usr/bin/lightforge-gui", &bazzite), Origin::Copr { atomic: true });

        let fedora = root("rpm", &[], &["usr/lib/sysimage/rpm"]);
        assert_eq!(detect("/usr/bin/lightforge-gui", &fedora), Origin::Rpm { atomic: false });

        for dir in [arch, source, debian, bazzite, fedora] {
            let _ = std::fs::remove_dir_all(dir);
        }
    }

    #[test]
    fn a_build_from_a_checkout_is_rebuilt_there() {
        let repo = root("checkout", &["Cargo.toml"], &["crates/launcher-gui", "target/release"]);
        let exe = repo.join("target/release/lightforge-gui");
        assert_eq!(
            Origin::detect_at(&exe, Path::new("/home/u"), None, Path::new("/nonexistent")),
            Origin::Checkout { repo: repo.clone() }
        );
        let Route::Instructions { commands, .. } = (Origin::Checkout { repo: repo.clone() }).route("1.0.0") else {
            panic!("a checkout gets instructions");
        };
        assert!(commands[0].contains("git pull") && commands[0].contains(&*repo.to_string_lossy()));
        let _ = std::fs::remove_dir_all(&repo);
    }

    #[test]
    fn only_the_home_folder_and_the_three_package_managers_get_a_button() {
        let pressable = |origin: Origin| !matches!(origin.route("1.0.0"), Route::Instructions { .. });
        assert!(pressable(Origin::Installer { bin_dir: "/home/u/.local/bin".into() }));
        assert!(pressable(Origin::Pacman { package: "lightforge-bin".into() }));
        assert!(pressable(Origin::Deb));
        assert!(pressable(Origin::Rpm { atomic: false }));
        for words in [
            Origin::Copr { atomic: false },
            Origin::Copr { atomic: true },
            Origin::Rpm { atomic: true },
            Origin::Cargo,
            Origin::Unknown,
        ] {
            assert!(!pressable(words.clone()), "{words:?}");
        }
    }

    #[test]
    fn an_asset_is_found_by_its_ending_in_the_checksum_list() {
        let sums = "\
aaaa  lightforge-1.0.0-beta.7-x86_64-linux.tar.gz
bbbb  lightforge_1.0.0.beta.7-1_amd64.deb
cccc  lightforge-1.0.0.beta.7-1.x86_64.rpm
";
        assert_eq!(listed(sums, "_amd64.deb"), Some(("bbbb", "lightforge_1.0.0.beta.7-1_amd64.deb")));
        assert_eq!(listed(sums, "-x86_64-linux.tar.gz").map(|(d, _)| d), Some("aaaa"));
        assert_eq!(listed(sums, ".pkg.tar.zst"), None);
    }
}
