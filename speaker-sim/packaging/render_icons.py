#!/usr/bin/env python3
"""Rend packaging/icon.svg en PNG (512, 256) et ICO (Windows) avec Qt (outil développeur)."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent.parent))
from qt_compat import QT_API, QtCore, QtGui, QtWidgets  # noqa: E402

if QT_API == "PyQt6":
    from PyQt6 import QtSvg  # noqa: E402
else:
    from PySide6 import QtSvg  # noqa: E402

HERE = Path(__file__).resolve().parent


def render(size: int) -> QtGui.QImage:
    img = QtGui.QImage(size, size, QtGui.QImage.Format.Format_ARGB32)
    img.fill(QtCore.Qt.GlobalColor.transparent)
    painter = QtGui.QPainter(img)
    QtSvg.QSvgRenderer(str(HERE / "icon.svg")).render(painter)
    painter.end()
    return img


if __name__ == "__main__":
    app = QtWidgets.QApplication(sys.argv)
    render(512).save(str(HERE / "icon.png"))
    render(256).save(str(HERE / "icon.ico"))
    print("icon.png, icon.ico")
