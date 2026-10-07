//! OpenGL, for applications that draw with it.
//!
//! An OpenGL application hands the runtime nothing to draw into, and the runtime hands it textures.
//! They are ordinary textures, so any format and any array size works. What Vulkan reads is a
//! **linear** staging image for each layer, allocated by Vulkan and imported into GL
//! (`GL_EXT_memory_object_fd`), which the application's picture is copied into on the GPU when it
//! releases the image.
//!
//! Why not simply share the application's texture: tiled images do not mean the same thing to two
//! drivers. Shared directly, a tiled texture came out as confetti -- the right colours in the wrong
//! places -- and the linear one did not, so the shared image is linear and the copy is what makes
//! tiling the application's own business. Both drivers have to be the same GPU's, which is why the
//! device is matched by UUID (see [`crate::vk::Vk::standalone`]).
//!
//! Every call here is made on a thread where the application's context is current, which is what
//! OpenXR requires of it when it calls the functions that lead here. Nothing is loaded linked: the
//! application has its own libGL and this reaches the same one by name.

use std::ffi::{c_char, c_void, CStr};
use std::os::fd::IntoRawFd;

use ash::vk;

pub const GL_TEXTURE_2D: u32 = 0x0DE1;
pub const GL_TEXTURE_2D_ARRAY: u32 = 0x8C1A;
const GL_TEXTURE_BINDING_2D: u32 = 0x8069;
const GL_TEXTURE_BINDING_2D_ARRAY: u32 = 0x8C1D;
const GL_HANDLE_TYPE_OPAQUE_FD_EXT: u32 = 0x9586;
const GL_NUM_DEVICE_UUIDS_EXT: u32 = 0x9596;
const GL_TEXTURE_TILING_EXT: u32 = 0x9580;
const GL_LINEAR_TILING_EXT: u32 = 0x9585;
const GL_DEVICE_UUID_EXT: u32 = 0x9597;

pub const SRGB8_ALPHA8: u32 = 0x8C43;
pub const RGBA8: u32 = 0x8058;
pub const DEPTH_COMPONENT16: u32 = 0x81A5;
pub const DEPTH_COMPONENT32F: u32 = 0x8CAC;
pub const DEPTH24_STENCIL8: u32 = 0x88F0;
pub const DEPTH32F_STENCIL8: u32 = 0x8CAD;

/// What an OpenGL swapchain may be made in: two colour formats, which are the two layouts the
/// compositor shows, and the depth formats an application asks for alongside (never shown).
pub const FORMATS: [u32; 6] = [
    SRGB8_ALPHA8,
    RGBA8,
    DEPTH_COMPONENT32F,
    DEPTH_COMPONENT16,
    DEPTH24_STENCIL8,
    DEPTH32F_STENCIL8,
];

/// The Vulkan format that has the same bytes as a GL colour format, or `None` for depth (which is
/// not shared) and anything else (which is not offered).
pub fn colour_format(internal: u32) -> Option<vk::Format> {
    match internal {
        SRGB8_ALPHA8 => Some(vk::Format::R8G8B8A8_SRGB),
        RGBA8 => Some(vk::Format::R8G8B8A8_UNORM),
        _ => None,
    }
}

pub fn is_depth(internal: u32) -> bool {
    matches!(internal, DEPTH_COMPONENT16 | DEPTH_COMPONENT32F | DEPTH24_STENCIL8 | DEPTH32F_STENCIL8)
}

type GetProcAddress = unsafe extern "C" fn(*const c_char) -> *const c_void;

/// The functions used, looked up once.
pub struct Gl {
    gen_textures: unsafe extern "C" fn(i32, *mut u32),
    delete_textures: unsafe extern "C" fn(i32, *const u32),
    bind_texture: unsafe extern "C" fn(u32, u32),
    get_integerv: unsafe extern "C" fn(u32, *mut i32),
    get_error: unsafe extern "C" fn() -> u32,
    copy_image_sub_data: Option<unsafe extern "C" fn(u32, u32, i32, i32, i32, i32, u32, u32, i32, i32, i32, i32, i32, i32, i32)>,
    tex_parameteri: Option<unsafe extern "C" fn(u32, u32, i32)>,
    finish: unsafe extern "C" fn(),
    tex_storage_2d: unsafe extern "C" fn(u32, i32, u32, i32, i32),
    tex_storage_3d: unsafe extern "C" fn(u32, i32, u32, i32, i32, i32),
    create_memory_objects: Option<unsafe extern "C" fn(i32, *mut u32)>,
    delete_memory_objects: Option<unsafe extern "C" fn(i32, *const u32)>,
    import_memory_fd: Option<unsafe extern "C" fn(u32, u64, u32, i32)>,
    tex_storage_mem_2d: Option<unsafe extern "C" fn(u32, i32, u32, i32, i32, u32, u64)>,
    tex_storage_mem_3d: Option<unsafe extern "C" fn(u32, i32, u32, i32, i32, i32, u32, u64)>,
    get_unsigned_bytei_v: Option<unsafe extern "C" fn(u32, u32, *mut u8)>,
}

unsafe fn open(name: &CStr) -> *mut c_void {
    // Already loaded by the application is the one wanted; loading another copy would be a second
    // driver with no context current in it.
    let loaded = libc::dlopen(name.as_ptr(), libc::RTLD_LAZY | libc::RTLD_NOLOAD);
    if !loaded.is_null() {
        return loaded;
    }
    libc::dlopen(name.as_ptr(), libc::RTLD_LAZY)
}

impl Gl {
    /// # Safety
    /// An OpenGL context must be current on this thread.
    pub unsafe fn load() -> Result<Gl, String> {
        let mut resolve: Option<GetProcAddress> = None;
        let gl = open(c"libGL.so.1");
        if !gl.is_null() {
            let f = libc::dlsym(gl, c"glXGetProcAddressARB".as_ptr());
            if !f.is_null() {
                resolve = Some(std::mem::transmute::<*mut c_void, GetProcAddress>(f));
            }
        }
        if resolve.is_none() {
            let egl = open(c"libEGL.so.1");
            if !egl.is_null() {
                let f = libc::dlsym(egl, c"eglGetProcAddress".as_ptr());
                if !f.is_null() {
                    resolve = Some(std::mem::transmute::<*mut c_void, GetProcAddress>(f));
                }
            }
        }
        let resolve = resolve.ok_or("no way to find OpenGL functions: neither libGL nor libEGL is loaded")?;
        macro_rules! need {
            ($name:literal) => {{
                let p = resolve(concat!($name, "\0").as_ptr() as *const c_char);
                if p.is_null() {
                    return Err(format!("OpenGL has no {}", $name));
                }
                std::mem::transmute(p)
            }};
        }
        macro_rules! want {
            ($name:literal) => {{
                let p = resolve(concat!($name, "\0").as_ptr() as *const c_char);
                if p.is_null() { None } else { Some(std::mem::transmute(p)) }
            }};
        }
        Ok(Gl {
            gen_textures: need!("glGenTextures"),
            delete_textures: need!("glDeleteTextures"),
            bind_texture: need!("glBindTexture"),
            get_integerv: need!("glGetIntegerv"),
            get_error: need!("glGetError"),
            copy_image_sub_data: want!("glCopyImageSubData"),
            tex_parameteri: want!("glTexParameteri"),
            finish: need!("glFinish"),
            tex_storage_2d: need!("glTexStorage2D"),
            tex_storage_3d: need!("glTexStorage3D"),
            create_memory_objects: want!("glCreateMemoryObjectsEXT"),
            delete_memory_objects: want!("glDeleteMemoryObjectsEXT"),
            import_memory_fd: want!("glImportMemoryFdEXT"),
            tex_storage_mem_2d: want!("glTexStorageMem2DEXT"),
            tex_storage_mem_3d: want!("glTexStorageMem3DEXT"),
            get_unsigned_bytei_v: want!("glGetUnsignedBytei_vEXT"),
        })
    }

    /// Whether memory can be shared with Vulkan at all.
    pub fn can_share(&self) -> bool {
        self.copy_image_sub_data.is_some()
            && self.create_memory_objects.is_some()
            && self.import_memory_fd.is_some()
            && self.tex_storage_mem_2d.is_some()
            && self.tex_storage_mem_3d.is_some()
    }

    /// The UUID of the GPU OpenGL is drawing on, if the driver will say.
    pub unsafe fn device_uuid(&self) -> Option<[u8; 16]> {
        let get = self.get_unsigned_bytei_v?;
        let mut count = 0i32;
        (self.get_integerv)(GL_NUM_DEVICE_UUIDS_EXT, &mut count);
        if count < 1 {
            return None;
        }
        let mut uuid = [0u8; 16];
        get(GL_DEVICE_UUID_EXT, 0, uuid.as_mut_ptr());
        // Clear whatever error asking raised on a driver that does not have the query.
        let _ = (self.get_error)();
        Some(uuid)
    }

    /// Copy one layer of the application's texture into a staging texture Vulkan can read.
    ///
    /// On the GPU, in the order of the application's own commands: what it drew is what is copied.
    pub unsafe fn copy_layer(&self, from: u32, array: bool, layer: u32, to: u32, width: u32, height: u32) {
        if let Some(copy) = self.copy_image_sub_data {
            let source = if array { GL_TEXTURE_2D_ARRAY } else { GL_TEXTURE_2D };
            copy(from, source, 0, 0, 0, layer as i32, to, GL_TEXTURE_2D, 0, 0, 0, 0, width as i32, height as i32, 1);
        }
    }

    /// Wait for everything the application has drawn. The compositor reads the memory from another
    /// API, which has no way to know when OpenGL is finished with it but to be told.
    pub unsafe fn finish(&self) {
        (self.finish)();
    }

    /// A texture backed by Vulkan's memory. Consumes `fd`.
    pub unsafe fn import_texture(
        &self,
        internal: u32,
        width: u32,
        height: u32,
        layers: u32,
        size: u64,
        fd: std::os::fd::OwnedFd,
        linear: bool,
    ) -> Result<(u32, u32), String> {
        let (create, import, storage2, storage3) = match (
            self.create_memory_objects,
            self.import_memory_fd,
            self.tex_storage_mem_2d,
            self.tex_storage_mem_3d,
        ) {
            (Some(a), Some(b), Some(c), Some(d)) => (a, b, c, d),
            _ => return Err("this OpenGL cannot import memory from Vulkan".into()),
        };
        let _ = (self.get_error)();
        let mut memory = 0u32;
        create(1, &mut memory);
        import(memory, size, GL_HANDLE_TYPE_OPAQUE_FD_EXT, fd.into_raw_fd());
        let array = layers > 1;
        let target = if array { GL_TEXTURE_2D_ARRAY } else { GL_TEXTURE_2D };
        let mut previous = 0i32;
        (self.get_integerv)(if array { GL_TEXTURE_BINDING_2D_ARRAY } else { GL_TEXTURE_BINDING_2D }, &mut previous);
        let mut texture = 0u32;
        (self.gen_textures)(1, &mut texture);
        (self.bind_texture)(target, texture);
        if let Some(parameter) = self.tex_parameteri.filter(|_| linear) {
            parameter(target, GL_TEXTURE_TILING_EXT, GL_LINEAR_TILING_EXT as i32);
        }
        if array {
            storage3(target, 1, internal, width as i32, height as i32, layers as i32, memory, 0);
        } else {
            storage2(target, 1, internal, width as i32, height as i32, memory, 0);
        }
        (self.bind_texture)(target, previous as u32);
        let error = (self.get_error)();
        if error != 0 {
            (self.delete_textures)(1, &texture);
            if let Some(delete) = self.delete_memory_objects {
                delete(1, &memory);
            }
            return Err(format!("importing the shared image failed: GL error {error:#x}"));
        }
        Ok((texture, memory))
    }

    /// An ordinary texture, for a depth buffer that nobody else reads.
    pub unsafe fn plain_texture(&self, internal: u32, width: u32, height: u32, layers: u32) -> Result<u32, String> {
        let _ = (self.get_error)();
        let array = layers > 1;
        let target = if array { GL_TEXTURE_2D_ARRAY } else { GL_TEXTURE_2D };
        let mut previous = 0i32;
        (self.get_integerv)(if array { GL_TEXTURE_BINDING_2D_ARRAY } else { GL_TEXTURE_BINDING_2D }, &mut previous);
        let mut texture = 0u32;
        (self.gen_textures)(1, &mut texture);
        (self.bind_texture)(target, texture);
        if array {
            (self.tex_storage_3d)(target, 1, internal, width as i32, height as i32, layers as i32);
        } else {
            (self.tex_storage_2d)(target, 1, internal, width as i32, height as i32);
        }
        (self.bind_texture)(target, previous as u32);
        let error = (self.get_error)();
        if error != 0 {
            (self.delete_textures)(1, &texture);
            return Err(format!("making a texture failed: GL error {error:#x}"));
        }
        Ok(texture)
    }

    pub unsafe fn delete(&self, texture: u32, memory: Option<u32>) {
        (self.delete_textures)(1, &texture);
        if let (Some(memory), Some(delete)) = (memory, self.delete_memory_objects) {
            delete(1, &memory);
        }
    }
}

// Plain function pointers into a driver, called only from the thread that owns the context.
unsafe impl Send for Gl {}
unsafe impl Sync for Gl {}
