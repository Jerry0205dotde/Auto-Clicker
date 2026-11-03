"""Measure real click throughput, jitter and stop latency of a built Klickmeister.

Runs benchmark_cases.py under the supervision of run.py. Results are measured
values for this machine and session, not pass/fail limits.
"""

import argparse
import sys
from pathlib import Path

from benchmark_metrics import cycles_for


def interval_list(text):
    values = [int(part) for part in text.split(",")]
    if any(value <= 0 for value in values):
        raise argparse.ArgumentTypeError("Intervals must be positive")
    return values


def mode_list(text):
    values = text.split(",")
    if not set(values) <= {"single", "double"}:
        raise argparse.ArgumentTypeError("Modes are single and/or double")
    return values


def options(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--binary", type=Path, default=Path("target/release/klickmeister")
    )
    parser.add_argument(
        "--keep-artifacts",
        action="store_true",
        help="keep the temporary directory with logs and raw events",
    )
    parser.add_argument(
        "--timeout", type=float, help="overall limit in seconds (default: estimated)"
    )
    parser.add_argument("--modes", type=mode_list, default=["single", "double"])
    parser.add_argument(
        "--intervals",
        type=interval_list,
        default=[100, 20, 10],
        help="ms, e.g. 100,20,10",
    )
    parser.add_argument(
        "--duration",
        type=float,
        default=5,
        help="ideal seconds per counted run; sets the cycle count",
    )
    parser.add_argument(
        "--long",
        action="store_true",
        help="one 60 s run per mode at 10 ms instead of the interval matrix",
    )
    parser.add_argument(
        "--stop-repeats",
        type=int,
        default=3,
        help="stop measurements per method (button, hotkey); 0 skips them",
    )
    parser.add_argument(
        "--stop-after", type=float, default=1, help="seconds of clicking before a stop"
    )
    parser.add_argument(
        "--quiet",
        type=float,
        default=1,
        help="seconds without new events required after a run has stopped",
    )
    args = parser.parse_args(argv)
    if args.long:
        args.intervals, args.duration = [10], 60
    return args


def estimated_timeout(args):
    # Generous: a slow run may take twice its ideal time, and every run first
    # configures the UI. The limit only bounds a hung run.
    runs = sum(
        2 * cycles_for(args.duration, interval) * interval / 1000 + 30
        for interval in args.intervals
        for _ in args.modes
    )
    stops = 2 * args.stop_repeats * (args.stop_after + args.quiet + 30)
    return 180 + runs + stops


def main():
    args = options()
    from run import run

    return run(
        args.binary,
        args.timeout or estimated_timeout(args),
        Path(__file__).with_name("benchmark_cases.py"),
        sys.argv[1:],
        keep_artifacts=args.keep_artifacts,
    )


if __name__ == "__main__":
    sys.exit(main())
