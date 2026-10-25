import sys
from pathlib import Path
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from compare_seek_candidate import compare_pair


class SeekCandidateGateTest(unittest.TestCase):
    def setUp(self):
        self.baseline = dict(ns=1_000_000, p95_ns=1_100_000,
                             allocated_bytes=12_000_000, allocation_calls=2,
                             peak_extra_live_bytes=12_000_000)

    def test_equal_and_improved_pass(self):
        self.assertEqual(compare_pair(self.baseline, self.baseline), [])
        better = self.baseline | dict(ns=800_000, allocated_bytes=0, allocation_calls=0)
        self.assertEqual(compare_pair(self.baseline, better), [])

    def test_time_and_allocation_regressions_fail(self):
        for field, value in [('ns', 1_200_000), ('p95_ns', 1_500_000),
                             ('allocated_bytes', 12_000_001), ('allocation_calls', 3),
                             ('peak_extra_live_bytes', 12_000_001)]:
            with self.subTest(field=field):
                self.assertTrue(compare_pair(self.baseline, self.baseline | {field: value}))

    def test_missing_and_invalid_metrics_fail_closed(self):
        self.assertTrue(compare_pair(self.baseline, {}))
        for value in [float('nan'), float('inf'), -1, True]:
            self.assertTrue(compare_pair(self.baseline, self.baseline | dict(ns=value)))


if __name__ == '__main__':
    unittest.main()
