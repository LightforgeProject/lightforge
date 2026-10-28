//! Lightforge core.
//!
//! UI-agnostic domain logic: product models, the on-disk path layout, the
//! `.build.info` engine, prefix provisioning, Proton-GE version management,
//! TACT/CASC downloading, addon management, and the themed asset pipeline.
//!
//! Networking lives behind `async fn` in the modules that need it; everything
//! else stays synchronous and directly testable. `core` never depends on iced
//! or clap.

#![cfg_attr(test, allow(clippy::cast_possible_truncation))]

pub mod addons;
pub mod advice;
pub mod assets;
pub mod backup;
pub mod battlenet;
pub mod buildinfo;
pub mod clientlocale;
pub mod config;
pub mod debug;
pub mod discover;
pub mod error;
pub mod families;
pub(crate) mod fsutil;
pub mod library;
pub mod logging;
pub mod net;
pub mod paths;
pub mod prefix;
pub mod preflight;
pub mod product;
pub mod proton;
pub mod releases;
pub mod runtime;
pub mod setup;
mod staging;
pub mod state;
pub mod steam;
pub mod steam_shortcut;
pub mod storage;
pub mod tact;
pub mod text;
pub mod transfers;
pub mod tweaks;
pub mod update;
pub mod upgrade;

pub use error::{CoreError, Result};
pub use paths::Layout;
pub use product::{Product, ProductCode, ProductProfile};
pub use state::ActionState;
