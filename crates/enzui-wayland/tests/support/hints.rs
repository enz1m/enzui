//! The fixture's surface hint globals: content type and tearing control.

use wayland_protocols::wp::content_type::v1::server::wp_content_type_manager_v1::{
    self, WpContentTypeManagerV1,
};
use wayland_protocols::wp::content_type::v1::server::wp_content_type_v1::{self, WpContentTypeV1};
use wayland_protocols::wp::tearing_control::v1::server::wp_tearing_control_manager_v1::{
    self, WpTearingControlManagerV1,
};
use wayland_protocols::wp::tearing_control::v1::server::wp_tearing_control_v1::{
    self, WpTearingControlV1,
};
use wayland_server::{Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, WEnum};

use super::{Comp, Seen};

fn value<T: Into<u32>>(value: WEnum<T>) -> u32 {
    match value {
        WEnum::Value(value) => value.into(),
        WEnum::Unknown(value) => value,
    }
}

impl GlobalDispatch<WpContentTypeManagerV1, ()> for Comp {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<WpContentTypeManagerV1>,
        (): &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<WpContentTypeManagerV1, ()> for Comp {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &WpContentTypeManagerV1,
        request: wp_content_type_manager_v1::Request,
        (): &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let wp_content_type_manager_v1::Request::GetSurfaceContentType { id, .. } = request {
            data_init.init(id, ());
            state
                .0
                .lock()
                .unwrap()
                .seen
                .push(Seen::Object("content type"));
        }
    }
}

impl Dispatch<WpContentTypeV1, ()> for Comp {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &WpContentTypeV1,
        request: wp_content_type_v1::Request,
        (): &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let wp_content_type_v1::Request::SetContentType { content_type } = request {
            let seen = Seen::ContentType(value(content_type));
            state.0.lock().unwrap().seen.push(seen);
        }
    }
}

impl GlobalDispatch<WpTearingControlManagerV1, ()> for Comp {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<WpTearingControlManagerV1>,
        (): &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<WpTearingControlManagerV1, ()> for Comp {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &WpTearingControlManagerV1,
        request: wp_tearing_control_manager_v1::Request,
        (): &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        if let wp_tearing_control_manager_v1::Request::GetTearingControl { id, .. } = request {
            data_init.init(id, ());
            state
                .0
                .lock()
                .unwrap()
                .seen
                .push(Seen::Object("tearing control"));
        }
    }
}

impl Dispatch<WpTearingControlV1, ()> for Comp {
    fn request(
        state: &mut Self,
        _: &Client,
        _: &WpTearingControlV1,
        request: wp_tearing_control_v1::Request,
        (): &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        if let wp_tearing_control_v1::Request::SetPresentationHint { hint } = request {
            state
                .0
                .lock()
                .unwrap()
                .seen
                .push(Seen::Tearing(value(hint)));
        }
    }
}
