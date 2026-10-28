//! TACT/NGDP/Ribbit and CASC storage.
//!
//! The boundary around `cascette-rs`: its types stay in here. `casc-storage`
//! 0.4.3 cannot write an index ([`idx`] is ours), and its `read` returns an
//! empty `Vec` for every entry of a real install.
//!
//! Networked entry points here are `async fn`; the rest of `core` stays
//! synchronous.

pub mod archive;
pub mod blte;
pub mod cdn;
pub mod compact;
pub mod config;
pub mod content;
pub mod download;
pub mod encoding;
pub mod explain;
pub mod entry;
pub mod idx;
pub mod indices;
pub mod install;
pub mod manifest;
pub mod plan;
pub mod productdb;
pub mod reclaim;
pub mod storage;
pub mod uninstall;
pub mod verify;
pub mod version;
pub mod writer;
