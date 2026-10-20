#!/usr/bin/env python3
"""Reuse the frozen workload protocol with all-camera 10Hz and draw-call counters."""
import hashlib
import json
import os
from pathlib import Path
import re
import sys

import measure_desktop_workload as probe

WRAPPER = Path(__file__)
HARNESS = Path(probe.__file__)
# Keep the primary 66-run harness byte-for-byte reproducible. This one explicit
# substitution selects a separate instrumented binary; the transformed hash is archived.
source = HARNESS.read_text()
old = "ROOT/'target/gpu-native/viewer-native-workloads'"
assert source.count(old) == 1
source = source.replace(old, "ROOT/'target/gpu-native/viewer-native-cadence'")
exec(compile(source, str(HARNESS), 'exec'), probe.__dict__)

INSTALL = """(()=>{window.cadence={begin:performance.now(),canvases:[]};
for(const C of [WebGLRenderingContext,WebGL2RenderingContext]){
const clear=C.prototype.clear; C.prototype.clear=function(mask){
if(mask&this.COLOR_BUFFER_BIT){let v=window.cadence.canvases.find(v=>v.canvas===this.canvas);
if(!v){v={canvas:this.canvas,times:[]};window.cadence.canvases.push(v)}
if(v.times.length<5000)v.times.push(performance.now());}
return clear.call(this,mask)}}})()"""
SNAPSHOT = """(()=>({begin:window.cadence.begin,end:performance.now(),
canvases:window.cadence.canvases.map(v=>({rect:v.canvas.getBoundingClientRect().toJSON(),times:v.times}))}))()"""


class CadenceCDP(probe.CDP):
    snapshot = None

    def evaluate(self, expression):
        if not getattr(self, 'cadence_installed', False):
            super().evaluate(INSTALL)
            self.cadence_installed = True
        if expression == "document.querySelector('[data-testid=play-button]').click()":
            super().evaluate('window.cadence.begin=performance.now();for(const v of window.cadence.canvases)v.times=[]')
        if expression.endswith('.pausePlayback()'):
            CadenceCDP.snapshot = super().evaluate(SNAPSHOT)
        return super().evaluate(expression)


probe.CDP = CadenceCDP
all_ten = '--all-camera-10hz' in sys.argv
if all_ten:
    sys.argv.remove('--all-camera-10hz')
    os.environ['MCAP_BENCH_ALL_CAMERA_10HZ'] = '1'
# Explicitly avoid forcing redundant Scene frames. Visible updates are the contract.
os.environ.pop('MCAP_BENCH_SCENE_EVERY_FRAME', None)
output = Path(sys.argv[sys.argv.index('--output') + 1])
try:
    probe.main()
finally:
    path = output / 'result.json'
    if path.exists():
        result = json.loads(path.read_text())
        result['cadence_experiment'] = {
            'all_camera_10hz': all_ten, 'force_scene_redraw': False,
            'wrapper_sha256': hashlib.sha256(WRAPPER.read_bytes()).hexdigest(),
            'transformed_harness_sha256': hashlib.sha256(source.encode()).hexdigest(),
            'native_cadence_patch_sha256': probe.digest(probe.REPORT / 'native-cadence.patch'),
            'lichtblick_color_clears': CadenceCDP.snapshot,
        }
        if result['app'] == 'native':
            text = (output / 'app.log').read_text()
            # Bound by the actual commands, excluding seeks and paused screenshots.
            segments = re.split(r'GPU_BENCH_COMMAND id=\d+ command=', text)
            playing = next((s for s in segments if s.startswith('play\n')), '')
            counts = {kind: len(re.findall(r'GPU_BENCH_CADENCE ' + kind + r'\b', playing))
                      for kind in ['present', 'scene']}
            result['cadence_experiment']['native_calls_during_play'] = counts
        if all_ten:
            result['camera_policy'] = 'Native all displayed cameras <=10Hz; Lichtblick receives all displayed 10Hz streams'
        path.write_text(json.dumps(result, indent=2) + '\n')
