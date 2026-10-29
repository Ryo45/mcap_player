//! Standalone Core Wasm guest. ROS/CDR and egui belong to the host.
use viewer_extension::panel::{
    HostCommand, InputStatus, MAX_OUTPUT_BYTES, MAX_UPDATE_BYTES, PANEL_API_VERSION,
    PanelDefinition, PanelEvent, PanelInputDefinition, PanelInputSnapshot, PanelOutput,
    PanelUpdate, PanelValue, UiNode,
};
static mut INPUT: [u8; MAX_UPDATE_BYTES] = [0; MAX_UPDATE_BYTES];
static mut OUTPUT: [u8; MAX_OUTPUT_BYTES] = [0; MAX_OUTPUT_BYTES];
#[unsafe(no_mangle)]
pub extern "C" fn panel_api_version() -> u32 {
    PANEL_API_VERSION
}
#[unsafe(no_mangle)]
pub extern "C" fn init() {}
#[unsafe(no_mangle)]
pub extern "C" fn input_ptr() -> *mut u8 {
    &raw mut INPUT as *mut u8
}
#[unsafe(no_mangle)]
pub extern "C" fn output_ptr() -> *mut u8 {
    &raw mut OUTPUT as *mut u8
}
fn input(length: u32) -> Option<&'static [u8]> {
    if length as usize > MAX_UPDATE_BYTES {
        return None;
    }
    // SAFETY: host calls exports serially; length is bounded by this fixed scratch buffer.
    Some(unsafe { std::slice::from_raw_parts(input_ptr(), length as usize) })
}
fn write(value: impl serde::Serialize) -> u32 {
    let Ok(bytes) = serde_json::to_vec(&value) else {
        return u32::MAX;
    };
    if bytes.len() > MAX_OUTPUT_BYTES {
        return u32::MAX;
    }
    // SAFETY: validated output fits this instance's scratch array.
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr(), output_ptr(), bytes.len());
    }
    bytes.len() as u32
}
#[unsafe(no_mangle)]
pub extern "C" fn define(length: u32) -> u32 {
    let Some(bytes) = input(length) else {
        return u32::MAX;
    };
    let Ok(settings) = serde_json::from_slice::<serde_json::Value>(bytes) else {
        return u32::MAX;
    };
    write(PanelDefinition {
        api_version: PANEL_API_VERSION,
        id: "robot-monitor".into(),
        title: "Robot / perception monitor".into(),
        inputs: vec![
            PanelInputDefinition {
                id: "status".into(),
                topic: settings["statusTopic"]
                    .as_str()
                    .unwrap_or("/robot/status")
                    .into(),
                fields: ["battery", "state"].map(String::from).to_vec(),
                plots: ["battery", "temperature"].map(String::from).to_vec(),
                history_seconds: 10,
                max_points: 128,
            },
            PanelInputDefinition {
                id: "tracks".into(),
                topic: settings["tracksTopic"]
                    .as_str()
                    .unwrap_or("/perception/tracks")
                    .into(),
                fields: [
                    "frame",
                    "objects[].id",
                    "objects[].label",
                    "objects[].confidence",
                    "objects[].distance",
                    "timings_ms[]",
                ]
                .map(String::from)
                .to_vec(),
                plots: vec![],
                history_seconds: 10,
                max_points: 128,
            },
        ],
    })
}
fn text(text: impl Into<String>) -> UiNode {
    UiNode::Text { text: text.into() }
}
fn array<'a>(input: &'a PanelInputSnapshot, path: &str) -> &'a [PanelValue] {
    match input.values.get(path) {
        Some(PanelValue::Array(values)) => values,
        _ => &[],
    }
}
fn display(input: &PanelInputSnapshot, path: &str) -> String {
    input
        .values
        .get(path)
        .map_or_else(|| "missing".into(), PanelValue::display)
}
#[unsafe(no_mangle)]
pub extern "C" fn update(length: u32) -> u32 {
    let Some(bytes) = input(length) else {
        return u32::MAX;
    };
    let Ok(update) = serde_json::from_slice::<PanelUpdate>(bytes) else {
        return u32::MAX;
    };
    let mut children = vec![text(
        "Independent latest inputs — no timestamp synchronization",
    )];
    if let Some(status) = update.inputs.get("status") {
        if status.status == InputStatus::Ready {
            children.push(UiNode::Row {
                children: vec![
                    UiNode::Value {
                        label: "Battery".into(),
                        value: display(status, "battery"),
                        unit: "%".into(),
                    },
                    UiNode::Value {
                        label: "Robot state".into(),
                        value: display(status, "state"),
                        unit: "".into(),
                    },
                ],
            });
            children.push(text(format!(
                "Status log time: {}",
                status.log_time_ns.as_deref().unwrap_or("unknown")
            )));
            children.push(UiNode::Plot {
                title: "Robot status history".into(),
                series: vec!["status/battery".into(), "status/temperature".into()],
            });
        } else {
            children.push(text(format!(
                "Status {:?}: {}",
                status.status,
                status.errors.join("; ")
            )));
        }
    }
    if let Some(tracks) = update.inputs.get("tracks") {
        if tracks.status == InputStatus::Ready {
            let ids = array(tracks, "objects[].id");
            let labels = array(tracks, "objects[].label");
            let confidence = array(tracks, "objects[].confidence");
            let distances = array(tracks, "objects[].distance");
            children.push(text(format!(
                "{} tracked objects | frame {} | log time {}",
                ids.len(),
                display(tracks, "frame"),
                tracks.log_time_ns.as_deref().unwrap_or("unknown")
            )));
            let mut rows = vec![text("Object ID / class / confidence / distance (m)")];
            for (index, id) in ids.iter().take(8).enumerate() {
                rows.push(UiNode::Row {
                    children: vec![
                        text(id.display()),
                        text(
                            labels
                                .get(index)
                                .map_or_else(|| "?".into(), PanelValue::display),
                        ),
                        text(
                            confidence
                                .get(index)
                                .map_or_else(|| "?".into(), PanelValue::display),
                        ),
                        text(
                            distances
                                .get(index)
                                .map_or_else(|| "?".into(), PanelValue::display),
                        ),
                    ],
                });
            }
            if ids.is_empty() {
                rows.push(text("No objects in latest perception frame"));
            }
            if ids.len() > 8 {
                rows.push(text(format!("Showing first 8 of {} objects", ids.len())));
            }
            children.push(UiNode::Column { children: rows });
            children.push(text(format!(
                "Perception stages (ms): {}",
                array(tracks, "timings_ms[]")
                    .iter()
                    .map(PanelValue::display)
                    .collect::<Vec<_>>()
                    .join(", ")
            )));
        } else {
            children.push(text(format!(
                "Tracks {:?}: {}",
                tracks.status,
                tracks.errors.join("; ")
            )));
        }
    }
    children.push(UiNode::Button {
        id: "toggle".into(),
        label: "Play / Pause".into(),
    });
    let commands = if update
        .events
        .iter()
        .any(|e| matches!(e,PanelEvent::Click{id}if id=="toggle"))
    {
        vec![HostCommand::TogglePlayback]
    } else {
        vec![]
    };
    write(PanelOutput {
        root: UiNode::Column { children },
        commands,
    })
}
