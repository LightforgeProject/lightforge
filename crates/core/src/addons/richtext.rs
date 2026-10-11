//! Addon descriptions and changelogs, as blocks rather than markup.
//!
//! CurseForge publishes both as HTML fragments. The proxy reduces them to a
//! short list of blocks before they reach us, so this module holds no parser,
//! the same division of labour as [`crate::news`], and for the same reason: a
//! site changing its markup is a redeploy, not a release.
//!
//! What arrives is deliberately lossy. Bold, italics, tables and inline
//! styling are already flattened to text upstream; what survives is the
//! structure that carries meaning in a changelog, the heading, the bullet,
//! the link and the image.

use serde::{Deserialize, Serialize};

/// A span of a block's text that links somewhere.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Link {
    pub text: String,
    pub url: String,
    /// Byte offsets into the owning block's `text`.
    ///
    /// The proxy sends offsets rather than leaving us to search for `text`,
    /// because a search picks the wrong occurrence the moment one block links
    /// the same words twice, "see here" and "here", which changelogs do
    /// constantly. `start == end` means the proxy could not re-find the span
    /// and the link should be drawn as a reference after the text.
    #[serde(default)]
    pub start: usize,
    #[serde(default)]
    pub end: usize,
}

impl Link {
    /// Whether this link addresses a real span, as opposed to being a
    /// trailing reference with nowhere to sit inside the text.
    pub fn is_inline(&self) -> bool {
        self.end > self.start
    }
}

/// What kind of block this is.
///
/// `Other` is not a parse failure: the proxy may add kinds, and the contract
/// is that an unrecognised one renders its `text` as a paragraph. Keeping the
/// raw name lets that happen without a release.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Heading,
    Para,
    Bullet,
    Code,
    Image,
    #[serde(untagged)]
    Other(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    pub kind: Kind,
    #[serde(default)]
    pub text: String,
    /// Heading depth, 1-6. Only meaningful on [`Kind::Heading`].
    #[serde(default)]
    pub level: u8,
    /// Nesting depth of a list item, 0 for a top-level bullet.
    #[serde(default)]
    pub indent: u8,
    #[serde(default)]
    pub links: Vec<Link>,
    /// Image source. Only set on [`Kind::Image`].
    #[serde(default)]
    pub url: Option<String>,
}

impl Block {
    /// The links that sit inside the text, in the order they appear.
    ///
    /// Sorted and clamped, because the offsets arrive from a service: a
    /// malformed pair would otherwise panic the slicing in a renderer, and a
    /// blank card is a better failure than a crashed launcher.
    pub fn inline_links(&self) -> Vec<Link> {
        let mut links: Vec<Link> = self
            .links
            .iter()
            .filter(|l| l.is_inline() && l.end <= self.text.len())
            .filter(|l| self.text.is_char_boundary(l.start) && self.text.is_char_boundary(l.end))
            .cloned()
            .collect();
        links.sort_by_key(|l| l.start);
        // Drop any that overlap the one before, so a renderer walking them in
        // order never has to seek backwards.
        let mut cursor = 0;
        links.retain(|l| {
            let keep = l.start >= cursor;
            if keep {
                cursor = l.end;
            }
            keep
        });
        links
    }

    /// The links with nowhere to sit, to be listed after the text.
    pub fn trailing_links(&self) -> impl Iterator<Item = &Link> {
        self.links.iter().filter(|l| !l.is_inline())
    }
}

/// One fetched description or changelog.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Document {
    #[serde(default)]
    pub blocks: Vec<Block>,
    /// The document was longer than the proxy's caps. Offer the CurseForge
    /// page for the rest rather than ending mid-list.
    #[serde(default)]
    pub truncated: bool,
}

impl Document {
    pub fn is_empty(&self) -> bool {
        self.blocks.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn block(text: &str, links: Vec<Link>) -> Block {
        Block {
            kind: Kind::Para,
            text: text.to_owned(),
            level: 0,
            indent: 0,
            links,
            url: None,
        }
    }

    fn link(text: &str, start: usize, end: usize) -> Link {
        Link {
            text: text.to_owned(),
            url: "https://example.com".to_owned(),
            start,
            end,
        }
    }

    #[test]
    fn an_unknown_kind_is_kept_rather_than_failing_the_parse() {
        let doc: Document =
            serde_json::from_str(r#"{"blocks":[{"kind":"table","text":"rows"}]}"#).unwrap();
        assert_eq!(doc.blocks[0].kind, Kind::Other("table".to_owned()));
        assert_eq!(doc.blocks[0].text, "rows");
    }

    #[test]
    fn known_kinds_parse_to_their_variants() {
        let doc: Document = serde_json::from_str(
            r#"{"blocks":[{"kind":"heading","level":2,"text":"A"},{"kind":"bullet","indent":1,"text":"B"}]}"#,
        )
        .unwrap();
        assert_eq!(doc.blocks[0].kind, Kind::Heading);
        assert_eq!(doc.blocks[0].level, 2);
        assert_eq!(doc.blocks[1].kind, Kind::Bullet);
        assert_eq!(doc.blocks[1].indent, 1);
    }

    #[test]
    fn missing_fields_default_rather_than_failing() {
        let doc: Document = serde_json::from_str(r#"{"blocks":[{"kind":"para"}]}"#).unwrap();
        assert_eq!(doc.blocks[0].text, "");
        assert!(doc.blocks[0].links.is_empty());
        assert!(!doc.truncated);
    }

    // The offsets come from a service, so every way they can be wrong has to
    // be survivable: a renderer slices the text with them.
    #[test]
    fn offsets_past_the_end_of_the_text_are_dropped() {
        let b = block("short", vec![link("x", 2, 500)]);
        assert!(b.inline_links().is_empty());
    }

    #[test]
    fn offsets_inside_a_character_are_dropped() {
        // "é" is two bytes, so 1 is not a boundary.
        let b = block("é", vec![link("x", 0, 1)]);
        assert!(b.inline_links().is_empty());
    }

    #[test]
    fn overlapping_links_keep_only_the_first() {
        let b = block("one two three", vec![link("a", 0, 7), link("b", 4, 13)]);
        let got = b.inline_links();
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].start, 0);
    }

    #[test]
    fn links_come_back_in_text_order() {
        let b = block("one two", vec![link("two", 4, 7), link("one", 0, 3)]);
        let got = b.inline_links();
        assert_eq!(got.iter().map(|l| l.start).collect::<Vec<_>>(), vec![0, 4]);
    }

    #[test]
    fn a_zero_width_link_is_trailing_not_inline() {
        let b = block("no anchor here", vec![link("docs", 0, 0)]);
        assert!(b.inline_links().is_empty());
        assert_eq!(b.trailing_links().count(), 1);
    }
}
