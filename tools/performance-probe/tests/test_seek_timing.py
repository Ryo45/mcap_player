"""Canvas-event timing must not include or race the external polling delay."""
import copy
from pathlib import Path
import sys
import unittest

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from measure_web_recording import seek_observation


class SeekTiming(unittest.TestCase):
    def setUp(self):
        self.issued = {'token': 3, 'time': 100.}
        self.state = {'cursor': .7, 'now': 180.,
                      'focus_presentation': {'seekToken': 3, 'job': 8, 't': 130.}}

    def test_polling_later_does_not_increase_draw_latency(self):
        first = seek_observation(self.state, .7, self.issued)
        late = copy.deepcopy(self.state)
        late['now'] = 500.
        second = seek_observation(late, .7, self.issued)
        self.assertEqual(first['focus_changed_ms'], 30.)
        self.assertEqual(second['focus_changed_ms'], 30.)
        self.assertEqual(second['observation_delay_ms'], 370.)
        self.assertEqual(second['job'], 8)

    def test_old_jobs_pre_dispatch_draws_and_uncommitted_cursors_do_not_complete(self):
        for change in ['missing', 'old_token', 'old_time', 'cursor']:
            state = copy.deepcopy(self.state)
            if change == 'missing':
                state.pop('focus_presentation')
            elif change == 'old_token':
                state['focus_presentation']['seekToken'] = 2
            elif change == 'old_time':
                state['focus_presentation']['t'] = 99.
            else:
                state['cursor'] = .2
            with self.subTest(change=change):
                self.assertIsNone(seek_observation(state, .7, self.issued))


if __name__ == '__main__':
    unittest.main()
