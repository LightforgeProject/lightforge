//! Lightforge core.
//!
//! UI-agnostic domain logic: product models, the on-disk path layout, the
//! `.build.info` engine, Proton-GE version management, TACT/CASC downloading,
//! addon management, and the themed asset pipeline.
//!
//! Networking lives behind `async fn` in the modules that need it; everything
//! else stays synchronous and directly testable. `core` never depends on iced
//! or clap.

pub mod buildinfo;
pub mod config;
pub mod error;
pub mod paths;
pub mod product;
pub mod proton;
pub mod state;

// Milestone placeholders — see the module docs for what lands where.
pub mod addons;
pub mod assets;
pub mod tact;

pub use error::{CoreError, Result};
pub use paths::Layout;
pub use product::{Product, ProductCode, ProductProfile};
pub use state::ActionState;
