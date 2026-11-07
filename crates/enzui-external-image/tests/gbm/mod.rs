//! Allocates DMA-BUF test buffers through GBM on the first render node.
//! GBM fixture support for the native `dma_buf` tests.

#![allow(unsafe_code, reason = "GBM allocates the buffers through its C API")]

use enzui_external_image::{DmaBuf, Plane, fourcc};
use std::os::fd::{FromRawFd, OwnedFd};

mod ffi {
    use std::ffi::{c_int, c_void};

    #[repr(C)]
    pub(crate) struct Device(c_void);
    #[repr(C)]
    pub(crate) struct Bo(c_void);

    pub(crate) const USE_RENDERING: u32 = 1 << 2;
    pub(crate) const USE_LINEAR: u32 = 1 << 4;
    pub(crate) const TRANSFER_WRITE: u32 = 1 << 1;

    #[link(name = "gbm")]
    unsafe extern "C" {
        pub(crate) fn gbm_create_device(fd: c_int) -> *mut Device;
        pub(crate) fn gbm_device_destroy(device: *mut Device);
        pub(crate) fn gbm_bo_create(
            device: *mut Device,
            width: u32,
            height: u32,
            format: u32,
            flags: u32,
        ) -> *mut Bo;
        pub(crate) fn gbm_bo_create_with_modifiers2(
            device: *mut Device,
            width: u32,
            height: u32,
            format: u32,
            modifiers: *const u64,
            count: u32,
            flags: u32,
        ) -> *mut Bo;
        pub(crate) fn gbm_bo_map(
            bo: *mut Bo,
            x: u32,
            y: u32,
            width: u32,
            height: u32,
            flags: u32,
            stride: *mut u32,
            map_data: *mut *mut c_void,
        ) -> *mut c_void;
        pub(crate) fn gbm_bo_unmap(bo: *mut Bo, map_data: *mut c_void);
        pub(crate) fn gbm_bo_get_fd_for_plane(bo: *mut Bo, plane: c_int) -> c_int;
        pub(crate) fn gbm_bo_get_stride_for_plane(bo: *mut Bo, plane: c_int) -> u32;
        pub(crate) fn gbm_bo_get_offset(bo: *mut Bo, plane: c_int) -> u32;
        pub(crate) fn gbm_bo_get_modifier(bo: *mut Bo) -> u64;
        pub(crate) fn gbm_bo_get_plane_count(bo: *mut Bo) -> c_int;
        pub(crate) fn gbm_bo_destroy(bo: *mut Bo);
    }
}

/// The GBM device on the first render node.
pub(crate) struct Gbm {
    device: *mut ffi::Device,
    _node: std::fs::File,
}

impl Gbm {
    pub(crate) fn open() -> Option<Self> {
        let node = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open("/dev/dri/renderD128")
            .ok()?;
        // SAFETY: a live render-node descriptor, kept open as long as the device.
        let device = unsafe { ffi::gbm_create_device(std::os::fd::AsRawFd::as_raw_fd(&node)) };
        (!device.is_null()).then_some(Self {
            device,
            _node: node,
        })
    }

    /// A `width` x `height` ARGB8888 buffer laid out as `modifier` (linear
    /// when `None`), filled by `pixel(x, y)` as B, G, R, A bytes.
    pub(crate) fn buffer(
        &self,
        width: u32,
        height: u32,
        modifier: Option<u64>,
        allocation: u64,
        pixel: impl Fn(u32, u32) -> [u8; 4],
    ) -> Option<DmaBuf> {
        // SAFETY: GBM calls on the live device; the buffer object is
        // destroyed before returning, after its descriptor was exported (the
        // descriptor keeps the memory alive).
        unsafe {
            let bo = match modifier {
                Some(modifier) => ffi::gbm_bo_create_with_modifiers2(
                    self.device,
                    width,
                    height,
                    fourcc::ARGB8888,
                    &modifier,
                    1,
                    ffi::USE_RENDERING,
                ),
                None => ffi::gbm_bo_create(
                    self.device,
                    width,
                    height,
                    fourcc::ARGB8888,
                    ffi::USE_RENDERING | ffi::USE_LINEAR,
                ),
            };
            if bo.is_null() || ffi::gbm_bo_get_plane_count(bo) != 1 {
                return None;
            }
            let mut stride = 0;
            let mut map_data = std::ptr::null_mut();
            let mapped = ffi::gbm_bo_map(
                bo,
                0,
                0,
                width,
                height,
                ffi::TRANSFER_WRITE,
                &mut stride,
                &mut map_data,
            )
            .cast::<u8>();
            if mapped.is_null() {
                ffi::gbm_bo_destroy(bo);
                return None;
            }
            for y in 0..height {
                for x in 0..width {
                    let at = mapped.add((y * stride + x * 4) as usize);
                    std::ptr::copy_nonoverlapping(pixel(x, y).as_ptr(), at, 4);
                }
            }
            ffi::gbm_bo_unmap(bo, map_data);
            let fd = ffi::gbm_bo_get_fd_for_plane(bo, 0);
            let buffer = DmaBuf {
                width,
                height,
                format: fourcc::ARGB8888,
                modifier: ffi::gbm_bo_get_modifier(bo),
                planes: vec![Plane::new(
                    OwnedFd::from_raw_fd(fd),
                    ffi::gbm_bo_get_offset(bo, 0),
                    ffi::gbm_bo_get_stride_for_plane(bo, 0),
                )],
                allocation,
            };
            ffi::gbm_bo_destroy(bo);
            Some(buffer)
        }
    }
}

impl Drop for Gbm {
    fn drop(&mut self) {
        // SAFETY: the device was created by `open` and is destroyed once.
        unsafe { ffi::gbm_device_destroy(self.device) };
    }
}
