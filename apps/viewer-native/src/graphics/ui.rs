use super::Graphics;
use crate::session::PlaybackSession;
use scene_renderer::SceneCameraMode;
use std::sync::Arc;
use viewer_ui::{playback_controls, source_status};
use winit::window::Window;

pub(super) struct UiOutput {
    pub(super) egui: egui::FullOutput,
    pub(super) seeked: bool,
    pub(super) bev_size: egui::Vec2,
    pub(super) scene_size: egui::Vec2,
    pub(super) accumulate_points: bool,
    pub(super) scene_wheel_delta: f32,
    pub(super) scene_orbit_delta: egui::Vec2,
    pub(super) reset_scene_camera: bool,
    pub(super) scene_camera_mode: SceneCameraMode,
}

impl Graphics {
    pub(super) fn build_ui(
        &mut self,
        window: &Window,
        session: &mut PlaybackSession,
        error: Option<String>,
    ) -> UiOutput {
        let input = self.egui_state.take_egui_input(window);
        self.sync_camera_catalog(session);
        let camera_topics = Arc::clone(&self.camera_topics);
        let mut camera_ids = camera_topics.iter().map(|(id, _)| *id).collect::<Vec<_>>();
        camera_ids.extend(session.state().camera.ids());
        camera_ids.sort_unstable();
        camera_ids.dedup();
        let mut focused_camera = self
            .focused_camera
            .filter(|camera_id| camera_ids.contains(camera_id))
            .or_else(|| camera_ids.first().copied());
        let model = session.presentation(
            error,
            focused_camera,
            self.presentation_metrics.snapshot().clone(),
            &self.overlay_status,
        );
        let focused_texture = focused_camera.and_then(|camera_id| self.camera_texture(camera_id));
        let camera_cards = camera_ids
            .iter()
            .filter_map(|camera_id| {
                self.camera_texture(*camera_id)
                    .map(|texture| (*camera_id, texture))
            })
            .collect::<Vec<_>>();
        let focused_label = model
            .focused_camera()
            .map_or("waiting", |camera| camera.topic.as_str());
        let focused_overlay = model.focused_camera().map_or_else(
            || "overlay waiting".to_owned(),
            |camera| camera.overlay.to_string(),
        );
        let bev_texture_id = self.bev_texture_id;
        let scene_texture_id = self.scene_texture_id;
        let bev_path_points = model.diagnostics.path_points;
        let current_scan_points = model.diagnostics.scan_points;
        let visible_scan_points = self.scene_renderer.visible_points();
        let scene_camera_distance = self.scene_renderer.camera().distance;
        let mut scene_camera_mode = self.scene_renderer.camera_mode();
        let mut accumulate_points = self.accumulate_points;
        let scene_diagnostics = self.scene_builder.diagnostics();
        let tf_status = if let Some(error) = &scene_diagnostics.current_tf_error {
            format!(
                "TF missing {} → {} · misses {}",
                error.source_frame, error.target_frame, scene_diagnostics.tf_misses
            )
        } else {
            scene_diagnostics.last_tf_route.as_ref().map_or_else(
                || format!("TF waiting · misses {}", scene_diagnostics.tf_misses),
                |route| {
                    format!(
                        "TF {route} · static {} dynamic {} · misses {}",
                        session.state().transforms.static_len(),
                        session.state().transforms.dynamic_len(),
                        scene_diagnostics.tf_misses
                    )
                },
            )
        };
        let mut bev_logical_size = egui::Vec2::ZERO;
        let mut scene_logical_size = egui::Vec2::ZERO;
        let mut scene_wheel_delta = 0.0_f32;
        let mut scene_orbit_delta = egui::Vec2::ZERO;
        let mut reset_scene_camera = false;
        let mut seeked = false;
        let egui = self.egui_context.run(input, |context| {
            if let Some(clock) = session.clock_mut() {
                egui::TopBottomPanel::bottom("playback-controls").show(context, |ui| {
                    seeked = playback_controls(ui, clock).seeked;
                });
            } else {
                egui::TopBottomPanel::bottom("live-status").show(context, |ui| {
                    ui.label("Live mode · timeline and playback clock disabled");
                });
            }
            egui::SidePanel::left("source-status")
                .resizable(true)
                .default_width(260.0)
                .show(context, |ui| source_status(ui, &model));
            egui::CentralPanel::default().show(context, |ui| {
                let top_size = egui::vec2(ui.available_width(), ui.available_height() * 0.52);
                ui.allocate_ui(top_size, |ui| {
                    ui.columns(2, |columns| {
                        columns[0].heading(format!(
                            "CAMERA FOCUS · {focused_label} · {focused_overlay}"
                        ));
                        columns[0].separator();
                        if let Some((id, (width, height))) = focused_texture {
                            let available = columns[0].available_size();
                            let scale = (available.x / width as f32)
                                .min((available.y - 70.0).max(1.0) / height as f32)
                                .max(0.0);
                            let size = egui::vec2(width as f32 * scale, height as f32 * scale);
                            let focus_area = egui::vec2(available.x, (available.y - 70.0).max(1.0));
                            columns[0].allocate_ui(focus_area, |ui| {
                                ui.centered_and_justified(|ui| {
                                    ui.add(egui::Image::new((id, size)))
                                        .on_hover_text("Focused camera");
                                });
                            });
                        } else {
                            columns[0].centered_and_justified(|ui| {
                                ui.vertical_centered(|ui| {
                                    ui.spinner();
                                    ui.label("Waiting for camera frame");
                                });
                            });
                        }
                        columns[0].horizontal_wrapped(|ui| {
                            for (camera_id, (texture_id, (width, height))) in &camera_cards {
                                let scale = (96.0 / *width as f32).min(72.0 / *height as f32);
                                let size = egui::vec2(
                                    *width as f32 * scale.max(0.01),
                                    *height as f32 * scale.max(0.01),
                                );
                                let response = ui
                                    .add(
                                        egui::Image::new((*texture_id, size))
                                            .sense(egui::Sense::click()),
                                    )
                                    .on_hover_text(format!("Focus camera {}", camera_id.0));
                                if response.clicked() {
                                    focused_camera = Some(*camera_id);
                                }
                            }
                            if camera_cards.is_empty() && !camera_ids.is_empty() {
                                ui.label("Waiting for camera frames…");
                            }
                        });

                        columns[1].heading(format!("BEV · PATH {bev_path_points} pts"));
                        columns[1].separator();
                        bev_logical_size = columns[1].available_size().max(egui::vec2(1.0, 1.0));
                        columns[1].add(egui::Image::new((bev_texture_id, bev_logical_size)));
                    });
                });
                ui.separator();
                ui.horizontal(|ui| {
                    ui.heading(format!("3D VIEW · SCAN {current_scan_points} pts"));
                    ui.separator();
                    egui::ComboBox::from_id_salt("scene-camera-mode")
                        .selected_text(scene_camera_mode.label())
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut scene_camera_mode,
                                SceneCameraMode::Chase,
                                SceneCameraMode::Chase.label(),
                            );
                            ui.selectable_value(
                                &mut scene_camera_mode,
                                SceneCameraMode::Free,
                                SceneCameraMode::Free.label(),
                            );
                            ui.selectable_value(
                                &mut scene_camera_mode,
                                SceneCameraMode::VehicleEye,
                                SceneCameraMode::VehicleEye.label(),
                            );
                        });
                    ui.checkbox(&mut accumulate_points, "Accumulate scans");
                    if accumulate_points {
                        ui.label(format!("visible {visible_scan_points}"));
                    }
                    ui.label(format!("camera {scene_camera_distance:.1} m"));
                    ui.label(tf_status.as_str());
                });
                scene_logical_size = ui.available_size().max(egui::vec2(1.0, 1.0));
                let response = ui
                    .add(
                        egui::Image::new((scene_texture_id, scene_logical_size))
                            .sense(egui::Sense::drag()),
                    )
                    .on_hover_text(match scene_camera_mode {
                        SceneCameraMode::Chase => {
                            "Vehicle-following chase view · Wheel: zoom · Double-click: reset"
                        }
                        SceneCameraMode::Free => {
                            "Free view · Wheel: zoom · Drag: orbit · Double-click: reset"
                        }
                        SceneCameraMode::VehicleEye => "Forward view from the vehicle",
                    });
                if response.hovered() && scene_camera_mode != SceneCameraMode::VehicleEye {
                    scene_wheel_delta = ui.input(|input| input.smooth_scroll_delta.y);
                }
                if scene_camera_mode == SceneCameraMode::Free
                    && response.dragged_by(egui::PointerButton::Primary)
                {
                    scene_orbit_delta = ui.input(|input| input.pointer.delta());
                }
                reset_scene_camera = response.double_clicked();
            });
        });
        self.focused_camera = focused_camera;
        session.set_focused_camera(focused_camera);
        UiOutput {
            egui,
            seeked,
            bev_size: bev_logical_size,
            scene_size: scene_logical_size,
            accumulate_points,
            scene_wheel_delta,
            scene_orbit_delta,
            reset_scene_camera,
            scene_camera_mode,
        }
    }
}
