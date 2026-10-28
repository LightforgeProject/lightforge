//! Release notes, compiled in.
//!
//! From `RELEASES.md` at the repository root, compiled in, so they work
//! offline:
//!
//! ```text
//! ## 0.2.0
//! A sentence or two about the release as a whole.
//!
//! - One line about one change.
//! - Another.
//!
//! ## 0.1.0
//! - ...
//! ```
//!
//! Hard-wrapped: a non-bullet line continues the bullet above, and lines
//! before the first bullet are the intro. Text before the first heading and
//! releases without bullets are dropped.

/// The notes as they are written in the repository.
pub const SOURCE: &str = include_str!("../../../RELEASES.md");

/// One version's notes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// The version exactly as the heading spells it, e.g. `0.2.0`.
    pub version: String,
    /// What the release is about, before the list of what changed.
    pub intro: Option<String>,
    /// One line per change, without its bullet.
    pub notes: Vec<String>,
}

/// Every release in the file, in the file's order (newest first).
pub fn all() -> Vec<Release> {
    parse(SOURCE)
}

/// The notes for one version, `None` for a development build.
pub fn notes_for(version: &str) -> Option<Release> {
    parse(SOURCE).into_iter().find(|r| r.version == version)
}

fn parse(source: &str) -> Vec<Release> {
    let mut releases: Vec<Release> = Vec::new();
    for line in source.lines() {
        let line = line.trim();
        if let Some(version) = line.strip_prefix("## ") {
            let version = version.trim();
            if !version.is_empty() {
                releases.push(Release { version: version.to_owned(), intro: None, notes: Vec::new() });
            }
            continue;
        }
        let Some(release) = releases.last_mut() else {
            // Anything before the first heading belongs to no release.
            continue;
        };
        // `-` and `*` both.
        match line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            Some(note) if !note.trim().is_empty() => {
                release.notes.push(note.trim().to_owned());
            }
            // A non-bullet, non-blank line continues the bullet above.
            None if !line.is_empty() => match release.notes.last_mut() {
                Some(last) => {
                    last.push(' ');
                    last.push_str(line);
                }
                // Before the first bullet: the intro, wrapped the same way.
                None => match &mut release.intro {
                    Some(intro) => {
                        intro.push(' ');
                        intro.push_str(line);
                    }
                    None => release.intro = Some(line.to_owned()),
                },
            },
            _ => {}
        }
    }
    releases.retain(|r| !r.notes.is_empty());
    releases
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shipped_file_parses_and_describes_this_build() {
        let releases = all();
        assert!(!releases.is_empty(), "RELEASES.md has no release in it");
        // A release without its own notes fails here, not on a user's machine.
        assert!(
            notes_for(env!("CARGO_PKG_VERSION")).is_some(),
            "RELEASES.md has no section for {}",
            env!("CARGO_PKG_VERSION"),
        );
    }

    #[test]
    fn a_heading_takes_the_bullets_under_it_and_nothing_else() {
        let parsed = parse(
            "# Lightforge\n\nSome prose.\n\n- a bullet before any heading\n\n\
             ## 0.2.0\n- second release, first note\n- and its second\n\n\
             ## 0.1.0\n\nProse under a heading,\nwrapped.\n* the first release\n",
        );
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].version, "0.2.0");
        assert_eq!(parsed[0].notes, vec!["second release, first note", "and its second"]);
        // Newest first, which is the order the file is written in.
        assert_eq!(parsed[1].version, "0.1.0");
        assert_eq!(parsed[1].notes, vec!["the first release"]);
        // The prose under a heading is that release's intro, and only its.
        assert_eq!(parsed[0].intro, None);
        assert_eq!(parsed[1].intro.as_deref(), Some("Prose under a heading, wrapped."));
    }

    /// A wrapped note is one note.
    #[test]
    fn a_wrapped_bullet_is_one_note() {
        let parsed = parse(
            "## 0.1.0\n- a note long enough that it had to be\n  wrapped onto a second line.\n\
             - a short one\n",
        );
        assert_eq!(
            parsed[0].notes,
            vec![
                "a note long enough that it had to be wrapped onto a second line.",
                "a short one",
            ],
        );
        // And the shipped file has none that end mid-sentence.
        for release in all() {
            for note in &release.notes {
                assert!(
                    note.ends_with('.') || note.ends_with(':'),
                    "{:?} reads as a sentence that was cut off",
                    note,
                );
            }
        }
    }

    #[test]
    fn a_release_with_nothing_under_it_is_not_a_release() {
        // An unreleased heading someone has added ahead of writing the notes.
        // Showing a popup with an empty body is worse than showing none.
        let parsed = parse("## 0.3.0\n\n## 0.2.0\n- something happened\n");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].version, "0.2.0");
    }
}
