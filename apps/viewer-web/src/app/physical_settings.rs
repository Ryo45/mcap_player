//! Independent physical geometry/calibration inputs and path placement controls.
use super::{APP, element};
use std::cell::Cell;
thread_local! { static GEOMETRY_GENERATION: Cell<u64> = const { Cell::new(0) }; static CALIBRATION_GENERATION: Cell<u64> = const { Cell::new(0) }; }
use wasm_bindgen::{JsCast, closure::Closure};
use wasm_bindgen_futures::{JsFuture, spawn_local};
use web_sys::{Event, HtmlElement, HtmlInputElement};

pub(super) fn install_controls() {
    let input: HtmlInputElement = element("platform-geometry-file");
    let callback = Closure::<dyn FnMut(Event)>::new(|e: Event| {
        let Some(file) = e
            .target()
            .and_then(|t| t.dyn_into::<HtmlInputElement>().ok())
            .and_then(|i| i.files())
            .and_then(|f| f.get(0))
        else {
            return;
        };
        let generation = GEOMETRY_GENERATION.with(|g| {
            let n = g.get().wrapping_add(1);
            g.set(n);
            n
        });
        spawn_local(async move {
            if file.size() > 64.0 * 1024.0 {
                show_error("Platform geometry JSON exceeds 64 KiB");
                return;
            }
            let result = JsFuture::from(file.text())
                .await
                .ok()
                .and_then(|v| v.as_string())
                .ok_or_else(|| "Unable to read platform geometry".to_owned())
                .and_then(|json| {
                    serde_json::from_str::<viewer_core::PlatformGeometry>(&json)
                        .map_err(|e| e.to_string())
                });
            if !GEOMETRY_GENERATION.with(|g| g.get() == generation) {
                return;
            }
            match result {
                Ok(g) => {
                    APP.with(|a| {
                        let mut a = a.borrow_mut();
                        a.platform = Some(g);
                        a.platform_enabled = true;
                    });
                    let enabled: HtmlInputElement = element("platform-enabled");
                    enabled.set_checked(true);
                    show_error("");
                    update_label();
                }
                Err(e) => show_error(&e),
            }
        });
    });
    input.set_onchange(Some(callback.as_ref().unchecked_ref()));
    callback.forget();
    let enabled: HtmlInputElement = element("platform-enabled");
    let callback = Closure::<dyn FnMut(Event)>::new(|_: Event| {
        let enabled: HtmlInputElement = element("platform-enabled");
        APP.with(|a| {
            a.borrow_mut().platform_enabled = enabled.checked();
        });
        update_label();
    });
    enabled.set_onchange(Some(callback.as_ref().unchecked_ref()));
    callback.forget();
    for id in ["ground-frame", "fixed-frame"] {
        let input: HtmlInputElement = element(id);
        let callback = Closure::<dyn FnMut(Event)>::new(|_: Event| {
            let fixed: HtmlInputElement = element("fixed-frame");
            let ground: HtmlInputElement = element("ground-frame");
            if fixed.value().trim().is_empty()
                || fixed.value().len() > 256
                || ground.value().len() > 256
            {
                show_error("World frame must be named");
                return;
            }
            APP.with(|a| {
                let mut a = a.borrow_mut();
                a.path_projection = viewer_core::PathProjectionPolicy {
                    fixed_frame: fixed.value().trim().into(),
                    ground_frame: (!ground.value().trim().is_empty())
                        .then(|| ground.value().trim().into()),
                };
            });
            update_label();
        });
        input.set_onchange(Some(callback.as_ref().unchecked_ref()));
        callback.forget();
    }
    let calibration: HtmlInputElement = element("camera-calibration-file");
    let callback = Closure::<dyn FnMut(Event)>::new(|e: Event| {
        let Some(file) = e
            .target()
            .and_then(|t| t.dyn_into::<HtmlInputElement>().ok())
            .and_then(|i| i.files())
            .and_then(|f| f.get(0))
        else {
            return;
        };
        let generation = CALIBRATION_GENERATION.with(|g| {
            let n = g.get().wrapping_add(1);
            g.set(n);
            n
        });
        spawn_local(async move {
            if file.size() > 64.0 * 1024.0 {
                show_error("Camera calibration JSON exceeds 64 KiB");
                return;
            }
            let result = JsFuture::from(file.text())
                .await
                .ok()
                .and_then(|v| v.as_string())
                .ok_or_else(|| "Unable to read camera calibration".to_owned())
                .and_then(|json| {
                    viewer_core::CameraCalibrationSet::from_json(&json).map_err(|e| e.to_string())
                });
            if !CALIBRATION_GENERATION.with(|g| g.get() == generation) {
                return;
            }
            match result {
                Ok(c) => {
                    APP.with(|a| {
                        let mut a = a.borrow_mut();
                        a.calibrations = c;
                        a.view.camera.camera_overlays.reset_source();
                    });
                    show_error("");
                    update_label();
                }
                Err(e) => show_error(&e),
            }
        });
    });
    calibration.set_onchange(Some(callback.as_ref().unchecked_ref()));
    callback.forget();
    update_label();
}
fn update_label() {
    let label: HtmlElement = element("platform-geometry-label");
    APP.with(|a| {
        let a = a.borrow();
        let shape = if a.platform_enabled {
            a.platform
                .as_ref()
                .map_or("No physical bounds".into(), |g| {
                    let e = g.extent();
                    format!(
                        "{} · {:.3} × {:.3} × {:.3} m",
                        g.reference_frame(),
                        e[0],
                        e[1],
                        e[2]
                    )
                })
        } else {
            "Physical body hidden".into()
        };
        let path = a
            .path_projection
            .ground_frame
            .as_ref()
            .map_or("Path: original xyz".into(), |g| {
                format!("Path: ground plane in {g}")
            });
        label.set_inner_text(&format!(
            "{shape} · {path} · {}",
            a.calibrations.provenance()
        ));
    });
}

fn show_error(message: &str) {
    let e: HtmlElement = element("physical-settings-error");
    e.set_inner_text(message);
}
