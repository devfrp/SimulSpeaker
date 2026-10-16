"""Interface graphique (Qt 6) : panneaux casque, haut-parleurs, pièce, sortie, courbes.

Toutes les opérations longues passent par :class:`Worker` (``QThreadPool``) ;
le thread de l'interface ne fait que lire l'état et enregistrer les réglages.
"""

from __future__ import annotations

import math
import sys
import traceback
from typing import Callable

from app import AppError, SpeakerSimApp, setup_logging
from config import APP_DIR, APP_NAME
from download import AUTOEQ_TARGETS, HeadphoneEntry, matches, query_patterns
import audio_engine as ae
from qt_compat import QtCore, QtGui, QtWidgets, Signal

Qt = QtCore.Qt

STYLE = """
* { font-size: 13px; }
QWidget#root { background: #131519; }
QLabel { color: #e6e8ec; }
QLabel[muted="true"] { color: #8e96a3; }
QLabel#title { font-size: 20px; font-weight: 600; }
QLabel#cardTitle { font-size: 14px; font-weight: 600; color: #f3f4f6; }
QFrame#card { background: #1c1f25; border: 1px solid #2a2e36; border-radius: 12px; }
QLineEdit, QComboBox, QDoubleSpinBox, QSpinBox {
    background: #23262d; color: #e6e8ec; border: 1px solid #343944; border-radius: 8px;
    padding: 6px 8px; selection-background-color: #4c7dff; }
QLineEdit:focus, QComboBox:focus, QDoubleSpinBox:focus, QSpinBox:focus { border-color: #5b8cff; }
QListView, QListWidget { background: #181b20; color: #e6e8ec; border: 1px solid #2a2e36;
    border-radius: 8px; outline: 0; }
QListView::item, QListWidget::item { padding: 6px 8px; border-radius: 6px; }
QListView::item:selected, QListWidget::item:selected { background: #2f4f9e; color: white; }
QListView::item:hover, QListWidget::item:hover { background: #262a33; }
QPushButton { background: #2a2e37; color: #e6e8ec; border: 1px solid #3a3f4b;
    border-radius: 8px; padding: 8px 14px; }
QPushButton:hover { background: #333845; }
QPushButton:disabled { color: #6b7280; background: #22252b; }
QPushButton#primary { background: #4c7dff; border: none; color: white; font-weight: 600; }
QPushButton#primary:hover { background: #5d8bff; }
QPushButton#danger { background: #e2555a; border: none; color: white; font-weight: 600; }
QPushButton#danger:hover { background: #ec676c; }
QCheckBox { color: #e6e8ec; spacing: 8px; }
QSlider::groove:horizontal { height: 4px; background: #343944; border-radius: 2px; }
QSlider::sub-page:horizontal { background: #4c7dff; border-radius: 2px; }
QSlider::handle:horizontal { background: #e6e8ec; width: 14px; margin: -6px 0; border-radius: 7px; }
QProgressBar { background: #23262d; border: none; border-radius: 4px; height: 8px; color: transparent; }
QProgressBar::chunk { background: #4c7dff; border-radius: 4px; }
QLabel#pill { border-radius: 11px; padding: 3px 12px; font-weight: 600; }
QScrollArea#bodyScroll, QWidget#body { border: none; background: #131519; }
QScrollBar:vertical { background: transparent; width: 10px; margin: 2px; }
QScrollBar::handle:vertical { background: #3a3f4b; border-radius: 4px; min-height: 40px; }
QScrollBar::handle:vertical:hover { background: #4a5060; }
QScrollBar:horizontal { background: transparent; height: 10px; margin: 2px; }
QScrollBar::handle:horizontal { background: #3a3f4b; border-radius: 4px; min-width: 40px; }
QScrollBar::add-line, QScrollBar::sub-line { width: 0; height: 0; }
QScrollBar::add-page, QScrollBar::sub-page { background: transparent; }
QToolTip { background: #23262d; color: #e6e8ec; border: 1px solid #343944; }
"""

PLOT_COLORS = ["#f5b14c", "#4cc9a0", "#7aa2ff", "#e36d9b"]


# ---------------------------------------------------------------------------
# Travail en arrière-plan
# ---------------------------------------------------------------------------
class WorkerSignals(QtCore.QObject):
    progress = Signal(float, str)
    done = Signal(object)
    failed = Signal(str)


class Worker(QtCore.QRunnable):
    def __init__(self, fn: Callable, *args, with_progress: bool = False):
        super().__init__()
        self.fn, self.args, self.with_progress = fn, args, with_progress
        self.signals = WorkerSignals()

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
# Widgets utilitaires
# ---------------------------------------------------------------------------
def card(title: str, subtitle: str = "") -> tuple[QtWidgets.QFrame, QtWidgets.QVBoxLayout]:
    frame = QtWidgets.QFrame()
    frame.setObjectName("card")
    lay = QtWidgets.QVBoxLayout(frame)
    lay.setContentsMargins(16, 14, 16, 16)
    lay.setSpacing(10)
    t = QtWidgets.QLabel(title)
    t.setObjectName("cardTitle")
    lay.addWidget(t)
    if subtitle:
        s = QtWidgets.QLabel(subtitle)
        s.setProperty("muted", True)
        s.setWordWrap(True)
        lay.addWidget(s)
    return frame, lay


def muted(text: str = "") -> QtWidgets.QLabel:
    lbl = QtWidgets.QLabel(text)
    lbl.setProperty("muted", True)
    lbl.setWordWrap(True)
    return lbl


class WordFilterProxy(QtCore.QSortFilterProxyModel):
    """Filtre multi-mots insensible à la casse (tous les mots doivent apparaître)."""

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


class ResponsePlot(QtWidgets.QWidget):
    """Courbes de réponse en fréquence (axe log, dessinées avec QPainter)."""

    def __init__(self, parent=None):
        super().__init__(parent)
        self.setMinimumHeight(220)
        self.freqs = None
        self.curves: dict[str, object] = {}
        self.placeholder = "Les courbes apparaîtront après la génération des filtres."

    def set_data(self, freqs, curves: dict) -> None:
        self.freqs, self.curves = freqs, curves
        self.update()

    def paintEvent(self, _event):
        p = QtGui.QPainter(self)
        p.setRenderHint(QtGui.QPainter.RenderHint.Antialiasing)
        r = self.rect().adjusted(44, 12, -12, -26)
        p.fillRect(self.rect(), QtGui.QColor("#181b20"))
        p.setPen(QtGui.QColor("#8e96a3"))
        if self.freqs is None or not self.curves:
            p.drawText(self.rect(), Qt.AlignmentFlag.AlignCenter, self.placeholder)
            return
        lo_db, hi_db = -30.0, 15.0

        def x_of(f):
            return r.left() + r.width() * (math.log10(f) - math.log10(20)) / 3.0

        def y_of(db):
            db = min(max(db, lo_db), hi_db)
            return r.bottom() - r.height() * (db - lo_db) / (hi_db - lo_db)

        grid = QtGui.QPen(QtGui.QColor("#2a2e36"))
        for f, label in ((20, "20"), (50, "50"), (100, "100"), (200, "200"), (500, "500"),
                         (1000, "1k"), (2000, "2k"), (5000, "5k"), (10000, "10k"), (20000, "20k")):
            x = x_of(f)
            p.setPen(grid)
            p.drawLine(QtCore.QPointF(x, r.top()), QtCore.QPointF(x, r.bottom()))
            p.setPen(QtGui.QColor("#8e96a3"))
            p.drawText(QtCore.QRectF(x - 20, r.bottom() + 4, 40, 16),
                       Qt.AlignmentFlag.AlignCenter, label)
        for db in range(-30, 16, 5):
            y = y_of(db)
            p.setPen(QtGui.QPen(QtGui.QColor("#3a3f4b" if db == 0 else "#2a2e36")))
            p.drawLine(QtCore.QPointF(r.left(), y), QtCore.QPointF(r.right(), y))
            p.setPen(QtGui.QColor("#8e96a3"))
            p.drawText(QtCore.QRectF(0, y - 8, 38, 16),
                       Qt.AlignmentFlag.AlignRight | Qt.AlignmentFlag.AlignVCenter, f"{db:+d}")

        # Les courbes sont recentrées sur leur moyenne 200 Hz–2 kHz pour la lisibilité.
        colors = [QtGui.QColor(PLOT_COLORS[i % len(PLOT_COLORS)]) for i in range(len(self.curves))]
        p.setBrush(Qt.BrushStyle.NoBrush)
        for color, values in zip(colors, self.curves.values()):
            band = [v for f, v in zip(self.freqs, values) if 200 <= f <= 2000]
            offset = sum(band) / len(band) if band else 0.0
            path = QtGui.QPainterPath()
            for j, (f, v) in enumerate(zip(self.freqs, values)):
                pt = QtCore.QPointF(x_of(f), y_of(v - offset))
                path.moveTo(pt) if j == 0 else path.lineTo(pt)
            p.setPen(QtGui.QPen(color, 2))
            p.drawPath(path)

        # Légende sur fond semi-opaque, en bas à droite (zone la moins chargée).
        names = list(self.curves)
        legend_w = max(p.fontMetrics().horizontalAdvance(n) for n in names) + 40
        legend_h = 17 * len(names) + 6
        box = QtCore.QRectF(r.right() - legend_w - 4, r.bottom() - legend_h - 4, legend_w, legend_h)
        p.setPen(Qt.PenStyle.NoPen)
        p.setBrush(QtGui.QColor(24, 27, 32, 225))
        p.drawRoundedRect(box, 6, 6)
        y = box.top() + 2
        for color, name in zip(colors, names):
            p.fillRect(QtCore.QRectF(box.left() + 6, y + 5, 14, 3), color)
            p.setPen(QtGui.QColor("#e6e8ec"))
            p.drawText(QtCore.QPointF(box.left() + 26, y + 11), name)
            y += 17


# ---------------------------------------------------------------------------
# Panneaux
# ---------------------------------------------------------------------------
class HeadphonePanel(QtWidgets.QFrame):
    selected = Signal(object)          # HeadphoneEntry
    eq_toggled = Signal(bool)
    import_requested = Signal()
    remove_requested = Signal(object)  # HeadphoneEntry importé

    def __init__(self, parent=None):
        frame, lay = card("Écouteurs", "8 850 mesures AutoEq (commit figé). "
                                       "La correction est téléchargée une seule fois.")
        super().__init__(parent)
        outer = QtWidgets.QVBoxLayout(self)
        outer.setContentsMargins(0, 0, 0, 0)
        outer.addWidget(frame)
        self.search = QtWidgets.QLineEdit()
        self.search.setPlaceholderText("Rechercher (ex. « hd 650 oratory »)…")
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
        self.view.setMinimumHeight(180)
        lay.addWidget(self.view, 1)
        self.count = muted("Chargement de l'index AutoEq…")
        lay.addWidget(self.count)
        self.current = QtWidgets.QLabel("Aucun casque sélectionné")
        self.current.setWordWrap(True)
        lay.addWidget(self.current)
        self.eq_enabled = QtWidgets.QCheckBox("Appliquer la correction du casque")
        import_btn = QtWidgets.QPushButton("Importer une mesure…")
        import_btn.setToolTip("Votre propre mesure (fréquence, dB : REW, Squiglink, CSV AutoEq)\n"
                              "ou un EQ AutoEq / Equalizer APO (ParametricEQ, GraphicEQ)")
        row = QtWidgets.QHBoxLayout()
        row.addWidget(self.eq_enabled, 1)
        row.addWidget(import_btn)
        lay.addLayout(row)
        import_btn.clicked.connect(self.import_requested.emit)
        self.view.setContextMenuPolicy(Qt.ContextMenuPolicy.CustomContextMenu)
        self.view.customContextMenuRequested.connect(self._context_menu)

        self._timer = QtCore.QTimer(self, singleShot=True, interval=150)
        self._timer.timeout.connect(self._apply_filter)
        self.search.textChanged.connect(lambda _t: self._timer.start())
        self.view.clicked.connect(self._on_clicked)
        self.eq_enabled.toggled.connect(self.eq_toggled.emit)

    def set_entries(self, entries: list[HeadphoneEntry]) -> None:
        self.model.clear()
        for e in entries:
            item = QtGui.QStandardItem(("★ " if e.is_import else "") + e.label)
            item.setData(e, Qt.ItemDataRole.UserRole)
            item.setData(f"{e.name} {e.source} {e.rig}".lower(), Qt.ItemDataRole.UserRole + 1)
            self.model.appendRow(item)
        self.search.setEnabled(True)
        self._apply_filter()

    def _context_menu(self, pos):
        entry = self.proxy.data(self.view.indexAt(pos), Qt.ItemDataRole.UserRole)
        if entry is None or not entry.is_import:
            return
        menu = QtWidgets.QMenu(self)
        remove = menu.addAction("Supprimer cet import")
        if menu.exec(self.view.viewport().mapToGlobal(pos)) == remove:
            self.remove_requested.emit(entry)

    def set_error(self, message: str) -> None:
        self.count.setText(f"Index indisponible : {message}")

    def _apply_filter(self):
        self.proxy.set_query(self.search.text())
        self.count.setText(f"{self.proxy.rowCount()} / {self.model.rowCount()} mesures")

    def _on_clicked(self, index):
        entry = self.proxy.data(index, Qt.ItemDataRole.UserRole)
        if entry is not None:
            self.show_current(entry)
            self.selected.emit(entry)

    def show_current(self, entry: HeadphoneEntry | None):
        if entry is None:
            self.current.setText("Aucun casque sélectionné")
        else:
            self.current.setText(f"<b>{entry.name}</b><br><span style='color:#8e96a3'>"
                                 f"{entry.source} · {entry.rig}</span>")


class SpeakerPanel(QtWidgets.QFrame):
    selected = Signal(str)

    def __init__(self, speakers: list[dict], parent=None):
        super().__init__(parent)
        n_studio = sum(sp.get("category") == "studio" for sp in speakers)
        frame, lay = card("Haut-parleurs", f"{len(speakers)} modèles mesurés, dont {n_studio} "
                                           "moniteurs de studio (CEA-2034, Spinorama / Klippel NFS).")
        outer = QtWidgets.QVBoxLayout(self)
        outer.setContentsMargins(0, 0, 0, 0)
        outer.addWidget(frame)
        self.search = QtWidgets.QLineEdit()
        self.search.setPlaceholderText("Filtrer par marque ou modèle…")
        self.search.setClearButtonEnabled(True)
        self.category = QtWidgets.QComboBox()
        self.category.addItem("Toutes", "")
        self.category.addItem("Moniteurs de studio", "studio")
        self.category.addItem("Hi-Fi", "hifi")
        self.category.setToolTip("Filtrer par type d'enceinte")
        row = QtWidgets.QHBoxLayout()
        row.addWidget(self.search, 1)
        row.addWidget(self.category)
        lay.addLayout(row)
        self.list = QtWidgets.QListWidget()
        self.list.setMinimumHeight(180)
        for spk in speakers:
            studio = spk.get("category") == "studio"
            item = QtWidgets.QListWidgetItem(f"{spk['brand']} {spk['model']}"
                                             + ("   · studio" if studio else ""))
            item.setData(Qt.ItemDataRole.UserRole, spk["id"])
            item.setData(Qt.ItemDataRole.UserRole + 1, spk.get("category", "hifi"))
            kind = {"bookshelves": "bibliothèque", "floorstanders": "colonne",
                    "center": "centrale"}.get(spk.get("shape"), spk.get("shape", ""))
            origin = spk.get("origin", "ASR")
            computed = " (CEA-2034 calculé depuis les mesures polaires)" \
                if spk.get("format") == "polar" else ""
            item.setToolTip(f"{'moniteur de studio' if studio else 'hi-fi'} · {kind} · "
                            f"{spk.get('type', '')}\nscore Olive {spk.get('pref_score')} "
                            f"({spk.get('pref_score_wsub')} avec caisson) · LFX {spk.get('lfx_hz')} Hz"
                            f"\nmesure : {origin}, Klippel NFS{computed}")
            self.list.addItem(item)
        lay.addWidget(self.list, 1)
        self.search.textChanged.connect(lambda _t: self._filter())
        self.category.currentIndexChanged.connect(lambda _i: self._filter())
        self.list.currentItemChanged.connect(self._changed)

    def _filter(self):
        patterns = query_patterns(self.search.text())
        wanted = self.category.currentData()
        for i in range(self.list.count()):
            item = self.list.item(i)
            ok = matches(patterns, item.text()) and (
                not wanted or item.data(Qt.ItemDataRole.UserRole + 1) == wanted)
            item.setHidden(not ok)

    def _changed(self, current, _previous):
        if current is not None:
            self.selected.emit(current.data(Qt.ItemDataRole.UserRole))

    def select_id(self, speaker_id: str):
        for i in range(self.list.count()):
            item = self.list.item(i)
            if item.data(Qt.ItemDataRole.UserRole) == speaker_id:
                self.list.blockSignals(True)
                self.list.setCurrentItem(item)
                self.list.blockSignals(False)
                self.list.scrollToItem(item)
                return


class RoomPanel(QtWidgets.QFrame):
    changed = Signal(dict)

    FIELDS = [
        # nom, libellé, min, max, pas, décimales, suffixe
        ("speaker_angle", "Angle des enceintes", 10, 60, 1, 0, " °"),
        ("speaker_distance", "Distance d'écoute", 0.5, 6, 0.1, 1, " m"),
        ("room_width", "Largeur de la pièce", 2, 20, 0.1, 1, " m"),
        ("room_length", "Longueur de la pièce", 2, 30, 0.1, 1, " m"),
        ("room_height", "Hauteur sous plafond", 2, 8, 0.1, 1, " m"),
        ("rt60", "Temps de réverbération (RT60)", 0.1, 2, 0.05, 2, " s"),
        ("preamp_db", "Préampli", -24, 12, 0.5, 1, " dB"),
    ]

    def __init__(self, parent=None):
        super().__init__(parent)
        frame, lay = card("Pièce & rendu", "Modèle de sources-images + réverbération "
                                           "tardive ; HRTF KEMAR (libmysofa).")
        outer = QtWidgets.QVBoxLayout(self)
        outer.setContentsMargins(0, 0, 0, 0)
        outer.addWidget(frame)
        form = QtWidgets.QFormLayout()
        form.setLabelAlignment(Qt.AlignmentFlag.AlignLeft)
        form.setHorizontalSpacing(14)
        self.spins: dict[str, QtWidgets.QDoubleSpinBox] = {}
        for name, label, lo, hi, step, dec, suffix in self.FIELDS:
            spin = QtWidgets.QDoubleSpinBox()
            spin.setRange(lo, hi)
            spin.setSingleStep(step)
            spin.setDecimals(dec)
            spin.setSuffix(suffix)
            spin.setKeyboardTracking(False)
            spin.setButtonSymbols(QtWidgets.QAbstractSpinBox.ButtonSymbols.NoButtons)
            spin.setToolTip("Molette ou flèches du clavier pour ajuster")
            spin.valueChanged.connect(lambda v, n=name: self.changed.emit({n: v}))
            self.spins[name] = spin
            form.addRow(muted(label), spin)
        self.amount = QtWidgets.QSlider(Qt.Orientation.Horizontal)
        self.amount.setRange(0, 150)
        self.amount.setToolTip("0 % = anéchoïque, 100 % = niveau physique de la pièce")
        self.amount_label = muted()
        row = QtWidgets.QHBoxLayout()
        row.addWidget(self.amount, 1)
        row.addWidget(self.amount_label)
        form.addRow(muted("Quantité de pièce"), row)
        self.order = QtWidgets.QSpinBox()
        self.order.setRange(0, 3)
        self.order.setButtonSymbols(QtWidgets.QAbstractSpinBox.ButtonSymbols.NoButtons)
        self.order.setToolTip("Ordre maximal des réflexions précoces simulées")
        form.addRow(muted("Ordre des réflexions"), self.order)
        lay.addLayout(form)
        self.room_enabled = QtWidgets.QCheckBox("Simuler la pièce (réflexions + réverbération)")
        self.df_eq = QtWidgets.QCheckBox("Égalisation champ diffus du HRTF (recommandé)")
        lay.addWidget(self.room_enabled)
        lay.addWidget(self.df_eq)

        self.amount.valueChanged.connect(self._amount_changed)
        self.amount.sliderReleased.connect(
            lambda: self.changed.emit({"room_amount": self.amount.value() / 100}))
        self.order.valueChanged.connect(lambda v: self.changed.emit({"reflection_order": v}))
        self.room_enabled.toggled.connect(lambda v: self.changed.emit({"room_enabled": v}))
        self.df_eq.toggled.connect(lambda v: self.changed.emit({"diffuse_field_eq": v}))

    def _amount_changed(self, value: int):
        self.amount_label.setText(f"{value} %")
        if not self.amount.isSliderDown():
            self.changed.emit({"room_amount": value / 100})

    def load(self, settings) -> None:
        widgets = list(self.spins.values()) + [self.amount, self.order, self.room_enabled, self.df_eq]
        for w in widgets:
            w.blockSignals(True)
        for name, spin in self.spins.items():
            spin.setValue(getattr(settings, name))
        self.amount.setValue(round(settings.room_amount * 100))
        self.amount_label.setText(f"{self.amount.value()} %")
        self.order.setValue(settings.reflection_order)
        self.room_enabled.setChecked(settings.room_enabled)
        self.df_eq.setChecked(settings.diffuse_field_eq)
        for w in widgets:
            w.blockSignals(False)


class OutputPanel(QtWidgets.QFrame):
    sink_changed = Signal(str)
    rate_changed = Signal(int)
    autostart_changed = Signal(bool)
    refresh_requested = Signal()

    def __init__(self, parent=None):
        super().__init__(parent)
        frame, lay = card("Sortie & démarrage")
        outer = QtWidgets.QVBoxLayout(self)
        outer.setContentsMargins(0, 0, 0, 0)
        outer.addWidget(frame)
        row = QtWidgets.QHBoxLayout()
        self.combo = QtWidgets.QComboBox()
        self.combo.setToolTip("Périphérique physique (casque) sur lequel le simulateur joue")
        refresh = QtWidgets.QPushButton("↻")
        refresh.setToolTip("Actualiser la liste des sorties")
        refresh.setFixedWidth(40)
        row.addWidget(self.combo, 1)
        row.addWidget(refresh)
        self.sink_label = muted("Casque (sortie physique)")
        lay.addWidget(self.sink_label)
        lay.addLayout(row)
        self.rate = QtWidgets.QComboBox()
        for label, value in (("Automatique (horloge PipeWire / 48 kHz)", 0), ("44,1 kHz", 44100),
                             ("48 kHz", 48000), ("96 kHz", 96000)):
            self.rate.addItem(label, value)
        self.rate.setToolTip("Doit correspondre à la fréquence du périphérique ou du convolveur")
        lay.addWidget(muted("Fréquence d'échantillonnage des filtres"))
        lay.addWidget(self.rate)
        self.autostart = QtWidgets.QCheckBox("Démarrer automatiquement à la connexion")
        self.autostart.setToolTip("Unité systemd utilisateur ; aucun droit root nécessaire")
        lay.addWidget(self.autostart)
        self.restore = muted("")
        lay.addWidget(self.restore)
        self.combo.activated.connect(
            lambda i: self.sink_changed.emit(self.combo.itemData(i) or ""))
        self.autostart.clicked.connect(self.autostart_changed.emit)
        refresh.clicked.connect(self.refresh_requested.emit)
        self.rate.activated.connect(lambda i: self.rate_changed.emit(int(self.rate.itemData(i))))
        self.live_widgets = [self.sink_label, self.combo, refresh, self.autostart, self.restore]

    def set_rate(self, value: int):
        idx = self.rate.findData(value)
        self.rate.setCurrentIndex(max(idx, 0))

    def set_sinks(self, sinks, current: str):
        self.combo.blockSignals(True)
        self.combo.clear()
        self.combo.addItem("Automatique (sortie par défaut actuelle)", "")
        for s in sinks:
            self.combo.addItem(s.label, s.name)
        idx = self.combo.findData(current)
        self.combo.setCurrentIndex(max(idx, 0))
        self.combo.blockSignals(False)


# ---------------------------------------------------------------------------
# Fenêtre principale
# ---------------------------------------------------------------------------
class MainWindow(QtWidgets.QMainWindow):
    def __init__(self, app: SpeakerSimApp):
        super().__init__()
        self.app = app
        self.pool = QtCore.QThreadPool.globalInstance()
        # Références fortes : sans elles, le QObject des signaux d'un Worker peut être
        # détruit avant la livraison (file d'attente) de ``done``/``failed``.
        self._tasks: set[Worker] = set()
        self.busy = False
        self.dirty = False
        self.setWindowTitle(APP_NAME)
        self.resize(1180, 820)

        root = QtWidgets.QWidget()
        root.setObjectName("root")
        self.setCentralWidget(root)
        main = QtWidgets.QVBoxLayout(root)
        main.setContentsMargins(20, 16, 20, 14)
        main.setSpacing(14)

        # En-tête
        header = QtWidgets.QHBoxLayout()
        titles = QtWidgets.QVBoxLayout()
        title = QtWidgets.QLabel(APP_NAME)
        title.setObjectName("title")
        self.subtitle = muted()
        titles.addWidget(title)
        titles.addWidget(self.subtitle)
        header.addLayout(titles, 1)
        self.pill = QtWidgets.QLabel()
        self.pill.setObjectName("pill")
        header.addWidget(self.pill, 0, Qt.AlignmentFlag.AlignVCenter)
        self.apply_btn = QtWidgets.QPushButton("Appliquer")
        self.apply_btn.setToolTip("Régénérer les filtres (et redémarrer si actif)")
        self.export_btn = QtWidgets.QPushButton("Exporter…")
        self.export_btn.setToolTip("Enregistrer les filtres (WAV, true stereo, Equalizer APO)")
        self.toggle_btn = QtWidgets.QPushButton()
        self.toggle_btn.setMinimumWidth(130)
        header.addWidget(self.export_btn)
        header.addWidget(self.apply_btn)
        header.addWidget(self.toggle_btn)
        main.addLayout(header)

        # Corps
        body = QtWidgets.QHBoxLayout()
        body.setSpacing(14)
        body.setContentsMargins(0, 0, 14, 0)  # place pour la barre de défilement
        left = QtWidgets.QVBoxLayout()
        left.setSpacing(14)
        self.hp_panel = HeadphonePanel()
        self.spk_panel = SpeakerPanel(app.speakers)
        left.addWidget(self.hp_panel, 3)
        left.addWidget(self.spk_panel, 2)
        right = QtWidgets.QVBoxLayout()
        right.setSpacing(14)
        self.room_panel = RoomPanel()
        self.out_panel = OutputPanel()
        plot_frame, plot_lay = card("Réponse simulée")
        self.plot = ResponsePlot()
        plot_lay.addWidget(self.plot, 1)
        self.notes = muted()
        plot_lay.addWidget(self.notes)
        right.addWidget(self.room_panel)
        right.addWidget(self.out_panel)
        right.addWidget(plot_frame, 1)
        lw, rw = QtWidgets.QWidget(), QtWidgets.QWidget()
        lw.setLayout(left)
        rw.setLayout(right)
        body.addWidget(lw, 5)
        body.addWidget(rw, 6)
        # Zone défilante : dans une fenêtre plus petite que le minimum (tuilage
        # Hyprland, petit écran), le contenu défile au lieu de se chevaucher.
        body_widget = QtWidgets.QWidget()
        body_widget.setObjectName("body")
        body_widget.setLayout(body)
        scroll = QtWidgets.QScrollArea()
        scroll.setObjectName("bodyScroll")
        scroll.setWidgetResizable(True)
        scroll.setFrameShape(QtWidgets.QFrame.Shape.NoFrame)
        scroll.setHorizontalScrollBarPolicy(Qt.ScrollBarPolicy.ScrollBarAsNeeded)
        scroll.setWidget(body_widget)
        main.addWidget(scroll, 1)

        # Pied
        footer = QtWidgets.QHBoxLayout()
        self.progress = QtWidgets.QProgressBar()
        self.progress.setRange(0, 1000)
        self.progress.setFixedWidth(220)
        self.progress.setVisible(False)
        self.status = muted("Prêt")
        footer.addWidget(self.status, 1)
        footer.addWidget(self.progress)
        main.addLayout(footer)

        # Connexions
        self.hp_panel.selected.connect(self.on_headphone)
        self.hp_panel.eq_toggled.connect(lambda v: self.on_setting({"headphone_eq_enabled": v}))
        self.hp_panel.import_requested.connect(self.on_import)
        self.hp_panel.remove_requested.connect(self.on_remove_import)
        self.spk_panel.selected.connect(self.on_speaker)
        self.room_panel.changed.connect(self.on_setting)
        self.out_panel.sink_changed.connect(lambda n: self.on_setting({"output_sink": n}))
        self.out_panel.autostart_changed.connect(self.on_autostart)
        self.out_panel.refresh_requested.connect(self.refresh_sinks)
        self.out_panel.rate_changed.connect(lambda v: self.on_setting({"sample_rate": v}))
        self.export_btn.clicked.connect(self.on_export)
        if not app.supports_live:
            # Windows / macOS : pas de PipeWire, l'application produit des filtres à exporter.
            for w in [self.toggle_btn, self.pill, *self.out_panel.live_widgets]:
                w.setVisible(False)
            self.export_btn.setObjectName("primary")
        self.toggle_btn.clicked.connect(self.on_toggle)
        self.apply_btn.clicked.connect(self.on_apply)

        self.load_settings()
        self.refresh_sinks()
        self.refresh_state()
        self.timer = QtCore.QTimer(self, interval=3000)
        self.timer.timeout.connect(self.refresh_state)
        self.timer.start()
        self.run_task(self.app.headphones, self._headphones_loaded, with_progress=True,
                      message="Index AutoEq…", on_error=self.hp_panel.set_error, quiet=True)

    # -- état -----------------------------------------------------------
    def load_settings(self):
        s = self.app.settings
        self.room_panel.load(s)
        self.spk_panel.select_id(s.speaker_id)
        self.hp_panel.show_current(self.app.headphone)
        self.hp_panel.eq_enabled.blockSignals(True)
        self.hp_panel.eq_enabled.setChecked(s.headphone_eq_enabled)
        self.hp_panel.eq_enabled.blockSignals(False)
        self.out_panel.autostart.setChecked(s.autostart)
        self.out_panel.set_rate(s.sample_rate)
        self.subtitle.setText(self.app.description())

    def refresh_sinks(self):
        if not self.app.supports_live:
            return
        try:
            sinks = self.app.pw.sinks()
        except Exception as exc:
            self.status.setText(f"PipeWire indisponible : {exc}")
            sinks = []
        self.out_panel.set_sinks(sinks, self.app.settings.output_sink)

    def refresh_state(self):
        running = self.app.is_running()
        self.pill.setText("● Actif" if running else "○ Arrêté")
        self.pill.setStyleSheet("background:#1f3b2c;color:#5fd38d;" if running
                                else "background:#2a2e36;color:#9aa1ad;")
        self.toggle_btn.setText("Arrêter" if running else "Démarrer")
        self.toggle_btn.setObjectName("danger" if running else "primary")
        self.toggle_btn.style().unpolish(self.toggle_btn)
        self.toggle_btn.style().polish(self.toggle_btn)
        self.apply_btn.setText("Appliquer •" if self.dirty else "Appliquer")
        for b in (self.toggle_btn, self.apply_btn, self.export_btn):
            b.setEnabled(not self.busy)
        prev = self.app.store.load_runtime().get("previous_sink")
        self.out_panel.restore.setText(
            f"Sortie restaurée à l'arrêt : {prev}" if running and prev else "")

    def mark_dirty(self):
        self.dirty = True
        self.subtitle.setText(self.app.description())
        self.refresh_state()

    # -- tâches ---------------------------------------------------------
    def run_task(self, fn, on_done=None, *, with_progress=False, message="",
                 on_error=None, quiet=False):
        if not quiet:
            self.busy = True
            self.refresh_state()
        self.status.setText(message)
        self.progress.setValue(0)
        self.progress.setVisible(with_progress)
        worker = Worker(fn, with_progress=with_progress)
        worker.setAutoDelete(False)
        self._tasks.add(worker)

        def finish():
            self._tasks.discard(worker)
            if not quiet:
                self.busy = False
            self.progress.setVisible(False)
            self.refresh_state()

        def done(result):
            finish()
            if on_done:
                on_done(result)

        def failed(msg):
            finish()
            self.status.setText("Erreur")
            if on_error:
                on_error(msg)
            else:
                QtWidgets.QMessageBox.critical(self, APP_NAME, msg)

        worker.signals.progress.connect(self._on_progress)
        worker.signals.done.connect(done)
        worker.signals.failed.connect(failed)
        self.pool.start(worker)

    def _on_progress(self, fraction: float, message: str):
        self.progress.setVisible(True)
        self.progress.setValue(int(fraction * 1000))
        self.status.setText(message)

    def _headphones_loaded(self, entries):
        self.hp_panel.set_entries(self.app.imported_headphones() + entries)
        self.status.setText(f"Index AutoEq chargé ({len(entries)} mesures)")

    def _show_result(self, result):
        self.dirty = False
        if result is not None:
            self.plot.set_data(result.plot_freqs, result.plot_curves)
            notes = [f"Gain appliqué : {result.gain_db:+.1f} dB · {result.fs} Hz"] + result.notes
            self.notes.setText(" · ".join(notes))
        self.status.setText("Filtres appliqués" if self.app.is_running() else "Filtres générés")
        self.refresh_state()

    # -- actions --------------------------------------------------------
    def on_headphone(self, entry: HeadphoneEntry):
        self.app.select_headphone(entry)
        self.mark_dirty()

    def on_speaker(self, speaker_id: str):
        self.app.select_speaker(speaker_id)
        self.mark_dirty()

    def on_setting(self, change: dict):
        self.app.update_settings(**change)
        if any(k.startswith("room_") or k == "speaker_distance" or k == "speaker_angle"
               for k in change):
            self.room_panel.load(self.app.settings)  # bornes/cohérence de la pièce
        self.mark_dirty()

    def on_toggle(self):
        if self.app.is_running():
            self.run_task(self.app.stop, lambda _r: self.status.setText("Simulation arrêtée"),
                          message="Arrêt…")
        else:
            self.run_task(self.app.start, self._show_result, with_progress=True,
                          message="Démarrage…")

    def on_apply(self):
        fn = self.app.apply if self.app.is_running() else self.app.build_filters
        self.run_task(fn, self._show_result, with_progress=True, message="Génération des filtres…")

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
        import os
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
        self.status.setText(f"Casque importé : {entry.label}")
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
            self.status.setText(f"{len(paths)} fichiers exportés vers {folder}")

        self.run_task(lambda progress: self.app.export(folder, progress), done,
                      with_progress=True, message="Export des filtres…")

    def on_autostart(self, enabled: bool):
        def done(_r):
            self.status.setText("Auto-démarrage " + ("activé" if enabled else "désactivé"))

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
