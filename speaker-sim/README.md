# speaker-sim — application package

The user documentation lives in the [repository README](../README.md)
([version française](../README.fr.md)). This file describes the code.

## Modules

| Module | Role |
|---|---|
| `main.py` | entry point: GUI, command line, systemd hooks (`--hook-started/--hook-stopped <slot>`) |
| `app.py` | `SpeakerSimApp`: orchestrates everything, no Qt (A/B switching, imports, export) |
| `audio_engine.py` | DSP: AutoEq EQs, CEA-2034 (file or computed from polar data), minimum phase, room materials and RT60 per octave, image sources, HRTF (libmysofa or h5py), rendering |
| `ui_panel.py` | Qt 6 interface: room plan, response and RT60 meters, side panels, background workers |
| `config.py` | XDG/OS paths, validated settings, room presets, runtime state |
| `download.py` | pinned downloads and cache (AutoEq, Spinorama, KEMAR, AutoEq targets) |
| `system.py` | PipeWire (`pw-dump`, `wpctl`), filter-chain config, systemd A/B units, hooks |
| `qt_compat.py` | PyQt6, or PySide6 as a fallback |
| `packaging/` | icon, PyInstaller spec (Windows exe, macOS app), bundled fonts |
| `tools/` | developer scripts that regenerate `data/speakers.json` |
| `data/` | speaker catalogue (130 entries) and default settings |

## Development

```bash
python3 main.py                                   # GUI
python3 -m unittest discover -s tests -t .        # tests (no network / PipeWire / systemd)
pyinstaller --noconfirm packaging/speaker-sim.spec  # standalone binary
```

The interface strings are in French; code comments and docstrings are in French too.
