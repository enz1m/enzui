//! What an outgoing drag shows: the icon surface under the pointer and the
//! window a toplevel drag moves with it.

use super::*;
use crate::drag::DragIcon;
use rustix::fs::{MemfdFlags, memfd_create};
use wayland_client::protocol::wl_buffer::{self, WlBuffer};
use wayland_client::protocol::wl_compositor::{self, WlCompositor};
use wayland_client::protocol::wl_shm::{self, Format, WlShm};
use wayland_client::protocol::wl_shm_pool::{self, WlShmPool};
use wayland_client::protocol::wl_surface::{self, WlSurface};
use wayland_protocols::xdg::toplevel_drag::v1::client::xdg_toplevel_drag_manager_v1::{
    self, XdgToplevelDragManagerV1,
};
use wayland_protocols::xdg::toplevel_drag::v1::client::xdg_toplevel_drag_v1::{
    self, XdgToplevelDragV1,
};

/// The optional globals a drag source uses beyond the data device.
#[derive(Default)]
pub(super) struct DragGlobals {
    compositor: Option<WlCompositor>,
    shm: Option<WlShm>,
    toplevel_drags: Option<XdgToplevelDragManagerV1>,
}

impl DragGlobals {
    pub(super) fn bind(
        globals: &wayland_client::globals::GlobalList,
        qh: &QueueHandle<State>,
    ) -> Self {
        Self {
            compositor: globals.bind(qh, 1..=5, ()).ok(),
            shm: globals.bind(qh, 1..=1, ()).ok(),
            toplevel_drags: globals.bind(qh, 1..=1, ()).ok(),
        }
    }

    /// The toplevel drag for `source`, which must not have started yet.
    pub(super) fn toplevel_drag(
        &self,
        source: &WlDataSource,
        qh: &QueueHandle<State>,
    ) -> Option<XdgToplevelDragV1> {
        Some(
            self.toplevel_drags
                .as_ref()?
                .get_xdg_toplevel_drag(source, qh, ()),
        )
    }
}

/// The surface showing a drag icon, and the buffer holding its pixels.
pub(super) struct IconSurface {
    pub(super) surface: WlSurface,
    buffer: WlBuffer,
}

impl IconSurface {
    /// A surface showing `icon` with its hotspot at the surface origin; the
    /// caller hands it to `start_drag` and then calls [`Self::commit`].
    /// `None` when the compositor offers no `wl_compositor` or `wl_shm`, or
    /// shared memory cannot be made; the drag then goes ahead without one.
    pub(super) fn create(
        globals: &DragGlobals,
        icon: &DragIcon,
        qh: &QueueHandle<State>,
    ) -> Option<Self> {
        let compositor = globals.compositor.as_ref()?;
        let shm = globals.shm.as_ref()?;
        let width = i32::try_from(icon.width).ok()?;
        let height = i32::try_from(icon.height).ok()?;
        let stride = width.checked_mul(4)?;
        let size = stride.checked_mul(height)?;
        let memory = memfd_create("enzui-drag-icon", MemfdFlags::CLOEXEC).ok()?;
        rustix::io::write(&memory, &icon.argb).ok()?;
        let pool = shm.create_pool(memory.as_fd(), size, qh, ());
        let buffer = pool.create_buffer(0, width, height, stride, Format::Argb8888, qh, ());
        pool.destroy();
        let surface = compositor.create_surface(qh, ());
        let scale = i32::try_from(icon.scale).ok()?;
        if surface.version() >= 3 {
            surface.set_buffer_scale(scale);
        }
        // The hotspot, in surface-local logical pixels, sits at the pointer.
        let hotspot_x = -icon.hotspot.0 / scale;
        let hotspot_y = -icon.hotspot.1 / scale;
        if surface.version() >= 5 {
            surface.attach(Some(&buffer), 0, 0);
            surface.offset(hotspot_x, hotspot_y);
        } else {
            surface.attach(Some(&buffer), hotspot_x, hotspot_y);
        }
        surface.damage_buffer(0, 0, width, height);
        Some(Self { surface, buffer })
    }

    /// Applies the icon; the surface has its drag-icon role by now.
    pub(super) fn commit(&self) {
        self.surface.commit();
    }

    pub(super) fn destroy(self) {
        self.surface.destroy();
        self.buffer.destroy();
    }
}

impl Dispatch<WlCompositor, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlCompositor,
        _: wl_compositor::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlShm, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlShm,
        _: wl_shm::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlShmPool, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlShmPool,
        _: wl_shm_pool::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlBuffer, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlBuffer,
        _: wl_buffer::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<WlSurface, ()> for State {
    fn event(
        _: &mut Self,
        _: &WlSurface,
        _: wl_surface::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<XdgToplevelDragManagerV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &XdgToplevelDragManagerV1,
        _: xdg_toplevel_drag_manager_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<XdgToplevelDragV1, ()> for State {
    fn event(
        _: &mut Self,
        _: &XdgToplevelDragV1,
        _: xdg_toplevel_drag_v1::Event,
        (): &(),
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}
