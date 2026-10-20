#!/usr/bin/env python3
"""Build a measurement-only Native; restore every product source even on build failure."""
import difflib
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[2]
REPORT = ROOT/'docs/performance/2026-10-20-desktop-workloads'
PATHS = [
    'apps/viewer-native/src/main.rs', 'apps/viewer-native/src/app.rs',
    'apps/viewer-native/src/graphics/views/camera.rs', 'apps/viewer-native/src/graphics/views/scene.rs',
    'apps/viewer-native/src/graphics/layout_host.rs', 'apps/viewer-native/src/graphics/ui.rs',
    'apps/viewer-native/src/panels/inspector.rs', 'crates/scene-renderer/src/lib.rs',
]
original = {p: (ROOT/p).read_text() for p in PATHS}


def replace(path, old, new):
    target = ROOT/path
    text = target.read_text()
    if text.count(old) != 1:
        raise RuntimeError((path, old, text.count(old)))
    target.write_text(text.replace(old, new))


try:
    if subprocess.run(['git','diff','--quiet','--',*PATHS],cwd=ROOT).returncode:
        raise RuntimeError('product sources must be clean')
    replace(PATHS[0], '    let mut workspace = NativeWorkspace::load_bundled_or_fallback(args.layout);',
            '''    let mut workspace = if let Ok(path) = std::env::var("MCAP_BENCH_LAYOUT") {
        NativeWorkspace::from_json(&std::fs::read_to_string(path)?).map_err(anyhow::Error::msg)?
    } else { NativeWorkspace::load_bundled_or_fallback(args.layout) };''')
    with (ROOT/PATHS[0]).open('a') as output:
        output.write('''
pub(crate) fn bench_rect(key: &str) -> egui::Rect {
    static VALUE: std::sync::OnceLock<serde_json::Value> = std::sync::OnceLock::new();
    let config = VALUE.get_or_init(|| serde_json::from_str(&std::fs::read_to_string(std::env::var("MCAP_BENCH_GEOMETRY").unwrap()).unwrap()).unwrap());
    let r = config["rects"][key].as_array().unwrap();
    egui::Rect::from_min_size(egui::pos2(r[0].as_f64().unwrap() as f32,r[1].as_f64().unwrap() as f32),egui::vec2(r[2].as_f64().unwrap() as f32,r[3].as_f64().unwrap() as f32))
}
''')
    replace(PATHS[1], '        self.advance_session(elapsed);', '''
        static LAST: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        if let Ok(path) = std::env::var("MCAP_BENCH_CONTROL")
            && let Ok(text) = std::fs::read_to_string(path)
            && let Some((id, command)) = text.trim().split_once(' ')
            && let Ok(id) = id.parse::<u64>()
            && id > LAST.load(std::sync::atomic::Ordering::Relaxed)
            && let Some(view) = self.session.as_ref().and_then(ViewerSession::playback_view)
        {
            let mut actions = Vec::new();
            if let Some(offset) = command.strip_prefix("seek ").and_then(|v| v.parse::<i64>().ok()) {
                if view.playing { actions.push(ViewerAction::Playback(viewer_core::PlaybackCommand::Toggle)); }
                actions.push(ViewerAction::Playback(viewer_core::PlaybackCommand::Seek(viewer_core::ArrivalTime(view.start.0.saturating_add(offset)))));
            } else if (command == "play" && !view.playing) || (command == "pause" && view.playing) {
                actions.push(ViewerAction::Playback(viewer_core::PlaybackCommand::Toggle));
            }
            self.apply_frame_actions(actions);
            LAST.store(id, std::sync::atomic::Ordering::Relaxed);
            log::info!("GPU_BENCH_COMMAND id={id}");
        }
        self.advance_session(elapsed);''')
    replace(PATHS[1], '        self.advance_session(elapsed);', '''
        self.advance_session(elapsed);
        if let Some(view) = self.session.as_ref().and_then(ViewerSession::playback_view) {
            static CLOCK: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(i64::MIN);
            if CLOCK.swap(view.cursor.0,std::sync::atomic::Ordering::Relaxed) != view.cursor.0 {
                log::info!("GPU_BENCH_CURSOR cursor={}",view.cursor.0);
            }
        }
        if let Some(frame) = self.workspace.runtime.as_ref().and_then(|r|r.point_cloud()).and_then(|c|c.point_cloud().latest()) {
            static CLOUD: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(i64::MIN);
            if CLOUD.swap(frame.arrival_time.0,std::sync::atomic::Ordering::Relaxed) != frame.arrival_time.0 {
                log::info!("GPU_BENCH_CLOUD arrival={} points={}",frame.arrival_time.0,frame.points.len());
            }
        }
''')
    replace(PATHS[6], '    if let Some(metadata) = inspection.metadata {', '''
    if let Some(metadata) = inspection.metadata {
        static RAW: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(i64::MIN);
        if RAW.swap(metadata.log_time.0,std::sync::atomic::Ordering::Relaxed) != metadata.log_time.0 {
            log::info!("GPU_BENCH_INSPECTOR log={}",metadata.log_time.0);
        }
''')
    replace(PATHS[1], '            Ok(mut session) => {', '''            Ok(mut session) => {
                log::info!("GPU_BENCH_SELECTION cameras={:?} cloud={:?}",session.plan().camera_topics(),session.plan().point_cloud_stream().map(|s|s.topic.as_str()));''')
    replace(PATHS[2], '    let focus_area = egui::vec2(available.x, (available.y - thumbnail_reserve).max(1.0));',
            '''    let focus_area = input.focused_camera.map_or_else(|| egui::vec2(available.x, (available.y - thumbnail_reserve).max(1.0)), |id| crate::bench_rect(&format!("camera_{}",id.0)).size());''')
    replace(PATHS[2], '    let image_rect = contain_image_rect(container, texture.size);',
            '''    let image_rect = contain_image_rect(crate::bench_rect(&format!("camera_{}",texture.camera_id.0)), texture.size);
    ui.set_clip_rect(crate::bench_rect(&format!("camera_{}",texture.camera_id.0)));
    static LAST_FRAME: [std::sync::atomic::AtomicI64; 32] = [const {std::sync::atomic::AtomicI64::new(i64::MIN)};32];
    if LAST_FRAME[usize::from(texture.camera_id.0)].swap(texture.arrival_time.0,std::sync::atomic::Ordering::Relaxed) != texture.arrival_time.0 {
        log::info!("GPU_BENCH_FRAME camera={} arrival={} rect={:?} size={:?}",texture.camera_id.0,texture.arrival_time.0,image_rect,texture.size);
    }''')
    replace(PATHS[3], '    let logical_size = ui.available_size().max(egui::vec2(1.0, 1.0));',
            '    let logical_size = crate::bench_rect("scene").size();\n    ui.set_clip_rect(crate::bench_rect("scene"));')
    replace(PATHS[3], '.add(egui::Image::new((input.texture_id, logical_size)).sense(egui::Sense::drag()))',
            '.put(crate::bench_rect("scene"),egui::Image::new((input.texture_id, logical_size)).sense(egui::Sense::drag()))')
    replace(PATHS[4], 'let rect = ui.available_rect_before_wrap();', 'let rect = crate::bench_rect("layout");')
    replace(PATHS[5], '.default_width(260.0)', '.default_width(320.0)')
    replace(PATHS[6], '                    ui.collapsing(', '                    egui::CollapsingHeader::new(')
    replace(PATHS[6], '''                        ),
                        |ui| show_container(ui, message, &child, state, visible, budget),
                    );''', '''                        ))
                        .default_open(std::env::var("MCAP_BENCH_RAW_EXPAND").as_deref() == Ok("1") && field.name == "ranges")
                        .show(ui, |ui| show_container(ui, message, &child, state, visible, budget));''')
    source = ROOT/PATHS[7]
    text = source.read_text().replace('distance: 6.0,','distance: 20.0,').replace('azimuth: 0.699,','azimuth: 45.0_f32.to_radians(),').replace('elevation: 0.48,','elevation: 30.0_f32.to_radians(),')
    text = text.replace('perspective_rh(48.0_f32.to_radians(), aspect, 0.01, 160.0)','perspective_rh(45.0_f32.to_radians(), aspect, 0.5, 5000.0)').replace('for meter in -50_i32..=50','for meter in -5_i32..=5').replace('for step in -50_i32..50','for step in -5_i32..5')
    start = text.index('        let color = if meter == 0 {')
    end = text.index('        // Short metric segments',start)
    text = text[:start]+'        let color = [0.017641954, 0.27049786, 1.0];\n'+text[end:]
    source.write_text(text)
    REPORT.mkdir(exist_ok=True)
    patch = ''.join(''.join(difflib.unified_diff(original[p].splitlines(True),(ROOT/p).read_text().splitlines(True),fromfile='a/'+p,tofile='b/'+p))for p in PATHS)
    (REPORT/'native-measurement.patch').write_text(patch)
    subprocess.run(['docker','compose','run','--rm','dev','bash','-lc',
                    'cargo build --release -p viewer-native -p performance-probe --locked && cp /home/dev/target/release/viewer-native target/gpu-native/viewer-native-workloads && cp /home/dev/target/release/performance-probe target/gpu-native/performance-probe'],cwd=ROOT,check=True,timeout=300)
finally:
    for path, text in original.items():
        (ROOT/path).write_text(text)
