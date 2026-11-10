"""Check the KDE-style coordinate inputs through the desktop's AT-SPI tree."""

import os
import subprocess
import tempfile
import time
from pathlib import Path

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi  # noqa: E402 -- require_version must run first


ROOT = Path(__file__).resolve().parents[1]
NAMES = (
    "X coordinate on the selected screen",
    "Y coordinate on the selected screen",
)
ROLES = {Atspi.Role.SPIN_BUTTON, Atspi.Role.TEXT}


def descendants(node, depth=0):
    if node is None or depth > 20:
        return
    yield node
    for index in range(node.get_child_count()):
        yield from descendants(node.get_child_at_index(index), depth + 1)


def coordinate_roles(pid):
    desktop = Atspi.get_desktop(0)
    for index in range(desktop.get_child_count()):
        app = desktop.get_child_at_index(index)
        if app.get_process_id() == pid:
            found = {name: set() for name in NAMES}
            # Fixed-position fields become visible after selecting that mode.
            for node in descendants(app):
                if (node.get_role() == Atspi.Role.RADIO_BUTTON
                        and node.get_name() == "Fixed position"
                        and not node.get_state_set().contains(Atspi.StateType.CHECKED)):
                    node.get_action_iface().do_action(0)
            for node in descendants(app):
                name = node.get_name()
                if name in found:
                    found[name].add(node.get_role())
            return found
    return {name: set() for name in NAMES}


def main():
    with tempfile.TemporaryDirectory(prefix="klickmeister-a11y-") as directory:
        fixture = Path(directory, "A11yProbe.qml")
        fixture.write_text(
            'import QtQuick\n'
            f'import "{(ROOT / "qml").as_uri()}" as App\n'
            'App.Main { smokeTest: true }\n'
        )
        env = os.environ.copy()
        env["QT_QUICK_CONTROLS_STYLE"] = "org.kde.desktop"
        env["QT_LINUX_ACCESSIBILITY_ALWAYS_ON"] = "1"
        process = subprocess.Popen(
            [
                "/usr/lib/qt6/bin/qmlscene",
                "-I",
                str(ROOT / "tests/qml/mocks"),
                str(fixture),
            ],
            env=env,
            stdout=subprocess.DEVNULL,
            stderr=subprocess.PIPE,
            text=True,
        )
        try:
            deadline = time.monotonic() + 10
            found = {name: set() for name in NAMES}
            while time.monotonic() < deadline and process.poll() is None:
                found = coordinate_roles(process.pid)
                if all(ROLES <= found[name] for name in NAMES):
                    for name in NAMES:
                        print(f"AT-SPI spin button and text: {name}")
                    return
                time.sleep(0.25)
            raise RuntimeError(f"Coordinate accessibility nodes missing: {found}")
        finally:
            process.terminate()
            try:
                _, stderr = process.communicate(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                _, stderr = process.communicate()
            if stderr:
                print(stderr, end="")


if __name__ == "__main__":
    main()
