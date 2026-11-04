"""Measure real click throughput, jitter and stop latency of a built Klickmeister.

Runs benchmark_cases.py under the supervision of run.py. Results are measured
values for this machine and session, not pass/fail limits.
"""

import argparse
import sys
from pathlib import Path

from benchmark_metrics import CLICKS, cycles_for

# Shortest interval the app accepts per mode: 500 single clicks/s.
MIN_INTERVAL_MS = {"single": 2, "double": 4}
# How much longer than planned a counted run may take before it counts as hung.
SLOW_RUN_FACTOR = 4


def interval_list(text):
    values = [int(part) for part in text.split(",")]
    if any(value <= 0 for value in values):
        raise argparse.ArgumentTypeError("Intervals must be positive")
    return values


def rate_list(text):
    values = [int(part) for part in text.split(",")]
    if any(value <= 0 or 1000 % value for value in values):
        raise argparse.ArgumentTypeError("Rates must divide 1000 into whole ms")
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
        "--rates",
        type=rate_list,
        help="planned single clicks/s instead of --intervals, e.g. 100,250,500",
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
        args.intervals, args.duration, args.rates = [10], 60, None
    if args.rates:
        # A double click needs twice the interval for the same click rate.
        args.runs = [
            (mode, CLICKS[mode] * 1000 // rate)
            for rate in args.rates
            for mode in args.modes
        ]
        args.stop_interval = 1000 // max(args.rates)
    else:
        args.runs = [
            (mode, interval) for interval in args.intervals for mode in args.modes
        ]
        args.stop_interval = min(args.intervals)
    for mode, interval in args.runs + [("single", args.stop_interval)]:
        if interval < MIN_INTERVAL_MS[mode]:
            parser.error(f"{mode} needs at least {MIN_INTERVAL_MS[mode]} ms")
    return args


def estimated_timeout(args):
    # Generous: at 500 clicks/s the portal reaches less than half the planned
    # rate, and every run first configures the UI. It only bounds a hung run.
    runs = sum(
        SLOW_RUN_FACTOR * cycles_for(args.duration, interval) * interval / 1000 + 30
        for _, interval in args.runs
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
