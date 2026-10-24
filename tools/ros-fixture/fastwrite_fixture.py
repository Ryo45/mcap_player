#!/usr/bin/env python3
"""Create a genuine rosbag2 fastwrite copy, or verify serialized ROS message fidelity.

Requires a sourced ROS 2 environment with rosbag2_py. Payloads are never decoded.
This is a fixture writer, not an archival converter: non-message records are not copied.
"""
import argparse
import hashlib
import json
from pathlib import Path

import rosbag2_py


def reader(path):
    result = rosbag2_py.SequentialReader()
    result.open(rosbag2_py.StorageOptions(uri=str(path), storage_id='mcap'),
                rosbag2_py.ConverterOptions('', ''))
    return result


def fingerprint(path):
    source = reader(path)
    topics = sorted((topic.name, topic.type, topic.serialization_format)
                    for topic in source.get_all_topics_and_types())
    digest = hashlib.sha256()
    count = 0
    first = last = None
    while source.has_next():
        topic, payload, timestamp = source.read_next()
        name = topic.encode()
        digest.update(len(name).to_bytes(8, 'little'))
        digest.update(name)
        digest.update(timestamp.to_bytes(8, 'little', signed=True))
        digest.update(len(payload).to_bytes(8, 'little'))
        digest.update(payload)
        first = timestamp if first is None else min(first, timestamp)
        last = timestamp if last is None else max(last, timestamp)
        count += 1
    return {'messages': count, 'topics': topics, 'first_ns': first, 'last_ns': last,
            'ordered_serialized_messages_sha256': digest.hexdigest()}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('mode', choices=['create', 'verify'])
    parser.add_argument('input', type=Path)
    parser.add_argument('output', type=Path)
    args = parser.parse_args()
    if args.mode == 'create':
        if args.output.exists():
            parser.error('output directory must be new')
        source = reader(args.input)
        target = rosbag2_py.SequentialWriter()
        target.open(rosbag2_py.StorageOptions(uri=str(args.output), storage_id='mcap',
                                            storage_preset_profile='fastwrite'),
                    rosbag2_py.ConverterOptions('', ''))
        for topic in source.get_all_topics_and_types():
            target.create_topic(topic)
        while source.has_next():
            topic, payload, timestamp = source.read_next()
            target.write(topic, payload, timestamp)
        del target  # Finalize Footer/Summary and metadata.yaml before checking.
    before, after = fingerprint(args.input), fingerprint(args.output)
    if before != after:
        raise RuntimeError(f'ROS message fidelity mismatch: {before} != {after}')
    print(json.dumps({'status': 'ok', 'mode': args.mode, 'input': str(args.input),
                      'output': str(args.output), 'facts': after}, indent=2))


if __name__ == '__main__':
    main()
