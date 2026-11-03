#!/usr/bin/env python3
"""Forensic reconstruction of draw times from the preserved v1 result archive.

No measurements are overwritten. This applies only to v1's four distinct targets:
its final state sample for a target is the state returned by wait().
"""
import argparse
import hashlib
import json
from pathlib import Path
import statistics
import tarfile

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--archive', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
if args.output.exists():
    parser.error('Use a new output file')
rows = []
with tarfile.open(args.archive, 'r:gz') as archive:
    for member in archive:
        if not member.isfile() or not member.name.endswith('/result.json'):
            continue
        if member.size > 64 * 1024 * 1024:
            raise ValueError('Oversized measurement')
        with archive.extractfile(member) as stream:
            result = json.load(stream)
        if not result.get('seeks'):
            continue
        if 'seek_timing' in result:
            raise ValueError('Reconstruction supports only the archived v1 observer')
        if [s['target'] for s in result['seeks']] != [.7, .2, .6, .3]:
            raise ValueError('Unexpected target sequence')
        seeks = []
        for token, seek in enumerate(result['seeks'], 1):
            samples = [s for s in result['samples'] if s['phase'] == 'seek'
                       and abs(s['cursor'] - seek['target']) < .0002]
            observed = samples[-1]['now']
            issued = observed - seek['focus_changed_ms']
            event = next(e for e in result['presentations'] if e['id'] == 'camera'
                         and e['seekToken'] == token and e['t'] >= issued)
            seeks.append({'target': seek['target'], 'token': token, 'issued_time_ms': issued,
                          'presented_time_ms': event['t'], 'state_sample_time_ms': observed,
                          'legacy_ms': seek['focus_changed_ms'], 'draw_ms': event['t'] - issued,
                          'sample_minus_draw_ms': observed - event['t']})
        rows.append({'source': member.name, 'legacy_median_ms': result['metrics']['seek_focus_ms'],
                     'draw_median_ms': statistics.median(s['draw_ms'] for s in seeks), 'seeks': seeks})
with args.archive.open('rb') as stream:
    sha = hashlib.file_digest(stream, 'sha256').hexdigest()
args.output.parent.mkdir(parents=True, exist_ok=True)
args.output.write_text(json.dumps({'archive_sha256': sha, 'runs': rows}, indent=2) + '\n')
print(f'Reconstructed {len(rows)} old processes without changing their original records')
