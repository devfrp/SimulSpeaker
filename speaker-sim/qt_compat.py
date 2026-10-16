"""Import de Qt : PyQt6 de préférence, PySide6 en repli (API Qt 6 identique)."""

from __future__ import annotations

try:
    from PyQt6 import QtCore, QtGui, QtWidgets

    Signal = QtCore.pyqtSignal
    Slot = QtCore.pyqtSlot
    QT_API = "PyQt6"
except ImportError:  # pragma: no cover - dépend de l'environnement
    from PySide6 import QtCore, QtGui, QtWidgets

    Signal = QtCore.Signal
    Slot = QtCore.Slot
    QT_API = "PySide6"

__all__ = ["QtCore", "QtGui", "QtWidgets", "Signal", "Slot", "QT_API"]
