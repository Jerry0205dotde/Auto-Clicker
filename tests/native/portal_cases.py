"""Native KDE portal input regression with position selection."""

import json
import time
from pathlib import Path

from access import Atspi, action, nodes, walk
from control import BASE, mark, rpc, wait_for
from scenarios import (
    choose,
    fixed_point,
    focus_app,
    focus_target,
    key,
    kwin,
    mouse_events,
    run_counted,
    status,
)

PORTAL_APP = "xdg-desktop-portal-kde"


def frame(title):
    return any(
        n.get_role_name() == "frame" and n.get_name() == title
        for n, _ in nodes(PORTAL_APP)
    )


def portal_button(title, names, position, count):
    """German KDE 6.7 fallback for buttons whose AT-SPI names are empty.

    The visible labels/order were verified visually in this version's dialog.
    Fail on a changed layout; never guess another dialog.
    """
    frames = [n for n, _ in nodes(PORTAL_APP) if n.get_role_name() == "frame"]
    assert len(frames) == 1 and frames[0].get_name() == title
    buttons = [
        n
        for n, _ in walk(frames[0])
        if n.get_role_name() == "button"
        and n.get_state_set().contains(Atspi.StateType.SHOWING)
    ]
    named = [n for n in buttons if n.get_name() in names]
    if len(named) == 1:
        button = named[0]
    else:
        assert len(buttons) == count and all(not n.get_name() for n in buttons)
        buttons.sort(
            key=lambda n: Atspi.Component.get_extents(n, Atspi.CoordType.SCREEN).x
        )
        button = buttons[position]
    assert button.get_state_set().contains(Atspi.StateType.ENABLED)
    assert button.get_action_iface().do_action(0)


def grant_remote():
    wait_for(lambda: frame("Fernsteuerung"), timeout=10)
    kwin("focus_portal")
    # Some KDE 6.7 builds name the accept button "OK" instead.
    portal_button("Fernsteuerung", ("Akzeptieren", "OK"), 0, 2)


def open_picker():
    action("Auswählen …", role="button")
    wait_for(lambda: "Esc / Rechtsklick" in picker_text())


def picker_text():
    return "\n".join(n.get_name() for n, _ in nodes() if n.get_role_name() == "label")


def focus_picker():
    kwin("focus_picker")
    wait_for(
        lambda: any(w["kind"] == "picker" and w["active"] for w in kwin("snapshot"))
    )
    # KWin activation reaches the Qt Wayland client asynchronously.
    time.sleep(0.15)


def pin_target():
    # A fresh picker follows the pointer: explicitly pin and verify every time.
    focus_picker()
    rpc("driver", cmd="move", x=80, y=400)
    rpc("driver", cmd="click")
    wait_for(lambda: "X: 80 · Y: 400" in picker_text())
    key(0xFF0D)
    wait_for(lambda: fixed_point() == (80, 400))


def main():
    grant_remote()
    wait_for(lambda: rpc("driver", cmd="state")["ready"])
    kwin("place")
    wait_for(lambda: "Bereit" in status())
    open_picker()
    pin_target()

    # Exercise actual clicks and Stop after position selection.
    before = len(mouse_events())
    focus_target()
    action("Starten", role="button")
    grant_remote()
    wait_for(lambda: "Gestoppt" in status())
    new = mouse_events()[before:]
    assert len(new) == 6, new
    assert all(e["x"] == 80 and e["y"] == 400 for e in new)
    assert [e["type"] == "MouseButtonRelease" for e in new] == [False, True] * 3
    mark("clicks_after_position_selection", passed=True, mouse_events=len(new))
    action("Bis zum Stoppen", role="radio button")
    focus_target()
    action("Starten", role="button")
    wait_for(lambda: "Klicken aktiv" in status())
    wait_for(lambda: len(mouse_events()) >= before + 10)
    action("Stoppen", role="button")
    wait_for(lambda: "Gestoppt" in status())
    count = len(mouse_events())
    time.sleep(0.3)
    assert len(mouse_events()) == count
    mark("stop_after_position_selection", passed=True, quiet_ms=300)

    # Exercise the visible option buttons and verify their checked states.
    action("Klickzyklen", role="radio button")
    for index, button in enumerate([1, 2, 4]):
        choose("Maustaste", index)
        for click_type in [0, 1]:
            choose("Klickart", click_type)
            run_counted(
                "native_click_matrix",
                button=button,
                double=bool(click_type),
                hotkey=False,
            )
    focus_target()
    rpc("driver", cmd="key_state", keysym=0xFFE9, state=1)
    try:
        key(0xFFC1)  # Alt+F4 must not remove the sink.
    finally:
        rpc("driver", cmd="key_state", keysym=0xFFE9, state=0)
    time.sleep(0.1)
    assert rpc("target", cmd="state")["visible"]
    mark("target_ignores_alt_f4", passed=True)

    focus_app()
    started = time.monotonic()
    kwin("close_app")
    pid = json.loads((BASE / "pids.json").read_text())["app"]
    wait_for(lambda: not Path(f"/proc/{pid}").exists())
    mark(
        "shutdown_after_input_tests",
        passed=True,
        elapsed_ms=(time.monotonic() - started) * 1000,
    )


if __name__ == "__main__":
    main()
