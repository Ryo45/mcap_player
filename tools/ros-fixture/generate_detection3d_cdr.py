#!/usr/bin/env python3
"""Generate independent ROS 2 serialized codec fixture (requires ROS Jazzy vision_msgs)."""
import copy
import json
import hashlib
import subprocess
from pathlib import Path
from rclpy.serialization import serialize_message
from vision_msgs.msg import Detection3DArray, Detection3D, ObjectHypothesisWithPose

out = Path('tests/fixtures/ros2-detection3d')
out.mkdir(parents=True, exist_ok=True)
message = Detection3DArray()
message.header.stamp.sec = 12
message.header.stamp.nanosec = 3456
message.header.frame_id = 'sensor'
d = Detection3D()
d.header = copy.deepcopy(message.header)
d.id = 'track-α-42'
d.bbox.center.position.x, d.bbox.center.position.y, d.bbox.center.position.z = 1.25, -2.0, 0.75
d.bbox.center.orientation.z, d.bbox.center.orientation.w = 0.3826834323650898, 0.9238795325112867
d.bbox.size.x, d.bbox.size.y, d.bbox.size.z = 0.4, 0.3, 1.0
hypothesis = ObjectHypothesisWithPose()
hypothesis.hypothesis.class_id, hypothesis.hypothesis.score = 'person', 0.91
hypothesis.pose.pose.position.x = 3.5
hypothesis.pose.pose.orientation.w = 1.0
hypothesis.pose.covariance = [i + 0.25 for i in range(36)]
other = copy.deepcopy(hypothesis)
other.hypothesis.class_id, other.hypothesis.score = 'car', 0.2
other.pose.pose.position.x = 7.0
other.pose.covariance[0] = -1.0
d.results = [hypothesis, other]
second = copy.deepcopy(d)
second.id, second.results = '', []
second.bbox.center.position.x = -4.0
message.detections = [d, second]
payload = serialize_message(message)
with (out / 'jazzy.cdr').open('xb') as f:
    f.write(payload)
(out / 'provenance.json').write_text(json.dumps({'serializer': 'rclpy.serialization.serialize_message', 'type': 'vision_msgs/msg/Detection3DArray', 'bytes': len(payload), 'package': 'ros-jazzy-vision-msgs', 'package_version': subprocess.check_output(['dpkg-query','-W','-f=${Version}','ros-jazzy-vision-msgs'], text=True), 'sha256': hashlib.sha256(payload).hexdigest(), 'note': 'Generated using ROS 2 Jazzy package; synthetic field values, not recorded detections.'}, indent=2) + '\n')
print(f'Wrote {len(payload)} ROS-serialized bytes')
