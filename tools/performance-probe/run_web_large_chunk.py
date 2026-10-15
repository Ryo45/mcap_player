#!/usr/bin/env python3
"""Watch Chrome independently of CDP while a large local Chunk can block its main thread."""
import argparse
import json
import os
from pathlib import Path
import subprocess
import sys
import time

from measure_viewer_pilot import ROOT, process_sample


def run(args):
    output = args.output.resolve()
    output.mkdir(parents=True)
    child_output = output / 'browser.json'
    group = f'mcap-web-large-chunk-{os.getpid()}.scope'
    command = ['systemd-run', '--user', '--scope', '--unit=' + group.removesuffix('.scope'),
               '-p', 'MemoryMax=3G', '-p', 'MemorySwapMax=0', sys.executable,
               str(ROOT / 'tools/performance-probe/diagnose_web_camera.py'),
               '--data', str(args.data.resolve()), '--mode', 'light',
               '--play-timeout', '180', '--output', str(child_output)]
    result = {'command': command, 'chrome_pss_limit_kib': 2 * 1024 * 1024,
              'cgroup_memory_max_bytes': 3 * 2**30, 'samples': [], 'status': 'running'}
    started = time.monotonic()
    chrome_pid = None
    cgroup = None
    with (output / 'process.log').open('w') as log:
        process = subprocess.Popen(command, cwd=ROOT, stdout=log, stderr=log)
        try:
            while process.poll() is None:
                if cgroup is None:
                    path = subprocess.run(
                        ['systemctl', '--user', 'show', group, '-p', 'ControlGroup', '--value'],
                        text=True, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL,
                        check=False).stdout.strip()
                    if path:
                        cgroup = Path('/sys/fs/cgroup' + path)
                sample = {'wall_seconds': time.monotonic() - started}
                if cgroup is not None:
                    try:
                        sample['cgroup_memory_current'] = int((cgroup / 'memory.current').read_text())
                        sample['cgroup_memory_events'] = dict(
                            (key, int(value)) for key, value in
                            (line.split() for line in (cgroup / 'memory.events').read_text().splitlines()))
                        if chrome_pid is None:
                            for pid in (cgroup / 'cgroup.procs').read_text().split():
                                try:
                                    cmd = Path(f'/proc/{pid}/cmdline').read_bytes().replace(b'\0', b' ')
                                except (FileNotFoundError, ProcessLookupError):
                                    continue
                                if b'--remote-debugging-port=9241' in cmd and b'--type=' not in cmd:
                                    chrome_pid = int(pid)
                                    break
                    except FileNotFoundError:
                        pass
                if chrome_pid is not None:
                    sample['chrome'] = process_sample(chrome_pid)
                result['samples'].append(sample)
                over_budget = sample.get('chrome', {}).get('pss_kib', 0) > result['chrome_pss_limit_kib']
                if over_budget or sample['wall_seconds'] > 240:
                    result['status'] = 'external-pss-limit' if over_budget else 'external-timeout'
                    subprocess.run(['systemctl', '--user', 'kill', group], check=False,
                                   stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                    break
                time.sleep(.1)
            result['returncode'] = process.wait(timeout=30)
        finally:
            subprocess.run(['systemctl', '--user', 'kill', group], check=False,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=10)
            if result['status'] == 'running':
                result['status'] = 'finished' if result.get('returncode') == 0 else 'process-failed'
            if child_output.exists():
                result['browser_status'] = json.loads(child_output.read_text())['status']
            result['peak_sampled_chrome_pss_kib'] = max(
                (s.get('chrome', {}).get('pss_kib', 0) for s in result['samples']), default=0)
            (output / 'external.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({key: value for key, value in result.items() if key not in ['command', 'samples']}))


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--data', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    if args.output.exists():
        parser.error('use a new output directory')
    run(args)
