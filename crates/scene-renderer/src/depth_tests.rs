use super::{SceneCameraMode, SceneFrame, SceneRenderer, camera_pose};

#[test]
#[ignore = "requires a working GPU adapter; run explicitly on Native"]
fn opaque_points_occlude_by_distance_in_both_orders() {
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    eprintln!("Depth regression adapter: {:?}", adapter.get_info());
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let mut renderer = SceneRenderer::new(&device, 64, 64);
    let frame = SceneFrame::default();
    let (eye, _) = camera_pose(SceneCameraMode::Free, renderer.camera(), frame);
    // Both points lie on the centre ray, above the grid. Convert renderer xyz to ROS xyz.
    let near = [-eye[2] * 0.4, -eye[0] * 0.4, eye[1] * 0.4];
    let far = [-eye[2] * 0.2, -eye[0] * 0.2, eye[1] * 0.2];
    for (revision, (points, colors)) in [
        ([near, far], [[1.0, 0.0, 0.0, 1.0], [0.0, 0.0, 1.0, 1.0]]),
        ([far, near], [[0.0, 0.0, 1.0, 1.0], [1.0, 0.0, 0.0, 1.0]]),
    ]
    .into_iter()
    .enumerate()
    {
        renderer.render(
            &device,
            &queue,
            SceneFrame {
                revision: revision as u64,
                cloud_revision: revision as u64,
                cloud: &points,
                cloud_colors: &colors,
                ..frame
            },
        );
        let readback = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("depth regression readback"),
            size: 64 * 256,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        let mut encoder = device.create_command_encoder(&Default::default());
        encoder.copy_texture_to_buffer(
            renderer.texture.as_image_copy(),
            wgpu::TexelCopyBufferInfo {
                buffer: &readback,
                layout: wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(256),
                    rows_per_image: None,
                },
            },
            wgpu::Extent3d {
                width: 64,
                height: 64,
                depth_or_array_layers: 1,
            },
        );
        queue.submit([encoder.finish()]);
        let (sender, receiver) = std::sync::mpsc::channel();
        readback
            .slice(..)
            .map_async(wgpu::MapMode::Read, move |result| {
                sender.send(result).unwrap();
            });
        device.poll(wgpu::PollType::wait_indefinitely()).unwrap();
        receiver.recv().unwrap().unwrap();
        let bytes = readback.slice(..).get_mapped_range();
        let offset = 32 * 256 + 32 * 4;
        assert_eq!(&bytes[offset..offset + 4], &[255, 0, 0, 255]);
        drop(bytes);
        readback.unmap();
    }
}

#[test]
#[ignore = "requires a working GPU adapter; run explicitly on Native"]
fn extension_updates_do_not_upload_existing_layers() {
    use viewer_core::{SceneBox, SceneLine, SceneObjectId, SceneOverlay};
    let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    eprintln!("Overlay regression adapter: {:?}", adapter.get_info());
    let (device, queue) = pollster::block_on(adapter.request_device(&Default::default())).unwrap();
    let mut renderer = SceneRenderer::new(&device, 64, 64);
    let overlay = SceneOverlay::new(
        vec![SceneLine {
            id: SceneObjectId(1),
            start: [0., 0., 1.],
            end: [1., 0., 1.],
            color: [1., 0., 0.],
        }],
        vec![SceneBox {
            id: SceneObjectId(2),
            center: [0., 0., 0.5],
            orientation: [0., 0., 0., 1.],
            size: [1., 0.5, 1.],
            color: [0., 1., 0.],
        }],
    )
    .unwrap();
    let base = SceneFrame {
        cloud: &[[1., 0., 0.]],
        path: &[[0., 0., 0.], [1., 0., 0.]],
        ..Default::default()
    };
    renderer.render(&device, &queue, base);
    let initial = renderer.metrics();
    let frame = SceneFrame {
        overlay: Some(&overlay),
        overlay_revision: 1,
        ..base
    };
    assert!(renderer.needs_render(frame));
    renderer.render(&device, &queue, frame);
    assert_eq!(renderer.overlay_vertices, 26);
    assert_eq!(
        renderer.metrics().overlay_uploads,
        initial.overlay_uploads + 1
    );
    assert_eq!(renderer.metrics().layer_uploads, initial.layer_uploads);
    assert_eq!(renderer.metrics().point_uploads, initial.point_uploads);
    assert!(!renderer.needs_render(frame));
    renderer.orbit(1., 1.);
    renderer.render(&device, &queue, frame);
    assert_eq!(
        renderer.metrics().overlay_uploads,
        initial.overlay_uploads + 1
    );
    let removed = SceneFrame {
        overlay_revision: 2,
        ..base
    };
    renderer.render(&device, &queue, removed);
    assert_eq!(renderer.overlay_vertices, 0);
    assert_eq!(
        renderer.metrics().overlay_uploads,
        initial.overlay_uploads + 1
    );
    assert_eq!(renderer.metrics().point_uploads, initial.point_uploads);
}
