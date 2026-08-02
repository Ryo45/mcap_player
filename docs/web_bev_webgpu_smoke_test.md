# Web BEV WebGPU smoke test

## Purpose

This spike verifies that the existing shared `bev-renderer` crate can run on
`wasm32-unknown-unknown` with WebGPU. It intentionally does not introduce a Web
workspace, panel runtime, or a second BEV drawing implementation.

## Boundary

The Web application continues to build the same platform-neutral `BevFrame`
with `BevFrameBuilder`. Browser-specific code owns the canvas surface and
presents the shared renderer's offscreen texture:

```text
DomainState
  -> BevFrameBuilder
  -> BevFrame
  -> shared BevRenderer
  -> BevRenderer::view()
  -> WebTexturePresenter
  -> WebGPU canvas surface
```

`WebGpuHost` owns the `wgpu::Instance`, canvas `Surface`, adapter, `Device`,
`Queue`, shared `BevRenderer`, and the Web-only presenter. It does not receive
the playback source, raw MCAP messages, camera canvas, or range-read spike.

## Rendering and allocation

`BevRenderer` remains unchanged. Its offscreen texture is sampled directly by a
fullscreen-triangle presenter; there is no CPU readback or `ImageData` copy.
The present pipeline is created once. The presenter bind group is recreated
only when `BevRenderer::resize()` replaces its output texture view. BEV path
uploads remain controlled by the existing domain revision.

The old Canvas 2D `draw_bev` implementation has been removed from the normal
Web path. Camera canvases remain Canvas 2D and are outside this spike.

## Resize and DPR

Each frame compares the canvas CSS client size and browser device-pixel ratio
with the configured physical size. A change updates the canvas backing size,
reconfigures the surface, resizes `BevRenderer`, and reconnects the presenter.
Zero-sized canvases are skipped. Each physical dimension is capped at 4096
pixels.

## Failure behavior

WebGPU initialization is asynchronous. If adapter/device/surface setup fails,
the BEV status reports the failure while camera playback and the range-read
spike remain usable. Lost or outdated surfaces are reconfigured; timeout skips
one frame; out-of-memory and other terminal surface errors disable only WebGPU
BEV rendering.

## Verification

The shared renderer and browser host compile for `wasm32-unknown-unknown`, the
Trunk bundle builds successfully, and the full native/workspace tests and
Clippy checks remain green. A synthetic frame is submitted after WebGPU
initialization so the GPU path can be checked before opening an MCAP file.

This spike does not add performance telemetry or a production fallback. A real
browser visual check is still required on a WebGPU-capable Chromium build.
