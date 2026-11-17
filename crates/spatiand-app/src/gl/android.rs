//! Android's two kinds of picture without a copy, both Android hardware buffers.
//!
//! **A window's own picture, handed to the room** ([`Shared`]), as the Deck hands a dmabuf
//! ([`super::linux`]): the texture a window is drawn into is a hardware buffer, and the buffer
//! goes to the compositor through `zwp_linux_dmabuf_v1` the way SpatiMesh's Chromium sends its
//! frames on the same phone (`third_party/chromium/android-libc/0021` and `0024`), so the room
//! has one way in for both. Android's GPU has no dmabuf to give, so the plane's "file" is a socket
//! the buffer was sent through (`AHardwareBuffer_sendHandleToUnixSocket`), marked with Spatiand's
//! own modifier ([`HARDWARE_BUFFER_MODIFIER`]); the room's EGL (`android/eglshim/eglshim.c`)
//! offers that modifier for RGBA and makes its image from the buffer it reads there. The socket
//! is kept: each frame's fence goes down it and the room's GPU waits for it before drawing, so
//! this thread does not wait for its own GPU. Where the room does not offer the modifier, or a
//! buffer cannot be had, the picture is read back and sent as memory, as it was before.
//!
//! **Somebody else's picture, to sample** ([`HardwareBufferTexture`]): a video decoder's, YUV in
//! the GPU's own layout. The buffer becomes an EGLImage (`EGL_ANDROID_get_native_client_buffer`,
//! `EGL_ANDROID_image_native_buffer`) bound to an external texture (`GL_OES_EGL_image_external`):
//! a shader samples it with `samplerExternalOES`, and the GPU turns its YUV into colour as it
//! reads, by what the buffer says it is -- as Android's own players show a decoder's pictures.

use std::ffi::c_void;
use std::os::fd::{AsFd, AsRawFd, FromRawFd, OwnedFd};

use khronos_egl as egl;

use super::Gl;
use crate::wire::Wire;
use crate::WindowId;

/// A texture target that is an image of the GPU's own layout, sampled as colour.
pub const TEXTURE_EXTERNAL_OES: u32 = 0x8D65;

const NATIVE_BUFFER_ANDROID: egl::Enum = 0x3140;
const IMAGE_PRESERVED_KHR: egl::Int = 0x30D2;

type GetClientBuffer = unsafe extern "C" fn(*const c_void) -> *mut c_void;
type CreateImage = unsafe extern "C" fn(*mut c_void, *mut c_void, egl::Enum, *mut c_void, *const egl::Int) -> *mut c_void;
type DestroyImage = unsafe extern "C" fn(*mut c_void, *mut c_void) -> egl::Boolean;
type ImageTarget = unsafe extern "C" fn(u32, *mut c_void);

/// An Android hardware buffer as an external texture ([`TEXTURE_EXTERNAL_OES`]). The buffer is
/// only borrowed, and must outlive the texture.
pub struct HardwareBufferTexture {
    pub texture: glow::Texture,
    image: *mut c_void,
}

impl Gl {
    /// `buffer` (an `AHardwareBuffer*`) as an external texture.
    pub fn hardware_buffer_texture(&self, buffer: *mut c_void) -> Result<HardwareBufferTexture, String> {
        use glow::HasContext;
        let find = |name: &str| self.egl.get_proc_address(name).ok_or_else(|| format!("this EGL has no {name}"));
        // SAFETY: the extensions' functions, by their published shapes.
        let (client_buffer, create, destroy, target): (GetClientBuffer, CreateImage, DestroyImage, ImageTarget) = unsafe {
            (
                std::mem::transmute(find("eglGetNativeClientBufferANDROID")?),
                std::mem::transmute(find("eglCreateImageKHR")?),
                std::mem::transmute(find("eglDestroyImageKHR")?),
                std::mem::transmute(find("glEGLImageTargetTexture2DOES")?),
            )
        };
        unsafe {
            let client = client_buffer(buffer);
            if client.is_null() {
                return Err("a hardware buffer has no EGL client buffer".into());
            }
            let attributes = [IMAGE_PRESERVED_KHR, egl::TRUE as egl::Int, egl::NONE];
            let image = create(self.display.as_ptr(), egl::NO_CONTEXT, NATIVE_BUFFER_ANDROID, client, attributes.as_ptr());
            if image.is_null() {
                return Err(format!("a hardware buffer would not become an image ({:?})", self.egl.get_error()));
            }
            let g = self.api();
            let texture = match g.create_texture() {
                Ok(texture) => texture,
                Err(e) => {
                    destroy(self.display.as_ptr(), image);
                    return Err(e);
                }
            };
            g.bind_texture(TEXTURE_EXTERNAL_OES, Some(texture));
            target(TEXTURE_EXTERNAL_OES, image);
            for (name, value) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                g.tex_parameter_i32(TEXTURE_EXTERNAL_OES, name, value as i32);
            }
            g.bind_texture(TEXTURE_EXTERNAL_OES, None);
            if g.get_error() != glow::NO_ERROR {
                g.delete_texture(texture);
                destroy(self.display.as_ptr(), image);
                return Err("a hardware buffer would not become a texture".into());
            }
            Ok(HardwareBufferTexture { texture, image })
        }
    }

    pub fn release_hardware_buffer_texture(&self, held: HardwareBufferTexture) {
        use glow::HasContext;
        unsafe { self.api().delete_texture(held.texture) };
        if let Some(destroy) = self.egl.get_proc_address("eglDestroyImageKHR") {
            // SAFETY: as above.
            let destroy: DestroyImage = unsafe { std::mem::transmute(destroy) };
            unsafe { destroy(self.display.as_ptr(), held.image) };
        }
    }
}

/// Spatiand's own dmabuf modifier: the plane's file is a socket an Android hardware buffer was
/// sent through, not a dmabuf. `android/eglshim/eglshim.c` and Chromium's patch 0021 have the
/// same number.
pub(crate) const HARDWARE_BUFFER_MODIFIER: u64 = 0xf500_0000_0000_0001;
/// `AB24`: red, green, blue, alpha in memory, which is what GL draws.
pub(crate) const ABGR8888: u32 = u32::from_le_bytes(*b"AB24");

/// `AHardwareBuffer_Desc`.
#[repr(C)]
#[derive(Default)]
struct Description {
    width: u32,
    height: u32,
    layers: u32,
    format: u32,
    usage: u64,
    stride: u32,
    rfu0: u32,
    rfu1: u64,
}

const FORMAT_R8G8B8A8_UNORM: u32 = 1;
const USAGE_GPU_SAMPLED_IMAGE: u64 = 1 << 8;
const USAGE_GPU_FRAMEBUFFER: u64 = 1 << 9;

#[link(name = "nativewindow")]
extern "C" {
    fn AHardwareBuffer_allocate(description: *const Description, buffer: *mut *mut c_void) -> i32;
    fn AHardwareBuffer_describe(buffer: *const c_void, description: *mut Description);
    fn AHardwareBuffer_release(buffer: *mut c_void);
    fn AHardwareBuffer_sendHandleToUnixSocket(buffer: *const c_void, socket: i32) -> i32;
}

const SYNC_NATIVE_FENCE_ANDROID: egl::Enum = 0x3144;
const SYNC_NATIVE_FENCE_FD_ANDROID: egl::Int = 0x3145;
const NO_NATIVE_FENCE_FD_ANDROID: egl::Int = -1;

type CreateSync = unsafe extern "C" fn(*mut c_void, egl::Enum, *const egl::Int) -> *mut c_void;
type DestroySync = unsafe extern "C" fn(*mut c_void, *mut c_void) -> egl::Boolean;
type DupFence = unsafe extern "C" fn(*mut c_void, *mut c_void) -> i32;

/// One picture of a window: the hardware buffer its texture draws into, and the two ends of the
/// socket it goes to the room through.
pub(super) struct Shared {
    buffer: *mut c_void,
    image: *mut c_void,
    /// The room's end, which stands as the dmabuf plane's file.
    theirs: OwnedFd,
    /// This end: the buffer went down it, and each frame's fence follows. `None` once a fence
    /// would not go down it at all: frames then wait for their GPU here.
    fences: std::cell::RefCell<Option<OwnedFd>>,
    /// Bytes a row, as the buffer says.
    stride: u32,
}

impl Shared {
    /// Make `texture`, which is bound and has no storage yet, a hardware buffer of this size.
    pub(super) fn new(gl: &Gl, _texture: glow::Texture, size: (u32, u32)) -> Result<Shared, String> {
        use glow::HasContext;
        let find = |name: &str| gl.egl.get_proc_address(name).ok_or_else(|| format!("this EGL has no {name}"));
        // SAFETY: the extensions' functions, by their published shapes.
        let (client_buffer, create, destroy, target): (GetClientBuffer, CreateImage, DestroyImage, ImageTarget) = unsafe {
            (
                std::mem::transmute(find("eglGetNativeClientBufferANDROID")?),
                std::mem::transmute(find("eglCreateImageKHR")?),
                std::mem::transmute(find("eglDestroyImageKHR")?),
                std::mem::transmute(find("glEGLImageTargetTexture2DOES")?),
            )
        };
        let wanted = Description {
            width: size.0,
            height: size.1,
            layers: 1,
            format: FORMAT_R8G8B8A8_UNORM,
            usage: USAGE_GPU_FRAMEBUFFER | USAGE_GPU_SAMPLED_IMAGE,
            ..Default::default()
        };
        // SAFETY: Android's buffer calls and EGL's, each given what it made or what the last gave.
        unsafe {
            let mut buffer = std::ptr::null_mut();
            let status = AHardwareBuffer_allocate(&wanted, &mut buffer);
            if status != 0 || buffer.is_null() {
                return Err(format!("no hardware buffer of {}x{} ({status})", size.0, size.1));
            }
            let fail = |image: *mut c_void, what: String| {
                if !image.is_null() {
                    destroy(gl.display.as_ptr(), image);
                }
                AHardwareBuffer_release(buffer);
                Err(what)
            };
            let mut got = Description::default();
            AHardwareBuffer_describe(buffer, &mut got);

            let client = client_buffer(buffer);
            if client.is_null() {
                return fail(std::ptr::null_mut(), "a hardware buffer has no EGL client buffer".into());
            }
            let attributes = [IMAGE_PRESERVED_KHR, egl::TRUE as egl::Int, egl::NONE];
            let image = create(gl.display.as_ptr(), egl::NO_CONTEXT, NATIVE_BUFFER_ANDROID, client, attributes.as_ptr());
            if image.is_null() {
                return fail(image, format!("a hardware buffer would not become an image ({:?})", gl.egl.get_error()));
            }
            let g = gl.api();
            // Whatever an application's own drawing left unread is not this texture's fault.
            for _ in 0..8 {
                if g.get_error() == glow::NO_ERROR {
                    break;
                }
            }
            target(glow::TEXTURE_2D, image);
            let error = g.get_error();
            if error != glow::NO_ERROR {
                return fail(image, format!("a hardware buffer would not become a texture ({error:#x})"));
            }

            // The room's half: the buffer goes through a socket, which the room's EGL reads it
            // from when it is given the socket as a dmabuf's file.
            let mut sockets = [-1i32; 2];
            if libc::socketpair(libc::AF_UNIX, libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC, 0, sockets.as_mut_ptr()) != 0 {
                return fail(image, format!("no socket for a hardware buffer: {}", std::io::Error::last_os_error()));
            }
            let (theirs, ours) = (OwnedFd::from_raw_fd(sockets[0]), OwnedFd::from_raw_fd(sockets[1]));
            let status = AHardwareBuffer_sendHandleToUnixSocket(buffer, ours.as_raw_fd());
            if status != 0 {
                return fail(image, format!("a hardware buffer would not go through its socket ({status})"));
            }
            Ok(Shared { buffer, image, theirs, fences: std::cell::RefCell::new(Some(ours)), stride: got.stride.max(size.0) * 4 })
        }
    }

    /// Hand the picture just drawn to the compositor. `key` comes back in [`Wire::released`]
    /// when the compositor has finished with it.
    pub(super) fn show(&self, gl: &Gl, wire: &mut Wire, window: WindowId, key: u32, size: (u32, u32)) -> Result<(), String> {
        use glow::HasContext;
        if !wire.takes_hardware_buffers() {
            return Err("the compositor takes no Android hardware buffers".into());
        }
        // The room samples this memory from a GPU context of its own: its GPU waits for this
        // frame's fence, or, where one cannot be sent, this thread waits for its own GPU.
        if !self.send_fence(gl) {
            unsafe { gl.api().finish() };
        }
        wire.show_dmabuf(window, key, size, ABGR8888, HARDWARE_BUFFER_MODIFIER, &[(self.theirs.as_fd(), 0, self.stride)])?;
        static SAID: std::sync::Once = std::sync::Once::new();
        SAID.call_once(|| log::info!("pictures go to the compositor as Android hardware buffers; no copies"));
        Ok(())
    }

    /// The fence of what was just drawn (`EGL_ANDROID_native_fence_sync`), down the buffer's
    /// socket to the room, whose GPU waits for it before drawing (the shim's
    /// `spatiand_eglshim_wait_for_clients`). False where it could not be sent.
    fn send_fence(&self, gl: &Gl) -> bool {
        use glow::HasContext;
        static CALLS: std::sync::OnceLock<Option<(CreateSync, DestroySync, DupFence)>> = std::sync::OnceLock::new();
        let calls = CALLS.get_or_init(|| {
            let find = |name: &str| gl.egl.get_proc_address(name);
            // SAFETY: the extensions' functions, by their published shapes.
            Some(unsafe {
                (
                    std::mem::transmute::<extern "system" fn(), CreateSync>(find("eglCreateSyncKHR")?),
                    std::mem::transmute::<extern "system" fn(), DestroySync>(find("eglDestroySyncKHR")?),
                    std::mem::transmute::<extern "system" fn(), DupFence>(find("eglDupNativeFenceFDANDROID")?),
                )
            })
        });
        let Some((create, destroy, dup)) = *calls else { return false };
        let mut fences = self.fences.borrow_mut();
        let Some(socket) = fences.as_ref() else { return false };
        let display = gl.display.as_ptr();
        // SAFETY: EGL's calls on this thread's display, with the context current.
        let fence = unsafe {
            let attributes = [SYNC_NATIVE_FENCE_FD_ANDROID, NO_NATIVE_FENCE_FD_ANDROID, egl::NONE];
            let sync = create(display, SYNC_NATIVE_FENCE_ANDROID, attributes.as_ptr());
            if sync.is_null() {
                return false;
            }
            // The fence is of what has gone to the GPU, and exists once it has.
            gl.api().flush();
            let fence = dup(display, sync);
            destroy(display, sync);
            if fence < 0 {
                return false;
            }
            OwnedFd::from_raw_fd(fence)
        };
        // One byte with the fence beside it, which is what the shim's half reads.
        // SAFETY: a message of one byte and one descriptor, laid out as sendmsg wants it.
        let sent = unsafe {
            let mut byte = b'F';
            let mut part = libc::iovec { iov_base: (&mut byte as *mut u8).cast(), iov_len: 1 };
            // Room for one descriptor's control message, aligned as its header must be.
            let mut control = [0u64; 4];
            let mut message: libc::msghdr = std::mem::zeroed();
            message.msg_iov = &mut part;
            message.msg_iovlen = 1;
            message.msg_control = control.as_mut_ptr().cast();
            message.msg_controllen = libc::CMSG_SPACE(std::mem::size_of::<i32>() as u32) as _;
            let header = libc::CMSG_FIRSTHDR(&message);
            (*header).cmsg_level = libc::SOL_SOCKET;
            (*header).cmsg_type = libc::SCM_RIGHTS;
            (*header).cmsg_len = libc::CMSG_LEN(std::mem::size_of::<i32>() as u32) as _;
            let descriptor = fence.as_raw_fd();
            std::ptr::copy_nonoverlapping((&descriptor as *const i32).cast::<u8>(), libc::CMSG_DATA(header), std::mem::size_of::<i32>());
            loop {
                let n = libc::sendmsg(socket.as_raw_fd(), &message, libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL);
                if n == 1 {
                    break Ok(());
                }
                let error = std::io::Error::last_os_error();
                if error.kind() != std::io::ErrorKind::Interrupted {
                    break Err(error);
                }
            }
        };
        match sent {
            Ok(()) => true,
            // Full: the room is not drawing just now, and reads them when it does. This frame
            // waits for its GPU here.
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => false,
            Err(e) => {
                log::debug!("a frame's fence would not go to the compositor ({e}); this picture's frames wait here");
                *fences = None;
                false
            }
        }
    }

    pub(super) fn destroy(self, gl: &Gl) {
        if let Some(destroy) = gl.egl.get_proc_address("eglDestroyImageKHR") {
            // SAFETY: as above.
            let destroy: DestroyImage = unsafe { std::mem::transmute(destroy) };
            unsafe { destroy(gl.display.as_ptr(), self.image) };
        }
        // SAFETY: the reference `new` took; the room holds its own.
        unsafe { AHardwareBuffer_release(self.buffer) };
    }
}

/// Said once in a process's life: its pictures are being copied to the compositor, and why.
pub(super) fn copying(why: &str) {
    static SAID: std::sync::Once = std::sync::Once::new();
    SAID.call_once(|| log::warn!("pictures will be copied to the compositor: {why}"));
}
