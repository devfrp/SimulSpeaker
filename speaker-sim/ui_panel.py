"""Interface graphique (Qt 6) — « console de salle ».

Disposition :

* barre supérieure : chaîne casque → enceinte → salle, état, bouton d'alimentation ;
* gauche : 01 Casque (recherche AutoEq + imports), 02 Enceintes (filtre studio / hi-fi) ;
* centre : vue en plan interactive de la pièce (traitement, matériaux, rayons des
  premières réflexions ; l'auditeur et les enceintes se déplacent à la souris),
  courbe de réponse et RT60 par octave ;
* droite : 03 Salle (préréglages, traitement, matériaux, placement), 04 Rendu, 05 Sortie.

Toutes les opérations longues passent par :class:`Worker` (``QThreadPool``).
En mode « Auto », chaque changement est appliqué après une courte pause ; la
bascule A/B du moteur évite toute coupure.
"""

from __future__ import annotations

import math
import os
import sys
import traceback
from typing import Callable

import numpy as np

import audio_engine as ae
from app import AppError, SpeakerSimApp, setup_logging
from config import APP_DIR, APP_NAME, ROOM_KEYS, ROOM_PRESETS
from download import AUTOEQ_TARGETS, HeadphoneEntry, matches, query_patterns
from qt_compat import QtCore, QtGui, QtWidgets, Signal

Qt = QtCore.Qt
FONT_DIR = APP_DIR / "packaging" / "fonts"
DISPLAY, BODY, MONO = "Space Grotesk", "Inter", "JetBrains Mono"

# Palette (identité devfrp)
BG, PANEL, PANEL2 = "#14151a", "#1b1d24", "#21232b"
BORDER, BORDER_STRONG = "#26282f", "#34363e"
TEXT, MUTED = "#ededef", "#8b8d98"
AMBER, TEAL, RED = "#e8a33d", "#49b9ae", "#e5484d"


def C(hex_color: str, alpha: int = 255) -> QtGui.QColor:
    c = QtGui.QColor(hex_color)
    c.setAlpha(alpha)
    return c


STYLE = f"""
* {{ font-family: "{BODY}"; font-size: 13px; color: {TEXT}; }}
QMainWindow, QWidget#root, QWidget#body, QScrollArea, QScrollArea > QWidget > QWidget {{
    background: {BG}; border: none; }}
QLabel[muted="true"] {{ color: {MUTED}; }}
QLabel[mono="true"] {{ font-family: "{MONO}"; font-size: 11px; letter-spacing: 0.5px; }}
QLabel#wordmark {{ font-family: "{MONO}"; font-weight: 700; font-size: 15px; }}
QLabel#chain {{ font-family: "{MONO}"; font-size: 12px; color: {MUTED}; }}
QLabel#sectionTitle {{ font-family: "{DISPLAY}"; font-size: 16px; font-weight: 600; }}
QLabel#tag {{ font-family: "{MONO}"; font-size: 11px; color: {AMBER};
    border: 1px solid {BORDER_STRONG}; border-radius: 9px; padding: 0 7px; }}
QLabel#value {{ font-family: "{MONO}"; font-size: 12px; color: {AMBER}; }}
QLabel#status {{ font-family: "{MONO}"; font-size: 11px; letter-spacing: 1px;
    border-radius: 10px; padding: 3px 10px; }}
QFrame#card {{ background: {PANEL}; border: 1px solid {BORDER}; border-radius: 14px; }}
QFrame#topbar {{ background: rgba(20,21,26,235); border-bottom: 1px solid {BORDER}; }}
QFrame#footer {{ background: {BG}; border-top: 1px solid {BORDER}; }}
QLineEdit, QComboBox {{ background: {PANEL2}; border: 1px solid {BORDER_STRONG};
    border-radius: 8px; padding: 7px 10px; selection-background-color: {AMBER};
    selection-color: {BG}; }}
QLineEdit:focus, QComboBox:focus {{ border-color: {TEAL}; }}
QComboBox QAbstractItemView {{ background: {PANEL2}; border: 1px solid {BORDER_STRONG};
    selection-background-color: {AMBER}; selection-color: {BG}; }}
QListView, QListWidget {{ background: {BG}; border: 1px solid {BORDER}; border-radius: 10px;
    padding: 4px; outline: 0; }}
QListView::item, QListWidget::item {{ padding: 6px 8px; border-radius: 6px; }}
QListView::item:hover, QListWidget::item:hover {{ background: {PANEL2}; }}
QListView::item:selected, QListWidget::item:selected {{ background: rgba(232,163,61,40);
    color: {AMBER}; }}
QPushButton {{ font-family: "{MONO}"; font-size: 12px; letter-spacing: 0.5px;
    background: {PANEL}; border: 1px solid {BORDER_STRONG}; border-radius: 8px;
    padding: 8px 14px; }}
QPushButton:hover {{ border-color: {TEAL}; background: {PANEL2}; }}
QPushButton:disabled {{ color: #5a5c66; border-color: {BORDER}; }}
QPushButton#primary {{ background: {AMBER}; color: {BG}; border-color: {AMBER}; font-weight: 700; }}
QPushButton#primary:hover {{ background: #f0b25a; }}
QPushButton#seg {{ font-size: 11px; padding: 6px 8px; border-radius: 0; background: {PANEL2};
    color: {MUTED}; border: 1px solid {BORDER_STRONG}; margin-left: -1px; }}
QPushButton#seg[pos="first"] {{ border-top-left-radius: 7px; border-bottom-left-radius: 7px;
    margin-left: 0; }}
QPushButton#seg[pos="last"] {{ border-top-right-radius: 7px; border-bottom-right-radius: 7px; }}
QPushButton#seg[pos="only"] {{ border-radius: 7px; margin-left: 0; }}
QPushButton#seg:checked {{ background: {AMBER}; color: {BG}; border-color: {AMBER}; font-weight: 700; }}
QPushButton#seg:hover:!checked {{ color: {TEXT}; }}
QPushButton#chip {{ font-size: 11px; padding: 5px 10px; border-radius: 13px; background: {PANEL2}; }}
QPushButton#chip:checked {{ border-color: {TEAL}; color: {TEAL}; background: rgba(73,185,174,28); }}
QCheckBox {{ spacing: 9px; }}
QCheckBox::indicator {{ width: 30px; height: 16px; border-radius: 8px; background: {BORDER_STRONG};
    border: none; }}
QCheckBox::indicator:checked {{ background: {TEAL}; }}
QSlider::groove:horizontal {{ height: 4px; background: {BORDER_STRONG}; border-radius: 2px; }}
QSlider::sub-page:horizontal {{ background: {AMBER}; border-radius: 2px; }}
QSlider::handle:horizontal {{ background: {TEXT}; width: 14px; height: 14px; margin: -5px 0;
    border-radius: 7px; }}
QSlider[accent="teal"]::sub-page:horizontal {{ background: {TEAL}; }}
QProgressBar {{ background: transparent; border: none; max-height: 2px; }}
QProgressBar::chunk {{ background: {AMBER}; }}
QScrollBar:vertical {{ background: transparent; width: 10px; margin: 2px; }}
QScrollBar::handle:vertical {{ background: {BORDER_STRONG}; border-radius: 4px; min-height: 40px; }}
QScrollBar:horizontal {{ background: transparent; height: 10px; margin: 2px; }}
QScrollBar::handle:horizontal {{ background: {BORDER_STRONG}; border-radius: 4px; min-width: 40px; }}
QScrollBar::add-line, QScrollBar::sub-line {{ width: 0; height: 0; }}
QScrollBar::add-page, QScrollBar::sub-page {{ background: transparent; }}
QToolTip {{ background: {PANEL2}; color: {TEXT}; border: 1px solid {BORDER_STRONG}; padding: 6px; }}
QMenu {{ background: {PANEL2}; border: 1px solid {BORDER_STRONG}; padding: 4px; }}
QMenu::item {{ padding: 6px 14px; border-radius: 5px; }}
QMenu::item:selected {{ background: {AMBER}; color: {BG}; }}
"""


def load_fonts() -> None:
    if FONT_DIR.is_dir():
        for font in sorted(FONT_DIR.glob("*.ttf")):
            QtGui.QFontDatabase.addApplicationFont(str(font))


def fr_num(value: float, decimals: int = 1) -> str:
    return f"{value:.{decimals}f}".replace(".", ",")


# ---------------------------------------------------------------------------
# Travail en arrière-plan
# ---------------------------------------------------------------------------
class WorkerSignals(QtCore.QObject):
    progress = Signal(float, str)
    done = Signal(object)
    failed = Signal(str)


class Worker(QtCore.QRunnable):
    """Tâche de fond. ``signals`` doit avoir un parent Qt vivant (la fenêtre) : sans
    parent, PySide6 peut détruire l'objet pendant une boucle d'événements imbriquée
    (boîte de dialogue ouverte pendant un calcul) → « Signal source has been deleted »."""

    def __init__(self, fn: Callable, *args, with_progress: bool = False,
                 signals: WorkerSignals | None = None):
        super().__init__()
        self.fn, self.args, self.with_progress = fn, args, with_progress
        self.signals = signals or WorkerSignals()

    def run(self):
        try:
            args = self.args + ((self.signals.progress.emit,) if self.with_progress else ())
            result = self.fn(*args)
        except AppError as exc:
            self.signals.failed.emit(str(exc))
        except Exception as exc:  # erreur inattendue : message + trace dans le journal
            traceback.print_exc()
            self.signals.failed.emit(f"{type(exc).__name__} : {exc}")
        else:
            self.signals.done.emit(result)


# ---------------------------------------------------------------------------
# Petits composants
# ---------------------------------------------------------------------------
def label(text: str = "", *, muted: bool = False, mono: bool = False, name: str = "",
          wrap: bool = False) -> QtWidgets.QLabel:
    lbl = QtWidgets.QLabel(text)
    if muted:
        lbl.setProperty("muted", True)
    if mono:
        lbl.setProperty("mono", True)
    if name:
        lbl.setObjectName(name)
    lbl.setWordWrap(wrap)
    return lbl


def section(tag: str, title: str, subtitle: str = "") -> tuple[QtWidgets.QFrame, QtWidgets.QVBoxLayout]:
    frame = QtWidgets.QFrame()
    frame.setObjectName("card")
    lay = QtWidgets.QVBoxLayout(frame)
    lay.setContentsMargins(16, 14, 16, 16)
    lay.setSpacing(10)
    head = QtWidgets.QHBoxLayout()
    head.setSpacing(10)
    head.addWidget(label(tag, name="tag"))
    head.addWidget(label(title, name="sectionTitle"), 1)
    lay.addLayout(head)
    if subtitle:
        lay.addWidget(label(subtitle, muted=True, wrap=True))
    return frame, lay


def caption(text: str) -> QtWidgets.QLabel:
    return label(text.upper(), muted=True, mono=True)


class Segmented(QtWidgets.QWidget):
    """Choix exclusif sous forme de boutons accolés."""

    changed = Signal(object)

    def __init__(self, options: list[tuple[object, str]], parent=None):
        super().__init__(parent)
        lay = QtWidgets.QHBoxLayout(self)
        lay.setContentsMargins(0, 0, 0, 0)
        lay.setSpacing(0)
        self.group = QtWidgets.QButtonGroup(self)
        self.group.setExclusive(True)
        self.values = [v for v, _ in options]
        for i, (_value, text) in enumerate(options):
            btn = QtWidgets.QPushButton(text)
            btn.setObjectName("seg")
            btn.setCheckable(True)
            btn.setCursor(Qt.CursorShape.PointingHandCursor)
            btn.setProperty("pos", "only" if len(options) == 1 else
                            "first" if i == 0 else "last" if i == len(options) - 1 else "mid")
            btn.setSizePolicy(QtWidgets.QSizePolicy.Policy.Expanding,
                              QtWidgets.QSizePolicy.Policy.Fixed)
            self.group.addButton(btn, i)
            lay.addWidget(btn)
        self.group.idClicked.connect(lambda i: self.changed.emit(self.values[i]))

    def set_value(self, value) -> None:
        if value in self.values:
            self.group.button(self.values.index(value)).setChecked(True)


class ValueSlider(QtWidgets.QWidget):
    """Curseur avec libellé et valeur ; ``changed`` en continu, ``committed`` à la fin."""

    changed = Signal(float)
    committed = Signal(float)

    def __init__(self, text: str, lo: float, hi: float, step: float,
                 fmt: Callable[[float], str], accent: str = "", parent=None):
        super().__init__(parent)
        self.lo, self.step, self.fmt = lo, step, fmt
        lay = QtWidgets.QVBoxLayout(self)
        lay.setContentsMargins(0, 0, 0, 0)
        lay.setSpacing(4)
        head = QtWidgets.QHBoxLayout()
        head.addWidget(label(text, muted=True), 1)
        self.value_label = label(name="value")
        head.addWidget(self.value_label)
        lay.addLayout(head)
        self.slider = QtWidgets.QSlider(Qt.Orientation.Horizontal)
        self.slider.setRange(0, int(round((hi - lo) / step)))
        if accent:
            self.slider.setProperty("accent", accent)
        lay.addWidget(self.slider)
        self._timer = QtCore.QTimer(self, singleShot=True, interval=350)
        self._timer.timeout.connect(lambda: self.committed.emit(self.value()))
        self.slider.valueChanged.connect(self._moved)
        self.slider.sliderReleased.connect(lambda: (self._timer.stop(),
                                                    self.committed.emit(self.value())))

    def value(self) -> float:
        return self.lo + self.slider.value() * self.step

    def _moved(self, _raw: int):
        v = self.value()
        self.value_label.setText(self.fmt(v))
        self.changed.emit(v)
        if not self.slider.isSliderDown():
            self._timer.start()   # clavier / molette : validation après une pause

    def set_value(self, v: float) -> None:
        self.slider.blockSignals(True)
        self.slider.setValue(int(round((v - self.lo) / self.step)))
        self.slider.blockSignals(False)
        self.value_label.setText(self.fmt(v))


class PowerButton(QtWidgets.QAbstractButton):
    """Bouton d'alimentation rond : ambre à l'arrêt, sarcelle lumineux en marche."""

    def __init__(self, parent=None):
        super().__init__(parent)
        self.on = False
        self.setFixedSize(46, 46)
        self.setCursor(Qt.CursorShape.PointingHandCursor)

    def set_on(self, on: bool):
        self.on = on
        self.setToolTip("Arrêter la simulation" if on else "Démarrer la simulation")
        self.update()

    def paintEvent(self, _e):
        p = QtGui.QPainter(self)
        p.setRenderHint(QtGui.QPainter.RenderHint.Antialiasing)
        color = C(TEAL) if self.on else C(AMBER)
        if not self.isEnabled():
            color = C(MUTED)
        r = QtCore.QRectF(5, 5, 36, 36)
        if self.on:
            glow = QtGui.QRadialGradient(r.center(), 24)
            glow.setColorAt(0.55, C(TEAL, 90))
            glow.setColorAt(1.0, C(TEAL, 0))
            p.setBrush(glow)
            p.setPen(Qt.PenStyle.NoPen)
            p.drawEllipse(QtCore.QRectF(0, 0, 46, 46))
        p.setBrush(C(PANEL2) if not self.underMouse() else C(BORDER_STRONG))
        p.setPen(QtGui.QPen(color, 2))
        p.drawEllipse(r)
        pen = QtGui.QPen(color, 2.4, Qt.PenStyle.SolidLine, Qt.PenCapStyle.RoundCap)
        p.setPen(pen)
        p.setBrush(Qt.BrushStyle.NoBrush)
        inner = QtCore.QRectF(15, 15, 16, 16)
        p.drawArc(inner, 120 * 16, -300 * 16)
        p.drawLine(QtCore.QPointF(23, 12), QtCore.QPointF(23, 22))


# ---------------------------------------------------------------------------
# Vue en plan de la pièce
# ---------------------------------------------------------------------------
class RoomPlan(QtWidgets.QWidget):
    """Vue de dessus : murs (traités ou non), matériaux, mobilier, enceintes, auditeur,
    son direct et premières réflexions. L'auditeur et les enceintes se déplacent."""

    committed = Signal(dict)

    def __init__(self, parent=None):
        super().__init__(parent)
        self.setMinimumSize(420, 340)
        self.setMouseTracking(True)
        self.s = None
        self.room: ae.RoomSpec | None = None
        self.preview: dict = {}
        self._drag: str | None = None
        self._scale = 1.0
        self._origin = QtCore.QPointF()

    def set_state(self, settings, room: ae.RoomSpec):
        self.s, self.room = settings, room
        self.preview = {}
        self.update()

    # -- géométrie ------------------------------------------------------------
    def _params(self):
        s = self.s
        return {"speaker_angle": self.preview.get("speaker_angle", s.speaker_angle),
                "speaker_distance": self.preview.get("speaker_distance", s.speaker_distance),
                "front_gap": self.preview.get("front_gap", s.front_gap)}

    def _layout(self):
        prm = self._params()
        room = self.room
        if "front_gap" in self.preview:
            from dataclasses import replace
            room = replace(room, front_gap=prm["front_gap"])
        try:
            return room, ae.stereo_layout(room, prm["speaker_angle"], prm["speaker_distance"])
        except ae.AudioEngineError:
            return room, None

    def _to_screen(self, x: float, y: float) -> QtCore.QPointF:
        # Mur avant (x = L) en haut, gauche de l'auditeur (+y) à gauche de l'écran.
        return QtCore.QPointF(self._origin.x() + (self.room.width - y) * self._scale,
                              self._origin.y() + (self.room.length - x) * self._scale)

    def _to_room(self, pt: QtCore.QPointF) -> tuple[float, float]:
        y = self.room.width - (pt.x() - self._origin.x()) / self._scale
        x = self.room.length - (pt.y() - self._origin.y()) / self._scale
        return x, y

    def _fit(self):
        m = 38
        w, h = self.width() - 2 * m, self.height() - 2 * m - 18
        self._scale = max(10.0, min(w / self.room.width, h / self.room.length))
        rw, rh = self.room.width * self._scale, self.room.length * self._scale
        self._origin = QtCore.QPointF((self.width() - rw) / 2, m + (h - rh) / 2)

    # -- dessin ---------------------------------------------------------------
    def paintEvent(self, _e):
        p = QtGui.QPainter(self)
        p.setRenderHint(QtGui.QPainter.RenderHint.Antialiasing)
        p.fillRect(self.rect(), C(BG))
        if self.room is None:
            return
        self._fit()
        room, layout = self._layout()
        sc = self._scale
        W, L = room.width, room.length
        tl = self._to_screen(L, W)
        rect = QtCore.QRectF(tl, self._to_screen(0, 0))

        # Sol
        p.setPen(Qt.PenStyle.NoPen)
        p.setBrush(C(PANEL))
        p.drawRect(rect)
        p.save()
        p.setClipRect(rect)
        floor = room.floor
        if floor == "tile":
            p.setPen(QtGui.QPen(C(TEXT, 14), 1))
            step = 0.5 * sc
            x = rect.left()
            while x < rect.right():
                p.drawLine(QtCore.QPointF(x, rect.top()), QtCore.QPointF(x, rect.bottom()))
                x += step
            y = rect.top()
            while y < rect.bottom():
                p.drawLine(QtCore.QPointF(rect.left(), y), QtCore.QPointF(rect.right(), y))
                y += step
        elif floor in ("wood", "rug"):
            p.setPen(QtGui.QPen(C(AMBER, 16), 1))
            y = rect.top()
            while y < rect.bottom():
                p.drawLine(QtCore.QPointF(rect.left(), y), QtCore.QPointF(rect.right(), y))
                y += 0.18 * sc
        elif floor == "carpet":
            p.fillRect(rect, C(TEAL, 18))
        p.restore()

        lis = layout.listener if layout is not None else None
        if floor == "rug" and lis is not None:
            c = self._to_screen(lis[0] + 0.6, W / 2)
            rug = QtCore.QRectF(c.x() - 1.3 * sc, c.y() - 1.0 * sc, 2.6 * sc, 2.0 * sc)
            p.setPen(QtGui.QPen(C(TEAL, 60), 1))
            p.setBrush(C(TEAL, 22))
            p.drawRoundedRect(rug, 8, 8)

        # Mobilier
        if room.furniture in ("furnished", "full") and lis is not None:
            sofa_x = max(0.45, lis[0] - 0.55)
            c = self._to_screen(sofa_x, W / 2)
            sofa = QtCore.QRectF(c.x() - 1.0 * sc, c.y() - 0.05 * sc, 2.0 * sc, 0.75 * sc)
            p.setPen(QtGui.QPen(C(BORDER_STRONG), 1))
            p.setBrush(C(PANEL2))
            p.drawRoundedRect(sofa, 6, 6)
        if room.furniture == "full":
            p.setBrush(C(PANEL2))
            p.setPen(QtGui.QPen(C(BORDER_STRONG), 1))
            a = self._to_screen(L * 0.55, 0.38)
            p.drawRect(QtCore.QRectF(a.x(), a.y(), 0.36 * sc, min(2.2, L * 0.4) * sc))
            b = self._to_screen(L * 0.35, W)
            p.drawRoundedRect(QtCore.QRectF(b.x(), b.y(), 0.9 * sc, 0.9 * sc), 5, 5)

        # Nuage acoustique (au plafond, au-dessus de la zone d'écoute)
        if room.ceiling == "cloud" and layout is not None:
            sp = layout.speakers["L"]
            a = self._to_screen(sp[0] - 0.1, W / 2 + 1.2)
            b = self._to_screen(lis[0] - 0.2, W / 2 - 1.2)
            pen = QtGui.QPen(C(TEAL, 140), 1.2, Qt.PenStyle.DashLine)
            p.setPen(pen)
            p.setBrush(C(TEAL, 14))
            p.drawRoundedRect(QtCore.QRectF(a, b).normalized(), 10, 10)
            p.setPen(C(TEAL, 170))
            p.setFont(QtGui.QFont(MONO, 8))
            p.drawText(QtCore.QRectF(a, b).normalized().adjusted(6, 4, 0, 0),
                       Qt.AlignmentFlag.AlignLeft | Qt.AlignmentFlag.AlignTop, "nuage")

        # Murs
        p.setBrush(Qt.BrushStyle.NoBrush)
        p.setPen(QtGui.QPen(C(BORDER_STRONG), 6, Qt.PenStyle.SolidLine,
                            Qt.PenCapStyle.SquareCap, Qt.PenJoinStyle.MiterJoin))
        p.drawRect(rect)

        # Panneaux absorbants répartis sur chaque mur
        t = room.treatment
        if t > 0:
            p.setPen(Qt.PenStyle.NoPen)
            p.setBrush(C(TEAL, 220))
            walls = [(rect.topLeft(), rect.topRight(), W), (rect.bottomLeft(), rect.bottomRight(), W),
                     (rect.topLeft(), rect.bottomLeft(), L), (rect.topRight(), rect.bottomRight(), L)]
            for a, b, length in walls:
                n = max(1, round(t * length / 0.6))
                width = min(0.6, t * length / n)
                horizontal = abs(a.y() - b.y()) < 1
                for i in range(n):
                    centre = (i + 0.5) / n
                    cx = a.x() + (b.x() - a.x()) * centre
                    cy = a.y() + (b.y() - a.y()) * centre
                    half = width * sc / 2
                    r = (QtCore.QRectF(cx - half, cy - 5, 2 * half, 10) if horizontal
                         else QtCore.QRectF(cx - 5, cy - half, 10, 2 * half))
                    p.drawRoundedRect(r, 2, 2)

        # Pièges à basses d'angle
        if room.bass_traps > 0:
            size = (0.25 + 0.25 * room.bass_traps) * sc
            p.setBrush(C(TEAL, 150))
            p.setPen(Qt.PenStyle.NoPen)
            for corner, dx, dy in ((rect.topLeft(), 1, 1), (rect.topRight(), -1, 1),
                                   (rect.bottomLeft(), 1, -1), (rect.bottomRight(), -1, -1)):
                tri = QtGui.QPolygonF([corner, corner + QtCore.QPointF(dx * size, 0),
                                       corner + QtCore.QPointF(0, dy * size)])
                p.drawPolygon(tri)

        # Diffuseur sur le mur arrière
        if room.diffusion > 0:
            span = room.diffusion * rect.width() * 0.8
            x0 = rect.center().x() - span / 2
            y = rect.bottom() - 4
            teeth = max(3, int(span / 9))
            path = QtGui.QPainterPath(QtCore.QPointF(x0, y))
            for i in range(teeth):
                xa = x0 + span * (i + 0.5) / teeth
                xb = x0 + span * (i + 1) / teeth
                path.lineTo(xa, y - 7 - 5 * ((i * 7) % 3))
                path.lineTo(xb, y)
            p.setPen(QtGui.QPen(C(AMBER, 200), 1.4))
            p.drawPath(path)

        if layout is None:
            p.setPen(C(RED))
            p.drawText(rect, Qt.AlignmentFlag.AlignCenter,
                       "Placement impossible dans cette pièce")
            self._labels(p, rect, room)
            return

        # Rayons : premières réflexions (murs) — seulement si elles sont simulées — puis son direct
        beta = room.reflection_bands()
        surfaces_2d = {"rear": 0, "front": 1, "right": 2, "left": 3}
        reflections_on = (self.s.room_enabled and self.s.room_amount > 0
                          and self.s.reflection_order >= 1)
        if not reflections_on:
            p.setPen(C(RED, 220))
            p.setFont(QtGui.QFont(MONO, 9))
            p.drawText(QtCore.QRectF(rect.left(), rect.center().y() + 0.9 * sc, rect.width(), 18),
                       Qt.AlignmentFlag.AlignCenter,
                       "pièce désactivée" if not self.s.room_enabled or self.s.room_amount <= 0
                       else "réflexions désactivées (ordre : aucune)")
        for side in (("L", "R") if reflections_on else ()):
            src = layout.speakers[side]
            for img in ae.image_sources(src, room, 1):
                if img.reflections != 1:
                    continue
                surf = ae.SURFACES[img.counts.index(1)]
                if surf not in surfaces_2d:
                    continue
                gain = float(img.gain_bands(beta)[3])  # 1 kHz
                lx, ly = lis[0], lis[1]
                ix, iy = img.position[0], img.position[1]
                if surf in ("rear", "front"):
                    wx = 0.0 if surf == "rear" else L
                    k = (wx - lx) / (ix - lx) if ix != lx else 0
                    hit = (wx, ly + k * (iy - ly))
                else:
                    wy = 0.0 if surf == "right" else W
                    k = (wy - ly) / (iy - ly) if iy != ly else 0
                    hit = (lx + k * (ix - lx), wy)
                pen = QtGui.QPen(C(AMBER, int(30 + 170 * gain ** 2)), 1.2, Qt.PenStyle.DashLine)
                p.setPen(pen)
                p.setBrush(Qt.BrushStyle.NoBrush)
                path = QtGui.QPainterPath(self._to_screen(src[0], src[1]))
                path.lineTo(self._to_screen(*hit))
                path.lineTo(self._to_screen(lx, ly))
                p.drawPath(path)
                p.setBrush(C(AMBER, int(60 + 160 * gain)))
                p.setPen(Qt.PenStyle.NoPen)
                p.drawEllipse(self._to_screen(*hit), 3, 3)
        p.setPen(QtGui.QPen(C(AMBER), 2))
        for side in ("L", "R"):
            p.drawLine(self._to_screen(*layout.speakers[side][:2]), self._to_screen(*lis[:2]))

        # Enceintes orientées vers l'auditeur
        for side in ("L", "R"):
            sp = layout.speakers[side]
            c = self._to_screen(sp[0], sp[1])
            head = self._to_screen(lis[0], lis[1])
            angle = math.degrees(math.atan2(head.y() - c.y(), head.x() - c.x())) - 90
            p.save()
            p.translate(c)
            p.rotate(angle)
            box = QtCore.QRectF(-0.13 * sc, -0.12 * sc, 0.26 * sc, 0.24 * sc)
            hot = self._drag == side
            p.setBrush(C(AMBER) if not hot else C("#f0b25a"))
            p.setPen(QtGui.QPen(C(BG), 1.5))
            p.drawRoundedRect(box, 4, 4)
            p.setBrush(C(BG))
            p.drawEllipse(QtCore.QPointF(0, 0.035 * sc), 0.07 * sc, 0.07 * sc)
            p.restore()
            p.setPen(C(AMBER))
            p.setFont(QtGui.QFont(MONO, 9, QtGui.QFont.Weight.Bold))
            off = -0.32 * sc if side == "L" else 0.32 * sc
            p.drawText(QtCore.QRectF(c.x() + off - 12, c.y() - 8, 24, 16),
                       Qt.AlignmentFlag.AlignCenter, side)

        # Auditeur : tête, nez vers l'avant
        hc = self._to_screen(lis[0], lis[1])
        r = 0.16 * sc
        if self._drag == "listener":
            glow = QtGui.QRadialGradient(hc, r * 2.4)
            glow.setColorAt(0, C(TEAL, 80))
            glow.setColorAt(1, C(TEAL, 0))
            p.setBrush(glow)
            p.setPen(Qt.PenStyle.NoPen)
            p.drawEllipse(hc, r * 2.4, r * 2.4)
        p.setBrush(C(TEAL))
        p.setPen(Qt.PenStyle.NoPen)
        p.drawPolygon(QtGui.QPolygonF([hc + QtCore.QPointF(-r * 0.35, -r * 0.85),
                                       hc + QtCore.QPointF(r * 0.35, -r * 0.85),
                                       hc + QtCore.QPointF(0, -r * 1.35)]))
        p.drawEllipse(hc + QtCore.QPointF(-r, 0), r * 0.25, r * 0.4)
        p.drawEllipse(hc + QtCore.QPointF(r, 0), r * 0.25, r * 0.4)
        p.setBrush(C(BG))
        p.setPen(QtGui.QPen(C(TEAL), 2))
        p.drawEllipse(hc, r, r)

        # Cote du triangle stéréo
        prm = self._params()
        mid = (self._to_screen(*layout.speakers["L"][:2]) + hc) / 2
        p.setPen(C(TEXT, 210))
        p.setFont(QtGui.QFont(MONO, 9))
        p.drawText(QtCore.QRectF(mid.x() - 120, mid.y() - 8, 110, 16),
                   Qt.AlignmentFlag.AlignRight | Qt.AlignmentFlag.AlignVCenter,
                   f"{fr_num(prm['speaker_distance'], 2)} m · {prm['speaker_angle']:.0f}°")
        self._labels(p, rect, room)

    def _labels(self, p: QtGui.QPainter, rect: QtCore.QRectF, room: ae.RoomSpec):
        p.setFont(QtGui.QFont(MONO, 9))
        p.setPen(C(MUTED))
        wide = self.width()
        p.drawText(QtCore.QRectF(rect.center().x() - wide / 2, rect.top() - 26, wide, 16),
                   Qt.AlignmentFlag.AlignCenter, f"mur avant · {fr_num(room.width)} m")
        p.drawText(QtCore.QRectF(rect.center().x() - wide / 2, rect.bottom() + 8, wide, 16),
                   Qt.AlignmentFlag.AlignCenter,
                   f"{fr_num(room.length)} m × {fr_num(room.width)} m × h {fr_num(room.height)} m"
                   f"  ·  {fr_num(room.volume, 0)} m³")
        p.save()
        p.translate(rect.left() - 22, rect.center().y())
        p.rotate(-90)
        p.drawText(QtCore.QRectF(-80, -8, 160, 16), Qt.AlignmentFlag.AlignCenter,
                   f"{fr_num(room.length)} m")
        p.restore()
        p.setPen(C(MUTED, 160))
        p.drawText(QtCore.QRectF(0, self.height() - 18, self.width() - 8, 16),
                   Qt.AlignmentFlag.AlignRight,
                   "glissez l'auditeur ou les enceintes  ·  — direct  ┄ réflexions")

    # -- interaction ----------------------------------------------------------
    def _hit(self, pos: QtCore.QPointF) -> str | None:
        if self.room is None:
            return None
        _room, layout = self._layout()
        if layout is None:
            return None
        targets = {"listener": layout.listener, "L": layout.speakers["L"], "R": layout.speakers["R"]}
        for name, pt in targets.items():
            sp = self._to_screen(pt[0], pt[1])
            if math.hypot(sp.x() - pos.x(), sp.y() - pos.y()) < max(16, 0.25 * self._scale):
                return name
        return None

    def mousePressEvent(self, e):
        hit = self._hit(e.position())
        if hit and e.button() == Qt.MouseButton.LeftButton:
            self._drag = hit
            self.setCursor(Qt.CursorShape.ClosedHandCursor)
            self.update()

    def mouseMoveEvent(self, e):
        if not self._drag:
            self.setCursor(Qt.CursorShape.OpenHandCursor if self._hit(e.position())
                           else Qt.CursorShape.ArrowCursor)
            return
        room, layout = self._layout()
        x, y = self._to_room(e.position())
        prm = self._params()
        a = math.radians(prm["speaker_angle"])
        sx = room.length - prm["front_gap"]
        dy = prm["speaker_distance"] * math.sin(a)
        lx = sx - prm["speaker_distance"] * math.cos(a)
        if self._drag == "listener":
            lx = min(max(x, 0.3), sx - 0.3)
        else:
            sx = min(max(x, lx + 0.3), room.length - 0.2)
            dy = min(max(abs(y - room.width / 2), 0.15), room.width / 2 - 0.25)
        dx = sx - lx
        dist = min(max(math.hypot(dx, dy), 0.5), 6.0)
        angle = min(max(math.degrees(math.atan2(dy, dx)), 10.0), 60.0)
        self.preview = {"speaker_distance": round(dist, 2), "speaker_angle": round(angle),
                        "front_gap": round(room.length - sx, 2)}
        self.update()

    def mouseReleaseEvent(self, _e):
        if self._drag:
            self._drag = None
            self.setCursor(Qt.CursorShape.OpenHandCursor)
            if self.preview:
                self.committed.emit(dict(self.preview))
            self.update()


# ---------------------------------------------------------------------------
# Courbes : réponse en fréquence et RT60 par octave
# ---------------------------------------------------------------------------
class ResponsePlot(QtWidgets.QWidget):
    COLORS = (AMBER, TEAL, TEXT)
    NAMES = ("enceinte", "casque", "oreille")

    def __init__(self, parent=None):
        super().__init__(parent)
        self.setMinimumHeight(170)
        self.freqs = None
        self.curves: dict[str, object] = {}

    def set_data(self, freqs, curves: dict) -> None:
        self.freqs, self.curves = freqs, curves
        self.update()

    def paintEvent(self, _e):
        p = QtGui.QPainter(self)
        p.setRenderHint(QtGui.QPainter.RenderHint.Antialiasing)
        r = QtCore.QRectF(self.rect()).adjusted(36, 22, -8, -20)
        p.setFont(QtGui.QFont(MONO, 8))
        if self.freqs is None or not self.curves:
            p.setPen(C(MUTED))
            p.drawText(self.rect(), Qt.AlignmentFlag.AlignCenter,
                       "« Appliquer » pour calculer la réponse simulée")
            return
        lo_db, hi_db = -24.0, 12.0

        def x_of(f):
            return r.left() + r.width() * (math.log10(f) - math.log10(20)) / 3.0

        def y_of(db):
            return r.bottom() - r.height() * (min(max(db, lo_db), hi_db) - lo_db) / (hi_db - lo_db)

        for f, txt in ((20, "20"), (100, "100"), (1000, "1k"), (10000, "10k"), (20000, "20k")):
            p.setPen(C(BORDER))
            p.drawLine(QtCore.QPointF(x_of(f), r.top()), QtCore.QPointF(x_of(f), r.bottom()))
            p.setPen(C(MUTED))
            p.drawText(QtCore.QRectF(x_of(f) - 18, r.bottom() + 4, 36, 14),
                       Qt.AlignmentFlag.AlignCenter, txt)
        for db in (-24, -12, 0, 12):
            p.setPen(C(BORDER_STRONG if db == 0 else BORDER))
            p.drawLine(QtCore.QPointF(r.left(), y_of(db)), QtCore.QPointF(r.right(), y_of(db)))
            p.setPen(C(MUTED))
            p.drawText(QtCore.QRectF(0, y_of(db) - 7, 30, 14),
                       Qt.AlignmentFlag.AlignRight | Qt.AlignmentFlag.AlignVCenter, f"{db:+d}")
        for i, values in enumerate(self.curves.values()):
            band = [v for f, v in zip(self.freqs, values) if 200 <= f <= 2000]
            offset = sum(band) / len(band) if band else 0.0
            path = QtGui.QPainterPath()
            for j, (f, v) in enumerate(zip(self.freqs, values)):
                pt = QtCore.QPointF(x_of(f), y_of(v - offset))
                path.moveTo(pt) if j == 0 else path.lineTo(pt)
            p.setPen(QtGui.QPen(C(self.COLORS[i % 3], 230), 1.8))
            p.setBrush(Qt.BrushStyle.NoBrush)
            p.drawPath(path)
        x = r.left()
        fm = p.fontMetrics()
        for i, name in enumerate(self.curves):
            short = self.NAMES[i] if i < len(self.NAMES) else name
            p.fillRect(QtCore.QRectF(x, 8, 12, 2), C(self.COLORS[i % 3]))
            p.setPen(C(TEXT, 210))
            p.drawText(QtCore.QPointF(x + 16, 12), short)
            x += 16 + fm.horizontalAdvance(short) + 16


class RtMeter(QtWidgets.QWidget):
    """RT60 par bande d'octave, façon vu-mètre ; zone conseillée en sarcelle."""

    BANDS = ("125", "250", "500", "1k", "2k", "4k", "8k")

    def __init__(self, parent=None):
        super().__init__(parent)
        self.setMinimumHeight(170)
        self.values = np.zeros(7)

    def set_values(self, values: np.ndarray):
        self.values = np.asarray(values, float)
        self.update()

    def paintEvent(self, _e):
        p = QtGui.QPainter(self)
        p.setRenderHint(QtGui.QPainter.RenderHint.Antialiasing)
        r = QtCore.QRectF(self.rect()).adjusted(8, 18, -8, -20)
        top = max(1.0, float(np.max(self.values)) * 1.15)

        def y_of(t):
            return r.bottom() - r.height() * min(t, top) / top

        # Zone conseillée pour l'écoute critique (0,2–0,4 s)
        p.fillRect(QtCore.QRectF(r.left(), y_of(0.4), r.width(), y_of(0.2) - y_of(0.4)), C(TEAL, 26))
        p.setPen(QtGui.QPen(C(TEAL, 90), 1, Qt.PenStyle.DashLine))
        p.drawLine(QtCore.QPointF(r.left(), y_of(0.4)), QtCore.QPointF(r.right(), y_of(0.4)))
        n = len(self.values)
        slot = r.width() / n
        p.setFont(QtGui.QFont(MONO, 8))
        for i, v in enumerate(self.values):
            x = r.left() + i * slot + slot * 0.22
            w = slot * 0.56
            segs = 14
            lit = int(round(segs * min(v, top) / top))
            for k in range(segs):
                seg_h = r.height() / segs
                y = r.bottom() - (k + 1) * seg_h + 1.5
                level = (k + 1) / segs * top
                if k < lit:
                    color = C(TEAL) if level <= 0.45 else C(AMBER) if level <= 0.8 else C(RED)
                else:
                    color = C(BORDER)
                p.fillRect(QtCore.QRectF(x, y, w, seg_h - 3), color)
            p.setPen(C(TEXT, 220))
            p.drawText(QtCore.QRectF(x - 10, r.top() - 17, w + 20, 14), Qt.AlignmentFlag.AlignCenter,
                       fr_num(v, 2))
            p.setPen(C(MUTED))
            p.drawText(QtCore.QRectF(x - 10, r.bottom() + 4, w + 20, 14), Qt.AlignmentFlag.AlignCenter,
                       self.BANDS[i])


# ---------------------------------------------------------------------------
# Panneaux latéraux
# ---------------------------------------------------------------------------
class WordFilterProxy(QtCore.QSortFilterProxyModel):
    def __init__(self, parent=None):
        super().__init__(parent)
        self._patterns = []

    def set_query(self, text: str) -> None:
        self._patterns = query_patterns(text)
        self.invalidateFilter()

    def filterAcceptsRow(self, row, parent):
        if not self._patterns:
            return True
        idx = self.sourceModel().index(row, 0, parent)
        return matches(self._patterns, self.sourceModel().data(idx, Qt.ItemDataRole.UserRole + 1) or "")


class HeadphonePanel(QtWidgets.QWidget):
    selected = Signal(object)
    eq_toggled = Signal(bool)
    import_requested = Signal()
    remove_requested = Signal(object)

    def __init__(self, parent=None):
        super().__init__(parent)
        frame, lay = section("01", "Casque")
        outer = QtWidgets.QVBoxLayout(self)
        outer.setContentsMargins(0, 0, 0, 0)
        outer.addWidget(frame)
        self.current = label("Aucun casque", wrap=True)
        self.current.setStyleSheet(f"font-family: '{DISPLAY}'; font-size: 15px; font-weight: 600;")
        self.current_sub = label(muted=True, mono=True, wrap=True)
        lay.addWidget(self.current)
        lay.addWidget(self.current_sub)
        self.search = QtWidgets.QLineEdit()
        self.search.setPlaceholderText("Rechercher parmi 8 850 mesures…")
        self.search.setClearButtonEnabled(True)
        self.search.setEnabled(False)
        lay.addWidget(self.search)
        self.model = QtGui.QStandardItemModel(self)
        self.proxy = WordFilterProxy(self)
        self.proxy.setSourceModel(self.model)
        self.view = QtWidgets.QListView()
        self.view.setModel(self.proxy)
        self.view.setUniformItemSizes(True)
        self.view.setHorizontalScrollBarPolicy(Qt.ScrollBarPolicy.ScrollBarAlwaysOff)
        self.view.setEditTriggers(QtWidgets.QAbstractItemView.EditTrigger.NoEditTriggers)
        self.view.setMinimumHeight(150)
        self.view.setContextMenuPolicy(Qt.ContextMenuPolicy.CustomContextMenu)
        lay.addWidget(self.view, 1)
        self.count = caption("chargement de l'index autoeq…")
        lay.addWidget(self.count)
        row = QtWidgets.QHBoxLayout()
        self.eq_enabled = QtWidgets.QCheckBox("Correction")
        self.eq_enabled.setToolTip("Appliquer la correction du casque (AutoEq ou import)")
        import_btn = QtWidgets.QPushButton("Importer…")
        import_btn.setToolTip("Votre mesure (fréquence, dB : REW, Squiglink, CSV AutoEq)\n"
                              "ou un EQ AutoEq / Equalizer APO (ParametricEQ, GraphicEQ)")
        row.addWidget(self.eq_enabled, 1)
        row.addWidget(import_btn)
        lay.addLayout(row)

        self._timer = QtCore.QTimer(self, singleShot=True, interval=150)
        self._timer.timeout.connect(self._apply_filter)
        self.search.textChanged.connect(lambda _t: self._timer.start())
        self.view.clicked.connect(self._on_clicked)
        self.view.customContextMenuRequested.connect(self._context_menu)
        self.eq_enabled.toggled.connect(self.eq_toggled.emit)
        import_btn.clicked.connect(self.import_requested.emit)

    def set_entries(self, entries: list[HeadphoneEntry]) -> None:
        self.model.clear()
        for e in entries:
            item = QtGui.QStandardItem(("★ " if e.is_import else "") + e.name)
            item.setToolTip(e.label)
            item.setData(e, Qt.ItemDataRole.UserRole)
            item.setData(f"{e.name} {e.source} {e.rig}".lower(), Qt.ItemDataRole.UserRole + 1)
            self.model.appendRow(item)
        self.search.setEnabled(True)
        self._apply_filter()

    def set_error(self, message: str) -> None:
        self.count.setText(f"INDEX INDISPONIBLE : {message}")

    def reveal(self, entry: HeadphoneEntry | None) -> None:
        """Sélectionne et montre le casque courant dans la liste."""
        if entry is None:
            return
        for row in range(self.proxy.rowCount()):
            idx = self.proxy.index(row, 0)
            e = self.proxy.data(idx, Qt.ItemDataRole.UserRole)
            if e is not None and e.path == entry.path:
                self.view.setCurrentIndex(idx)
                self.view.scrollTo(idx, QtWidgets.QAbstractItemView.ScrollHint.PositionAtCenter)
                return

    def _apply_filter(self):
        self.proxy.set_query(self.search.text())
        self.count.setText(f"{self.proxy.rowCount()} / {self.model.rowCount()} mesures".upper())

    def _on_clicked(self, index):
        entry = self.proxy.data(index, Qt.ItemDataRole.UserRole)
        if entry is not None:
            self.show_current(entry)
            self.selected.emit(entry)

    def _context_menu(self, pos):
        entry = self.proxy.data(self.view.indexAt(pos), Qt.ItemDataRole.UserRole)
        if entry is None or not entry.is_import:
            return
        menu = QtWidgets.QMenu(self)
        remove = menu.addAction("Supprimer cet import")
        if menu.exec(self.view.viewport().mapToGlobal(pos)) == remove:
            self.remove_requested.emit(entry)

    def show_current(self, entry: HeadphoneEntry | None):
        if entry is None:
            self.current.setText("Aucun casque")
            self.current_sub.setText("SANS CORRECTION")
        else:
            self.current.setText(entry.name)
            self.current_sub.setText(f"{entry.source} · {entry.rig}".upper())


class SpeakerPanel(QtWidgets.QWidget):
    selected = Signal(str)

    def __init__(self, speakers: list[dict], parent=None):
        super().__init__(parent)
        n_studio = sum(sp.get("category") == "studio" for sp in speakers)
        frame, lay = section("02", "Enceintes")
        outer = QtWidgets.QVBoxLayout(self)
        outer.setContentsMargins(0, 0, 0, 0)
        outer.addWidget(frame)
        self.current = label(wrap=True)
        self.current.setStyleSheet(f"font-family: '{DISPLAY}'; font-size: 15px; font-weight: 600;")
        self.current_sub = label(muted=True, mono=True, wrap=True)
        lay.addWidget(self.current)
        lay.addWidget(self.current_sub)
        self.search = QtWidgets.QLineEdit()
        self.search.setPlaceholderText(f"Filtrer {len(speakers)} modèles…")
        self.search.setClearButtonEnabled(True)
        lay.addWidget(self.search)
        self.category = Segmented([("", "Toutes"), ("studio", f"Studio · {n_studio}"),
                                   ("hifi", f"Hi-Fi · {len(speakers) - n_studio}")])
        self.category.set_value("")
        lay.addWidget(self.category)
        self.list = QtWidgets.QListWidget()
        self.list.setMinimumHeight(150)
        self.list.setHorizontalScrollBarPolicy(Qt.ScrollBarPolicy.ScrollBarAlwaysOff)
        self.by_id = {}
        for spk in speakers:
            studio = spk.get("category") == "studio"
            item = QtWidgets.QListWidgetItem(f"{spk['brand']} {spk['model']}")
            item.setData(Qt.ItemDataRole.UserRole, spk["id"])
            item.setData(Qt.ItemDataRole.UserRole + 1, spk.get("category", "hifi"))
            kind = {"bookshelves": "bibliothèque", "floorstanders": "colonne",
                    "center": "centrale"}.get(spk.get("shape"), spk.get("shape", ""))
            computed = " (CEA-2034 calculé depuis les mesures polaires)" \
                if spk.get("format") == "polar" else ""
            item.setToolTip(f"{'moniteur de studio' if studio else 'hi-fi'} · {kind} · "
                            f"{spk.get('type', '')}\nscore Olive {spk.get('pref_score')} "
                            f"({spk.get('pref_score_wsub')} avec caisson) · LFX {spk.get('lfx_hz')} Hz"
                            f"\nmesure : {spk.get('origin', 'ASR')}, Klippel NFS{computed}")
            self.list.addItem(item)
            self.by_id[spk["id"]] = spk
        lay.addWidget(self.list, 1)
        self.search.textChanged.connect(lambda _t: self._filter())
        self.category.changed.connect(lambda _v: self._filter())
        self.list.currentItemChanged.connect(self._changed)

    def _filter(self):
        patterns = query_patterns(self.search.text())
        wanted = self.category.values[self.category.group.checkedId()] \
            if self.category.group.checkedId() >= 0 else ""
        for i in range(self.list.count()):
            item = self.list.item(i)
            ok = matches(patterns, item.text()) and (
                not wanted or item.data(Qt.ItemDataRole.UserRole + 1) == wanted)
            item.setHidden(not ok)

    def _changed(self, current, _previous):
        if current is not None:
            sid = current.data(Qt.ItemDataRole.UserRole)
            self.show_current(sid)
            self.selected.emit(sid)

    def show_current(self, speaker_id: str):
        spk = self.by_id.get(speaker_id)
        if not spk:
            return
        self.current.setText(f"{spk['brand']} {spk['model']}")
        cat = "studio" if spk.get("category") == "studio" else "hi-fi"
        score = spk.get("pref_score")
        self.current_sub.setText(f"{cat} · score {score if score is not None else '—'} · "
                                 f"lfx {spk.get('lfx_hz')} hz".upper())

    def select_id(self, speaker_id: str):
        self.show_current(speaker_id)
        for i in range(self.list.count()):
            item = self.list.item(i)
            if item.data(Qt.ItemDataRole.UserRole) == speaker_id:
                self.list.blockSignals(True)
                self.list.setCurrentItem(item)
                self.list.blockSignals(False)
                self.list.scrollToItem(item)
                return


class RoomPanel(QtWidgets.QWidget):
    """03 Salle, 04 Rendu : tous les réglages de la pièce et du rendu."""

    changed = Signal(dict)          # réglages validés
    preview = Signal(dict)          # aperçu pendant un glissement
    preset_chosen = Signal(str)
    enable_room = Signal()

    def __init__(self, parent=None):
        super().__init__(parent)
        outer = QtWidgets.QVBoxLayout(self)
        outer.setContentsMargins(0, 0, 0, 0)
        outer.setSpacing(14)

        # 03 Salle
        frame, lay = section("03", "Salle")
        outer.addWidget(frame)
        self.warning = QtWidgets.QFrame()
        self.warning.setStyleSheet(f"QFrame {{ background: rgba(229,72,77,28); border: 1px solid "
                                   f"rgba(229,72,77,110); border-radius: 9px; }} "
                                   f"QLabel {{ background: transparent; border: none; }}")
        wl = QtWidgets.QHBoxLayout(self.warning)
        wl.setContentsMargins(10, 8, 8, 8)
        self.warning_text = label(wrap=True)
        self.warning_text.setStyleSheet(f"color: {TEXT}; font-size: 12px;")
        enable = QtWidgets.QPushButton("Activer")
        enable.setObjectName("primary")
        enable.clicked.connect(self.enable_room.emit)
        wl.addWidget(self.warning_text, 1)
        wl.addWidget(enable)
        lay.addWidget(self.warning)
        lay.addWidget(caption("préréglages"))
        grid = QtWidgets.QGridLayout()
        grid.setSpacing(6)
        self.preset_group = QtWidgets.QButtonGroup(self)
        self.preset_group.setExclusive(True)
        self.preset_keys = list(ROOM_PRESETS)
        for i, key in enumerate(self.preset_keys):
            btn = QtWidgets.QPushButton(ROOM_PRESETS[key]["label"])
            btn.setObjectName("chip")
            btn.setCheckable(True)
            btn.setCursor(Qt.CursorShape.PointingHandCursor)
            self.preset_group.addButton(btn, i)
            grid.addWidget(btn, i // 2, i % 2)
        self.preset_group.idClicked.connect(lambda i: self.preset_chosen.emit(self.preset_keys[i]))
        lay.addLayout(grid)
        self.custom = caption("")
        lay.addWidget(self.custom)

        self.sliders: dict[str, ValueSlider] = {}

        def add_slider(layout, key, text, lo, hi, step, fmt, accent=""):
            w = ValueSlider(text, lo, hi, step, fmt, accent)
            w.committed.connect(lambda v, k=key: self.changed.emit({k: v}))
            w.changed.connect(lambda v, k=key: self.preview.emit({k: v}))
            self.sliders[key] = w
            layout.addWidget(w)
            return w

        pct = lambda v: f"{v * 100:.0f} %"
        meters = lambda v: f"{fr_num(v)} m"
        lay.addWidget(caption("traitement acoustique"))
        add_slider(lay, "room_treatment", "Murs couverts de panneaux absorbants", 0, 1, 0.05, pct,
                   accent="teal")
        self.segs: dict[str, Segmented] = {}

        def add_seg(layout, key, text, options):
            if text:
                layout.addWidget(label(text, muted=True))
            seg = Segmented(options)
            seg.changed.connect(lambda v, k=key: self.changed.emit({k: v}))
            self.segs[key] = seg
            layout.addWidget(seg)

        add_seg(lay, "room_bass_traps", "Pièges à basses (angles)",
                [(0.0, "Aucun"), (0.5, "Quelques-uns"), (1.0, "Complet")])
        add_slider(lay, "room_diffusion", "Diffusion du mur arrière", 0, 1, 0.05, pct)
        add_seg(lay, "room_ceiling", "Plafond", list(ae.CEILINGS.items()))

        lay.addWidget(caption("matériaux"))
        add_seg(lay, "room_walls", "Murs", [("masonry", "Maçonnerie"), ("drywall", "Placo")])
        add_seg(lay, "room_floor", "Sol", [("tile", "Carrelage"), ("wood", "Parquet"),
                                           ("rug", "Tapis"), ("carpet", "Moquette")])
        add_seg(lay, "room_furniture", "Mobilier", list(ae.FURNITURE.items()))

        lay.addWidget(caption("dimensions"))
        add_slider(lay, "room_length", "Longueur", 2, 15, 0.1, meters)
        add_slider(lay, "room_width", "Largeur", 2, 12, 0.1, meters)
        add_slider(lay, "room_height", "Hauteur sous plafond", 2, 6, 0.1, meters)

        lay.addWidget(caption("placement"))
        add_slider(lay, "speaker_angle", "Angle des enceintes", 10, 60, 1, lambda v: f"{v:.0f}°")
        add_slider(lay, "speaker_distance", "Distance d'écoute", 0.5, 6, 0.05,
                   lambda v: f"{fr_num(v, 2)} m")
        add_slider(lay, "front_gap", "Enceintes – mur avant", 0.2, 3, 0.05,
                   lambda v: f"{fr_num(v, 2)} m")
        add_slider(lay, "ear_height", "Hauteur des oreilles", 0.8, 1.8, 0.05,
                   lambda v: f"{fr_num(v, 2)} m")

        # 04 Rendu
        frame2, lay2 = section("04", "Rendu")
        outer.addWidget(frame2)
        self.room_enabled = QtWidgets.QCheckBox("Simuler la pièce")
        self.room_enabled.toggled.connect(lambda v: self.changed.emit({"room_enabled": v}))
        lay2.addWidget(self.room_enabled)
        add_slider(lay2, "room_amount", "Quantité de pièce", 0, 1.5, 0.05, pct)
        add_seg(lay2, "reflection_order", "Ordre des réflexions",
                [(0, "Aucune"), (1, "1"), (2, "2"), (3, "3")])
        self.segs["reflection_order"].setToolTip(
            "Nombre de rebonds simulés pour chaque réflexion précoce.\n"
            "« Aucune » : seule la réverbération tardive reste ; le mur avant,\n"
            "la hauteur des oreilles et la diffusion n'ont alors plus d'effet.")
        self.rt_auto = QtWidgets.QCheckBox("RT60 calculé depuis les matériaux")
        self.rt_auto.toggled.connect(self._rt_auto_toggled)
        lay2.addWidget(self.rt_auto)
        add_slider(lay2, "rt60", "RT60 imposé (500 Hz – 1 kHz)", 0.1, 2, 0.05,
                   lambda v: f"{fr_num(v, 2)} s")
        self.df_eq = QtWidgets.QCheckBox("Égalisation champ diffus du HRTF")
        self.df_eq.toggled.connect(lambda v: self.changed.emit({"diffuse_field_eq": v}))
        lay2.addWidget(self.df_eq)
        add_slider(lay2, "preamp_db", "Préampli", -24, 12, 0.5, lambda v: f"{v:+.1f} dB")
        lay2.addWidget(label("Fréquence d'échantillonnage", muted=True))
        self.rate = QtWidgets.QComboBox()
        for text, value in (("Auto (horloge PipeWire / 48 kHz)", 0), ("44,1 kHz", 44100),
                            ("48 kHz", 48000), ("96 kHz", 96000)):
            self.rate.addItem(text, value)
        self.rate.activated.connect(
            lambda i: self.changed.emit({"sample_rate": int(self.rate.itemData(i))}))
        lay2.addWidget(self.rate)

    def _rt_auto_toggled(self, on: bool):
        self.sliders["rt60"].setEnabled(not on)
        self.changed.emit({"rt60_auto": on})

    def load(self, s) -> None:
        for key, w in self.sliders.items():
            w.set_value(getattr(s, key))
        for key, seg in self.segs.items():
            seg.set_value(getattr(s, key))
        for w, v in ((self.room_enabled, s.room_enabled), (self.rt_auto, s.rt60_auto),
                     (self.df_eq, s.diffuse_field_eq)):
            w.blockSignals(True)
            w.setChecked(v)
            w.blockSignals(False)
        self.sliders["rt60"].setEnabled(not s.rt60_auto)
        if not s.room_enabled or s.room_amount <= 0:
            self.warning_text.setText("La pièce n'est pas simulée : les réglages de salle "
                                      "n'ont aucun effet.")
            self.warning.setVisible(True)
        elif s.reflection_order == 0:
            self.warning_text.setText("Réflexions précoces désactivées : mur avant, hauteur "
                                      "des oreilles et diffusion sans effet.")
            self.warning.setVisible(True)
        else:
            self.warning.setVisible(False)
        idx = self.rate.findData(s.sample_rate)
        self.rate.setCurrentIndex(max(idx, 0))
        if s.room_preset in self.preset_keys:
            self.preset_group.button(self.preset_keys.index(s.room_preset)).setChecked(True)
            self.custom.setText("")
        else:
            self.preset_group.setExclusive(False)
            for b in self.preset_group.buttons():
                b.setChecked(False)
            self.preset_group.setExclusive(True)
            self.custom.setText("RÉGLAGES PERSONNALISÉS")


class OutputPanel(QtWidgets.QWidget):
    sink_changed = Signal(str)
    autostart_changed = Signal(bool)
    refresh_requested = Signal()

    def __init__(self, parent=None):
        super().__init__(parent)
        frame, lay = section("05", "Sortie")
        outer = QtWidgets.QVBoxLayout(self)
        outer.setContentsMargins(0, 0, 0, 0)
        outer.addWidget(frame)
        lay.addWidget(label("Casque (sortie physique)", muted=True))
        row = QtWidgets.QHBoxLayout()
        self.combo = QtWidgets.QComboBox()
        refresh = QtWidgets.QPushButton("↻")
        refresh.setFixedWidth(40)
        refresh.setToolTip("Actualiser la liste des sorties")
        row.addWidget(self.combo, 1)
        row.addWidget(refresh)
        lay.addLayout(row)
        self.autostart = QtWidgets.QCheckBox("Démarrer à la connexion")
        lay.addWidget(self.autostart)
        self.restore = caption("")
        self.restore.setWordWrap(True)
        lay.addWidget(self.restore)
        self.combo.activated.connect(lambda i: self.sink_changed.emit(self.combo.itemData(i) or ""))
        self.autostart.clicked.connect(self.autostart_changed.emit)
        refresh.clicked.connect(self.refresh_requested.emit)

    def set_sinks(self, sinks, current: str):
        self.combo.blockSignals(True)
        self.combo.clear()
        self.combo.addItem("Automatique (sortie par défaut)", "")
        for s in sinks:
            self.combo.addItem(s.label, s.name)
        self.combo.setCurrentIndex(max(self.combo.findData(current), 0))
        self.combo.blockSignals(False)


# ---------------------------------------------------------------------------
# Fenêtre principale
# ---------------------------------------------------------------------------
def scroll_wrap(widget: QtWidgets.QWidget, horizontal: bool = False) -> QtWidgets.QScrollArea:
    area = QtWidgets.QScrollArea()
    area.setWidgetResizable(True)
    area.setFrameShape(QtWidgets.QFrame.Shape.NoFrame)
    area.setHorizontalScrollBarPolicy(Qt.ScrollBarPolicy.ScrollBarAsNeeded if horizontal
                                      else Qt.ScrollBarPolicy.ScrollBarAlwaysOff)
    area.setWidget(widget)
    return area


class MainWindow(QtWidgets.QMainWindow):
    AUTO_DELAY_MS = 1500

    def __init__(self, app: SpeakerSimApp):
        super().__init__()
        self.app = app
        self.pool = QtCore.QThreadPool.globalInstance()
        # Références fortes : sans elles, le QObject des signaux d'un Worker peut être
        # détruit avant la livraison (file d'attente) de ``done``/``failed``.
        self._tasks: set[Worker] = set()
        self.busy = False
        self.dirty = False
        self.pending_apply = False
        self.setWindowTitle(APP_NAME)
        self.resize(1440, 900)

        root = QtWidgets.QWidget()
        root.setObjectName("root")
        self.setCentralWidget(root)
        main = QtWidgets.QVBoxLayout(root)
        main.setContentsMargins(0, 0, 0, 0)
        main.setSpacing(0)

        # Barre supérieure
        top = QtWidgets.QFrame()
        top.setObjectName("topbar")
        tl = QtWidgets.QHBoxLayout(top)
        tl.setContentsMargins(20, 10, 16, 10)
        tl.setSpacing(14)
        mark = label("<span style='color:%s'>●</span> speaker-sim" % TEAL, name="wordmark")
        tl.addWidget(mark)
        self.chain = label(name="chain")
        self.chain.setTextFormat(Qt.TextFormat.RichText)
        tl.addWidget(self.chain, 1)
        self.status_pill = label(name="status")
        tl.addWidget(self.status_pill)
        self.auto = QtWidgets.QCheckBox("Auto")
        self.auto.setToolTip("Appliquer automatiquement chaque changement pendant la simulation\n"
                             "(bascule sans coupure)")
        tl.addWidget(self.auto)
        self.export_btn = QtWidgets.QPushButton("Exporter")
        self.export_btn.setToolTip("Enregistrer les filtres (WAV, true stereo, Equalizer APO)")
        self.apply_btn = QtWidgets.QPushButton("Appliquer")
        self.power = PowerButton()
        tl.addWidget(self.export_btn)
        tl.addWidget(self.apply_btn)
        tl.addWidget(self.power)
        main.addWidget(top)
        self.progress = QtWidgets.QProgressBar()
        self.progress.setRange(0, 1000)
        self.progress.setTextVisible(False)
        self.progress.setFixedHeight(2)
        main.addWidget(self.progress)

        # Corps : trois colonnes
        body = QtWidgets.QWidget()
        body.setObjectName("body")
        bl = QtWidgets.QHBoxLayout(body)
        bl.setContentsMargins(16, 14, 16, 14)
        bl.setSpacing(14)

        left = QtWidgets.QWidget()
        left.setFixedWidth(330)
        ll = QtWidgets.QVBoxLayout(left)
        ll.setContentsMargins(0, 0, 0, 0)
        ll.setSpacing(14)
        self.hp_panel = HeadphonePanel()
        self.spk_panel = SpeakerPanel(app.speakers)
        ll.addWidget(self.hp_panel, 1)
        ll.addWidget(self.spk_panel, 1)
        bl.addWidget(left)

        center = QtWidgets.QWidget()
        cl = QtWidgets.QVBoxLayout(center)
        cl.setContentsMargins(0, 0, 0, 0)
        cl.setSpacing(14)
        plan_frame = QtWidgets.QFrame()
        plan_frame.setObjectName("card")
        pl = QtWidgets.QVBoxLayout(plan_frame)
        pl.setContentsMargins(14, 12, 14, 8)
        head = QtWidgets.QHBoxLayout()
        head.addWidget(label("PLAN", name="tag"))
        self.plan_title = label(name="sectionTitle")
        head.addWidget(self.plan_title, 1)
        self.hud = label(mono=True, muted=True)
        head.addWidget(self.hud)
        pl.addLayout(head)
        self.plan = RoomPlan()
        pl.addWidget(self.plan, 1)
        cl.addWidget(plan_frame, 5)
        lower = QtWidgets.QHBoxLayout()
        lower.setSpacing(14)
        resp_frame, resp_lay = section("Hz", "Réponse simulée")
        self.plot = ResponsePlot()
        resp_lay.addWidget(self.plot, 1)
        rt_frame, rt_lay = section("RT", "Réverbération")
        self.rt = RtMeter()
        rt_lay.addWidget(self.rt, 1)
        rt_lay.addWidget(caption("zone sarcelle : 0,2–0,4 s, écoute critique"))
        lower.addWidget(resp_frame, 3)
        lower.addWidget(rt_frame, 2)
        cl.addLayout(lower, 4)
        bl.addWidget(center, 1)

        right = QtWidgets.QWidget()
        rl = QtWidgets.QVBoxLayout(right)
        rl.setContentsMargins(0, 0, 6, 0)
        rl.setSpacing(14)
        self.room_panel = RoomPanel()
        self.out_panel = OutputPanel()
        rl.addWidget(self.room_panel)
        rl.addWidget(self.out_panel)
        rl.addStretch(1)
        right_scroll = scroll_wrap(right)
        right_scroll.setFixedWidth(340)
        bl.addWidget(right_scroll)

        body.setMinimumSize(1160, 720)
        main.addWidget(scroll_wrap(body, horizontal=True), 1)

        # Pied de page
        foot = QtWidgets.QFrame()
        foot.setObjectName("footer")
        fl = QtWidgets.QHBoxLayout(foot)
        fl.setContentsMargins(20, 6, 20, 6)
        self.status = label("PRÊT", mono=True, muted=True)
        self.filter_info = label(mono=True, muted=True)
        fl.addWidget(self.status, 1)
        fl.addWidget(self.filter_info)
        main.addWidget(foot)

        # Connexions
        self.hp_panel.selected.connect(self.on_headphone)
        self.hp_panel.eq_toggled.connect(lambda v: self.on_setting({"headphone_eq_enabled": v}))
        self.hp_panel.import_requested.connect(self.on_import)
        self.hp_panel.remove_requested.connect(self.on_remove_import)
        self.spk_panel.selected.connect(self.on_speaker)
        self.room_panel.changed.connect(self.on_setting)
        self.room_panel.preview.connect(self.on_preview)
        self.room_panel.preset_chosen.connect(self.on_preset)
        self.room_panel.enable_room.connect(lambda: self.on_setting({
            "room_enabled": True, "room_amount": max(self.app.settings.room_amount, 1.0),
            "reflection_order": max(self.app.settings.reflection_order, 2)}))
        self.plan.committed.connect(self.on_setting)
        self.out_panel.sink_changed.connect(lambda n: self.on_setting({"output_sink": n}))
        self.out_panel.autostart_changed.connect(self.on_autostart)
        self.out_panel.refresh_requested.connect(self.refresh_sinks)
        self.power.clicked.connect(self.on_toggle)
        self.apply_btn.clicked.connect(self.on_apply)
        self.export_btn.clicked.connect(self.on_export)
        self.auto.toggled.connect(lambda v: self.app.update_settings(
            extra={**self.app.settings.extra, "auto_apply": v}))
        self._auto_timer = QtCore.QTimer(self, singleShot=True, interval=self.AUTO_DELAY_MS)
        self._auto_timer.timeout.connect(self._auto_apply)

        if not app.supports_live:
            # Windows / macOS : pas de PipeWire, l'application produit des filtres à exporter.
            for w in (self.power, self.status_pill, self.auto, self.out_panel):
                w.setVisible(False)
            self.export_btn.setObjectName("primary")

        self.load_settings()
        self.refresh_sinks()
        self.refresh_state()
        self.timer = QtCore.QTimer(self, interval=3000)
        self.timer.timeout.connect(self.refresh_state)
        self.timer.start()
        self.run_task(self.app.headphones, self._headphones_loaded, with_progress=True,
                      message="Index AutoEq…", on_error=self.hp_panel.set_error, quiet=True)

    # -- état -----------------------------------------------------------------
    def load_settings(self):
        s = self.app.settings
        self.room_panel.load(s)
        self.spk_panel.select_id(s.speaker_id)
        self.hp_panel.show_current(self.app.headphone)
        self.hp_panel.eq_enabled.blockSignals(True)
        self.hp_panel.eq_enabled.setChecked(s.headphone_eq_enabled)
        self.hp_panel.eq_enabled.blockSignals(False)
        self.out_panel.autostart.setChecked(s.autostart)
        self.auto.blockSignals(True)
        self.auto.setChecked(bool(s.extra.get("auto_apply", True)))
        self.auto.blockSignals(False)
        self.refresh_room()

    def refresh_room(self, preview: dict | None = None):
        s = self.app.settings
        if preview:
            from dataclasses import replace
            s = replace(s, **preview)
        room = self.app.room_spec(s)
        self.plan.set_state(s, room)
        self.rt.set_values(room.rt60_bands())
        preset = ROOM_PRESETS.get(s.room_preset, {}).get("label", "Pièce personnalisée")
        self.plan_title.setText(preset)
        self.hud.setText(f"RT60 {fr_num(room.rt60, 2)} s  ·  Dc {fr_num(room.critical_distance(), 2)} m"
                         f"  ·  traitement {room.treatment * 100:.0f} %".upper())
        hp = self.app.headphone
        spk = self.app.speaker()
        sep = f" <span style='color:{AMBER}'>→</span> "
        self.chain.setText(sep.join([hp.name if hp else "sans casque",
                                     f"{spk['brand']} {spk['model']}", preset]))

    def refresh_sinks(self):
        if not self.app.supports_live:
            return
        try:
            sinks = self.app.pw.sinks()
        except Exception as exc:
            self.status.setText(f"PIPEWIRE INDISPONIBLE : {exc}")
            sinks = []
        self.out_panel.set_sinks(sinks, self.app.settings.output_sink)

    def refresh_state(self):
        running = self.app.is_running()
        self.power.set_on(running)
        slots = self.app.systemd.active_slots() if running else []
        if running:
            self.status_pill.setText(f"● LIVE · {'/'.join(slots).upper() or 'V1'}")
            self.status_pill.setStyleSheet(f"background: rgba(73,185,174,30); color: {TEAL};")
        else:
            self.status_pill.setText("○ VEILLE")
            self.status_pill.setStyleSheet(f"background: {PANEL2}; color: {MUTED};")
        self.apply_btn.setText("Appliquer •" if self.dirty else "Appliquer")
        for b in (self.power, self.apply_btn, self.export_btn):
            b.setEnabled(not self.busy)
        prev = self.app.store.load_runtime().get("previous_sink")
        self.out_panel.restore.setText(f"À L'ARRÊT : RETOUR SUR {prev}" if running and prev else "")

    def mark_dirty(self):
        self.dirty = True
        self.refresh_room()
        self.refresh_state()
        if self.auto.isChecked() and self.app.supports_live and self.app.is_running():
            self._auto_timer.start()

    def _auto_apply(self):
        if not self.dirty:
            return
        if self.busy:
            self.pending_apply = True
            return
        self.on_apply(auto=True)

    # -- tâches ---------------------------------------------------------------
    def run_task(self, fn, on_done=None, *, with_progress=False, message="",
                 on_error=None, quiet=False):
        if not quiet:
            self.busy = True
            self.refresh_state()
        self.status.setText(message.upper())
        self.progress.setValue(0)
        signals = WorkerSignals(self)
        worker = Worker(fn, with_progress=with_progress, signals=signals)
        worker.setAutoDelete(False)
        self._tasks.add(worker)

        def finish():
            self._tasks.discard(worker)
            signals.deleteLater()
            if not quiet:
                self.busy = False
            self.progress.setValue(0)
            self.refresh_state()
            if self.pending_apply and not self.busy:
                self.pending_apply = False
                self._auto_timer.start()

        def done(result):
            finish()
            if on_done:
                on_done(result)

        def failed(msg):
            finish()
            self.status.setText("ERREUR")
            if on_error:
                on_error(msg)
            else:
                QtWidgets.QMessageBox.critical(self, APP_NAME, msg)

        worker.signals.progress.connect(self._on_progress)
        worker.signals.done.connect(done)
        worker.signals.failed.connect(failed)
        self.pool.start(worker)

    def _on_progress(self, fraction: float, message: str):
        self.progress.setValue(int(fraction * 1000))
        self.status.setText(message.upper())

    def _headphones_loaded(self, entries):
        self.hp_panel.set_entries(self.app.imported_headphones() + entries)
        self.hp_panel.reveal(self.app.headphone)
        self.status.setText(f"INDEX AUTOEQ CHARGÉ · {len(entries)} MESURES")

    def _show_result(self, result):
        if result is not None:
            self.plot.set_data(result.plot_freqs, result.plot_curves)
            self.filter_info.setText(f"GAIN {result.gain_db:+.1f} DB · {result.fs} HZ")
            self.filter_info.setToolTip("\n".join(result.notes))
        self.status.setText("FILTRES APPLIQUÉS" if self.app.is_running() else "FILTRES GÉNÉRÉS")
        self.refresh_state()

    # -- actions --------------------------------------------------------------
    def on_headphone(self, entry: HeadphoneEntry):
        self.app.select_headphone(entry)
        self.mark_dirty()

    def on_speaker(self, speaker_id: str):
        self.app.select_speaker(speaker_id)
        self.mark_dirty()

    def on_preview(self, change: dict):
        self.refresh_room(change)

    def on_setting(self, change: dict):
        if any(k in ROOM_KEYS for k in change):
            change = {**change, "room_preset": ""}
        self.app.update_settings(**change)
        self.room_panel.load(self.app.settings)   # bornes / cohérence de la pièce
        self.mark_dirty()

    def on_preset(self, key: str):
        self.app.apply_room_preset(key)
        self.room_panel.load(self.app.settings)
        self.mark_dirty()

    def on_toggle(self):
        if self.app.is_running():
            self.run_task(self.app.stop, lambda _r: self.status.setText("SIMULATION ARRÊTÉE"),
                          message="Arrêt…")
        else:
            self.run_task(self.app.start, self._done_apply, with_progress=True,
                          message="Démarrage…")

    def _done_apply(self, result):
        self.dirty = False
        self._show_result(result)

    def on_apply(self, auto: bool = False):
        fn = self.app.apply if self.app.is_running() else self.app.build_filters
        self.run_task(fn, self._done_apply, with_progress=True,
                      message="Application automatique…" if auto else "Génération des filtres…")

    def _reload_headphones(self):
        try:
            self.hp_panel.set_entries(self.app.imported_headphones() + self.app.headphones())
        except Exception:  # index AutoEq indisponible : au moins les imports
            self.hp_panel.set_entries(self.app.imported_headphones())

    def on_import(self):
        path, _ = QtWidgets.QFileDialog.getOpenFileName(
            self, "Importer une mesure ou un EQ de casque", str(self.app.paths.state_dir.parent),
            "Mesures et EQ (*.txt *.csv *.frd *.tsv);;Tous les fichiers (*)")
        if not path:
            return
        try:
            with open(path, encoding="utf-8", errors="replace") as fh:
                kind = ae.detect_headphone_file_kind(fh.read())
        except OSError as exc:
            QtWidgets.QMessageBox.critical(self, APP_NAME, f"Lecture impossible : {exc}")
            return
        name, ok = QtWidgets.QInputDialog.getText(
            self, "Nom du casque", "Nom affiché :", text=os.path.splitext(os.path.basename(path))[0])
        if not ok:
            return
        target = ""
        if kind == "measurement":
            target, ok = QtWidgets.QInputDialog.getItem(
                self, "Courbe cible",
                "Mesure brute détectée : vers quelle cible corriger ?\n"
                "(Harman over-ear = cible des corrections AutoEq de casques circum-auriculaires)",
                list(AUTOEQ_TARGETS), 0, False)
            if not ok:
                return
        try:
            entry = self.app.import_headphone(path, name, target)
        except AppError as exc:
            QtWidgets.QMessageBox.critical(self, APP_NAME, str(exc))
            return
        self._reload_headphones()
        self.hp_panel.show_current(entry)
        self.hp_panel.reveal(entry)
        self.status.setText(f"CASQUE IMPORTÉ : {entry.label}".upper())
        self.mark_dirty()

    def on_remove_import(self, entry):
        self.app.remove_import(entry)
        self._reload_headphones()
        self.hp_panel.show_current(self.app.headphone)
        self.mark_dirty()

    def on_export(self):
        folder = QtWidgets.QFileDialog.getExistingDirectory(
            self, "Dossier d'export des filtres", str(self.app.paths.state_dir.parent))
        if not folder:
            return

        def done(paths):
            self._show_result(self.app.last_result)
            self.status.setText(f"{len(paths)} FICHIERS EXPORTÉS VERS {folder}".upper())

        self.run_task(lambda progress: self.app.export(folder, progress), done,
                      with_progress=True, message="Export des filtres…")

    def on_autostart(self, enabled: bool):
        def done(_r):
            self.status.setText("AUTO-DÉMARRAGE " + ("ACTIVÉ" if enabled else "DÉSACTIVÉ"))

        def failed(msg):
            self.out_panel.autostart.setChecked(not enabled)
            QtWidgets.QMessageBox.critical(self, APP_NAME, msg)

        self.run_task(lambda: self.app.set_autostart(enabled), done,
                      message="Configuration de l'auto-démarrage…", on_error=failed)


def run(argv: list[str] | None = None) -> int:
    qapp = QtWidgets.QApplication(argv if argv is not None else sys.argv)
    qapp.setApplicationName(APP_NAME)
    qapp.setDesktopFileName("devfrp-speaker-sim")
    qapp.setStyle("Fusion")
    load_fonts()
    qapp.setStyleSheet(STYLE)
    icon = APP_DIR / "packaging" / "icon.png"
    if icon.is_file():
        qapp.setWindowIcon(QtGui.QIcon(str(icon)))
    try:
        app = SpeakerSimApp()
    except Exception as exc:
        QtWidgets.QMessageBox.critical(None, APP_NAME, f"Initialisation impossible : {exc}")
        return 1
    setup_logging(app.paths)
    win = MainWindow(app)
    win.show()
    return qapp.exec()
