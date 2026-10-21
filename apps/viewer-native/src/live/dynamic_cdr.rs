use super::catalog::LiveStreamRole;
use rclrs::{DynamicMessage, DynamicMessageView, SequenceValue, SimpleValue, Value};
use viewer_core::{
    CompressedImage, LaserScan, MeasurementTime, Odometry, PathMessage, TransformStamped,
    encode_compressed_image_cdr, encode_laser_scan_cdr, encode_odometry_cdr, encode_path_cdr,
    encode_tf_message_cdr,
};

pub(super) fn encode_dynamic_message(
    role: LiveStreamRole,
    message: &DynamicMessage,
) -> Result<Vec<u8>, String> {
    let encoded = match role {
        LiveStreamRole::Camera => encode_compressed_image_cdr(&extract_compressed_image(message)?),
        LiveStreamRole::Path => encode_path_cdr(&extract_path(message)?),
        LiveStreamRole::Odometry => encode_odometry_cdr(&extract_odometry(message)?),
        LiveStreamRole::LaserScan => encode_laser_scan_cdr(&extract_laser_scan(message)?),
        LiveStreamRole::DynamicTf | LiveStreamRole::StaticTf => {
            encode_tf_message_cdr(&extract_transforms(message)?)
        }
    };
    encoded.map_err(|error| error.to_string())
}

fn extract_compressed_image(message: &DynamicMessage) -> Result<CompressedImage, String> {
    let (measurement_time, frame_id) = header(message.get("header"), "header")?;
    let format = string(message.get("format"), "format")?;
    let jpeg = match message.get("data") {
        Some(Value::Sequence(SequenceValue::Uint8Sequence(value)))
        | Some(Value::Sequence(SequenceValue::OctetSequence(value))) => value.as_slice().to_vec(),
        _ => return Err("data has the wrong type".into()),
    };
    Ok(CompressedImage {
        measurement_time,
        frame_id,
        format,
        data: jpeg,
    })
}

fn extract_path(message: &DynamicMessage) -> Result<PathMessage, String> {
    let (measurement_time, frame_id) = header(message.get("header"), "header")?;
    let poses = match message.get("poses") {
        Some(Value::Sequence(SequenceValue::MessageSequence(poses))) => poses,
        _ => return Err("poses has the wrong type".into()),
    };
    let points = poses
        .iter()
        .map(|pose_stamped| {
            let (_, pose_frame) = header(pose_stamped.get("header"), "poses[].header")?;
            if !pose_frame.is_empty()
                && pose_frame.trim_start_matches('/') != frame_id.trim_start_matches('/')
            {
                return Err("Path poses must share the Path header frame".into());
            }
            let pose = message_view(pose_stamped.get("pose"), "poses[].pose")?;
            let position = message_view(pose.get("position"), "poses[].pose.position")?;
            Ok([
                double(position.get("x"), "poses[].pose.position.x")?,
                double(position.get("y"), "poses[].pose.position.y")?,
                double(position.get("z"), "poses[].pose.position.z")?,
            ])
        })
        .collect::<Result<Vec<_>, String>>()?;
    Ok(PathMessage {
        measurement_time,
        frame_id,
        points,
    })
}

fn extract_odometry(message: &DynamicMessage) -> Result<Odometry, String> {
    let (measurement_time, frame_id) = header(message.get("header"), "header")?;
    let child_frame_id = string(message.get("child_frame_id"), "child_frame_id")?;
    let pose_with_covariance = message_view(message.get("pose"), "pose")?;
    let pose = message_view(pose_with_covariance.get("pose"), "pose.pose")?;
    let position = vector3(pose.get("position"), "pose.pose.position")?;
    let orientation = quaternion(pose.get("orientation"), "pose.pose.orientation")?;
    let twist_with_covariance = message_view(message.get("twist"), "twist")?;
    let twist = message_view(twist_with_covariance.get("twist"), "twist.twist")?;
    let linear_velocity = vector3(twist.get("linear"), "twist.twist.linear")?;
    let angular_velocity = vector3(twist.get("angular"), "twist.twist.angular")?;
    Ok(Odometry {
        measurement_time,
        frame_id,
        child_frame_id,
        position,
        orientation,
        linear_velocity,
        angular_velocity,
    })
}

fn extract_laser_scan(message: &DynamicMessage) -> Result<LaserScan, String> {
    let (measurement_time, frame_id) = header(message.get("header"), "header")?;
    let ranges = match message.get("ranges") {
        Some(Value::Sequence(SequenceValue::FloatSequence(ranges))) => ranges.as_slice().to_vec(),
        _ => return Err("ranges has the wrong type".into()),
    };
    Ok(LaserScan {
        measurement_time,
        frame_id,
        angle_min: float(message.get("angle_min"), "angle_min")?,
        angle_increment: float(message.get("angle_increment"), "angle_increment")?,
        range_min: float(message.get("range_min"), "range_min")?,
        range_max: float(message.get("range_max"), "range_max")?,
        ranges,
    })
}

fn extract_transforms(message: &DynamicMessage) -> Result<Vec<TransformStamped>, String> {
    let transforms = match message.get("transforms") {
        Some(Value::Sequence(SequenceValue::MessageSequence(transforms))) => transforms,
        _ => return Err("transforms has the wrong type".into()),
    };
    transforms
        .iter()
        .map(|stamped| {
            let (measurement_time, frame_id) =
                header(stamped.get("header"), "transforms[].header")?;
            let child_frame_id =
                string(stamped.get("child_frame_id"), "transforms[].child_frame_id")?;
            let transform = message_view(stamped.get("transform"), "transforms[].transform")?;
            Ok(TransformStamped {
                measurement_time,
                frame_id,
                child_frame_id,
                translation: vector3(
                    transform.get("translation"),
                    "transforms[].transform.translation",
                )?,
                rotation: quaternion(transform.get("rotation"), "transforms[].transform.rotation")?,
            })
        })
        .collect()
}

fn header(value: Option<Value<'_>>, name: &str) -> Result<(MeasurementTime, String), String> {
    let header = message_view(value, name)?;
    let stamp = message_view(header.get("stamp"), &format!("{name}.stamp"))?;
    let seconds = int32(stamp.get("sec"), &format!("{name}.stamp.sec"))?;
    let nanoseconds = uint32(stamp.get("nanosec"), &format!("{name}.stamp.nanosec"))?;
    if nanoseconds >= 1_000_000_000 {
        return Err(format!("{name}.stamp has invalid nanoseconds"));
    }
    let measurement_time = i64::from(seconds)
        .checked_mul(1_000_000_000)
        .and_then(|value| value.checked_add(i64::from(nanoseconds)))
        .ok_or_else(|| format!("{name}.stamp overflows signed nanoseconds"))?;
    Ok((
        MeasurementTime(measurement_time),
        string(header.get("frame_id"), &format!("{name}.frame_id"))?,
    ))
}

fn vector3(value: Option<Value<'_>>, name: &str) -> Result<[f64; 3], String> {
    let value = message_view(value, name)?;
    Ok([
        double(value.get("x"), &format!("{name}.x"))?,
        double(value.get("y"), &format!("{name}.y"))?,
        double(value.get("z"), &format!("{name}.z"))?,
    ])
}

fn quaternion(value: Option<Value<'_>>, name: &str) -> Result<[f64; 4], String> {
    let value = message_view(value, name)?;
    Ok([
        double(value.get("x"), &format!("{name}.x"))?,
        double(value.get("y"), &format!("{name}.y"))?,
        double(value.get("z"), &format!("{name}.z"))?,
        double(value.get("w"), &format!("{name}.w"))?,
    ])
}

fn message_view<'a>(
    value: Option<Value<'a>>,
    name: &str,
) -> Result<DynamicMessageView<'a>, String> {
    match value {
        Some(Value::Simple(SimpleValue::Message(value))) => Ok(value),
        _ => Err(format!("{name} has the wrong type")),
    }
}

fn string(value: Option<Value<'_>>, name: &str) -> Result<String, String> {
    match value {
        Some(Value::Simple(SimpleValue::String(value))) => Ok(value.to_string()),
        _ => Err(format!("{name} has the wrong type")),
    }
}

fn int32(value: Option<Value<'_>>, name: &str) -> Result<i32, String> {
    match value {
        Some(Value::Simple(SimpleValue::Int32(value))) => Ok(*value),
        _ => Err(format!("{name} has the wrong type")),
    }
}

fn uint32(value: Option<Value<'_>>, name: &str) -> Result<u32, String> {
    match value {
        Some(Value::Simple(SimpleValue::Uint32(value))) => Ok(*value),
        _ => Err(format!("{name} has the wrong type")),
    }
}

fn float(value: Option<Value<'_>>, name: &str) -> Result<f32, String> {
    match value {
        Some(Value::Simple(SimpleValue::Float(value))) => Ok(*value),
        _ => Err(format!("{name} has the wrong type")),
    }
}

fn double(value: Option<Value<'_>>, name: &str) -> Result<f64, String> {
    match value {
        Some(Value::Simple(SimpleValue::Double(value))) => Ok(*value),
        _ => Err(format!("{name} has the wrong type")),
    }
}
