std_msgs/Header header
vision_msgs/Detection3D[] detections
================================================================================
MSG: std_msgs/Header
builtin_interfaces/Time stamp
string frame_id
================================================================================
MSG: builtin_interfaces/Time
int32 sec
uint32 nanosec
================================================================================
MSG: vision_msgs/Detection3D
std_msgs/Header header
vision_msgs/ObjectHypothesisWithPose[] results
vision_msgs/BoundingBox3D bbox
string id
================================================================================
MSG: vision_msgs/ObjectHypothesisWithPose
vision_msgs/ObjectHypothesis hypothesis
geometry_msgs/PoseWithCovariance pose
================================================================================
MSG: vision_msgs/ObjectHypothesis
string class_id
float64 score
================================================================================
MSG: geometry_msgs/PoseWithCovariance
geometry_msgs/Pose pose
float64[36] covariance
================================================================================
MSG: geometry_msgs/Pose
geometry_msgs/Point position
geometry_msgs/Quaternion orientation
================================================================================
MSG: geometry_msgs/Point
float64 x
float64 y
float64 z
================================================================================
MSG: geometry_msgs/Quaternion
float64 x
float64 y
float64 z
float64 w
================================================================================
MSG: vision_msgs/BoundingBox3D
geometry_msgs/Pose center
geometry_msgs/Vector3 size
================================================================================
MSG: geometry_msgs/Vector3
float64 x
float64 y
float64 z
