//! Exercise complete production restoration without GPU pixel conversion.
use anyhow::{Result, ensure};
use serde_json::json;
use std::{path::Path, time::Instant};
use viewer_core::{
    ArrivalTime, FeatureRuntime, PlaybackRequirements, RestorePlanner, SessionPlan,
    WorkspaceBindings,
};
use viewer_mcap::McapPlayback;

pub(super) fn measure(path: &str, pattern: &str, count: usize) -> Result<()> {
    ensure!(
        matches!(pattern, "same" | "alternating" | "scrub"),
        "invalid seek pattern"
    );
    ensure!((1..=100).contains(&count), "seek count must be 1..100");
    let mut playback = McapPlayback::open_file(Path::new(path))?;
    let mut requirements = PlaybackRequirements::empty();
    requirements.require_camera_topic("/camera/0");
    let bindings: WorkspaceBindings =
        serde_json::from_str(include_str!("../../../config/workspace_bindings.json"))?;
    let plan = SessionPlan::build(playback.catalog(), "/camera/0", &requirements, &bindings)?;
    playback.select_streams(plan.selected_stream_ids())?;
    let mut runtime = FeatureRuntime::new(&plan, true);
    let range = playback.recording_source().time_range();
    let distant = if range.end_exclusive.0 - range.start.0 > 6_000_000_000 {
        5_710_000_000
    } else {
        2_710_000_000
    };
    let mut samples = Vec::new();
    for index in 0..count {
        let offset = match pattern {
            "same" => [310_000_000, 710_000_000][index % 2],
            "alternating" => [310_000_000, distant][index % 2],
            _ => [
                110_000_000,
                210_000_000,
                310_000_000,
                410_000_000,
                310_000_000,
                210_000_000,
                110_000_000,
            ][index % 7],
        };
        let target = ArrivalTime(
            range
                .start
                .0
                .checked_add(offset)
                .ok_or_else(|| anyhow::anyhow!("seek target timestamp overflow"))?,
        );
        let restore =
            RestorePlanner::new(playback.catalog()).plan(target, plan.restore_inputs())?;
        let before = playback.recording_source().read_diagnostics();
        let started = Instant::now();
        playback.seek_with(&restore, |target, messages| {
            runtime.restore_transactional(target, &messages)
        })?;
        let elapsed_ms = started.elapsed().as_secs_f64() * 1000.0;
        let after = playback.recording_source().read_diagnostics();
        let frame = runtime
            .cameras()
            .state()
            .latest_for(viewer_core::CameraId(0))
            .ok_or_else(|| anyhow::anyhow!("restored camera absent"))?;
        ensure!(frame.arrival_time <= target, "invalid predecessor");
        ensure!(
            frame.arrival_time.0 == range.start.0 + offset / 100_000_000 * 100_000_000,
            "unexpected fixture frame"
        );
        samples.push(json!({"target_ns":target.0.to_string(),"ms":elapsed_ms,"chunks_expanded":after.chunks_expanded-before.chunks_expanded,"read_bytes":after.file_read_bytes-before.file_read_bytes,"memory":super::memory()}));
    }
    println!(
        "{}",
        json!({"path":path,"pattern":pattern,"samples":samples,"io":super::io(playback.recording_source()),"memory":super::memory(),"note":"full seek and feature restore; GPU/pixel conversion excluded; deterministic 10Hz fixture"})
    );
    Ok(())
}
