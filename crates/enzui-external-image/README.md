# enzui-external-image

Draws frames another thread or process rendered (a web engine, a video
decoder, a camera) straight from GPU memory. Each frame arrives as a DMA-BUF:
plane file descriptors, offsets and strides, the DRM format and modifier, and
an allocation id. The renderer imports it on its own Vulkan device and samples
it in place; nothing is copied through the CPU.

```rust,ignore
let frame = ExternalFrame::new(dma_buf, Lease::new(move || give_back(allocation)));
let view = enz_external_image(&frame).fallback(cpu_copy);
```

- The lease is handed back once nothing shows or samples the frame: drop the
  frame when a newer one arrives, and the renderer lets go after the GPU has
  finished with it.
- Images are centered by default. Use `content_fit(ContentFit::None)` with
  `align_x(Horizontal::Left)` and `align_y(Vertical::Top)` for native-size
  content fixed to the viewport origin. Output scaling applies to image
  dimensions; both GPU frames and CPU fallbacks clip at the widget bounds.
- Imports are cached by allocation, up to `CACHED_ALLOCATIONS`;
  `imports_made()` shows a cycled pool is imported once per buffer.
- `support()` lists the importable formats and modifiers (single-plane
  layouts); a refused frame says why through `ExternalFrame::status` and
  shows its fallback, as does the software renderer.
- `Support::device` names the DRM device those formats were queried on
  (primary and render node numbers from `VK_EXT_physical_device_drm`;
  `None` when the driver does not say). `Support::compare_node(fd)` compares
  it with the node a producer allocates on, from the descriptor alone, as
  `Same`, `Different` or `Unknown`; treat `Unknown` like `Different`. A
  successful import does not prove the frame was not copied.
- Before the first frame, `enz_external_image_probe().on_support(..)` (an
  invisible widget) makes the renderer read what its own device imports and
  reports it once, so a producer can allocate its first buffer in a format
  and modifier that imports: `None` when nothing imports. Mount it with
  `.over(content)`: it has no size, and a row or column never draws it.

The `dma_buf` tests allocate real driver buffers through GBM on the render
node and check a pool's reuse and leases and a refused frame's fallback; they
skip without a render node or an importing device.
