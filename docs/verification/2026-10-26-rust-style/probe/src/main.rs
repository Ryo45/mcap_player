pub use viewer_core::{ArrivalTime, MeasurementTime};
#[allow(dead_code)]
mod before;
use std::{hint::black_box, time::Instant};
fn main() {
    let mut rows = Vec::new();
    for budget in [400, 4_000, 10_000] {
        let mut old = before::SignalOverviewReducer::new(ArrivalTime(0), ArrivalTime(1_000_001), budget);
        let mut new = viewer_core::SignalOverviewReducer::new(ArrivalTime(0), ArrivalTime(1_000_001), budget);
        for arrival in 0..1_000_000 {
            let value = (arrival % 997) as f64;
            old.push(before::SignalSample {measurement_time: Some(MeasurementTime(arrival-1)), arrival_time:ArrivalTime(arrival), value});
            new.push(viewer_core::SignalSample {measurement_time: Some(MeasurementTime(arrival-1)), arrival_time:ArrivalTime(arrival), value});
        }
        let a=old.finish(before::SignalId::Speed).unwrap();
        let b=new.finish(viewer_core::SignalId::Speed).unwrap();
        assert_eq!(a.display.x_seconds,b.display.x_seconds);
        assert_eq!(a.display.values,b.display.values);
        let mut times=[Vec::new(),Vec::new()];
        for round in 0..23 {
            for offset in 0..2 {
                let which=(round+offset)%2;
                let t=Instant::now();
                for _ in 0..100 {
                    if which==0 {drop(black_box(old.finish(black_box(before::SignalId::Speed))));}
                    else {drop(black_box(new.finish(black_box(viewer_core::SignalId::Speed))));}
                }
                if round>=3 {times[which].push(t.elapsed().as_nanos() as f64/100.0);}
            }
        }
        for values in &mut times { values.sort_by(f64::total_cmp); }
        let old_ns=(times[0][9]+times[0][10])/2.0;
        let new_ns=(times[1][9]+times[1][10])/2.0;
        rows.push(serde_json::json!({"point_budget":budget,"before_ns":old_ns,"after_ns":new_ns,"ratio":new_ns/old_ns,"before_batches":times[0],"after_batches":times[1]}));
    }
    println!("{}",serde_json::to_string_pretty(&rows).unwrap());
}
