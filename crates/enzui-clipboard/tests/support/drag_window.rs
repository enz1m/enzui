//! The fixture compositor's drag extras, advertised only when
//! [`Config::drag_window`](super::compositor::Config) asks for them:
//! `wl_shm`, a bare `xdg_wm_base` that never configures, and
//! `xdg_toplevel_drag_manager_v1`. It records the icon surface a drag starts
//! with, as committed, and every window attached to a toplevel drag.

use std::collections::HashMap;
use std::os::fd::{AsFd, OwnedFd};
use std::sync::Arc;

use wayland_protocols::xdg::shell::server::xdg_surface::{self, XdgSurface};
use wayland_protocols::xdg::shell::server::xdg_toplevel::{self, XdgToplevel};
use wayland_protocols::xdg::shell::server::xdg_wm_base::{self, XdgWmBase};
use wayland_protocols::xdg::toplevel_drag::v1::server::xdg_toplevel_drag_manager_v1::{
    self, XdgToplevelDragManagerV1,
};
use wayland_protocols::xdg::toplevel_drag::v1::server::xdg_toplevel_drag_v1::{
    self, XdgToplevelDragV1,
};
use wayland_server::backend::ObjectId;
use wayland_server::protocol::wl_buffer::{self, WlBuffer};
use wayland_server::protocol::wl_shm::{self, WlShm};
use wayland_server::protocol::wl_shm_pool::{self, WlShmPool};
use wayland_server::protocol::wl_surface::{self, WlSurface};
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource};

use super::Server;

/// A drag icon as the compositor would show it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IconSeen {
    pub width: i32,
    pub height: i32,
    /// The surface offset: minus the hotspot, in logical pixels.
    pub offset: (i32, i32),
    pub scale: i32,
    /// The first pixel's bytes as stored (B, G, R, A, premultiplied).
    pub first_pixel: [u8; 4],
}

/// One buffer's size and first pixel.
#[derive(Debug, Clone, Copy)]
pub(crate) struct BufferInfo {
    width: i32,
    height: i32,
    first_pixel: [u8; 4],
}

/// A surface's pending and committed state.
#[derive(Debug, Default, Clone)]
pub(crate) struct SurfaceSeen {
    buffer: Option<BufferInfo>,
    offset: (i32, i32),
    scale: i32,
    committed: Option<(BufferInfo, (i32, i32), i32)>,
}

#[derive(Default)]
pub(crate) struct DragWindowRecord {
    surfaces: HashMap<ObjectId, SurfaceSeen>,
    icon: Option<ObjectId>,
    pub(crate) attached: Vec<(String, i32, i32)>,
}

impl DragWindowRecord {
    pub(crate) fn started_with_icon(&mut self, icon: Option<&WlSurface>) {
        self.icon = icon.map(Resource::id);
    }

    pub(crate) fn icon(&self) -> Option<IconSeen> {
        let (buffer, offset, scale) = self.surfaces.get(self.icon.as_ref()?)?.committed?;
        Some(IconSeen {
            width: buffer.width,
            height: buffer.height,
            offset,
            scale,
            first_pixel: buffer.first_pixel,
        })
    }

    pub(crate) fn surface_request(&mut self, surface: &WlSurface, request: &wl_surface::Request) {
        let seen = self
            .surfaces
            .entry(surface.id())
            .or_insert_with(|| SurfaceSeen {
                scale: 1,
                ..SurfaceSeen::default()
            });
        match request {
            wl_surface::Request::Attach { buffer, x, y } => {
                seen.buffer = buffer
                    .as_ref()
                    .and_then(|buffer| buffer.data::<BufferInfo>().copied());
                seen.offset = (seen.offset.0 + x, seen.offset.1 + y);
            }
            wl_surface::Request::Offset { x, y } => {
                seen.offset = (seen.offset.0 + x, seen.offset.1 + y);
            }
            wl_surface::Request::SetBufferScale { scale } => seen.scale = *scale,
            wl_surface::Request::Commit => {
                if let Some(buffer) = seen.buffer {
                    seen.committed = Some((buffer, seen.offset, seen.scale));
                }
            }
            _ => {}
        }
    }
}

macro_rules! bind_global {
    ($interface:ty) => {
        impl GlobalDispatch<$interface, ()> for Server {
            fn bind(
                _: &mut Self,
                _: &DisplayHandle,
                _: &Client,
                resource: New<$interface>,
                (): &(),
                data_init: &mut DataInit<'_, Self>,
            ) {
                data_init.init(resource, ());
            }
        }
    };
}

bind_global!(WlShm);
bind_global!(XdgWmBase);
bind_global!(XdgToplevelDragManagerV1);

impl Dispatch<WlShm, ()> for Server {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &WlShm,
        request: wl_shm::Request,
        (): &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let wl_shm::Request::CreatePool { id, fd, .. } = request {
            data_init.init(id, Arc::new(fd));
        }
    }
}

impl Dispatch<WlShmPool, Arc<OwnedFd>> for Server {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &WlShmPool,
        request: wl_shm_pool::Request,
        fd: &Arc<OwnedFd>,
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let wl_shm_pool::Request::CreateBuffer {
            id,
            offset,
            width,
            height,
            ..
        } = request
        {
            let mut first_pixel = [0; 4];
            let _ = rustix::io::pread(fd.as_fd(), &mut first_pixel, offset as u64);
            data_init.init(
                id,
                BufferInfo {
                    width,
                    height,
                    first_pixel,
                },
            );
        }
    }
}

impl Dispatch<WlBuffer, BufferInfo> for Server {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &WlBuffer,
        _: wl_buffer::Request,
        _: &BufferInfo,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }
}

impl Dispatch<XdgWmBase, ()> for Server {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &XdgWmBase,
        request: xdg_wm_base::Request,
        (): &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let xdg_wm_base::Request::GetXdgSurface { id, .. } = request {
            data_init.init(id, ());
        }
    }
}

impl Dispatch<XdgSurface, ()> for Server {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &XdgSurface,
        request: xdg_surface::Request,
        (): &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let xdg_surface::Request::GetToplevel { id } = request {
            data_init.init(id, std::sync::Mutex::new(String::new()));
        }
    }
}

impl Dispatch<XdgToplevel, std::sync::Mutex<String>> for Server {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &XdgToplevel,
        request: xdg_toplevel::Request,
        title: &std::sync::Mutex<String>,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let xdg_toplevel::Request::SetTitle { title: new } = request
            && let Ok(mut title) = title.lock()
        {
            *title = new;
        }
    }
}

impl Dispatch<XdgToplevelDragManagerV1, ()> for Server {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &XdgToplevelDragManagerV1,
        request: xdg_toplevel_drag_manager_v1::Request,
        (): &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let xdg_toplevel_drag_manager_v1::Request::GetXdgToplevelDrag { id, .. } = request {
            data_init.init(id, ());
        }
    }
}

impl Dispatch<XdgToplevelDragV1, ()> for Server {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &XdgToplevelDragV1,
        request: xdg_toplevel_drag_v1::Request,
        (): &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let xdg_toplevel_drag_v1::Request::Attach {
            toplevel,
            x_offset,
            y_offset,
        } = request
        {
            let title = toplevel
                .data::<std::sync::Mutex<String>>()
                .and_then(|title| title.lock().ok().map(|title| title.clone()))
                .unwrap_or_default();
            state.drag_window.attached.push((title, x_offset, y_offset));
        }
    }
}
