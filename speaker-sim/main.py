#!/usr/bin/env python3
"""Point d'entrée : interface graphique par défaut, ou commandes en ligne.

Exemples ::

    python3 main.py                                  # interface graphique
    python3 main.py --search-headphones "hd 650"     # chercher un casque
    python3 main.py --set-headphone "oratory1990/over-ear/Sennheiser HD 650"
    python3 main.py --set-speaker "KEF R3" --start   # choisir et démarrer
    python3 main.py --status
    python3 main.py --stop
    python3 main.py --autostart on
    python3 main.py --export ~/irs                   # Windows/macOS : filtres portables
    python3 main.py --import-headphone mesure.txt --name "Mon casque" --target "Harman in-ear 2019"

``--hook-started`` / ``--hook-stopped`` sont appelés par l'unité systemd.
"""

from __future__ import annotations

import argparse
import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))


def build_parser() -> argparse.ArgumentParser:
    p = argparse.ArgumentParser(prog="speaker-sim",
                                description="Simulation de haut-parleurs au casque (PipeWire)")
    p.add_argument("--start", action="store_true", help="générer les filtres et démarrer")
    p.add_argument("--apply", action="store_true", help="synonyme de --start")
    p.add_argument("--stop", action="store_true", help="arrêter et restaurer la sortie")
    p.add_argument("--status", action="store_true", help="afficher l'état (JSON)")
    p.add_argument("--build", action="store_true", help="générer les filtres sans démarrer")
    p.add_argument("--export", metavar="DOSSIER",
                   help="exporter les filtres (WAV, true stereo, Equalizer APO)")
    p.add_argument("--version", action="store_true", help="afficher la version")
    p.add_argument("--list-speakers", action="store_true", help="lister les haut-parleurs du catalogue")
    p.add_argument("--search-headphones", metavar="TEXTE", help="chercher dans l'index AutoEq")
    p.add_argument("--set-headphone", metavar="CHEMIN",
                   help="casque, par chemin AutoEq (source/banc/modèle)")
    p.add_argument("--set-speaker", metavar="ID", help="haut-parleur, par identifiant")
    p.add_argument("--autostart", choices=("on", "off"), help="auto-démarrage à la connexion")
    p.add_argument("-v", "--verbose", action="store_true")
    p.add_argument("--import-headphone", metavar="FICHIER",
                   help="importer une mesure (fréquence, dB) ou un EQ AutoEq / Equalizer APO")
    p.add_argument("--name", metavar="NOM", help="nom du casque importé")
    p.add_argument("--target", metavar="CIBLE",
                   help="cible pour une mesure brute (défaut : Harman over-ear 2018)")
    # Hooks systemd : argument facultatif = instance (a/b) ; absent = unité v1.0.
    p.add_argument("--hook-started", nargs="?", const="", default=None, help=argparse.SUPPRESS)
    p.add_argument("--hook-stopped", nargs="?", const="", default=None, help=argparse.SUPPRESS)
    return p


def run_hooks(args) -> int:
    from config import ConfigStore, Paths
    from system import PipeWire, hook_started, hook_stopped

    store = ConfigStore(Paths.default())
    pw = PipeWire()
    try:
        if args.hook_started is not None:
            return hook_started(store, pw, args.hook_started, log=lambda m: print(m, file=sys.stderr))
        return hook_stopped(store, pw, args.hook_stopped, log=lambda m: print(m, file=sys.stderr))
    except Exception as exc:  # un hook ne doit jamais faire échouer l'unité
        print(f"hook: {exc}", file=sys.stderr)
        return 0


def run_cli(args) -> int:
    from app import AppError, SpeakerSimApp, setup_logging

    app = SpeakerSimApp()
    setup_logging(app.paths, args.verbose)

    def progress(fraction: float, message: str) -> None:
        print(f"[{fraction * 100:3.0f} %] {message}", file=sys.stderr)

    try:
        if args.list_speakers:
            for spk in app.speakers:
                score = spk.get("pref_score_wsub")
                print(f"{spk['id']:<40} {spk.get('category', 'hifi'):<7} "
                      f"score {score if score is not None else '—':>4}  {spk['shape']}")
        if args.search_headphones is not None:
            for e in app.search_headphones(args.search_headphones, limit=50):
                print(f"{e.path}\t({e.source}, {e.rig})")
        if args.set_headphone:
            entry = next((e for e in app.headphones() if e.path == args.set_headphone), None)
            if entry is None:
                matches = app.search_headphones(args.set_headphone, limit=2)
                if len(matches) != 1:
                    raise AppError("Casque ambigu ou introuvable ; utilisez le chemin "
                                   "affiché par --search-headphones")
                entry = matches[0]
            app.select_headphone(entry)
            print(f"Casque : {entry.label}")
        if args.import_headphone:
            entry = app.import_headphone(args.import_headphone, args.name or "", args.target or "")
            print(f"Casque importé et sélectionné : {entry.label}")
        if args.set_speaker:
            app.select_speaker(args.set_speaker)
            print(f"Haut-parleur : {app.description()}")
        if args.build:
            result = app.build_filters(progress)
            for note in result.notes:
                print(note)
        if args.export:
            for path in app.export(args.export, progress):
                print(path)
        if args.start or args.apply:
            result = app.start(progress)
            for note in result.notes:
                print(note)
        if args.stop:
            app.stop()
        if args.autostart:
            app.set_autostart(args.autostart == "on")
        if args.status:
            print(json.dumps(app.status(), indent=2, ensure_ascii=False))
    except AppError as exc:
        print(f"Erreur : {exc}", file=sys.stderr)
        return 1
    return 0


def run_gui() -> int:
    from ui_panel import run

    return run()


def main(argv: list[str] | None = None) -> int:
    args = build_parser().parse_args(argv)
    if args.version:
        from config import APP_NAME, APP_VERSION
        print(f"{APP_NAME} {APP_VERSION}")
        return 0
    if args.hook_started is not None or args.hook_stopped is not None:
        return run_hooks(args)
    cli = any([args.start, args.apply, args.stop, args.status, args.build, args.list_speakers,
               args.search_headphones is not None, args.set_headphone, args.set_speaker,
               args.autostart, args.export, args.import_headphone])
    return run_cli(args) if cli else run_gui()


if __name__ == "__main__":
    sys.exit(main())
