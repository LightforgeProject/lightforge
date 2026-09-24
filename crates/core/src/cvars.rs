//! `Config.wtf` — the client's startup settings file.
//!
//! The file is a flat list of `SET name "value"` lines that the client
//! rewrites from memory every time it exits. That has two consequences this
//! module is built around:
//!
//! * Unknown lines must survive untouched. The client puts everything it knows
//!   about in here — resolution, locale, account name — and a writer that
//!   rebuilt the file from a managed set would silently reset the user's
//!   entire client configuration.
//! * A value written here is authoritative only until the next exit. Anything
//!   Lightforge wants to *hold* has to be re-asserted before each launch,
//!   which is why [`apply`] is cheap and idempotent rather than one-shot.
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
    /// Anything else — blank lines, comments, malformed entries — kept verbatim.
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
    /// writing the settings ahead of the first run is both valid and useful —
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
    /// An existing entry keeps its position *and its spelling* — rewriting
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

/// Apply `settings` to the `Config.wtf` at `path`, removing `unset`.
///
/// Returns how many entries changed, so a caller can stay quiet when a
/// re-assertion before launch found nothing to do — the common case.
pub fn apply(
    path: &Path,
    settings: &[(&str, String)],
    unset: &[&str],
) -> Result<usize, CvarError> {
    let mut file = ConfigWtf::read(path)?;
    let mut changed = 0;
    for name in unset {
        changed += usize::from(file.remove(name));
    }
    for (name, value) in settings {
        changed += usize::from(file.set(name, value));
    }
    if changed > 0 {
        write_atomic(path, file.render().as_bytes())?;
    }
    Ok(changed)
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

    #[test]
    fn apply_writes_only_when_something_changed() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Config.wtf");
        std::fs::write(&path, SAMPLE).unwrap();

        let settings = [("gxApi", "D3D11".to_owned()), ("scriptProfile", "0".to_owned())];
        assert_eq!(apply(&path, &settings, &[]).unwrap(), 2);
        // Re-asserting before every launch must be free when nothing drifted.
        assert_eq!(apply(&path, &settings, &[]).unwrap(), 0);

        let written = std::fs::read_to_string(&path).unwrap();
        assert!(written.contains("SET gxApi \"D3D11\""));
        assert!(written.contains("SET locale \"enGB\""));
    }

    #[test]
    fn a_toggle_switched_off_removes_its_settings() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("Config.wtf");
        std::fs::write(&path, SAMPLE).unwrap();

        assert_eq!(apply(&path, &[], &["gxApi", "neverSet"]).unwrap(), 1);
        let file = ConfigWtf::read(&path).unwrap();
        assert_eq!(file.get("gxApi"), None);
        assert_eq!(file.get("locale"), Some("enGB"));
    }

    #[test]
    fn a_file_that_does_not_exist_yet_is_created() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("WTF").join("Config.wtf");
        assert_eq!(apply(&path, &[("scriptProfile", "0".to_owned())], &[]).unwrap(), 1);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "SET scriptProfile \"0\"\n");
    }
}
