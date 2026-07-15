# LX AudioLabs — CLAP Audio Plugins

Open-source audio effect plugins in [CLAP](https://cleveraudio.org/) format, built with [truce](https://github.com/LX-Audiolabs/truce) and Rust.

## Plugins

| Plugin | Type | Role | Status |
|--------|------|------|--------|
| **Equilibrium** | 5-Band Spectral Balancer | Master Bus — precision band correction | Stable |
| **Meridian** | Group Track Sculptor | Tracks & Buses — EQ, compressor, saturation | Stable |
| **Aether** | Headphone Correction | Monitoring FX — Harman target curve + crossover | Stable |

## Download

Pre-built CLAP binaries: [lx-audiolabs.github.io](https://lx-audiolabs.github.io/plugins/)

## Build from Source

### Prerequisites

Install [Rust](https://rustup.rs/) first (`rustup` ships `cargo` and the toolchain this repo expects). On Windows, use the MSVC toolchain and have the Visual Studio C++ build tools available.

```bash
# Linux / macOS
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Windows: download and run rustup-init.exe from https://rustup.rs/
rustc --version
cargo --version
```

### Build

This workspace pins [LX-Audiolabs/truce](https://github.com/LX-Audiolabs/truce) on branch `main` (suite patches + truce-vizia ↔ [vizia-audio](https://github.com/LX-Audiolabs/vizia-audio)). Use that fork — not upstream `truce-audio/truce`.

```bash
# Install cargo-truce from the same fork
cargo install --git https://github.com/LX-Audiolabs/truce --branch main cargo-truce

# Build all plugins (CLAP)
cargo truce build --clap

# Build a specific plugin
cargo truce build --clap -p equilibrium

# VST3 — no extra dependencies, truce bundles the SDK
cargo truce build --vst3
cargo truce build --vst3 -p equilibrium

# LV2 — no extra dependencies, truce bundles the SDK
cargo truce build --lv2
cargo truce build --lv2 -p equilibrium
```

Output: `target/bundles/<PluginName>.clap`, `.vst3`, or `.lv2`

### CLAP validation

`cargo truce validate` runs [clap-validator](https://github.com/LX-Audiolabs/clap-validator) against installed `.clap` bundles. Use our maintained fork (not upstream `free-audio/clap-validator`) — it includes fixes and extra lifecycle tests relevant to these plugins.

Requires Rust (same `rustup` setup as above). Install the validator once:

```bash
cargo install --git https://github.com/LX-Audiolabs/clap-validator --locked
clap-validator --version
```

Then build, install, and validate a plugin:

```bash
cargo truce build --clap -p equilibrium
cargo truce install --clap -p equilibrium
cargo truce validate --clap -p equilibrium
```

`cargo truce doctor` reports whether `clap-validator` is on `PATH` or in `~/.cargo/bin`. To point at a custom binary: `CLAP_VALIDATOR=/path/to/clap-validator`.

## Tech Stack

- **Language:** Rust (Edition 2024)
- **Framework:** [LX-Audiolabs/truce](https://github.com/LX-Audiolabs/truce) (`main`) + truce-vizia
- **GUI:** [LX-Audiolabs/vizia-audio](https://github.com/LX-Audiolabs/vizia-audio) (Skia/OpenGL, baseview)
- **Formats:** CLAP, VST3, LV2
- **Validator:** [LX-Audiolabs/clap-validator](https://github.com/LX-Audiolabs/clap-validator)

## License

[GNU General Public License v3.0](LICENSE) — Copyright 2024–2026 LX AudioLabs

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Bug reports welcome.
