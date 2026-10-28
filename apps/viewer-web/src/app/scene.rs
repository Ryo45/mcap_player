//! Scene/BEV presentation and browser WebGPU lifecycle.
use super::{WebViewState, element};
use crate::{playback::WebPlayback, webgpu::WebGpuHost};
use bev_renderer::BevFrame;
use scene_renderer::{SceneCameraMode, SceneFrame};
use std::cell::RefCell;
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

pub(super) fn update_scene_presentation(
    session: &WebPlayback,
    view: &mut WebViewState,
    platform: Option<&viewer_core::PlatformGeometry>,
    policy: &viewer_core::PathProjectionPolicy,
) {
    let empty = viewer_core::PointCloudState::default();
    let points = session
        .point_cloud()
        .map_or(&empty, |scene| scene.point_cloud());
    let accumulate: HtmlInputElement = element("accumulate-scans");
    let color: HtmlSelectElement = element("point-cloud-color");
    let min: HtmlInputElement = element("intensity-min");
    let max: HtmlInputElement = element("intensity-max");
    let coloring = viewer_core::PointCloudColoring {
        mode: match color.value().as_str() {
            "solid" => viewer_core::PointCloudColorMode::Solid,
            "rgb" => viewer_core::PointCloudColorMode::Rgb,
            "intensity" => viewer_core::PointCloudColorMode::Intensity,
            _ => viewer_core::PointCloudColorMode::Auto,
        },
        intensity_range: [min.value_as_number() as f32, max.value_as_number() as f32],
    };
    let scene = view.scene.build(viewer_core::ScenePresentationInput {
        path: session.path().state(),
        telemetry: session.odometry().state(),
        point_cloud: points,
        transforms: session.transforms().state(),
        platform,
        path_projection: policy,
        accumulate: accumulate.checked(),
        coloring,
    });
    let bev = scene.bev;
    let info: HtmlElement = element("scene-info");
    info.set_inner_text(&format!(
        "Point cloud: {} visible points · {}",
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
    if let Some(error) = scene.diagnostics.point_cloud_color_error {
        info.set_inner_text(&format!("{} · {error}", info.inner_text()));
    }
    for e in [
        &scene.diagnostics.path_tf_error,
        &scene.diagnostics.platform_tf_error,
    ]
    .into_iter()
    .flatten()
    {
        info.set_inner_text(&format!(
            "{} · TF missing: {} → {}",
            info.inner_text(),
            e.source_frame,
            e.target_frame
        ));
    }
    if let Some(error) = scene.diagnostics.platform_geometry_error {
        info.set_inner_text(&format!("{} · {error}", info.inner_text()));
    }
    if let Some(error) = scene.diagnostics.path_geometry_error {
        info.set_inner_text(&format!("{} · {error}", info.inner_text()));
    }
    render_scene(
        BevFrame {
            revision: bev.revision,
            path: bev.path,
            footprint: bev.footprint,
        },
        SceneFrame {
            revision: scene.revision,
            cloud_revision: scene.cloud_revision,
            overlay_revision: scene.overlay_revision,
            overlay: scene.overlay,
            focus: scene.focus,
            forward: scene.forward,
            view_radius: scene.view_radius,
            platform: scene.platform,
            path: scene.path,
            cloud: scene.cloud,
            cloud_colors: scene.cloud_colors,
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
