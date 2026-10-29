//! Lightforge as a game in the Steam library.
//!
//! One entry in `userdata/<account>/config/shortcuts.vdf` (binary VDF), and
//! pictures named by app id in `config/grid/`, read when Steam starts.
//! Measured 2026-10-16: an entry written while Steam runs survives its
//! shutdown, but starting a non-Steam game first writes the file back from
//! memory and drops it. Other entries are kept byte for byte.

use std::path::{Path, PathBuf};

use crate::steam::SteamRoot;

/// What a SteamID64 is, less the account number in its low 32 bits.
const STEAM_ID64_BASE: u64 = 76_561_197_960_265_728;

/// One picture for the library, and the extension Steam should see it under.
#[derive(Debug, Clone, Copy)]
pub struct Image<'a> {
    pub bytes: &'a [u8],
    pub ext: &'static str,
}

/// Everything Steam shows for a game, by where it shows it.
#[derive(Debug, Clone, Copy)]
pub struct Artwork<'a> {
    /// 600x900, the library's grid.
    pub capsule: Image<'a>,
    /// 920x430, recent games and the wide grid.
    pub wide: Image<'a>,
    /// 1920x620, across the top of the game's page.
    pub hero: Image<'a>,
    /// Laid over the hero.
    pub logo: Image<'a>,
    /// Square, the list and the overlay.
    pub icon: Image<'a>,
}

/// What [`add`] did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Added {
    /// The account the shortcut was added for.
    pub account: u32,
    pub appid: u32,
    /// Whether an entry for it was there already and was updated instead.
    pub updated: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum ShortcutError {
    #[error("Steam is not installed")]
    NoSteam,

    #[error("no Steam account has signed in on this machine yet")]
    NoAccount,

    #[error("{path} is not a shortcuts file Lightforge can read")]
    Malformed { path: PathBuf },

    #[error("{path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> ShortcutError + '_ {
    move |source| ShortcutError::Io { path: path.to_path_buf(), source }
}


/// Put `name`, started by `exe`, into the Steam library of the account that
/// signed in last, with `art` for its pictures.
pub fn add(steam: &SteamRoot, name: &str, exe: &Path, art: &Artwork) -> Result<Added, ShortcutError> {
    let account = last_account(steam).ok_or(ShortcutError::NoAccount)?;
    let config = steam.path().join("userdata").join(account.to_string()).join("config");
    let file = config.join("shortcuts.vdf");

    let mut root = match std::fs::read(&file) {
        Ok(bytes) => parse(&bytes).ok_or_else(|| ShortcutError::Malformed { path: file.clone() })?,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(source) => return Err(ShortcutError::Io { path: file, source }),
    };

    let exe_field = quoted(exe);
    let appid = appid(&exe_field, name);
    let grid = config.join("grid");
    let icon = grid.join(format!("{appid}_icon.{}", art.icon.ext));

    let shortcuts = shortcuts_mut(&mut root);
    let fields = entry(appid, name, &exe_field, exe, &icon);
    // Ours by name or by program: a moved launcher is still one entry.
    let ours = shortcuts.iter().position(|(_, value)| match value {
        Value::Map(entry) => {
            get_str(entry, "AppName").is_some_and(|n| n.eq_ignore_ascii_case(name.as_bytes()))
                || get_str(entry, "Exe") == Some(exe_field.as_bytes())
        }
        _ => false,
    });
    let updated = match ours {
        Some(at) => {
            // Only our fields: launch options, play time and collections are theirs.
            if let (_, Value::Map(entry)) = &mut shortcuts[at] {
                for (key, value) in fields.into_iter().filter(|(key, _)| IDENTITY.contains(key)) {
                    set(entry, key, value);
                }
            }
            true
        }
        None => {
            // Keys are positions; the largest plus one is safe even with gaps.
            let next = shortcuts
                .iter()
                .filter_map(|(key, _)| std::str::from_utf8(key).ok()?.parse::<u32>().ok())
                .max()
                .map_or(0, |n| n + 1);
            let mut fresh = Vec::new();
            for (key, value) in fields {
                set(&mut fresh, key, value);
            }
            shortcuts.push((next.to_string().into_bytes(), Value::Map(fresh)));
            false
        }
    };

    for (image, file) in [
        (art.capsule, format!("{appid}p.{}", art.capsule.ext)),
        (art.wide, format!("{appid}.{}", art.wide.ext)),
        (art.hero, format!("{appid}_hero.{}", art.hero.ext)),
        (art.logo, format!("{appid}_logo.{}", art.logo.ext)),
        (art.icon, format!("{appid}_icon.{}", art.icon.ext)),
    ] {
        let path = grid.join(file);
        crate::fsutil::write_atomic(&path, image.bytes).map_err(io(&path))?;
    }
    crate::fsutil::write_atomic(&file, &render(&root)).map_err(io(&file))?;

    Ok(Added { account, appid, updated })
}

/// The app id Steam gives a non-Steam game: a CRC-32 of its program, quoted
/// as Steam stores it, and its name, with the top bit set. The grid files are
/// named by it.
pub fn appid(exe_field: &str, name: &str) -> u32 {
    let mut crc = flate2::Crc::new();
    crc.update(exe_field.as_bytes());
    crc.update(name.as_bytes());
    crc.sum() | 0x8000_0000
}

fn quoted(path: &Path) -> String {
    format!("\"{}\"", path.display())
}

/// The fields [`add`] owns on an entry that is already there.
const IDENTITY: [&str; 5] = ["appid", "AppName", "Exe", "StartDir", "icon"];

/// The fields of one shortcut, in the order Steam itself writes them.
fn entry(appid: u32, name: &str, exe_field: &str, exe: &Path, icon: &Path) -> Vec<(&'static str, Value)> {
    let start = exe.parent().map(quoted).unwrap_or_default();
    let text = |s: &str| Value::Str(s.as_bytes().to_vec());
    vec![
        ("appid", Value::Int(appid)),
        ("AppName", text(name)),
        ("Exe", text(exe_field)),
        ("StartDir", text(&start)),
        ("icon", text(&icon.display().to_string())),
        ("ShortcutPath", text("")),
        ("LaunchOptions", text("")),
        ("IsHidden", Value::Int(0)),
        ("AllowDesktopConfig", Value::Int(1)),
        ("AllowOverlay", Value::Int(1)),
        ("OpenVR", Value::Int(0)),
        ("Devkit", Value::Int(0)),
        ("DevkitGameID", text("")),
        ("DevkitOverrideAppID", Value::Int(0)),
        ("LastPlayTime", Value::Int(0)),
        ("FlatpakAppID", text("")),
        ("tags", Value::Map(Vec::new())),
    ]
}

/// The account that signed in last, by `loginusers.vdf`; failing that, the
/// only account that has ever used this Steam.
fn last_account(steam: &SteamRoot) -> Option<u32> {
    let users = std::fs::read_to_string(steam.path().join("config").join("loginusers.vdf"));
    if let Some(account) = users.ok().as_deref().and_then(most_recent) {
        return Some(account);
    }
    let accounts: Vec<u32> = std::fs::read_dir(steam.path().join("userdata"))
        .ok()?
        .filter_map(|entry| entry.ok()?.file_name().to_str()?.parse().ok())
        .filter(|&id| id != 0)
        .collect();
    match accounts.as_slice() {
        [only] => Some(*only),
        _ => None,
    }
}

/// The account that signed in last, by a `loginusers.vdf`.
///
/// Older clients mark `"MostRecent" "1"`; October 2026's writes only a
/// `"timestamp"`, and the newest wins. Text VDF read as a flat token list.
pub(crate) fn most_recent(users: &str) -> Option<u32> {
    let mut tokens = Vec::new();
    let mut rest = users;
    while let Some(at) = rest.find(['"', '{', '}']) {
        match rest.as_bytes()[at] {
            b'"' => {
                let tail = &rest[at + 1..];
                let end = tail.find('"')?;
                tokens.push(&tail[..end]);
                rest = &tail[end + 1..];
            }
            brace => {
                tokens.push(if brace == b'{' { "{" } else { "}" });
                rest = &rest[at + 1..];
            }
        }
    }

    // (SteamID64, marked most recent, timestamp) per account.
    let mut accounts: Vec<(u64, bool, u64)> = Vec::new();
    for window in tokens.windows(2) {
        match window {
            [id, "{"] => {
                if let Ok(id) = id.parse::<u64>() {
                    accounts.push((id, false, 0));
                }
            }
            [key, value] => {
                let Some(account) = accounts.last_mut() else { continue };
                if key.eq_ignore_ascii_case("MostRecent") {
                    account.1 = *value == "1";
                } else if key.eq_ignore_ascii_case("timestamp") {
                    account.2 = value.parse().unwrap_or(0);
                }
            }
            _ => {}
        }
    }
    let (id, _, _) = accounts.into_iter().max_by_key(|&(_, marked, timestamp)| (marked, timestamp))?;
    id.checked_sub(STEAM_ID64_BASE).and_then(|n| u32::try_from(n).ok())
}

// ------------------------------------------------------------ binary VDF --

/// One value in a binary VDF file. Strings are kept as bytes, so whatever
/// another tool wrote into somebody's shortcuts goes back out unchanged.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum Value {
    Map(Vec<(Vec<u8>, Value)>),
    Str(Vec<u8>),
    Int(u32),
    Float(u32),
    Long(u64),
}

const MAP: u8 = 0x00;
const STR: u8 = 0x01;
const INT: u8 = 0x02;
const FLOAT: u8 = 0x03;
const LONG: u8 = 0x07;
const END: u8 = 0x08;

/// The whole file: the top-level map's entries.
pub(crate) fn parse(bytes: &[u8]) -> Option<Vec<(Vec<u8>, Value)>> {
    let mut at = 0;
    map(bytes, &mut at, true)
}

/// A map's entries, to its end marker or, at the top level, the end of the
/// file. An inner map hitting the end is a truncated file, an error.
fn map(bytes: &[u8], at: &mut usize, top: bool) -> Option<Vec<(Vec<u8>, Value)>> {
    let mut entries = Vec::new();
    loop {
        let Some(&kind) = bytes.get(*at) else {
            return top.then_some(entries);
        };
        *at += 1;
        if kind == END {
            return Some(entries);
        }
        let key = cstr(bytes, at)?;
        let value = match kind {
            MAP => Value::Map(map(bytes, at, false)?),
            STR => Value::Str(cstr(bytes, at)?),
            INT => Value::Int(u32::from_le_bytes(take(bytes, at)?)),
            FLOAT => Value::Float(u32::from_le_bytes(take(bytes, at)?)),
            LONG => Value::Long(u64::from_le_bytes(take(bytes, at)?)),
            _ => return None,
        };
        entries.push((key, value));
    }
}

fn cstr(bytes: &[u8], at: &mut usize) -> Option<Vec<u8>> {
    let len = bytes.get(*at..)?.iter().position(|&b| b == 0)?;
    let s = bytes[*at..*at + len].to_vec();
    *at += len + 1;
    Some(s)
}

fn take<const N: usize>(bytes: &[u8], at: &mut usize) -> Option<[u8; N]> {
    let chunk = bytes.get(*at..*at + N)?.try_into().ok()?;
    *at += N;
    Some(chunk)
}

pub(crate) fn render(root: &[(Vec<u8>, Value)]) -> Vec<u8> {
    let mut out = Vec::new();
    write_map(&mut out, root);
    out.push(END);
    out
}

fn write_map(out: &mut Vec<u8>, entries: &[(Vec<u8>, Value)]) {
    for (key, value) in entries {
        let kind = match value {
            Value::Map(_) => MAP,
            Value::Str(_) => STR,
            Value::Int(_) => INT,
            Value::Float(_) => FLOAT,
            Value::Long(_) => LONG,
        };
        out.push(kind);
        out.extend_from_slice(key);
        out.push(0);
        match value {
            Value::Map(inner) => {
                write_map(out, inner);
                out.push(END);
            }
            Value::Str(s) => {
                out.extend_from_slice(s);
                out.push(0);
            }
            Value::Int(n) | Value::Float(n) => out.extend_from_slice(&n.to_le_bytes()),
            Value::Long(n) => out.extend_from_slice(&n.to_le_bytes()),
        }
    }
}

/// The `shortcuts` map at the top of the file, made if it is not there.
fn shortcuts_mut(root: &mut Vec<(Vec<u8>, Value)>) -> &mut Vec<(Vec<u8>, Value)> {
    let at = match root.iter().position(|(key, value)| {
        key.eq_ignore_ascii_case(b"shortcuts") && matches!(value, Value::Map(_))
    }) {
        Some(at) => at,
        None => {
            root.push((b"shortcuts".to_vec(), Value::Map(Vec::new())));
            root.len() - 1
        }
    };
    match &mut root[at].1 {
        Value::Map(entries) => entries,
        _ => unreachable!("checked to be a map above"),
    }
}

/// A string field, by a key Steam does not always spell the same way.
fn get_str<'v>(entry: &'v [(Vec<u8>, Value)], key: &str) -> Option<&'v [u8]> {
    entry.iter().find_map(|(k, v)| match v {
        Value::Str(s) if k.eq_ignore_ascii_case(key.as_bytes()) => Some(s.as_slice()),
        _ => None,
    })
}

/// Replace a field in place, or add it at the end.
fn set(entry: &mut Vec<(Vec<u8>, Value)>, key: &str, value: Value) {
    match entry.iter_mut().find(|(k, _)| k.eq_ignore_ascii_case(key.as_bytes())) {
        Some((_, slot)) => *slot = value,
        None => entry.push((key.as_bytes().to_vec(), value)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn art() -> Artwork<'static> {
        let image = |bytes: &'static [u8], ext| Image { bytes, ext };
        Artwork {
            capsule: image(b"capsule", "jpg"),
            wide: image(b"wide", "jpg"),
            hero: image(b"hero", "jpg"),
            logo: image(b"logo", "png"),
            icon: image(b"icon", "png"),
        }
    }

    /// A Steam root with one signed-in account, and that account's id.
    fn steam(name: &str) -> (tempfile::TempDir, SteamRoot, u32) {
        let dir = tempfile::Builder::new().prefix(name).tempdir().unwrap();
        let account = 125_143_762;
        let id64 = STEAM_ID64_BASE + account as u64;
        std::fs::create_dir_all(dir.path().join("config")).unwrap();
        std::fs::write(
            dir.path().join("config/loginusers.vdf"),
            format!(
                "\"users\"\n{{\n\t\"76561198000000001\"\n\t{{\n\t\t\"AccountName\"\t\t\"other\"\n\t\t\"MostRecent\"\t\t\"0\"\n\t}}\n\
                 \t\"{id64}\"\n\t{{\n\t\t\"AccountName\"\t\t\"me\"\n\t\t\"MostRecent\"\t\t\"1\"\n\t}}\n}}\n"
            ),
        )
        .unwrap();
        let root = SteamRoot::with_path(dir.path());
        (dir, root, account)
    }

    fn shortcuts_of(root: &SteamRoot, account: u32) -> Vec<(Vec<u8>, Value)> {
        let file = root.path().join(format!("userdata/{account}/config/shortcuts.vdf"));
        let mut parsed = parse(&std::fs::read(file).unwrap()).unwrap();
        std::mem::take(shortcuts_mut(&mut parsed))
    }

    /// The id other tools compute for the same program and name, checked
    /// against Python's zlib.crc32.
    #[test]
    fn the_app_id_is_the_one_steam_gives_it() {
        assert_eq!(appid("\"/usr/bin/lightforge-gui\"", "Lightforge"), 2_447_834_017);
    }

    #[test]
    fn the_last_account_to_sign_in_is_the_one_used() {
        let (_dir, root, account) = steam("lf-steam-user");
        assert_eq!(last_account(&root), Some(account));
    }

    /// The client of October 2026 marks nobody; the newest sign-in wins.
    #[test]
    fn without_a_mark_the_newest_sign_in_is_the_one_used() {
        let users = "\"users\"\n{\n\t\"76561198085409490\"\n\t{\n\t\t\"AccountName\"\t\t\"a\"\n\
                     \t\t\"timestamp\"\t\t\"1792178000\"\n\t}\n\t\"76561198000000001\"\n\t{\n\
                     \t\t\"AccountName\"\t\t\"b\"\n\t\t\"timestamp\"\t\t\"1700000000\"\n\t}\n}\n";
        assert_eq!(most_recent(users), Some(125_143_762));
    }

    /// Someone's own shortcuts stay, byte for byte, and adding Lightforge a
    /// second time updates its entry instead of adding another.
    #[test]
    fn adding_keeps_every_other_shortcut_and_never_adds_twice() {
        let (_dir, root, account) = steam("lf-steam-add");
        let theirs = vec![(
            b"shortcuts".to_vec(),
            Value::Map(vec![(
                b"0".to_vec(),
                Value::Map(vec![
                    (b"appid".to_vec(), Value::Int(123)),
                    (b"AppName".to_vec(), Value::Str(b"Their Game \xff".to_vec())),
                    (b"Exe".to_vec(), Value::Str(b"\"/opt/game\"".to_vec())),
                    (b"LastPlayTime".to_vec(), Value::Int(1_700_000_000)),
                ]),
            )]),
        )];
        let file = root.path().join(format!("userdata/{account}/config/shortcuts.vdf"));
        crate::fsutil::write_atomic(&file, &render(&theirs)).unwrap();

        let exe = Path::new("/home/deck/.local/bin/lightforge-gui");
        let first = add(&root, "Lightforge", exe, &art()).unwrap();
        assert!(!first.updated);

        // Launch options typed into Steam since, which a second add keeps.
        let mut parsed = parse(&std::fs::read(&file).unwrap()).unwrap();
        if let Value::Map(ours) = &mut shortcuts_mut(&mut parsed)[1].1 {
            set(ours, "LaunchOptions", Value::Str(b"MANGOHUD=1 %command%".to_vec()));
        }
        crate::fsutil::write_atomic(&file, &render(&parsed)).unwrap();

        let second = add(&root, "Lightforge", exe, &art()).unwrap();
        assert!(second.updated);
        assert_eq!(first.appid, second.appid);

        let shortcuts = shortcuts_of(&root, account);
        assert_eq!(shortcuts.len(), 2, "Lightforge was added twice");
        let Value::Map(ref kept) = shortcuts[0].1 else { panic!("not a map") };
        let Value::Map(ref old) = theirs[0].1 else { unreachable!() };
        let Value::Map(ref old) = old[0].1 else { unreachable!() };
        assert_eq!(kept, old, "their shortcut changed");
        let Value::Map(ref ours) = shortcuts[1].1 else { panic!("not a map") };
        assert_eq!(shortcuts[1].0, b"1");
        assert_eq!(get_str(ours, "Exe"), Some(b"\"/home/deck/.local/bin/lightforge-gui\"".as_slice()));
        assert_eq!(get_str(ours, "LaunchOptions"), Some(b"MANGOHUD=1 %command%".as_slice()));

        let grid = root.path().join(format!("userdata/{account}/config/grid"));
        for name in ["p.jpg", ".jpg", "_hero.jpg", "_logo.png", "_icon.png"] {
            assert!(grid.join(format!("{}{name}", first.appid)).is_file(), "no {name}");
        }
    }

    #[test]
    fn a_first_shortcut_makes_the_file() {
        let (_dir, root, account) = steam("lf-steam-new");
        add(&root, "Lightforge", Path::new("/usr/bin/lightforge-gui"), &art()).unwrap();
        assert_eq!(shortcuts_of(&root, account).len(), 1);
    }

    /// What Steam itself wrote, read and written back unchanged.
    #[test]
    fn a_file_reads_and_writes_back_byte_for_byte() {
        let mut bytes = b"\x00shortcuts\x00\x000\x00\x02appid\x00".to_vec();
        bytes.extend_from_slice(&0x9234_5678u32.to_le_bytes());
        bytes.extend_from_slice(b"\x01AppName\x00Probe\x00\x00tags\x00\x010\x00fav\x00\x08\x08\x08\x08");
        assert_eq!(render(&parse(&bytes).unwrap()), bytes);
    }

    #[test]
    fn a_truncated_file_is_refused_rather_than_rewritten() {
        assert!(parse(b"\x00shortcuts\x00\x000\x00\x01AppName\x00Pro").is_none());
        // Cut between two fields: every string is whole, the maps are not.
        assert!(parse(b"\x00shortcuts\x00\x000\x00\x01AppName\x00Probe\x00").is_none());
    }
}
