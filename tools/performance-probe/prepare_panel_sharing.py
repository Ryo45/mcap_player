#!/usr/bin/env python3
"""Prepare isolated Git source copies with identical Web decode/Scene raster sizes.

Only these copies are patched. Source geometry, input messages and scheduling stay unchanged.
Build Native normally; these patches affect only browser display resolution.
"""
import argparse
import difflib
import io
from pathlib import Path
import subprocess
import tarfile

SOURCE_PATHS = ['Cargo.toml', 'Cargo.lock', 'crates', 'apps', 'config',
                'tools/ros-fixture', 'tools/preview-builder', 'tools/performance-probe']


def prepare(revision, destination):
    revision = subprocess.check_output(['git','rev-parse','--verify','--end-of-options',revision+'^{commit}'],text=True).strip()
    destination.mkdir()
    archive = subprocess.check_output(['git', 'archive', revision, *SOURCE_PATHS])
    with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
        tar.extractall(destination, filter='data')
    patches = []
    for relative in ['apps/viewer-web/src/camera_bitmap.rs', 'apps/viewer-web/src/egui_app/gpu.rs']:
        path = destination/relative
        before = path.read_text()
        if relative.endswith('camera_bitmap.rs'):
            needle = 'let pixels = image_size(source, target);'
            if before.count(needle) != 1:
                raise RuntimeError('Camera queue changed; review the measurement patch')
            after = before.replace(needle, 'let _ = target;\n            let pixels = image_size(source, (960, 540));')
        else:
            offset = before.index('{', before.index('    pub fn render_scene')) + 1
            after = before[:offset]+'\n        let _ = size;\n        let size = (960, 540);'+before[offset:]
        path.write_text(after)
        patches.extend(difflib.unified_diff(before.splitlines(True), after.splitlines(True),
                                           fromfile='a/'+relative, tofile='b/'+relative))
    (destination/'measurement.patch').write_text(''.join(patches))
    (destination/'source-revision.txt').write_text(subprocess.check_output(
        ['git', 'rev-parse', revision], text=True))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--before', required=True)
    parser.add_argument('--after', required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output.mkdir(parents=True, exist_ok=False)
    prepare(args.before, args.output/'before')
    prepare(args.after, args.output/'after')


if __name__ == '__main__':
    main()
