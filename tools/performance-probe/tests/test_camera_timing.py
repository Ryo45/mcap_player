import sys
from pathlib import Path
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from measure_desktop_workload import frame_at_or_before, frame_offset_ns, identity_probes, identify_camera, valid_camera_match


class CameraFixtureTest(unittest.TestCase):
    def test_thirty_hz_predecessors_at_integer_nanosecond_boundaries(self):
        for index in range(150):
            stamp = frame_offset_ns(index, 30)
            self.assertEqual(frame_at_or_before(stamp, 30), index)
            if index:
                self.assertEqual(frame_at_or_before(stamp - 1, 30), index - 1)
        self.assertEqual(frame_offset_ns(30, 30), 1_000_000_000)

    def test_visual_id_rejects_ambiguous_marker_values(self):
        bank = [[224.0 if value & (1 << bit) else 32.0 for bit in range(7)] for value in range(128)]
        probes = identity_probes(bank, range(7), 7)
        for index, vector in enumerate(bank):
            match = identify_camera(vector, bank, probes)
            self.assertEqual(match['variant'], index)
            self.assertTrue(valid_camera_match(match))
        uncertain = bank[5].copy()
        uncertain[0] = 128.0
        self.assertFalse(valid_camera_match(identify_camera(uncertain, bank, probes)))


if __name__ == '__main__':
    unittest.main()
