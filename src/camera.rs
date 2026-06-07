use glam::{Mat4, Vec3};
use winit::event::{ElementState, KeyEvent, MouseButton, WindowEvent};
use winit::keyboard::{KeyCode, PhysicalKey};

pub struct Camera {
    pub target: Vec3,  // 注視点 (見る中心)
    pub distance: f32, // 中心からの距離
    pub yaw: f32,      // 横回転 (ラジアン)
    pub pitch: f32,    // 縦回転 (ラジアン)
    pub aspect: f32,   // 画面のアスペクト比
    pub fov: f32,      // 視野角
}

impl Camera {
    pub fn new(width:u32, height:u32) -> Self {
        Self {
            target: Vec3::new(0.0,0.0,0.0),
            distance: 10.0,
            yaw: -std::f32::consts::FRAC_PI_4, // 45度
            pitch: std::f32::consts::FRAC_PI_4, // 45度
            aspect: width as f32 / height as f32,
            fov: 45.0_f32.to_radians(),
        }
    }

    pub fn resize(&mut self, width:u32, height:u32) {
        self.aspect = width as f32 / height as f32;
    }

    pub fn build_view_proj_matrix(&self) -> (Mat4,Mat4) {
        let (sin_pitch, cos_pitch) = self.pitch.sin_cos();
        let (sin_yaw, cos_yaw) = self.yaw.sin_cos();

        let eye_position = self.target + Vec3::new(
            self.distance * cos_pitch * cos_yaw,
            self.distance * cos_pitch * sin_yaw,
            self.distance * sin_pitch
        );
        let view = Mat4::look_at_rh(eye_position, self.target, Vec3::Z);
        let proj = Mat4::perspective_rh(self.fov, self.aspect, 0.1, 1000.0);

        (view,proj)
    }
}

pub struct CameraController {
    is_drag_active :bool,
    last_mouse_pos : Option<(f64,f64)>,
    sensitivity : f32,
    zoom_speed: f32
}

impl CameraController{
    pub fn new() -> Self {
        Self {
            is_drag_active: false,
            last_mouse_pos: None,
            sensitivity: 0.005,
            zoom_speed: 1.0,
        }
    }

    pub fn process_events(&mut self, event: &WindowEvent, camera: &mut Camera) -> bool {
        match event {
            // マウスホイールでズーム
            WindowEvent::MouseWheel { delta, .. } => {
                let scroll_y = match delta {
                    winit::event::MouseScrollDelta::LineDelta(_, y) => *y,
                    winit::event::MouseScrollDelta::PixelDelta(pos) => (pos.y / 10.0) as f32,
                };
                camera.distance -= scroll_y * self.zoom_speed;
                if camera.distance < 0.1 { camera.distance = 0.1; }
                true
            }
            // 左クリックドラッグの開始/終了
            WindowEvent::MouseInput { state, button: MouseButton::Left, .. } => {
                self.is_drag_active = *state == ElementState::Pressed;
                true
            }
            // カーソル移動 (ドラッグ中のみ回転)
            WindowEvent::CursorMoved { position, .. } => {
                let (x, y) = (position.x, position.y);
                if let Some((last_x, last_y)) = self.last_mouse_pos {
                    if self.is_drag_active {
                        let dx = (x - last_x) as f32;
                        let dy = (y - last_y) as f32;

                        // 横回転 (Yaw) - ROS座標系だと逆回転しやすいので符号調整
                        camera.yaw -= dx * self.sensitivity;
                        
                        // 縦回転 (Pitch) - 上下限を制限 (-89度 ~ 89度)
                        camera.pitch += dy * self.sensitivity;
                        camera.pitch = camera.pitch.clamp(
                            -std::f32::consts::FRAC_PI_2 + 0.1, 
                             std::f32::consts::FRAC_PI_2 - 0.1
                        );
                        
                        self.last_mouse_pos = Some((x, y));
                        return true;
                    }
                }
                self.last_mouse_pos = Some((x, y));
                false
            }
            _ => false,
        }
    }
}

