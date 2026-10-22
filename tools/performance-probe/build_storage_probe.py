#!/usr/bin/env python3
"""Build current Native with measurement-only geometry, controls and 10 Hz cameras."""
import argparse
import difflib
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]
REPORT = ROOT / 'docs/performance/2026-10-22-raw-storage'


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--variant', choices=['storage', 'prefetch', 'seek-cache'], default='storage')
    args = parser.parse_args()
    report = ROOT / 'docs/performance/2026-10-22-seek-revisit' if args.variant == 'seek-cache' else REPORT
    patch = ROOT / 'docs/performance/2026-10-20-desktop-workloads/native-measurement.patch'
    clean_paths = [line[6:] for line in patch.read_text().splitlines() if line.startswith('+++ b/')]
    clean_paths += ['crates/viewer-core/src/feature/controllers/camera.rs']
    paths = clean_paths + (['crates/viewer-mcap/src/playback.rs'] if args.variant != 'storage' else [])
    original = {p: (ROOT / p).read_text() for p in paths}
    try:
        subprocess.run(['git', 'diff', '--exit-code', '--', *clean_paths], cwd=ROOT, check=True)
        subprocess.run(['git', 'apply', '--check', str(patch)], cwd=ROOT, check=True)
        subprocess.run(['git', 'apply', str(patch)], cwd=ROOT, check=True)
        camera = ROOT / 'crates/viewer-core/src/feature/controllers/camera.rs'
        text = camera.read_text()
        old = 'const BACKGROUND_CAMERA_INTERVAL: Duration = Duration::from_millis(200);'
        assert text.count(old) == 1
        camera.write_text(text.replace(old, 'const BACKGROUND_CAMERA_INTERVAL: Duration = Duration::from_millis(100);'))
        if args.variant != 'storage':
            playback = ROOT / 'crates/viewer-mcap/src/playback.rs'
            text = playback.read_text()
            anchor = '        self.source_read.record(read_started.elapsed());'
            assert text.count(anchor) == 1
            text = text.replace(anchor, anchor + '''
        let d = self.source.read_diagnostics();
        static LAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(u64::MAX);
        let event = d.chunks_expanded.saturating_add(d.prefetch_ready_hits).saturating_add(d.prefetch_waits);
        if LAST.swap(event, std::sync::atomic::Ordering::Relaxed) != event {
            eprintln!("GPU_BENCH_IO expanded={} ready={} waits={} wait_ns={} active={} prepared_estimate={}", d.chunks_expanded, d.prefetch_ready_hits, d.prefetch_waits, d.prefetch_wait_ns, d.sequential_cache_backing_bytes, d.prefetch_estimated_bytes);
        }''')
            playback.write_text(text)
        report.mkdir(parents=True, exist_ok=True)
        (report / f'native-{args.variant}.patch').write_text(''.join(
            ''.join(difflib.unified_diff(original[p].splitlines(True), (ROOT / p).read_text().splitlines(True), fromfile='a/' + p, tofile='b/' + p))
            for p in paths))
        build = ('cargo build --release -p viewer-native -p performance-probe --locked '
                 f'&& cp /home/dev/target/release/viewer-native target/gpu-native/viewer-native-{args.variant} '
                 f'&& cp /home/dev/target/release/performance-probe target/gpu-native/performance-probe-{args.variant}')
        subprocess.run(['docker', 'compose', 'run', '--rm', '-e', 'CARGO_BUILD_JOBS=1', 'dev', 'bash', '-lc', build], cwd=ROOT, check=True, timeout=600)
    finally:
        for p, text in original.items():
            (ROOT / p).write_text(text)


if __name__ == '__main__':
    main()
