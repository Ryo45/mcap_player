//! Measure existing stage_restore with populated vs already-empty feature state.
//! The empty-state control is a cost isolation experiment, not an alternative restore implementation.
use anyhow::{Result, ensure};
use serde_json::json;
use std::{
    alloc::{GlobalAlloc, Layout, System},
    hint::black_box,
    sync::atomic::{AtomicBool, AtomicUsize, Ordering::Relaxed},
    time::{Duration, Instant},
};
use viewer_core::{
    ArrivalTime, FeatureRuntime, PlaybackRequirements, RawMessage, SessionPlan, SourceCatalog,
    StreamDescriptor, StreamId, StreamTimingSummary, WorkspaceBindings,
};

struct CountingAllocator;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static ENABLED: AtomicBool = AtomicBool::new(false);
static ALLOCATED: AtomicUsize = AtomicUsize::new(0);
static CALLS: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
fn allocated(size: usize) {
    let live = LIVE.fetch_add(size, Relaxed) + size;
    if ENABLED.load(Relaxed) {
        ALLOCATED.fetch_add(size, Relaxed);
        CALLS.fetch_add(1, Relaxed);
        PEAK.fetch_max(live, Relaxed);
    }
}
// SAFETY: Every operation delegates the same pointer/layout to System. Counters do not allocate.
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            allocated(layout.size());
        }
        ptr
    }
    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            allocated(layout.size());
        }
        ptr
    }
    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Relaxed);
        unsafe { System.dealloc(ptr, layout) };
    }
    unsafe fn realloc(&self, ptr: *mut u8, old: Layout, size: usize) -> *mut u8 {
        let new = unsafe { System.realloc(ptr, old, size) };
        if !new.is_null() {
            LIVE.fetch_sub(old.size(), Relaxed);
            allocated(size);
        }
        new
    }
}
#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

fn u32_cdr(out: &mut Vec<u8>, value: u32) {
    while !(out.len() - 4).is_multiple_of(4) {
        out.push(0);
    }
    out.extend_from_slice(&value.to_le_bytes());
}
fn string(out: &mut Vec<u8>, value: &str) {
    u32_cdr(out, (value.len() + 1) as u32);
    out.extend_from_slice(value.as_bytes());
    out.push(0);
}
fn message(count: u32, color: bool, intensity: bool) -> RawMessage {
    let fields = 3 + u32::from(color) + u32::from(intensity);
    let step = fields * 4;
    let mut out = vec![0, 1, 0, 0];
    u32_cdr(&mut out, 1);
    u32_cdr(&mut out, 0);
    string(&mut out, "base_link");
    u32_cdr(&mut out, 1);
    u32_cdr(&mut out, count);
    u32_cdr(&mut out, fields);
    let mut names = vec![("x", 7), ("y", 7), ("z", 7)];
    if color {
        names.push(("rgba", 6));
    }
    if intensity {
        names.push(("intensity", 7));
    }
    for (index, (name, datatype)) in names.into_iter().enumerate() {
        string(&mut out, name);
        u32_cdr(&mut out, index as u32 * 4);
        out.push(datatype);
        u32_cdr(&mut out, 1);
    }
    out.push(0);
    u32_cdr(&mut out, step);
    u32_cdr(&mut out, step * count);
    u32_cdr(&mut out, step * count);
    for i in 0..count {
        for v in [i as f32 * 0.001, 0.5, 1.0] {
            out.extend_from_slice(&v.to_le_bytes());
        }
        if color {
            out.extend_from_slice(&0xff804020u32.to_le_bytes());
        }
        if intensity {
            out.extend_from_slice(&((i % 256) as f32).to_le_bytes());
        }
    }
    out.push(1);
    RawMessage {
        stream_id: StreamId(1),
        arrival_time: ArrivalTime(1_000_000_000),
        publish_time_ns: None,
        sequence: Some(0),
        payload: out.into(),
    }
}
fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    ensure!(
        args.len() == 3,
        "seek_clone POINTS xyz|rgba|rgba-intensity SAMPLES"
    );
    let points: u32 = args[0].parse()?;
    let samples: usize = args[2].parse()?;
    ensure!(
        (1..=1_000_000).contains(&points) && (5..=500).contains(&samples),
        "invalid workload size"
    );
    let (color, intensity) = match args[1].as_str() {
        "xyz" => (false, false),
        "rgba" => (true, false),
        "rgba-intensity" => (true, true),
        _ => anyhow::bail!("invalid attributes"),
    };
    let catalog = SourceCatalog {
        streams: vec![StreamDescriptor {
            id: StreamId(1),
            topic: "/points".into(),
            schema: "sensor_msgs/msg/PointCloud2".into(),
            schema_definition: None,
            message_encoding: "cdr".into(),
            timing: StreamTimingSummary::default(),
        }],
        ..Default::default()
    };
    let mut requirements = PlaybackRequirements::empty();
    requirements.require_point_cloud();
    let mut bindings: WorkspaceBindings =
        serde_json::from_str(include_str!("../../../config/workspace_bindings.json"))?;
    bindings.point_cloud_topic = "/points".into();
    let plan = SessionPlan::build(&catalog, "/unused", &requirements, &bindings)?;
    let good = message(points, color, intensity);
    let mut runtime = FeatureRuntime::new(&plan, true);
    runtime.process_messages(Duration::ZERO, std::slice::from_ref(&good));
    let cloud = runtime.point_cloud().unwrap().point_cloud();
    let old_revision = cloud.revision();
    let frame = cloud.latest().unwrap();
    ensure!(
        frame.points.len() == points as usize,
        "fixture decode failed"
    );
    ensure!(
        frame.attributes.rgba.as_ref().map(Vec::len) == color.then_some(points as usize),
        "color fixture mismatch"
    );
    ensure!(
        frame.attributes.intensity.as_ref().map(Vec::len) == intensity.then_some(points as usize),
        "intensity fixture mismatch"
    );
    let pointer = frame.points.as_ptr();
    let counters = runtime.counters();
    let target = ArrivalTime(2_000_000_000);
    // Setup is not measured. This control preserves routes, priority and counters using existing
    // production code, but has an extra revision increment. It is NOT a proposed replacement API.
    let empty = runtime.stage_restore(target, &[])?;
    let mut bad = good.clone();
    bad.payload = vec![0, 1, 0, 0].into();
    let mut rows = Vec::with_capacity(samples * 6);
    for round in 0..samples + 10 {
        for mode in ["clear-only", "decode", "failure"] {
            for index in 0..2 {
                let control = (round + index) % 2 == 1;
                let input = if control { &empty } else { &runtime };
                let messages = match mode {
                    "clear-only" => &[][..],
                    "decode" => std::slice::from_ref(&good),
                    _ => std::slice::from_ref(&bad),
                };
                let baseline = LIVE.load(Relaxed);
                ALLOCATED.store(0, Relaxed);
                CALLS.store(0, Relaxed);
                PEAK.store(baseline, Relaxed);
                ENABLED.store(true, Relaxed);
                let start = Instant::now();
                let restored =
                    black_box(input).stage_restore(black_box(target), black_box(messages));
                let elapsed_ns = start.elapsed().as_nanos() as u64;
                ENABLED.store(false, Relaxed);
                let allocated_bytes = ALLOCATED.load(Relaxed);
                let allocation_calls = CALLS.load(Relaxed);
                let peak_extra_live_bytes = PEAK.load(Relaxed).saturating_sub(baseline);
                if mode == "failure" {
                    ensure!(restored.is_err(), "malformed restore accepted");
                } else {
                    let candidate = restored.as_ref().unwrap();
                    let candidate_cloud = candidate.point_cloud().unwrap().point_cloud();
                    if mode == "clear-only" {
                        ensure!(candidate_cloud.latest().is_none(), "old frame survived");
                    } else {
                        ensure!(
                            candidate_cloud.latest().unwrap().points.len() == points as usize,
                            "restore count mismatch"
                        );
                    }
                    ensure!(
                        candidate_cloud.revision()
                            == input
                                .point_cloud()
                                .unwrap()
                                .point_cloud()
                                .revision()
                                .wrapping_add(if mode == "decode" { 2 } else { 1 }),
                        "revision mismatch"
                    );
                }
                // No commit: success and failure must leave the authoritative frame/counters intact.
                ensure!(
                    runtime
                        .point_cloud()
                        .unwrap()
                        .point_cloud()
                        .latest()
                        .unwrap()
                        .points
                        .as_ptr()
                        == pointer,
                    "authoritative frame changed"
                );
                ensure!(
                    runtime.point_cloud().unwrap().point_cloud().revision() == old_revision
                        && runtime.counters() == counters,
                    "authoritative state changed"
                );
                if round >= 10 {
                    rows.push(json!({"mode":mode,"control":control,"ns":elapsed_ns,"allocated_bytes":allocated_bytes,"allocation_calls":allocation_calls,"peak_extra_live_bytes":peak_extra_live_bytes}));
                }
                drop(black_box(restored));
            }
        }
    }
    println!(
        "{}",
        json!({"points":points,"attributes":args[1],"samples":samples,"warmups":10,"old_decoded_vector_bytes":points as u64 * (12 + u64::from(color)*4 + u64::from(intensity)*4),"raw_message_bytes":good.payload.len(),"rows":rows,"note":"production stage_restore; empty-state control isolates discarded previous state, not a semantically equivalent new implementation; allocator requested bytes, not RSS/VRAM; no I/O or GPU"})
    );
    Ok(())
}
