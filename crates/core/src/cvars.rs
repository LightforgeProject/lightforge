//! `Config.wtf`: the client's startup settings file.
//!
//! The file is a flat list of `SET name "value"` lines that the client
//! rewrites from memory every time it exits. That has two consequences this
//! module is built around:
//!
//! * Unknown lines must survive untouched. The client puts everything it knows
//!   about in here, resolution, locale, account name, and a writer that
//!   rebuilt the file from a managed set would silently reset the user's
//!   entire client configuration.
//! * A value written here is authoritative only until the next exit, and the
//!   player can change the same setting in the game's own options. So
//!   [`reconcile`] runs before each launch against a record of what Lightforge
//!   itself wrote: a setting it never wrote is never touched, and one the
//!   player has changed since is left to the player.
//!
//! Nothing here touches the running client: the file is edited only while the
//! game is closed, behind the same pre-flight gate as every other mutation.

use std::path::{Path, PathBuf};

/// A parsed `Config.wtf`, preserving every line it did not come to change.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ConfigWtf {
    lines: Vec<Line>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Line {
    /// `SET name "value"`, split so the value can be replaced in place.
    Set { name: String, value: String },
    /// Anything else, blank lines, comments, malformed entries, kept verbatim.
    Other(String),
}

impl ConfigWtf {
    pub fn parse(text: &str) -> Self {
        let lines = text.lines().map(parse_line).collect();
        Self { lines }
    }

    /// Read the file, treating a missing one as empty.
    ///
    /// A product that has never been launched has no `Config.wtf` yet, and
    /// writing the settings ahead of the first run is both valid and useful,
    /// the client merges what it finds.
    pub fn read(path: &Path) -> Result<Self, CvarError> {
        match std::fs::read_to_string(path) {
            Ok(text) => Ok(Self::parse(&text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(source) => Err(CvarError::Io { path: path.to_path_buf(), source }),
        }
    }

    /// The value of a CVar, if the file sets it.
    ///
    /// Names are matched case-insensitively: the client's casing is the one
    /// that ends up in the file, and it does not always match the casing the
    /// wikis and guides use.
    pub fn get(&self, name: &str) -> Option<&str> {
        self.lines.iter().find_map(|line| match line {
            Line::Set { name: n, value } if n.eq_ignore_ascii_case(name) => Some(value.as_str()),
            _ => None,
        })
    }

    /// Set a CVar, returning whether the file actually changed.
    ///
    /// An existing entry keeps its position *and its spelling*: rewriting
    /// `gxApi` as `gxapi` would leave the client with two entries for one
    /// setting and no guarantee which one wins.
    pub fn set(&mut self, name: &str, value: &str) -> bool {
        for line in &mut self.lines {
            if let Line::Set { name: n, value: v } = line {
                if n.eq_ignore_ascii_case(name) {
                    if v == value {
                        return false;
                    }
                    *v = value.to_owned();
                    return true;
                }
            }
        }
        self.lines.push(Line::Set { name: name.to_owned(), value: value.to_owned() });
        true
    }

    /// Drop a CVar, returning whether it was there.
    ///
    /// Used when a toggle is switched off: leaving the value behind would mean
    /// an "off" toggle that still enforces its setting.
    pub fn remove(&mut self, name: &str) -> bool {
        let before = self.lines.len();
        self.lines.retain(
            |line| !matches!(line, Line::Set { name: n, .. } if n.eq_ignore_ascii_case(name)),
        );
        self.lines.len() != before
    }

    pub fn render(&self) -> String {
        let mut out = String::new();
        for line in &self.lines {
            match line {
                Line::Set { name, value } => {
                    out.push_str("SET ");
                    out.push_str(name);
                    out.push_str(" \"");
                    out.push_str(value);
                    out.push('"');
                }
                Line::Other(raw) => out.push_str(raw),
            }
            out.push('\n');
        }
        out
    }
}

fn parse_line(raw: &str) -> Line {
    let rest = match raw.trim_start().strip_prefix("SET ") {
        Some(rest) => rest,
        None => return Line::Other(raw.to_owned()),
    };
    let mut parts = rest.splitn(2, char::is_whitespace);
    match (parts.next(), parts.next()) {
        (Some(name), Some(value)) if !name.is_empty() => Line::Set {
            name: name.to_owned(),
            value: value.trim().trim_matches('"').to_owned(),
        },
        // `SET` with no value is not something the client writes; keeping it
        // verbatim is safer than inventing an empty value for it.
        _ => Line::Other(raw.to_owned()),
    }
}

/// The record beside `Config.wtf` of what Lightforge wrote into it.
///
/// Kept in the game's `WTF` folder rather than Lightforge's own data, so it
/// travels with the file it describes when an install moves between drives.
pub const RECORD: &str = ".lightforge-cvars.json";

/// One setting Lightforge wrote.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Written {
    pub value: String,
    /// The player changed it in the game since: the value is theirs until
    /// they ask Lightforge for a different one.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub yielded: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct Record {
    #[serde(default)]
    written: std::collections::BTreeMap<String, Written>,
}

impl Record {
    /// A record that cannot be read is treated as empty: the worst case is
    /// that a setting Lightforge wrote is treated as the player's.
    fn read(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|bytes| serde_json::from_slice(&bytes).ok())
            .unwrap_or_default()
    }
}

/// What a [`reconcile`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Reconciled {
    /// Entries of `Config.wtf` written or removed.
    pub changed: usize,
    /// Settings left as the player set them in the game, though a toggle
    /// asks for something else.
    pub yielded: Vec<String>,
}

/// Bring the `Config.wtf` at `path` in line with `wanted`, the settings the
/// enabled toggles ask for, touching only what Lightforge wrote.
///
/// Per setting, against the record of what was written last time:
///
/// * never written and not wanted: left alone, whatever its value;
/// * wanted and not yet written: written, or adopted when already equal;
/// * written, and the file still holds that value (or lost it): kept in step
///   with `wanted`, and removed when no toggle asks for it any more;
/// * written, and the file now holds something else: the player changed it.
///   It is left alone and marked yielded, until the toggle is turned off or
///   set to a different value, which is the player asking again.
///
/// `retired` are names earlier versions wrote by mistake, removed outright.
pub fn reconcile(
    path: &Path,
    wanted: &[(&str, String)],
    retired: &[&str],
) -> Result<Reconciled, CvarError> {
    let record_path = path.with_file_name(RECORD);
    let mut file = ConfigWtf::read(path)?;
    let old = Record::read(&record_path);
    let mut record = Record::default();
    let mut out = Reconciled::default();

    for name in retired {
        out.changed += usize::from(file.remove(name));
    }

    let want = |name: &str| wanted.iter().find(|(n, _)| n.eq_ignore_ascii_case(name)).map(|(_, v)| v);

    // What was written before and is no longer asked for.
    for (name, written) in &old.written {
        if want(name).is_some() {
            continue;
        }
        if !written.yielded && file.get(name) == Some(written.value.as_str()) {
            out.changed += usize::from(file.remove(name));
        }
    }

    for (name, value) in wanted {
        let current = file.get(name).map(str::to_owned);
        let ours = match old.written.get(*name) {
            None => true,
            // Still ours, or the client dropped it: write it again. Changed
            // in the game, it is the player's, unless the toggle now asks for
            // a different value than it did, which is the player asking.
            Some(w) if !w.yielded => {
                current.is_none() || current.as_deref() == Some(w.value.as_str()) || w.value != *value
            }
            // The player's, unless they have since asked for something new.
            Some(w) => w.value != *value,
        };
        let ours = ours || current.as_deref() == Some(value.as_str());
        if ours {
            out.changed += usize::from(file.set(name, value));
            record.written.insert((*name).to_owned(), Written { value: value.clone(), yielded: false });
        } else {
            record.written.insert((*name).to_owned(), Written { value: value.clone(), yielded: true });
            out.yielded.push((*name).to_owned());
        }
    }

    if out.changed > 0 {
        write_atomic(path, file.render().as_bytes())?;
    }
    // After Config.wtf, so a crash in between leaves a record that is behind
    // the file, which the next run reads as "already equal, adopt it".
    if record != old {
        let json = serde_json::to_vec_pretty(&record).expect("a map of strings serializes");
        write_atomic(&record_path, &json)?;
    }
    Ok(out)
}

/// Write via a temp file in the destination directory plus a rename: a crash
/// mid-write must not be able to truncate the user's client configuration.
fn write_atomic(path: &Path, bytes: &[u8]) -> Result<(), CvarError> {
    use std::io::Write;

    let io = |p: &Path| {
        let p = p.to_path_buf();
        move |source| CvarError::Io { path: p.clone(), source }
    };

    let dir = path.parent().unwrap_or(Path::new("."));
    std::fs::create_dir_all(dir).map_err(io(dir))?;

    let mut tmp = tempfile::NamedTempFile::new_in(dir).map_err(io(dir))?;
    tmp.write_all(bytes).map_err(io(tmp.path()))?;
    tmp.as_file().sync_all().map_err(io(tmp.path()))?;
    tmp.persist(path).map_err(|e| CvarError::Io { path: path.to_path_buf(), source: e.error })?;
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum CvarError {
    #[error("{path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "SET readTOS \"1\"\nSET gxApi \"D3D12\"\nSET locale \"enGB\"\n";

    #[test]
    fn unmanaged_lines_survive_a_write() {
        let mut file = ConfigWtf::parse(SAMPLE);
        file.set("gxApi", "D3D11");
        let out = file.render();
        assert!(out.contains("SET readTOS \"1\""));
        assert!(out.contains("SET locale \"enGB\""));
        assert!(out.contains("SET gxApi \"D3D11\""));
        assert!(!out.contains("D3D12"));
    }

    #[test]
    fn an_existing_entry_keeps_its_place_and_its_spelling() {
        let mut file = ConfigWtf::parse(SAMPLE);
        // The guides write `gxapi`; the client writes `gxApi`. Matching
        // loosely but preserving the file's own casing is what keeps the
        // client from ending up with two entries for one setting.
        assert!(file.set("gxapi", "D3D11"));
        assert_eq!(
            file.render(),
            "SET readTOS \"1\"\nSET gxApi \"D3D11\"\nSET locale \"enGB\"\n"
        );
    }

    #[test]
    fn a_new_entry_is_appended() {
        let mut file = ConfigWtf::parse(SAMPLE);
        assert!(file.set("scriptProfile", "0"));
        assert!(file.render().ends_with("SET scriptProfile \"0\"\n"));
    }

    #[test]
    fn setting_a_value_it_already_has_is_not_a_change() {
        let mut file = ConfigWtf::parse(SAMPLE);
        assert!(!file.set("gxApi", "D3D12"));
    }

    #[test]
    fn comments_and_blank_lines_are_not_mistaken_for_settings() {
        let text = "\n# hand-edited\nSET gxApi \"D3D12\"\nSET\n";
        let file = ConfigWtf::parse(text);
        assert_eq!(file.get("gxApi"), Some("D3D12"));
        assert_eq!(file.render(), text);
    }

    #[test]
    fn a_missing_file_reads_as_empty_rather_than_failing() {
        let dir = tempfile::tempdir().unwrap();
        let file = ConfigWtf::read(&dir.path().join("Config.wtf")).unwrap();
        assert_eq!(file, ConfigWtf::default());
    }

    fn want(pairs: &[(&'static str, &str)]) -> Vec<(&'static str, String)> {
        pairs.iter().map(|(n, v)| (*n, (*v).to_owned())).collect()
    }

    fn wtf(dir: &tempfile::TempDir, text: &str) -> PathBuf {
        let path = dir.path().join("Config.wtf");
        std::fs::write(&path, text).unwrap();
        path
    }

    /// The client rewriting the file on exit, with the player's choices.
    fn player_sets(path: &Path, name: &str, value: &str) {
        let mut file = ConfigWtf::read(path).unwrap();
        file.set(name, value);
        std::fs::write(path, file.render()).unwrap();
    }

    fn get(path: &Path, name: &str) -> Option<String> {
        ConfigWtf::read(path).unwrap().get(name).map(str::to_owned)
    }

    #[test]
    fn settings_lightforge_never_wrote_are_never_touched() {
        // The bug this replaced: a default profile removed the player's own
        // gxApi, frame cap and view distance before every launch.
        let dir = tempfile::tempdir().unwrap();
        let path = wtf(&dir, "SET gxApi \"D3D12\"\nSET maxFPS \"144\"\nSET farclip \"5000\"\n");

        let done = reconcile(&path, &[], &[]).unwrap();
        assert_eq!(done.changed, 0);
        assert_eq!(get(&path, "gxApi").as_deref(), Some("D3D12"));
        assert_eq!(get(&path, "maxFPS").as_deref(), Some("144"));
    }

    #[test]
    fn a_wanted_setting_is_written_once_and_then_left_alone_when_unchanged() {
        let dir = tempfile::tempdir().unwrap();
        let path = wtf(&dir, SAMPLE);
        let wanted = want(&[("gxApi", "D3D11"), ("scriptProfile", "0")]);

        assert_eq!(reconcile(&path, &wanted, &[]).unwrap().changed, 2);
        assert_eq!(reconcile(&path, &wanted, &[]).unwrap().changed, 0);
        assert_eq!(get(&path, "locale").as_deref(), Some("enGB"));
    }

    #[test]
    fn a_toggle_switched_off_takes_back_only_its_own_value() {
        let dir = tempfile::tempdir().unwrap();
        let path = wtf(&dir, "SET locale \"enGB\"\n");
        reconcile(&path, &want(&[("gxApi", "D3D11")]), &[]).unwrap();

        assert_eq!(reconcile(&path, &[], &[]).unwrap().changed, 1);
        assert_eq!(get(&path, "gxApi"), None);
        assert_eq!(get(&path, "locale").as_deref(), Some("enGB"));
    }

    #[test]
    fn a_setting_changed_in_the_game_is_the_players() {
        let dir = tempfile::tempdir().unwrap();
        let path = wtf(&dir, "");
        let wanted = want(&[("maxFPS", "60")]);
        reconcile(&path, &wanted, &[]).unwrap();

        player_sets(&path, "maxFPS", "144");
        let done = reconcile(&path, &wanted, &[]).unwrap();
        assert_eq!(done.yielded, ["maxFPS"]);
        assert_eq!(get(&path, "maxFPS").as_deref(), Some("144"));

        // Still theirs on the launch after, not reclaimed.
        assert_eq!(reconcile(&path, &wanted, &[]).unwrap().changed, 0);
        assert_eq!(get(&path, "maxFPS").as_deref(), Some("144"));

        // Switching the toggle off leaves their value where it is.
        reconcile(&path, &[], &[]).unwrap();
        assert_eq!(get(&path, "maxFPS").as_deref(), Some("144"));
    }

    #[test]
    fn asking_for_a_new_value_in_lightforge_takes_it_back() {
        let dir = tempfile::tempdir().unwrap();
        let path = wtf(&dir, "");
        reconcile(&path, &want(&[("maxFPS", "60")]), &[]).unwrap();
        player_sets(&path, "maxFPS", "144");
        reconcile(&path, &want(&[("maxFPS", "60")]), &[]).unwrap();

        let done = reconcile(&path, &want(&[("maxFPS", "90")]), &[]).unwrap();
        assert!(done.yielded.is_empty());
        assert_eq!(get(&path, "maxFPS").as_deref(), Some("90"));
    }

    #[test]
    fn a_value_the_client_dropped_is_written_again() {
        let dir = tempfile::tempdir().unwrap();
        let path = wtf(&dir, "");
        let wanted = want(&[("scriptProfile", "0")]);
        reconcile(&path, &wanted, &[]).unwrap();

        std::fs::write(&path, "").unwrap();
        assert_eq!(reconcile(&path, &wanted, &[]).unwrap().changed, 1);
        assert_eq!(get(&path, "scriptProfile").as_deref(), Some("0"));
    }

    #[test]
    fn retired_names_are_removed_outright() {
        let dir = tempfile::tempdir().unwrap();
        let path = wtf(&dir, "SET addonProfilerEnabled \"0\"\nSET locale \"enGB\"\n");
        assert_eq!(reconcile(&path, &[], &["addonProfilerEnabled"]).unwrap().changed, 1);
        assert_eq!(get(&path, "addonProfilerEnabled"), None);
    }

    #[test]
    fn a_file_that_does_not_exist_yet_is_created_with_its_record() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("WTF").join("Config.wtf");
        assert_eq!(reconcile(&path, &want(&[("scriptProfile", "0")]), &[]).unwrap().changed, 1);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "SET scriptProfile \"0\"\n");
        assert!(path.with_file_name(RECORD).is_file());
    }
}
