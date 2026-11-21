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
#   ~/.local/share/devfrp-speaker-sim/speaker-sim   ~/.local/bin/speaker-sim (lien)
#   ~/.local/share/applications/devfrp-speaker-sim.desktop
#
# Binaire précompilé (x86_64) depuis la release ; sinon compilation depuis les sources
# avec cargo (branche, autre architecture).
set -euo pipefail

REPO="devfrp/SimulSpeaker"
APP_ID="devfrp-speaker-sim"
ASSET="SpeakerSim-Linux-x64"
DATA_HOME="${XDG_DATA_HOME:-$HOME/.local/share}"
APP_HOME="$DATA_HOME/$APP_ID"
APP_BIN="$APP_HOME/speaker-sim"
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
    -h|--help) sed -n '2,18p' "$0" 2>/dev/null || true; exit 0 ;;
    *) die "Option inconnue : $1" ;;
  esac
done

[ "$(uname -s)" = "Linux" ] || die "Ce script est pour Linux. Windows/macOS : https://devfrp.github.io/SimulSpeaker/#download"
[ "$(id -u)" -ne 0 ] || die "N'exécutez pas ce script en root : tout s'installe dans votre dossier personnel."

STATE_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/$APP_ID"

uninstall() {
  if [ -x "$LAUNCHER" ]; then
    "$LAUNCHER" --stop >/dev/null 2>&1 || true
    "$LAUNCHER" --autostart off >/dev/null 2>&1 || true
  fi
  # Uniquement ce que l'installateur a créé (y compris l'ancienne version Python).
  rm -rf "$APP_BIN" "$APP_BIN.new" "$APP_HOME/app" "$APP_HOME/app.new" "$APP_HOME/app.old" \
         "$APP_HOME/venv" "$APP_HOME/VERSION"
  rmdir "$APP_HOME" 2>/dev/null || true
  rm -f "$LAUNCHER" "$DESKTOP_FILE" "$ICON_FILE"
  if [ "$1" = "purge" ]; then
    rm -rf "$STATE_DIR/config.json" "$STATE_DIR/runtime.json" "$STATE_DIR/filters" "$STATE_DIR/imports" \
           "$STATE_DIR/$APP_ID-py.service" "$STATE_DIR/$APP_ID-py-a.service" "$STATE_DIR/$APP_ID-py-b.service" \
           "$STATE_DIR/speaker-sim.log"*
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
    echo "sudo pacman -S --needed pipewire wireplumber"
  elif command -v apt-get >/dev/null; then
    echo "sudo apt install pipewire wireplumber pipewire-bin"
  elif command -v dnf >/dev/null; then
    echo "sudo dnf install pipewire wireplumber pipewire-utils"
  elif command -v zypper >/dev/null; then
    echo "sudo zypper install pipewire wireplumber pipewire-tools"
  else
    echo "installez : pipewire, wireplumber (pw-dump, wpctl)"
  fi
}

missing=()
for tool in curl pipewire pw-dump wpctl systemctl systemd-run; do
  command -v "$tool" >/dev/null || missing+=("$tool")
done
[ ${#missing[@]} -eq 0 ] || die "Outils manquants : ${missing[*]}
  → $(pkg_hint)"

# --- Version ------------------------------------------------------------------
if [ -z "$REF" ]; then
  REF="$(curl -fsSL "https://api.github.com/repos/$REPO/releases/latest" 2>/dev/null \
         | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n1 || true)"
  [ -n "$REF" ] || REF="master"
fi

TMP="$(mktemp -d)"
trap 'rm -rf "$TMP"' EXIT
mkdir -p "$APP_HOME"

# --- Binaire : release précompilée, sinon compilation -------------------------
fetch_release() {
  [ "$(uname -m)" = "x86_64" ] || return 1
  case "$REF" in v[0-9]*) ;; *) return 1 ;; esac
  curl -fsSL "https://github.com/$REPO/releases/download/$REF/$ASSET" -o "$TMP/speaker-sim" 2>/dev/null
}

build_from_source() {
  command -v cargo >/dev/null \
    || die "Pas de binaire précompilé pour $REF ($(uname -m)) et cargo est absent.
  → installez Rust (https://rustup.rs), puis relancez cette commande."
  say "Compilation depuis les sources ($REF), quelques minutes…"
  if [ -n "${SPEAKER_SIM_TARBALL:-}" ]; then   # tests : archive locale
    cp "$SPEAKER_SIM_TARBALL" "$TMP/src.tar.gz"
  else
    curl -fsSL "https://codeload.github.com/$REPO/tar.gz/$REF" -o "$TMP/src.tar.gz" \
      || die "Téléchargement impossible (https://github.com/$REPO, réf. $REF)."
  fi
  mkdir -p "$TMP/src"
  tar -xzf "$TMP/src.tar.gz" -C "$TMP/src" --strip-components=1
  [ -f "$TMP/src/speaker-sim/Cargo.toml" ] || die "Archive inattendue : speaker-sim/Cargo.toml absent."
  (cd "$TMP/src/speaker-sim" && CARGO_TARGET_DIR="$TMP/target" cargo build --release --locked --quiet) \
    || die "La compilation a échoué."
  cp "$TMP/target/release/speaker-sim" "$TMP/speaker-sim"
  cp "$TMP/src/speaker-sim/packaging/icon.png" "$TMP/icon.png"
}

if [ -z "${SPEAKER_SIM_TARBALL:-}" ] && fetch_release; then
  say "Téléchargement de Speaker Sim $REF (binaire précompilé)…"
else
  build_from_source
fi
chmod +x "$TMP/speaker-sim"
"$TMP/speaker-sim" --version >/dev/null || die "Le binaire téléchargé ne démarre pas."

# Remplacement atomique : les unités systemd appellent ce chemin pour leurs hooks.
cp "$TMP/speaker-sim" "$APP_BIN.new"
mv -f "$APP_BIN.new" "$APP_BIN"
echo "$REF" > "$APP_HOME/VERSION"

# Ancienne version Python : environnement et sources inutiles désormais.
MIGRATED=0
if [ -d "$APP_HOME/venv" ] || [ -d "$APP_HOME/app" ]; then
  rm -rf "$APP_HOME/venv" "$APP_HOME/app" "$APP_HOME/app.new" "$APP_HOME/app.old"
  MIGRATED=1
fi

# --- Lanceur, menu, icône -----------------------------------------------------
mkdir -p "$BIN_DIR"
ln -sfn "$APP_BIN" "$LAUNCHER"

if [ "$DESKTOP" -eq 1 ]; then
  mkdir -p "$(dirname "$DESKTOP_FILE")" "$(dirname "$ICON_FILE")"
  if [ ! -f "$TMP/icon.png" ]; then
    curl -fsSL "https://raw.githubusercontent.com/$REPO/$REF/speaker-sim/packaging/icon.png" \
      -o "$TMP/icon.png" 2>/dev/null || true
  fi
  [ -s "$TMP/icon.png" ] && cp "$TMP/icon.png" "$ICON_FILE"
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

# Migration depuis Python : les unités systemd appelaient l'ancien interpréteur.
if [ "$MIGRATED" -eq 1 ]; then
  status="$("$LAUNCHER" --status 2>/dev/null || true)"
  case "$status" in
    *'"autostart": true'*) "$LAUNCHER" --autostart on >/dev/null 2>&1 \
                             || warn "Auto-démarrage à réactiver depuis l'interface." ;;
  esac
  case "$status" in
    *'"running": true'*) "$LAUNCHER" --apply >/dev/null 2>&1 \
                           || warn "Relancez la simulation depuis l'interface." ;;
  esac
  say "Ancienne version Python remplacée (réglages conservés)."
fi

say "Installé : ${C_DIM}$APP_BIN${C_0} ($("$LAUNCHER" --version))"
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
