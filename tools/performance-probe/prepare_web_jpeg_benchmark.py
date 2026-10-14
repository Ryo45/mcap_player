#!/usr/bin/env python3
"""Prepare the temporary Trunk feature entry, or copy extracted JPEG samples after build."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil

root = Path(__file__).resolve().parents[2]
web = root / 'apps/viewer-web'
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--samples', type=Path)
args = parser.parse_args()
if args.samples is None:
    entry = web / '.jpeg-benchmark.html'
    entry.write_text((web/'index.html').read_text().replace(
        'data-cargo-no-default-features',
        'data-cargo-no-default-features data-cargo-features="jpeg-benchmark"'))
    print('docker compose run --rm dev bash -lc "cd apps/viewer-web && trunk build .jpeg-benchmark.html --release"')
else:
    files = sorted(args.samples.glob('*.jpg'))
    if len(files) != 32:
        parser.error('expected exactly 32 extracted JPEG files')
    destination = web/'dist/samples'
    destination.mkdir(exist_ok=True)
    manifest = []
    for file in files:
        shutil.copyfile(file, destination/file.name)
        manifest.append(dict(file=file.name, bytes=file.stat().st_size,
            sha256=hashlib.sha256(file.read_bytes()).hexdigest()))
    (destination/'manifest.json').write_text(json.dumps(manifest, indent=2)+'\n')
