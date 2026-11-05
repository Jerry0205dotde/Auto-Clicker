import json
import os
import sys
import time

from PySide6.QtCore import QEvent, Qt, QTimer
from PySide6.QtGui import QColor, QPainter, QPen
from PySide6.QtNetwork import QLocalServer
from PySide6.QtWidgets import QApplication, QWidget

base = os.environ["KLICKMEISTER_NATIVE_DIR"]
app = QApplication(sys.argv)
app.setApplicationName("Klickmeister Native Test Target")
log = open(base + "/mouse-events.jsonl", "a", buffering=1)
# "event" repaints after every mouse event. "limited" repaints at most every
# REPAINT_MS: a full-screen repaint per event costs the target more CPU than
# the clicker and caps the measured click rate. Logging is the same in both.
REPAINT = os.environ.get("KLICKMEISTER_NATIVE_TARGET_REPAINT", "event")
if REPAINT not in ("event", "limited"):
    raise RuntimeError(f"Unbekannter Neuzeichnen-Modus: {REPAINT}")
REPAINT_MS = 100


class Target(QWidget):
    def __init__(self):
        super().__init__()
        self.setWindowTitle("Klickmeister – isoliertes Klicktestziel")
        self.setFocusPolicy(Qt.StrongFocus)
        self.setMouseTracking(True)
        self.count = 0
        self.last_point = None
        self.repaint_timer = QTimer(self)
        self.repaint_timer.setSingleShot(True)
        self.repaint_timer.setInterval(REPAINT_MS)
        self.repaint_timer.timeout.connect(self.update)

    def paintEvent(self, event):
        p = QPainter(self)
        p.fillRect(self.rect(), QColor("#152334"))
        p.setPen(QColor("white"))
        p.drawText(
            30, 50, f"Klickmeister: eigenes Testziel auf {self.screen().name()}"
        )
        p.drawText(30, 80, f"Empfangene Mausereignisse: {self.count}")
        point = self.last_point or (80, 400)
        p.drawText(
            30,
            110,
            f"Zuletzt empfangen: X={point[0]:g}, Y={point[1]:g}"
            if self.last_point is not None
            else "Noch kein Klick empfangen. Jede Position in diesem Fenster ist ein Testziel.",
        )
        x, y = round(point[0]), round(point[1])
        p.setPen(QPen(QColor("#5eeab0"), 3))
        p.drawEllipse(x - 30, y - 30, 60, 60)
        p.drawLine(x - 15, y, x + 15, y)
        p.drawLine(x, y - 15, x, y + 15)

    def closeEvent(self, event):
        # Keep the sink mapped even if a window-manager shortcut is delivered
        # here while clicks repeatedly transfer focus back to the target.
        event.ignore()

    def event(self, e):
        if e.type() in (
            QEvent.MouseButtonPress,
            QEvent.MouseButtonDblClick,
            QEvent.MouseButtonRelease,
        ):
            self.count += 1
            self.last_point = (e.position().x(), e.position().y())
            log.write(
                json.dumps(
                    {
                        "t_ns": time.monotonic_ns(),
                        "type": e.type().name,
                        "button": e.button().value,
                        "buttons": e.buttons().value,
                        "x": e.position().x(),
                        "y": e.position().y(),
                        "global_x": e.globalPosition().x(),
                        "global_y": e.globalPosition().y(),
                    }
                )
                + "\n"
            )
            if REPAINT == "event":
                self.update()
            elif not self.repaint_timer.isActive():
                self.repaint_timer.start()
        elif e.type() in (QEvent.KeyPress, QEvent.KeyRelease):
            log.write(
                json.dumps(
                    {"t_ns": time.monotonic_ns(), "type": e.type().name, "key": e.key()}
                )
                + "\n"
            )
        return super().event(e)


w = Target()
screen_name = os.environ.get("KLICKMEISTER_NATIVE_SCREEN")
if screen_name:
    matches = [screen for screen in app.screens() if screen.name() == screen_name]
    if len(matches) != 1:
        raise RuntimeError(f"Testbildschirm nicht eindeutig verfügbar: {screen_name}")
    w.setScreen(matches[0])
    w.move(matches[0].geometry().topLeft())
w.showFullScreen()
server = QLocalServer()
assert server.listen(base + "/target.sock")
connections = []


def accept():
    c = server.nextPendingConnection()
    connections.append(c)

    def read():
        while c.canReadLine():
            request = json.loads(bytes(c.readLine()))
            cmd = request["cmd"]
            if cmd == "activate":
                w.showFullScreen()
                w.raise_()
                w.activateWindow()
            elif cmd == "quit":
                QTimer.singleShot(50, app.quit)
            elif cmd == "grab":
                w.grab().save(request["path"])
            s = w.screen()
            g = s.geometry()
            response = {
                "count": w.count,
                "active": w.isActiveWindow(),
                "visible": w.isVisible(),
                "geometry": [g.x(), g.y(), g.width(), g.height()],
                "repaint": REPAINT,
                "screen": {
                    "name": s.name(),
                    "manufacturer": s.manufacturer(),
                    "model": s.model(),
                    "serial": s.serialNumber(),
                    "width": g.width(),
                    "height": g.height(),
                    "scale": s.devicePixelRatio(),
                },
            }
            c.write((json.dumps(response) + "\n").encode())
            c.flush()

    c.readyRead.connect(read)

    def disconnected():
        connections.remove(c)
        c.deleteLater()

    c.disconnected.connect(disconnected)


server.newConnection.connect(accept)
# The click target must outlive all click runs. The orchestrator closes it only
# after the application has exited; an independent timeout is unsafe here.
sys.exit(app.exec())
