//! `spatiand_xr_v1`, on the host.
//!
//! An application that draws its own two eyes -- a VR game, through the OpenXR runtime in
//! `spatiand-openxr` -- says what it is by this protocol, exactly as it would on the headset's own
//! compositor. That is the point of the runtime speaking a protocol rather than a private channel:
//! the same library serves both places, and nothing in the application knows which one it is in.
//!
//! What the host does with it is what `appcontrol` already does for the viewers that cannot name a
//! surface: what an application says it is -- its eye layout, its layer -- becomes the
//! application's *presentation*, and the main loop tells the session, which claims the room through
//! its own `spatiand_xr_v1` on the other end. And the pose channel is handed out here from the same
//! ring the network thread writes the headset's viewports into.
//!
//! The application is found by the process that connected, as every window is. A game inside a
//! container still connects from a process the host can trace to the application it started.

use std::os::fd::AsFd;

use smithay::reexports::wayland_server::{
    Client, DataInit, Dispatch, DisplayHandle, GlobalDispatch, New, Resource,
};
use spatiand_proto::server::spatiand_xr_pose_channel_v1::{self, SpatiandXrPoseChannelV1};
use spatiand_proto::server::spatiand_xr_surface_v1::{
    self, EyeLayout as WireLayout, Layer as WireLayer, SpatiandXrSurfaceV1,
};
use spatiand_proto::server::spatiand_xr_v1::{self, SpatiandXrV1};
use spatiand_stream::{Eyes, Layer};

use crate::state::Host;

/// The version this host serves. The surface requests past layout and layer (fades, anchors) mean
/// nothing to a picture that is being streamed, and are accepted and ignored.
pub const VERSION: u32 = 5;

/// Which application an xr object belongs to, fixed when it was made.
pub struct Owner {
    app: String,
}

/// A pose channel somebody asked for, and the render size it was last told.
pub struct Channel {
    pub resource: SpatiandXrPoseChannelV1,
    pub told: Option<(u32, u32)>,
}

pub fn create_global(display: &DisplayHandle) {
    display.create_global::<Host, SpatiandXrV1, ()>(VERSION, ());
}

impl GlobalDispatch<SpatiandXrV1, ()> for Host {
    fn bind(
        _: &mut Self,
        _: &DisplayHandle,
        _: &Client,
        resource: New<SpatiandXrV1>,
        _: &(),
        data_init: &mut DataInit<'_, Self>,
    ) {
        data_init.init(resource, ());
    }
}

impl Dispatch<SpatiandXrV1, ()> for Host {
    fn request(
        host: &mut Self,
        client: &Client,
        _: &SpatiandXrV1,
        request: spatiand_xr_v1::Request,
        _: &(),
        _: &DisplayHandle,
        data_init: &mut DataInit<'_, Self>,
    ) {
        match request {
            spatiand_xr_v1::Request::GetXrSurface { id, .. } => {
                let app = host.app_for(Some(client));
                data_init.init(id, Owner { app });
            }
            spatiand_xr_v1::Request::GetPoseChannel { id } => {
                let channel = data_init.init(id, ());
                match host.pose_fd.as_ref().map(|fd| fd.try_clone()) {
                    Some(Ok(fd)) => {
                        channel.channel(fd.as_fd(), spatiand_proto::pose::channel_size() as u32);
                    }
                    _ => channel.unavailable("this host has no headset to take a head from".into()),
                }
                host.xr_channels.push(Channel {
                    resource: channel,
                    told: None,
                });
            }
            _ => {}
        }
    }
}

impl Dispatch<SpatiandXrSurfaceV1, Owner> for Host {
    fn request(
        host: &mut Self,
        _: &Client,
        resource: &SpatiandXrSurfaceV1,
        request: spatiand_xr_surface_v1::Request,
        owner: &Owner,
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
        match request {
            spatiand_xr_surface_v1::Request::SetEyeLayout { layout } => {
                let eyes = match layout.into_result() {
                    Ok(WireLayout::Mono) => Eyes::Mono,
                    Ok(WireLayout::SideBySide) => Eyes::SideBySide,
                    Ok(WireLayout::TopBottom) => Eyes::TopBottom,
                    _ => {
                        resource.post_error(spatiand_xr_v1::Error::BadLayout, "not an eye layout this version knows");
                        return;
                    }
                };
                crate::appcontrol::say_eyes(host, &owner.app, eyes);
            }
            spatiand_xr_surface_v1::Request::SetLayer { layer } => {
                let wanted = match layer.into_result() {
                    Ok(WireLayer::Window) => Layer::Window,
                    Ok(WireLayer::Projection) => Layer::Projection,
                    Ok(other) => {
                        // Refused out loud, as the protocol asks: silence is the yes. The
                        // session has no way to show these from a stream yet.
                        log::info!("layer_refused({other:?}): not carried over a stream yet");
                        resource.layer_refused(other, "this host cannot carry that layer".into());
                        return;
                    }
                    Err(_) => {
                        resource.post_error(spatiand_xr_v1::Error::BadLayer, "not a layer this version knows");
                        return;
                    }
                };
                crate::appcontrol::say_layer(host, &owner.app, wanted);
            }
            spatiand_xr_surface_v1::Request::SetCursorDrawn { enable } => {
                crate::appcontrol::say_cursor(host, &owner.app, enable != 0);
            }
            spatiand_xr_surface_v1::Request::SetFramePose { token, .. } => {
                // The viewport the picture was drawn for (stored plus one, see `pose::slot`).
                // The head itself is the session's to look up: it sent the viewport.
                if token == 0 {
                    host.frame_viewports.remove(&owner.app);
                } else {
                    host.frame_viewports.insert(owner.app.clone(), token - 1);
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<SpatiandXrPoseChannelV1, ()> for Host {
    fn request(
        _: &mut Self,
        _: &Client,
        _: &SpatiandXrPoseChannelV1,
        _: spatiand_xr_pose_channel_v1::Request,
        _: &(),
        _: &DisplayHandle,
        _: &mut DataInit<'_, Self>,
    ) {
    }

    fn destroyed(
        host: &mut Self,
        _: smithay::reexports::wayland_server::backend::ClientId,
        resource: &SpatiandXrPoseChannelV1,
        _: &(),
    ) {
        host.xr_channels.retain(|c| &c.resource != resource);
    }
}

/// Tell every pose channel the size the session wants an eye drawn at, where it has not been told.
///
/// `size` is both eyes together, as the session sends it; one eye is half the width.
pub fn tell_render_size(host: &mut Host, size: Option<(u32, u32)>) {
    let Some((width, height)) = size else { return };
    let eye = (width / 2, height);
    for channel in host.xr_channels.iter_mut() {
        if channel.told == Some(eye) || !channel.resource.is_alive() {
            continue;
        }
        if channel.resource.version() >= 2 {
            channel.resource.render_size(eye.0, eye.1);
        }
        channel.told = Some(eye);
    }
}
