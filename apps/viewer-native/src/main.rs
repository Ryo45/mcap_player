mod app;
mod args;
mod bookmarks;
mod diagnostics;
mod graphics;
mod inspection;
mod inspector_loader;
mod interaction;
#[cfg(any(test, feature = "ros2-live"))]
mod live;
mod numeric_plot_loader;
mod odometry_signals;
mod panels;
mod presentation;
mod preview;
mod query_error;
#[cfg(feature = "scene-extension")]
mod scene_extension;
mod session;
mod signal_view;
mod ui_components;
mod window_query_loader;
mod workspace;

use anyhow::Result;
use app::App;
use args::Args;
use diagnostics::AppDiagnostics;
use presentation::PresentationState;
use preview::PreviewCoordinator;
use std::time::Instant;
use winit::event_loop::{ControlFlow, EventLoop};
use workspace::NativeWorkspace;

fn main() -> Result<()> {
    env_logger::init();
    let args = Args::parse()?;
    let mut workspace = NativeWorkspace::load_bundled_or_fallback(args.layout);
    if let Some(topic) = &args.point_cloud_topic {
        workspace.set_point_cloud_topic(topic.clone());
    }
    #[allow(unused_mut)]
    let mut presentation_state = PresentationState::default();
    #[cfg(feature = "scene-extension")]
    if let Some(manifest) = &args.scene_extension {
        use viewer_extension::VisualizationExtension;
        let extension = viewer_extension::WasmExtension::load(manifest)?;
        workspace.enable_scene_extension(&extension.descriptor().topic)?;
        presentation_state.set_scene_extension(Box::new(extension));
    }
    #[cfg(not(feature = "scene-extension"))]
    debug_assert!(args.scene_extension.is_none());
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        args,
        window: None,
        session: None,
        workspace,
        preview: PreviewCoordinator::default(),
        bookmarks: bookmarks::BookmarkState::default(),
        presentation_state,
        graphics: None,
        last_frame: Instant::now(),
        diagnostics: AppDiagnostics::default(),
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}
