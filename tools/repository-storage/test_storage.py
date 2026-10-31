"""Deterministic policy/preflight regressions; no network or changes to the real repository."""
import contextlib
import io
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

import storage


class StorageTests(unittest.TestCase):
    def test_candidate_selection_respects_usage_not_only_extension(self):
        prefix = 'docs/performance/day/'
        self.assertIsNotNone(storage.candidate_reason(prefix + 'repeat.png', 10, [], b''))
        self.assertIsNone(storage.candidate_reason(prefix + 'figure.png', 10, [{'kind': 'image', 'source': 'report.md'}], b''))
        self.assertIsNone(storage.candidate_reason(prefix + 'baseline.json', 200000, [{'kind': 'literal', 'source': 'tools/probe.py'}], b'{}'))
        self.assertIsNone(storage.candidate_reason(prefix + 'summary.json', 200000, [], b'{"samples":[]}'))
        self.assertIsNone(storage.candidate_reason(prefix + 'run.log.gz', 1024, [], b''))
        self.assertIsNotNone(storage.candidate_reason(prefix + 'run.log.gz', 70000, [], b''))
        self.assertIsNone(storage.candidate_reason('tests/fixtures/test.mcap', 100000, [], b''))
        self.assertIsNone(storage.candidate_reason('docs/verification/report.png', 100000, [], b''))

    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory(prefix='mcap-storage-test-', dir='/tmp')
        self.repo = Path(self.temporary.name)
        self.env = os.environ | {'GIT_CONFIG_GLOBAL': '/dev/null', 'GIT_CONFIG_NOSYSTEM': '1'}
        self.run_git('init', '-q', '-b', 'main')
        self.write('README.md', 'source docs\n')
        self.run_git('add', '.')
        self.run_git('-c', 'user.name=Test', '-c', 'user.email=test@example.invalid', 'commit', '-qm', 'baseline')

    def tearDown(self):
        self.temporary.cleanup()

    def run_git(self, *args):
        return subprocess.check_output(['git', '-C', str(self.repo), *args], env=self.env)

    def write(self, path, text):
        target = self.repo / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text)

    def test_unhydrated_optional_artifact_has_an_explicit_fetch_instruction(self):
        path = 'docs/performance/day/artifacts/raw.json'
        self.write(path, storage.POINTER.decode() + 'oid sha256:' + 'a' * 64 + '\nsize 123\n')
        self.run_git('add', '.')
        with self.assertRaisesRegex(ValueError, 'raw.json'):
            storage.require(self.repo, ['docs/performance/day'])
        with self.assertRaises(ValueError) as error:
            storage.require(self.repo, ['docs/performance/day'])
        self.assertIn('docs/performance/day/**', str(error.exception))
        self.assertIn('--exclude=""', str(error.exception))
        self.write(path, '{"actual":"evidence"}')
        with contextlib.redirect_stdout(io.StringIO()):
            storage.require(self.repo, [path])
        with self.assertRaises(ValueError):
            storage.require(self.repo, ['../outside'])

    def test_policy_rejects_raw_index_data_under_lfs_and_required_pointers(self):
        path = 'docs/performance/day/artifacts/raw.json'
        self.write('.gitattributes', f'{path} filter=lfs -text\n')
        self.write(path, '{"raw":true}')
        self.run_git('add', '.')
        with self.assertRaisesRegex(ValueError, 'non-pointer index blob'):
            storage.check(self.repo)
        self.write(path, storage.POINTER.decode() + 'oid sha256:' + 'a' * 64 + '\nsize 123\n')
        self.run_git('add', '.')
        with contextlib.redirect_stdout(io.StringIO()):
            storage.check(self.repo)
        self.write('tests/fixtures/required.mcap', storage.POINTER.decode())
        self.run_git('add', '.')
        with self.assertRaisesRegex(ValueError, 'must stay ordinary Git'):
            storage.check(self.repo)

    def test_original_commit_lookup_accepts_only_unambiguous_maps(self):
        mapping = self.repo / 'map.csv'
        mapping.write_text('a' * 40 + ',' + 'b' * 40 + '\n')
        capture = io.StringIO()
        with contextlib.redirect_stdout(capture):
            storage.resolve(self.repo, mapping, 'a' * 10)
        self.assertEqual(capture.getvalue().strip(), 'b' * 40)
        mapping.write_text('a' * 40 + ',' + 'b' * 40 + '\n' + 'a' * 10 + 'c' * 30 + ',' + 'd' * 40 + '\n')
        with self.assertRaises(ValueError):
            storage.resolve(self.repo, mapping, 'a' * 10)

    def test_audit_is_read_only(self):
        before = self.run_git('show-ref')
        with contextlib.redirect_stdout(io.StringIO()):
            storage.audit(self.repo, self.repo / 'target/audit')
        self.assertEqual(self.run_git('show-ref'), before)
        self.assertTrue((self.repo / 'target/audit/summary.json').is_file())


if __name__ == '__main__':
    unittest.main()
