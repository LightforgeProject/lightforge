//! `.build.info` document model.
//!
//! The file is a pipe-delimited table with a typed header row:
//!
//! ```text
//! Branch!STRING:0|Active!DEC:1|Build Key!HEX:16|CDN Key!HEX:16|Version!STRING:0|Product!STRING:0
//! us|1|abc…|def…|11.0.5.57171|wow
//! ```
//!
//! Two rules drive the design:
//!
//! 1. **Columns are located by name, never by index.** Blizzard reorders and
//!    adds columns across expansions; hardcoded offsets silently corrupt rows.
//! 2. **Only the target product's row is ever mutated.** Retail, Classic and
//!    PTR share one file, so a whole-file rewrite would clobber siblings.
//!
//! Milestone 1 provides the parser and the in-memory model. The atomic
//! temp-file-and-rename writer lands in Milestone 3.

use std::collections::HashMap;

use crate::product::ProductCode;

/// Column whose value identifies the product a row belongs to.
const COL_PRODUCT: &str = "Product";
/// Columns the update engine compares against Ribbit.
pub const COL_BUILD_KEY: &str = "Build Key";
pub const COL_CDN_KEY: &str = "CDN Key";
pub const COL_VERSION: &str = "Version";

/// A parsed `.build.info`, preserving column order and every untouched row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildInfo {
    /// Header cells verbatim, e.g. `Build Key!HEX:16`, in file order.
    headers: Vec<String>,
    /// Bare column name -> index, e.g. `Build Key` -> 2.
    index: HashMap<String, usize>,
    rows: Vec<Row>,
}

/// One product's row, stored as raw cells aligned to [`BuildInfo::headers`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    cells: Vec<String>,
}

impl BuildInfo {
    pub fn parse(contents: &str) -> Result<Self, BuildInfoError> {
        let mut lines = contents.lines().filter(|l| !l.trim().is_empty());
        let header_line = lines.next().ok_or(BuildInfoError::Empty)?;

        let headers: Vec<String> = header_line.split('|').map(str::to_owned).collect();
        let mut index = HashMap::with_capacity(headers.len());
        for (i, header) in headers.iter().enumerate() {
            // `Build Key!HEX:16` -> `Build Key`. A column with no `!` type
            // suffix is still addressable by its full text.
            let name = header.split('!').next().unwrap_or(header).trim();
            index.insert(name.to_owned(), i);
        }

        if !index.contains_key(COL_PRODUCT) {
            return Err(BuildInfoError::MissingColumn(COL_PRODUCT.to_owned()));
        }

        let rows = lines
            .map(|line| {
                let cells: Vec<String> = line.split('|').map(str::to_owned).collect();
                if cells.len() != headers.len() {
                    return Err(BuildInfoError::ColumnCountMismatch {
                        expected: headers.len(),
                        found: cells.len(),
                    });
                }
                Ok(Row { cells })
            })
            .collect::<Result<_, _>>()?;

        Ok(Self { headers, index, rows })
    }

    /// Index of a column by bare name.
    pub fn column(&self, name: &str) -> Result<usize, BuildInfoError> {
        self.index.get(name).copied().ok_or_else(|| BuildInfoError::MissingColumn(name.to_owned()))
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// The row belonging to `code`, if the product is installed.
    pub fn row(&self, code: &ProductCode) -> Option<&Row> {
        let product_col = self.index[COL_PRODUCT];
        self.rows.iter().find(|r| r.cells.get(product_col).map(String::as_str) == Some(code.as_str()))
    }

    /// Mutable access to a single product's row. Every other row is untouched
    /// by construction — this is the row-isolation guarantee.
    pub fn row_mut(&mut self, code: &ProductCode) -> Result<&mut Row, BuildInfoError> {
        let product_col = self.index[COL_PRODUCT];
        self.rows
            .iter_mut()
            .find(|r| r.cells.get(product_col).map(String::as_str) == Some(code.as_str()))
            .ok_or_else(|| BuildInfoError::UnknownProduct(code.as_str().to_owned()))
    }

    /// Read one cell of a product's row by column name.
    pub fn get(&self, code: &ProductCode, column: &str) -> Result<&str, BuildInfoError> {
        let col = self.column(column)?;
        let row = self
            .row(code)
            .ok_or_else(|| BuildInfoError::UnknownProduct(code.as_str().to_owned()))?;
        Ok(row.cells[col].as_str())
    }

    /// Write one cell of a product's row by column name.
    pub fn set(&mut self, code: &ProductCode, column: &str, value: impl Into<String>) -> Result<(), BuildInfoError> {
        let col = self.column(column)?;
        self.row_mut(code)?.cells[col] = value.into();
        Ok(())
    }

    /// Serialize back to the on-disk format, preserving column order.
    pub fn to_file_contents(&self) -> String {
        let mut out = String::new();
        out.push_str(&self.headers.join("|"));
        out.push('\n');
        for row in &self.rows {
            out.push_str(&row.cells.join("|"));
            out.push('\n');
        }
        out
    }
}

impl Row {
    pub fn cells(&self) -> &[String] {
        &self.cells
    }
}

#[derive(Debug, thiserror::Error)]
pub enum BuildInfoError {
    #[error(".build.info is empty or has no header row")]
    Empty,

    #[error(".build.info has no `{0}` column")]
    MissingColumn(String),

    #[error(".build.info row has {found} columns, expected {expected}")]
    ColumnCountMismatch { expected: usize, found: usize },

    #[error("`{0}` has no row in .build.info")]
    UnknownProduct(String),
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A real single-product `.build.info` from a live 12.1.0 retail install.
    ///
    /// Account and geoip tags are neutralized; everything else — column set,
    /// `!TYPE:N` suffixes, empty `HEX:16`/`DEC:4` cells, the `?`/`:` tag
    /// grammar, trailing newline — is verbatim, because that is exactly the
    /// shape hand-written fixtures get wrong.
    const REAL_RETAIL: &str = concat!(
        "Branch!STRING:0|Active!DEC:1|Build Key!HEX:16|CDN Key!HEX:16|Install Key!HEX:16|",
        "IM Size!DEC:4|CDN Path!STRING:0|CDN Hosts!STRING:0|CDN Servers!STRING:0|Tags!STRING:0|",
        "Armadillo!STRING:0|Last Activated!STRING:0|Version!STRING:0|KeyRing!HEX:16|Product!STRING:0\n",
        "eu|1|9712ae9c84cc54111e9e3ddf9a6b8019|904e68a1fd418798b47ec3f9dba8da38|||tpr/wow|",
        "level3.blizzard.com|http://level3.blizzard.com/?maxhosts=8 ",
        "https://level3.ssl.blizzard.com/?maxhosts=8&fallback=1|",
        "Windows x86_64 EU? acct-XXX? geoip-XX? enUS speech?:",
        "Windows x86_64 EU? acct-XXX? geoip-XX? enUS text?|||12.1.0.69814|",
        "3ca57fe7319a297346440e4d2a03a0cd|wow\n",
    );

    #[test]
    fn parses_a_real_retail_build_info() {
        let info = BuildInfo::parse(REAL_RETAIL).unwrap();
        assert_eq!(info.get(&ProductCode::Retail, "Version").unwrap(), "12.1.0.69814");
        assert_eq!(
            info.get(&ProductCode::Retail, "Build Key").unwrap(),
            "9712ae9c84cc54111e9e3ddf9a6b8019"
        );
        assert_eq!(info.get(&ProductCode::Retail, "CDN Path").unwrap(), "tpr/wow");
        // Empty typed cells are legal and must survive as empty strings.
        assert_eq!(info.get(&ProductCode::Retail, "Install Key").unwrap(), "");
        assert_eq!(info.get(&ProductCode::Retail, "IM Size").unwrap(), "");
    }

    #[test]
    fn a_real_file_round_trips_byte_for_byte() {
        // Agent re-scans anything it doesn't recognize, so rewriting a file
        // Lightforge only read must change nothing at all.
        let info = BuildInfo::parse(REAL_RETAIL).unwrap();
        assert_eq!(info.to_file_contents(), REAL_RETAIL);
    }

    #[test]
    fn mutating_one_cell_leaves_every_other_byte_alone() {
        let mut info = BuildInfo::parse(REAL_RETAIL).unwrap();
        info.set(&ProductCode::Retail, "Version", "12.1.5.70000").unwrap();
        let written = info.to_file_contents();

        assert_eq!(written.replace("12.1.5.70000", "12.1.0.69814"), REAL_RETAIL);
    }

    const SAMPLE: &str = "Branch!STRING:0|Active!DEC:1|Build Key!HEX:16|CDN Key!HEX:16|Version!STRING:0|Product!STRING:0\n\
us|1|aaaa|bbbb|11.0.5.57171|wow\n\
us|1|cccc|dddd|1.15.5.57638|wow_classic\n";

    #[test]
    fn columns_resolve_by_name_not_position() {
        let bi = BuildInfo::parse(SAMPLE).unwrap();
        assert_eq!(bi.column("Build Key").unwrap(), 2);
        assert_eq!(bi.column(COL_VERSION).unwrap(), 4);

        // A reordered schema must still resolve correctly.
        let reordered = "Product!STRING:0|Version!STRING:0|Build Key!HEX:16\nwow|1.2.3|ffff\n";
        let bi = BuildInfo::parse(reordered).unwrap();
        assert_eq!(bi.get(&ProductCode::Retail, "Build Key").unwrap(), "ffff");
    }

    #[test]
    fn mutating_one_product_leaves_siblings_untouched() {
        let mut bi = BuildInfo::parse(SAMPLE).unwrap();
        bi.set(&ProductCode::Retail, COL_BUILD_KEY, "9999").unwrap();

        assert_eq!(bi.get(&ProductCode::Retail, COL_BUILD_KEY).unwrap(), "9999");
        assert_eq!(bi.get(&ProductCode::Classic, COL_BUILD_KEY).unwrap(), "cccc");
        assert_eq!(bi.get(&ProductCode::Classic, COL_VERSION).unwrap(), "1.15.5.57638");
    }

    #[test]
    fn round_trips_unchanged() {
        let bi = BuildInfo::parse(SAMPLE).unwrap();
        assert_eq!(bi.to_file_contents(), SAMPLE);
    }

    #[test]
    fn rejects_ragged_rows() {
        let ragged = "Product!STRING:0|Version!STRING:0\nwow|1.2.3|extra\n";
        assert!(matches!(
            BuildInfo::parse(ragged),
            Err(BuildInfoError::ColumnCountMismatch { expected: 2, found: 3 })
        ));
    }

    #[test]
    fn missing_product_row_is_an_error_not_a_panic() {
        let mut bi = BuildInfo::parse(SAMPLE).unwrap();
        assert!(matches!(
            bi.set(&ProductCode::Ptr, COL_BUILD_KEY, "x"),
            Err(BuildInfoError::UnknownProduct(_))
        ));
    }
}
