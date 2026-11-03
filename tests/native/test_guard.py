"""Guard regression tests: no desktop or real mouse input needed."""

import os
import select
import subprocess
import sys
import tempfile
import time
import unittest
from pathlib import Path

from guard import supervise, terminate


class GuardTests(unittest.TestCase):
    def spawn(self, code):
        p = subprocess.Popen([sys.executable, "-c", code], start_new_session=True)
        self.addCleanup(terminate, p)
        return p

    def test_target_loss_stops_producer(self):
        with tempfile.TemporaryDirectory() as directory:
            log = Path(directory) / "events"
            app = self.spawn(
                f'import time\nf=open({str(log)!r}, "w", buffering=1)\n'
                'while True:\n f.write("event\\n"); time.sleep(.01)'
            )
            scenario = self.spawn("import time; time.sleep(20)")
            target = self.spawn("import time; time.sleep(.2)")
            with self.assertRaisesRegex(RuntimeError, "target exited"):
                supervise(app, target, scenario, 2)
            self.assertIsNotNone(app.poll())
            self.assertTrue(log.read_text())
            count = log.stat().st_size
            time.sleep(0.1)
            self.assertEqual(log.stat().st_size, count)

    def test_invisible_target_stops_app(self):
        app = self.spawn("import time; time.sleep(20)")
        target = self.spawn("import time; time.sleep(20)")
        scenario = self.spawn("import time; time.sleep(20)")

        def lost():
            raise RuntimeError("Target disappeared")

        with self.assertRaisesRegex(RuntimeError, "disappeared"):
            supervise(app, target, scenario, 2, lost)
        self.assertIsNotNone(app.poll())
        self.assertIsNone(target.poll())

    def test_failure_cleanup_stops_descendants_even_after_parent_exit(self):
        for parent_exits in (False, True):
            with (
                self.subTest(parent_exits=parent_exits),
                tempfile.TemporaryDirectory() as directory,
            ):
                pid_file = Path(directory) / "child.pid"
                # The direct fixture owns a session; descendants must inherit
                # its group, just like kwin.py/spectacle launched by a scenario.
                parent = self.spawn(
                    "import subprocess, sys, time\n"
                    "from pathlib import Path\n"
                    'child = subprocess.Popen([sys.executable, "-c", "import time; time.sleep(30)"])\n'
                    f"path = Path({str(pid_file)!r})\n"
                    'path.with_suffix(".tmp").write_text(str(child.pid))\n'
                    'path.with_suffix(".tmp").replace(path)\n'
                    + ("" if parent_exits else "time.sleep(30)\n")
                )
                deadline = time.monotonic() + 5
                while not pid_file.exists() and time.monotonic() < deadline:
                    time.sleep(0.01)
                self.assertTrue(pid_file.exists(), "descendant did not start")
                fd = os.pidfd_open(int(pid_file.read_text()))
                try:
                    if parent_exits:
                        parent.wait(timeout=5)
                    self.assertEqual(select.select([fd], [], [], 0)[0], [])
                    terminate(parent)
                    self.assertIsNotNone(parent.poll())
                    # pidfd readiness proves exit even before the orphan reaper
                    # has removed a killed descendant's /proc entry.
                    self.assertEqual(select.select([fd], [], [], 5)[0], [fd])
                    terminate(parent)  # Repeated cleanup must also be harmless.
                finally:
                    os.close(fd)

    def test_scenario_failure_stops_app_before_target(self):
        app = self.spawn("import time; time.sleep(20)")
        target = self.spawn("import time; time.sleep(20)")
        scenario = self.spawn("raise SystemExit(3)")
        self.assertEqual(supervise(app, target, scenario, 2), 3)
        self.assertIsNotNone(app.poll())
        self.assertIsNone(target.poll())


    def test_app_crash_fails_even_if_scenario_passes(self):
        for code, reason in (
            ("raise SystemExit(101)", "exit code 101"),
            # SIGTERM: like SIGSEGV a deadly signal, but without a core dump.
            ("import os, signal; os.kill(os.getpid(), signal.SIGTERM)", "SIGTERM"),
        ):
            with self.subTest(reason=reason):
                app = self.spawn(code)
                target = self.spawn("import time; time.sleep(20)")
                # Like shutdown_after_input_tests: wait for the app PID to vanish.
                scenario = self.spawn(
                    "import time\nfrom pathlib import Path\n"
                    f"while Path('/proc/{app.pid}').exists(): time.sleep(.01)"
                )
                with self.assertRaisesRegex(RuntimeError, reason):
                    supervise(app, target, scenario, 5)
                self.assertIsNone(target.poll())

    def test_app_clean_exit_keeps_scenario_result(self):
        app = self.spawn("raise SystemExit(0)")
        target = self.spawn("import time; time.sleep(20)")
        scenario = self.spawn(
            "import time\nfrom pathlib import Path\n"
            f"while Path('/proc/{app.pid}').exists(): time.sleep(.01)"
        )
        self.assertEqual(supervise(app, target, scenario, 5), 0)
        self.assertEqual(app.returncode, 0)
        self.assertIsNone(target.poll())

if __name__ == "__main__":
    unittest.main()
