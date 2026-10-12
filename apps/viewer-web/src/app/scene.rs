//! Scene/BEV presentation and browser WebGPU lifecycle.
use super::{WebViewState, element};
use crate::{playback::WebPlayback, webgpu::WebGpuHost};
use bev_renderer::BevFrame;
use scene_renderer::{SceneCameraMode, SceneFrame};
use std::cell::RefCell;
use viewer_core::BevFrameBuilder;
use wasm_bindgen::{JsCast, closure::Closure};
use wasm_bindgen_futures::spawn_local;
use web_sys::{
    Event, HtmlButtonElement, HtmlCanvasElement, HtmlElement, HtmlInputElement, HtmlSelectElement,
};
enum WebGpuState {
    Initializing,
    Ready(Box<WebGpuHost>),
    Unavailable,
}
thread_local! { static WEBGPU: RefCell<WebGpuState> = const {
    RefCell::new(WebGpuState::Initializing)
}; }
fn set_bev_status(message: &str, error: bool) {
    let status: HtmlElement = element("bev-status");
    status.set_inner_text(message);
    status.set_class_name(if error { "error" } else { "" });
}

pub(super) fn initialize_webgpu() {
    let canvas: HtmlCanvasElement = element("bev");
    set_bev_status("WebGPU: initializing", false);
    spawn_local(async move {
        match WebGpuHost::new(canvas).await {
            Ok(host) => {
                WEBGPU.with(|state| *state.borrow_mut() = WebGpuState::Ready(Box::new(host)));
                set_bev_status("WebGPU ready", false);
            }
            Err(error) => {
                WEBGPU.with(|state| *state.borrow_mut() = WebGpuState::Unavailable);
                set_bev_status(&format!("WebGPU unavailable: {error}"), true);
            }
        }
    });
}

pub(super) fn invalidate_gpu() {
    WEBGPU.with(|state| {
        if let WebGpuState::Ready(host) = &mut *state.borrow_mut() {
            host.invalidate();
        }
    });
}

pub(super) fn render_scene(bev: BevFrame<'_>, scene: SceneFrame<'_>) {
    let mode: HtmlSelectElement = element("view-mode");
    let error = WEBGPU.with(|state| {
        let mut state = state.borrow_mut();
        match &mut *state {
            WebGpuState::Ready(host) => host.render(bev, scene, mode.value() == "bev").err(),
            WebGpuState::Initializing | WebGpuState::Unavailable => None,
        }
    });
    if let Some(error) = error {
        set_bev_status(&format!("WebGPU stopped: {error}"), true);
        WEBGPU.with(|state| *state.borrow_mut() = WebGpuState::Unavailable);
    }
}

pub(super) fn update_scene_presentation(session: &WebPlayback, view: &mut WebViewState) {
    let bev = BevFrameBuilder::new(session.path().state()).build();
    let empty = viewer_core::PointCloudState::default();
    let points = session
        .point_cloud()
        .map_or(&empty, |scene| scene.point_cloud());
    let accumulate: HtmlInputElement = element("accumulate-scans");
    let scene = view.scene.build(
        session.path().state(),
        session.odometry().state(),
        points,
        session.transforms().state(),
        accumulate.checked(),
    );
    let info: HtmlElement = element("scene-info");
    info.set_inner_text(&format!(
        "Scan: {} visible points · {}",
        scene.cloud.len(),
        scene.diagnostics.current_tf_error.as_ref().map_or_else(
            || scene
                .diagnostics
                .last_tf_route
                .clone()
                .unwrap_or_else(|| "TF: waiting".into()),
            |error| format!(
                "TF missing: {} → {}",
                error.source_frame, error.target_frame
            )
        )
    ));
    render_scene(
        BevFrame {
            revision: bev.revision,
            path: bev.path,
        },
        SceneFrame {
            revision: scene.revision,
            cloud_revision: scene.cloud_revision,
            ego_position: scene.ego_position,
            ego_yaw: scene.ego_yaw,
            path: scene.path,
            cloud: scene.cloud,
        },
    );
}

pub(super) fn install_controls() {
    let canvas: HtmlCanvasElement = element("bev");
    let callback = Closure::<dyn FnMut(web_sys::WheelEvent)>::new(|event: web_sys::WheelEvent| {
        event.prevent_default();
        WEBGPU.with(|state| {
            if let WebGpuState::Ready(host) = &mut *state.borrow_mut() {
                host.scene_renderer.zoom(-event.delta_y() as f32);
            }
        });
    });
    canvas.set_onwheel(Some(callback.as_ref().unchecked_ref()));
    callback.forget();
    let callback = Closure::<dyn FnMut(web_sys::MouseEvent)>::new(|event: web_sys::MouseEvent| {
        if event.buttons() & 1 == 0 {
            return;
        }
        WEBGPU.with(|state| {
            if let WebGpuState::Ready(host) = &mut *state.borrow_mut() {
                host.scene_renderer
                    .orbit(event.movement_x() as f32, event.movement_y() as f32);
            }
        });
    });
    canvas.set_onmousemove(Some(callback.as_ref().unchecked_ref()));
    callback.forget();
    let mode: HtmlSelectElement = element("scene-camera-mode");
    let callback = Closure::<dyn FnMut(Event)>::new(|_: Event| {
        let mode: HtmlSelectElement = element("scene-camera-mode");
        let mode = match mode.value().as_str() {
            "free" => SceneCameraMode::Free,
            "eye" => SceneCameraMode::VehicleEye,
            _ => SceneCameraMode::Chase,
        };
        WEBGPU.with(|state| {
            if let WebGpuState::Ready(host) = &mut *state.borrow_mut() {
                host.scene_renderer.set_camera_mode(mode);
            }
        });
    });
    mode.set_onchange(Some(callback.as_ref().unchecked_ref()));
    callback.forget();
    let reset: HtmlButtonElement = element("reset-scene");
    let callback = Closure::<dyn FnMut()>::new(|| {
        WEBGPU.with(|state| {
            if let WebGpuState::Ready(host) = &mut *state.borrow_mut() {
                host.scene_renderer.reset_camera();
            }
        })
    });
    reset.set_onclick(Some(callback.as_ref().unchecked_ref()));
    callback.forget();
}
