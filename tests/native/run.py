"""Run a native scenario with an owned click target and fail-closed supervision."""

import argparse
import json
import os
import shutil
import signal
import subprocess
import sys
import tempfile
import time
from pathlib import Path

from guard import supervise, terminate

APP_ID = "io.github.jerry0205.klickmeister"
# Without Registry, older portal frontends identify the app by its launch context.
NO_REGISTRY = {
    "org.freedesktop.DBus.Error.ServiceUnknown",
    "org.freedesktop.DBus.Error.UnknownInterface",
    "org.freedesktop.DBus.Error.UnknownMethod",
}


def portal_register():
    """Register a throwaway connection under the app ID, as the app does."""
    import dbus

    bus = dbus.bus.BusConnection(dbus.bus.BUS_SESSION)
    try:
        bus.get_object(
            "org.freedesktop.portal.Desktop", "/org/freedesktop/portal/desktop"
        ).Register(
            APP_ID,
            {},
            dbus_interface="org.freedesktop.host.portal.Registry",
            signature="sa{sv}",
        )
    finally:
        bus.close()


def registration_problem(register=portal_register):
    """Explain why the portal would reject the app ID, or return None.

    The portal resolves the desktop file with its own PATH and drops it when
    Exec=klickmeister is not found there, so an installed file is not enough.
    """
    try:
        register()
    except Exception as error:
        name = getattr(error, "get_dbus_name", lambda: None)()
        if name is None:
            raise
        if name in NO_REGISTRY:
            return None
        return (
            f"The portal rejects the app ID {APP_ID}: {error}\n"
            f"Install {APP_ID}.desktop into an applications directory and let the "
            "portal resolve its Exec: link the build into the portal's PATH, use a "
            "local desktop file with an absolute Exec, or install the package. "
            "See CONTRIBUTING.md, Desktop-Integration."
        )
    return None


def monitor_identity(screen):
    """Match JSON.stringify in MonitorSelection.qml, including integral floats."""
    scale = screen["scale"]
    if isinstance(scale, float) and scale.is_integer():
        scale = int(scale)
    identity = [
        screen["name"],
        screen["manufacturer"],
        screen["model"],
        screen["serial"],
        screen["width"],
        screen["height"],
        scale,
    ]
    return json.dumps(identity, ensure_ascii=False, separators=(",", ":"))


def config_text(screen):
    # A JSON string is a TOML basic string, except that TOML also forbids a raw DEL.
    identity = json.dumps(monitor_identity(screen), ensure_ascii=False)
    return (
        'interval_ms = 100\nmouse_button = "left"\nclick_type = "single"\n'
        'repeat_mode = "count"\nrepeat_count = 3\nposition_mode = "fixed"\n'
        'fixed_x = 80\nfixed_y = 400\nhotkey = "Pause"\nmonitor_identity = '
        + identity.replace("\x7f", "\\u007f")
        + "\n"
    )


def run(binary, timeout, scenario_path, scenario_args=(), keep_artifacts=True):
    problem = registration_problem()
    if problem:
        raise SystemExit(problem)
    base = Path(tempfile.mkdtemp(prefix="klickmeister-native-"))
    os.environ["KLICKMEISTER_NATIVE_DIR"] = str(base)
    os.environ["XDG_CONFIG_HOME"] = str(base / "config")
    os.environ["QT_QPA_PLATFORM"] = "wayland"
    os.environ["QT_LINUX_ACCESSIBILITY_ALWAYS_ON"] = "1"
    print(f"Native artifacts: {base}", flush=True)
    from control import rpc, wait_for

    processes = {}
    logs = []

    def start(name, command):
        log = (base / f"{name}.log").open("w")
        logs.append(log)
        processes[name] = subprocess.Popen(
            command, stdout=log, stderr=subprocess.STDOUT, start_new_session=True
        )
        return processes[name]

    def stopped(signum, frame):
        raise SystemExit(128 + signum)

    signal.signal(signal.SIGTERM, stopped)
    try:
        target = start(
            "target", [sys.executable, str(Path(__file__).with_name("target.py"))]
        )
        wait_for(lambda: (base / "target.sock").exists())
        state = rpc("target", cmd="state")
        assert state["visible"], "Target is not mapped"
        config = base / "config/klickmeister/config.toml"
        config.parent.mkdir(parents=True)
        config.write_text(config_text(state["screen"]), encoding="utf-8")
        start("driver", [sys.executable, str(Path(__file__).with_name("driver.py"))])
        app = start("app", [str(binary.resolve())])
        (base / "pids.json").write_text(
            json.dumps({name: p.pid for name, p in processes.items()})
        )
        scenario = start(
            "scenario", [sys.executable, str(scenario_path.resolve()), *scenario_args]
        )
        last_check = 0

        def check_target():
            nonlocal last_check
            if processes["driver"].poll() is not None:
                raise RuntimeError("Input driver exited")
            if time.monotonic() - last_check >= 0.25:
                assert rpc("target", cmd="state", timeout=0.1)["visible"], (
                    "Click target disappeared"
                )
                last_check = time.monotonic()

        return supervise(app, target, scenario, timeout, check_target)
    finally:
        # Never close the sink while either the app or scenario can send input.
        for name in ("scenario", "app", "driver", "target"):
            if name in processes:
                terminate(processes[name])
        for log in logs:
            log.close()
        path = base / "scenario.log"
        if path.exists():
            print(path.read_text(), end="")
        if not keep_artifacts:
            shutil.rmtree(base, ignore_errors=True)
            print(f"Removed native artifacts: {base}", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--binary", type=Path, default=Path("target/release/klickmeister")
    )
    parser.add_argument("--timeout", type=float, default=120)
    parser.add_argument("scenario", type=Path)
    parser.add_argument("args", nargs=argparse.REMAINDER)
    args = parser.parse_args()
    return run(args.binary, args.timeout, args.scenario, args.args)


if __name__ == "__main__":
    sys.exit(main())
