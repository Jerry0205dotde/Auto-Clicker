"""Event validation and figures of the native benchmark."""

import unittest

from benchmark import estimated_timeout, options
from benchmark_metrics import analyze, cycles_for, percentile


def click(t_ms, kind="MouseButtonPress", button=1, x=80.0, y=400.0):
    return [
        {
            "t_ns": int(t_ms * 1e6),
            "type": kind,
            "button": button,
            "buttons": button,
            "x": x,
            "y": y,
        },
        {
            "t_ns": int((t_ms + 1) * 1e6),
            "type": "MouseButtonRelease",
            "button": button,
            "buttons": 0,
            "x": x,
            "y": y,
        },
    ]


class AnalyzeTests(unittest.TestCase):
    def test_double_clicks_count_dblclick_as_a_press_and_measure_cycle_starts(self):
        events = []
        for start in (0, 10, 25, 35):  # gaps 10, 15, 10 ms
            events += click(start) + click(start + 3, "MouseButtonDblClick")
        result = analyze(events, clicks_per_cycle=2)
        self.assertEqual(result["cycles"], 4)
        self.assertEqual(result["clicks"], 8)
        self.assertEqual(result["double_click_events"], 4)
        self.assertAlmostEqual(result["cycles_per_s"], 3 / 0.035)
        self.assertAlmostEqual(result["clicks_per_s"], 6 / 0.035)
        self.assertEqual(result["gap_median_ms"], 10)
        self.assertEqual(result["gap_max_ms"], 15)
        self.assertTrue(result["complete_cycles"])

    def test_rejects_wrong_button_position_order_and_missing_release(self):
        cases = [
            click(0) + click(10, button=2),
            click(0) + click(10, x=81.0),
            click(0)[::-1],
            click(0) + click(10)[:1],
        ]
        for events in cases:
            with self.subTest(events=events), self.assertRaises(AssertionError):
                analyze(events, clicks_per_cycle=1)

    def test_partial_double_cycle_is_reported(self):
        events = click(0) + click(3) + click(10)
        result = analyze(events, clicks_per_cycle=2)
        self.assertFalse(result["complete_cycles"])
        self.assertEqual(result["cycles"], 1)

    def test_percentile_uses_nearest_rank(self):
        values = list(range(1, 101))
        self.assertEqual(percentile(values, 0.95), 95)
        self.assertEqual(percentile(values, 0.99), 99)
        self.assertEqual(percentile([7], 0.99), 7)


class OptionTests(unittest.TestCase):
    def test_cycles_span_the_requested_duration(self):
        self.assertEqual(cycles_for(60, 10), 6001)
        self.assertEqual(cycles_for(5, 100), 51)
        self.assertEqual(cycles_for(0.001, 100), 2)

    def test_long_run_replaces_the_matrix(self):
        args = options(["--long"])
        self.assertEqual((args.intervals, args.duration), ([10], 60))
        self.assertEqual(args.modes, ["single", "double"])
        self.assertGreater(estimated_timeout(args), 2 * 60 * 2)

    def test_rejects_unknown_modes(self):
        with self.assertRaises(SystemExit):
            options(["--modes", "triple"])


if __name__ == "__main__":
    unittest.main()
