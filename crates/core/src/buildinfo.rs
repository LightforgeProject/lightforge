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
//! Writes are atomic: Agent memory-maps this file, and a torn write triggers
//! its repair scan. Whether Agent is running is [`crate::preflight`]'s check.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::product::ProductCode;

/// Column whose value identifies the product a row belongs to.
pub const COL_PRODUCT: &str = "Product";
/// Columns the update engine compares against Ribbit.
pub const COL_BUILD_KEY: &str = "Build Key";
pub const COL_CDN_KEY: &str = "CDN Key";
pub const COL_VERSION: &str = "Version";

/// The header line of a real retail `.build.info`, used for fresh installs.
///
/// Verbatim from `/mnt/games/battle-net/World of Warcraft`, 12.1.0.69814.
pub const DEFAULT_SCHEMA: &str = concat!(
    "Branch!STRING:0|Active!DEC:1|Build Key!HEX:16|CDN Key!HEX:16|Install Key!HEX:16|",
    "IM Size!DEC:4|CDN Path!STRING:0|CDN Hosts!STRING:0|CDN Servers!STRING:0|Tags!STRING:0|",
    "Armadillo!STRING:0|Last Activated!STRING:0|Version!STRING:0|KeyRing!HEX:16|Product!STRING:0\n"
);

/// A parsed `.build.info`, preserving column order and every untouched row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildInfo {
    /// Header cells verbatim, e.g. `Build Key!HEX:16`, in file order.
    headers: Vec<String>,
    /// Bare column name -> index, e.g. `Build Key` -> 2.
    index: HashMap<String, usize>,
    rows: Vec<Row>,
}

/// One product's row, stored as raw cells aligned to the header row.
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

    /// Every product this file has a row for, by the `Product` column.
    pub fn products(&self) -> impl Iterator<Item = ProductCode> + '_ {
        self.rows().iter().filter_map(|row| {
            self.column(COL_PRODUCT)
                .ok()
                .and_then(|index| row.cells.get(index))
                .filter(|code| !code.is_empty())
                .map(|code| code.parse().expect("product codes are infallible"))
        })
    }

    /// The row belonging to `code`, if the product is installed.
    pub fn row(&self, code: &ProductCode) -> Option<&Row> {
        let product_col = self.index[COL_PRODUCT];
        self.rows.iter().find(|r| r.cells.get(product_col).map(String::as_str) == Some(code.as_str()))
    }

    /// Mutable access to one product's row; the others cannot be touched.
    pub fn row_mut(&mut self, code: &ProductCode) -> Result<&mut Row, BuildInfoError> {
        let product_col = self.index[COL_PRODUCT];
        self.rows
            .iter_mut()
            .find(|r| r.cells.get(product_col).map(String::as_str) == Some(code.as_str()))
            .ok_or_else(|| BuildInfoError::UnknownProduct(code.as_str().to_owned()))
    }

    /// Drop a product's row, keeping the rest. Returns whether one was there.
    pub fn remove_product(&mut self, code: &ProductCode) -> bool {
        let Ok(product_col) = self.column(COL_PRODUCT) else {
            return false;
        };
        let before = self.rows.len();
        self.rows
            .retain(|row| row.cells.get(product_col).map(String::as_str) != Some(code.as_str()));
        self.rows.len() != before
    }

    /// Whether any product rows remain.
    pub fn is_empty(&self) -> bool {
        self.rows.is_empty()
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

    /// Read and parse a `.build.info` from disk.
    pub fn load(path: impl AsRef<Path>) -> Result<Self, BuildInfoError> {
        let path = path.as_ref();
        let contents = std::fs::read_to_string(path).map_err(io(path))?;
        Self::parse(&contents)
    }

    /// Atomically replace `path` with this document, keeping its mode.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), BuildInfoError> {
        let path = path.as_ref();
        crate::fsutil::write_atomic(path, self.to_file_contents().as_bytes()).map_err(io(path))
    }

    /// Load, mutate, and atomically write back in one call.
    ///
    /// No lock: pre-flight guarantees nobody else is writing.
    pub fn update<E>(
        path: impl AsRef<Path>,
        edit: impl FnOnce(&mut Self) -> Result<(), E>,
    ) -> Result<Self, E>
    where
        E: From<BuildInfoError>,
    {
        let path = path.as_ref();
        let mut info = Self::load(path)?;
        edit(&mut info)?;
        info.save(path)?;
        Ok(info)
    }

    /// Point a product's row at a new build, the three cells Agent compares
    /// against the CDN.
    ///
    /// All three or none: a `Version` disagreeing with its `Build Key` makes
    /// Agent call the install damaged.
    pub fn set_build(
        &mut self,
        code: &ProductCode,
        build_key: &str,
        cdn_key: &str,
        version: &str,
    ) -> Result<(), BuildInfoError> {
        // Resolve every column before mutating, so a schema missing one of
        // them leaves the row exactly as it was.
        let cols = [
            (self.column(COL_BUILD_KEY)?, build_key),
            (self.column(COL_CDN_KEY)?, cdn_key),
            (self.column(COL_VERSION)?, version),
        ];
        let row = self.row_mut(code)?;
        for (col, value) in cols {
            row.cells[col] = value.to_owned();
        }
        Ok(())
    }

    /// An empty `.build.info` with the column schema Agent writes.
    ///
    /// For a fresh install; the schema is copied from a real retail one.
    pub fn with_default_schema() -> Self {
        Self::parse(DEFAULT_SCHEMA).expect("the pinned schema parses")
    }

    /// Add or replace a product's row, taking cells by column name.
    ///
    /// Unnamed columns stay empty, as Agent leaves them on a fresh install.
    /// An unknown column is an error.
    pub fn set_product(
        &mut self,
        code: &ProductCode,
        values: &[(&str, &str)],
    ) -> Result<(), BuildInfoError> {
        // Resolve every column first, for the same reason `set_build` does:
        // a half-written row is worse than an unwritten one.
        let mut resolved = Vec::with_capacity(values.len() + 1);
        for (column, value) in values {
            resolved.push((self.column(column)?, (*value).to_owned()));
        }
        resolved.push((self.column(COL_PRODUCT)?, code.as_str().to_owned()));

        let mut cells = vec![String::new(); self.headers.len()];
        for (column, value) in resolved {
            cells[column] = value;
        }

        match self.rows.iter_mut().find(|r| {
            self.index
                .get(COL_PRODUCT)
                .and_then(|i| r.cells.get(*i))
                .is_some_and(|p| p == code.as_str())
        }) {
            Some(row) => *row = Row { cells },
            None => self.rows.push(Row { cells }),
        }
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

    #[error("i/o error at {path}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
}

fn io(path: &Path) -> impl FnOnce(std::io::Error) -> BuildInfoError + '_ {
    move |source| BuildInfoError::Io { path: path.to_path_buf(), source }
}


#[cfg(test)]
mod tests {
    use super::*;

    /// A real single-product `.build.info` from a live 12.1.0 retail install.
    ///
    /// Account and geoip tags neutralized; everything else verbatim.
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

    #[test]
    fn a_fresh_build_info_gets_the_schema_agent_writes() {
        let info = BuildInfo::with_default_schema();
        assert!(info.rows().is_empty());

        // Every column the update engine touches has to resolve by name.
        for column in [COL_BUILD_KEY, COL_CDN_KEY, COL_VERSION, "Tags", "Branch", "Product"] {
            assert!(info.column(column).is_ok(), "{column} is missing");
        }
    }

    #[test]
    fn a_product_row_can_be_created_then_replaced_in_place() {
        let code: ProductCode = "wow".parse().unwrap();
        let mut info = BuildInfo::with_default_schema();

        info.set_product(
            &code,
            &[
                ("Branch", "eu"),
                ("Active", "1"),
                (COL_BUILD_KEY, "9712ae9c84cc54111e9e3ddf9a6b8019"),
                (COL_CDN_KEY, "ae86d19f4ff37f2239363425d9289417"),
                ("CDN Path", "tpr/wow"),
                (COL_VERSION, "12.1.0.69814"),
            ],
        )
        .unwrap();

        assert_eq!(info.rows().len(), 1);
        assert_eq!(info.get(&code, COL_VERSION).unwrap(), "12.1.0.69814");
        // Columns Agent leaves blank on a fresh install stay blank.
        assert_eq!(info.get(&code, "Install Key").unwrap(), "");
        assert_eq!(info.get(&code, "Product").unwrap(), "wow");

        // A second call replaces the row instead of appending a duplicate.
        info.set_product(&code, &[(COL_VERSION, "12.1.5.70000")]).unwrap();
        assert_eq!(info.rows().len(), 1);
        assert_eq!(info.get(&code, COL_VERSION).unwrap(), "12.1.5.70000");
    }

    #[test]
    fn a_row_naming_a_column_the_schema_lacks_is_refused_whole() {
        let code: ProductCode = "wow".parse().unwrap();
        let mut info = BuildInfo::with_default_schema();

        assert!(info
            .set_product(&code, &[(COL_VERSION, "12.1.0"), ("Nonexistent", "x")])
            .is_err());
        // Nothing was written: a partial row is worse than no row.
        assert!(info.rows().is_empty());
    }

    #[test]
    fn a_fresh_row_round_trips_through_the_file_format() {
        let code: ProductCode = "wow".parse().unwrap();
        let mut info = BuildInfo::with_default_schema();
        info.set_product(&code, &[("Branch", "eu"), (COL_VERSION, "12.1.0.69814")]).unwrap();

        let text = info.to_file_contents();
        let reparsed = BuildInfo::parse(&text).unwrap();
        assert_eq!(reparsed, info);
        assert_eq!(text.lines().count(), 2);
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

    fn scratch(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("lightforge-buildinfo-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn a_saved_file_reloads_identically_and_leaves_no_temp_files() {
        let dir = scratch("roundtrip");
        let path = dir.join(".build.info");
        std::fs::write(&path, REAL_RETAIL).unwrap();

        let mut info = BuildInfo::load(&path).unwrap();
        info.set(&ProductCode::Retail, COL_VERSION, "12.1.5.70000").unwrap();
        info.save(&path).unwrap();

        assert_eq!(BuildInfo::load(&path).unwrap(), info);
        // A stray `.tmpXXXX` next to `.build.info` is something Agent notices.
        let left: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name()).collect();
        assert_eq!(left.len(), 1, "{left:?}");
    }

    #[test]
    fn saving_preserves_the_original_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let dir = scratch("perms");
        let path = dir.join(".build.info");
        std::fs::write(&path, REAL_RETAIL).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();

        BuildInfo::load(&path).unwrap().save(&path).unwrap();

        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o644, "tempfile's 0600 must not be inherited");
    }

    #[test]
    fn a_failed_edit_leaves_the_file_exactly_as_it_was() {
        let dir = scratch("failed-edit");
        let path = dir.join(".build.info");
        std::fs::write(&path, REAL_RETAIL).unwrap();

        let result = BuildInfo::update(&path, |info| {
            info.set(&ProductCode::Retail, COL_VERSION, "12.1.5.70000")?;
            // A later step fails: nothing may reach disk.
            info.set(&ProductCode::Classic, COL_BUILD_KEY, "dead")
        });

        assert!(matches!(result, Err(BuildInfoError::UnknownProduct(_))));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), REAL_RETAIL);
    }

    #[test]
    fn set_build_writes_all_three_cells_or_none() {
        let mut bi = BuildInfo::parse(SAMPLE).unwrap();
        bi.set_build(&ProductCode::Retail, "1111", "2222", "11.9.9.99999").unwrap();
        assert_eq!(bi.get(&ProductCode::Retail, COL_BUILD_KEY).unwrap(), "1111");
        assert_eq!(bi.get(&ProductCode::Retail, COL_CDN_KEY).unwrap(), "2222");
        assert_eq!(bi.get(&ProductCode::Retail, COL_VERSION).unwrap(), "11.9.9.99999");
        assert_eq!(bi.get(&ProductCode::Classic, COL_BUILD_KEY).unwrap(), "cccc");

        // A schema without a CDN Key column must not leave a half-updated row:
        // a Version that disagrees with its Build Key is what makes Agent
        // decide the install is damaged.
        let partial = "Product!STRING:0|Version!STRING:0|Build Key!HEX:16\nwow|1.2.3|ffff\n";
        let mut bi = BuildInfo::parse(partial).unwrap();
        assert!(matches!(
            bi.set_build(&ProductCode::Retail, "1111", "2222", "9.9.9"),
            Err(BuildInfoError::MissingColumn(_))
        ));
        assert_eq!(bi.to_file_contents(), partial);
    }

    #[test]
    fn a_missing_file_names_itself() {
        let err = BuildInfo::load("/nonexistent/games/.build.info").unwrap_err();
        assert!(err.to_string().contains("/nonexistent/games/.build.info"));
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
