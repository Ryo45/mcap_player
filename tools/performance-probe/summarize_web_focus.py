#!/usr/bin/env python3
"""Summarize the controlled 10 Hz / 32 JPEG variant focus probe (not arbitrary MCAPs)."""
import argparse
import bisect
import gzip
import json
import math
from pathlib import Path


def percentile(values, fraction):
    values = sorted(values)
    return values[min(len(values)-1, math.ceil(fraction*len(values))-1)] if values else None


def summarize(path):
    with (gzip.open(path, 'rt') if path.suffix == '.gz' else path.open()) as source:
        data = json.load(source)
    if data['status'] != 'ok' or data['mode'] != 'focus':
        raise ValueError(f'{path}: incomplete focus run')
    variants = {item['signature']: i for i, item in enumerate(data['jpeg_manifest'])}
    span = data['span_seconds']
    origin = data.get('time_origin_ms') if data.get('time_origin_source')=='loaded-page' else None
    # Older baseline capture predates explicit timeOrigin. Rates/gaps and paint
    # ages don't require it; don't report sample ages or switch latency for it.
    cameras = [[] for _ in range(8)]
    for paint in data['paints']:
        if paint['phase'] != 'play':
            continue
        job = data['jobs'][paint['job']]
        variant = variants[job['signature']]
        modulo = (variant-paint['camera']*7) % 32
        latest = math.floor(job['cursor']*span*10 + .06)  # timeline rounded to 4 decimals
        sequence = modulo + 32*((latest-modulo)//32)
        admission_age = job['cursor']*span-sequence/10
        # The fixture repeats every 3.2 s. These are cycle-disambiguation
        # checks, not evidence of a general upper bound on camera latency.
        if sequence < 0 or not -.007 <= admission_age <= .5:
            # A focus switch can paint before the selected card DOM is updated
            # in old captures. Exclude only the documented transition region.
            if origin is None and any(abs(paint['cursor']*span-x) < .5 for x in (20, 40)):
                continue
            raise ValueError(f'{path}: ambiguous job/camera identity {paint}')
        if (job.get('received', job['post'])-job['post']) >= 3000:
            raise ValueError('decode crossed JPEG repetition period')
        cameras[paint['camera']].append(paint | {'sequence': sequence,
            'age_ms': 1000*(paint['cursor']*span-sequence/10)})
    segments = []
    for start, end, focus in [(2,18,0), (22,38,3), (42,58,0)]:
        entries = [[p for p in paints if start <= p['cursor']*span < end]
                   for paints in cameras]
        # Playback is real-time here; also record measured wall duration at
        # sampled crossings so that a slower run cannot silently inflate Hz.
        samples = [s for s in data['samples'] if s['phase']=='play']
        def wall_at(log_time):
            for a, b in zip(samples, samples[1:]):
                lo, hi = a['cursor']*span, b['cursor']*span
                if lo <= log_time <= hi and hi > lo:
                    return a['now']+(b['now']-a['now'])*(log_time-lo)/(hi-lo)
            raise ValueError('missing measurement interval')
        wall_seconds = (wall_at(end)-wall_at(start))/1000
        rates = [len(values)/wall_seconds for values in entries]
        # Include the paints immediately outside the steady interval when
        # measuring gaps, so silence at an interval boundary is not hidden.
        gaps = []
        for paints in cameras:
            pairs = [(a,b) for a,b in zip(paints, paints[1:])
                     if a['cursor']*span < end and b['cursor']*span > start]
            gaps.append(max((b['t']-a['t'] for a,b in pairs), default=wall_seconds*1000))
        bucket_min = min(sum(lo <= p['cursor']*span < lo+5 for p in cameras[c]) /
                         ((wall_at(lo+5)-wall_at(lo))/1000)
                         for c in range(8) if c != focus for lo in range(start+1, end-4, 5))
        sample_ages = []
        if origin is not None:
            times = [p['t'] for p in cameras[focus]]
            for sample in samples:
                if start <= sample['cursor']*span < end:
                    index = bisect.bisect_right(times, origin+sample['now'])-1
                    if index >= 0:
                        sample_ages.append(1000*(sample['cursor']*span-cameras[focus][index]['sequence']/10))
        segments.append({'log_seconds':[start,end], 'wall_seconds':wall_seconds, 'focus':focus,
            'camera_hz':rates,'focus_hz':rates[focus],
            'background_min_hz':min(r for c,r in enumerate(rates) if c!=focus),
            'background_min_5s_hz':bucket_min,
            'background_max_gap_ms':max(g for c,g in enumerate(gaps) if c!=focus and g is not None),
            'focus_max_gap_ms':gaps[focus],
            'focus_paint_age_p95_ms':percentile([p['age_ms'] for p in entries[focus]],.95),
            'focus_sample_age_p95_ms':percentile(sample_ages,.95)})
    switches = []
    if origin is not None:
        for change in data['focus_changes']:
            start = origin+change['now']
            paint = next(p for p in cameras[change['camera']] if p['t']>=start
                         and data['jobs'][p['job']]['width']==960)
            switches.append({'camera':change['camera'],'latency_ms':paint['t']-start})
    return {'input':str(path),'segments':segments,'focus_switches':switches,
            'play_seconds':data['play_ms']/1000,
            'jobs_per_second':sum(j['phase']=='play' for j in data['jobs'])/(data['play_ms']/1000),
            'peak_pss_mib':max(s['process']['pss_kib'] for s in data['samples'])/1024,
            'peak_wasm_mib':max(s['wasm_bytes'] or 0 for s in data['samples'])/1024**2}


if __name__=='__main__':
    parser=argparse.ArgumentParser(description=__doc__)
    parser.add_argument('inputs',type=Path,nargs='+')
    parser.add_argument('--output',type=Path,required=True)
    args=parser.parse_args()
    args.output.write_text(json.dumps([summarize(p) for p in args.inputs],indent=2)+'\n')
