//! The Diagnostics block's checks, off the UI thread: `ldconfig` and every
//! library's filesystem are read.

use std::path::PathBuf;

use lightforge_core::config::Config;
use lightforge_core::doctor::{self, Check};
use lightforge_core::paths::Layout;

pub async fn diagnose(config: Config, layout: Layout, proton_dir: PathBuf) -> Vec<Check> {
    tokio::task::spawn_blocking(move || doctor::run(&config, &layout, &proton_dir)).await.unwrap_or_default()
}
