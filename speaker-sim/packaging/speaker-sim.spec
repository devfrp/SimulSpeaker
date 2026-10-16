# -*- mode: python ; coding: utf-8 -*-
"""PyInstaller : SpeakerSim.exe (Windows), Speaker Sim.app (macOS), binaire (Linux).

    pyinstaller --noconfirm packaging/speaker-sim.spec
"""
import sys
from pathlib import Path

ROOT = Path(SPECPATH).parent
NAME = "Speaker Sim" if sys.platform == "darwin" else "SpeakerSim"
icon = str(ROOT / "packaging" / ("icon.ico" if sys.platform == "win32" else "icon.png"))

a = Analysis(
    [str(ROOT / "main.py")],
    pathex=[str(ROOT)],
    datas=[(str(ROOT / "data"), "data"),
           (str(ROOT / "packaging" / "icon.png"), "packaging")],
    hiddenimports=["h5py", "scipy.signal", "scipy.io.wavfile"],
    excludes=["tkinter", "matplotlib", "PySide6" if sys.platform != "linux" else "PyQt5"],
    noarchive=False,
)
pyz = PYZ(a.pure)

if sys.platform == "darwin":
    exe = EXE(pyz, a.scripts, [], exclude_binaries=True, name=NAME, console=False,
              icon=icon, argv_emulation=False)
    coll = COLLECT(exe, a.binaries, a.datas, name=NAME)
    app = BUNDLE(coll, name=f"{NAME}.app", icon=icon,
                 bundle_identifier="io.github.devfrp.speakersim",
                 info_plist={"CFBundleShortVersionString": "1.0.0",
                             "NSHighResolutionCapable": True,
                             "NSHumanReadableCopyright": "© 2026 devfrp — MIT"})
else:
    exe = EXE(pyz, a.scripts, a.binaries, a.datas, [], name=NAME, console=False,
              icon=icon, upx=False)
