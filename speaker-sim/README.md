# Speaker Sim (devfrp)

Simulation de haut-parleurs réels dans une pièce, au casque, sous Linux/PipeWire.

Le son d'un haut-parleur mesuré (CEA-2034, base Spinorama), placé dans une pièce
virtuelle et entendu à travers une tête KEMAR (HRTF via libmysofa), est
restitué sur votre casque, dont la réponse est corrigée par AutoEq. Le tout
tourne comme un filtre PipeWire : une sortie audio virtuelle « Speaker Sim ».

```
Applications ──► [Speaker Sim] (sortie virtuelle, devient la sortie par défaut)
                   │  4 convolutions : enceinte G/D → oreille G/D
                   ▼
                 votre casque (sortie physique)
```

## Fonctionnalités

- **Écouteurs** : recherche parmi les **8 850** mesures AutoEq (index figé au
  commit `7ae0f56d`) ; la correction est téléchargée une seule fois puis mise en cache.
- **Haut-parleurs** : **85** modèles mesurés par ASR sur Klippel NFS (Spinorama,
  commit `acc757bb`), sélectionnés par score de préférence (≤ 4 par marque).
- **Filtrage** :
  - son direct filtré par la *Listening Window*, réflexions précoces par
    les *Early Reflections*, réverbération par la *Sound Power* (FIR à phase minimale) ;
  - pièce parallélépipédique (sources-images d'ordre 0–3, RT60 réglable,
    niveau de réverbération déduit de la distance critique) ;
  - HRTF MIT KEMAR (libmysofa, interpolation et rééchantillonnage natifs)
    avec égalisation champ diffus ;
  - correction du casque AutoEq (EQ paramétrique → IIR → réponse impulsionnelle,
    phase minimale) ;
  - normalisation automatique anti-écrêtage (pire cas : signal mono).
- **États** : démarrage/arrêt, auto-démarrage à la connexion, bascule
  automatique de la sortie par défaut et **restauration de la sortie précédente**
  à l'arrêt (même si le service est arrêté hors de l'application).
- **Changements sans coupure** : « Appliquer » démarre une seconde instance (A/B),
  en fait la sortie par défaut, attend que les flux l'aient rejointe puis arrête
  l'ancienne ; la sortie virtuelle ne disparaît jamais, les lecteurs ne se mettent
  pas en pause.
- **Import de vos mesures de casque** : EQ AutoEq / Equalizer APO (ParametricEQ,
  GraphicEQ) appliqué tel quel, ou mesure brute « fréquence, dB » (REW, Squiglink,
  CSV AutoEq) corrigée vers une cible AutoEq au choix (Harman over-ear 2018,
  Harman in-ear 2019, champ diffus ISO 11904-1 ou GRAS KEMAR).
- **Interface** Qt 6 sombre, courbes de réponse, et une **ligne de commande** complète.

## Installation

**En une ligne (Linux, sans root)** :

```bash
curl -fsSL https://devfrp.github.io/SimulSpeaker/install.sh | bash
```

**Windows / macOS** : téléchargez l'exe ou le .dmg depuis les
[releases](https://github.com/devfrp/SimulSpeaker/releases/latest). PipeWire
n'existant pas sur ces systèmes, l'application y fonctionne en **mode export** :
bouton « Exporter… » (ou `--export DOSSIER`) qui écrit

- `ir_LL/LR/RL/RR.wav` et `speaker-sim-true-stereo.wav` (4 canaux, ordre LL, LR, RL, RR),
- `speaker-sim.txt` pour **Equalizer APO ≥ 1.2** (Windows) : copiez le dossier dans
  `C:\Program Files\EqualizerAPO\config\` et ajoutez `Include: speaker-sim.txt` à `config.txt`.

Le HRTF y est lu par un lecteur SOFA en Python (h5py) à la place de libmysofa.
Réglages : `%LOCALAPPDATA%\devfrp-speaker-sim` (Windows), `~/Library/Application Support`
et `~/Library/Caches` (macOS).

**Depuis les sources** — dépendances système (Arch / Omarchy) :

```bash
sudo pacman -S --needed pipewire wireplumber libmysofa python-numpy python-scipy \
    python-requests python-pyqt6
```

Debian/Ubuntu : `sudo apt install pipewire wireplumber libmysofa1 python3-numpy
python3-scipy python3-requests python3-pyqt6`.

Ou dans un environnement virtuel (libmysofa reste un paquet système) :

```bash
cd speaker-sim
python3 -m venv .venv && . .venv/bin/activate
pip install -r requirements.txt
```

> PySide6 est accepté à la place de PyQt6 (repli automatique, voir `qt_compat.py`).

## Utilisation

### Interface graphique

```bash
python3 main.py
```

1. **Écouteurs** : tapez quelques mots (« hd 650 oratory ») puis cliquez sur votre modèle.
   Préférez les mesures *oratory1990* ou *crinacle* quand elles existent.
2. **Haut-parleurs** : choisissez un modèle (infobulle : type, score Olive, LFX).
3. **Pièce & rendu** : angle (30° = triangle stéréo standard), distance, dimensions,
   RT60, quantité de pièce (0 % = anéchoïque).
4. **Sortie** : laissez « Automatique » (le casque est la sortie par défaut
   au démarrage) ou choisissez explicitement le périphérique.
5. **Démarrer**. « Appliquer » régénère les filtres (et redémarre si actif) ;
   un « • » signale des réglages non appliqués.

### Ligne de commande

```bash
python3 main.py --search-headphones "hd 650"
python3 main.py --set-headphone "oratory1990/over-ear/Sennheiser HD 650"
python3 main.py --list-speakers
python3 main.py --set-speaker "Genelec 8030C" --start
python3 main.py --status
python3 main.py --stop
python3 main.py --autostart on      # ou off
python3 main.py --build             # générer les filtres sans démarrer
python3 main.py --import-headphone ma-mesure.txt --name "Mon casque" --target "Harman in-ear 2019"
```

### Exemple d'API (rendu hors ligne)

`examples/render_offline.py` produit les quatre IR sans PipeWire, réutilisables
dans tout convolveur (EasyEffects, Equalizer APO…) :

```bash
python3 examples/render_offline.py "Genelec 8030C" "oratory1990/over-ear/Sennheiser HD 650" /tmp/irs
```

## Fichiers

Tout s'exécute en tant qu'utilisateur. L'application n'écrit **que** dans :

| Emplacement | Contenu |
|---|---|
| `~/.local/state/devfrp-speaker-sim/config.json` | paramètres |
| `~/.local/state/devfrp-speaker-sim/runtime.json` | sortie précédente, état |
| `~/.local/state/devfrp-speaker-sim/filters/{a,b}/` | `ir_LL/LR/RL/RR.wav`, `filter-chain.conf` par instance |
| `~/.local/state/devfrp-speaker-sim/imports/` | mesures/EQ de casques importés + `index.json` |
| `~/.local/state/devfrp-speaker-sim/devfrp-speaker-sim-py-a.service` | unité (auto-démarrage) |
| `~/.local/state/devfrp-speaker-sim/speaker-sim.log` | journal |
| `~/.cache/devfrp-speaker-sim/autoeq/<commit>/` | index et corrections AutoEq |
| `~/.cache/devfrp-speaker-sim/spinorama/<commit>/` | mesures CEA-2034 |
| `~/.cache/devfrp-speaker-sim/hrtf/` | KEMAR, seulement si libmysofa n'en fournit pas |

`XDG_STATE_HOME` et `XDG_CACHE_HOME` sont respectés. Les réglages par défaut
sont dans `data/default_config.json`.

**Aucune configuration de PipeWire, WirePlumber ou Omarchy n'est modifiée.**
Le filtre est un processus `pipewire -c filters/filter-chain.conf` séparé,
lancé comme unité systemd *transitoire* (`systemd-run --user`). La seule
interaction avec WirePlumber est le choix de la sortie par défaut
(`wpctl set-default`), comme depuis le sélecteur de volume.

**Auto-démarrage** : l'unité est écrite dans le répertoire d'état puis
déclarée par `systemctl --user link` + `enable`. systemd crée alors des
**liens symboliques** dans `~/.config/systemd/user/` (c'est son seul moyen
de connaître une unité au démarrage) ; décocher l'option (`disable`) les supprime.

### Coexistence avec le greffon Omarchy `devfrp.speaker-sim`

Noms distincts (unités `devfrp-speaker-sim-py-{a,b}.service`, nœuds
`devfrp_speaker_sim_py_{a,b}`, fichiers `config.json`/`runtime.json`/`filters/`) : rien
n'écrase les fichiers du greffon (`settings.json`, `headphones/`). Le cache AutoEq
a la même disposition et le même commit, il est donc partagé. Les nœuds des deux
simulateurs ne sont jamais proposés comme « casque ». Évitez de les faire
tourner en même temps : chacun prendrait la sortie par défaut à son démarrage.

## Architecture

| Module | Rôle |
|---|---|
| `main.py` | point d'entrée : interface, ligne de commande, hooks systemd |
| `app.py` | `SpeakerSimApp` : orchestre les modules (sans Qt) |
| `audio_engine.py` | DSP : EQ AutoEq, CEA-2034, phase minimale, pièce, HRTF, rendu |
| `ui_panel.py` | interface Qt 6 (panneaux, courbes, tâches en arrière-plan) |
| `config.py` | chemins XDG, paramètres validés, état d'exécution |
| `download.py` | téléchargements figés et cache (AutoEq, Spinorama, KEMAR) |
| `system.py` | PipeWire (`pw-dump`, `wpctl`), filter-chain, systemd |
| `qt_compat.py` | PyQt6, ou PySide6 en repli |
| `packaging/` | icône, spec PyInstaller (exe Windows, app macOS) |
| `tools/build_speaker_catalog.py` | régénère `data/speakers.json` (développeurs) |

## Tests

```bash
python3 -m unittest discover -s tests -t .
```

Les tests n'utilisent ni le réseau, ni PipeWire, ni systemd (simulés).

## Dépannage

- **« libmysofa introuvable »** : installez `libmysofa` (le fichier
  `/usr/share/libmysofa/MIT_KEMAR_normal_pinna.sofa` est utilisé s'il existe).
- **Le service s'arrête aussitôt** : `journalctl --user -u 'devfrp-speaker-sim-py*' -n 50`.
- **Pas de son** : vérifiez avec `wpctl status` que « Speaker Sim » est la sortie par
  défaut et que `devfrp_speaker_sim_py.output` est relié au casque.
- **Volume faible** : la normalisation laisse de la marge pour le pire cas ;
  montez le *Préampli* (attention à l'écrêtage sur les basses).
- **Hors ligne** : seules les mesures déjà en cache sont utilisables.

## Licences des données

AutoEq (MIT, Jaakko Pasanen) ; Spinorama (GPL-3.0, Pierre Aubert) avec mesures
ASR/Klippel ; MIT KEMAR (Gardner & Martin, MIT Media Lab). Les données sont
téléchargées à l'usage, pas redistribuées.
