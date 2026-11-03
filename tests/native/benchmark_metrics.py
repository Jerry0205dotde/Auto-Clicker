"""Validate received mouse events and derive throughput and jitter figures.

Pure functions without desktop dependencies, so they run in the unit tests.
"""

import math
import statistics

PRESSES = ("MouseButtonPress", "MouseButtonDblClick")
RELEASE = "MouseButtonRelease"


def percentile(values, fraction):
    """Nearest-rank percentile: always one of the measured values."""
    ordered = sorted(values)
    return ordered[max(0, math.ceil(fraction * len(ordered)) - 1)]


def check_events(events, *, button, point):
    """Every click is one press and one release of the configured button."""
    if len(events) % 2:
        raise AssertionError(f"Incomplete press/release pair: {len(events)} events")
    for index, event in enumerate(events):
        # Qt reports a second press within the double-click time as
        # MouseButtonDblClick. It still starts one individual click.
        expected = (RELEASE,) if index % 2 else PRESSES
        if event["type"] not in expected:
            raise AssertionError(
                f"Event {index} is {event['type']}, expected {expected}"
            )
        if event["button"] != button:
            raise AssertionError(f"Event {index} uses button {event['button']}")
        if (event["x"], event["y"]) != point:
            raise AssertionError(f"Event {index} at {(event['x'], event['y'])}")
        held = 0 if event["type"] == RELEASE else button
        if event["buttons"] != held:
            raise AssertionError(
                f"Event {index} reports held buttons {event['buttons']}"
            )


def analyze(events, *, clicks_per_cycle, button=1, point=(80, 400)):
    """Cycle rate from the receive time of the first event of every cycle.

    Rates use (cycles - 1) / (last start - first start). Individual clicks per
    second are the cycle rate times the clicks of one cycle.
    """
    check_events(events, button=button, point=point)
    per_cycle = 2 * clicks_per_cycle
    starts = [e["t_ns"] for e in events[::per_cycle]]
    gaps = [(b - a) / 1e6 for a, b in zip(starts, starts[1:])]
    result = {
        "mouse_events": len(events),
        "clicks": len(events) // 2,
        "double_click_events": sum(e["type"] == "MouseButtonDblClick" for e in events),
        "cycles": len(events) // per_cycle,
        "complete_cycles": len(events) % per_cycle == 0,
    }
    if len(starts) >= 2:
        span = starts[-1] - starts[0]
        cycles_per_s = (len(starts) - 1) * 1e9 / span
        result.update(
            run_s=span / 1e9,
            cycles_per_s=cycles_per_s,
            clicks_per_s=cycles_per_s * clicks_per_cycle,
            gap_median_ms=statistics.median(gaps),
            gap_p95_ms=percentile(gaps, 0.95),
            gap_p99_ms=percentile(gaps, 0.99),
            gap_max_ms=max(gaps),
        )
    return result


def cycles_for(duration_s, interval_ms):
    """Cycles whose ideal start-to-start span is the requested duration."""
    return max(2, round(duration_s * 1000 / interval_ms) + 1)
