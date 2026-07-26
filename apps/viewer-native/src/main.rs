mod app;
mod args;
mod graphics;
mod live;
mod session;

use anyhow::Result;
use app::App;
use args::Args;
use std::time::Instant;
use winit::event_loop::{ControlFlow, EventLoop};

fn main() -> Result<()> {
    env_logger::init();
    let args = Args::parse()?;
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App {
        args,
        window: None,
        graphics: None,
        session: None,
        last_frame: Instant::now(),
        error: None,
    };
    event_loop.run_app(&mut app)?;
    Ok(())
}
