//! The compositor's half of the Mac's pictures: a placeholder in, an `IOSurface` out.
//!
//! A window whose picture is an `IOSurface` -- a host's, decoded by VideoToolbox, or one of this
//! Mac's own, captured by the app -- has a placeholder for a buffer: the picture's size, and a
//! token (see `spatiand_video::mac` and the client's `Placeholder`). Here, each frame, the
//! token finds the window's output, and its newest surface is bound to an ordinary texture,
//! which [`substitute`] hands the scene in place of the placeholder's. From there on the window
//! is drawn exactly as on the Deck: the same quad, frame, title bar and pointer.
//!
//! Nothing is copied. ANGLE wraps an `IOSurface` as a pbuffer, and a pbuffer can be a texture's
//! image; the GPU samples the very memory the decoder or the window server wrote.

use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::ffi::c_void;

use smithay::backend::egl::ffi::egl;
use smithay::backend::renderer::gles::{ffi, GlesRenderer};
use smithay::reexports::wayland_server::protocol::wl_surface::WlSurface;
use spatiand_video::mac::{lookup, Frame, PLACEHOLDER_MAGIC};

const IOSURFACE_ANGLE: u32 = 0x3454;
const IOSURFACE_PLANE_ANGLE: i32 = 0x345A;
const TEXTURE_TYPE_ANGLE: i32 = 0x345C;
const TEXTURE_INTERNAL_FORMAT_ANGLE: i32 = 0x345D;
const GL_BGRA_EXT: i32 = 0x80E1;

/// How many frames a picture taken off screen is kept before it is let go of, so the GPU has
/// surely finished reading it.
const RETIRE_AFTER: u64 = 3;

/// A picture on screen: the surface, held, and the pbuffer made of it.
struct Showing {
    frame: Frame,
    pbuffer: *const c_void,
}

/// What one window's picture is bound to.
struct Slot {
    texture: u32,
    size: (u32, u32),
    showing: Option<Showing>,
    /// Frames this slot was asked for last, so one nobody asks for any more can go.
    last_used: u64,
}

#[derive(Default)]
struct Pictures {
    slots: HashMap<u32, Slot>,
    retired: VecDeque<(Showing, u64)>,
    frame: u64,
    complained: bool,
}

thread_local! {
    // Everything here is GL state, which belongs to the compositor's thread and nowhere else.
    static PICTURES: RefCell<Pictures> = RefCell::new(Pictures::default());
}

/// The token in a surface's buffer, if its buffer is a placeholder.
fn token_of(surface: &WlSurface) -> Option<u32> {
    use smithay::backend::renderer::utils::with_renderer_surface_state;
    let buffer = with_renderer_surface_state(surface, |st| st.buffer().cloned()).flatten()?;
    smithay::wayland::shm::with_buffer_contents(&buffer, |ptr, len, data| {
        let at = data.offset as usize;
        if len < at + 8 {
            return None;
        }
        let head = unsafe { std::slice::from_raw_parts(ptr.add(at), 8) };
        let magic = u32::from_le_bytes([head[0], head[1], head[2], head[3]]);
        let token = u32::from_le_bytes([head[4], head[5], head[6], head[7]]);
        (magic == PLACEHOLDER_MAGIC).then_some(token)
    })
    .ok()
    .flatten()
}

/// The config the current context was made with, which a pbuffer for it must share.
unsafe fn current_config(display: *const c_void) -> Option<*const c_void> {
    let context = egl::GetCurrentContext();
    let mut id = 0;
    if egl::QueryContext(display, context, egl::CONFIG_ID as i32, &mut id) == egl::FALSE {
        return None;
    }
    let attributes = [egl::CONFIG_ID as i32, id, egl::NONE as i32];
    let mut config = std::ptr::null();
    let mut found = 0;
    (egl::ChooseConfig(display, attributes.as_ptr(), &mut config, 1, &mut found) != egl::FALSE && found > 0)
        .then_some(config)
}

/// An `IOSurface` as a pbuffer a texture can take its image from.
unsafe fn pbuffer_of(display: *const c_void, frame: &Frame) -> Option<*const c_void> {
    let config = current_config(display)?;
    let (width, height) = frame.surface.size();
    let attributes = [
        egl::WIDTH as i32, width as i32,
        egl::HEIGHT as i32, height as i32,
        IOSURFACE_PLANE_ANGLE, 0,
        egl::TEXTURE_TARGET as i32, egl::TEXTURE_2D as i32,
        TEXTURE_INTERNAL_FORMAT_ANGLE, GL_BGRA_EXT,
        egl::TEXTURE_FORMAT as i32, egl::TEXTURE_RGBA as i32,
        TEXTURE_TYPE_ANGLE, ffi::UNSIGNED_BYTE as i32,
        egl::NONE as i32,
    ];
    let pbuffer = egl::CreatePbufferFromClientBuffer(
        display,
        IOSURFACE_ANGLE,
        frame.surface.raw() as *mut c_void,
        config,
        attributes.as_ptr(),
    );
    (pbuffer != egl::NO_SURFACE).then_some(pbuffer)
}

unsafe fn let_go(display: *const c_void, showing: Showing) {
    egl::DestroySurface(display, showing.pbuffer);
    drop(showing.frame);
}

/// The texture to draw for this surface: its newest picture if it is a placeholder's, and
/// `texture` -- its own -- otherwise.
pub fn substitute(renderer: &mut GlesRenderer, surface: &WlSurface, texture: u32) -> u32 {
    let Some(token) = token_of(surface) else {
        return texture;
    };
    let Some(output) = lookup(token) else {
        return texture;
    };
    PICTURES.with(|pictures| {
        let mut pictures = pictures.borrow_mut();
        let pictures = &mut *pictures;
        let frame_no = pictures.frame;
        let fresh = output.take_latest();
        let drawn = renderer.with_context(|gl| unsafe {
            let display = egl::GetCurrentDisplay();
            let slot = pictures.slots.entry(token).or_insert_with(|| {
                let mut id = 0u32;
                gl.GenTextures(1, &mut id);
                Slot { texture: id, size: (0, 0), showing: None, last_used: frame_no }
            });
            slot.last_used = frame_no;
            if let Some(frame) = fresh {
                match pbuffer_of(display, &frame) {
                    Some(pbuffer) => {
                        gl.BindTexture(ffi::TEXTURE_2D, slot.texture);
                        if let Some(old) = slot.showing.as_ref() {
                            egl::ReleaseTexImage(display, old.pbuffer, egl::BACK_BUFFER as i32);
                        }
                        if egl::BindTexImage(display, pbuffer, egl::BACK_BUFFER as i32) == egl::FALSE {
                            log::warn!("pictures: a surface would not become a texture ({:#x})", egl::GetError());
                            egl::DestroySurface(display, pbuffer);
                        } else {
                            for (name, value) in [
                                (ffi::TEXTURE_MIN_FILTER, ffi::LINEAR),
                                (ffi::TEXTURE_MAG_FILTER, ffi::LINEAR),
                                (ffi::TEXTURE_WRAP_S, ffi::CLAMP_TO_EDGE),
                                (ffi::TEXTURE_WRAP_T, ffi::CLAMP_TO_EDGE),
                            ] {
                                gl.TexParameteri(ffi::TEXTURE_2D, name, value as i32);
                            }
                            slot.size = frame.surface.size();
                            if let Some(old) = slot.showing.replace(Showing { frame, pbuffer }) {
                                pictures.retired.push_back((old, frame_no));
                            }
                        }
                        gl.BindTexture(ffi::TEXTURE_2D, 0);
                    }
                    None => {
                        if !std::mem::replace(&mut pictures.complained, true) {
                            log::error!(
                                "pictures: ANGLE would not wrap an IOSurface ({:#x}); windows stay blank",
                                egl::GetError()
                            );
                        }
                    }
                }
            }
            (slot.size != (0, 0)).then_some(slot.texture)
        });
        drawn.ok().flatten().unwrap_or(texture)
    })
}

/// Which part of the picture being shown for this surface is picture, where it is not all of
/// it, in the scene's order.
pub fn crop_of(surface: &WlSurface) -> Option<[f32; 4]> {
    let token = token_of(surface)?;
    PICTURES.with(|pictures| {
        let crop = pictures.borrow().slots.get(&token)?.showing.as_ref()?.frame.crop;
        // The scene's order: (u0, u1, v0, v1).
        (crop != [0.0, 0.0, 1.0, 1.0]).then_some([crop[0], crop[2], crop[1], crop[3]])
    })
}

/// Once a frame, after it is drawn: pictures off screen long enough are let go of, and windows
/// that are gone give their textures up.
pub fn end_frame(renderer: &mut GlesRenderer) {
    PICTURES.with(|pictures| {
        let mut pictures = pictures.borrow_mut();
        let pictures = &mut *pictures;
        pictures.frame += 1;
        let now = pictures.frame;
        let _ = renderer.with_context(|gl| unsafe {
            let display = egl::GetCurrentDisplay();
            while pictures.retired.front().is_some_and(|(_, at)| now >= at + RETIRE_AFTER) {
                let (showing, _) = pictures.retired.pop_front().unwrap();
                let_go(display, showing);
            }
            let stale: Vec<u32> = pictures
                .slots
                .iter()
                .filter(|(token, slot)| lookup(**token).is_none() && now > slot.last_used + RETIRE_AFTER)
                .map(|(token, _)| *token)
                .collect();
            for token in stale {
                if let Some(slot) = pictures.slots.remove(&token) {
                    if let Some(showing) = slot.showing {
                        egl::ReleaseTexImage(display, showing.pbuffer, egl::BACK_BUFFER as i32);
                        pictures.retired.push_back((showing, now));
                    }
                    gl.DeleteTextures(1, &slot.texture);
                }
            }
        });
    });
}
