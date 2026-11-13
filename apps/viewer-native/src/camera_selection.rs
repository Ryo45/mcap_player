use std::collections::BTreeMap;
use viewer_layout::PanelId;
/// Small prospective bindings, staged separately from the active decoded runtime.
pub(crate) struct CameraSelection {
    pub(crate) available_topics: Vec<String>,
    pub(crate) panels: BTreeMap<PanelId, Option<String>>,
}

impl CameraSelection {
    pub(crate) fn priority_topic<'a>(&'a self, preferred: &'a str) -> &'a str {
        self.panels
            .values()
            .flatten()
            .find(|topic| topic.as_str() == preferred)
            .or_else(|| self.panels.values().flatten().next())
            .map_or(preferred, String::as_str)
    }
}
