#!/usr/bin/env python3
"""Verify an actual recorded Image with exact Native timestamps and independent source pixels.

The source manifest and PNGs are extracted without modifying CDR or timestamps.
Uses the existing measurement-only Native (layout/control/frame timestamps).
"""
import argparse
import bisect
import json
import os
from pathlib import Path
import re
import signal
import subprocess
import time

from PIL import Image
from check_native_gpu import X11
from compare_native_desktop import capture, digest, wait_for, window_for

ROOT = Path(__file__).resolve().parents[2]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source', type=Path, required=True)
    parser.add_argument('--data', type=Path, required=True)
    parser.add_argument('--binary', type=Path, default=ROOT / 'target/gpu-native/viewer-native-input-rate')
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    args.output = args.output.resolve()
    args.output.mkdir(parents=True, exist_ok=False)
    facts = json.loads(args.source.read_text())
    frames = facts['images']
    assert frames and len({(f['width'], f['height'], f['encoding']) for f in frames}) == 1
    times = [f['time_ns'] for f in frames]
    start = facts['start_ns']
    topic = '/cam/camera/image_raw'
    rect = [320, 76, 1600, 926]
    layout = {'schemaVersion': 1, 'root': {'kind': 'panel', 'id': 'camera-0', 'panelType': 'camera',
              'configVersion': 1, 'config': {'cameraTopic': topic, 'showThumbnails': False,
                                            'showOverlay': False, 'schedulerPriority': True}}}
    (args.output / 'layout.json').write_text(json.dumps(layout))
    (args.output / 'geometry.json').write_text(json.dumps({'rects': {'layout': [320, 44, 1600, 958], 'camera_0': rect}}))
    control = args.output / 'control.txt'
    control.write_text('1 seek 1000000000\n')
    env = os.environ | {'DISPLAY': ':1', 'XAUTHORITY': '/run/user/1000/gdm/Xauthority',
                        'VK_DRIVER_FILES': '/usr/share/vulkan/icd.d/nvidia_icd.json',
                        'RUST_LOG': 'info,wgpu_core::instance=debug,wgpu_hal::vulkan=info',
                        'MCAP_BENCH_LAYOUT': str(args.output / 'layout.json'),
                        'MCAP_BENCH_GEOMETRY': str(args.output / 'geometry.json'),
                        'MCAP_BENCH_CONTROL': str(control)}
    result = {'status': 'running', 'source_sha256': facts['source_sha256'], 'mcap_sha256': digest(args.data),
              'binary_sha256': digest(args.binary), 'harness_sha256': digest(__file__), 'seeks': []}
    # Sample source pixels away from panel borders, at exactly the same normalized locations.
    width, height = frames[0]['width'], frames[0]['height']
    scale = min(rect[2] / width, rect[3] / height)
    picture = [rect[0] + (rect[2] - width * scale) / 2, rect[1] + (rect[3] - height * scale) / 2,
               width * scale, height * scale]
    points = [(round((x + .5) * width / 11), round((y + .5) * height / 8)) for y in range(8) for x in range(11)]
    refs = []
    for i in range(len(frames)):
        with Image.open(args.source.parent / 'images' / f'{i:03d}.png') as image:
            image = image.convert('RGB')
            refs.append([v for point in points for v in image.getpixel(point)])
    process = x11 = None
    logpath = args.output / 'app.log'
    begun = time.monotonic()
    try:
        result['display_before'] = subprocess.check_output(['xset', 'q'], env=env, text=True)
        subprocess.run(['xset', 'dpms', 'force', 'on'], env=env, check=True)
        with logpath.open('w') as log:
            process = subprocess.Popen([str(args.binary.resolve()), '--mcap', str(args.data.resolve()),
                                        '--camera-topic', topic, '--no-platform-geometry'],
                                       cwd=ROOT, env=env, stdout=log, stderr=log, start_new_session=True)
            result['pid'] = process.pid
            window = wait_for(lambda: window_for(process.pid, env), 15, 'Native window')
            x11 = X11(':1')
            x11.x.XMoveResizeWindow(x11.display, window, 40, 60, 1920, 1080)
            x11.x.XFlush(x11.display)
            wait_for(lambda: 'GPU_BENCH_COMMAND id=1\n' in logpath.read_text(), 20, 'source open')
            assert re.search(r'Request adapter result.*NVIDIA.*DiscreteGpu.*Vulkan', logpath.read_text())
            result['selection'] = next(line for line in logpath.read_text().splitlines() if 'GPU_BENCH_SELECTION' in line)
            assert topic in result['selection'] and 'cloud=None' in result['selection']
            request = 1

            def send(command):
                nonlocal request
                request += 1
                tmp = control.with_suffix('.tmp')
                tmp.write_text(f'{request} {command}\n')
                tmp.replace(control)
                wait_for(lambda: f'GPU_BENCH_COMMAND id={request}\n' in logpath.read_text(), 15, command)

            def sample():
                raw, stride = capture(x11, window, [int(picture[0]), int(picture[1]), int(picture[2]), int(picture[3])])
                return [raw[round(y * scale) * stride + round(x * scale) * 4 + k] for x, y in points for k in (2, 1, 0)]

            def current():
                values = re.findall(r'GPU_BENCH_FRAME camera=0 arrival=(\d+) ', logpath.read_text())
                return int(values[-1]) if values else None

            def shot(name):
                raw, stride = capture(x11, window, [0, 0, 1920, 1080])
                Image.frombytes('RGB', (1920, 1080), raw, 'raw', 'BGRX', stride, 1).save(args.output / name)

            # Ordered both directions, including recording end and restart after seek.
            for ns in [1_000_000_000, 7_000_000_000, 4_000_000_000, 9_900_000_000, 1_000_000_000]:
                before = time.monotonic()
                send(f'seek {ns}')
                index = bisect.bisect_right(times, start + ns) - 1
                assert index >= 0
                wait_for(lambda: current() == times[index], 5, 'exact predecessor texture timestamp')
                time.sleep(.15)
                values = sample()
                errors = [sum(abs(a - b) for a, b in zip(values, ref)) / len(ref) for ref in refs]
                assert errors[index] < 12, (index, errors[index])
                assert errors[index] <= min(errors) + .5, (index, errors.index(min(errors)), errors[index], min(errors))
                result['seeks'].append({'offset_ns': ns, 'expected_image': index, 'texture_log_ns': current(),
                                        'source_pixel_mae': errors[index], 'closest_image': errors.index(min(errors)),
                                        'verified_after_ms': (time.monotonic() - before) * 1000,
                                        'note': 'correctness wait includes 150ms settling; not seek latency'})
            shot('seeked.png')
            before_log = len(logpath.read_text())
            send('play')
            time.sleep(5)
            send('pause')
            text = logpath.read_text()[before_log:]
            stamps = [int(v) for v in re.findall(r'GPU_BENCH_FRAME camera=0 arrival=(\d+) ', text)]
            assert len(stamps) >= 40 and all(v in times for v in stamps)
            assert stamps == sorted(set(stamps)), 'playback must be monotonically increasing without duplicates'
            assert 4_500_000_000 < stamps[-1] - stamps[0] < 5_500_000_000
            result['playback'] = {'seconds_requested': 5, 'visible_frames': len(stamps), 'first_log_ns': stamps[0],
                                  'last_log_ns': stamps[-1], 'all_times_are_original': True}
            shot('paused.png')
            paused = current()
            time.sleep(.5)
            assert current() == paused
            assert 'Camera 0 base image upload failed' not in logpath.read_text()
            result['status'] = 'ok'
    except Exception as error:
        result['error'] = repr(error)
        result['status'] = 'failed'
        raise
    finally:
        if process and process.poll() is None:
            try:
                os.killpg(process.pid, signal.SIGTERM)
            except ProcessLookupError:
                pass
            process.wait(timeout=10)
        if x11:
            x11.x.XCloseDisplay(x11.display)
        result['wall_seconds'] = time.monotonic() - begun
        (args.output / 'result.json').write_text(json.dumps(result, indent=2) + '\n')
        print(json.dumps(result, indent=2))


if __name__ == '__main__':
    main()
