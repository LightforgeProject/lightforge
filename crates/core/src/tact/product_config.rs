//! A product's config: what Blizzard's launcher knows about installing it,
//! JSON beside the build configs, named in `/versions`' `ProductConfig`.
//!
//! Read for two facts that cannot be derived from the product code: the
//! folder a flavour installs into (`wowf` uses `_forever_`) and the name of
//! its game binary.

use crate::net::Http;

use super::cdn::{CdnError, Client, Endpoint, Kind};
use super::version::Cdn;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ProductConfig {
    /// `shared_container_default_subfolder`, e.g. `_retail_`.
    pub install_dir: Option<String>,
    /// The Windows game binary's file name, e.g. `WoW.exe`.
    pub executable: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ProductConfigError {
    #[error(transparent)]
    Cdn(#[from] CdnError),
    #[error("product config {hash} is not JSON")]
    Json {
        hash: String,
        #[source]
        source: serde_json::Error,
    },
}

pub fn parse(json: &str) -> Result<ProductConfig, serde_json::Error> {
    let value: serde_json::Value = serde_json::from_str(json)?;
    let text = |pointer: &str| value.pointer(pointer).and_then(serde_json::Value::as_str);
    Ok(ProductConfig {
        // Joined onto the install root: a single plain folder name or nothing.
        install_dir: text("/all/config/shared_container_default_subfolder")
            .filter(|dir| !dir.is_empty() && *dir != ".." && !dir.contains(['/', '\\']))
            .map(str::to_owned),
        executable: text("/platform/win/config/binaries/game/relative_path")
            .and_then(|path| path.rsplit(['/', '\\']).next())
            .filter(|name| !name.is_empty())
            .map(str::to_owned),
    })
}

/// Fetch and read the product config `hash` from `cdn`'s config path.
pub async fn fetch(http: &Http, cdn: &Cdn, hash: &str) -> Result<ProductConfig, ProductConfigError> {
    let configs = Cdn { path: cdn.config_path.clone(), ..cdn.clone() };
    let bytes = Client::new(Endpoint::from_cdn(&configs))?.fetch_md5_verified(http, Kind::ProductConfig, hash).await?;
    parse(&String::from_utf8_lossy(&bytes)).map_err(|source| ProductConfigError::Json { hash: hash.to_owned(), source })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `wowf` and `wow` as the EU CDN served them on 2026-10-28.
    const WOWF: &str = include_str!("../../tests/fixtures/product-config/wowf.json");
    const WOW: &str = include_str!("../../tests/fixtures/product-config/wow.json");

    #[test]
    fn forever_installs_into_the_folder_its_config_names() {
        let config = parse(WOWF).unwrap();
        assert_eq!(config.install_dir.as_deref(), Some("_forever_"));
        assert_eq!(config.executable.as_deref(), Some("Wow.exe"));
    }

    #[test]
    fn retail_names_its_binary_with_blizzards_capitals() {
        let config = parse(WOW).unwrap();
        assert_eq!(config.install_dir.as_deref(), Some("_retail_"));
        assert_eq!(config.executable.as_deref(), Some("WoW.exe"));
    }

    #[test]
    fn a_folder_that_would_leave_the_install_is_ignored() {
        let json = r#"{"all":{"config":{"shared_container_default_subfolder":"../elsewhere"}}}"#;
        assert_eq!(parse(json).unwrap().install_dir, None);
    }
}
