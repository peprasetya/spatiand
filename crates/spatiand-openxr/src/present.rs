//! `xrEndFrame`: from the application's eye images to a picture in the room.
//!
//! The first projection layer is taken, its two eyes are copied side by side into one linear
//! image whose memory is exported as a dmabuf, and the compositor is shown that. Everything else
//! -- quad layers, a second projection layer, depth -- is not composed yet, and said so once in the
//! log rather than every frame.
//!
//! Three frames rotate. A frame is reused only once the compositor has released its `wl_buffer`,
//! because drawing into one still being read is a tear; if all three are held, this frame is
//! dropped, which is what a compositor that cannot keep up is owed.

use openxr_sys::{Result as R, StructureType};

use crate::object::*;
use crate::vk::{self, EyeCopy};

/// How many frames are in flight between the application and the compositor.
const FRAMES: usize = 3;

/// Whether a rectangle of an image is in the image, and has an area.
fn rect_is_inside(rect: &openxr_sys::Rect2Di, width: u32, height: u32) -> bool {
    rect.offset.x >= 0
        && rect.offset.y >= 0
        && rect.extent.width > 0
        && rect.extent.height > 0
        && (rect.offset.x as i64 + rect.extent.width as i64) <= width as i64
        && (rect.offset.y as i64 + rect.extent.height as i64) <= height as i64
}

/// A pose's orientation has to be a unit quaternion.
fn pose_is_valid(pose: &openxr_sys::Posef) -> bool {
    let q = pose.orientation;
    let length_squared = q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w;
    (length_squared - 1.0).abs() <= 0.01 && pose.position.x.is_finite() && pose.position.y.is_finite() && pose.position.z.is_finite()
}

/// What `xrEndFrame` owes an application that submits a layer that cannot be used: before anything is
/// composed, so a frame with one bad layer shows none of it.
pub unsafe fn validate_layers(layers: &[*const openxr_sys::CompositionLayerBaseHeader]) -> R {
    for layer in layers {
        let header = &**layer;
        if header.ty == StructureType::COMPOSITION_LAYER_PROJECTION {
            let p = &*(*layer as *const openxr_sys::CompositionLayerProjection);
            if SPACES.get(openxr_sys::Handle::into_raw(p.space)).is_none() {
                return R::ERROR_HANDLE_INVALID;
            }
            if p.view_count != 2 || p.views.is_null() {
                return R::ERROR_VALIDATION_FAILURE;
            }
            for view in std::slice::from_raw_parts(p.views, 2) {
                let Some(swapchain) = SWAPCHAINS.get(view.sub_image.swapchain.into_raw_handle()) else {
                    return R::ERROR_HANDLE_INVALID;
                };
                if swapchain.state.lock().unwrap().released.is_none() {
                    return R::ERROR_LAYER_INVALID;
                }
                if !pose_is_valid(&view.pose) {
                    return R::ERROR_POSE_INVALID;
                }
                if !rect_is_inside(&view.sub_image.image_rect, swapchain.width, swapchain.height) {
                    return R::ERROR_SWAPCHAIN_RECT_INVALID;
                }
                if view.sub_image.image_array_index >= swapchain.array_size {
                    return R::ERROR_VALIDATION_FAILURE;
                }
            }
        } else if header.ty == StructureType::COMPOSITION_LAYER_QUAD {
            let q = &*(*layer as *const openxr_sys::CompositionLayerQuad);
            if SPACES.get(openxr_sys::Handle::into_raw(q.space)).is_none() {
                return R::ERROR_HANDLE_INVALID;
            }
            let Some(swapchain) = SWAPCHAINS.get(q.sub_image.swapchain.into_raw_handle()) else {
                return R::ERROR_HANDLE_INVALID;
            };
            if swapchain.state.lock().unwrap().released.is_none() {
                return R::ERROR_LAYER_INVALID;
            }
            if !pose_is_valid(&q.pose) {
                return R::ERROR_POSE_INVALID;
            }
            if !rect_is_inside(&q.sub_image.image_rect, swapchain.width, swapchain.height) {
                return R::ERROR_SWAPCHAIN_RECT_INVALID;
            }
            if q.sub_image.image_array_index >= swapchain.array_size {
                return R::ERROR_VALIDATION_FAILURE;
            }
            // Nothing is drawn of a quad with no size, which is still a quad.
            if !(q.size.width >= 0.0 && q.size.height >= 0.0) {
                return R::ERROR_VALIDATION_FAILURE;
            }
        }
    }
    R::SUCCESS
}

pub unsafe fn submit(
    session: &Session,
    layers: &[*const openxr_sys::CompositionLayerBaseHeader],
    display_time_ns: i64,
) -> R {
    let (Some(vkd), Some(link)) = (&session.vk, &session.link) else {
        return R::SUCCESS;
    };
    // Vulkan reads what OpenGL drew; nothing tells it OpenGL has finished.
    if let Some(gl) = &session.gl {
        gl.finish();
    }
    let mut projection = None;
    let mut quad_layers = Vec::new();
    for layer in layers {
        let header = &**layer;
        if header.ty == StructureType::COMPOSITION_LAYER_PROJECTION {
            if projection.is_none() {
                projection = Some(*layer as *const openxr_sys::CompositionLayerProjection);
            }
        } else if header.ty == StructureType::COMPOSITION_LAYER_QUAD {
            quad_layers.push(*layer as *const openxr_sys::CompositionLayerQuad);
        } else {
            let mut run = session.run.lock().unwrap();
            if !run.warned_layers {
                run.warned_layers = true;
                log::warn!("a composition layer of type {:?} is not composed yet", header.ty);
            }
        }
    }
    if projection.is_none() && quad_layers.is_empty() {
        return R::SUCCESS;
    }
    let projection_ptr = projection;
    let views: &[openxr_sys::CompositionLayerProjectionView] = match projection {
        Some(p) => {
            let p = &*p;
            if p.view_count != 2 {
                return R::ERROR_VALIDATION_FAILURE;
            }
            std::slice::from_raw_parts(p.views, 2)
        }
        None => &[],
    };

    let mut copies = Vec::with_capacity(2);
    let mut format = None;
    for (eye, view) in views.iter().enumerate() {
        let Some(swapchain) = SWAPCHAINS.get(view.sub_image.swapchain.into_raw_handle()) else {
            return R::ERROR_HANDLE_INVALID;
        };
        let Some(index) = swapchain.state.lock().unwrap().released else {
            return R::ERROR_LAYER_INVALID;
        };
        let rect = view.sub_image.image_rect;
        format.get_or_insert(swapchain.format);
        let Some((image, layer)) = swapchain.vulkan_image(index, view.sub_image.image_array_index) else {
            return R::ERROR_LAYER_INVALID;
        };
        copies.push(EyeCopy {
            image,
            layer,
            rect: (
                rect.offset.x,
                rect.offset.y,
                rect.extent.width.max(1) as u32,
                rect.extent.height.max(1) as u32,
            ),
            eye: eye as u32,
            layout: swapchain.layout(),
            flip_y: swapchain.is_gl(),
        });
    }
    // With no projection layer there is no picture size to take from the application: a quad-only
    // application is shown at the size the headset wants an eye, as a game that draws only a
    // menu would be.
    let eye_size = copies.first().map(|c| (c.rect.2, c.rect.3)).unwrap_or(crate::api::recommended_eye_size());
    let mut quads = Vec::new();
    let mut placed = Vec::new();
    for quad in quad_layers {
        if let Some(draw) = quad_draw(session, &*quad, display_time_ns, &mut placed) {
            format.get_or_insert(draw.format);
            quads.push(draw);
        }
    }
    let format = format.unwrap_or(ash::vk::Format::B8G8R8A8_SRGB);
    session.run.lock().unwrap().quads = placed;

    let mut output = session.output.lock().unwrap();
    let rebuild = output
        .as_ref()
        .is_none_or(|o| o.eye_size != eye_size || o.format != format);
    if rebuild {
        if let Some(old) = output.take() {
            let _ = vkd.device.device_wait_idle();
            for frame in &old.frames {
                vkd.destroy_image(&frame.target.image);
            }
        }
        match make_output(vkd, link, eye_size, format) {
            Ok(o) => {
                log::info!("composing {}x{} per eye as {:?}", eye_size.0, eye_size.1, format);
                *output = Some(o);
            }
            Err(e) => {
                log::error!("could not make the frame buffers: {e}");
                return R::ERROR_RUNTIME_FAILURE;
            }
        }
    }
    let output = output.as_mut().unwrap();
    let count = output.frames.len();
    let Some(pick) = (0..count)
        .map(|k| (output.next + k) % count)
        .find(|i| output.frames[*i].buffer.is_free())
    else {
        // All held by the compositor: this frame is the one to lose.
        return R::SUCCESS;
    };
    output.next = (pick + 1) % count;
    let frame = &mut output.frames[pick];
    if let Err(e) = vkd.compose(&mut frame.target, &copies, &quads, eye_size) {
        log::error!("composing the frame: {e}");
        return R::ERROR_RUNTIME_FAILURE;
    }
    // The head the application drew for, so the compositor can show the picture turned by how far
    // the head has gone since.
    if let (Some(p), Some(view)) = (projection_ptr, views.first()) {
        if let Some(space) = SPACES.get(openxr_sys::Handle::into_raw((*p).space)) {
            if let Some(base) = session.world_from(&space, display_time_ns) {
                let q = (base.orientation * crate::pose::Pose::from_raw(&view.pose).orientation).normalize();
                // And the token the pose carried, from the very sample the application was given.
                let token = session.sample_at(display_time_ns).token;
                {
                    // Said now and then: how far the head the picture was drawn for is from the
                    // head now, and whether the field of view the application drew with is the one the
                    // compositor will take it to have. Either being off shows as a picture that is
                    // not where it should be.
                    static LAST_FRAME_LOG: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
                    let now = crate::time::now_ns();
                    if now - LAST_FRAME_LOG.load(std::sync::atomic::Ordering::Relaxed) > 1_000_000_000 {
                        LAST_FRAME_LOG.store(now, std::sync::atomic::Ordering::Relaxed);
                        let here = session.sample_at(now);
                        let apart = q.angle_between(here.head.orientation).to_degrees();
                        let f = view.fov;
                        let g = here.eyes[0].fov;
                        log::debug!(
                            "picture drawn for a head {apart:.1} degrees from the head now; drawn with fov [{:.1} {:.1} {:.1} {:.1}], the glasses' [{:.1} {:.1} {:.1} {:.1}] degrees; sample {} ms old",
                            f.angle_left.to_degrees(), f.angle_right.to_degrees(), f.angle_up.to_degrees(), f.angle_down.to_degrees(),
                            g[0].to_degrees(), g[1].to_degrees(), g[2].to_degrees(), g[3].to_degrees(),
                            (now - here.sample_ns.min(now)) / 1_000_000,
                        );
                    }
                }
                link.set_frame_pose(token, [q.x, q.y, q.z, q.w]);
            }
        }
    }
    link.present(&frame.buffer, eye_size.0 * 2, eye_size.1);
    R::SUCCESS
}

unsafe fn make_output(
    vkd: &vk::Vk,
    link: &crate::wl::Link,
    eye_size: (u32, u32),
    format: ash::vk::Format,
) -> Result<Output, String> {
    let mut frames = Vec::new();
    for _ in 0..FRAMES {
        let target = vkd.create_exportable(format, eye_size.0 * 2, eye_size.1)?;
        let fd = vkd.export_fd(&target)?;
        let buffer = link.make_buffer(
            eye_size.0 * 2,
            eye_size.1,
            vk::drm_fourcc(format),
            vk::DRM_MODIFIER_LINEAR,
            crate::wl::Plane {
                fd,
                offset: target.offset,
                stride: target.stride,
            },
        )?;
        frames.push(Frame { target, buffer });
    }
    Ok(Output {
        eye_size,
        format,
        frames,
        next: 0,
    })
}

/// `Swapchain::into_raw` under a name that does not collide with the handle trait's.
trait RawHandle {
    fn into_raw_handle(self) -> u64;
}

impl RawHandle for openxr_sys::Swapchain {
    fn into_raw_handle(self) -> u64 {
        openxr_sys::Handle::into_raw(self)
    }
}

/// One quad layer as the compositor draws it: where it is in each eye, and which part of its image.
///
/// `None` when it cannot be placed or has nothing released to show -- a quad in a space that is not
/// tracked, or from a swapchain the application has not finished a frame in.
unsafe fn quad_draw(
    session: &Session,
    quad: &openxr_sys::CompositionLayerQuad,
    time_ns: i64,
    placed_out: &mut Vec<crate::input::QuadPlace>,
) -> Option<crate::vk::QuadDraw> {
    use openxr_sys::{CompositionLayerFlags as F, EyeVisibility};
    let swapchain = SWAPCHAINS.get(openxr_sys::Handle::into_raw(quad.sub_image.swapchain))?;
    let index = swapchain.state.lock().unwrap().released?;
    let space = SPACES.get(openxr_sys::Handle::into_raw(quad.space))?;
    let placed = session.world_from(&space, time_ns)?.then(&crate::pose::Pose::from_raw(&quad.pose));
    let sample = session.sample_at(time_ns);
    let fitted = crate::input::fit_quad(&sample.head, sample.eyes[0].fov, &placed, [quad.size.width, quad.size.height]);
    placed_out.push(fitted);
    let model = fitted.shown.matrix() * glam::Mat4::from_scale(glam::Vec3::new(fitted.shown_size[0], fitted.shown_size[1], 1.0));
    let mut mvp = [[0f32; 16]; 2];
    for eye in 0..2 {
        let view = sample.eyes[eye].pose.inverse().matrix();
        let clip = crate::pose::clip_from_eye(sample.eyes[eye].fov);
        mvp[eye] = (clip * view * model).to_cols_array();
    }
    let rect = quad.sub_image.image_rect;
    let (w, h) = (swapchain.width.max(1) as f32, swapchain.height.max(1) as f32);
    let visible = match quad.eye_visibility {
        EyeVisibility::LEFT => [true, false],
        EyeVisibility::RIGHT => [false, true],
        _ => [true, true],
    };
    {
        // Said once per quad shape, not every frame: what a quad is, in the words an application
        // used for it, is the first thing to look at when one is in the wrong place. Where it is
        // moves with the head for a panel that follows, so that is only said now and then, and in the
        // terms that matter -- how far from the middle of the view it is, in degrees.
        static LAST: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
        static LAST_POSE: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);
        let line = format!(
            "quad {}x{} m (shown {:.0}%) in {:?}, flags {:?}, eyes {:?}, image {}x{} rect {:?}",
            quad.size.width,
            quad.size.height,
            100.0 * fitted.shown_size[0] / quad.size.width.max(1e-6),
            space.kind,
            quad.layer_flags,
            quad.eye_visibility,
            swapchain.width,
            swapchain.height,
            (rect_of(quad.sub_image.image_rect)),
        );
        let mut last = LAST.lock().unwrap();
        if *last != line {
            log::info!("{line}");
            *last = line;
        }
        let now = crate::time::now_ns();
        if now - LAST_POSE.load(std::sync::atomic::Ordering::Relaxed) > 1_000_000_000 {
            LAST_POSE.store(now, std::sync::atomic::Ordering::Relaxed);
            let in_view = sample.head.inverse().then(&placed).position;
            let distance = in_view.length();
            let head_forward = sample.head.orientation * glam::Vec3::NEG_Z;
            log::debug!(
                "head at ({:.2}, {:.2}, {:.2}) in the room, looking {:.0} degrees {} of level; quad in the room at ({:.2}, {:.2}, {:.2})",
                sample.head.position.x,
                sample.head.position.y,
                sample.head.position.z,
                head_forward.y.asin().to_degrees().abs(),
                if head_forward.y >= 0.0 { "above" } else { "below" },
                placed.position.x,
                placed.position.y,
                placed.position.z,
            );
            log::debug!(
                "quad at ({:.2}, {:.2}, {:.2}) in {:?}: {:.1} m away, {:.0} degrees {} and {:.0} {} of where the head points",
                quad.pose.position.x,
                quad.pose.position.y,
                quad.pose.position.z,
                space.kind,
                distance,
                in_view.x.atan2(-in_view.z).to_degrees().abs(),
                if in_view.x >= 0.0 { "right" } else { "left" },
                in_view.y.atan2((in_view.x * in_view.x + in_view.z * in_view.z).sqrt()).to_degrees().abs(),
                if in_view.y >= 0.0 { "up" } else { "down" },
            );
        }
    }
    let (image, layer) = swapchain.vulkan_image(index, quad.sub_image.image_array_index)?;
    Some(crate::vk::QuadDraw {
        image,
        layer,
        layout: swapchain.layout(),
        format: swapchain.format,
        uv_rect: {
            let (u0, u1) = (rect.offset.x as f32 / w, (rect.offset.x + rect.extent.width) as f32 / w);
            let (v0, v1) = (rect.offset.y as f32 / h, (rect.offset.y + rect.extent.height) as f32 / h);
            // An OpenGL image's first row is its bottom: read it from the other end.
            if swapchain.is_gl() { [u0, v1, u1, v0] } else { [u0, v0, u1, v1] }
        },
        mvp,
        visible,
        use_alpha: quad.layer_flags.contains(F::BLEND_TEXTURE_SOURCE_ALPHA),
        unpremultiplied: quad.layer_flags.contains(F::UNPREMULTIPLIED_ALPHA),
    })
}

fn rect_of(r: openxr_sys::Rect2Di) -> (i32, i32, i32, i32) {
    (r.offset.x, r.offset.y, r.extent.width, r.extent.height)
}
