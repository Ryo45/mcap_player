//! Borrowed inputs for one frame. Hosts retain sources, queries and GPU resources.
use crate::inspector::InspectionView;
use crate::views::camera::CameraTextureView;
use scene_renderer::SceneCameraMode;
use viewer_core::{
    ArrivalTime, Bookmark, LoadedSignal, PlaybackView, SceneDiagnostics, SignalId, SignalOverview,
    SignalSample, TopicTree, ViewerPresentation,
};

#[derive(Clone, Copy)]
pub struct PanelResourceView<'a> {
    pub camera_textures: &'a [CameraTextureView],
    pub preview_camera_textures: &'a [CameraTextureView],
    pub bev_texture: egui::TextureId,
    pub scene_texture: egui::TextureId,
}
#[derive(Clone, Copy)]
pub struct PreviewDataView<'a> {
    pub active: bool,
    pub speed_overview: Option<&'a SignalOverview>,
    pub bookmarks: &'a [Bookmark],
}
#[derive(Clone, Copy)]
pub struct SceneDataView<'a> {
    pub diagnostics: &'a SceneDiagnostics,
    pub visible_scan_points: usize,
    pub camera_distance: f32,
    pub camera_mode: SceneCameraMode,
    pub static_transform_count: usize,
    pub dynamic_transform_count: usize,
}
#[derive(Clone, Copy, Default)]
pub struct SignalInput<'a> {
    pub signal: Option<&'a LoadedSignal>,
    pub current: Option<SignalSample>,
    pub loading: bool,
    pub error: Option<&'a dyn std::fmt::Display>,
}
#[derive(Clone, Copy, Default)]
pub struct SignalInputs<'a> {
    pub speed: SignalInput<'a>,
    pub yaw_rate: SignalInput<'a>,
}
impl<'a> SignalInputs<'a> {
    pub fn get(self, id: SignalId) -> SignalInput<'a> {
        match id {
            SignalId::Speed => self.speed,
            SignalId::YawRate => self.yaw_rate,
        }
    }
}

pub struct PanelInput<'a> {
    pub playback: Option<PlaybackView>,
    pub display_time: Option<ArrivalTime>,
    pub presentation: &'a ViewerPresentation,
    pub camera_overlays: &'a viewer_renderer::CameraOverlayState,
    pub signals: SignalInputs<'a>,
    pub preview: PreviewDataView<'a>,
    pub resources: PanelResourceView<'a>,
    pub scene: Option<SceneDataView<'a>>,
    pub inspections: &'a [InspectionView<'a>],
    pub topic_inspections: &'a [InspectionView<'a>],
    pub inspector_topic: Option<&'a str>,
    pub topic_tree: &'a TopicTree,
    /// None disables the host's optional numeric-field query capability.
    pub numeric_series: Option<&'a [(viewer_core::StreamId, &'a str)]>,
}
