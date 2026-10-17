//! TACT/NGDP/Ribbit and CASC storage.
//!
//! This module is the boundary around `cascette-rs`. Its types stay inside
//! here; the rest of `core` and both front-ends see Lightforge types only.
//! Two reasons, both learned from spiking the crate against a real install:
//!
//! 1. `casc-storage` 0.4.3 cannot write an index, so [`idx`] is ours. If that
//!    changes upstream, the swap happens here and nowhere else.
//! 2. `CascStorage::read` returns an empty `Vec` for every entry in a real
//!    install, its BLTE streaming reader yields nothing where the one-shot
//!    `blte::decompress_blte` returns correct data. Working around that in one
//!    place beats discovering it in five.
//!
//! Networked entry points here are `async fn`; the rest of `core` stays
//! synchronous.

pub mod archive;
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
