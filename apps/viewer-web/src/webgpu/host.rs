use super::{PhysicalSize, physical_size, presenter::WebTexturePresenter};
use bev_renderer::{BevFrame, BevRenderer};
use web_sys::HtmlCanvasElement;

pub(crate) struct WebGpuHost {
    _instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    _adapter: wgpu::Adapter,
    device: wgpu::Device,
    queue: wgpu::Queue,
    surface_config: wgpu::SurfaceConfiguration,
    canvas: HtmlCanvasElement,
    bev_renderer: BevRenderer,
    presenter: WebTexturePresenter,
    physical_size: Option<PhysicalSize>,
}

impl WebGpuHost {
    pub(crate) async fn new(canvas: HtmlCanvasElement) -> Result<Self, String> {
        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor {
            backends: wgpu::Backends::BROWSER_WEBGPU,
            ..Default::default()
        });
        let surface = instance
            .create_surface(wgpu::SurfaceTarget::Canvas(canvas.clone()))
            .map_err(|error| format!("create WebGPU canvas surface: {error}"))?;
        let adapter = instance
            .request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::HighPerformance,
                compatible_surface: Some(&surface),
                force_fallback_adapter: false,
            })
            .await
            .map_err(|error| format!("request WebGPU adapter: {error}"))?;
        let (device, queue) = adapter
            .request_device(&wgpu::DeviceDescriptor {
                label: Some("viewer-web WebGPU device"),
                required_features: wgpu::Features::empty(),
                experimental_features: wgpu::ExperimentalFeatures::disabled(),
                required_limits: wgpu::Limits::default(),
                memory_hints: wgpu::MemoryHints::default(),
                trace: wgpu::Trace::Off,
            })
            .await
            .map_err(|error| format!("request WebGPU device: {error}"))?;
        let capabilities = surface.get_capabilities(&adapter);
        let surface_format = capabilities
            .formats
            .iter()
            .copied()
            .find(wgpu::TextureFormat::is_srgb)
            .or_else(|| capabilities.formats.first().copied())
            .ok_or_else(|| "WebGPU canvas surface has no supported format".to_owned())?;
        let present_mode = capabilities
            .present_modes
            .iter()
            .copied()
            .find(|mode| *mode == wgpu::PresentMode::Fifo)
            .or_else(|| capabilities.present_modes.first().copied())
            .ok_or_else(|| "WebGPU canvas surface has no present mode".to_owned())?;
        let alpha_mode = capabilities
            .alpha_modes
            .first()
            .copied()
            .unwrap_or(wgpu::CompositeAlphaMode::Auto);
        let initial = current_physical_size(&canvas).unwrap_or(PhysicalSize {
            width: 1,
            height: 1,
        });
        let surface_config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format: surface_format,
            width: initial.width,
            height: initial.height,
            present_mode,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: vec![],
        };
        let bev_renderer = BevRenderer::new(&device, initial.width, initial.height);
        let presenter = WebTexturePresenter::new(&device, surface_format, bev_renderer.view());
        let mut host = Self {
            _instance: instance,
            surface,
            _adapter: adapter,
            device,
            queue,
            surface_config,
            canvas,
            bev_renderer,
            presenter,
            physical_size: None,
        };
        let synthetic = [[0.0, 0.0], [0.0, 4.0], [2.0, 7.0]];
        host.render(BevFrame {
            revision: u64::MAX,
            path: &synthetic,
        })?;
        Ok(host)
    }

    pub(crate) fn render(&mut self, frame: BevFrame<'_>) -> Result<(), String> {
        let Some(size) = current_physical_size(&self.canvas) else {
            return Ok(());
        };
        let resized = self.physical_size != Some(size);
        if resized {
            self.canvas.set_width(size.width);
            self.canvas.set_height(size.height);
            self.surface_config.width = size.width;
            self.surface_config.height = size.height;
            self.surface.configure(&self.device, &self.surface_config);
            if self
                .bev_renderer
                .resize(&self.device, size.width, size.height)
            {
                self.presenter
                    .set_source(&self.device, self.bev_renderer.view());
            }
            self.physical_size = Some(size);
        }
        if resized || self.bev_renderer.needs_render(frame) {
            self.bev_renderer.render(&self.device, &self.queue, frame);
        }
        let surface_texture = match self.surface.get_current_texture() {
            Ok(texture) => texture,
            Err(wgpu::SurfaceError::Lost | wgpu::SurfaceError::Outdated) => {
                self.surface.configure(&self.device, &self.surface_config);
                return Ok(());
            }
            Err(wgpu::SurfaceError::Timeout) => return Ok(()),
            Err(wgpu::SurfaceError::OutOfMemory) => {
                return Err("WebGPU canvas surface is out of memory".to_owned());
            }
            Err(wgpu::SurfaceError::Other) => {
                return Err("WebGPU canvas surface failed".to_owned());
            }
        };
        let target = surface_texture
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.presenter.present(&self.device, &self.queue, &target);
        surface_texture.present();
        Ok(())
    }
}

fn current_physical_size(canvas: &HtmlCanvasElement) -> Option<PhysicalSize> {
    let dpr = web_sys::window()?.device_pixel_ratio();
    physical_size(canvas.client_width(), canvas.client_height(), dpr)
}
