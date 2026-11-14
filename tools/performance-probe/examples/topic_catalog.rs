//! Isolated catalog UI preparation, excluding MCAP I/O, for responsiveness comparisons.
use std::{hint::black_box, sync::Arc, time::Instant};
use viewer_core::{
    SchemaDefinition, SourceCatalog, StreamDescriptor, StreamId, StreamTimingSummary, TopicTree,
};
fn main() {
    let schema = Arc::new(SchemaDefinition {
        encoding: "ros2msg".into(),
        data: (0..64)
            .map(|i| format!("float64 field_{i}\n"))
            .collect::<String>()
            .into_bytes()
            .into(),
    });
    let mut results = Vec::new();
    for count in [500usize, 5000] {
        let catalog = SourceCatalog {
            streams: (0..count)
                .map(|i| StreamDescriptor {
                    id: StreamId(i as u32),
                    topic: format!("/robot/sensor_{i:05}"),
                    schema: "demo/msg/Status".into(),
                    message_encoding: "cdr".into(),
                    schema_definition: Some(schema.clone()),
                    timing: StreamTimingSummary {
                        message_count: Some(1000),
                    },
                })
                .collect(),
            ..Default::default()
        };
        let mut samples = Vec::new();
        for _ in 0..7 {
            let begin = Instant::now();
            let tree = TopicTree::from_catalog(black_box(&catalog));
            black_box(&tree);
            samples.push(begin.elapsed().as_secs_f64() * 1000.);
        }
        samples.sort_by(f64::total_cmp);
        results.push(serde_json::json!({"topics":count,"fields_per_schema":64,"median_ms":samples[3],"samples_ms":samples}));
    }
    println!(
        "{}",
        serde_json::json!({"scope":"TopicTree construction only; no MCAP I/O or message decoding","cases":results})
    );
}
