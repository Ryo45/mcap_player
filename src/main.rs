mod renderer;
mod camera;
mod ui;
mod input;
mod player;
mod transform;

use std::sync::Arc;
use std::sync::mpsc::Receiver;
use std::time::Instant;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use renderer::Renderer;
use camera::{Camera, CameraController}; // 追加
use input::{LoadedPacket, start_input_thread};
use player::PlayerState;

struct McapPlayerApp {
    window: Option<Arc<Window>>,
    renderer: Option<Renderer>,
    // カメラの状態と操作コントローラーを持つ
    camera: Option<Camera>,
    camera_controller: CameraController,
    data_receiver: Option<Receiver<LoadedPacket>>,
    player_state: PlayerState,
    last_frame: Instant,
}

impl McapPlayerApp {
    fn new() -> Self {
        Self {
            window: None,
            renderer: None,
            camera: None,
            camera_controller: CameraController::new(),
            data_receiver: None,
            player_state: PlayerState::new(),
            last_frame: Instant::now(),
        }
    }
}

impl ApplicationHandler for McapPlayerApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_none() {
            // 初期化処理
            let window_attributes = Window::default_attributes()
                .with_title("MCAP Player - Grid & Orbit")
                .with_inner_size(winit::dpi::LogicalSize::new(1024.0, 768.0));
            
            let window = Arc::new(event_loop.create_window(window_attributes).unwrap());
            self.window = Some(window.clone());
            
            let renderer = pollster::block_on(Renderer::new(window.clone()));
            // カメラもここでウィンドウサイズに合わせて初期化
            let size = window.inner_size();
            self.camera = Some(Camera::new(size.width, size.height));
            
            // データ関係
            let rx = start_input_thread("mcap/hku1_0.mcap");
            // let rx = start_input_thread("mcap/nissan_zala_50_zeg_4_0.mcap");

            self.data_receiver = Some(rx);
            self.last_frame = Instant::now();

            self.renderer = Some(renderer);
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
 
        let window = self.window.as_ref().unwrap();

        // 1. UIにイベントを流し、応答を受け取る
        let mut ui_response = egui_winit::EventResponse {
            consumed: false,
            repaint: false,
        };

        if let Some(renderer) = &mut self.renderer {
            ui_response = renderer.handle_event(window, &event);
        }

        if ui_response.repaint {
            window.request_redraw();
        }

        // 2. UIがイベントを消費しなかった場合のみ、カメラ操作を行う
        if !ui_response.consumed {
            if let Some(camera) = &mut self.camera {
                if self.camera_controller.process_events(&event, camera) {
                    // カメラが動いた場合も再描画リクエスト
                    window.request_redraw();
                }
            }
        }

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(physical_size) => {
                if let Some(renderer) = &mut self.renderer {
                    renderer.resize(physical_size.width, physical_size.height);
                }
                if let Some(camera) = &mut self.camera {
                    camera.resize(physical_size.width, physical_size.height);
                }
                // リサイズ時も必ず再描画
                window.request_redraw();
            }
            WindowEvent::RedrawRequested => {
                // ここで描画処理
                // データ更新
                let now = Instant::now();
                let dt = now.duration_since(self.last_frame).as_secs_f32();
                print!("delta_t:{:.6}",dt);
                self.last_frame = now;
                if let Some(rx) = &self.data_receiver {
                    
                    let packets = self.player_state.update(dt,rx);
                    for packet in packets {
                        if packet.topic.contains("lidar") {
                            if let Some(renderer) = &mut self.renderer {
                                renderer.update_lidar(&packet);
                           }
                        }
                        println!("Got packet: time={} topic={} length={}", packet.log_time, packet.topic, packet.data.len());
                    }
                }
                if let Some(renderer) = &mut self.renderer {
                    if let Some(camera) = &self.camera {
                        match renderer.render(window, camera) {
                            Ok(_) => {}
                            Err(wgpu::SurfaceError::Lost) => renderer.resize(0, 0),
                            Err(wgpu::SurfaceError::OutOfMemory) => event_loop.exit(),
                            Err(e) => eprintln!("{:?}", e),
                        }
                    }
                }
                // window.request_redraw();

            }
            _ => {}
        }
    }

}

fn main() {
    env_logger::init();
    let event_loop = EventLoop::new().unwrap();
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = McapPlayerApp::new();
    event_loop.run_app(&mut app).unwrap();
}