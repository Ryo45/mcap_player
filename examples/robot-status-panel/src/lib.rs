//! Standalone Core Wasm guest. ROS/CDR and egui belong to the host.
use viewer_extension::panel::{
    HostCommand, InputStatus, MAX_OUTPUT_BYTES, MAX_UPDATE_BYTES, PanelDefinition, PanelEvent,
    PanelOutput, PanelUpdate, UiNode,
};
static mut INPUT: [u8; MAX_UPDATE_BYTES] = [0; MAX_UPDATE_BYTES];
static mut OUTPUT: [u8; MAX_OUTPUT_BYTES] = [0; MAX_OUTPUT_BYTES];
#[unsafe(no_mangle)]
pub extern "C" fn panel_api_version() -> u32 {
    1
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
    let Some(input) = input(length) else {
        return u32::MAX;
    };
    let Ok(settings) = serde_json::from_slice::<serde_json::Value>(input) else {
        return u32::MAX;
    };
    write(PanelDefinition {
        api_version: 1,
        id: "robot-status".into(),
        title: "Robot status".into(),
        topic: settings["topic"].as_str().unwrap_or("/robot/status").into(),
        fields: ["battery", "temperature", "state", "sequence", "enabled"]
            .map(String::from)
            .to_vec(),
        plots: ["battery", "temperature"].map(String::from).to_vec(),
        history_seconds: 10,
        max_points: 128,
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn update(length: u32) -> u32 {
    let Some(input) = input(length) else {
        return u32::MAX;
    };
    let Ok(update) = serde_json::from_slice::<PanelUpdate>(input) else {
        return u32::MAX;
    };
    let root = if update.status == InputStatus::Ready {
        let heading = if cfg!(feature = "alternate") {
            "Robot telemetry — alternate guest"
        } else {
            "Robot telemetry — Wasm guest"
        };
        let values = [
            ("battery", "Battery", "%"),
            ("temperature", "Temperature", "°C"),
            ("state", "State", ""),
            ("sequence", "Sequence", ""),
            ("enabled", "Enabled", ""),
        ]
        .into_iter()
        .map(|(key, label, unit)| UiNode::Value {
            label: label.into(),
            value: update
                .values
                .get(key)
                .map(|v| v.display())
                .unwrap_or_else(|| "missing".into()),
            unit: unit.into(),
        })
        .collect();
        UiNode::Column {
            children: vec![
                UiNode::Text {
                    text: heading.into(),
                },
                UiNode::Row { children: values },
                UiNode::Plot {
                    title: "Battery / temperature history".into(),
                    series: vec!["battery".into(), "temperature".into()],
                },
                UiNode::Text {
                    text: format!(
                        "{} reduced samples",
                        update.histories.values().map(Vec::len).sum::<usize>()
                    ),
                },
                UiNode::Button {
                    id: "toggle".into(),
                    label: "Play / Pause".into(),
                },
            ],
        }
    } else {
        UiNode::Text {
            text: format!("Input {:?}: {}", update.status, update.errors.join("; ")),
        }
    };
    let commands = if update
        .events
        .iter()
        .any(|e| matches!(e,PanelEvent::Click{id} if id=="toggle"))
    {
        vec![HostCommand::TogglePlayback]
    } else {
        vec![]
    };
    write(PanelOutput { root, commands })
}
