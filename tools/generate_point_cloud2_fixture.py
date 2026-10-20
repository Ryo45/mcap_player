#!/usr/bin/env python3
"""Generate tiny interoperability fixtures with ROS 2 Jazzy's actual serializer."""
import json
from pathlib import Path
import struct
from rclpy.serialization import serialize_message
from sensor_msgs.msg import PointCloud2, PointField
from std_msgs.msg import Header
from builtin_interfaces.msg import Time

root = Path(__file__).resolve().parents[1] / "tests/fixtures/point-cloud2"
root.mkdir(parents=True, exist_ok=True)
fields = [
    PointField(name="z", offset=8, datatype=PointField.FLOAT32, count=1),
    PointField(name="rgba", offset=12, datatype=PointField.UINT32, count=1),
    PointField(name="x", offset=0, datatype=PointField.FLOAT32, count=1),
    PointField(name="intensity", offset=16, datatype=PointField.FLOAT64, count=1),
    PointField(name="y", offset=4, datatype=PointField.FLOAT32, count=1),
    PointField(name="ring", offset=24, datatype=PointField.UINT16, count=1),
]
points = [(1., 2., 3., 0xffff0000, 0.), (4., 5., 6., 0x8000ff00, .5),
          (float("nan"), 1., 2., 0xffff00ff, .75), (-1., -2., -3., 0xff0000ff, 1.)]
for big_endian in [False, True]:
    data = bytearray()
    for row in range(2):
        for x,y,z,rgba,intensity in points[row*2:row*2+2]:
            data += struct.pack((">" if big_endian else "<")+"fffIdH6x",x,y,z,rgba,intensity,7)
        data += b"\xa5"*8
    message = PointCloud2(header=Header(stamp=Time(sec=12,nanosec=345000000),frame_id="lidar"),
                         height=2,width=2,fields=fields,is_bigendian=big_endian,
                         point_step=32,row_step=72,data=bytes(data),is_dense=True)
    (root/("organized-big-data.cdr" if big_endian else "organized-little-data.cdr")).write_bytes(serialize_message(message))
(root/"empty.cdr").write_bytes(serialize_message(PointCloud2()))
(root/"expected.json").write_text(json.dumps({
    "serializer":"rclpy.serialization.serialize_message (ROS 2 Jazzy)",
    "measurement_time_ns":12345000000,"frame_id":"lidar","height":2,"width":2,
    "point_step":32,"row_step":72,"is_dense":True,
    "finite_xyz":[[1,2,3],[4,5,6],[-1,-2,-3]],
    "rgba":[[255,0,0,255],[0,255,0,128],[0,0,255,255]],"intensity":[0,.5,1],
    "invalid_xyz":"one NaN point despite is_dense=true"
},indent=2)+"\n")
