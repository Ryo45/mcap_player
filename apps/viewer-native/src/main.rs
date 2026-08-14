mod app;
mod args;
mod bookmarks;
mod diagnostics;
mod graphics;
mod inspection;
mod interaction;
mod live;
mod panels;
mod plot_loader;
mod presentation;
mod preview;
mod session;
mod signal_query;
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
    let workspace = NativeWorkspace::load_bundled_or_fallback(args.layout);
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
