use winit::window::Window;
use winit::event::WindowEvent;
use wgpu::{Device, Queue, TextureFormat};
use egui::{Context,FullOutput};
use egui_wgpu::Renderer as EguiRenderer;
use egui_wgpu::ScreenDescriptor;
use egui_winit::State;

pub struct Gui {
    pub context: Context,
    state: egui_winit::State,
    renderer: EguiRenderer,

    // アプリケーションの状態変数をここに持つ
    // (本来はAppStatusなどに分離しますが、今はここに置きます)
    pub show_grid: bool,
    pub point_size: f32,
}

impl Gui {
    pub fn new(
        window: &Window,
        device: &Device,
        output_format: TextureFormat,
     ) -> Self {
        let context = Context::default();

        let id = context.viewport_id();
        let state = egui_winit::State::new(
            context.clone(),
            id,
            &window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );

        let renderer_options = egui_wgpu::RendererOptions{msaa_samples: 1,
            depth_stencil_format: None,
            dithering: true,
            predictable_texture_filtering: false};
        let renderer = EguiRenderer::new(
            device,
            output_format,
            renderer_options,
        );

        Self {
            context,
            state,
            renderer,
            show_grid:true,
            point_size:0.1,
        }
    }

    pub fn handle_event(&mut self, window: &Window, event: &WindowEvent) -> egui_winit::EventResponse {
        // eguiがイベントを消費した(ボタンを押した等)ならtrueを返す
        self.state.on_window_event(window, event)
    }

    pub fn prepare(
        &mut self,
        window: &Window,
        fps: f32, // デバッグ表示用
    ) -> FullOutput {
        let raw_input = self.state.take_egui_input(window);
        
        self.context.run(raw_input, |ctx| {
            // ここにUIの見た目を書く
            egui::Window::new("Control Panel")
                .default_open(true)
                .show(ctx, |ui| {
                    ui.heading("Settings");
                    ui.separator();
                    
                    ui.checkbox(&mut self.show_grid, "Show Grid");
                    
                    ui.add(egui::Slider::new(&mut self.point_size, 0.01..=1.0).text("Point Size"));
                    
                    ui.separator();
                    ui.label(format!("FPS: {:.1}", fps));
                    ui.label(format!("show_grid: {}", self.show_grid));
                    ui.label(format!("Mouse: {:?}", ui.input(|i| i.pointer.hover_pos())));
                    ui.label("Step 3: UI Integration Complete!");
                });
        })
    }

    // 描画コマンドの発行
    pub fn render(
        &mut self,
        device: &Device,
        queue: &Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        screen_descriptor: &ScreenDescriptor,
        output: FullOutput,
        window: &Window,
    ) {
        self.state.handle_platform_output(window, output.platform_output);

        let tris = self.context.tessellate(output.shapes, output.pixels_per_point);

        for (id, image_delta) in &output.textures_delta.set {
            self.renderer.update_texture(device, queue, *id, image_delta);
        }

        // コマンドエンコーダへの書き込み
        self.renderer.update_buffers(device, queue, encoder, &tris, screen_descriptor);

        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("Egui Render Pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                resolve_target: None,
                ops: wgpu::Operations {
                    // 重要: ClearではなくLoadにして、3D描画の上に重ねる
                    load: wgpu::LoadOp::Load, 
                    store: wgpu::StoreOp::Store,
                },
                depth_slice:None,
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
        }).forget_lifetime();

        self.renderer.render(&mut rpass, &tris, screen_descriptor);
        
        // テクスチャの解放処理
        for id in &output.textures_delta.free {
            self.renderer.free_texture(id);
        }
    }
}
