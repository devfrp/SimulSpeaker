
<p align="center"><img src="speaker-sim/packaging/icon.png" width="96" alt=""></p>

<h1 align="center">Speaker Sim</h1>

<p align="center"><b>Real loudspeakers, in a real room, on your headphones.</b><br>
<a href="https://devfrp.github.io/SimulSpeaker/">Website</a> ·
<a href="https://github.com/devfrp/SimulSpeaker/releases/latest">Downloads</a> ·
<a href="README.fr.md">Version française</a></p>

<p align="center"><img src="docs/assets/screenshot.png" width="860" alt="Speaker Sim main window: headphone and speaker strips, interactive room plan, response and RT60 meters, room settings"></p>

Speaker Sim turns a pair of **measured loudspeakers** into a virtual audio output.
It combines their CEA-2034 response, a **simulated room** built from real materials
and acoustic treatment, a **KEMAR head** (HRTF) and an **AutoEq correction** for your
exact headphone model. On Linux it runs live as a PipeWire filter. On Windows
and macOS it exports the filters for a system-wide convolver.

```
Applications ──► [Speaker Sim] (virtual output, becomes the default output)
                   │  4 convolutions: left/right speaker → left/right ear
                   ▼
                 your headphones (physical output)
```

## Install

| Platform | Mode | How |
|---|---|---|
| **Linux** | live virtual PipeWire output | `curl -fsSL https://devfrp.github.io/SimulSpeaker/install.sh \| bash` |
| **Windows** | export → Equalizer APO | [`SpeakerSim-Windows-x64.exe`](https://github.com/devfrp/SimulSpeaker/releases/latest/download/SpeakerSim-Windows-x64.exe) |
| **macOS** | export → any convolver | [Apple Silicon](https://github.com/devfrp/SimulSpeaker/releases/latest/download/SpeakerSim-macOS-AppleSilicon.dmg) · [Intel](https://github.com/devfrp/SimulSpeaker/releases/latest/download/SpeakerSim-macOS-Intel.dmg) |

The Linux script needs no root. It installs into `~/.local/share/devfrp-speaker-sim`
and adds the `speaker-sim` command plus a menu entry. Options (after `bash -s --`):
`--uninstall`, `--purge`, `--ref v1.0.0`, `--no-desktop`. Re-run the same command to update.
The Windows and macOS binaries are unsigned. On Windows: SmartScreen → *More info* →
*Run anyway*. On macOS: right-click → *Open* the first time.

<details>
<summary>From source</summary>

System packages (Arch / Omarchy):

```bash
sudo pacman -S --needed pipewire wireplumber libmysofa python-numpy python-scipy \
    python-requests python-pyqt6
```

Debian/Ubuntu: `sudo apt install pipewire wireplumber libmysofa1 python3-numpy
python3-scipy python3-requests python3-pyqt6`.

Or in a virtual environment (libmysofa stays a system package):

```bash
cd speaker-sim
python3 -m venv .venv && . .venv/bin/activate
pip install -r requirements.txt
python3 main.py
```

PySide6 works in place of PyQt6 (automatic fallback, see `qt_compat.py`).
</details>

## Features

- **Headphones**: search **8,850** AutoEq measurements (index pinned at commit
  `7ae0f56d`). Each correction is downloaded once, then cached.
  **Import your own**: an AutoEq / Equalizer APO EQ (ParametricEQ, GraphicEQ) used as is,
  or a raw *frequency, dB* measurement (REW, Squiglink, AutoEq CSV) corrected toward an
  AutoEq target (Harman over-ear 2018, Harman in-ear 2019, diffuse field ISO 11904-1 or GRAS KEMAR).
- **Speakers**: **130** models measured on a Klippel NFS (ASR, Erin's Audio Corner,
  via Spinorama at commit `acc757bb`), including **69 studio monitors** (Genelec, Neumann,
  Kali, Adam, JBL, Kii, Focal, Yamaha…). When Spinorama only has polar data, the CEA-2034
  curves are computed locally. The computation matches Klippel's software exactly, which
  was verified on the KEF R3.
- **Room**: dimensions, **acoustic treatment rate** (share of the walls covered with
  absorbers), bass traps, wall type (masonry / drywall), floor (tile, wood, rug, carpet),
  ceiling (plaster or acoustic cloud), furniture, rear-wall diffusion, speaker distance
  from the front wall, ear height. Six presets are included: living room, treated
  studio, very dead control room, home cinema, bedroom, bare room.
  Going further: **per-wall treatment** (front, sides, rear, ceiling), absorber type
  (5 cm foam, 10 cm wool, 20 cm broadband wool), tweeter height separate from ear height,
  an **off-centre seat** (lateral offset), up to 4th-order reflections, rooms up to
  30 × 20 × 8 m, listening distance up to 10 m, angle up to 80°, manual RT60 up to 4 s.
- **Listening**: bass and treble shelves (105 Hz / 10 kHz), left/right balance, preamp.
- **Tabs with switches**: the settings sit under the room plan in nine tabs (*Room*,
  *Treatment*, *Reflections*, *Reverberation*, *Placement*, *Headphones*, *Listening*,
  *Advanced*, *Output*), each with its own **on/off switch**. A tab that is switched off
  removes its settings and says what applies instead (room off → free field, placement
  off → standard stereo triangle, and so on); the values are kept for next time. The
  *Room* switch is the only way to remove the room (the *room amount* goes down to 5 %).
  The tab bar is always visible: the **Settings** button in the top bar only collapses
  the content, and clicking a tab expands it again. No scrolling: everything fits a
  1200 × 730 window.
- **A/B comparison**: the **A | B** switch in the top bar keeps two complete setups
  (speakers, room, placement, listening). Switching from one to the other applies it
  without dropouts; the headphones and the output stay shared.
- **Headphones tab**: number of filters and preamp of the correction, choice of **another
  measurement of the same model** (another source or rig, e.g. oratory1990 or crinacle)
  and, for an imported raw measurement, a **target** you can change at any time.
- **Seamless changes**: *Apply* starts a second instance (A/B), makes it the default
  output, waits for the streams to move over, then stops the old one. The output never
  disappears, so players never pause. In **Auto** mode every change is applied this way
  on its own.
- **States**: start/stop, auto-start at login, and **restore of the previous output**
  on stop (even if the service is stopped outside the app).
- **Interface**: an interactive **top-down room plan**. Treatment, materials and
  first-reflection rays are drawn to scale, and you can drag the listener or the speakers.
  It also shows the simulated frequency response and **RT60 meters per octave**. The
  interface is in **French or English**: switch with the FR | EN button in the top bar
  (it follows your system language by default). A full **command line** is also available.

## How it works

Five stages are baked into four minimum-phase impulse responses (speaker *s* → ear *e*):

```
IR[s][e] = HP⁻¹ · ( DF⁻¹ · ( LW · Direct[s][e] + ER · Reflections[s][e] ) + SP · Reverb[s][e] )
```

1. **Speaker**: the *Listening Window* filters the direct sound, *Early Reflections* the
   first reflections, *Sound Power* the late reverberation.
2. **Room**: absorption coefficients per octave band (125 Hz–8 kHz) for every surface.
   The RT60 is computed per band (Eyring, with air absorption), or imposed manually.
   A shoebox image-source model (order 0–4) computes the reflections, each one filtered
   by the materials of every wall it hits. The late reverberation is decorrelated
   octave-band noise with the RT60 of each band. Its level follows the critical distance,
   capped below the Schroeder frequency where the statistical model breaks down.
3. **Head**: every arrival goes through the MIT KEMAR HRTF from its true direction,
   with exact fractional delays and diffuse-field equalisation.
4. **Headphone**: the AutoEq parametric EQ becomes IIR, then a minimum-phase impulse
   response.
5. **Output**: the IRs are normalised for the worst case (the same signal on both
   speakers). A separate `pipewire -c filter-chain.conf` process runs them as a
   transient systemd user unit.

## Usage

### GUI

```bash
speaker-sim            # or: python3 speaker-sim/main.py
```

1. **01 Casque** (headphones): type a few words (“hd 650 oratory”) and pick your model,
   or **Importer…** your own measurement.
2. **02 Enceintes** (speakers): filter *Studio* / *Hi-Fi*. The tooltip shows the
   Olive score, the LFX and the measurement origin.
3. **Plan**: drag yourself (the teal head) to move closer or further, or drag a speaker
   to change the angle and the distance from the front wall.
4. **Settings tabs** (under the plan): in *Room*, pick a preset or set the dimensions and
   materials; *Treatment*, *Reflections*, *Reverberation*, *Placement* and *Listening* refine the
   rest, and each switch turns its part off.
   The RT60 meters update live; the teal zone (0.2–0.4 s) is the usual target for
   critical listening.
5. Press the **power button**. With **Auto** on, later changes apply by themselves,
   without dropouts.

### Command line

```bash
speaker-sim --search-headphones "hd 650"
speaker-sim --set-headphone "oratory1990/over-ear/Sennheiser HD 650"
speaker-sim --import-headphone my-measurement.txt --name "My headphones" --target "Harman in-ear 2019"
speaker-sim --list-speakers                    # id, studio/hifi, score
speaker-sim --set-speaker "Neumann KH 150" --start
speaker-sim --status
speaker-sim --stop                             # restores your previous output
speaker-sim --autostart on                     # or off
speaker-sim --export ~/irs                     # portable filters (WAV, true stereo, Equalizer APO)
```

### Windows / macOS (export mode)

**Exporter** (or `--export FOLDER`) writes:

- `ir_LL/LR/RL/RR.wav` and `speaker-sim-true-stereo.wav` (4 channels: LL, LR, RL, RR);
- `speaker-sim.txt` for **Equalizer APO ≥ 1.2**: copy the folder into
  `C:\Program Files\EqualizerAPO\config\` and add `Include: speaker-sim.txt` to `config.txt`.

On these systems a pure-Python SOFA reader (h5py) replaces libmysofa.

## Files

Everything runs as your user. On Linux the app only writes to:

| Location | Content |
|---|---|
| `~/.local/state/devfrp-speaker-sim/config.json` | settings |
| `~/.local/state/devfrp-speaker-sim/runtime.json` | previous output, active instance |
| `~/.local/state/devfrp-speaker-sim/filters/{a,b}/` | IRs + `filter-chain.conf` per instance |
| `~/.local/state/devfrp-speaker-sim/imports/` | imported headphone measurements/EQs |
| `~/.local/state/devfrp-speaker-sim/devfrp-speaker-sim-py-a.service` | unit (auto-start) |
| `~/.cache/devfrp-speaker-sim/` | AutoEq, Spinorama and KEMAR data (pinned commits) |

`XDG_STATE_HOME` / `XDG_CACHE_HOME` are honoured. Windows uses `%LOCALAPPDATA%\devfrp-speaker-sim`;
macOS uses `~/Library/Application Support` and `~/Library/Caches`.

**No PipeWire, WirePlumber or Omarchy configuration is modified.** The only interaction
with WirePlumber is choosing the default output (`wpctl set-default`), as the volume
menu would. **Auto-start** writes the unit into the state folder and registers it with
`systemctl --user link` + `enable`. systemd then creates symlinks in
`~/.config/systemd/user/` (its only way to know a unit at login); disabling removes them.

## Repository

```
speaker-sim/            Python application (see speaker-sim/README.md)
install.sh              Linux installer (curl | bash)
docs/                   GitHub Pages website
.github/workflows/      CI (tests), Release (.exe / .dmg), Site (Pages)
```

Publish a version with `git tag vX.Y.Z && git push origin vX.Y.Z`. GitHub Actions builds
the Windows exe and the macOS apps and smoke-tests each binary by exporting real filters.
It then creates the release with SHA-256 checksums.

Run the tests with `cd speaker-sim && python3 -m unittest discover -s tests -t .`.
They need no network, PipeWire or systemd (all are mocked).

## Troubleshooting

- **“libmysofa not found”**: install `libmysofa`, or let the app fall back to h5py
  (the installer adds it when needed).
- **The service stops right away**: `journalctl --user -u 'devfrp-speaker-sim-py*' -n 50`.
- **No sound**: `wpctl status` should show *Speaker Sim* as the default output and
  `devfrp_speaker_sim_py_a.output` (or `_b`) linked to your headphones.
- **Too quiet**: the filters keep headroom for the worst case. Raise the *Préampli*
  (preamp), and watch for clipping in the bass.
- **Offline**: only data already in the cache can be used.

## License

MIT © 2026 devfrp. Data: AutoEq (MIT, Jaakko Pasanen), Spinorama (GPL-3.0, Pierre Aubert)
with ASR / Erin's Audio Corner Klippel measurements, MIT KEMAR (Gardner & Martin, MIT Media Lab).
The data is downloaded on demand, never redistributed. Fonts: Space Grotesk, Inter,
JetBrains Mono (SIL Open Font License, see `speaker-sim/packaging/fonts`).
