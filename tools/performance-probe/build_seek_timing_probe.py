#!/usr/bin/env python3
"""Build a Native with opt-in seek timestamps, restoring every product source afterwards.

No seek/decode/render order is changed. MCAP_SEEK_TRACE=1 enables timestamps;
the same binary without that variable is the logging-overhead control.
"""
import argparse
import difflib
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--report', type=Path, default=ROOT / 'docs/performance/2026-10-23-seek-latency/order')
parser.add_argument('--binary-name', default='viewer-native-seek-order')
parser.add_argument('--allow-working-tree', action='store_true')
parser.add_argument('--latency-control', action='store_true', help='Measurement-only env override between FIFO latency 1 and 2')
args = parser.parse_args()
if not args.binary_name.replace('-', '').replace('_', '').isalnum():
    parser.error('binary name must be alphanumeric, dash or underscore')
REPORT = args.report.resolve()
PATCH = ROOT / 'docs/performance/2026-10-20-desktop-workloads/native-measurement.patch'
paths = [line[6:] for line in PATCH.read_text().splitlines() if line.startswith('+++ b/')]
paths += ['apps/viewer-native/src/graphics.rs', 'crates/viewer-core/src/lib.rs', 'crates/viewer-renderer/src/camera_decode.rs',
          'apps/viewer-native/src/graphics/camera.rs', 'apps/viewer-native/src/graphics/surface.rs',
          'apps/viewer-native/src/session.rs', 'crates/viewer-mcap/src/playback.rs']
original = {p: (ROOT / p).read_text() for p in paths}


def replace(path, old, new):
    p = ROOT / path
    text = p.read_text()
    assert text.count(old) == 1, (path, old, text.count(old))
    p.write_text(text.replace(old, new))


def event(stage, arrival='0'):
    return f'viewer_core::__seek_probe("{stage}", {arrival});'


try:
    if not args.allow_working_tree:
        subprocess.run(['git', 'diff', '--exit-code', '--', *paths], cwd=ROOT, check=True)
    subprocess.run(['git', 'apply', '--check', str(PATCH)], cwd=ROOT, check=True)
    subprocess.run(['git', 'apply', str(PATCH)], cwd=ROOT, check=True)
    p = ROOT / 'crates/viewer-core/src/lib.rs'
    with p.open('a') as out:
        out.write('''
// Measurement-only API, absent from product builds.
static SEEK_PROBE_OP: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
pub fn __seek_probe_op(id: u64) {
    SEEK_PROBE_OP.store(id, std::sync::atomic::Ordering::Relaxed);
}
pub fn __seek_probe(stage: &str, arrival: i64) {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    if !*ENABLED.get_or_init(|| std::env::var("MCAP_SEEK_TRACE").as_deref() == Ok("1")) { return; }
    static ORIGIN: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();
    let ns = ORIGIN.get_or_init(std::time::Instant::now).elapsed().as_nanos();
    let op = SEEK_PROBE_OP.load(std::sync::atomic::Ordering::Relaxed);
    eprintln!("SEEK_TRACE stage={stage} ns={ns} op={op} arrival={arrival}");
}
''')
    app = 'apps/viewer-native/src/app.rs'
    replace(app, '            self.apply_frame_actions(actions);',
            '            viewer_core::__seek_probe_op(id);\n' + event('command_accept') + '\n            self.apply_frame_actions(actions);\n' + event('command_done'))
    replace(app, '        self.advance_session(elapsed);', event('frame_begin') + '\n        self.advance_session(elapsed);')
    replace(app, '        self.update_camera_presentation(camera_queue_error);', event('update_begin') + '\n        self.update_camera_presentation(camera_queue_error);\n' + event('update_end'))
    replace(app, '        let output = self.render_frame(window, elapsed, frame, surface_wait);', '        let output = self.render_frame(window, elapsed, frame, surface_wait);\n' + event('render_done'))
    session = 'apps/viewer-native/src/session.rs'
    replace(session, '                PlaybackCommand::Seek(cursor) => {',
            '                PlaybackCommand::Seek(cursor) => {\n' + event('seek_begin', 'cursor.0'))
    replace(session, '                    playback.seek_with(&restore_plan, restore)?;',
            event('plan_ready', 'target.0') + '\n                    playback.seek_with(&restore_plan, restore)?;\n' + event('seek_done', 'target.0'))
    playback = 'crates/viewer-mcap/src/playback.rs'
    anchor = '''        self.source.cancel_prefetch();
        let reader = self.source.restore_reader();'''
    replace(playback, anchor, event('cancel_begin', 'plan.target.0') + '\n        self.source.cancel_prefetch();\n' + event('cancel_done', 'plan.target.0') + '\n        let reader = self.source.restore_reader();')
    replace(playback, '        let after_target = viewer_core::ArrivalTime(plan.target.0.saturating_add(1));',
            event('restore_read_done', 'plan.target.0') + '\n        let after_target = viewer_core::ArrivalTime(plan.target.0.saturating_add(1));')
    replace(playback, '        restore(plan.target, messages).map_err(McapSeekError::Restore)?;',
            event('position_ready', 'plan.target.0') + '\n        restore(plan.target, messages).map_err(McapSeekError::Restore)?;\n' + event('feature_restored', 'plan.target.0'))
    decode = 'crates/viewer-renderer/src/camera_decode.rs'
    replace(decode, '                            let start = Instant::now();',
            event('worker_start', 'request.frame.arrival_time.0') + '\n                            let start = Instant::now();')
    replace(decode, '                            let result = CameraDecodeResult {',
            event('worker_done', 'request.frame.arrival_time.0') + '\n                            let result = CameraDecodeResult {')
    replace(decode, '        self.shared.available.notify_all();\n        Ok(true)',
            event('decode_submitted', 'key.0.0') + '\n        self.shared.available.notify_all();\n        Ok(true)')
    replace(decode, '                return Some(result);', event('result_collected', 'result.frame.arrival_time.0') + '\n                return Some(result);')
    camera = 'apps/viewer-native/src/graphics/camera.rs'
    replace(camera, '            let upload_started = Instant::now();',
            event('upload_begin', 'frame.arrival_time.0') + '\n            let upload_started = Instant::now();')
    replace(camera, '            self.camera_geometry.insert(camera_id, decoded.geometry);',
            event('upload_done', 'frame.arrival_time.0') + '\n            self.camera_geometry.insert(camera_id, decoded.geometry);')
    view = 'apps/viewer-native/src/graphics/views/camera.rs'
    replace(view, '        log::info!("GPU_BENCH_FRAME camera={} arrival={} rect={:?} size={:?}",texture.camera_id.0,texture.arrival_time.0,image_rect,texture.size);',
            event('image_draw', 'texture.arrival_time.0') + '\n        log::info!("GPU_BENCH_FRAME camera={} arrival={} rect={:?} size={:?}",texture.camera_id.0,texture.arrival_time.0,image_rect,texture.size);')
    surface = 'apps/viewer-native/src/graphics/surface.rs'
    if args.latency_control:
        replace(surface, 'desired_maximum_frame_latency: 2,', '''desired_maximum_frame_latency: if std::env::var("MCAP_BENCH_FRAME_LATENCY").as_deref() == Ok("1") { 1 } else { 2 },''')
    replace(surface, '        self.surface.get_current_texture()',
            event('acquire_begin') + '\n        let frame = self.surface.get_current_texture()?;\n' + event('acquire_done') + '\n        Ok(frame)')
    replace(surface, '        self.queue\n            .submit(callbacks.into_iter().chain([encoder.finish()]));\n        frame.present();',
            event('queue_submit_begin') + '\n        self.queue\n            .submit(callbacks.into_iter().chain([encoder.finish()]));\n' + event('queue_submit_done') + '\n' + event('present_begin') + '\n        frame.present();\n' + event('present_done'))
    REPORT.mkdir(parents=True, exist_ok=True)
    product_patch = ''.join(
        ''.join(difflib.unified_diff(
            subprocess.check_output(['git', 'show', 'HEAD:' + p], cwd=ROOT, text=True).splitlines(True),
            original[p].splitlines(True), fromfile='a/' + p, tofile='b/' + p))
        for p in paths)
    (REPORT / 'product.patch').write_text(product_patch)
    (REPORT / 'native-seek-timing.patch').write_text(''.join(''.join(difflib.unified_diff(
        original[p].splitlines(True), (ROOT / p).read_text().splitlines(True), fromfile='a/' + p, tofile='b/' + p)) for p in paths))
    subprocess.run(['docker', 'compose', 'run', '--rm', '-e', 'CARGO_BUILD_JOBS=1', 'dev', 'bash', '-lc',
                    'cargo build --release -p viewer-native --locked && cp /home/dev/target/release/viewer-native target/gpu-native/' + args.binary_name],
                   cwd=ROOT, check=True, timeout=600)
finally:
    for p, text in original.items():
        (ROOT / p).write_text(text)
