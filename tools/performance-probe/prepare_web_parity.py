#!/usr/bin/env python3
"""Freeze main DOM and current egui sources with matched display raster sizes.

Only isolated copies are modified; physical geometry and scheduling are unchanged.
The current tree must be reviewed/clean for a published comparison. Every patch is saved.
"""
import argparse
import difflib
import io
from pathlib import Path
import shutil
import subprocess
import tarfile
from prepare_panel_sharing import SOURCE_PATHS


def prepare(destination, revision=None):
    destination.mkdir(parents=True, exist_ok=False)
    if revision:
        revision = subprocess.check_output(['git', 'rev-parse', '--verify', '--end-of-options', revision+'^{commit}'], text=True).strip()
        archive = subprocess.check_output(['git', 'archive', revision, *SOURCE_PATHS])
        with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
            tar.extractall(destination, filter='data')
    else:
        for relative in SOURCE_PATHS:
            source = Path(relative)
            if source.is_dir():
                shutil.copytree(source, destination/relative, ignore=shutil.ignore_patterns('target', 'dist', '__pycache__', '.git'))
            else:
                shutil.copy2(source, destination/relative)
        revision = subprocess.check_output(['git', 'rev-parse', 'HEAD'], text=True).strip()+' + working tree'
        (destination/'working-tree.patch').write_bytes(subprocess.check_output(['git', 'diff', '--', *SOURCE_PATHS]))
    patches = []

    def patch(relative, transform):
        path = destination/relative
        before = path.read_text()
        after = transform(before)
        if before == after:
            raise RuntimeError('Measurement patch failed: '+relative)
        path.write_text(after)
        patches.extend(difflib.unified_diff(before.splitlines(True), after.splitlines(True), fromfile='a/'+relative, tofile='b/'+relative))

    def replace_once(text, needle, value):
        if text.count(needle) != 1:
            raise RuntimeError('Source changed; review measurement patch: '+needle)
        return text.replace(needle, value)

    patch('apps/viewer-web/src/camera_bitmap.rs', lambda s: replace_once(s,
          'viewer_renderer::camera_target_size(source, target)',
          'viewer_renderer::camera_target_size(source, if target.0 > 256 || target.1 > 256 { (960, 540) } else { (112, 63) })'))
    if revision.endswith('working tree'):
        patch('apps/viewer-web/src/egui_app/gpu.rs', lambda s: replace_once(s,
            "pub fn render_scene(&mut self, size: (u32, u32), frame: SceneFrame<'_>) -> egui::TextureId {",
            "pub fn render_scene(&mut self, _size: (u32, u32), frame: SceneFrame<'_>) -> egui::TextureId {\n        let size = (960, 540);"))
    else:
        patch('apps/viewer-web/src/webgpu/host.rs', lambda s: replace_once(s,
            'let resized = self.physical_size != Some(size);',
            'let _ = size;\n        let size = PhysicalSize {width:960,height:540};\n        let resized = self.physical_size != Some(size);'))
        patch('apps/viewer-web/src/app/camera.rs', lambda s: replace_once(s,
            'presented_frames: BTreeMap<CameraId, viewer_core::CameraFrameMetadata>,',
            'pub(super) presented_frames: BTreeMap<CameraId, viewer_core::CameraFrameMetadata>,\n    pub(super) uploads_by_camera: BTreeMap<CameraId, u64>,').replace(
            'view.presented_frames.insert(id, frame);',
            '*view.uploads_by_camera.entry(id).or_default() += 1;\n                view.presented_frames.insert(id, frame);'))
        patch('apps/viewer-web/src/app.rs', lambda s: s+'''
// Read-only measurement instrumentation, absent from production main.
#[wasm_bindgen]
pub fn parity_status() -> String {
    APP.with(|cell| {
        let app = cell.borrow();
        let Some(session) = &app.playback else {return "{}".into()};
        let view = session.clock().view();
        serde_json::json!({"cursor":view.cursor.0.to_string(), "start":view.start.0.to_string(),
            "end":view.end.0.to_string(), "playing":view.playing,
            "load":format!("{:?}",session.load_state()), "camera":app.focused_camera.map(|id|id.0),
            "uploads_by_camera":app.view.camera.uploads_by_camera, "camera_count":app.view.camera.presented_frames.len(),
            "error":app.view.camera.errors}).to_string()
    })
}
''')
    (destination/'measurement.patch').write_text(''.join(patches))
    (destination/'source-revision.txt').write_text(revision+'\n')


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--main', default='main')
    p.add_argument('--output', type=Path, required=True)
    args = p.parse_args()
    prepare(args.output/'main', args.main)
    prepare(args.output/'egui')


if __name__ == '__main__':
    main()
