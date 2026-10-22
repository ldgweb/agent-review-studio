// Library crate for Agent Review Studio.
//
// The native core lives here so it can be shared by both binaries:
//   - src/main.rs        the Tauri desktop shell
//   - src/bin/ars.rs     the headless `ars` CLI
//
// `app_core` (not `core`) avoids colliding with Rust's built-in `core` crate
// inside macro expansions.
#[path = "core/mod.rs"]
pub mod app_core;

pub mod cli;
