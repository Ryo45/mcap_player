//! Fixed-range query controls and presentation; acquisition belongs to QueryService.
use super::{
    NativePanel, PanelCompositionInput, PanelOutput, PlaceholderPanel,
    inspector::{InspectorTreeState, show_message_fields},
};
use crate::query_service::{
    MAX_TOPICS, QueryMode, QueryRequest, QueryResult, QueryService, QueryState, QueryTopic,
};
use std::collections::BTreeMap;
use viewer_core::{ArrivalTime, DataWindowTimeRange, MessageScalar, SelectedValue, StreamId};
use viewer_layout::PanelNode;

pub(crate) struct QueryPanel {
    pub(super) service: QueryService,
    initialized: bool,
    start: String,
    end: String,
    fields: BTreeMap<StreamId, String>,
    mode: QueryMode,
    last: Option<QueryRequest>,
    history: Vec<Option<viewer_mcap::RangePageCursor>>,
    selected: Option<usize>,
    inspector: InspectorTreeState,
    error: Option<String>,
}
impl QueryPanel {
    pub fn create(node: &PanelNode) -> NativePanel {
        if node.config_version != 1 || node.config != serde_json::json!({}) {
            return NativePanel::Placeholder(PlaceholderPanel::invalid_config(
                node,
                "Query requires version 1 and empty config".into(),
            ));
        }
        NativePanel::Query(Box::new(Self {
            service: Default::default(),
            initialized: false,
            start: String::new(),
            end: String::new(),
            fields: BTreeMap::new(),
            mode: QueryMode::Table,
            last: None,
            history: Vec::new(),
            selected: None,
            inspector: Default::default(),
            error: None,
        }))
    }
    pub fn reset_for_source(&mut self) {
        self.service.bind_source(None);
        self.initialized = false;
        self.start.clear();
        self.end.clear();
        self.fields.clear();
        self.changed();
    }
    fn changed(&mut self) {
        self.service.cancel();
        self.last = None;
        self.history.clear();
        self.selected = None;
        self.inspector = Default::default();
        self.error = None;
    }
    fn run(&mut self, request: QueryRequest) {
        self.selected = None;
        self.inspector = Default::default();
        self.error = None;
        self.last = Some(request.clone());
        self.service.submit(request);
    }
    fn request(&self) -> Result<QueryRequest, String> {
        let start = self
            .start
            .trim()
            .parse::<i64>()
            .map_err(|_| "Start must be signed integer log-time nanoseconds")?;
        let end = self
            .end
            .trim()
            .parse::<i64>()
            .map_err(|_| "End must be signed integer log-time nanoseconds")?;
        let range = DataWindowTimeRange::new(ArrivalTime(start), ArrivalTime(end))
            .map_err(|e| e.to_string())?;
        Ok(QueryRequest {
            range,
            mode: self.mode,
            after: None,
            topics: self
                .fields
                .iter()
                .map(|(stream, text)| QueryTopic {
                    stream: *stream,
                    fields: text
                        .split(',')
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                        .map(str::to_owned)
                        .collect(),
                })
                .collect(),
        })
    }
    pub fn show(&mut self, ui: &mut egui::Ui, input: &PanelCompositionInput<'_>) -> PanelOutput {
        ui.heading("Query recording");
        self.service.poll();
        let Some(range) = self.service.range() else {
            ui.label(
                "Indexed recording only · arbitrary history queries are unavailable in ROS 2 Live.",
            );
            return PanelOutput::default();
        };
        if !self.initialized {
            self.initialized = true;
            self.start = range.start.0.to_string();
            self.end = range.end_exclusive.0.to_string();
        }
        ui.small(
            "Fixed MCAP log-time range [start, end) · playback/Seek do not change these conditions",
        );
        let mut changed = false;
        ui.horizontal(|ui| {
            ui.label("Start ns");
            changed |= ui
                .add(
                    egui::TextEdit::singleline(&mut self.start)
                        .char_limit(24)
                        .desired_width(200.),
                )
                .changed();
            ui.label("End ns (exclusive)");
            changed |= ui
                .add(
                    egui::TextEdit::singleline(&mut self.end)
                        .char_limit(24)
                        .desired_width(200.),
                )
                .changed();
        });
        ui.horizontal(|ui| {
            if ui.button("Full recording").clicked() {
                self.start = range.start.0.to_string();
                self.end = range.end_exclusive.0.to_string();
                changed = true;
            }
            if let Some(view) = input.playback {
                if ui.button("Start at cursor").clicked() {
                    self.start = view.cursor.0.to_string();
                    changed = true;
                }
                if ui.button("End after cursor").clicked()
                    && let Some(end) = view.cursor.0.checked_add(1)
                {
                    self.end = end.to_string();
                    changed = true;
                }
            }
            changed |= ui
                .selectable_value(&mut self.mode, QueryMode::Table, "Table")
                .changed();
            changed |= ui
                .selectable_value(&mut self.mode, QueryMode::Plot, "Time series Plot")
                .changed();
        });
        ui.small("Select up to 8 streams. Fields: comma-separated ROS paths; e.g. twist.twist.linear.x. Table: leave blank for lazy Inspector. Plot: scalar numeric fields only.");
        if let Some(catalog) = self.service.catalog() {
            egui::ScrollArea::vertical()
                .id_salt("query-streams")
                .max_height(170.)
                .show_rows(
                    ui,
                    ui.spacing().interact_size.y,
                    catalog.streams.len(),
                    |ui, rows| {
                        for stream in &catalog.streams[rows] {
                            let mut selected = self.fields.contains_key(&stream.id);
                            ui.horizontal(|ui| {
                                if ui
                                    .add_enabled(
                                        selected || self.fields.len() < MAX_TOPICS,
                                        egui::Checkbox::new(
                                            &mut selected,
                                            format!(
                                                "{} · {} (#{})",
                                                stream.topic, stream.schema, stream.id.0
                                            ),
                                        ),
                                    )
                                    .changed()
                                {
                                    if selected {
                                        self.fields.insert(stream.id, String::new());
                                    } else {
                                        self.fields.remove(&stream.id);
                                    }
                                    changed = true;
                                }
                                if let Some(fields) = self.fields.get_mut(&stream.id) {
                                    changed |= ui
                                        .add(
                                            egui::TextEdit::singleline(fields)
                                                .char_limit(1024)
                                                .hint_text("selected fields")
                                                .desired_width(280.),
                                        )
                                        .changed();
                                }
                            });
                        }
                    },
                );
        }
        if changed {
            self.changed();
        }
        ui.horizontal(|ui| {
            if ui.button("Run query").clicked() { match self.request() { Ok(request)=>{self.history.clear();self.run(request)}, Err(e)=>{self.changed();self.error=Some(e);} } }
            if ui.button("Cancel / clear").clicked() {self.changed();}
            ui.small("64 rows / 4 MiB detail cache · 8 fields · 2048 Plot points per field · 256 MiB read/expansion work");
        });
        if let Some(error) = &self.error {
            ui.colored_label(egui::Color32::LIGHT_RED, error);
        }
        let mut next_request = None;
        match &self.service.state {
            QueryState::Unavailable => {
                ui.label("Recording only");
            }
            QueryState::Idle => {
                ui.label("Idle · choose conditions and Run query");
            }
            QueryState::Loading => {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label("Loading query… playback remains independent");
                });
            }
            QueryState::Error(error) => {
                ui.colored_label(egui::Color32::LIGHT_RED, error);
            }
            QueryState::Completed(QueryResult::Table { rows, next }) => {
                ui.label(format!(
                    "Completed · {} rows{}",
                    rows.len(),
                    if next.is_some() {
                        " · more available"
                    } else {
                        " · end of range"
                    }
                ));
                ui.horizontal(|ui| {
                    if ui
                        .add_enabled(!self.history.is_empty(), egui::Button::new("Previous page"))
                        .clicked()
                        && let Some(mut request) = self.last.clone()
                    {
                        request.after = self.history.pop().unwrap();
                        next_request = Some(request);
                    }
                    if ui
                        .add_enabled(next.is_some(), egui::Button::new("Next page"))
                        .clicked()
                        && let Some(mut request) = self.last.clone()
                    {
                        if self.history.len() == 16 {
                            self.history.remove(0);
                        }
                        self.history.push(request.after);
                        request.after = *next;
                        next_request = Some(request);
                    }
                });
                egui::ScrollArea::both()
                    .id_salt("query-rows")
                    .max_height(260.)
                    .show(ui, |ui| {
                        egui::Grid::new("query-table").striped(true).show(ui, |ui| {
                            ui.strong("Log time ns");
                            ui.strong("Topic / stream");
                            ui.strong("Selected fields");
                            ui.end_row();
                            for (index, row) in rows.iter().enumerate() {
                                if ui
                                    .selectable_label(
                                        self.selected == Some(index),
                                        row.time.0.to_string(),
                                    )
                                    .clicked()
                                {
                                    self.selected = Some(index);
                                }
                                let topic =
                                    self.service.catalog().and_then(|c| c.by_id(row.stream));
                                ui.label(topic.map_or("Unknown", |s| s.topic.as_str()));
                                let names = self
                                    .last
                                    .as_ref()
                                    .and_then(|r| r.topics.iter().find(|t| t.stream == row.stream));
                                let values = row
                                    .values
                                    .iter()
                                    .enumerate()
                                    .map(|(i, v)| {
                                        format!(
                                            "{} = {}",
                                            names
                                                .and_then(|n| n.fields.get(i))
                                                .map_or("?", String::as_str),
                                            match v {
                                                Ok(v) => value(v),
                                                Err(e) => format!("Error: {e}"),
                                            }
                                        )
                                    })
                                    .collect::<Vec<_>>()
                                    .join("; ");
                                ui.label(if values.is_empty() {
                                    format!("{} bytes · select time for fields", row.payload_bytes)
                                } else {
                                    values
                                });
                                ui.end_row();
                            }
                        });
                    });
                if let Some(row) = self.selected.and_then(|i| rows.get(i)) {
                    ui.separator();
                    ui.label(format!(
                        "Message at {} ns · stream {}",
                        row.time.0, row.stream.0
                    ));
                    match &row.message {
                        Ok(message) => show_message_fields(ui, message, &mut self.inspector),
                        Err(e) => {
                            ui.colored_label(egui::Color32::LIGHT_RED, e);
                        }
                    }
                }
            }
            QueryState::Completed(QueryResult::Plot(series)) => {
                ui.label("Completed · bounded min/max overview, independent of playback");
                if let Some(request) = &self.last {
                    let origin = request.range.start;
                    egui_plot::Plot::new("query-plot")
                        .height(320.)
                        .legend(egui_plot::Legend::default())
                        .x_axis_label("Seconds from query start (log time)")
                        .show(ui, |plot| {
                            for series in series {
                                let topic = self
                                    .service
                                    .catalog()
                                    .and_then(|c| c.by_id(series.stream))
                                    .map_or("?", |s| s.topic.as_str());
                                let points = series
                                    .overview
                                    .samples
                                    .iter()
                                    .map(|s| [seconds(s.arrival_time, origin), s.value])
                                    .collect::<Vec<_>>();
                                plot.line(egui_plot::Line::new(
                                    format!("{topic}: {}", series.field),
                                    points,
                                ));
                            }
                            if let Some(view) = input.playback {
                                plot.vline(egui_plot::VLine::new(
                                    "Playback cursor",
                                    seconds(view.cursor, origin),
                                ));
                            }
                        });
                    for s in series {
                        ui.small(format!(
                            "{}: {} inputs, {} decode errors",
                            s.field, s.overview.input_sample_count, s.overview.decode_errors
                        ));
                    }
                }
            }
        }
        if let Some(request) = next_request {
            self.run(request);
        }
        PanelOutput::default()
    }
}
fn seconds(time: ArrivalTime, origin: ArrivalTime) -> f64 {
    (i128::from(time.0) - i128::from(origin.0)) as f64 / 1e9
}
fn value(v: &SelectedValue) -> String {
    match v {
        SelectedValue::Scalar(s) => match s {
            MessageScalar::Bool(v) => v.to_string(),
            MessageScalar::Signed(v) => v.to_string(),
            MessageScalar::Unsigned(v) => v.to_string(),
            MessageScalar::Float(v) => v.to_string(),
            MessageScalar::Text(v) => v.clone(),
        },
        SelectedValue::Array(v) => {
            format!("[{}]", v.iter().map(value).collect::<Vec<_>>().join(", "))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::query_service::QueryRow;
    fn panel() -> QueryPanel {
        let NativePanel::Query(panel) = QueryPanel::create(&PanelNode {
            id: viewer_layout::PanelId::new("query").unwrap(),
            panel_type: "query".into(),
            config_version: 1,
            title: None,
            config: serde_json::json!({}),
        }) else {
            panic!()
        };
        *panel
    }
    #[test]
    fn edited_conditions_and_source_reset_drop_old_results_without_playback_commands() {
        let mut p = panel();
        p.initialized = true;
        p.start = "100".into();
        p.end = "200".into();
        p.fields.insert(StreamId(1), "value, status".into());
        let request = p.request().unwrap();
        assert_eq!(request.range.start, ArrivalTime(100));
        assert_eq!(request.topics[0].fields, ["value", "status"]);
        p.service.state = QueryState::Completed(QueryResult::Table {
            rows: Vec::<QueryRow>::new(),
            next: None,
        });
        p.last = Some(request);
        p.changed();
        assert!(p.last.is_none());
        assert!(matches!(p.service.state, QueryState::Unavailable));
        p.reset_for_source();
        assert!(p.fields.is_empty());
        assert!(p.start.is_empty());
        assert!(!p.initialized);
        p.start = i64::MIN.to_string();
        p.end = i64::MAX.to_string();
        assert!(p.request().is_ok());
        p.end = p.start.clone();
        assert!(p.request().is_err());
    }
    #[test]
    fn plot_offsets_do_not_overflow_at_i64_boundaries() {
        assert!(seconds(ArrivalTime(i64::MAX), ArrivalTime(i64::MIN)) > 0.);
    }
}
