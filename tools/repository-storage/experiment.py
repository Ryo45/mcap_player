#!/usr/bin/env python3
"""Rehearse a reviewed LFS migration ONLY in a new /tmp mirror, with no remote.

The source must be a complete local bare mirror. All commands that can rewrite refs
run in the newly created temporary directory. There is deliberately no push/upload.
"""
import argparse
import csv
import hashlib
import json
import os
from pathlib import Path
import subprocess
import tempfile

from storage import POINTER, git, tree


def measure(repo):
    return {'count_objects': git(repo, 'count-objects', '-vH').decode(),
            'pack_bytes': sum(p.stat().st_size for p in (repo / 'objects/pack').glob('*.pack'))}


def verify(source, migrated, mapping, selected):
    # The exact full-history map also lets reports resolve original source SHAs.
    with mapping.open() as stream:
        pairs = list(csv.reader(stream))
    checked = set()
    for old, new in pairs:
        before = {r['path']: r for r in tree(source, old)}
        after = {r['path']: r for r in tree(migrated, new)}
        if set(before) - {'.gitattributes'} != set(after) - {'.gitattributes'}:
            raise ValueError(f'Paths changed beyond attributes: {old}')
        if '.gitattributes' in before:
            original = git(source, 'cat-file', 'blob', before['.gitattributes']['oid'])
            converted = git(migrated, 'cat-file', 'blob', after['.gitattributes']['oid'])
            if any(line and line not in converted.splitlines() for line in original.splitlines()):
                raise ValueError('Existing attributes were lost')
        for path, row in before.items():
            if path == '.gitattributes':
                continue
            converted = after[path]
            if path not in selected:
                if converted['oid'] != row['oid']:
                    raise ValueError(f'Unselected content changed: {old}:{path}')
                continue
            identity = row['oid'], converted['oid']
            if identity in checked:
                continue
            data = git(source, 'cat-file', 'blob', row['oid'])
            if data.startswith(POINTER):
                raise ValueError('This first-migration rehearsal does not hydrate existing source LFS objects')
            sha = hashlib.sha256(data).hexdigest()
            pointer = git(migrated, 'cat-file', 'blob', converted['oid'])
            expected = POINTER + f'oid sha256:{sha}\nsize {len(data)}\n'.encode()
            if pointer != expected:
                raise ValueError(f'Wrong LFS pointer: {path}')
            payload = migrated / 'lfs/objects' / sha[:2] / sha[2:4] / sha
            with payload.open('rb') as stream:
                digest = hashlib.file_digest(stream, 'sha256').hexdigest()
            if digest != sha or payload.stat().st_size != len(data):
                raise ValueError(f'LFS payload mismatch: {path}')
            checked.add(identity)
    return {'mapped_commits_checked': len(pairs), 'distinct_payload_versions_checked': len(checked)}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--manifest', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    source = args.source.resolve(strict=True)
    if git(source, 'rev-parse', '--is-bare-repository').strip() != b'true':
        parser.error('--source must be a complete local bare mirror, never a working repository')
    if git(source, 'rev-parse', '--is-shallow-repository').strip() == b'true':
        parser.error('Shallow source is not a history backup')
    original_refs = git(source, 'show-ref')
    refs = git(source, 'for-each-ref', '--format=%(refname)').decode().splitlines()
    if any(not ref.startswith(('refs/heads/', 'refs/tags/')) for ref in refs):
        parser.error('Review non-head/tag refs separately; do not silently retain old remote/PR refs')
    with args.manifest.open() as stream:
        rows = list(csv.DictReader(stream, delimiter='\t'))
    if not rows:
        parser.error('Empty migration manifest')
    for row in rows:
        path = row['path']
        if not path.startswith('docs/performance/') or any(char in path for char in '*?[],:\n\r') or '..' in Path(path).parts:
            parser.error(f'Expected an exact reviewed performance-artifact path: {path}')
        data = git(source, 'show', 'refs/heads/main:' + path)
        if hashlib.sha256(data).hexdigest() != row['sha256']:
            parser.error(f'Stale manifest: {path}; audit/review again')
    temporary = Path(tempfile.mkdtemp(prefix='mcap-storage-', dir='/tmp'))
    migrated = temporary / 'migrated.git'
    fresh = temporary / 'fresh.git'
    mapping = temporary / 'commit-map.csv'
    env = os.environ | {'GIT_LFS_SKIP_SMUDGE': '1'}
    with (temporary / 'commands.log').open('w') as log:
        def run(command):
            subprocess.run(command, env=env, check=True, stdout=log, stderr=log)
        run(['git', 'clone', '--mirror', '--no-local', str(source), str(migrated)])
        for remote in git(migrated, 'remote').decode().splitlines():
            run(['git', '-C', str(migrated), 'remote', 'remove', remote])
        run(['git', '-C', str(migrated), 'lfs', 'install', '--local', '--skip-smudge'])
        run(['git', '-C', str(migrated), 'lfs', 'migrate', 'import', '--everything', '--skip-fetch',
             '--include=' + ','.join(row['path'] for row in rows), '--object-map=' + str(mapping)])
        verification = verify(source, migrated, mapping, {row['path'] for row in rows})
        run(['git', 'clone', '--mirror', '--no-local', str(migrated), str(fresh)])
        run(['git', '-C', str(fresh), 'fsck', '--full', '--no-reflogs'])
    if git(source, 'show-ref') != original_refs:
        raise ValueError('Source mirror refs changed unexpectedly')
    lfs = [p for p in (migrated / 'lfs/objects').rglob('*') if p.is_file()]
    report = {'temporary_directory': str(temporary), 'baseline': measure(source), 'after': measure(fresh),
              'lfs_unique_objects': len(lfs), 'lfs_all_history_bytes': sum(p.stat().st_size for p in lfs),
              'before_refs': original_refs.decode().splitlines(), 'after_refs': git(migrated, 'show-ref').decode().splitlines(),
              'verification': verification, 'remote_uploads': False, 'source_refs_unchanged': True}
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(json.dumps(report, indent=2) + '\n')
    args.output.with_suffix('.commit-map.csv').write_bytes(mapping.read_bytes())
    print(json.dumps(report, indent=2))


if __name__ == '__main__':
    main()
