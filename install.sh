#!/usr/bin/env bash
# Speaker Sim (devfrp) — installation Linux en une ligne :
#
#   curl -fsSL https://devfrp.github.io/SimulSpeaker/install.sh | bash
#
# Options (après « bash -s -- ») :
#   --ref <tag|branche>  version à installer (défaut : dernière release, sinon master)
#   --uninstall          désinstaller (garde réglages et cache)
#   --purge              désinstaller et supprimer réglages + cache
#   --no-desktop         ne pas créer d'entrée de menu
#
# Tout s'installe pour l'utilisateur courant, sans root :
#   ~/.local/share/devfrp-speaker-sim/{app,venv}   ~/.local/bin/speaker-sim
#   ~/.local/share/applications/devfrp-speaker-sim.desktop
set -euo pipefail

REPO="devfrp/SimulSpeaker"
APP_ID="devfrp-speaker-sim"
DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
APP_HOME="$DATA_HOME/$APP_ID"
BIN_DIR="$HOME/.local/bin"
LAUNCHER="$BIN_DIR/speaker-sim"
DESKTOP_FILE="$DATA_HOME/applications/$APP_ID.desktop"
ICON_FILE="$DATA_HOME/icons/hicolor/512x512/apps/$APP_ID.png"

REF=""
MODE="install"
DESKTOP=1

if [ -t 1 ]; then
  C_OK=$'\e[38;5;79m'; C_WARN=$'\e[38;5;179m'; C_ERR=$'\e[38;5;167m'; C_DIM=$'\e[2m'; C_0=$'\e[0m'
else
  C_OK=""; C_WARN=""; C_ERR=""; C_DIM=""; C_0=""
fi
say()  { printf '%s●%s %s\n' "$C_OK" "$C_0" "$*"; }
warn() { printf '%s!%s %s\n' "$C_WARN" "$C_0" "$*" >&2; }
die()  { printf '%s✗%s %s\n' "$C_ERR" "$C_0" "$*" >&2; exit 1; }

while [ $# -gt 0 ]; do
  case "$1" in
    --ref) REF="${2:?--ref attend une valeur}"; shift 2 ;;
    --uninstall) MODE="uninstall"; shift ;;
    --purge) MODE="purge"; shift ;;
    --no-desktop) DESKTOP=0; shift ;;
    -h|--help) sed -n '2,16p' "$0" 2>/dev/null || true; exit 0 ;;
    *) die "Option inconnue : $1" ;;
  esac
done

[ "$(uname -s)" = "Linux" ] || die "Ce script est pour Linux. Windows/macOS : https://devfrp.github.io/SimulSpeaker/#download"
[ "$(id -u)" -ne 0 ] || die "N'exécutez pas ce script en root : tout s'installe dans votre dossier personnel."

uninstall() {
  if [ -x "$LAUNCHER" ]; then
    "$LAUNCHER" --stop >/dev/null 2>&1 || true
    "$LAUNCHER" --autostart off >/dev/null 2>&1 || true
  fi
  # Uniquement ce que l'installateur a créé, jamais le dossier entier.
  rm -rf "$APP_HOME/app" "$APP_HOME/app.new" "$APP_HOME/app.old" "$APP_HOME/venv" "$APP_HOME/VERSION"
  rmdir "$APP_HOME" 2>/dev/null || true
  rm -f "$LAUNCHER" "$DESKTOP_FILE" "$ICON_FILE"
  if [ "$1" = "purge" ]; then
    rm -rf "${XDG_STATE_HOME:-$HOME/.local/state}/$APP_ID/config.json" \
           "${XDG_STATE_HOME:-$HOME/.local/state}/$APP_ID/runtime.json" \
           "${XDG_STATE_HOME:-$HOME/.local/state}/$APP_ID/filters" \
           "${XDG_STATE_HOME:-$HOME/.local/state}/$APP_ID/$APP_ID-py.service" \
           "${XDG_STATE_HOME:-$HOME/.local/state}/$APP_ID/speaker-sim.log"*
    say "Réglages supprimés (le cache partagé ~/.cache/$APP_ID est conservé)."
  fi
  say "Speaker Sim désinstallé."
}

if [ "$MODE" != "install" ]; then
  uninstall "$MODE"
  exit 0
fi

# --- Dépendances --------------------------------------------------------------
pkg_hint() {
  if command -v pacman >/dev/null; then
    echo "sudo pacman -S --needed python pipewire wireplumber libmysofa python-numpy python-scipy python-requests python-pyqt6"
  elif command -v apt-get >/dev/null; then
    echo "sudo apt install python3 python3-venv pipewire wireplumber libmysofa1 python3-numpy python3-scipy python3-requests python3-pyqt6"
  elif command -v dnf >/dev/null; then
    echo "sudo dnf install python3 pipewire wireplumber libmysofa python3-numpy python3-scipy python3-requests python3-pyqt6"
  elif command -v zypper >/dev/null; then
    echo "sudo zypper install python3 pipewire wireplumber libmysofa1 python3-numpy python3-scipy python3-requests python3-PyQt6"
  else
    echo "installez : python3 (≥ 3.10), pipewire, wireplumber, libmysofa"
  fi
}

# (pas de « grep -q » : avec pipefail, la sortie anticipée provoque un SIGPIPE)
has_mysofa() { ldconfig -p 2>/dev/null | grep libmysofa >/dev/null; }

# Python du système de préférence : il voit numpy/scipy/Qt installés par la distribution.
PYTHON="${SPEAKER_SIM_PYTHON:-}"
if [ -z "$PYTHON" ]; then
  if [ -x /usr/bin/python3 ]; then PYTHON=/usr/bin/python3; else PYTHON="$(command -v python3 || true)"; fi
fi
[ -n "$PYTHON" ] || die "python3 introuvable. $(pkg_hint)"
"$PYTHON" -c 'import sys; sys.exit(sys.version_info < (3, 10))' \
  || die "Python ≥ 3.10 requis ($("$PYTHON" --version 2>&1))."
"$PYTHON" -c 'import venv, ensurepip' 2>/dev/null \
  || die "Module venv absent pour $PYTHON. $(pkg_hint)"

missing=()
for tool in curl tar pipewire pw-dump wpctl systemctl systemd-run; do
  command -v "$tool" >/dev/null || missing+=("$tool")
done
[ ${#missing[@]} -eq 0 ] || die "Outils manquants : ${missing[*]}
  → $(pkg_hint)"

if ! has_mysofa; then
  warn "libmysofa absent : repli sur le lecteur SOFA Python (h5py) et téléchargement du HRTF KEMAR."
  warn "Recommandé : $(pkg_hint)"
fi

# --- Téléchargement -----------------------------------------------------------
if [ -z "$REF" ]; then
  REF="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" 2>/dev/null \
         | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n1 || true)"
  [ -n "$REF" ] || REF="master"
fi
say "Téléchargement de Speaker Sim ($REF)…"

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
if [ -n "${SPEAKER_SIM_TARBALL:-}" ]; then   # tests : archive locale
  cp "$SPEAKER_SIM_TARBALL" "$TMP/src.tar.gz"
else
  curl -fsSL "https://codeload.github.com/$REPO/tar.gz/$REF" -o "$TMP/src.tar.gz" \
    || die "Téléchargement impossible (https://github.com/$REPO, réf. $REF)."
fi
mkdir -p "$TMP/src"
tar -xzf "$TMP/src.tar.gz" -C "$TMP/src" --strip-components=1
[ -f "$TMP/src/speaker-sim/main.py" ] || die "Archive inattendue : speaker-sim/main.py absent."

mkdir -p "$APP_HOME"
rm -rf "$APP_HOME/app.new"
cp -r "$TMP/src/speaker-sim" "$APP_HOME/app.new"
rm -rf "$APP_HOME/app.old"
[ -d "$APP_HOME/app" ] && mv "$APP_HOME/app" "$APP_HOME/app.old"
mv "$APP_HOME/app.new" "$APP_HOME/app"
rm -rf "$APP_HOME/app.old"
echo "$REF" > "$APP_HOME/VERSION"

# --- Environnement Python -----------------------------------------------------
VENV="$APP_HOME/venv"
if [ ! -x "$VENV/bin/python" ]; then
  say "Création de l'environnement Python ($("$PYTHON" --version 2>&1))…"
  "$PYTHON" -m venv --system-site-packages "$VENV"
fi
need=()
"$VENV/bin/python" -c 'import numpy' 2>/dev/null || need+=(numpy)
"$VENV/bin/python" -c 'import scipy' 2>/dev/null || need+=(scipy)
"$VENV/bin/python" -c 'import requests' 2>/dev/null || need+=(requests)
"$VENV/bin/python" -c 'import PyQt6.QtWidgets' 2>/dev/null \
  || "$VENV/bin/python" -c 'import PySide6.QtWidgets' 2>/dev/null || need+=(PyQt6)
if ! has_mysofa; then
  "$VENV/bin/python" -c 'import h5py' 2>/dev/null || need+=(h5py)
fi
if [ ${#need[@]} -gt 0 ]; then
  say "Installation (pip, dans l'environnement isolé) : ${need[*]}"
  "$VENV/bin/python" -m pip install --quiet --disable-pip-version-check "${need[@]}" \
    || die "pip a échoué. Installez plutôt les paquets système : $(pkg_hint)"
fi

# --- Lanceur, menu, icône -----------------------------------------------------
mkdir -p "$BIN_DIR"
cat > "$LAUNCHER" <<EOF
#!/bin/sh
exec "$VENV/bin/python" "$APP_HOME/app/main.py" "\$@"
EOF
chmod +x "$LAUNCHER"

if [ "$DESKTOP" -eq 1 ]; then
  mkdir -p "$(dirname "$DESKTOP_FILE")" "$(dirname "$ICON_FILE")"
  cp "$APP_HOME/app/packaging/icon.png" "$ICON_FILE"
  cat > "$DESKTOP_FILE" <<EOF
[Desktop Entry]
Type=Application
Name=Speaker Sim
GenericName=Simulation de haut-parleurs
Comment=Écoutez de vrais haut-parleurs dans une pièce, au casque (PipeWire)
Exec=$LAUNCHER
Icon=$APP_ID
Terminal=false
Categories=AudioVideo;Audio;Settings;
Keywords=headphones;speakers;hrtf;autoeq;pipewire;casque;enceintes;
StartupWMClass=devfrp-speaker-sim
EOF
  if command -v update-desktop-database >/dev/null; then
    update-desktop-database -q "$(dirname "$DESKTOP_FILE")" || true
  fi
fi

"$LAUNCHER" --version >/dev/null || die "L'application ne démarre pas ; voir $LAUNCHER"

say "Installé : ${C_DIM}$APP_HOME${C_0}"
case ":$PATH:" in
  *":$BIN_DIR:"*) ;;
  *) warn "$BIN_DIR n'est pas dans votre PATH ; ajoutez-le ou lancez $LAUNCHER" ;;
esac
cat <<EOF

  speaker-sim                                   interface graphique
  speaker-sim --search-headphones "hd 650"      chercher un casque
  speaker-sim --set-speaker "KEF R3" --start    démarrer en ligne de commande
  speaker-sim --stop                            arrêter (la sortie précédente est restaurée)

  Mise à jour : relancez la même commande curl.
  Désinstallation : curl -fsSL https://devfrp.github.io/SimulSpeaker/install.sh | bash -s -- --uninstall
EOF
