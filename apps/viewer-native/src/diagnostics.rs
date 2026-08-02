#[derive(Default)]
pub(crate) struct AppDiagnostics {
    playback_error: Option<String>,
    presentation_error: Option<String>,
    sidecar_warnings: Vec<String>,
}

impl AppDiagnostics {
    pub(crate) fn reset_for_source(&mut self) {
        self.playback_error = None;
        self.presentation_error = None;
        self.sidecar_warnings.clear();
    }

    pub(crate) fn set_playback_error(&mut self, error: impl Into<String>) {
        self.playback_error = Some(error.into());
    }

    pub(crate) fn set_presentation_error(&mut self, error: impl Into<String>) {
        self.presentation_error = Some(error.into());
    }

    pub(crate) fn add_sidecar_warning(&mut self, warning: impl Into<String>) {
        self.sidecar_warnings.push(warning.into());
    }

    pub(crate) fn message(&self, current_warnings: &[Option<&str>]) -> Option<String> {
        let mut messages = Vec::new();
        if let Some(error) = &self.playback_error {
            messages.push(format!("Playback: {error}"));
        }
        if let Some(error) = &self.presentation_error {
            messages.push(format!("Presentation: {error}"));
        }
        messages.extend(self.sidecar_warnings.iter().cloned());
        messages.extend(
            current_warnings
                .iter()
                .filter_map(|warning| warning.map(str::to_owned)),
        );
        (!messages.is_empty()).then(|| messages.join("; "))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagnostics_keep_playback_presentation_and_sidecar_failures_distinct() {
        let mut diagnostics = AppDiagnostics::default();
        diagnostics.set_playback_error("source read failed");
        diagnostics.set_presentation_error("camera upload failed");
        diagnostics.add_sidecar_warning("Preview unavailable");

        assert_eq!(
            diagnostics.message(&[Some("Bookmarks ignored")]).as_deref(),
            Some(
                "Playback: source read failed; Presentation: camera upload failed; Preview unavailable; Bookmarks ignored"
            )
        );

        diagnostics.reset_for_source();
        assert!(diagnostics.message(&[]).is_none());
    }
}
