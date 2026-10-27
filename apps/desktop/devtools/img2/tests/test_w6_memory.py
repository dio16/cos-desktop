"""Resource arithmetic boundaries, not process-counter-to-allocator guesses."""
from __future__ import annotations

from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from w6_memory_verify import matched_differences, requirements, window_metrics


class MemoryArithmeticTests(unittest.TestCase):
    def test_maximum_is_one_observed_window_not_individual_maxima_sum(self):
        windows = [
            {"complete": True, "totals": {"working_set_bytes": 120, "private_bytes": 110}},
            {"complete": True, "totals": {"working_set_bytes": 150, "private_bytes": 90}},
        ]
        result = window_metrics(windows)
        self.assertEqual(result["max_observed_window_private"], 110)
        self.assertEqual(result["max_observed_window_working_set"], 150)

    def test_incomplete_window_cannot_supply_a_numeric_maximum(self):
        missing = {"complete": False, "totals": None}
        self.assertEqual(window_metrics([missing])["max_observed_window_private"], None)
        result = window_metrics([missing, {"complete": True, "totals": {
            "working_set_bytes": 200, "private_bytes": 100}}])
        self.assertEqual((result["complete_window_count"], result["incomplete_window_count"]), (1, 1))
        self.assertEqual(result["max_observed_window_private"], 100)

    def test_known_requirements_count_conversion_once_not_scratch_twice(self):
        fixture = {"bytes": 54, "decoded_bytes_required": 6,
                   "bgra32_bytes_required": 8, "declared_scratch_bytes": 2}
        self.assertEqual(requirements([fixture, fixture])["sum_of_known_requirements"], 140)

    def test_matched_run_decrease_is_not_clamped_to_allocation_growth(self):
        experiments = []
        for repeat in (1, 2):
            for name, value in (("before", 100), ("small", 80)):
                experiments.append({"id": f"cohort_{name}_{repeat}", "metrics": {
                    "max_observed_window_working_set": value * 2,
                    "max_observed_window_private": value}})
        differences = matched_differences(experiments)
        for row in differences:
            self.assertEqual(row["difference"]["max_observed_window_private"], -20)
            self.assertEqual(row["difference"]["max_observed_window_working_set"], -40)


if __name__ == "__main__":
    unittest.main()
