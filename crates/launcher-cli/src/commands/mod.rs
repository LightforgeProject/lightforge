//! One function per command, grouped the way `--help` reads.

pub(crate) mod info;
pub(crate) mod proton;
pub(crate) mod battlenet;
pub(crate) mod play;
pub(crate) mod download;
pub(crate) mod addons;
pub(crate) mod storage;
pub(crate) mod inspect;

/// Clap's check on every product code argument: a custom code becomes a
/// folder name, so it is held to the same rule as one in the config.
pub(crate) fn product_code(code: &str) -> Result<String, String> {
    match lightforge_core::product::is_well_formed(code) {
        true => Ok(code.to_owned()),
        false => Err("a product code is letters, digits and `_` only, e.g. `wow_classic`".to_owned()),
    }
}
