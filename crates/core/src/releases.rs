//! Release notes, compiled in.
//!
//! The notes describe the version that is running, which the binary knows
//! better than any server does, so they ship with it rather than being
//! fetched. That also means they are there on a machine with no network, which
//! is exactly the machine someone has just updated and restarted.
//!
//! The source is `RELEASES.md` at the repository root, so it doubles as the
//! changelog a visitor reads. Its shape is the smallest thing worth parsing:
//!
//! ```text
//! ## 0.2.0
//! - One line about one change.
//! - Another.
//!
//! ## 0.1.0
//! - ...
//! ```
//!
//! The file is hard-wrapped, so a line under a bullet that is not itself a
//! bullet continues it. Anything before the first heading, a title, an
//! explanation, belongs to no release and is dropped, as is a release with
//! no bullets under it: a popup with an empty body is worse than none.

/// The notes as they are written in the repository.
pub const SOURCE: &str = include_str!("../../../RELEASES.md");

/// One version's notes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Release {
    /// The version exactly as the heading spells it, e.g. `0.2.0`.
    pub version: String,
    /// One line per change, without its bullet.
    pub notes: Vec<String>,
}

/// Every release in the file, newest first, which is the order it is written
/// in. The order is the file's, not a parsed comparison of version numbers:
/// sorting would need a semver dependency to tell `0.10.0` from `0.9.0`, and
/// the file is hand-written in order anyway.
pub fn all() -> Vec<Release> {
    parse(SOURCE)
}

/// The notes for one version, or `None` when the file says nothing about it,
/// which is the normal state of a development build between releases.
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
                releases.push(Release { version: version.to_owned(), notes: Vec::new() });
            }
            continue;
        }
        let Some(release) = releases.last_mut() else {
            // Anything before the first heading belongs to no release.
            continue;
        };
        // `-` and `*` both, because both are Markdown and the file is written
        // by hand.
        match line.strip_prefix("- ").or_else(|| line.strip_prefix("* ")) {
            Some(note) if !note.trim().is_empty() => {
                release.notes.push(note.trim().to_owned());
            }
            // A line that is not a bullet and not blank continues the bullet
            // above it. The file is wrapped at the width everything else in
            // the repository is wrapped at, so most notes are two lines; the
            // first version of this read only the first of them and cut every
            // long note off mid-sentence. The wrap is a fact about the file,
            // not about the sentence.
            None if !line.is_empty() => {
                if let Some(last) = release.notes.last_mut() {
                    last.push(' ');
                    last.push_str(line);
                }
            }
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
        // The popup exists to describe the version someone has just started.
        // A build whose own version is missing from the file would ship with
        // nothing to say, and that is a release-time mistake worth failing a
        // test over rather than discovering on a user's machine.
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
             ## 0.1.0\n\nProse under a heading.\n* the first release\n",
        );
        assert_eq!(parsed.len(), 2);
        assert_eq!(parsed[0].version, "0.2.0");
        assert_eq!(parsed[0].notes, vec!["second release, first note", "and its second"]);
        // Newest first, which is the order the file is written in.
        assert_eq!(parsed[1].version, "0.1.0");
        assert_eq!(parsed[1].notes, vec!["the first release"]);
    }

    /// The file is hard-wrapped like everything else in the repository, and a
    /// wrapped note is one note. Reading only the first line cut most of them
    /// off mid-sentence, which is what the popup showed on its first run.
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
