<p align="center"><img src="speaker-sim/packaging/icon.png" width="96" alt=""></p>

<h1 align="center">Speaker Sim</h1>

<p align="center"><b>De vraies enceintes, dans une vraie pièce, dans votre casque.</b><br>
<a href="https://devfrp.github.io/SimulSpeaker/">Site</a> ·
<a href="https://github.com/devfrp/SimulSpeaker/releases/latest">Téléchargements</a> ·
<a href="speaker-sim/README.md">Documentation</a></p>

<p align="center"><img src="docs/assets/screenshot.png" width="760" alt="Fenêtre principale de Speaker Sim"></p>

Speaker Sim combine la réponse mesurée (CEA-2034) de **85 enceintes**, une
**pièce simulée** (sources-images + réverbération), une **tête KEMAR** (HRTF)
et la **correction AutoEq** de votre casque parmi **8 850 mesures**.

## Installation

| Plateforme | Mode | Installation |
|---|---|---|
| **Linux** | sortie PipeWire virtuelle, en direct | `curl -fsSL https://devfrp.github.io/SimulSpeaker/install.sh \| bash` |
| **Windows** | export → Equalizer APO | [`SpeakerSim-Windows-x64.exe`](https://github.com/devfrp/SimulSpeaker/releases/latest/download/SpeakerSim-Windows-x64.exe) |
| **macOS** | export → convolveur | [Apple Silicon](https://github.com/devfrp/SimulSpeaker/releases/latest/download/SpeakerSim-macOS-AppleSilicon.dmg) · [Intel](https://github.com/devfrp/SimulSpeaker/releases/latest/download/SpeakerSim-macOS-Intel.dmg) |

Le script Linux n'a pas besoin de root : il installe dans `~/.local/share/devfrp-speaker-sim`,
ajoute la commande `speaker-sim` et une entrée de menu. Options : `bash -s -- --uninstall`,
`--purge`, `--ref v1.0.0`. Les binaires Windows/macOS ne sont pas signés.

## Dépôt

```
speaker-sim/            application Python (voir son README)
install.sh              installation Linux (curl | bash)
docs/                   site GitHub Pages
.github/workflows/      CI (tests), Release (.exe / .dmg), Site (Pages)
```

Publier une version : `git tag v1.0.0 && git push origin v1.0.0` — GitHub Actions
construit l'exe et les apps macOS, les teste en exportant réellement des filtres,
puis crée la release.

## Licence

MIT © 2026 devfrp. Données : AutoEq (MIT), Spinorama (GPL-3.0, mesures ASR/Klippel),
MIT KEMAR — téléchargées à l'usage, non redistribuées.
