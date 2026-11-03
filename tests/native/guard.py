"""Supervise only the processes created by the native test runner (Linux)."""

import os
import selectors
import signal
import time


def terminate(process):
    # Every owned Popen starts a new session. Its children inherit this group;
    # they still need cleanup when the group leader has already exited.
    try:
        os.killpg(process.pid, signal.SIGKILL)
    except ProcessLookupError:
        pass
    process.wait(timeout=5)


def describe(returncode):
    if returncode < 0:
        try:
            return f"signal {signal.Signals(-returncode).name}"
        except ValueError:  # Realtime signals have no enum member.
            return f"signal {-returncode}"
    return f"exit code {returncode}"


def supervise(app, target, scenario, timeout, check_target=lambda: None):
    """Target loss must stop the app without relying on focus, AT-SPI or D-Bus."""
    descriptors = []
    try:
        with selectors.DefaultSelector() as selector:
            for process in (target, scenario):
                fd = os.pidfd_open(process.pid)
                descriptors.append(fd)
                selector.register(fd, selectors.EVENT_READ)
            deadline = time.monotonic() + timeout
            while True:
                # Reap a closed app while the scenario verifies cleanup. The
                # scenario only sees the PID vanish; a crash must fail the run.
                code = app.poll()
                if code:
                    raise RuntimeError(f"App exited abnormally: {describe(code)}")
                if target.poll() is not None:
                    raise RuntimeError(
                        "Click target exited: test invalid; stopping app"
                    )
                if scenario.poll() is not None:
                    return scenario.returncode
                if time.monotonic() >= deadline:
                    raise TimeoutError("Native test deadline exceeded")
                check_target()
                selector.select(timeout=0.1)
    finally:
        # Also applies to failed checks, Ctrl+C and errors during setup. The
        # caller may remove the target only after this process has been reaped.
        terminate(app)
        for fd in descriptors:
            os.close(fd)
