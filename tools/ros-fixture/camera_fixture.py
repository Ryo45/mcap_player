#!/usr/bin/env python3
"""ROS Jazzy helpers for the JPEG Camera MVP.

Modes:
  synthetic  Publish generated 320x240 JPEG frames at 10 Hz.
  bridge     Convert sensor_msgs/Image input to CompressedImage output.
  convert    Convert the first ~3 seconds of a rosbag2 raw-image topic.

The ROS imports intentionally happen after argument parsing so this file can be
inspected outside a sourced ROS environment. `convert` requires rosbag2_py;
the live modes require rclpy, sensor_msgs and cv_bridge.
"""

import argparse
import time


def parser():
    result = argparse.ArgumentParser()
    result.add_argument("mode", choices=("synthetic", "bridge", "convert"))
    result.add_argument("--input-topic", default="/camera/image_raw")
    result.add_argument("--output-topic", default="/camera/front/image/compressed")
    result.add_argument("--bag")
    result.add_argument("--output", default="tests/fixtures/camera-jpeg/camera_front_3s.mcap")
    return result


def live(args):
    import cv2
    import numpy as np
    import rclpy
    from cv_bridge import CvBridge
    from rclpy.executors import ExternalShutdownException
    from rclpy.node import Node
    from rclpy.qos import qos_profile_sensor_data
    from sensor_msgs.msg import CompressedImage, Image

    class CameraNode(Node):
        def __init__(self):
            super().__init__(f"mcap_player_camera_{args.mode}")
            self.publisher = self.create_publisher(CompressedImage, args.output_topic, qos_profile_sensor_data)
            self.bridge = CvBridge()
            self.sequence = 0
            if args.mode == "synthetic":
                self.create_timer(0.1, self.publish_synthetic)
            else:
                self.create_subscription(Image, args.input_topic, self.on_raw, qos_profile_sensor_data)

        def encode(self, image, header):
            ok, encoded = cv2.imencode(".jpg", image, [cv2.IMWRITE_JPEG_QUALITY, 82])
            if not ok:
                self.get_logger().error("JPEG encode failed")
                return
            message = CompressedImage()
            message.header = header
            message.format = "jpeg compressed bgr8"
            message.data = encoded.tobytes()
            self.publisher.publish(message)

        def publish_synthetic(self):
            from std_msgs.msg import Header
            x = np.arange(320, dtype=np.uint16)[None, :]
            y = np.arange(240, dtype=np.uint16)[:, None]
            image = np.empty((240, 320, 3), np.uint8)
            image[:, :, 0] = (x + self.sequence * 5) % 256
            image[:, :, 1] = (y + self.sequence * 9) % 256
            image[:, :, 2] = 130
            header = Header()
            header.stamp = self.get_clock().now().to_msg()
            header.frame_id = "camera_front_optical_frame"
            self.encode(image, header)
            self.sequence += 1

        def on_raw(self, message):
            self.encode(self.bridge.imgmsg_to_cv2(message, "bgr8"), message.header)

    rclpy.init()
    node = CameraNode()
    try:
        rclpy.spin(node)
    except ExternalShutdownException:
        pass
    finally:
        node.destroy_node()
        if rclpy.ok():
            rclpy.shutdown()


def convert(args):
    if not args.bag:
        raise SystemExit("convert requires --bag")
    # Keep the offline conversion in ROS tooling: rosbag2_py preserves receive
    # timestamps, while deserialize_message preserves the source Header.
    import cv2
    from cv_bridge import CvBridge
    from mcap.writer import Writer
    from rclpy.serialization import deserialize_message, serialize_message
    from rosbag2_py import ConverterOptions, SequentialReader, StorageOptions
    from sensor_msgs.msg import CompressedImage, Image

    reader = SequentialReader()
    reader.open(StorageOptions(uri=args.bag, storage_id="mcap"), ConverterOptions("cdr", "cdr"))
    bridge = CvBridge()
    with open(args.output, "wb") as stream:
        writer = Writer(stream)
        writer.start(profile="ros2", library="mcap-player ros-fixture")
        schema = writer.register_schema("sensor_msgs/msg/CompressedImage", "ros2msg", b"std_msgs/Header header\nstring format\nuint8[] data\n")
        channel = writer.register_channel(args.output_topic, "cdr", schema)
        start = None
        count = 0
        while reader.has_next() and count < 30:
            topic, payload, receive_time = reader.read_next()
            if topic != args.input_topic:
                continue
            start = receive_time if start is None else start
            if receive_time - start > 3_000_000_000:
                break
            raw = deserialize_message(payload, Image)
            ok, jpeg = cv2.imencode(".jpg", bridge.imgmsg_to_cv2(raw, "bgr8"), [cv2.IMWRITE_JPEG_QUALITY, 82])
            if not ok:
                continue
            compressed = CompressedImage(header=raw.header, format="jpeg compressed bgr8", data=jpeg.tobytes())
            measurement = raw.header.stamp.sec * 1_000_000_000 + raw.header.stamp.nanosec
            writer.add_message(channel, receive_time, serialize_message(compressed), publish_time=measurement, sequence=count)
            count += 1
        writer.finish()
    print(f"wrote {count} frames to {args.output}")


if __name__ == "__main__":
    arguments = parser().parse_args()
    if arguments.mode == "convert":
        convert(arguments)
    else:
        live(arguments)
