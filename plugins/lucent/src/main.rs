//! Entry point for standalone mode - run the plugin as a regular
//! desktop app via `cargo truce run`, no DAW needed. Only compiled
//! when the `standalone` feature is enabled (see `[[bin]]` in
//! Cargo.toml).
//!
//! Safe to delete this file (and the `standalone` feature + bin
//! entry in Cargo.toml) if you don't want a standalone build.

use lucent::Plugin;

fn main() {
    truce_standalone::run::<Plugin>();
}
