#!/usr/bin/env python3
"""Read-only repository storage audit, policy check and optional-artifact preflight.

No command rewrites history, installs Git hooks, uploads objects or pushes refs.
Audit output belongs under target/. Migration is a separate temporary-clone script.
"""
import argparse
import collections
import csv
import hashlib
import json
import os
from pathlib import Path
import re
import subprocess
import sys
from urllib.parse import unquote

ROOT = Path(__file__).resolve().parents[2]
POINTER = b'version https://git-lfs.github.com/spec/v1\n'
TEXT = {'.md', '.py', '.rs', '.js', '.sh', '.toml', '.html', '.css', '.yaml', '.yml'}


def git(repo, *args, data=None):
    return subprocess.check_output(['git', '-C', str(repo), *args], input=data)


def tree(repo, ref='HEAD'):
    result = []
    for item in git(repo, 'ls-tree', '-r', '-l', '-z', ref).split(b'\0'):
        if not item:
            continue
        metadata, path = item.split(b'\t', 1)
        _, kind, oid, size = metadata.split()
        if kind == b'blob':
            result.append({'path': os.fsdecode(path), 'oid': oid.decode(), 'bytes': int(size)})
    return result


def references(repo, files):
    """Conservative textual inventory, not proof about dynamically constructed paths."""
    tracked = {row['path'] for row in files}
    refs = collections.defaultdict(list)
    for row in files:
        path = Path(row['path'])
        if path.suffix not in TEXT:
            continue
        text = git(repo, 'cat-file', 'blob', row['oid']).decode('utf-8', errors='replace')
        if path.suffix == '.md':
            for match in re.finditer(r'(!?)\[[^\]]*\]\(([^)\s]+)(?:\s+[^)]*)?\)', text):
                target = unquote(match[2]).strip('<>').split('#')[0].split('?')[0]
                if not target or '://' in target:
                    continue
                target = os.path.normpath(os.path.join(path.parent, target))
                if target in tracked:
                    refs[target].append({'source': str(path), 'kind': 'image' if match[1] else 'link'})
        for match in re.finditer(r'docs/(?:performance|verification)/[^\s\"\'`<>)}\],;]+', text):
            if match[0] in tracked:
                refs[match[0]].append({'source': str(path), 'kind': 'literal'})
    return dict(refs)


def candidate_reason(path, size, refs, content):
    if not path.startswith('docs/performance/'):
        return None
    if any(r['kind'] == 'image' or (r['kind'] == 'literal' and not r['source'].endswith('.md')) for r in refs):
        return None
    suffix = Path(path).suffix
    if suffix == '.png':
        return 'non-embedded repeated screenshot'
    if suffix == '.gz' and size >= 65536:
        return 'large compressed raw capture'
    if suffix == '.jsonl':
        return 'raw per-run records'
    if suffix == '.log' and size >= 65536:
        return 'large raw log'
    if suffix == '.json' and size >= 131072 and not any(name in Path(path).name for name in ['summary', 'manifest', 'environment', 'references']):
        try:
            value = json.loads(content)
        except (ValueError, UnicodeError):
            return None
        if isinstance(value, dict) and any(isinstance(value.get(key), list) and len(value[key]) >= 16 for key in ['samples', 'heartbeat', 'paint_events', 'paints', 'fingerprints']):
            return 'large raw sampled trace'
    return None


def audit(repo, output):
    output.mkdir(parents=True, exist_ok=True)
    files = tree(repo)
    refs = references(repo, files)
    candidates = []
    for row in files:
        if not row['path'].startswith('docs/performance/'):
            continue
        data = git(repo, 'cat-file', 'blob', row['oid'])
        reason = candidate_reason(row['path'], row['bytes'], refs.get(row['path'], []), data)
        if reason:
            candidates.append(row | {'sha256': hashlib.sha256(data).hexdigest(), 'reason': reason})
    lines = git(repo, 'rev-list', '--objects', '--all').decode().splitlines()
    names = {line.split(' ', 1)[0]: line.partition(' ')[2] for line in lines}
    batch = git(repo, 'cat-file', '--batch-check=%(objectname) %(objecttype) %(objectsize)', data=('\n'.join(names) + '\n').encode())
    blobs = [{'oid': oid, 'path': names[oid], 'bytes': int(size)} for oid, kind, size in (line.split() for line in batch.decode().splitlines()) if kind == 'blob']
    groups = {}
    for prefix in ['docs/performance/', 'docs/verification/', 'tests/fixtures/', 'docs/images/']:
        selected = [r for r in files if r['path'].startswith(prefix)]
        extensions = collections.Counter()
        for row in selected:
            extensions[Path(row['path']).suffix] += row['bytes']
        groups[prefix] = {'files': len(selected), 'bytes': sum(r['bytes'] for r in selected), 'extension_bytes': dict(extensions)}
    summary = {
        'head': git(repo, 'rev-parse', 'HEAD').decode().strip(),
        'refs': git(repo, 'show-ref').decode().splitlines(),
        'count_objects': git(repo, 'count-objects', '-vH').decode(),
        'head_files': len(files), 'head_bytes': sum(r['bytes'] for r in files),
        'history_blob_count': len(blobs), 'history_blob_bytes': sum(r['bytes'] for r in blobs),
        'groups': groups, 'candidate_files': len(candidates),
        'candidate_path_bytes': sum(r['bytes'] for r in candidates),
        'candidate_unique_payload_bytes': sum({r['sha256']: r['bytes'] for r in candidates}.values()),
        'reference_scope': 'Current HEAD Markdown links/images and literal paths; manually inspect dynamic script paths and historical documents.',
    }
    for name, value in [('summary', summary), ('head-files', files), ('history-blobs', blobs), ('references', refs)]:
        (output / (name + '.json')).write_text(json.dumps(value, indent=2) + '\n')
    with (output / 'lfs-candidates.tsv').open('w', newline='') as stream:
        writer = csv.DictWriter(stream, fieldnames=['path', 'oid', 'bytes', 'sha256', 'reason'], delimiter='\t', lineterminator='\n')
        writer.writeheader()
        writer.writerows(candidates)
    print(json.dumps(summary, indent=2))


def is_pointer(path):
    with path.open('rb') as stream:
        return stream.read(len(POINTER)) == POINTER


def require(repo, requested):
    tracked = git(repo, 'ls-files', '-z').decode().split('\0')
    selected = set()
    for item in requested:
        relative = os.path.normpath(item).rstrip('/')
        if relative.startswith('../') or Path(relative).is_absolute():
            raise ValueError('Use repository-relative paths')
        matches = [p for p in tracked if p and (p == relative or p.startswith(relative + '/'))]
        if not matches:
            raise ValueError(f'No tracked artifact matches {item}')
        selected.update(matches)
    missing = [p for p in sorted(selected) if not (repo / p).is_file() or is_pointer(repo / p)]
    if missing:
        patterns=[item.rstrip('/')+'/**' if (repo/item).is_dir() else item for item in requested]
        raise ValueError('Optional artifacts are absent/unhydrated:\n' + '\n'.join(missing) + '\nFetch explicitly: git lfs pull --include="' + ','.join(patterns) + '" --exclude=""')
    print(f'{len(selected)} requested files are available')


def check(repo):
    paths = [os.fsdecode(p) for p in git(repo, 'ls-files', '-z').split(b'\0') if p]
    output = git(repo, 'check-attr', '--cached', '-z', '--stdin', 'filter', data=b'\0'.join(os.fsencode(p) for p in paths) + b'\0').split(b'\0')
    attributes = {os.fsdecode(output[i]): output[i + 2].decode() for i in range(0, len(output) - 1, 3)}
    issues = []
    for path in paths:
        protected = path.startswith(('apps/', 'config/', 'tests/fixtures/', 'docs/images/')) or Path(path).suffix in TEXT | {'.patch', '.csv', '.lock'}
        if protected and (attributes[path] == 'lfs' or ((repo / path).is_file() and is_pointer(repo / path))):
            issues.append(f'Required source/site/fixture/document must stay ordinary Git: {path}')
        if attributes[path] == 'lfs':
            data = git(repo, 'show', ':' + path)
            if not data.startswith(POINTER):
                issues.append(f'LFS path has a non-pointer index blob (install LFS and re-add explicitly): {path}')
    if issues:
        raise ValueError('\n'.join(issues))
    print(f'Storage policy OK: {len(paths)} index paths; optional LFS payloads are not required')


def resolve(repo, mapping, commit):
    if not re.fullmatch(r'[0-9a-fA-F]{7,40}', commit):
        raise ValueError('Supply a 7–40 digit original commit hash')
    with mapping.open() as stream:
        rows=list(csv.reader(stream))
    if any(len(row)!=2 or any(not re.fullmatch(r'[0-9a-f]{40}', part) for part in row) for row in rows):
        raise ValueError('Expected a complete git lfs migrate CSV commit map')
    matches=[new for old,new in rows if old.startswith(commit.lower())]
    if len(set(matches)) == 1:
        print(matches[0])
    elif matches:
        raise ValueError('Ambiguous original hash; use the full SHA')
    else:
        print(git(repo, 'rev-parse', '--verify', commit + '^{commit}').decode().strip())


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--repo', type=Path, default=ROOT)
    commands = parser.add_subparsers(dest='command', required=True)
    scan = commands.add_parser('audit')
    scan.add_argument('--output', type=Path, default=ROOT / 'target/repository-storage/audit')
    commands.add_parser('check')
    need = commands.add_parser('require')
    need.add_argument('paths', nargs='+')
    lookup = commands.add_parser('resolve')
    lookup.add_argument('commit')
    lookup.add_argument('--map', type=Path, required=True)
    args = parser.parse_args()
    try:
        if args.command == 'audit':
            audit(args.repo, args.output)
        elif args.command == 'check':
            check(args.repo)
        elif args.command == 'require':
            require(args.repo, args.paths)
        else:
            resolve(args.repo, args.map, args.commit)
    except (ValueError, OSError, subprocess.CalledProcessError) as error:
        print(error, file=sys.stderr)
        return 1
    return 0


if __name__ == '__main__':
    raise SystemExit(main())
