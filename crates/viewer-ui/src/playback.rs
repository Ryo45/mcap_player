use viewer_core::{ArrivalTime, PlaybackCommand, PlaybackSpeed, PlaybackView};

#[derive(Clone, Debug, PartialEq)]
pub struct PlaybackUiResponse {
    pub commands: Vec<PlaybackCommand>,
    pub play_rect: egui::Rect,
    pub timeline_rect: egui::Rect,
}

pub fn playback_controls(ui: &mut egui::Ui, view: PlaybackView) -> PlaybackUiResponse {
    let mut response = PlaybackUiResponse {
        commands: Vec::new(),
        play_rect: egui::Rect::NOTHING,
        timeline_rect: egui::Rect::NOTHING,
    };
    ui.horizontal(|ui| {
        let label = if view.playing { "Pause" } else { "Play" };
        let play = ui.button(label);
        response.play_rect = play.rect;
        if play.clicked() {
            response.commands.push(PlaybackCommand::Toggle);
        }
        let mut selected = view.speed;
        egui::ComboBox::from_id_salt("playback-speed")
            .selected_text(selected.label())
            .show_ui(ui, |ui| {
                for speed in PlaybackSpeed::ALL {
                    ui.selectable_value(&mut selected, speed, speed.label());
                }
            });
        if selected != view.speed {
            response.commands.push(PlaybackCommand::SetSpeed(selected));
        }
    });
    let start = view.start.0;
    let end = view.end.0.max(start.saturating_add(1));
    let mut cursor = view.cursor.0;
    let timeline = ui.add(
        egui::Slider::new(&mut cursor, start..=end)
            .show_value(false)
            .text("timeline"),
    );
    response.timeline_rect = timeline.rect;
    if timeline.changed() {
        response
            .commands
            .push(PlaybackCommand::Seek(ArrivalTime(cursor)));
    }
    ui.label(format!(
        "{:.3}s / {:.3}s",
        (view.cursor.0 as i128 - start as i128) as f64 / 1e9,
        (end as i128 - start as i128) as f64 / 1e9
    ));
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unchanged_frames_do_not_request_seek_or_other_commands() {
        let context = egui::Context::default();
        let view = PlaybackView {
            start: ArrivalTime(1_700_000_000_000_000_000),
            end: ArrivalTime(1_700_000_010_000_000_000),
            cursor: ArrivalTime(1_700_000_003_000_000_000),
            playing: false,
            speed: PlaybackSpeed::Normal,
        };
        for _ in 0..3 {
            let _ = context.run(egui::RawInput::default(), |context| {
                egui::CentralPanel::default().show(context, |ui| {
                    let response = playback_controls(ui, view);
                    assert!(response.commands.is_empty());
                    assert!(response.play_rect.is_positive());
                    assert!(response.timeline_rect.is_positive());
                });
            });
        }
    }
}
