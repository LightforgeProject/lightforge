//! Resolving "the file the game asked for" into archive bytes.
//!
//! Three hops, each needing the one before it:
//!
//! ```text
//! file id ──root──▶ content key ──encoding──▶ encoding key ──index──▶ bytes
//! ```
//!
//! The build config bootstraps the chain by naming the encoding manifest with
//! *both* of its keys. Encoding is therefore the one manifest reachable
//! without encoding, and everything else, root included, resolves through it
//! afterwards.
//!
//! Both manifests are large: encoding decodes to ~190 MB on a current retail
//! install. [`Content::open`] pays that cost once and keeps the result, so the
//! caller should hold onto it rather than reopening per file.

use tact_parser::wow_root::{LocaleFlags, WowRoot};

use super::encoding::{Encoding, EncodingError};

use super::config::{BuildConfig, CKey, ConfigError};
use super::storage::{EKey, Storage, StorageError};

/// A locale the root manifest can be filtered to.
///
/// Root lists most files once per locale, so this is a memory decision as much
/// as a correctness one: keeping all of them on a current build costs hundreds
/// of megabytes of entries the client will never ask for.
///
/// Exists so callers never handle a `tact_parser` type, the boundary this
/// module is here to hold.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Locale {
    /// Every locale. Correct when the caller does not know or care which
    /// language the install is, at the cost of a much larger root.
    #[default]
    Any,
    EnUs,
    EnGb,
    DeDe,
    FrFr,
    EsEs,
    EsMx,
    ItIt,
    PtBr,
    PtPt,
    RuRu,
    KoKr,
    ZhCn,
    ZhTw,
}

impl Locale {
    fn flags(self) -> LocaleFlags {
        if self == Self::Any {
            return LocaleFlags::any_locale();
        }
        let mut f = LocaleFlags::new();
        match self {
            Self::Any => unreachable!("handled above"),
            Self::EnUs => f.set_en_us(true),
            Self::EnGb => f.set_en_gb(true),
            Self::DeDe => f.set_de_de(true),
            Self::FrFr => f.set_fr_fr(true),
            Self::EsEs => f.set_es_es(true),
            Self::EsMx => f.set_es_mx(true),
            Self::ItIt => f.set_it_it(true),
            Self::PtBr => f.set_pt_br(true),
            Self::PtPt => f.set_pt_pt(true),
            Self::RuRu => f.set_ru_ru(true),
            Self::KoKr => f.set_ko_kr(true),
            Self::ZhCn => f.set_zh_cn(true),
            Self::ZhTw => f.set_zh_tw(true),
        }
        f
    }
}

impl std::str::FromStr for Locale {
    type Err = ContentError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s.to_ascii_lowercase().replace(['-', '_'], "").as_str() {
            "any" | "all" => Self::Any,
            "enus" => Self::EnUs,
            "engb" => Self::EnGb,
            "dede" => Self::DeDe,
            "frfr" => Self::FrFr,
            "eses" => Self::EsEs,
            "esmx" => Self::EsMx,
            "itit" => Self::ItIt,
            "ptbr" => Self::PtBr,
            "ptpt" => Self::PtPt,
            "ruru" => Self::RuRu,
            "kokr" => Self::KoKr,
            "zhcn" => Self::ZhCn,
            "zhtw" => Self::ZhTw,
            _ => return Err(ContentError::UnknownLocale(s.to_owned())),
        })
    }
}

/// The manifests that describe one build's contents.
pub struct Content {
    build: BuildConfig,
    encoding: Encoding,
    root: WowRoot,
}

impl std::fmt::Debug for Content {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Content")
            .field("build_name", &self.build.build_name)
            .field("file_ids", &self.root.fid_md5.len())
            .finish_non_exhaustive()
    }
}

impl Content {
    /// Load the encoding and root manifests for `build` out of local storage.
    ///
    /// `locale` filters the root manifest as it parses. It is not a
    /// convenience: root lists most files once per locale, and keeping every
    /// one of them costs memory for entries the client will never ask for.
    pub fn open(
        storage: &mut Storage,
        build: BuildConfig,
        locale: Locale,
    ) -> Result<Self, ContentError> {
        // Encoding first, and by ekey. It is the only manifest that can be
        // found without already having encoding loaded.
        let ekey = build.encoding.ekey.ok_or(ContentError::EncodingHasNoEKey)?;
        let raw = storage.read_verified(&ekey, &build.encoding.ckey).map_err(|source| ContentError::Read {
            what: "encoding manifest",
            source,
        })?;
        let encoding = Encoding::parse(&raw)?;

        // Root is listed by content key alone, so it goes through encoding.
        let root_ekey = encoding
            .ekey_for(&build.root.ckey)
            .ok_or(ContentError::UnknownContentKey { what: "root manifest", ckey: build.root.ckey })?;
        let raw = storage.read_verified(&root_ekey, &build.root.ckey).map_err(|source| ContentError::Read {
            what: "root manifest",
            source,
        })?;
        let root = WowRoot::parse(&mut std::io::Cursor::new(raw), locale.flags())
            .map_err(|e| ContentError::Parse { what: "root manifest", detail: e.to_string() })?;

        Ok(Self { build, encoding, root })
    }

    pub fn build(&self) -> &BuildConfig {
        &self.build
    }

    /// How many file ids this build exposes for the locale it was opened with.
    pub fn file_count(&self) -> usize {
        self.root.fid_md5.len()
    }

    /// Content key -> encoding key, the lookup local storage is addressed by.
    pub fn ekey_for_ckey(&self, ckey: &CKey) -> Option<EKey> {
        self.encoding.ekey_for(ckey)
    }

    /// How many content keys the encoding manifest maps.
    pub fn encoded_count(&self) -> usize {
        self.encoding.len()
    }

    /// Encoding key -> content key, for checking downloaded bytes against the
    /// key that can actually verify them.
    pub fn ckey_for_ekey(&self, ekey: &EKey) -> Option<CKey> {
        self.encoding.ckey_for_ekey(ekey)
    }

    /// Decoded size of a file, from the encoding manifest rather than the
    /// index, the index only knows the compressed size.
    pub fn size_of(&self, ckey: &CKey) -> Option<u64> {
        self.encoding.size_of(ckey)
    }

    /// File id -> content key.
    ///
    /// A file id can be listed under several locale/content flag combinations;
    /// any of them names the same bytes for our purposes, so the first is
    /// taken. Filtering happened at parse time via the locale.
    pub fn ckey_for_fdid(&self, fdid: u32) -> Option<CKey> {
        self.root.fid_md5.get(&fdid)?.values().next().copied()
    }

    /// File id -> encoding key, both hops at once.
    pub fn ekey_for_fdid(&self, fdid: u32) -> Option<EKey> {
        self.ekey_for_ckey(&self.ckey_for_fdid(fdid)?)
    }

    /// Resolve a game-relative path such as `Interface/FrameXML/GlobalStrings.lua`.
    ///
    /// Paths are matched by a Jenkins hash of the normalized path, not by
    /// string comparison, and modern roots omit most of them, a `None` here
    /// usually means the build ships no name for that file, not that the file
    /// is absent. Prefer a file id where one is known.
    pub fn fdid_for_path(&self, path: &str) -> Option<u32> {
        self.root.get_fid(path)
    }

    /// Read a file's decoded bytes by file id.
    pub fn read_fdid(&self, storage: &mut Storage, fdid: u32) -> Result<Vec<u8>, ContentError> {
        let ckey = self.ckey_for_fdid(fdid).ok_or(ContentError::UnknownFileId(fdid))?;
        self.read_ckey(storage, &ckey)
    }

    /// Read a file's decoded bytes by content key.
    pub fn read_ckey(&self, storage: &mut Storage, ckey: &CKey) -> Result<Vec<u8>, ContentError> {
        let ekey = self
            .ekey_for_ckey(ckey)
            .ok_or(ContentError::UnknownContentKey { what: "file", ckey: *ckey })?;
        storage
            .read_verified(&ekey, ckey)
            .map_err(|source| ContentError::Read { what: "file", source })
    }

    /// Every file id in the build, ascending.
    pub fn file_ids(&self) -> impl Iterator<Item = u32> + '_ {
        self.root.fid_md5.keys().copied()
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ContentError {
    #[error("the build config names no encoding key for the encoding manifest")]
    EncodingHasNoEKey,

    #[error("no content key {} in the encoding manifest for the {what}", crate::text::hex(.ckey))]
    UnknownContentKey { what: &'static str, ckey: CKey },

    #[error("file id {0} is not in this build's root manifest")]
    UnknownFileId(u32),

    #[error("could not read the {what} from local storage")]
    Read {
        what: &'static str,
        #[source]
        source: StorageError,
    },

    #[error("could not parse the {what}: {detail}")]
    Parse { what: &'static str, detail: String },

    #[error("`{0}` is not a locale this build can be filtered to")]
    UnknownLocale(String),

    #[error(transparent)]
    Encoding(#[from] EncodingError),

    #[error(transparent)]
    Config(#[from] ConfigError),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tact::config::{parse_hash, KeyPair};
    use crate::text::hex;

    fn build_config(encoding_ekey: Option<EKey>) -> BuildConfig {
        BuildConfig {
            build_name: Some("WOW-69814patch12.1.0_Retail".into()),
            build_uid: Some("wow".into()),
            encoding: KeyPair {
                ckey: parse_hash("bdb644bbe038d17b9ad370d8a855f305").unwrap(),
                ekey: encoding_ekey,
            },
            root: KeyPair {
                ckey: parse_hash("669be7c403ea35a20f73ea7effcaa544").unwrap(),
                ekey: None,
            },
            install: None,
            download: None,
            size: None,
        }
    }

    #[test]
    fn a_build_config_without_an_encoding_ekey_cannot_bootstrap() {
        // The whole chain hangs off this one field: without it there is no way
        // to reach encoding, and so no way to resolve anything else.
        let dir = std::env::temp_dir().join("lightforge-content-noekey");
        let data = dir.join("Data").join("data");
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&data).unwrap();

        let mut index = crate::tact::idx::Index::new(0);
        index.insert(crate::tact::idx::Entry::archive_marker(0, 0));
        index.save_new_generation(&data).unwrap();

        let mut storage = Storage::open(&dir).unwrap();
        let err = Content::open(&mut storage, build_config(None), Locale::Any).unwrap_err();
        assert!(matches!(err, ContentError::EncodingHasNoEKey));
    }

    #[test]
    fn errors_name_the_manifest_and_the_key_they_failed_on() {
        // A bare hash in a log is useless; every failure in the chain has to
        // say which hop it was and which key it was looking for.
        let err = ContentError::UnknownContentKey {
            what: "root manifest",
            ckey: parse_hash("669be7c403ea35a20f73ea7effcaa544").unwrap(),
        };
        let text = err.to_string();
        assert!(text.contains("root manifest"), "{text}");
        assert!(text.contains("669be7c403ea35a20f73ea7effcaa544"), "{text}");

        assert!(ContentError::UnknownFileId(1091001).to_string().contains("1091001"));
    }

    #[test]
    fn locales_parse_in_the_spellings_people_actually_type() {
        for (text, want) in [
            ("enUS", Locale::EnUs),
            ("en_us", Locale::EnUs),
            ("en-GB", Locale::EnGb),
            ("dede", Locale::DeDe),
            ("any", Locale::Any),
        ] {
            assert_eq!(text.parse::<Locale>().unwrap(), want, "{text}");
        }
        assert!("klingon".parse::<Locale>().is_err());
        assert_eq!(Locale::default(), Locale::Any);
    }

    #[test]
    fn a_single_locale_sets_exactly_one_flag() {
        // `any_locale` is every bit set; a specific locale must not be.
        assert!(Locale::Any.flags().all());
        assert!(!Locale::EnUs.flags().all());
        assert!(Locale::EnUs.flags().any());
        assert!(Locale::EnUs.flags().en_us());
        assert!(!Locale::EnUs.flags().de_de());
    }

    #[test]
    fn hex_rendering_matches_the_config_spelling() {
        let ckey = parse_hash("669be7c403ea35a20f73ea7effcaa544").unwrap();
        assert_eq!(hex(&ckey), "669be7c403ea35a20f73ea7effcaa544");
    }
}
