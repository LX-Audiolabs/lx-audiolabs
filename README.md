# LX AudioLabs — Audio Plugins

Open-source audio effect plugins in [CLAP](https://cleveraudio.org/) format, built with [truce](https://github.com/truce-audio/truce) and Rust.

## Plugins

| Plugin | Type | Status |
|--------|------|--------|
| **Equilibrium** | Spectral balance processor | Stable |
| **Meridian** | Stereo field processor | Stable |
| **Aether** | Reverb | Stable |
| **Aurum** | Saturation / distortion | Pre-production |
| **Lucent** | Visualizer | Pre-production |
| **Lucent Relay** | Visualizer companion | Pre-production |

## Download

Pre-built CLAP binaries: [lxndrbe.github.io](https://lxndrbe.github.io/plugins/)

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

- **Language:** Rust (Edition 2021)
- **Framework:** truce 3.0 + truce-vizia (Skia/OpenGL)
- **Format:** CLAP

## License

[Apache License 2.0](LICENSE) — Copyright 2024-2026 LX AudioLabs

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Bug reports welcome.
