mod app;
mod args;
mod bookmarks;
mod graphics;
mod interaction;
mod live;
mod panels;
mod plot_loader;
mod presentation;
mod preview;
mod session;
mod workspace;

use anyhow::Result;
use app::App;
use args::Args;
use plot_loader::PlotLoader;
use presentation::PresentationState;
use preview::PreviewCoordinator;
use std::time::Instant;
use winit::event_loop::{ControlFlow, EventLoop};
use workspace::NativeWorkspace;

fn main() -> Result<()> {
    env_logger::init();
    let args = Args::parse()?;
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        args,
        window: None,
        session: None,
        workspace: NativeWorkspace::load_bundled_or_fallback(),
        plot_loader: PlotLoader::default(),
        preview: PreviewCoordinator::default(),
        bookmarks: bookmarks::BookmarkState::default(),
        presentation_state: PresentationState::default(),
        graphics: None,
        last_frame: Instant::now(),
        error: None,
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}
