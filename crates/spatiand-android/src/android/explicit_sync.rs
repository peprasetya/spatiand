//! Clients' GPUs finishing their frames, waited for by the room's GPU instead of their threads.
//!
//! On Linux a dmabuf carries its own fences and the room's GPU waits for the client's without
//! being told (implicit synchronisation). An Android hardware buffer does not, and the Wayland
//! protocols that carry fences are of no use here: Chromium speaks only `wp_linux_drm_syncobj_v1`,
//! which needs DRM devices Android has not got. So SpatiMesh's Chromium waited for its own GPU before
//! sending each frame, looking at the fence on its GPU process's main thread, behind tile raster: a
//! scrolled frame went out up to 13 ms after it was drawn.
//!
//! Its hardware buffers come to the room through a socket each (`android/eglshim`), which it
//! keeps: each frame's fence (a sync file) goes down that socket just before the frame is
//! committed, and here, before the room draws, its GPU waits for every fence that has come
//! (`eglWaitSyncKHR`, in the shim, which holds the sockets). Waiting for one that belongs to a
//! frame not committed yet costs nothing worth having: it is a frame already drawn by then.

/// Before the room draws: its GPU waits for the frames its clients have said are being drawn.
pub fn wait_for_clients(renderer: &mut smithay::backend::renderer::gles::GlesRenderer) {
    type Wait = unsafe extern "C" fn() -> i32;
    static WAIT: std::sync::OnceLock<Option<Wait>> = std::sync::OnceLock::new();
    let wait = WAIT.get_or_init(|| unsafe {
        let shim = libc::dlopen(c"libEGL.so.1".as_ptr(), libc::RTLD_NOW | libc::RTLD_NOLOAD);
        if shim.is_null() {
            return None;
        }
        let f = libc::dlsym(shim, c"spatiand_eglshim_wait_for_clients".as_ptr());
        (!f.is_null()).then(|| std::mem::transmute::<*mut libc::c_void, Wait>(f))
    });
    if let Some(wait) = wait {
        let _ = renderer.with_context(|_gl| unsafe {
            wait();
        });
    }
}
