#!/usr/bin/env python3
"""Paired Web comparison of two already-built products using one timing observer."""
import argparse
import copy
import json
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[3]
TOOLS = ROOT / 'tools/performance-probe'
sys.path.insert(0, str(TOOLS))
from regression import CASES, LIMITS, compare, machine, write
from measure_web_recording import digest

parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--baseline-dist', type=Path, required=True)
parser.add_argument('--candidate-dist', type=Path, required=True)
parser.add_argument('--data', type=Path, required=True)
parser.add_argument('--output', type=Path, required=True)
args = parser.parse_args()
args.output.mkdir(parents=True, exist_ok=False)
spec = json.loads(CASES.read_text())
fixtures = json.loads((args.data / 'fixtures.json').read_text())
if not fixtures.get('complete') or fixtures['cases_sha256'] != digest(CASES):
    raise SystemExit('Fixture preparation/recipe mismatch')
base = {'version': 1, 'engine': 'web', 'machine': machine(), 'cases_sha256': digest(CASES),
        'fixtures': fixtures, 'limits': LIMITS, 'repeats': 3, 'runs': [],
        'input_cache_policy': 'SHA256 read immediately before each fresh process; cache-allowed',
        'harness': {p.name: digest(p) for p in [Path(__file__), TOOLS/'regression.py',
                    TOOLS/'measure_web_recording.py', TOOLS/'web_recording_probe.js',
                    TOOLS/'benchmark_browser.py', TOOLS/'browser_cdp.py']},
        'product_identity': 'Actual web_assets hashes per run; no inference from harness checkout'}
suites = {label: copy.deepcopy(base) for label in ['baseline', 'candidate']}
for repeat in range(3):
    for case in (spec['web'] if repeat % 2 == 0 else list(reversed(spec['web']))):
        for label in (['baseline', 'candidate'] if repeat % 2 == 0 else ['candidate', 'baseline']):
            data = args.data / (case['fixture'] + '.mcap')
            if digest(data) != fixtures['fixtures'][case['fixture']]['sha256']:
                raise SystemExit('Fixture hash changed: ' + str(data))
            destination = args.output / f'{repeat}-{label}-{case["id"]}'
            dist = args.baseline_dist if label == 'baseline' else args.candidate_dist
            command = [sys.executable, str(TOOLS/'measure_web_recording.py'), '--data', str(data),
                       '--dist', str(dist), '--output', str(destination), '--gpu', 'hardware',
                       '--cameras', str(case['cameras']), '--duration-seconds', str(case['span']),
                       '--seconds', str(case['seconds']), '--wake-display', '--vulkan-icd',
                       '/usr/share/vulkan/icd.d/nvidia_icd.json']
            if case.get('seeks'):
                command.append('--seeks')
            subprocess.run(command, check=True, timeout=240)
            result = json.loads((destination/'result.json').read_text())
            env = result['environment']
            row = {'case': 'web/' + case['id'], 'repeat': repeat, 'status': result['status'],
                   'metrics': result['metrics'], 'web_assets': result['web_assets'],
                   'seek_timing': result['seek_timing'], 'environment': {
                       'browser': env.get('version'), 'gpu_mode': env.get('mode'),
                       'gpu_devices': env.get('gpu', {}).get('devices'),
                       'adapter': result['initial'].get('adapter'), 'viewport': env.get('viewport'),
                       'canvases': result['final'].get('canvases')}}
            suites[label]['runs'].append(row)
            write(args.output/(label+'-suite.json'), suites[label])
problems = compare(suites['baseline'], suites['candidate'])
write(args.output/'comparison.json', {'problems': problems})
print(json.dumps({'problems': problems}), flush=True)
raise SystemExit(bool(problems))
