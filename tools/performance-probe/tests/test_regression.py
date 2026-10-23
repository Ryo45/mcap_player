import copy
import json
import sys
from pathlib import Path
import unittest
import tempfile
from types import SimpleNamespace
from unittest.mock import patch
import subprocess

sys.path.insert(0, str(Path(__file__).resolve().parents[1]))
from regression import CASES, LIMITS, compare, prepare


def suite():
    return {'version': 1, 'engine': 'native', 'machine': {'cpu': 'test'}, 'cases_sha256': 'recipe',
            'harness': {'runner': 'same'}, 'limits': LIMITS,
            'fixtures': {'fixtures': {'input': {'sha256': 'same'}}}, 'repeats': 3,
            'runs': [{'case': 'native/' + case['id'], 'repeat': repeat, 'status': 'ok',
                      'environment': {}, 'metrics': {('seek_revisit_ms' if case['id'].startswith('seek-') else 'workload_ms'): 100, 'peak_rss_mib': 400}}
                     for case in json.loads(CASES.read_text())['native'] for repeat in range(3)]}


class RegressionComparison(unittest.TestCase):
    def test_serialized_baseline_compares_to_in_memory_run(self):
        current = suite()
        baseline = json.loads(json.dumps(current))
        self.assertEqual(compare(baseline, current), [])

    def test_identical_and_subthreshold_noise_pass(self):
        before = suite()
        after = copy.deepcopy(before)
        after['runs'][0]['metrics']['workload_ms'] = 500  # median tolerates one noisy repetition
        self.assertEqual(compare(before, after), [])
        for row in after['runs']:
            if 'workload_ms' in row['metrics']:
                row['metrics']['workload_ms'] = 115
        self.assertEqual(compare(before, after), [])

    def test_slowdown_and_memory_growth_fail(self):
        before = suite()
        after = copy.deepcopy(before)
        for row in after['runs']:
            row['metrics']['peak_rss_mib'] = 500
            for key in ['workload_ms', 'seek_revisit_ms']:
                if key in row['metrics']:
                    row['metrics'][key] = 130
        problems = compare(before, after)
        self.assertTrue(any('workload_ms' in problem for problem in problems))
        self.assertTrue(any('peak_rss_mib' in problem for problem in problems))

    def test_failures_missing_cases_and_duplicate_repetitions_do_not_disappear(self):
        before = suite()
        for change in ['failed', 'missing', 'duplicate', 'empty']:
            with self.subTest(change=change):
                after = copy.deepcopy(before)
                if change == 'failed':
                    after['runs'][0]['status'] = 'external-guard'
                elif change == 'missing':
                    after['runs'].pop()
                elif change == 'duplicate':
                    after['runs'][0]['repeat'] = 1
                else:
                    after['runs'] = []
                self.assertTrue(compare(before, after))

    def test_hardware_input_and_observer_changes_refuse_comparison(self):
        before = suite()
        for key in ['machine', 'harness', 'cases_sha256', 'fixtures']:
            after = copy.deepcopy(before)
            if key == 'fixtures':
                after[key]['fixtures']['input']['sha256'] = 'changed'
            else:
                after[key] = 'changed'
            self.assertTrue(compare(before, after))

    def test_nonfinite_missing_or_negative_measurements_fail(self):
        before = suite()
        for value in [None, float('nan'), float('inf'), -1]:
            after = copy.deepcopy(before)
            after['runs'][0]['metrics']['workload_ms'] = value
            self.assertTrue(compare(before, after))

    def test_empty_baselines_and_short_baselines_are_invalid(self):
        empty = suite()
        empty['runs'] = []
        self.assertTrue(compare(empty, empty))
        short = suite()
        short['repeats'] = 1
        short['runs'] = [row for row in short['runs'] if row['repeat'] == 0]
        self.assertTrue(compare(short, short))


class FixturePreparation(unittest.TestCase):
    def test_failed_generation_resumes_verified_inputs_and_accepts_empty_stdout(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            probe = root / 'generator'
            probe.write_bytes(b'generator identity')
            args = SimpleNamespace(data=root / 'data', probe=probe)
            calls = {'one': 0, 'two': 0}

            def generate(command, **_kwargs):
                path = Path(command[2])
                name = path.stem
                calls[name] += 1
                path.write_bytes(name.encode())
                if name == 'two' and calls[name] == 1:
                    raise subprocess.CalledProcessError(1, command)
                return SimpleNamespace(stdout='')

            spec = {'fixtures': {'one': ['generate'], 'two': ['generate']}}
            with patch('regression.subprocess.run', side_effect=generate):
                with self.assertRaises(subprocess.CalledProcessError):
                    prepare(args, spec)
                self.assertFalse((args.data / 'two.mcap').exists())
                prepare(args, spec)
                prepare(args, spec)
            self.assertEqual(calls, {'one': 1, 'two': 2})
            self.assertTrue(json.loads((args.data / 'fixtures.json').read_text())['complete'])


if __name__ == '__main__':
    unittest.main()
