#!/usr/bin/env python3
"""Check the known 3.1-second synthetic scan against both calibrated screenshots."""
import argparse
import json
import math
from pathlib import Path

from PIL import Image


def dot(a, b):
    return sum(x*y for x, y in zip(a, b))


def cross(a, b):
    return [a[1]*b[2]-a[2]*b[1], a[2]*b[0]-a[0]*b[2], a[0]*b[1]-a[1]*b[0]]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--native', type=Path, required=True)
    parser.add_argument('--lichtblick', type=Path, required=True)
    parser.add_argument('--output', type=Path, required=True)
    args = parser.parse_args()
    elevation, azimuth = math.radians(30), math.radians(45)
    eye = [-20*math.cos(elevation)*math.cos(azimuth),
           -20*math.cos(elevation)*math.sin(azimuth), 20*math.sin(elevation)]
    forward = [-v/20 for v in eye]
    right = cross(forward, [0, 0, 1])
    length = math.sqrt(dot(right, right))
    right = [v/length for v in right]
    up = cross(right, forward)
    result = {'sequence': 31, 'scene_rect': [320, 76, 1120, 926],
              'camera_distance_m': 20, 'elevation_deg': 30, 'azimuth_deg': 45,
              'fov_y_deg': 45, 'tolerance_px': 3, 'images': {}}
    for app in ['native', 'lichtblick']:
        image = Image.open(getattr(args, app)).convert('RGB')
        if image.size != (1920, 1080):
            raise ValueError('expected FHD content capture')
        hits = 0
        for index in range(360):
            distance = 1 + ((index+31) % 200)/100
            point = [distance*math.cos(index*.01745), distance*math.sin(index*.01745), 0]
            vector = [p-e for p, e in zip(point, eye)]
            scale = 926/(2*math.tan(math.radians(22.5))*dot(vector, forward))
            x = round(880 + dot(vector, right)*scale)
            y = round(539 - dot(vector, up)*scale)
            pixels = [image.getpixel((x+dx, y+dy)) for dx in range(-3, 4) for dy in range(-3, 4)]
            hits += any(r < 150 and g > 160 and b > 200 for r, g, b in pixels)
        result['images'][app] = {'path': str(getattr(args, app)), 'projected_points': 360,
                                 'cyan_pixel_hits': hits}
    args.output.write_text(json.dumps(result, indent=2)+'\n')
    if any(v['cyan_pixel_hits'] != 360 for v in result['images'].values()):
        raise SystemExit('scan projection mismatch')
    print(json.dumps(result))


if __name__ == '__main__':
    main()
