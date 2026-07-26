use viewer_core::CameraId;

#[derive(Clone, Debug, Default)]
pub(crate) struct ViewerSettings {
    pub(crate) focused_camera: Option<CameraId>,
    pub(crate) accumulate_points: bool,
}
