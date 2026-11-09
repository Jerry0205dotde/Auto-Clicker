"""Native benchmark scenario; start it with benchmark.py, not directly.

Measured runs touch only GUI nodes resolved once beforehand and read only new
lines of the target's event log. Setup between runs may walk the AT-SPI tree.
"""

import json
import os
import platform
import subprocess
import time
import tomllib
from pathlib import Path

from access import Atspi, action, find, nodes
from benchmark import SLOW_RUN_FACTOR, options
from benchmark_metrics import CLICKS, analyze, check_events, cycles_for
from control import BASE, mark, rpc, wait_for
from portal_cases import PORTAL_APP, frame, grant_remote
from scenarios import choose, fixed_point, focus_app, focus_target, key, kwin, status

PIDS = json.loads((BASE / "pids.json").read_text())
HOTKEY = 0xFF13  # Pause, the suggested key; main() checks that it is bound
TICKS = os.sysconf("SC_CLK_TCK")
TARGET_REPAINT = {
    "limited": "Neuzeichnen höchstens alle 100 ms (--target-repaint limited)",
    "event": "Neuzeichnen nach jedem Mausereignis (--target-repaint event)",
}


class EventTail:
    """Mouse events that the target logged after this object was created."""

    def __init__(self):
        self.file = (BASE / "mouse-events.jsonl").open()
        self.file.seek(0, os.SEEK_END)
        self.partial = ""
        self.events = []

    def poll(self):
        lines = (self.partial + self.file.read()).split("\n")
        self.partial = lines.pop()
        self.events += [
            e for e in map(json.loads, filter(None, lines)) if "button" in e
        ]
        return self.events


def bound_hotkey():
    """The hotkey as the portal bound it, read from the footer label."""
    prefix = "Tastenkürzel: "
    name = next(
        (n.get_name() for n, _ in nodes() if (n.get_name() or "").startswith(prefix)),
        prefix,
    )
    return name.removeprefix(prefix)


class Gui:
    """Status label and run button, resolved once for cheap polling."""

    def __init__(self):
        self.status = next(
            n for n, _ in nodes() if (n.get_name() or "").startswith("Status:")
        )
        self.button = find("Starten", role="button")

    def text(self):
        self.status.clear_cache()
        return self.status.get_name()

    def press(self):
        assert self.button.get_action_iface().do_action(0)


class Usage:
    """CPU time of the app, the target and this observer at one instant."""

    def __init__(self):
        self.wall = time.monotonic()
        self.app = cpu_seconds(PIDS["app"])
        self.target = cpu_seconds(PIDS["target"])
        self.observer = time.process_time()

    def since(self, start):
        wall = self.wall - start.wall
        return {
            f"{name}_cpu_pct": 100 * (getattr(self, name) - getattr(start, name)) / wall
            for name in ("app", "target", "observer")
        }


def cpu_seconds(pid):
    fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
    return (int(fields[11]) + int(fields[12])) / TICKS  # utime + stime


def memory(pid):
    rows = dict(
        line.split(":", 1)
        for line in Path(f"/proc/{pid}/status").read_text().splitlines()
    )
    return {
        "app_rss_kib": int(rows["VmRSS"].split()[0]),
        "app_peak_rss_kib": int(rows["VmHWM"].split()[0]),
    }


def poll_until(predicate, timeout, step=0.005):
    deadline = time.monotonic() + timeout
    while True:
        result = predicate()
        if result or time.monotonic() >= deadline:
            return result
        time.sleep(step)


def command(*args):
    try:
        return subprocess.run(
            args, capture_output=True, text=True, timeout=10, check=True
        ).stdout.strip()
    except (OSError, subprocess.SubprocessError):
        return None


def environment():
    app = PIDS["app"]
    binary = Path(os.readlink(f"/proc/{app}/exe"))
    repo = Path(__file__).resolve().parents[2]
    commit = command("git", "-C", str(repo), "rev-parse", "HEAD")
    changes = command("git", "-C", str(repo), "status", "--porcelain")
    maps = Path(f"/proc/{app}/maps").read_text().splitlines()
    qt = sorted(
        {Path(line.split()[-1]).name for line in maps if "libQt6Core.so" in line}
    )
    release = dict(
        line.split("=", 1)
        for line in Path("/etc/os-release").read_text().splitlines()
        if "=" in line
    )
    cpus = Path("/proc/cpuinfo").read_text().splitlines()
    from PySide6 import QtCore

    state = rpc("target", cmd="state")
    return {
        "commit": commit,
        "uncommitted_changes": None if changes is None else bool(changes),
        "binary": str(binary),
        "build_profile": binary.parent.name
        if binary.parent.name in ("release", "debug")
        else None,
        "binary_modified": time.strftime(
            "%Y-%m-%d %H:%M:%S", time.localtime(binary.stat().st_mtime)
        ),
        "os": release.get("PRETTY_NAME", "").strip('"'),
        "kernel": platform.release(),
        "cpu": next(
            (
                line.split(":", 1)[1].strip()
                for line in cpus
                if line.startswith("model name")
            ),
            None,
        ),
        "cpu_count": os.cpu_count(),
        "app_qt_core": qt,
        "target_qt": QtCore.qVersion(),
        "kwin": command("kwin_wayland", "--version"),
        "session": os.environ.get("XDG_SESSION_TYPE"),
        "desktop": os.environ.get("XDG_CURRENT_DESKTOP"),
        "screen": state["screen"],
        "target_repaint": state["repaint"],
    }


def set_spin(name, value):
    """Type a value like a user; QML only reports interactive edits."""

    def current():
        return round(Atspi.Value.get_current_value(find(name, role="spin button")))

    if current() == value:
        return
    focus_app()
    assert Atspi.Component.grab_focus(find(name, role="spin button"))
    rpc("driver", cmd="key_state", keysym=0xFFE3, state=1)  # Control_L
    try:
        key(0x61)  # a: select the whole text
    finally:
        rpc("driver", cmd="key_state", keysym=0xFFE3, state=0)
    for digit in str(value):
        key(ord(digit))
    key(0xFF09)  # Tab commits the edit without activating anything
    wait_for(lambda: current() == value)


def configure(mode, interval, cycles=None):
    choose("Klickart", ["single", "double"].index(mode))
    if cycles is None:
        action("Bis zum Stoppen", role="radio button")
    else:
        action("Klickzyklen", role="radio button")
        set_spin("Anzahl der Klickzyklen", cycles)
    set_spin("Klickintervall in Millisekunden", interval)


def check_saved(mode, interval, cycles):
    saved = tomllib.loads((BASE / "config/klickmeister/config.toml").read_text())
    assert saved["interval_ms"] == interval, saved
    assert saved["click_type"] == mode, saved
    assert saved["repeat_mode"] == ("count" if cycles else "until_stopped"), saved
    if cycles:
        assert saved["repeat_count"] == cycles, saved


def portal_frames():
    return [n.get_name() for n, _ in nodes(PORTAL_APP) if n.get_role_name() == "frame"]


def warm_up(gui):
    """The first start asks for permission; keep that out of measured runs."""
    configure("single", 100, 3)
    tail = EventTail()
    focus_target()
    triggered = time.monotonic_ns()
    gui.press()
    deadline = time.monotonic() + 60
    while not tail.poll():
        assert time.monotonic() < deadline, "No clicks after the first start"
        if frame("Fernsteuerung"):
            grant_remote()
        time.sleep(0.1)
    wait_for(lambda: len(tail.poll()) >= 6)
    wait_for(lambda: "Gestoppt" in gui.text())
    check_events(tail.poll(), button=1, point=(80, 400))
    mark(
        "benchmark_first_start",
        passed=True,
        start_to_first_event_ms=(tail.events[0]["t_ns"] - triggered) / 1e6,
        note="includes the permission dialog when one was shown",
    )


def counted_run(gui, mode, interval, cycles, quiet):
    configure(mode, interval, cycles)
    expected = 2 * CLICKS[mode] * cycles
    tail = EventTail()
    focus_target()
    triggered = time.monotonic_ns()
    gui.press()
    assert poll_until(tail.poll, 10), "Run did not start"
    first = Usage()
    assert poll_until(
        lambda: len(tail.poll()) >= expected,
        SLOW_RUN_FACTOR * cycles * interval / 1000 + 10,
    ), (len(tail.events), expected)
    last = Usage()
    wait_for(lambda: "Gestoppt" in gui.text())
    time.sleep(quiet)
    result = analyze(tail.poll(), clicks_per_cycle=CLICKS[mode])
    assert result["cycles"] == cycles and result["complete_cycles"], result
    check_saved(mode, interval, cycles)
    mark(
        "benchmark_run",
        passed=True,
        mode=mode,
        interval_ms=interval,
        target_cycles_per_s=1000 / interval,
        target_clicks_per_s=CLICKS[mode] * 1000 / interval,
        start_to_first_event_ms=(tail.events[0]["t_ns"] - triggered) / 1e6,
        **result,
        **last.since(first),
        **memory(PIDS["app"]),
    )


def stop_run(gui, method, interval, stop_after, quiet):
    configure("single", interval)
    tail = EventTail()
    focus_target()
    start = (lambda: key(HOTKEY)) if method == "hotkey" else gui.press
    start()
    assert poll_until(tail.poll, 10), "Run did not start"
    wait = tail.events[0]["t_ns"] / 1e9 + stop_after - time.monotonic()
    time.sleep(max(0, wait))
    requested = time.monotonic_ns()
    start()  # The same button and hotkey stop a running run.
    returned = time.monotonic_ns()
    assert poll_until(lambda: "Gestoppt" in gui.text(), 5), gui.text()
    stopped = time.monotonic_ns()
    count = len(tail.poll())
    time.sleep(quiet)
    events = tail.poll()
    assert len(events) == count, f"{len(events) - count} events after 'Gestoppt'"
    check_events(events, button=1, point=(80, 400))
    check_saved("single", interval, None)
    late = [e for e in events if e["t_ns"] > requested]
    mark(
        "benchmark_stop",
        passed=True,
        method=method,
        interval_ms=interval,
        clicks=len(events) // 2,
        request_call_ms=(returned - requested) / 1e6,
        request_to_status_ms=(stopped - requested) / 1e6,
        events_after_request=len(late),
        request_to_last_event_ms=(late[-1]["t_ns"] - requested) / 1e6 if late else 0,
        quiet_s=quiet,
    )


def table(rows, columns):
    lines = ["  ".join(f"{title:>{width}}" for title, _, width, _ in columns)]
    for row in rows:
        lines.append(
            "  ".join(f"{row[key]:>{width}{spec}}" for _, key, width, spec in columns)
        )
    return "\n".join(lines)


def report(env):
    rows = [
        json.loads(line) for line in (BASE / "results.jsonl").read_text().splitlines()
    ]
    runs = [r for r in rows if r["test"] == "benchmark_run"]
    stops = [r for r in rows if r["test"] == "benchmark_stop"]
    print("\nUmgebung:", json.dumps(env, ensure_ascii=False))
    print(f"\nTestziel: {TARGET_REPAINT[env['target_repaint']]}")
    print("\nKlickläufe (Abstände der Zyklusbeginne in ms):")
    print(
        table(
            runs,
            [
                ("Modus", "mode", 6, ""),
                ("ms", "interval_ms", 4, ""),
                ("Zyklen", "cycles", 6, ""),
                ("Soll-Zyklen/s", "target_cycles_per_s", 13, ".2f"),
                ("Ist-Zyklen/s", "cycles_per_s", 12, ".2f"),
                ("Soll-Klicks/s", "target_clicks_per_s", 13, ".2f"),
                ("Ist-Klicks/s", "clicks_per_s", 12, ".2f"),
                ("Median", "gap_median_ms", 6, ".2f"),
                ("P95", "gap_p95_ms", 6, ".2f"),
                ("P99", "gap_p99_ms", 6, ".2f"),
                ("Max", "gap_max_ms", 6, ".2f"),
                ("Start", "start_to_first_event_ms", 6, ".1f"),
                ("App-CPU%", "app_cpu_pct", 8, ".1f"),
                ("RSS-KiB", "app_rss_kib", 8, ""),
            ],
        )
    )
    if stops:
        print("\nStopp (ab Anforderung, in ms):")
        print(
            table(
                stops,
                [
                    ("Weg", "method", 6, ""),
                    ("ms", "interval_ms", 4, ""),
                    ("Aufruf", "request_call_ms", 7, ".1f"),
                    ("Status", "request_to_status_ms", 7, ".1f"),
                    ("Letztes Ereignis", "request_to_last_event_ms", 16, ".1f"),
                    ("Ereignisse danach", "events_after_request", 17, ""),
                ],
            )
        )


def main():
    args = options()
    # A fresh session first asks to confirm the app's global shortcut. Leave
    # that to the user and grant the input driver only an unambiguous dialog.
    wait_for(lambda: "Bereit" in status(), timeout=120)
    wait_for(lambda: portal_frames() == ["Fernsteuerung"], timeout=30)
    grant_remote()
    wait_for(lambda: rpc("driver", cmd="state")["ready"], timeout=30)
    kwin("place")
    assert fixed_point() == (80, 400), "run.py writes a fixed position at (80, 400)"
    # The portal may bind another key than the suggested Pause. Pressing
    # Pause then never starts a run, so fail before the first measurement.
    hotkey = bound_hotkey()
    assert not args.stop_repeats or hotkey == "Pause", (
        "The hotkey stop measurements press Pause, but the bound hotkey is "
        f"{hotkey!r}. Change it to Pause or pass --stop-repeats 0."
    )
    env = environment()
    mark("benchmark_environment", **env)
    gui = Gui()
    warm_up(gui)
    for mode, interval in args.runs:
        counted_run(
            gui, mode, interval, cycles_for(args.duration, interval), args.quiet
        )
    for method in ("button", "hotkey"):
        for _ in range(args.stop_repeats):
            stop_run(gui, method, args.stop_interval, args.stop_after, args.quiet)
    report(env)


if __name__ == "__main__":
    main()
