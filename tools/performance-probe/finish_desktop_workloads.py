#!/usr/bin/env python3
"""Audit owned benchmark PIDs and conditionally restore the initially blank display."""
import argparse
import json
import os
from pathlib import Path
import subprocess


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--results', type=Path, action='append', required=True)
    parser.add_argument('--output', type=Path, required=True)
    parser.add_argument('--session', required=True)
    parser.add_argument('--restore-initially-off', action='store_true')
    args = parser.parse_args()
    pids = set()
    for root in args.results:
        for path in root.rglob('result.json'):
            result = json.loads(path.read_text())
            if result.get('pid'):
                pids.add(result['pid'])
            for key in ['before_play', 'after_play', 'after_pause']:
                pids.update(result.get(key, {}).get('pids', []))
            for row in result.get('samples', []) + result.get('pss_samples', []):
                pids.update(row.get('resources', {}).get('pids', []))
    existing = []
    for pid in sorted(pids):
        try:
            state = Path(f'/proc/{pid}/stat').read_text().rsplit(')', 1)[1].split()[0]
            existing.append({'pid': pid, 'state': state})
        except (FileNotFoundError, ProcessLookupError):
            pass
    env = os.environ | {'DISPLAY': ':1', 'XAUTHORITY': '/run/user/1000/gdm/Xauthority'}
    session = subprocess.run(['loginctl', 'show-session', args.session, '-p', 'LockedHint', '-p', 'IdleHint'],
                             capture_output=True, text=True, timeout=5)
    result = {'observed_pids': len(pids), 'existing_pids': existing,
              'session_query_returncode': session.returncode, 'session': session.stdout,
              'initial_display': 'Off, recorded before the task' if args.restore_initially_off else 'unspecified; leave unchanged',
              'display_before_cleanup': subprocess.check_output(['xset', 'q'], env=env, text=True, timeout=5),
              'display_restored_off': False}
    # Never blank an unlocked/non-idle session or stop any other application.
    if args.restore_initially_off and not existing and session.returncode == 0 and 'LockedHint=yes' in session.stdout and 'IdleHint=yes' in session.stdout:
        subprocess.run(['xset', 'dpms', 'force', 'off'], env=env, check=True, timeout=5)
        result['display_restored_off'] = True
    result['display_after_cleanup'] = subprocess.check_output(['xset', 'q'], env=env, text=True, timeout=5)
    args.output.write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps({'observed_pids': len(pids), 'existing_pids': existing,
                      'display_restored_off': result['display_restored_off']}))


if __name__ == '__main__':
    main()
