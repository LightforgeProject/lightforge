//! Storage libraries: the partitions Lightforge is allowed to put games on.
//!
//! A library is one directory on one filesystem, holding a `games/` folder the
//! same shape as the one under the Blizzard root. The first library is that
//! root, the user's home, and every other is a partition they added, where
//! Lightforge creates `Lightforge/games/` and nothing else.
//!
//! # Why a marker file
//!
//! An unmounted drive does not disappear; its mount point stays behind as an
//! empty directory on whatever filesystem holds it. A library identified by
//! path alone is therefore indistinguishable, at exactly the wrong moment,
//! from a library that is simply empty, and the cost of that mistake is a
//! 120 GB download onto the system disk, or a client that reports a perfectly
//! good install as needing repair.
//!
//! So the truth about a library lives *on the volume*: [`MARKER`], holding the
//! id the config recorded. A library is usable only when that file reads back
//! with the matching id, which an unmounted drive fails immediately and
//! without any guessing. The filesystem UUID and the path relative to the
//! mount point are recorded too, but only as hints. They are what lets a
//! drive that came back at a different mount point be found again
//! ([`relocate`]) rather than declared lost.

use std::collections::{BTreeMap, HashMap};
use std::ffi::CString;
use std::os::unix::ffi::OsStrExt;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The file that proves a library is the one the config means, written at the
/// root of its `games/` directory.
///
/// Inside `games/` rather than beside it deliberately: the hazard being
/// guarded against is "the games directory exists and is empty", so the proof
/// has to live where that emptiness would be measured.
pub const MARKER: &str = ".lightforge-library";

/// Folder created on an added partition, under which `games/` lives. The
/// first library is the Blizzard root itself and has no such folder.
pub const DRIVE_FOLDER: &str = "Lightforge";

/// The `games/` directory inside a library root.
const GAMES_DIR: &str = "games";

/// Id of the default library: the one derived from the Blizzard root, where
/// Battle.net and the prefix live.
///
/// Fixed, because it is re-seeded from the config on every start and a random
/// id would not match its own marker the next time. Still `"main"` although
/// the concept is now called "default": that string is already in the marker
/// of every library set up before the rename.
pub const DEFAULT_ID: &str = "main";

/// Drive letter the first library's WoW install has always had. Pinned rather
/// than allocated: `product.db` and `.build.info` record the path Battle.net
/// matches an install by, and re-lettering an install it has already adopted
/// is how a 150 GB library stops being recognised.
pub const DEFAULT_WOW_LETTER: &str = "d:";
/// Drive letter for the first library's `games/` directory, the one that
/// goes into Battle.net's `DefaultInstallPath`.
pub const DEFAULT_GAMES_LETTER: &str = "e:";

/// Letters a library pair may be allocated from, in order.
///
/// `c:` is the prefix's own `drive_c` and `z:` is Wine's catch-all mapping of
/// `/`; `a:` and `b:` are left alone because enough Windows software still
/// treats them as removable media. That leaves 23 letters, so 11 libraries
/// beyond the first.
const LETTER_POOL: &[&str] = &[
    "d:", "e:", "f:", "g:", "h:", "i:", "j:", "k:", "l:", "m:", "n:", "o:", "p:", "q:", "r:",
    "s:", "t:", "u:", "v:", "w:", "x:", "y:",
];

/// One partition Lightforge may install games to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Library {
    /// Opaque id, also written into this library's [`MARKER`]. The only thing
    /// that actually identifies the library.
    pub id: String,
    /// What the user sees: the partition label, else the device name.
    pub label: String,
    /// The `games/` directory itself, as last resolved. A cache of where the
    /// library was, not proof that it is still there.
    pub games: PathBuf,
    /// Filesystem UUID of the partition, when one could be read. A hint for
    /// [`relocate`], not an identity: btrfs gives every subvolume of one
    /// filesystem the same UUID, so `/` and `/home` are indistinguishable by
    /// it.
    pub uuid: Option<String>,
    /// `games` relative to the mount point it was under, so the library can be
    /// rebuilt from a mount point found by UUID.
    pub relative: PathBuf,
    /// The standard place to install: where a game goes unless another drive
    /// is picked, and the folder Battle.net proposes for its own titles.
    ///
    /// Only that. It is not "the drive Lightforge lives on", the Wine prefix
    /// and the Battle.net client stay at the Blizzard root chosen during
    /// setup, whichever drive is the default and however many are added.
    #[serde(rename = "default", alias = "main")]
    pub is_default: bool,
    /// `dosdevices` entry mapping a drive letter over `games`, giving
    /// Battle.net a Windows directory to install other titles into.
    pub games_letter: String,
    /// `dosdevices` entry mapping a drive letter over this library's WoW
    /// install. Separate from `games_letter` because the in-game shop's
    /// browser only starts when the install is a drive *root*: see
    /// [`crate::paths::Layout::wow_drive`].
    pub wow_letter: String,
    /// Whether [`Library::claim`] has ever succeeded here.
    ///
    /// This is what separates "the drive is not mounted" from "this library
    /// has not been set up yet", which look identical on disk: both are a
    /// path with no marker under it. Without it, the first setup of a brand
    /// new library and a 150 GB install whose drive is asleep are the same
    /// state, and whichever way Lightforge guessed would be wrong half the
    /// time.
    #[serde(default)]
    pub claimed: bool,
}

/// What a library is doing right now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    /// Configured but never set up: nothing has been written to it yet.
    New,
    /// Here, ours, and safe to write to.
    Ready,
    /// Set up once and not here now, almost always an unmounted drive.
    /// Nothing may be written, created or relinked under it.
    Missing,
    /// A marker that belongs to a different library. Someone copied a
    /// `Lightforge/` folder, or two config entries point at one volume.
    Foreign,
}

impl State {
    /// Whether a write to this library is allowed.
    pub fn is_ready(self) -> bool {
        matches!(self, Self::Ready)
    }

    /// Why this library cannot be written to, if it cannot.
    ///
    /// [`State::New`] is deliberately not a reason: a library that has never
    /// been written to is the normal state of a drive before the first
    /// install, and saying so in the colour of an error would make a first run
    /// look broken.
    pub fn reason(self, label: &str) -> Option<String> {
        match self {
            Self::Ready | Self::New => None,
            Self::Missing => Some(format!("{label} is not mounted")),
            Self::Foreign => Some(format!("{label} holds another Lightforge library")),
        }
    }

    /// Whether the drive is here at all, whether or not we have written to it.
    ///
    /// The question the *measurements* ask, where [`State::is_ready`] is the
    /// question a *write* asks: a drive with nothing of ours on it still has
    /// free space worth reporting.
    pub fn is_present(self) -> bool {
        matches!(self, Self::Ready | Self::New)
    }
}

impl Library {
    /// A library on a partition the user just added: `<mount>/Lightforge/games`.
    pub fn on_partition(mount: &Path, label: impl Into<String>, uuid: Option<String>) -> Self {
        let games = mount.join(DRIVE_FOLDER).join(GAMES_DIR);
        Self {
            id: new_id(),
            label: label.into(),
            relative: games.strip_prefix(mount).unwrap_or(&games).to_path_buf(),
            games,
            uuid,
            is_default: false,
            games_letter: String::new(),
            wow_letter: String::new(),
            claimed: false,
        }
    }

    /// A library made out of a games directory that already exists, the
    /// parent of a WoW install found somewhere else on this machine.
    ///
    /// The difference from [`Library::on_partition`] is that nothing is placed:
    /// the directory is taken as it is, because the games in it are already
    /// there and the point of adoption is that no byte moves. See
    /// [`crate::discover::adopt`].
    pub fn adopted(games: PathBuf, label: impl Into<String>) -> Self {
        Self {
            id: new_id(),
            label: label.into(),
            relative: games.clone(),
            games,
            uuid: None,
            is_default: false,
            games_letter: String::new(),
            wow_letter: String::new(),
            claimed: false,
        }
    }

    /// The first library: whatever games directory the install already used,
    /// with the drive letters it already had.
    ///
    /// Used when migrating a config written before libraries existed. Keeping
    /// the path and the letters means the prefix's links, the recorded Windows
    /// paths and an install Battle.net has already adopted all stay valid.
    pub fn main_at(games: PathBuf, label: impl Into<String>) -> Self {
        Self {
            id: DEFAULT_ID.to_owned(),
            label: label.into(),
            relative: games.clone(),
            games,
            uuid: None,
            is_default: true,
            games_letter: DEFAULT_GAMES_LETTER.to_owned(),
            wow_letter: DEFAULT_WOW_LETTER.to_owned(),
            claimed: false,
        }
    }

    /// Fill in the hints [`relocate`] needs by reading the mount table.
    ///
    /// Kept out of the constructors on purpose: a [`Library`] is built every
    /// time a [`crate::paths::Layout`] is, and reading `/proc/self/mounts` on
    /// each of those would be I/O in what callers treat as a pure value. This
    /// runs once, where a library is genuinely being added or migrated, and
    /// the result is persisted.
    pub fn probe(&mut self) {
        let mount = mount_point_of(&self.games);
        self.relative = self.games.strip_prefix(&mount).unwrap_or(&self.games).to_path_buf();
        self.uuid = uuid_of(&self.games);
    }

    /// The `games/` directory games are installed under.
    pub fn games(&self) -> &Path {
        &self.games
    }

    /// The library root, the `Lightforge/` folder on an added partition, or
    /// the Blizzard root for the first one.
    pub fn root(&self) -> &Path {
        self.games.parent().unwrap_or(&self.games)
    }

    /// Where this library's proof of identity is.
    pub fn marker(&self) -> PathBuf {
        self.games.join(MARKER)
    }

    /// What this library is doing right now, the question every write to it
    /// has to ask first.
    pub fn state(&self) -> State {
        match (read_marker(&self.marker()), self.claimed) {
            (Some(id), _) if id == self.id => State::Ready,
            (Some(_), _) => State::Foreign,
            (None, true) => State::Missing,
            (None, false) => State::New,
        }
    }

    /// Here, ours, and safe to write to.
    pub fn online(&self) -> bool {
        self.state().is_ready()
    }

    /// Create the library's directories and write its marker.
    ///
    /// Idempotent, and only ever called where the user has asked for this
    /// library to be set up, adding a drive, or provisioning the prefix.
    ///
    /// A library that has never been claimed adopts a marker that is already
    /// there, so a `Lightforge/` folder left by an earlier install is picked
    /// back up rather than duplicated. One that *has* been claimed never
    /// adopts: a different id under our path means we are looking at the
    /// wrong volume, and quietly rebinding to it is how a config entry ends
    /// up pointing at someone else's games.
    pub fn claim(&mut self) -> Result<(), LibraryError> {
        let io = |path: &Path, source| LibraryError::Io { path: path.to_path_buf(), source };
        match self.state() {
            State::Ready => {
                self.claimed = true;
                return Ok(());
            }
            State::Foreign if self.claimed => {
                return Err(LibraryError::Foreign { label: self.label.clone() })
            }
            State::Missing => return Err(LibraryError::Offline { label: self.label.clone() }),
            State::Foreign | State::New => {}
        }
        std::fs::create_dir_all(&self.games).map_err(|e| io(&self.games, e))?;
        let marker = self.marker();
        match read_marker(&marker) {
            Some(existing) => self.id = existing,
            None => {
                std::fs::write(&marker, marker_text(&self.id)).map_err(|e| io(&marker, e))?
            }
        }
        self.claimed = true;
        Ok(())
    }

    /// Free and total space on the filesystem this library is on, or `None`
    /// when it could not be read.
    pub fn disk(&self) -> Option<crate::storage::Disk> {
        self.state()
            .is_present()
            .then(|| crate::storage::disk(&self.games).ok())
            .flatten()
    }

    /// The Windows path this library's `games/` directory is reachable at,
    /// what Battle.net is told to install into.
    ///
    /// Spelled with a backslash because this one goes into
    /// `Battle.net.config`, where Blizzard writes its own default as
    /// `C:\Program Files (x86)`; [`Library::windows_wow_path`] keeps forward
    /// slashes because that is the spelling already recorded in every
    /// `.product.db` Lightforge has written.
    pub fn windows_games_path(&self) -> String {
        format!("{}\\", drive(&self.games_letter))
    }

    /// The Windows path this library's WoW install is recorded under in
    /// `.product.db`, which is how Battle.net matches it.
    ///
    /// The first library keeps [`crate::paths::WINDOWS_WOW_PATH`], the
    /// `C:\Program Files (x86)` symlink, because that is the string an
    /// already-adopted install has on disk. Every other library is presented
    /// under its own `games/` drive, which is a real Windows directory with a
    /// parent, as Battle.net expects.
    pub fn windows_wow_path(&self) -> String {
        match self.is_default {
            true => crate::paths::WINDOWS_WOW_PATH.to_owned(),
            false => format!("{}/{}", drive(&self.games_letter), crate::paths::WOW_DIR),
        }
    }
}

/// `"d:"` → `"D:"`, the spelling a Windows path wants.
fn drive(letter: &str) -> String {
    letter.trim_end_matches(':').to_uppercase() + ":"
}

/// Everything the config holds about storage, resolved against the machine as
/// it is right now.
///
/// Cheap to clone and free of I/O once built, so [`crate::paths::Layout`] can
/// carry one and answer path questions without touching the filesystem.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Libraries {
    entries: Vec<Library>,
    /// Which library each product was installed into.
    assigned: HashMap<crate::product::ProductCode, String>,
}

impl Libraries {
    pub fn new(entries: Vec<Library>) -> Self {
        Self { entries, assigned: HashMap::new() }
    }

    /// Record that a product lives in a given library.
    pub fn assign(&mut self, code: crate::product::ProductCode, id: impl Into<String>) {
        self.assigned.insert(code, id.into());
    }

    pub fn all(&self) -> &[Library] {
        &self.entries
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn get(&self, id: &str) -> Option<&Library> {
        self.entries.iter().find(|l| l.id == id)
    }

    /// The standard install target: the one flagged as default, else the
    /// first one there is.
    pub fn default(&self) -> Option<&Library> {
        self.entries.iter().find(|l| l.is_default).or_else(|| self.entries.first())
    }

    /// Where a product is installed, or would be: its own library when it has
    /// one that still exists, else [`Libraries::main`].
    ///
    /// Falling back rather than failing keeps a product whose library was
    /// *removed from the config* installable again, which is the only way out
    /// of that state. It does **not** cover a library that is merely offline:
    /// that one is still in the config, so this returns it and the caller sees
    /// [`Library::online`] come back false.
    pub fn of(&self, code: &crate::product::ProductCode) -> Option<&Library> {
        self.assigned
            .get(code)
            .and_then(|id| self.get(id))
            .or_else(|| self.default())
    }
}

/// A mounted filesystem, as `/proc/self/mounts` describes it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mount {
    pub device: PathBuf,
    pub point: PathBuf,
    pub fstype: String,
    pub read_only: bool,
}

/// A partition offered to the user as somewhere to add.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Partition {
    /// Partition label when it has one, else the device name, the spelling
    /// the user picks it by.
    pub label: String,
    /// `/dev/nvme1n1p1`
    pub device: PathBuf,
    pub point: PathBuf,
    pub fstype: String,
    pub uuid: Option<String>,
    pub disk: Option<crate::storage::Disk>,
    /// A library already covers this mount point.
    pub in_use: bool,
}

/// Filesystems that can hold a game install.
///
/// An allow-list rather than a deny-list: the question is not "is this a real
/// filesystem" but "would a 100 GB install on it work", and a list of the ones
/// known to is far shorter, and fails safe, next to a list of every pseudo
/// filesystem a kernel might mount.
const GAME_FILESYSTEMS: &[&str] = &[
    "ext2", "ext3", "ext4", "btrfs", "xfs", "f2fs", "jfs", "reiserfs", "nilfs2", "bcachefs",
    "zfs", "vfat", "exfat", "ntfs", "ntfs3", "fuseblk",
];

/// Mount points never offered: the system's own, whatever they hold.
///
/// A writability check is not enough on its own. A sticky, world-writable
/// system directory passes it perfectly well, `/var/tmp` is the one that
/// actually turned up on the machine this was written on, a btrfs subvolume of
/// the root filesystem, and is still the last place a 150 GB game install
/// belongs.
const SYSTEM_PREFIXES: &[&str] = &[
    "/boot", "/dev", "/efi", "/etc", "/proc", "/root", "/run", "/sys", "/tmp", "/usr", "/var",
];

/// Carved back out of [`SYSTEM_PREFIXES`]: where removable drives are
/// mounted, which is exactly what this is all for.
const REMOVABLE_PREFIXES: &[&str] = &["/run/media", "/media"];

/// Whether a mount point is the system's rather than the user's.
fn is_system_path(point: &Path) -> bool {
    if point == Path::new("/") {
        return true;
    }
    if REMOVABLE_PREFIXES.iter().any(|p| point.starts_with(p)) {
        return false;
    }
    SYSTEM_PREFIXES.iter().any(|p| point.starts_with(p))
}

/// Every mounted filesystem, from `/proc/self/mounts`.
///
/// Read directly rather than shelled out to `lsblk` or `findmnt`: a launcher
/// that needs a particular distribution's `util-linux` in `PATH` to list the
/// user's own disks is a launcher that breaks on an immutable image.
pub fn mounts() -> Vec<Mount> {
    let Ok(text) = std::fs::read_to_string("/proc/self/mounts") else {
        return Vec::new();
    };
    text.lines().filter_map(parse_mount).collect()
}

fn parse_mount(line: &str) -> Option<Mount> {
    let mut fields = line.split_whitespace();
    let device = unescape(fields.next()?);
    let point = unescape(fields.next()?);
    let fstype = fields.next()?.to_owned();
    let options = fields.next().unwrap_or_default();
    Some(Mount {
        device: PathBuf::from(device),
        point: PathBuf::from(point),
        fstype,
        read_only: options.split(',').any(|o| o == "ro"),
    })
}

/// `/proc` escapes space, tab, newline and backslash in octal.
fn unescape(field: &str) -> String {
    let mut out = String::with_capacity(field.len());
    let mut chars = field.chars();
    while let Some(c) = chars.next() {
        if c != '\\' {
            out.push(c);
            continue;
        }
        let octal: String = chars.clone().take(3).collect();
        match u8::from_str_radix(&octal, 8) {
            Ok(byte) if octal.len() == 3 => {
                out.push(byte as char);
                chars.nth(2);
            }
            _ => out.push(c),
        }
    }
    out
}

/// The partitions worth offering as a library, most interesting first.
///
/// Two filters, because neither alone is enough. Writability keeps out the
/// mounts that would need privileges Lightforge does not ask for: most of the
/// root filesystem's subvolumes. [`is_system_path`] keeps out the ones that
/// are writable and still wrong, `/var/tmp` being the real example. A
/// partition that is not mounted is not offered at all, mounting it would mean a dependency on udisks2 or a
/// setuid helper, and the system's own file manager already does it better.
pub fn partitions(existing: &[Library]) -> Vec<Partition> {
    let labels = by_id("/dev/disk/by-label");
    let uuids = by_id("/dev/disk/by-uuid");
    let mut found: Vec<Partition> = Vec::new();

    for mount in mounts() {
        if !mount.device.starts_with("/dev/")
            || mount.read_only
            || !GAME_FILESYSTEMS.contains(&mount.fstype.as_str())
            || is_system_path(&mount.point)
            || !is_writable(&mount.point)
            || found.iter().any(|p| p.point == mount.point)
        {
            continue;
        }
        let device = mount.device.file_name().unwrap_or_default().to_string_lossy();
        found.push(Partition {
            label: labels.get(device.as_ref()).cloned().unwrap_or_else(|| device.to_string()),
            in_use: existing.iter().any(|l| l.root().parent() == Some(&mount.point)
                || l.games.starts_with(&mount.point)),
            uuid: uuids.get(device.as_ref()).cloned(),
            disk: crate::storage::disk(&mount.point).ok(),
            device: mount.device,
            point: mount.point,
            fstype: mount.fstype,
        });
    }
    found.sort_by(|a, b| a.label.cmp(&b.label));
    found
}

/// Device name → symlink name, for one of `/dev/disk`'s index directories.
fn by_id(dir: &str) -> BTreeMap<String, String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return BTreeMap::new();
    };
    entries
        .flatten()
        .filter_map(|entry| {
            let target = std::fs::canonicalize(entry.path()).ok()?;
            let device = target.file_name()?.to_string_lossy().into_owned();
            Some((device, entry.file_name().to_string_lossy().into_owned()))
        })
        .collect()
}

/// Whether the current user could create the `Lightforge/` folder here.
fn is_writable(path: &Path) -> bool {
    let Ok(c_path) = CString::new(path.as_os_str().as_bytes()) else {
        return false;
    };
    // SAFETY: a valid NUL-terminated path; access only reads.
    unsafe { libc::access(c_path.as_ptr(), libc::W_OK) == 0 }
}

/// Find a library whose drive came back somewhere else, and return its new
/// `games` directory.
///
/// Only ever called for a library that is offline at its recorded path. The
/// UUID narrows the search to the mount points that *could* be it; the marker
/// decides. A library with no recorded UUID is searched for the same way
/// across every mount, since that is cheaper than telling the user their drive
/// is gone when it is sitting at a new path.
pub fn relocate(library: &Library) -> Option<PathBuf> {
    let uuids = by_id("/dev/disk/by-uuid");
    mounts()
        .into_iter()
        .filter(|mount| match (&library.uuid, mount.device.file_name()) {
            (Some(want), Some(device)) => {
                uuids.get(device.to_string_lossy().as_ref()) == Some(want)
            }
            // Nothing recorded, or a device `/dev/disk` has no entry for:
            // every mount is a candidate and the marker is the filter.
            _ => library.uuid.is_none(),
        })
        .map(|mount| mount.point.join(&library.relative))
        .find(|games| read_marker(&games.join(MARKER)).as_deref() == Some(library.id.as_str()))
}

/// The mount point a path is on: the longest mount point it sits under.
pub fn mount_point_of(path: &Path) -> PathBuf {
    mounts()
        .into_iter()
        .filter(|mount| path.starts_with(&mount.point))
        .max_by_key(|mount| mount.point.as_os_str().len())
        .map(|mount| mount.point)
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// How the user would name the drive `path` is on, with the folder the
/// library sits in when that is not the drive's root: "nvme2 · Battle.net".
///
/// The partition label, else the device name, as [`partitions`] spells it, so
/// a library adopted from a folder and one added from the same partition read
/// as the same drive. `None` when no device backs the path.
pub fn drive_label(games: &Path) -> Option<String> {
    let mount = mount_point_of(games);
    let device = mounts().into_iter().find(|m| m.point == mount)?.device;
    let name = device.file_name()?.to_string_lossy().into_owned();
    let drive = by_id("/dev/disk/by-label").get(&name).cloned().unwrap_or(name);
    let root = games.strip_prefix(&mount).ok().and_then(Path::parent);
    Some(match root {
        Some(root) if !root.as_os_str().is_empty() => format!("{drive} · {}", root.display()),
        _ => drive,
    })
}

/// The filesystem UUID a path is on, when `/dev/disk` names one.
pub fn uuid_of(path: &Path) -> Option<String> {
    let point = mount_point_of(path);
    let device = mounts().into_iter().find(|m| m.point == point)?.device;
    let name = device.file_name()?.to_string_lossy().into_owned();
    by_id("/dev/disk/by-uuid").get(&name).cloned()
}

/// Two free drive letters for a new library: one for `games/`, one for the WoW
/// install inside it.
///
/// `None` when the prefix has run out of letters, which takes twelve
/// libraries. The caller refuses the add rather than reusing a letter, a
/// letter that moves is a recorded install path that stops resolving.
pub fn allocate_letters(existing: &[Library]) -> Option<(String, String)> {
    let taken: Vec<&str> = existing
        .iter()
        .flat_map(|l| [l.games_letter.as_str(), l.wow_letter.as_str()])
        .collect();
    let mut free = LETTER_POOL.iter().filter(|letter| !taken.contains(*letter));
    Some((free.next()?.to_string(), free.next()?.to_string()))
}

/// Read the id out of a marker file, or `None` when it is missing or says
/// nothing we understand.
fn read_marker(path: &Path) -> Option<String> {
    let text = std::fs::read_to_string(path).ok()?;
    text.lines()
        .find_map(|line| line.trim().strip_prefix("id =").map(str::trim))
        .filter(|id| !id.is_empty())
        .map(str::to_owned)
}

fn marker_text(id: &str) -> String {
    format!(
        "# Lightforge storage library — please do not delete.\n\
         # This file is how Lightforge recognises the drive. Without it the\n\
         # drive looks like a new one, and the games on it look missing.\n\
         id = {id}\n"
    )
}

/// A short opaque id, from the kernel's entropy pool.
///
/// Falls back to the clock and the process id on the vanishingly unlikely
/// chance `/dev/urandom` cannot be read; an id only has to be unique among the
/// handful of drives one machine has.
fn new_id() -> String {
    use std::io::Read;

    let mut bytes = [0u8; 6];
    let read = std::fs::File::open("/dev/urandom").and_then(|mut f| f.read_exact(&mut bytes));
    if read.is_err() {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0)
            ^ u64::from(std::process::id());
        bytes.copy_from_slice(&now.to_le_bytes()[..6]);
    }
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[derive(Debug, thiserror::Error)]
pub enum LibraryError {
    #[error("i/o error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("{label} is not mounted")]
    Offline { label: String },
    #[error("{label} holds a Lightforge library that is not this one")]
    Foreign { label: String },
    #[error("the Wine prefix has no drive letters left; remove a drive before adding another")]
    NoLettersLeft,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("lightforge-library-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_claimed_library_is_online_and_an_empty_directory_is_not() {
        let root = scratch("claim");
        let mut library = Library::on_partition(&root, "data", None);
        assert!(!library.online());
        library.claim().unwrap();
        assert!(library.online());
        assert_eq!(library.games, root.join("Lightforge").join("games"));

        // What an unmounted drive looks like: the path is there, the marker
        // is not. This is the case that must never read as "empty library".
        std::fs::remove_file(library.marker()).unwrap();
        assert!(!library.online());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn claiming_a_folder_that_already_has_a_marker_adopts_its_id() {
        let root = scratch("adopt");
        let mut first = Library::on_partition(&root, "data", None);
        first.claim().unwrap();

        let mut second = Library::on_partition(&root, "data", None);
        assert_ne!(second.id, first.id);
        second.claim().unwrap();
        assert_eq!(second.id, first.id);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_library_is_found_again_at_a_new_mount_point() {
        let root = scratch("move");
        let mut library = Library::on_partition(&root.join("before"), "data", None);
        std::fs::create_dir_all(root.join("before")).unwrap();
        library.claim().unwrap();

        // The same volume, mounted elsewhere. With no UUID recorded every
        // mount is a candidate, so the search is driven by the marker, which
        // is also what makes this testable without a real device.
        std::fs::create_dir_all(root.join("after")).unwrap();
        std::fs::rename(root.join("before").join(DRIVE_FOLDER), root.join("after").join(DRIVE_FOLDER))
            .unwrap();
        assert!(!library.online());

        let moved = Library { games: root.join("after").join(DRIVE_FOLDER).join(GAMES_DIR), ..library.clone() };
        assert!(moved.online());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn letters_come_in_pairs_and_never_repeat() {
        let main = Library::main_at(PathBuf::from("/home/u/Games/Blizzard/games"), "home");
        assert_eq!((main.wow_letter.as_str(), main.games_letter.as_str()), ("d:", "e:"));

        let mut libraries = vec![main];
        let (games, wow) = allocate_letters(&libraries).unwrap();
        assert_eq!((games.as_str(), wow.as_str()), ("f:", "g:"));
        libraries.push(Library {
            games_letter: games,
            wow_letter: wow,
            ..Library::on_partition(Path::new("/mnt/games"), "nvme2", None)
        });
        assert_eq!(allocate_letters(&libraries).unwrap().0, "h:");
    }

    #[test]
    fn a_second_library_is_presented_under_its_own_games_drive() {
        let main = Library::main_at(PathBuf::from("/home/u/Games/Blizzard/games"), "home");
        assert_eq!(main.windows_wow_path(), crate::paths::WINDOWS_WOW_PATH);
        assert_eq!(main.windows_games_path(), r"E:\");

        let second = Library {
            games_letter: "f:".to_owned(),
            wow_letter: "g:".to_owned(),
            ..Library::on_partition(Path::new("/mnt/games"), "nvme2", None)
        };
        assert_eq!(second.windows_wow_path(), "F:/World of Warcraft");
        assert_eq!(second.windows_games_path(), r"F:\");
    }

    #[test]
    fn a_writable_system_directory_is_still_not_somewhere_to_install_a_game() {
        // The one that actually turned up: a btrfs subvolume of the root
        // filesystem, sticky and world-writable, so a writability check alone
        // would have offered it.
        assert!(is_system_path(Path::new("/var/tmp")));
        assert!(is_system_path(Path::new("/")));
        assert!(is_system_path(Path::new("/boot/efi")));
        // Where a removable drive lands, which is the whole point.
        assert!(!is_system_path(Path::new("/run/media/kenny/games")));
        assert!(!is_system_path(Path::new("/media/games")));
        assert!(!is_system_path(Path::new("/mnt/games")));
        assert!(!is_system_path(Path::new("/home")));
    }

    #[test]
    fn mount_lines_are_unescaped() {
        let mount = parse_mount("/dev/sda1 /mnt/my\\040games ext4 rw,noatime 0 0").unwrap();
        assert_eq!(mount.point, PathBuf::from("/mnt/my games"));
        assert!(!mount.read_only);
        assert!(parse_mount("/dev/sdb1 /mnt/x ext4 ro,noatime 0 0").unwrap().read_only);
    }

    #[test]
    fn the_real_machine_has_a_root_mount() {
        let mounts = mounts();
        assert!(mounts.iter().any(|m| m.point == Path::new("/")));
        // Nothing pseudo, nothing read-only and nothing we could not write to
        // is ever offered as somewhere to install a game, which is also what
        // keeps `/` and the root filesystem's other subvolumes off the list.
        for partition in partitions(&[]) {
            eprintln!(
                "offered: {} ({}) at {} on {}",
                partition.label,
                partition.fstype,
                partition.point.display(),
                partition.device.display(),
            );
            assert!(partition.device.starts_with("/dev/"));
            assert!(GAME_FILESYSTEMS.contains(&partition.fstype.as_str()));
            assert!(is_writable(&partition.point));
            assert!(!is_system_path(&partition.point));
        }
    }

    #[test]
    fn a_product_falls_back_to_the_default_library() {
        use crate::product::ProductCode;
        let main = Library::main_at(PathBuf::from("/home/u/Games/Blizzard/games"), "home");
        let second = Library::on_partition(Path::new("/mnt/games"), "nvme2", None);
        let second_id = second.id.clone();
        let mut libraries = Libraries::new(vec![main.clone(), second]);

        assert_eq!(libraries.of(&ProductCode::Retail).unwrap().id, main.id);
        libraries.assign(ProductCode::Retail, &second_id);
        assert_eq!(libraries.of(&ProductCode::Retail).unwrap().id, second_id);
        // An assignment to a library that is no longer configured.
        libraries.assign(ProductCode::Classic, "gone");
        assert_eq!(libraries.of(&ProductCode::Classic).unwrap().id, main.id);
    }
}
