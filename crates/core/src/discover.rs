//! Blizzard installs that were on this machine before Lightforge was.
//!
//! A first run should not ask someone to download 120 GB they already have.
//! Almost everyone arriving here has played WoW on this machine through Lutris,
//! Bottles, Heroic or a hand-made prefix, and every one of those put a
//! Battle.net in a Wine prefix, which means Agent's own registry, `product.db`,
//! is already on disk saying which products are installed and exactly where.
//! That file is the whole discovery mechanism: no filesystem crawl, no
//! guessing, and it answers for games on other disks because the paths it holds
//! are absolute.
//!
//! The scan is deliberately bounded. It looks only where a Battle.net prefix is
//! actually put ([`hints`]), reads two files per prefix, and never walks a
//! drive it was not pointed at. Nothing leaves the machine and nothing is
//! written, [`adopt`] is a separate call the user has to ask for.

use std::path::{Path, PathBuf};

use crate::buildinfo::BuildInfo;
use crate::library::{self, Library};
use crate::paths::{self, Layout, WOW_DIR};
use crate::product::ProductCode;
use crate::tact::productdb;

/// How deep under a hint directory a prefix may sit before it is given up on.
///
/// Two: Lutris keeps prefixes at `~/Games/<game>`, Bottles at
/// `<bottles>/<name>`, Steam at `compatdata/<appid>/pfx`. Nobody nests one
/// deeper than that, and every level costs a `read_dir` of someone's home.
const MAX_DEPTH: usize = 2;

/// Directories read per level. A guard against being pointed at something
/// enormous, not a real limit, no launcher keeps hundreds of prefixes.
const MAX_ENTRIES: usize = 128;

/// Which launcher left a prefix behind, for a sentence the user can recognise.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Origin {
    Lutris,
    Bottles,
    Heroic,
    Faugus,
    Steam,
    /// A prefix made by hand, `~/.wine` and friends.
    Wine,
    /// Somewhere the user pointed us at themselves.
    Pointed,
}

impl Origin {
    pub fn label(self) -> &'static str {
        match self {
            Self::Lutris => "Lutris",
            Self::Bottles => "Bottles",
            Self::Heroic => "Heroic",
            Self::Faugus => "Faugus",
            Self::Steam => "Steam",
            Self::Wine => "Wine",
            Self::Pointed => "the folder you picked",
        }
    }
}

/// One Battle.net prefix found on this machine, and what is installed in it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    /// The Wine prefix itself, the directory holding `drive_c`.
    pub prefix: PathBuf,
    pub origin: Origin,
    /// Battle.net's own install directory, when the prefix has one.
    pub battle_net: Option<PathBuf>,
    /// WoW installs, one per install root, several flavours share one.
    pub wow: Vec<WowInstall>,
    /// Everything else Battle.net has installed in there.
    pub hand_offs: Vec<HandOff>,
}

impl Found {
    /// Nothing worth offering the user.
    pub fn is_empty(&self) -> bool {
        self.wow.is_empty() && self.hand_offs.is_empty()
    }
}

/// One WoW install root, `World of Warcraft/`, with its flavours inside.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WowInstall {
    pub path: PathBuf,
    /// The products installed in it, as `.build.info` and `product.db` agree.
    pub products: Vec<ProductCode>,
    pub bytes: u64,
    /// Why this install cannot be adopted as it stands, if it cannot.
    pub blocked: Option<String>,
}

impl WowInstall {
    /// Adopting this install means renaming its folder first.
    ///
    /// A library installs its games as `<games>/World of Warcraft`, so a folder
    /// called anything else cannot be adopted in place without moving it. The
    /// rename is within one directory, which costs nothing, but it is still a
    /// change to someone's existing install, so it is said out loud rather than
    /// done quietly.
    pub fn needs_rename(&self) -> bool {
        self.path.file_name().is_some_and(|name| name != WOW_DIR)
    }
}

/// One game Battle.net installed that Lightforge does not manage.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HandOff {
    /// Agent's product code, e.g. `hs_beta`.
    pub code: String,
    /// The install folder's own name, "Hearthstone". Agent's codes name
    /// nothing to anyone, and the folder is what the user recognises.
    pub label: String,
    pub path: PathBuf,
    pub version: Option<String>,
    pub bytes: u64,
}

/// Every place a Battle.net prefix is plausibly kept, under `home`.
///
/// Ordered by how likely it is to hold a real one, because the wizard shows
/// what it found in this order.
pub fn hints(home: &Path) -> Vec<(PathBuf, Origin)> {
    let flatpak = home.join(".var").join("app");
    let mut found = vec![
        // Lutris installs each game into its own prefix directory here.
        (home.join("Games"), Origin::Lutris),
        (home.join(".local").join("share").join("bottles").join("bottles"), Origin::Bottles),
        (
            flatpak.join("com.usebottles.bottles").join("data").join("bottles").join("bottles"),
            Origin::Bottles,
        ),
        (home.join("Games").join("Heroic").join("Prefixes"), Origin::Heroic),
        (
            flatpak.join("com.heroicgameslauncher.hgl").join("config").join("heroic").join("Prefixes"),
            Origin::Heroic,
        ),
        // Faugus, the launcher Bazzite suggests, puts each prefix straight
        // under `~/Faugus/<name>`, flatpak or not.
        (home.join("Faugus"), Origin::Faugus),
        (home.join(".wine"), Origin::Wine),
        (home.join(".local").join("share").join("wineprefixes"), Origin::Wine),
    ];
    // Steam's own compat data, in every library folder it has, a Battle.net
    // added as a non-Steam game lives in one of these.
    if let Some(steam) = crate::steam::SteamRoot::discover_in(home) {
        for library in steam.library_paths() {
            found.push((library.join("steamapps").join("compatdata"), Origin::Steam));
        }
    }
    found
}

/// Scan this machine for Battle.net prefixes, skipping `ours`.
///
/// `ours` is the prefix Lightforge manages: it is found by exactly the same
/// rules as everyone else's, and offering someone their own games back would
/// be a comedy.
pub fn scan(ours: &Path) -> Vec<Found> {
    let Some(dirs) = directories::UserDirs::new() else {
        return Vec::new();
    };
    let mut found = Vec::new();
    let mut seen: Vec<PathBuf> = Vec::new();
    for (hint, origin) in hints(dirs.home_dir()) {
        for prefix in prefixes_under(&hint, MAX_DEPTH) {
            if prefix == ours || seen.contains(&prefix) {
                continue;
            }
            seen.push(prefix.clone());
            if let Some(entry) = inspect(&prefix, origin) {
                if !entry.is_empty() {
                    found.push(entry);
                }
            }
        }
    }
    found
}

/// Every Wine prefix at or under `dir`, at most `depth` levels down.
fn prefixes_under(dir: &Path, depth: usize) -> Vec<PathBuf> {
    if paths::is_prefix(dir) {
        return vec![dir.to_path_buf()];
    }
    // Proton always builds its prefix at `<compat data>/pfx`, so a directory
    // that holds one is a prefix one level down by a name that is never
    // anything else. Checked here rather than by spending a level of `depth`
    // on it: Lightforge's own root is `<root>/prefix/pfx`, three deep under
    // `~/Games`, and a scan that walked three levels of someone's home to find
    // it would read every game directory they have.
    if paths::is_prefix(&dir.join("pfx")) {
        return vec![dir.join("pfx")];
    }
    if depth == 0 {
        return Vec::new();
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .flatten()
        .take(MAX_ENTRIES)
        .filter(|entry| entry.file_type().is_ok_and(|t| t.is_dir()))
        .flat_map(|entry| prefixes_under(&entry.path(), depth - 1))
        .collect()
}

/// Read one prefix: what Battle.net has installed in it, and where.
///
/// `None` only when the directory is not a prefix at all. A prefix with a
/// Battle.net and no games still comes back, so the wizard can say "Battle.net
/// is here but it has nothing installed" rather than nothing.
pub fn inspect(prefix: &Path, origin: Origin) -> Option<Found> {
    if !paths::is_prefix(prefix) {
        return None;
    }
    // Someone else's prefix has no [`Layout`], Lightforge does not own it and
    // its root is not laid out like ours. What is the same in every prefix is
    // the inside of `drive_c`, which is what these three free functions read.
    let dosdevices = prefix.join("dosdevices");
    let battle_net = paths::battle_net_dir_in(prefix);
    let mut entry = Found {
        prefix: prefix.to_path_buf(),
        origin,
        battle_net: battle_net.is_dir().then_some(battle_net),
        wow: Vec::new(),
        hand_offs: Vec::new(),
    };

    let installed =
        productdb::read(&paths::agent_dir_in(prefix).join("product.db")).unwrap_or_default();
    let mut wow_roots: Vec<(PathBuf, Vec<ProductCode>)> = Vec::new();
    for record in installed {
        if record.is_infrastructure() {
            continue;
        }
        let Some(path) = paths::resolve_windows_in(&dosdevices, &record.install_path) else {
            continue;
        };
        if !path.is_dir() {
            continue;
        }
        // Through the symlinks, deliberately. A Windows path inside a prefix
        // routinely resolves to a link pointing at the real install elsewhere,
        // Lightforge's own `C:\Program Files (x86)\World of Warcraft` is one,
        // and adopting the link would make a library out of `drive_c` instead
        // of the directory the games are actually in.
        let path = std::fs::canonicalize(&path).unwrap_or(path);
        match is_wow(&record) {
            true => {
                let code: ProductCode = record.product.parse().expect("codes are infallible");
                match wow_roots.iter_mut().find(|(root, _)| root == &path) {
                    Some((_, codes)) => codes.push(code),
                    None => wow_roots.push((path, vec![code])),
                }
            }
            false => entry.hand_offs.push(HandOff {
                code: record.product,
                label: folder_name(&path),
                version: record.version,
                bytes: crate::fsutil::dir_size(&path),
                path,
            }),
        }
    }

    for (path, mut products) in wow_roots {
        // `.build.info` is the authority on which flavours are really there:
        // Agent keeps a record for a product whose files were deleted by hand,
        // and offering to adopt a flavour that is not on disk would end in a
        // client that offers a 90 GB repair.
        if let Ok(info) = BuildInfo::load(path.join(".build.info")) {
            let rows: Vec<ProductCode> = info.products().collect();
            if !rows.is_empty() {
                products = rows;
            }
        }
        products.retain(|code| path.join(code.install_dir().as_ref()).is_dir());
        if products.is_empty() {
            continue;
        }
        entry.wow.push(WowInstall {
            bytes: crate::fsutil::dir_size(&path),
            blocked: None,
            products,
            path,
        });
    }
    Some(entry)
}

/// Inspect a directory the user pointed at themselves.
///
/// Accepts either a Wine prefix or a WoW install root, because people point at
/// what they recognise and "the folder with my game in it" is far more
/// recognisable than "the directory containing drive_c".
pub fn inspect_pointed(path: &Path) -> Option<Found> {
    if let Some(found) = inspect(path, Origin::Pointed) {
        return Some(found);
    }
    // A Proton prefix is named `pfx` inside its compat-data directory, and a
    // file picker lands on the parent.
    if let Some(found) = inspect(&path.join("pfx"), Origin::Pointed) {
        return Some(found);
    }
    let install = wow_install_at(path)?;
    Some(Found {
        prefix: PathBuf::new(),
        origin: Origin::Pointed,
        battle_net: None,
        wow: vec![install],
        hand_offs: Vec::new(),
    })
}

/// Read a WoW install root directly, with no Battle.net anywhere in sight.
///
/// The `.build.info` is what makes this safe to offer: it names the products,
/// their builds and their CDN configuration, which is everything an adopted
/// install needs to be patched from. A directory of game files without one is
/// not an install Lightforge can manage, however much it looks like one.
pub fn wow_install_at(path: &Path) -> Option<WowInstall> {
    let info = BuildInfo::load(path.join(".build.info")).ok()?;
    let products: Vec<ProductCode> = info
        .products()
        .filter(|code| path.join(code.install_dir().as_ref()).is_dir())
        .collect();
    if products.is_empty() {
        return None;
    }
    Some(WowInstall {
        path: path.to_path_buf(),
        products,
        bytes: crate::fsutil::dir_size(path),
        blocked: None,
    })
}

/// Whether an Agent record describes a WoW flavour rather than another game.
///
/// The family field is the honest answer, Agent writes `wow` for every
/// flavour, but it is empty in records written by older clients, so the code
/// prefix is the fallback. Both are needed: `w1r` (Warcraft I Remastered)
/// starts with `w` and is nobody's WoW.
fn is_wow(record: &productdb::Installed) -> bool {
    record.family == "wow" || record.product.starts_with("wow")
}

fn folder_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.display().to_string())
}

/// What adopting an install would do, before anything is written.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    /// The install as it is right now, before any rename.
    pub install: PathBuf,
    /// The library the install's parent directory becomes.
    pub library: Library,
    /// The products that library will hold.
    pub products: Vec<ProductCode>,
    /// The folder has to be renamed to `World of Warcraft` first.
    pub rename: Option<PathBuf>,
}

/// Plan the adoption of a WoW install found elsewhere, without touching disk.
///
/// Adoption is **in place**: not one byte is copied. The install's parent
/// directory becomes a Lightforge storage library, the products in it are
/// assigned to that library, and the prefix reaches them through the library's
/// own two drive letters like any other. A 123 GB install is adopted in the
/// time it takes to write a marker file.
///
/// The parent, not the install: a library is a `games/` directory with game
/// folders inside it, which is exactly what the parent of
/// `…/World of Warcraft` is.
pub fn plan(install: &WowInstall, existing: &[Library]) -> Result<Plan, AdoptError> {
    let parent = install.path.parent().ok_or(AdoptError::NoParent)?;
    let rename = install.needs_rename().then(|| parent.join(WOW_DIR));
    if let Some(target) = &rename {
        if target.exists() {
            return Err(AdoptError::Occupied(target.clone()));
        }
    }
    if existing.iter().any(|l| l.games() == parent) {
        return Err(AdoptError::AlreadyOurs);
    }
    let (games_letter, wow_letter) =
        library::allocate_letters(existing).ok_or(library::LibraryError::NoLettersLeft)?;
    let label = library::drive_label(parent).unwrap_or_else(|| folder_name(parent));
    let mut library = Library::adopted(parent.to_path_buf(), label);
    library.games_letter = games_letter;
    library.wow_letter = wow_letter;
    library.probe();
    Ok(Plan {
        install: install.path.clone(),
        library,
        products: install.products.clone(),
        rename,
    })
}

/// What adoption did, and what it could not do.
#[derive(Debug, Clone)]
pub struct Adopted {
    /// The library to add to the config.
    pub library: Library,
    /// The install root's owner-write bit was missing and was added.
    pub repaired: bool,
    /// Why Battle.net was not told about the install. The games are adopted
    /// either way; this is the half that only Battle.net needs.
    pub battle_net: Option<String>,
}

/// Carry out a [`Plan`]: rename if it must, claim the library, and tell
/// Battle.net where the install now is.
///
/// Returns the library to add; the caller assigns products, saves and
/// relinks, as for adding a drive. Battle.net is told through `.product.db`
/// (what Locate reads) and Agent's registry (what the library is built
/// from), both rewritten from the install's `.build.info`. **Neither is
/// fatal**: the marker, config entry and drive letter already make the
/// install Lightforge's, and a root that refused `.product.db` once left a
/// 100 GB game reading Install. The reason comes back for the caller to show.
pub fn adopt(plan: &Plan, layout: &Layout) -> Result<Adopted, AdoptError> {
    let mut library = plan.library.clone();
    if let Some(target) = &plan.rename {
        std::fs::rename(&plan.install, target).map_err(|source| AdoptError::Io {
            path: target.clone(),
            source,
        })?;
    }
    library.claim()?;

    let install = library.games().join(WOW_DIR);
    let repaired = matches!(
        crate::paths::ensure_writable(&install),
        Ok(crate::paths::Repaired::Yes)
    );
    let info = BuildInfo::load(install.join(".build.info"))?;
    let mut records = Vec::new();
    for code in &plan.products {
        records.push(productdb::ProductInstall::from_build_info(
            &info,
            code,
            windows_wow_path(&library),
        )?);
    }

    let battle_net = match productdb::write(&install, &records) {
        // A prefix that has not been built yet has no registry to merge into,
        // which is the ordinary case during a first run. The next pre-flight
        // writes it, so a `false` here is not worth reporting.
        Ok(_) => match productdb::register(&layout.agent_dir(), &records) {
            Ok(_) => None,
            Err(e) => Some(crate::error::chain(&e)),
        },
        Err(e) => Some(crate::error::chain(&e)),
    };
    Ok(Adopted { library, repaired, battle_net })
}

/// The Windows path an adopted install is recorded under.
fn windows_wow_path(library: &Library) -> String {
    format!("{}/{}", drive(&library.games_letter), WOW_DIR)
}

fn drive(letter: &str) -> String {
    letter.trim_end_matches(':').to_uppercase() + ":"
}

/// Make a game Battle.net installed elsewhere visible to Lightforge's
/// Battle.net, without moving it.
///
/// The folder is symlinked into the default library's `games/` (already a
/// drive, so it gets `E:\Hearthstone`), and its registry record is copied
/// from the other prefix **as raw bytes** with only the path rewritten:
/// Agent's record has dozens of fields nothing here understands. `false` when
/// there was no record, meaning Battle.net will have to find it itself.
pub fn link_hand_off(
    game: &HandOff,
    foreign: &Path,
    layout: &Layout,
    library: &Library,
) -> Result<bool, AdoptError> {
    use crate::prefix::LinkState;

    let link = library.games().join(&game.label);
    let io = |source| AdoptError::Io { path: link.clone(), source };
    match crate::prefix::inspect_link(&link, &game.path) {
        // Already linked, by us or by a previous run of this.
        LinkState::Correct | LinkState::Pending(_) => {}
        // Left by an earlier adoption of the same game from somewhere else.
        // Only the symlink goes, never what it pointed at.
        LinkState::Mislinked(_) => {
            std::fs::remove_file(&link).map_err(io)?;
            std::os::unix::fs::symlink(&game.path, &link).map_err(io)?;
        }
        LinkState::Missing => std::os::unix::fs::symlink(&game.path, &link).map_err(io)?,
        LinkState::OccupiedEmpty | LinkState::Occupied => return Err(AdoptError::Occupied(link)),
    }

    let their_registry = paths::agent_dir_in(foreign).join("product.db");
    let Ok(bytes) = std::fs::read(&their_registry) else {
        return Ok(false);
    };
    let windows = format!("{}{}", library.windows_games_path(), game.label);
    let Some(record) = productdb::lift(&bytes, &game.code, &windows) else {
        return Ok(false);
    };
    Ok(productdb::graft(&layout.agent_dir(), &[record])?)
}

#[derive(Debug, thiserror::Error)]
pub enum AdoptError {
    #[error("that install has no parent directory to make a library of")]
    NoParent,

    #[error("Lightforge already manages that folder")]
    AlreadyOurs,

    #[error("{0} already exists")]
    Occupied(PathBuf),

    #[error(transparent)]
    Library(#[from] library::LibraryError),

    #[error(transparent)]
    BuildInfo(#[from] crate::buildinfo::BuildInfoError),

    #[error(transparent)]
    ProductDb(#[from] productdb::ProductDbError),

    #[error("i/o error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tact::productdb::ProductInstall;

    /// A real single-product `.build.info`, trimmed to the columns a
    /// `product.db` record is built from.
    const BUILD_INFO: &str = concat!(
        "Branch!STRING:0|Active!DEC:1|Build Key!HEX:16|CDN Key!HEX:16|Tags!STRING:0|",
        "Version!STRING:0|Product!STRING:0\n",
        "eu|1|9712ae9c84cc54111e9e3ddf9a6b8019|904e68a1fd418798b47ec3f9dba8da38|",
        "Windows x86_64 EU? enUS speech?:Windows x86_64 EU? enUS text?|12.1.0.69814|wow\n",
    );

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir()
            .join(format!("lightforge-discover-{}-{name}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn record(product: &str, install_path: &str, family: &str) -> ProductInstall {
        ProductInstall {
            product: product.to_owned(),
            install_path: install_path.to_owned(),
            region: "eu".to_owned(),
            locale: "enUS".to_owned(),
            account_country: String::new(),
            geo_country: String::new(),
            flavor_dir: String::new(),
            version: "1.0.0".to_owned(),
            build_key: "9712ae9c84cc54111e9e3ddf9a6b8019".to_owned(),
            tags: String::new(),
            key_ring: None,
            installed_at_ms: 0,
            family: family.to_owned(),
        }
    }

    /// Someone else's prefix: a `drive_c`, a `dosdevices` mapping, Agent's
    /// registry, and games on two different directories.
    fn foreign_prefix(dir: &Path, games: &Path) -> PathBuf {
        let prefix = dir.join("pfx");
        let drive_c = prefix.join("drive_c");
        std::fs::create_dir_all(drive_c.join("Program Files (x86)")).unwrap();
        std::fs::create_dir_all(prefix.join("dosdevices")).unwrap();
        std::os::unix::fs::symlink("../drive_c", prefix.join("dosdevices").join("c:")).unwrap();
        // A second drive letter over a directory outside the prefix: the whole
        // reason the install paths are resolved rather than assumed.
        std::fs::create_dir_all(games).unwrap();
        std::os::unix::fs::symlink(games, prefix.join("dosdevices").join("f:")).unwrap();

        let wow = games.join(WOW_DIR);
        std::fs::create_dir_all(wow.join("_retail_")).unwrap();
        std::fs::write(wow.join(".build.info"), BUILD_INFO).unwrap();
        std::fs::write(wow.join("_retail_").join("Wow.exe"), [0u8; 8]).unwrap();

        let hearthstone = drive_c.join("Program Files (x86)").join("Hearthstone");
        std::fs::create_dir_all(&hearthstone).unwrap();
        std::fs::write(hearthstone.join("Hearthstone.exe"), [0u8; 16]).unwrap();

        let agent = paths::agent_dir_in(&prefix);
        std::fs::create_dir_all(&agent).unwrap();
        std::fs::write(
            agent.join("product.db"),
            productdb::encode(&[
                record("agent", "C:/ProgramData/Battle.net/Agent", "agent"),
                record("wow", "F:/World of Warcraft", "wow"),
                record("hs_beta", "C:/Program Files (x86)/Hearthstone", "hsb"),
                // A product Agent still remembers whose files are gone. Offering
                // to adopt it would end in the client proposing a repair.
                record("wow_classic_era", "F:/Classic Era That Is Not There", "wow"),
            ]),
        )
        .unwrap();
        prefix
    }

    #[test]
    fn a_foreign_prefix_gives_up_its_games_and_their_real_paths() {
        let dir = scratch("inspect");
        let games = dir.join("mnt").join("nvme2").join("Games");
        let prefix = foreign_prefix(&dir, &games);

        let found = inspect(&prefix, Origin::Lutris).expect("that is a prefix");
        assert_eq!(found.origin, Origin::Lutris);
        assert!(found.battle_net.is_none(), "no Battle.net directory was created");

        // One WoW install, on the other drive, resolved through `dosdevices`.
        assert_eq!(found.wow.len(), 1);
        assert_eq!(found.wow[0].path, games.join(WOW_DIR));
        assert_eq!(found.wow[0].products, vec![ProductCode::Retail]);
        assert!(found.wow[0].bytes > 0);
        assert!(!found.wow[0].needs_rename());

        // Agent itself is infrastructure, not a game.
        assert_eq!(found.hand_offs.len(), 1);
        assert_eq!(found.hand_offs[0].code, "hs_beta");
        assert_eq!(found.hand_offs[0].label, "Hearthstone");
    }

    #[test]
    fn a_product_agent_remembers_but_disk_does_not_is_not_offered() {
        let dir = scratch("stale");
        let games = dir.join("games");
        let prefix = foreign_prefix(&dir, &games);
        // The record names a directory that does not exist at all, so it never
        // reaches the flavour check, and the one that does exist is listed
        // only because `.build.info` and the disk agree about it.
        let found = inspect(&prefix, Origin::Lutris).unwrap();
        assert_eq!(found.wow.len(), 1);
        assert_eq!(found.wow[0].products, vec![ProductCode::Retail]);
    }

    #[test]
    fn a_directory_that_is_not_a_prefix_is_not_one() {
        let dir = scratch("empty");
        assert!(inspect(&dir, Origin::Wine).is_none());
        assert!(inspect_pointed(&dir).is_none());
    }

    #[test]
    fn pointing_at_a_wow_folder_works_without_any_battle_net() {
        let dir = scratch("pointed");
        let wow = dir.join("Games").join(WOW_DIR);
        std::fs::create_dir_all(wow.join("_retail_")).unwrap();
        std::fs::write(wow.join(".build.info"), BUILD_INFO).unwrap();

        let found = inspect_pointed(&wow).expect("a .build.info is enough");
        assert_eq!(found.origin, Origin::Pointed);
        assert_eq!(found.wow[0].path, wow);
        assert_eq!(found.wow[0].products, vec![ProductCode::Retail]);

        // Without a `.build.info` there is nothing to patch the install from,
        // whatever else the folder holds.
        std::fs::remove_file(wow.join(".build.info")).unwrap();
        assert!(inspect_pointed(&wow).is_none());
    }

    #[test]
    fn pointing_at_the_compat_data_directory_finds_the_prefix_inside_it() {
        let dir = scratch("compatdata");
        let games = dir.join("games");
        foreign_prefix(&dir, &games);
        // A file picker lands on the directory holding `pfx`, not on `pfx`.
        let found = inspect_pointed(&dir).expect("pfx is one level in");
        assert_eq!(found.wow.len(), 1);
    }

    #[test]
    fn adoption_makes_the_installs_parent_a_library_and_moves_nothing() {
        let dir = scratch("adopt");
        let games = dir.join("mnt").join("nvme2").join("Games");
        let prefix = foreign_prefix(&dir, &games);
        let found = inspect(&prefix, Origin::Lutris).unwrap();
        let existing = vec![Library::main_at(dir.join("Blizzard").join("games"), "Home")];

        let plan = plan(&found.wow[0], &existing).expect("that install can be adopted");
        assert_eq!(plan.library.games(), games);
        assert!(plan.rename.is_none());
        // Not the default library's letters: a letter that moves is a recorded
        // install path that stops resolving.
        assert_ne!(plan.library.games_letter, existing[0].games_letter);
        assert_ne!(plan.library.wow_letter, existing[0].wow_letter);

        let layout = Layout::with_root(dir.join("Blizzard"));
        let adopted = adopt(&plan, &layout).expect("adoption writes a marker and a product.db");
        assert_eq!(adopted.library.state(), library::State::Ready);
        assert!(adopted.library.claimed);
        assert!(!adopted.repaired, "the install root was writable all along");
        assert!(
            adopted.battle_net.is_none(),
            "`.product.db` went in: {:?}",
            adopted.battle_net
        );
        // The install is where it always was, with its own files untouched.
        assert!(games.join(WOW_DIR).join("_retail_").join("Wow.exe").is_file());
        // And Battle.net is told where to find it, under the library's letter.
        let registered = productdb::read(&games.join(WOW_DIR).join(".product.db")).unwrap();
        assert_eq!(registered.len(), 1);
        assert_eq!(registered[0].product, "wow");
        assert_eq!(
            registered[0].install_path,
            format!("{}/{}", plan.library.games_letter.trim_end_matches(':').to_uppercase() + ":", WOW_DIR)
        );
    }

    /// The install root a user already has is not always one that will take a
    /// new file. Adoption writes `.product.db` into it, and that used to be
    /// the step that decided whether a 100 GB game appeared in the launcher
    /// at all.
    #[test]
    fn an_install_root_that_refuses_a_file_is_still_adopted() {
        use std::os::unix::fs::PermissionsExt;

        if unsafe { libc::geteuid() } == 0 {
            return;
        }
        let dir = scratch("refusing-root");
        let games = dir.join("mnt").join("nvme2").join("Games");
        let prefix = foreign_prefix(&dir, &games);
        let found = inspect(&prefix, Origin::Lutris).unwrap();
        let install = games.join(WOW_DIR);
        std::fs::set_permissions(&install, std::fs::Permissions::from_mode(0o555)).unwrap();

        let plan = plan(&found.wow[0], &[]).unwrap();
        let adopted = adopt(&plan, &Layout::with_root(dir.join("Blizzard")))
            .expect("a read-only root does not stop adoption");

        assert!(adopted.repaired, "the owner-write bit was added");
        assert!(adopted.battle_net.is_none(), "so `.product.db` went in after all");
        assert!(install.join(".product.db").is_file());
    }

    /// And when it cannot be written for a reason no permission change fixes,
    /// the games are still adopted. Only Battle.net's half is lost, and the
    /// caller is told which half and why.
    #[test]
    fn a_product_db_that_cannot_be_written_costs_battle_net_and_nothing_else() {
        let dir = scratch("no-product-db");
        let games = dir.join("mnt").join("nvme2").join("Games");
        let prefix = foreign_prefix(&dir, &games);
        let found = inspect(&prefix, Origin::Lutris).unwrap();
        // A non-empty directory where the file goes: the rename cannot land,
        // whatever the permissions say.
        let blocker = games.join(WOW_DIR).join(".product.db");
        std::fs::create_dir_all(blocker.join("in the way")).unwrap();

        let plan = plan(&found.wow[0], &[]).unwrap();
        let adopted = adopt(&plan, &Layout::with_root(dir.join("Blizzard")))
            .expect("the games are adopted regardless");

        assert_eq!(adopted.library.state(), library::State::Ready);
        let reason = adopted.battle_net.expect("the reason comes back");
        assert!(reason.contains(".product.db"), "it names the file: {reason}");
        assert!(reason.contains(": "), "and carries its cause: {reason}");
        // The files themselves are untouched.
        assert!(games.join(WOW_DIR).join("_retail_").join("Wow.exe").is_file());
    }

    #[test]
    fn an_install_in_a_folder_by_another_name_is_renamed_rather_than_moved() {
        let dir = scratch("rename");
        let wow = dir.join("Games").join("wow-retail");
        std::fs::create_dir_all(wow.join("_retail_")).unwrap();
        std::fs::write(wow.join(".build.info"), BUILD_INFO).unwrap();
        let install = wow_install_at(&wow).unwrap();
        assert!(install.needs_rename());

        let plan = plan(&install, &[]).unwrap();
        assert_eq!(plan.rename, Some(dir.join("Games").join(WOW_DIR)));
        adopt(&plan, &Layout::with_root(dir.join("Blizzard"))).unwrap();
        assert!(dir.join("Games").join(WOW_DIR).join("_retail_").is_dir());
        assert!(!wow.exists());
    }

    #[test]
    fn a_folder_lightforge_already_manages_is_not_adopted_twice() {
        let dir = scratch("ours");
        let games = dir.join("Blizzard").join("games");
        let wow = games.join(WOW_DIR);
        std::fs::create_dir_all(wow.join("_retail_")).unwrap();
        std::fs::write(wow.join(".build.info"), BUILD_INFO).unwrap();
        let install = wow_install_at(&wow).unwrap();
        let existing = vec![Library::main_at(games, "Home")];
        assert!(matches!(
            plan(&install, &existing),
            Err(AdoptError::AlreadyOurs)
        ));
    }

    #[test]
    fn a_hand_off_is_linked_into_the_library_rather_than_copied() {
        let dir = scratch("handoff");
        let games = dir.join("games");
        let prefix = foreign_prefix(&dir, &games);
        let found = inspect(&prefix, Origin::Lutris).unwrap();
        let game = &found.hand_offs[0];

        let mut library = Library::main_at(dir.join("Blizzard").join("games"), "Home");
        library.claim().unwrap();
        let layout = Layout::with_root(dir.join("Blizzard"));
        std::fs::create_dir_all(layout.agent_dir()).unwrap();
        // Our own registry, with only Battle.net's own components in it.
        std::fs::write(
            layout.agent_dir().join("product.db"),
            productdb::encode(&[record("agent", "C:/ProgramData/Battle.net/Agent", "agent")]),
        )
        .unwrap();

        assert!(link_hand_off(game, &prefix, &layout, &library).unwrap());
        let link = library.games().join("Hearthstone");
        assert!(link.is_symlink(), "the game is linked, not copied");
        assert_eq!(std::fs::read_link(&link).unwrap(), game.path);

        // The grafted record keeps everything Agent wrote and only moves the
        // install path onto the library's own drive.
        let ours = productdb::read(&layout.agent_dir().join("product.db")).unwrap();
        let hs = ours.iter().find(|i| i.product == "hs_beta").expect("grafted");
        assert_eq!(hs.install_path, "E:\\Hearthstone");
        assert_eq!(hs.version.as_deref(), Some("1.0.0"));
        assert!(ours.iter().any(|i| i.product == "agent"), "nothing else was dropped");

        // Running it twice is not an error: the link is already there.
        assert!(link_hand_off(game, &prefix, &layout, &library).is_ok());
    }

    #[test]
    fn a_hand_off_link_pointing_elsewhere_is_repointed() {
        let dir = scratch("handoff-stale");
        let games = dir.join("games");
        let prefix = foreign_prefix(&dir, &games);
        let found = inspect(&prefix, Origin::Lutris).unwrap();
        let game = &found.hand_offs[0];

        let mut library = Library::main_at(dir.join("Blizzard").join("games"), "Home");
        library.claim().unwrap();
        let layout = Layout::with_root(dir.join("Blizzard"));
        let old = dir.join("moved-away");
        std::fs::create_dir_all(&old).unwrap();
        let link = library.games().join("Hearthstone");
        std::os::unix::fs::symlink(&old, &link).unwrap();

        link_hand_off(game, &prefix, &layout, &library).unwrap();

        assert_eq!(std::fs::read_link(&link).unwrap(), game.path);
        assert!(old.is_dir(), "the old target is left alone");
    }

    #[test]
    fn hints_never_wander_outside_the_home_they_were_given() {
        let home = Path::new("/home/someone");
        for (path, _) in hints(home) {
            assert!(
                path.starts_with(home) || path.starts_with("/"),
                "{} is not under {}",
                path.display(),
                home.display()
            );
        }
        // Lutris and a hand-made prefix are both covered; a scan that only knew
        // about Steam would miss the two most common ways to run Battle.net.
        let origins: Vec<Origin> = hints(home).into_iter().map(|(_, o)| o).collect();
        assert!(origins.contains(&Origin::Lutris));
        assert!(origins.contains(&Origin::Wine));
        assert!(origins.contains(&Origin::Bottles));
        assert!(origins.contains(&Origin::Heroic));
        assert!(origins.contains(&Origin::Faugus));
    }
}
