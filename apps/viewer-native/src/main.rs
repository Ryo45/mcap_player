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
mod session;
mod signal_view;
mod ui_components;
mod window_query_loader;
mod workspace;
mod workspace_bindings;

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
    let bindings = workspace_bindings::load(
        args.workspace_bindings.as_deref(),
        args.point_cloud_topic.as_deref(),
    )?;
    workspace.set_bindings(bindings);
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        args,
        window: None,
        session: None,
        workspace,
        preview: PreviewCoordinator::default(),
        bookmarks: bookmarks::BookmarkState::default(),
        presentation_state: PresentationState::default(),
        graphics: None,
        last_frame: Instant::now(),
        diagnostics: AppDiagnostics::default(),
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}
