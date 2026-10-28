//! `.product.db`: Agent's record of what is installed where.
//!
//! Battle.net will not adopt an install without one: "Locate the game" finds
//! no `<install>/.product.db` and answers `error 2800` ("This folder doesn't
//! contain the correct version of this game").
//!
//! Protobuf wire format with no published schema; the layout was read field
//! by field from a real install, and only observed fields are written.
//!
//! ```text
//! 1  ProductInstall            (repeated: one per installed product)
//!    1  uid                    "wow_classic_era"
//!    2  product_code           "wow_classic_era"
//!    3  Settings
//!       1  install_path        "C:/Program Files (x86)/World of Warcraft"
//!       2  region              "eu"
//!       3,4,5                  0, 2, 3      (constant in every real record)
//!       6,7 locale             "enUS"
//!       8  AssetLocale { 1 locale, 2: 3 }
//!       9,10 empty messages
//!       11 account country     "AUT"
//!       12 geo country         "AT"
//!       13 flavor directory    "_classic_era_"
//!    4  CachedProductState
//!       1  BaseProductState
//!          1,2,3 installed / playable / update complete
//!          7  version          "1.15.9.69722"
//!          12,14 build key     (twice, same value)
//!          17 tags
//!          19 key ring         (absent when the product has none)
//!          21 timestamp (ms)
//!       2,3 timing stubs
//!    6  product family         "wow"
//!    7  0
//!    8  settings json          {"user_install_package_settings":[]}
//! ```

mod read;
mod wire;
mod write;
#[cfg(test)]
mod tests;

pub use read::{decode, read, Installed};
pub use write::{
    encode, graft, lift, register, splice, unregister, upsert, without, write, ProductInstall,
    Record,
};

use std::path::{Path, PathBuf};
// The tests check the bytes with the wire helpers.
#[cfg(test)]
use wire::*;

/// Agent's tag expression for a region, locale and account.
///
/// Both halves are identical but for the trailing `speech?` / `text?`.
pub fn agent_tags(region: &str, locale: &str, account: &str, geo: &str) -> String {
    let mut common = format!("Windows x86_64 {}?", region.to_uppercase());
    if !account.is_empty() {
        common.push_str(&format!(" acct-{account}?"));
    }
    if !geo.is_empty() {
        common.push_str(&format!(" geoip-{geo}?"));
    }
    common.push_str(&format!(" {locale}"));
    format!("{common} speech?:{common} text?")
}

/// The locale named in a tag expression, e.g. `enUS` from
/// `Windows x86_64 EU enUS speech:…`.
fn locale_from_tags(tags: &str) -> Option<String> {
    tags.split([' ', ':'])
        .map(|tag| tag.trim_end_matches('?'))
        .find(|tag| {
            tag.len() == 4
                && tag.is_ascii()
                && tag[..2].chars().all(|c| c.is_ascii_lowercase())
                && tag[2..].chars().all(|c| c.is_ascii_uppercase())
        })
        .map(str::to_owned)
}

#[derive(Debug, thiserror::Error)]
pub enum ProductDbError {
    #[error("`.build.info` has no {0} for this product")]
    MissingColumn(String),

    #[error("i/o error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> ProductDbError + '_ {
    move |source| ProductDbError::Io { path: path.to_path_buf(), source }
}
