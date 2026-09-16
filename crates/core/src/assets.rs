//! Dynamic theming asset pipeline — **Milestone 5**.
//!
//! Hybrid manifest + per-asset ETags against Tuxcore-hosted branding:
//! `manifest.json` is revalidated with `If-None-Match` on each launch, and the
//! logo/background images it names carry their own independent ETags, so a
//! metadata-only manifest change never re-downloads unchanged art.
//!
//! Cached under [`crate::paths::AppDirs::asset_cache_dir`].
