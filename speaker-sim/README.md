# speaker-sim — application crate

The user documentation lives in the [repository README](../README.md)
([version française](../README.fr.md)). This file describes the code.

## Modules

| Module | Role |
|---|---|
| `src/main.rs` | entry point: GUI, command line, systemd hooks (`--hook-started/--hook-stopped <slot>`) |
| `src/app.rs` | `SpeakerSimApp`: orchestrates everything, no GUI (A/B switching, imports, export) |
| `src/audio_engine.rs` | AutoEq EQs, CEA-2034 (file or computed from polar data), room materials and RT60 per octave, image sources, HRTF (SOFA reader `sofar`), rendering |
| `src/dsp.rs` | FFT, minimum phase, biquads, Butterworth (SOS), zero-phase filtering, smoothing, noise |
| `src/ui/` | egui interface: room plan, response and RT60 meters, side panels, settings tabs, background worker |
| `src/config.rs` | XDG/OS paths, validated settings, room presets, runtime state |
| `src/download.rs` | pinned downloads and cache (AutoEq, Spinorama, KEMAR, AutoEq targets) |
| `src/system.rs` | PipeWire (`pw-dump`, `wpctl`), filter-chain config, systemd A/B units, hooks |
| `src/i18n.rs` | interface language: French source strings, English dictionary (`tr`, `trf`, `tr_message`) |
| `src/logging.rs` | log file in the state folder |
| `tests/` | integration tests; network, PipeWire and systemd are simulated (`tests/common`) |
| `examples/render_offline.rs` | render the four IRs without PipeWire or GUI |
| `tools/` | developer commands: regenerate `data/speakers.json`, render the icons |
| `packaging/` | icon (SVG, PNG, ICO) and bundled fonts |
| `data/` | speaker catalogue (137 entries) and default settings, embedded in the binary |

The `gui` feature (default) adds the egui interface; without it the binary keeps the
engine, the command line and the systemd hooks.

## Development

```bash
cargo run --release                                  # GUI
cargo test --workspace                               # tests (no network / PipeWire / systemd)
cargo clippy --workspace --all-targets -- -D warnings
cargo run --release --example render_offline -- "Genelec 8030C" "oratory1990/over-ear/Sennheiser HD 650" /tmp/irs
cargo run -p speaker-sim-tools -- catalog [metadata.json]         # regenerate data/speakers.json
cargo run -p speaker-sim-tools -- studio-monitors metadata.json   # add the studio monitors
cargo run -p speaker-sim-tools -- icons                           # icon.svg → icon.png, icon.ico
```

Releases are built by `.github/workflows/release.yml` (`cargo build --release`, then a
`.app`/`.dmg` on macOS).

Interface strings are written in French and translated to English by `src/i18n.rs`; add new
visible texts to its `EN` dictionary. Code comments and doc comments are in French.
