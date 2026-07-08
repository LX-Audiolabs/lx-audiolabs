# LX AudioLabs — CLAP Audio Plugins

Open-source audio effect plugins in [CLAP](https://cleveraudio.org/) format, built with [truce](https://github.com/truce-audio/truce) and Rust.

## Plugins

| Plugin | Type | Role | Status |
|--------|------|------|--------|
| **Equilibrium** | 5-Band Spectral Balancer | Master Bus — precision band correction | Stable |
| **Meridian** | Group Track Sculptor | Tracks & Buses — EQ, compressor, saturation | Stable |
| **Aether** | Headphone Correction | Monitoring FX — Harman target curve + crossover | Stable |

## Download

Pre-built CLAP binaries: [lx-audiolabs.github.io](https://lx-audiolabs.github.io/plugins/)

## Build from Source

```bash
# Install cargo-truce
cargo install --git https://github.com/truce-audio/truce cargo-truce

# Build all plugins
cargo truce build --clap

# Build a specific plugin
cargo truce build --clap -p equilibrium
```

Output: `target/bundles/<PluginName>.clap`

## Tech Stack

- **Language:** Rust (Edition 2024)
- **Framework:** truce 3.1.2 + truce-vizia (Skia/OpenGL)
- **Format:** CLAP

## License

[GNU General Public License v3.0 or later](LICENSE) — Copyright 2024–2026 LX AudioLabs

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Bug reports welcome.
