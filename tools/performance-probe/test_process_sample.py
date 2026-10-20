"""Linux regression: descendants spawned by a non-main thread must be counted."""
import os
import subprocess
import sys
import threading
import unittest

from measure_viewer_pilot import process_sample


class ProcessSampleTest(unittest.TestCase):
    def test_child_spawned_from_worker_thread(self):
        spawned = threading.Event()
        release = threading.Event()
        children = []

        def worker():
            child = subprocess.Popen([sys.executable, '-c', 'import time; time.sleep(30)'])
            children.append(child)
            try:
                spawned.set()
                release.wait(10)
            finally:
                child.terminate()
                child.wait(timeout=5)

        thread = threading.Thread(target=worker)
        thread.start()
        try:
            self.assertTrue(spawned.wait(5))
            for include_pss in [False, True]:
                sample = process_sample(os.getpid(), include_pss=include_pss)
                self.assertIn(children[0].pid, sample['pids'])
                self.assertIn(children[0].pid, [p['pid'] for p in sample['processes']])
                self.assertGreater(sample['summed_rss_kib'], 0)
                if include_pss:
                    self.assertGreater(sample['pss_kib'], 0)
                else:
                    self.assertIsNone(sample['pss_kib'])
        finally:
            release.set()
            thread.join(timeout=5)
        self.assertFalse(thread.is_alive())


if __name__ == '__main__':
    unittest.main()
