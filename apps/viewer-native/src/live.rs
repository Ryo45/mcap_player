//! ROS adapter boundary shared by a future `rclrs` callback implementation.
//!
//! The bounded latest mailbox is usable without ROS, which keeps normal and
//! WASM builds free from ROS native dependencies.

#![allow(dead_code)]

use std::sync::Mutex;

#[derive(Debug)]
pub struct LatestMailbox<T> {
    value: Mutex<Option<T>>,
    coalesced: Mutex<u64>,
}

impl<T> Default for LatestMailbox<T> {
    fn default() -> Self {
        Self {
            value: Mutex::new(None),
            coalesced: Mutex::new(0),
        }
    }
}

impl<T> LatestMailbox<T> {
    pub fn push(&self, value: T) {
        let mut slot = self.value.lock().expect("latest mailbox poisoned");
        if slot.replace(value).is_some() {
            *self.coalesced.lock().expect("counter poisoned") += 1;
        }
    }

    pub fn take(&self) -> Option<T> {
        self.value.lock().expect("latest mailbox poisoned").take()
    }
    pub fn coalesced(&self) -> u64 {
        *self.coalesced.lock().expect("counter poisoned")
    }
}

#[cfg(feature = "ros2-live")]
mod ros {
    use super::LatestMailbox;
    use rclrs::{
        Context, CreateBasicExecutor, DynamicMessage, QOS_PROFILE_SENSOR_DATA,
        QoSReliabilityPolicy, RclrsErrorFilter, SequenceValue, SimpleValue, SpinOptions,
        SubscriptionOptions, Value,
    };
    use std::{
        sync::{
            Arc, Mutex,
            atomic::{AtomicU64, Ordering},
        },
        thread,
        time::{SystemTime, UNIX_EPOCH},
    };
    use viewer_core::{
        ArrivalTime, CompressedImage, MeasurementTime, RawMessage, StreamId,
        encode_compressed_image_cdr,
    };

    pub struct RosLiveHandle {
        mailbox: Arc<LatestMailbox<RawMessage>>,
        copied_bytes: Arc<AtomicU64>,
        received: Arc<AtomicU64>,
        error: Arc<Mutex<Option<String>>>,
    }

    impl RosLiveHandle {
        pub fn start(topic: String, reliable: bool) -> Self {
            let mailbox = Arc::new(LatestMailbox::default());
            let copied_bytes = Arc::new(AtomicU64::new(0));
            let received = Arc::new(AtomicU64::new(0));
            let error = Arc::new(Mutex::new(None));
            let thread_mailbox = Arc::clone(&mailbox);
            let thread_copied = Arc::clone(&copied_bytes);
            let thread_received = Arc::clone(&received);
            let thread_error = Arc::clone(&error);
            thread::Builder::new()
                .name("viewer-ros2-executor".into())
                .spawn(move || {
                    let result = run_executor(
                        topic,
                        reliable,
                        thread_mailbox,
                        thread_copied,
                        thread_received,
                    );
                    if let Err(value) = result {
                        *thread_error.lock().expect("ROS error state poisoned") = Some(value);
                    }
                })
                .expect("spawn ROS executor thread");
            Self {
                mailbox,
                copied_bytes,
                received,
                error,
            }
        }

        pub fn take(&self) -> Option<RawMessage> {
            self.mailbox.take()
        }
        pub fn coalesced(&self) -> u64 {
            self.mailbox.coalesced()
        }
        pub fn copied_bytes(&self) -> u64 {
            self.copied_bytes.load(Ordering::Relaxed)
        }
        pub fn received(&self) -> u64 {
            self.received.load(Ordering::Relaxed)
        }
        pub fn error(&self) -> Option<String> {
            self.error.lock().expect("ROS error state poisoned").clone()
        }
    }

    fn run_executor(
        topic: String,
        reliable: bool,
        mailbox: Arc<LatestMailbox<RawMessage>>,
        copied_bytes: Arc<AtomicU64>,
        received: Arc<AtomicU64>,
    ) -> Result<(), String> {
        let context = Context::default_from_env().map_err(|error| error.to_string())?;
        let mut executor = context.create_basic_executor();
        let node = executor
            .create_node("mcap_player_camera_viewer")
            .map_err(|error| error.to_string())?;
        let mut options = SubscriptionOptions::new(&topic);
        options.qos = QOS_PROFILE_SENSOR_DATA;
        if reliable {
            options.qos.reliability = QoSReliabilityPolicy::Reliable;
        }
        let callback = move |message: DynamicMessage, _info: rclrs::MessageInfo| {
            // Arrival is captured before introspection or CDR reconstruction work.
            let arrival = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .ok()
                .and_then(|duration| i64::try_from(duration.as_nanos()).ok());
            let Some(arrival) = arrival else {
                return;
            };
            let Ok(image) = extract_compressed_image(&message) else {
                return;
            };
            let Ok(payload) = encode_compressed_image_cdr(&image) else {
                return;
            };
            copied_bytes.fetch_add(payload.len() as u64, Ordering::Relaxed);
            received.fetch_add(1, Ordering::Relaxed);
            mailbox.push(RawMessage {
                stream_id: StreamId(1),
                arrival_time: ArrivalTime(arrival),
                payload: payload.into(),
            });
        };
        let _subscription = node
            .create_dynamic_subscription(
                "sensor_msgs/msg/CompressedImage"
                    .try_into()
                    .map_err(|error: rclrs::DynamicMessageError| error.to_string())?,
                options,
                callback,
            )
            .map_err(|error| error.to_string())?;
        executor
            .spin(SpinOptions::default())
            .first_error()
            .map_err(|error| error.to_string())
    }

    fn extract_compressed_image(message: &DynamicMessage) -> Result<CompressedImage, String> {
        let header = simple_message(message.get("header"), "header")?;
        let stamp = simple_message(header.get("stamp"), "header.stamp")?;
        let seconds = match stamp.get("sec") {
            Some(Value::Simple(SimpleValue::Int32(value))) => *value,
            _ => return Err("header.stamp.sec has the wrong type".into()),
        };
        let nanoseconds = match stamp.get("nanosec") {
            Some(Value::Simple(SimpleValue::Uint32(value))) => *value,
            _ => return Err("header.stamp.nanosec has the wrong type".into()),
        };
        if nanoseconds >= 1_000_000_000 {
            return Err("header timestamp has invalid nanoseconds".into());
        }
        let measurement = i64::from(seconds)
            .checked_mul(1_000_000_000)
            .and_then(|value| value.checked_add(i64::from(nanoseconds)))
            .ok_or_else(|| "header timestamp overflow".to_owned())?;
        let frame_id = simple_string(header.get("frame_id"), "header.frame_id")?;
        let format = simple_string(message.get("format"), "format")?;
        let jpeg = match message.get("data") {
            Some(Value::Sequence(SequenceValue::Uint8Sequence(value))) => value.to_vec(),
            Some(Value::Sequence(SequenceValue::OctetSequence(value))) => value.to_vec(),
            _ => return Err("data has the wrong type".into()),
        };
        Ok(CompressedImage {
            measurement_time: MeasurementTime(measurement),
            frame_id,
            format,
            jpeg,
        })
    }

    fn simple_message<'a>(
        value: Option<Value<'a>>,
        name: &str,
    ) -> Result<rclrs::DynamicMessageView<'a>, String> {
        match value {
            Some(Value::Simple(SimpleValue::Message(value))) => Ok(value),
            _ => Err(format!("{name} has the wrong type")),
        }
    }

    fn simple_string(value: Option<Value<'_>>, name: &str) -> Result<String, String> {
        match value {
            Some(Value::Simple(SimpleValue::String(value))) => Ok(value.to_string()),
            _ => Err(format!("{name} has the wrong type")),
        }
    }
}

#[cfg(feature = "ros2-live")]
pub use ros::RosLiveHandle;

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn replaces_unconsumed_value() {
        let mailbox = LatestMailbox::default();
        mailbox.push(1);
        mailbox.push(2);
        assert_eq!(mailbox.take(), Some(2));
        assert_eq!(mailbox.coalesced(), 1);
    }
}
