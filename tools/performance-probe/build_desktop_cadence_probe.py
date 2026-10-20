#!/usr/bin/env python3
"""Apply the archived layout patch, add cadence experiments, build, restore sources."""
import difflib
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]
REPORT = ROOT / 'docs/performance/2026-10-20-desktop-workloads'
BASE_PATCH = REPORT / 'native-measurement.patch'
paths = [line[6:] for line in BASE_PATCH.read_text().splitlines() if line.startswith('+++ b/')]
paths += ['apps/viewer-native/src/graphics.rs', 'apps/viewer-native/src/graphics/surface.rs',
          'crates/viewer-core/src/feature/controllers/camera.rs']
original = {p: (ROOT / p).read_text() for p in paths}


def replace(path, old, new):
    target = ROOT / path
    text = target.read_text()
    if text.count(old) != 1:
        raise RuntimeError((path, old, text.count(old)))
    target.write_text(text.replace(old, new))


try:
    subprocess.run(['git', 'diff', '--exit-code', '--', *paths], cwd=ROOT, check=True)
    subprocess.run(['git', 'apply', str(BASE_PATCH)], cwd=ROOT, check=True)
    camera = paths[-1]
    replace(camera, 'const BACKGROUND_CAMERA_INTERVAL: Duration = Duration::from_millis(200);',
            '''fn background_camera_interval() -> Duration {
    static INTERVAL: std::sync::OnceLock<Duration> = std::sync::OnceLock::new();
    *INTERVAL.get_or_init(|| Duration::from_millis(if std::env::var_os("MCAP_BENCH_ALL_CAMERA_10HZ").is_some() { 100 } else { 200 }))
}''')
    (ROOT / camera).write_text((ROOT / camera).read_text().replace('BACKGROUND_CAMERA_INTERVAL', 'background_camera_interval()'))
    replace('apps/viewer-native/src/app.rs', 'log::info!("GPU_BENCH_COMMAND id={id}");',
            'log::info!("GPU_BENCH_COMMAND id={id} command={command}");')
    replace('apps/viewer-native/src/graphics.rs',
            'if scene_resized || self.scene_renderer.needs_render(frame) {',
            '''if scene_resized || self.scene_renderer.needs_render(frame) || std::env::var_os("MCAP_BENCH_SCENE_EVERY_FRAME").is_some() {
            log::info!("GPU_BENCH_CADENCE scene");''')
    replace('apps/viewer-native/src/graphics/surface.rs', '        frame.present();',
            '        frame.present();\n        log::info!("GPU_BENCH_CADENCE present");')
    patch = ''.join(''.join(difflib.unified_diff(original[p].splitlines(True), (ROOT / p).read_text().splitlines(True),
                                               fromfile='a/' + p, tofile='b/' + p)) for p in paths)
    (REPORT / 'native-cadence.patch').write_text(patch)
    subprocess.run(['docker', 'compose', 'run', '--rm', 'dev', 'bash', '-lc',
                    'cargo build --release -p viewer-native --locked && cp /home/dev/target/release/viewer-native target/gpu-native/viewer-native-cadence'],
                   cwd=ROOT, check=True, timeout=300)
finally:
    for path, text in original.items():
        (ROOT / path).write_text(text)
