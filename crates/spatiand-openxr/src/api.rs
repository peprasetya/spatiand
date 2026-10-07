//! The OpenXR entry points.
//!
//! Every function here is called by somebody else's program, through the loader, with pointers it
//! made. They are `unsafe extern "system"` because that is the ABI; the safety argument for each
//! is the same one: OpenXR requires the application to pass valid structures, and this checks
//! only what the specification says a runtime must check -- handles, structure types, the order
//! of calls, and capacities -- and returns the specified error rather than trusting a value.

use std::ffi::{c_char, c_void, CStr};
use std::sync::Arc;

use ash::vk::{self, Handle as _};
use openxr_sys::{
    Bool32, Duration, EnvironmentBlendMode, Handle, Instance as XrInstance, Path, Result as R,
    Session as XrSession, Space as XrSpace, StructureType, Swapchain as XrSwapchain,
    SystemId, Time, ViewConfigurationType,
};

use crate::object::*;
use crate::pose::Pose;
use crate::time::now_ns;

const RUNTIME_NAME: &str = "Spatiand";

/// The extensions this runtime offers, and the version of each specification it implements.
pub const EXTENSIONS: &[(&str, u32)] = &[
    ("XR_KHR_vulkan_enable", 8),
    ("XR_KHR_vulkan_enable2", 2),
    ("XR_KHR_opengl_enable", 10),
    ("XR_KHR_convert_timespec_time", 1),
    ("XR_KHR_composition_layer_depth", 6),
    ("XR_KHR_locate_spaces", 1),
];

/// What the headset shows, per eye, unless `SPATIAND_OPENXR_EYE` or the viewer says otherwise. The XREAL Air's
/// panels are 1080p, and an application asked to draw more only makes the Deck work harder to
/// show detail the glasses cannot.
fn eye_size() -> (u32, u32) {
    static PROBED: std::sync::OnceLock<Option<(u32, u32)>> = std::sync::OnceLock::new();
    std::env::var("SPATIAND_OPENXR_EYE")
        .ok()
        .and_then(|v| {
            let (w, h) = v.split_once('x')?;
            Some((w.parse().ok()?, h.parse().ok()?))
        })
        // What the viewer wants, if it is one that says -- a headset across a network does, with the
        // size of its own panels and the overscan it corrects rotation with.
        .or_else(|| {
            *PROBED.get_or_init(|| {
                let size = crate::wl::probe_render_size();
                if let Some((w, h)) = size {
                    log::info!("the viewer wants eyes drawn at {w}x{h}");
                }
                size
            })
        })
        .unwrap_or((1920, 1080))
}

/// The size of one eye before an application has said what it will draw: the XREAL Air's panels.
pub fn recommended_eye_size() -> (u32, u32) {
    eye_size()
}

fn write_str<const N: usize>(dst: &mut [c_char; N], text: &str) {
    for (i, byte) in text.bytes().take(N - 1).enumerate() {
        dst[i] = byte as c_char;
    }
    dst[text.len().min(N - 1)] = 0;
}

fn read_str(src: &[c_char]) -> String {
    let bytes: Vec<u8> = src.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

/// The two-call idiom: report how many there are, and copy them if there is room.
unsafe fn two_call<T: Copy>(capacity: u32, count_out: *mut u32, items: &[T], out: *mut T) -> R {
    if count_out.is_null() {
        return R::ERROR_VALIDATION_FAILURE;
    }
    *count_out = items.len() as u32;
    if capacity == 0 {
        return R::SUCCESS;
    }
    if (capacity as usize) < items.len() {
        return R::ERROR_SIZE_INSUFFICIENT;
    }
    if out.is_null() {
        return R::ERROR_VALIDATION_FAILURE;
    }
    for (i, item) in items.iter().enumerate() {
        *out.add(i) = *item;
    }
    R::SUCCESS
}

macro_rules! handle {
    ($table:expr, $h:expr) => {
        match $table.get($h.into_raw()) {
            Some(object) => object,
            None => return R::ERROR_HANDLE_INVALID,
        }
    };
}

// ---- instance ------------------------------------------------------------------------------

pub unsafe extern "system" fn enumerate_instance_extension_properties(
    layer_name: *const c_char,
    capacity: u32,
    count_out: *mut u32,
    properties: *mut openxr_sys::ExtensionProperties,
) -> R {
    crate::logger::init();
    let _ = layer_name;
    if count_out.is_null() {
        return R::ERROR_VALIDATION_FAILURE;
    }
    *count_out = EXTENSIONS.len() as u32;
    if capacity == 0 {
        return R::SUCCESS;
    }
    if (capacity as usize) < EXTENSIONS.len() {
        return R::ERROR_SIZE_INSUFFICIENT;
    }
    for (i, (name, version)) in EXTENSIONS.iter().enumerate() {
        let p = &mut *properties.add(i);
        p.ty = StructureType::EXTENSION_PROPERTIES;
        write_str(&mut p.extension_name, name);
        p.extension_version = *version;
    }
    R::SUCCESS
}

pub unsafe extern "system" fn create_instance(
    info: *const openxr_sys::InstanceCreateInfo,
    out: *mut XrInstance,
) -> R {
    crate::logger::init();
    let info = &*info;
    if info.ty != StructureType::INSTANCE_CREATE_INFO {
        return R::ERROR_VALIDATION_FAILURE;
    }
    // The versions of OpenXR this runtime speaks are 1.0 and 1.1; patch levels do not matter.
    let wanted = info.application_info.api_version;
    if wanted.major() != 1 || wanted.minor() > 1 {
        return R::ERROR_API_VERSION_UNSUPPORTED;
    }
    let mut extensions = Vec::new();
    for i in 0..info.enabled_extension_count as usize {
        let name = CStr::from_ptr(*info.enabled_extension_names.add(i)).to_string_lossy().into_owned();
        if !EXTENSIONS.iter().any(|(n, _)| *n == name) {
            log::warn!("an application asked for {name}, which this runtime does not have");
            return R::ERROR_EXTENSION_NOT_PRESENT;
        }
        extensions.push(name);
    }
    let app = read_str(&info.application_info.application_name);
    let handle = new_handle();
    log::info!("xrCreateInstance for \"{app}\" with {extensions:?}");
    INSTANCES.insert(handle, Arc::new(Instance::new(handle, app, extensions, wanted.minor() as u32)));
    *out = XrInstance::from_raw(handle);
    R::SUCCESS
}

pub unsafe extern "system" fn destroy_instance(instance: XrInstance) -> R {
    let _ = handle!(INSTANCES, instance);
    for session in SESSIONS.all() {
        if session.instance.handle == instance.into_raw() {
            SESSIONS.remove(session.handle);
        }
    }
    INSTANCES.remove(instance.into_raw());
    R::SUCCESS
}

pub unsafe extern "system" fn get_instance_properties(
    instance: XrInstance,
    properties: *mut openxr_sys::InstanceProperties,
) -> R {
    let _ = handle!(INSTANCES, instance);
    let p = &mut *properties;
    p.runtime_version = openxr_sys::Version::new(0, 1, 0);
    write_str(&mut p.runtime_name, RUNTIME_NAME);
    R::SUCCESS
}

pub unsafe extern "system" fn poll_event(instance: XrInstance, buffer: *mut openxr_sys::EventDataBuffer) -> R {
    let instance = handle!(INSTANCES, instance);
    // A window the wearer closed ends the session the way a runtime's own exit would.
    for session in SESSIONS.all() {
        if session.instance.handle != instance.handle {
            continue;
        }
        let closed = session.link.as_ref().is_some_and(|l| l.is_closed());
        let state = session.run.lock().unwrap().state;
        if closed && matches!(state, SessionState::Focused | SessionState::Visible | SessionState::Synchronized) {
            log::info!("the window was closed; asking the application to stop");
            session.push_stopping();
        }
    }
    let Some(event) = instance.events.lock().unwrap().pop_front() else {
        return R::EVENT_UNAVAILABLE;
    };
    match event {
        Event::InteractionProfileChanged { session } => {
            let out = buffer as *mut openxr_sys::EventDataInteractionProfileChanged;
            *out = openxr_sys::EventDataInteractionProfileChanged {
                ty: StructureType::EVENT_DATA_INTERACTION_PROFILE_CHANGED,
                next: std::ptr::null(),
                session: XrSession::from_raw(session),
            };
        }
        Event::SessionState { session, state, time_ns } => {
            let out = buffer as *mut openxr_sys::EventDataSessionStateChanged;
            *out = openxr_sys::EventDataSessionStateChanged {
                ty: StructureType::EVENT_DATA_SESSION_STATE_CHANGED,
                next: std::ptr::null(),
                session: XrSession::from_raw(session),
                state: state.raw(),
                time: Time::from_nanos(time_ns),
            };
        }
    }
    R::SUCCESS
}

/// The name of a result as the specification spells it, and what to say of one this does not know.
fn result_name(value: R) -> String {
    let debug = format!("{value:?}");
    if debug.parse::<i64>().is_ok() {
        let raw = value.into_raw();
        return if raw < 0 { format!("XR_UNKNOWN_FAILURE_{raw}") } else { format!("XR_UNKNOWN_SUCCESS_{raw}") };
    }
    if debug.starts_with("XR_") { debug } else { format!("XR_{debug}") }
}

/// The name of a structure type as the specification spells it.
fn structure_type_name(value: StructureType) -> String {
    let debug = format!("{value:?}");
    if debug.parse::<i64>().is_ok() {
        return format!("XR_UNKNOWN_STRUCTURE_TYPE_{}", value.into_raw());
    }
    if debug.starts_with("XR_TYPE_") { debug } else { format!("XR_TYPE_{debug}") }
}

pub unsafe extern "system" fn result_to_string(_: XrInstance, value: R, buffer: *mut c_char) -> R {
    let text = result_name(value);
    let out = std::slice::from_raw_parts_mut(buffer, openxr_sys::MAX_RESULT_STRING_SIZE);
    for (i, b) in text.bytes().take(out.len() - 1).enumerate() {
        out[i] = b as c_char;
    }
    out[text.len().min(out.len() - 1)] = 0;
    R::SUCCESS
}

pub unsafe extern "system" fn structure_type_to_string(
    _: XrInstance,
    value: StructureType,
    buffer: *mut c_char,
) -> R {
    let text = structure_type_name(value);
    let out = std::slice::from_raw_parts_mut(buffer, openxr_sys::MAX_STRUCTURE_NAME_SIZE);
    for (i, b) in text.bytes().take(out.len() - 1).enumerate() {
        out[i] = b as c_char;
    }
    out[text.len().min(out.len() - 1)] = 0;
    R::SUCCESS
}

/// A path: `/` and then components between slashes, none empty, none just `.` or `..`, each of
/// lower-case letters, digits, `-`, `.` and `_`.
pub fn path_format_is_valid(text: &str) -> bool {
    // `XR_MAX_PATH_LENGTH` counts the terminating zero.
    if text.len() >= 256 {
        return false;
    }
    let Some(rest) = text.strip_prefix('/') else {
        return false;
    };
    rest.split('/').all(|c| {
        !c.is_empty()
            && c != "."
            && c != ".."
            && c.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'.' | b'_'))
    })
}

pub unsafe extern "system" fn string_to_path(instance: XrInstance, text: *const c_char, out: *mut Path) -> R {
    let instance = handle!(INSTANCES, instance);
    let text = CStr::from_ptr(text).to_string_lossy();
    if !path_format_is_valid(&text) {
        return R::ERROR_PATH_FORMAT_INVALID;
    }
    *out = Path::from_raw(instance.path(&text));
    R::SUCCESS
}

pub unsafe extern "system" fn path_to_string(
    instance: XrInstance,
    path: Path,
    capacity: u32,
    count_out: *mut u32,
    buffer: *mut c_char,
) -> R {
    let instance = handle!(INSTANCES, instance);
    let Some(text) = instance.path_text(path.into_raw()) else {
        return R::ERROR_PATH_INVALID;
    };
    *count_out = text.len() as u32 + 1;
    if capacity == 0 {
        return R::SUCCESS;
    }
    if (capacity as usize) < text.len() + 1 {
        return R::ERROR_SIZE_INSUFFICIENT;
    }
    for (i, b) in text.bytes().enumerate() {
        *buffer.add(i) = b as c_char;
    }
    *buffer.add(text.len()) = 0;
    R::SUCCESS
}

// ---- system and view configuration ----------------------------------------------------------

pub unsafe extern "system" fn get_system(
    instance: XrInstance,
    info: *const openxr_sys::SystemGetInfo,
    out: *mut SystemId,
) -> R {
    let _ = handle!(INSTANCES, instance);
    if (*info).form_factor != openxr_sys::FormFactor::HEAD_MOUNTED_DISPLAY {
        return R::ERROR_FORM_FACTOR_UNSUPPORTED;
    }
    *out = SystemId::from_raw(SYSTEM_ID);
    R::SUCCESS
}

pub unsafe extern "system" fn get_system_properties(
    instance: XrInstance,
    system: SystemId,
    properties: *mut openxr_sys::SystemProperties,
) -> R {
    let _ = handle!(INSTANCES, instance);
    if system.into_raw() != SYSTEM_ID {
        return R::ERROR_SYSTEM_INVALID;
    }
    let p = &mut *properties;
    p.system_id = system;
    p.vendor_id = 0;
    write_str(&mut p.system_name, "Spatiand");
    p.graphics_properties = openxr_sys::SystemGraphicsProperties {
        max_swapchain_image_height: 8192,
        max_swapchain_image_width: 8192,
        max_layer_count: MAX_LAYERS,
    };
    p.tracking_properties = openxr_sys::SystemTrackingProperties {
        orientation_tracking: Bool32::from(true),
        position_tracking: Bool32::from(true),
    };
    R::SUCCESS
}

pub unsafe extern "system" fn enumerate_environment_blend_modes(
    instance: XrInstance,
    system: SystemId,
    view_configuration: ViewConfigurationType,
    capacity: u32,
    count_out: *mut u32,
    modes: *mut EnvironmentBlendMode,
) -> R {
    let _ = handle!(INSTANCES, instance);
    if system.into_raw() != SYSTEM_ID {
        return R::ERROR_SYSTEM_INVALID;
    }
    if view_configuration != ViewConfigurationType::PRIMARY_STEREO {
        return R::ERROR_VIEW_CONFIGURATION_TYPE_UNSUPPORTED;
    }
    two_call(capacity, count_out, &[EnvironmentBlendMode::OPAQUE], modes)
}

pub unsafe extern "system" fn enumerate_view_configurations(
    instance: XrInstance,
    _system: SystemId,
    capacity: u32,
    count_out: *mut u32,
    types: *mut ViewConfigurationType,
) -> R {
    let _ = handle!(INSTANCES, instance);
    two_call(capacity, count_out, &[ViewConfigurationType::PRIMARY_STEREO], types)
}

pub unsafe extern "system" fn get_view_configuration_properties(
    instance: XrInstance,
    _system: SystemId,
    kind: ViewConfigurationType,
    properties: *mut openxr_sys::ViewConfigurationProperties,
) -> R {
    let _ = handle!(INSTANCES, instance);
    if kind != ViewConfigurationType::PRIMARY_STEREO {
        return R::ERROR_VIEW_CONFIGURATION_TYPE_UNSUPPORTED;
    }
    let p = &mut *properties;
    p.view_configuration_type = kind;
    p.fov_mutable = Bool32::from(false);
    R::SUCCESS
}

pub unsafe extern "system" fn enumerate_view_configuration_views(
    instance: XrInstance,
    _system: SystemId,
    kind: ViewConfigurationType,
    capacity: u32,
    count_out: *mut u32,
    views: *mut openxr_sys::ViewConfigurationView,
) -> R {
    let _ = handle!(INSTANCES, instance);
    if kind != ViewConfigurationType::PRIMARY_STEREO {
        return R::ERROR_VIEW_CONFIGURATION_TYPE_UNSUPPORTED;
    }
    *count_out = 2;
    if capacity == 0 {
        return R::SUCCESS;
    }
    if capacity < 2 {
        return R::ERROR_SIZE_INSUFFICIENT;
    }
    let (w, h) = eye_size();
    for i in 0..2 {
        let v = &mut *views.add(i);
        v.recommended_image_rect_width = w;
        v.max_image_rect_width = 8192;
        v.recommended_image_rect_height = h;
        v.max_image_rect_height = 8192;
        v.recommended_swapchain_sample_count = 1;
        // Multisampled images cannot be copied out of; an application resolves them itself.
        v.max_swapchain_sample_count = 1;
    }
    R::SUCCESS
}

// ---- Vulkan (XR_KHR_vulkan_enable2) ----------------------------------------------------------

pub unsafe extern "system" fn get_vulkan_graphics_requirements2(
    instance: XrInstance,
    system: SystemId,
    requirements: *mut openxr_sys::GraphicsRequirementsVulkanKHR,
) -> R {
    let instance = handle!(INSTANCES, instance);
    if !instance.has_extension("XR_KHR_vulkan_enable2") && !instance.has_extension("XR_KHR_vulkan_enable") {
        return R::ERROR_FUNCTION_UNSUPPORTED;
    }
    if system.into_raw() != SYSTEM_ID {
        return R::ERROR_SYSTEM_INVALID;
    }
    instance.requirements_asked.store(true, std::sync::atomic::Ordering::Relaxed);
    let r = &mut *requirements;
    r.min_api_version_supported = openxr_sys::Version::new(1, 0, 0);
    r.max_api_version_supported = openxr_sys::Version::new(1, 4, 0);
    R::SUCCESS
}

pub unsafe extern "system" fn get_opengl_graphics_requirements(
    instance: XrInstance,
    system: SystemId,
    requirements: *mut openxr_sys::GraphicsRequirementsOpenGLKHR,
) -> R {
    let instance = handle!(INSTANCES, instance);
    if !instance.has_extension("XR_KHR_opengl_enable") {
        return R::ERROR_FUNCTION_UNSUPPORTED;
    }
    if system.into_raw() != SYSTEM_ID {
        return R::ERROR_SYSTEM_INVALID;
    }
    instance.requirements_asked.store(true, std::sync::atomic::Ordering::Relaxed);
    let r = &mut *requirements;
    // Texture storage and memory objects are what is used, which is 4.5 in spirit and 4.2 in fact.
    r.min_api_version_supported = openxr_sys::Version::new(4, 2, 0);
    r.max_api_version_supported = openxr_sys::Version::new(4, 6, 0);
    R::SUCCESS
}

pub unsafe extern "system" fn create_vulkan_instance(
    instance: XrInstance,
    info: *const openxr_sys::VulkanInstanceCreateInfoKHR,
    out: *mut openxr_sys::platform::VkInstance,
    vk_result: *mut openxr_sys::platform::VkResult,
) -> R {
    let _ = handle!(INSTANCES, instance);
    match crate::vk::create_instance(&*info) {
        Ok((result, created)) => {
            *vk_result = result.as_raw();
            *out = created.as_raw() as usize as *const c_void;
            if result == vk::Result::SUCCESS {
                R::SUCCESS
            } else {
                R::ERROR_VALIDATION_FAILURE
            }
        }
        Err(e) => {
            log::error!("xrCreateVulkanInstanceKHR: {e}");
            R::ERROR_RUNTIME_FAILURE
        }
    }
}

/// The GPU to use: a real one in preference to a software renderer.
unsafe fn pick_physical_device(
    vk_instance: openxr_sys::platform::VkInstance,
) -> Result<openxr_sys::platform::VkPhysicalDevice, R> {
    let entry = ash::Entry::load().map_err(|e| {
        log::error!("no Vulkan loader: {e}");
        R::ERROR_RUNTIME_FAILURE
    })?;
    let instance = ash::Instance::load(entry.static_fn(), vk::Instance::from_raw(vk_instance as u64));
    let devices = match instance.enumerate_physical_devices() {
        Ok(d) if !d.is_empty() => d,
        Ok(_) => {
            log::error!("this Vulkan instance has no physical device to give");
            return Err(R::ERROR_RUNTIME_FAILURE);
        }
        Err(e) => {
            log::error!("enumerating Vulkan physical devices: {e}");
            return Err(R::ERROR_RUNTIME_FAILURE);
        }
    };
    let pick = devices
        .iter()
        .copied()
        .find(|d| {
            let t = instance.get_physical_device_properties(*d).device_type;
            t == vk::PhysicalDeviceType::DISCRETE_GPU || t == vk::PhysicalDeviceType::INTEGRATED_GPU
        })
        .unwrap_or(devices[0]);
    Ok(pick.as_raw() as usize as *const c_void)
}

pub unsafe extern "system" fn get_vulkan_graphics_device2(
    instance: XrInstance,
    info: *const openxr_sys::VulkanGraphicsDeviceGetInfoKHR,
    out: *mut openxr_sys::platform::VkPhysicalDevice,
) -> R {
    let _ = handle!(INSTANCES, instance);
    match pick_physical_device((*info).vulkan_instance) {
        Ok(device) => {
            *out = device;
            R::SUCCESS
        }
        Err(e) => e,
    }
}

// ---- Vulkan (XR_KHR_vulkan_enable, the older form) -----------------------------------------------
//
// `wineopenxr` -- what runs a Windows game's OpenXR on a Linux runtime -- asks for the extensions
// to enable by name rather than handing over its create info, so this form has to exist too.

pub const VULKAN_INSTANCE_EXTENSIONS: &str =
    "VK_KHR_get_physical_device_properties2 VK_KHR_external_memory_capabilities VK_KHR_external_semaphore_capabilities";
pub const VULKAN_DEVICE_EXTENSIONS: &str =
    "VK_KHR_external_memory VK_KHR_external_memory_fd VK_EXT_external_memory_dma_buf VK_KHR_dedicated_allocation VK_KHR_get_memory_requirements2";

unsafe fn names_out(text: &str, capacity: u32, count_out: *mut u32, buffer: *mut c_char) -> R {
    *count_out = text.len() as u32 + 1;
    if capacity == 0 {
        return R::SUCCESS;
    }
    if (capacity as usize) < text.len() + 1 {
        return R::ERROR_SIZE_INSUFFICIENT;
    }
    for (i, b) in text.bytes().enumerate() {
        *buffer.add(i) = b as c_char;
    }
    *buffer.add(text.len()) = 0;
    R::SUCCESS
}

pub unsafe extern "system" fn get_vulkan_instance_extensions(
    instance: XrInstance,
    _system: SystemId,
    capacity: u32,
    count_out: *mut u32,
    buffer: *mut c_char,
) -> R {
    let _ = handle!(INSTANCES, instance);
    names_out(VULKAN_INSTANCE_EXTENSIONS, capacity, count_out, buffer)
}

pub unsafe extern "system" fn get_vulkan_device_extensions(
    instance: XrInstance,
    _system: SystemId,
    capacity: u32,
    count_out: *mut u32,
    buffer: *mut c_char,
) -> R {
    let _ = handle!(INSTANCES, instance);
    names_out(VULKAN_DEVICE_EXTENSIONS, capacity, count_out, buffer)
}

pub unsafe extern "system" fn get_vulkan_graphics_device(
    instance: XrInstance,
    _system: SystemId,
    vk_instance: openxr_sys::platform::VkInstance,
    out: *mut openxr_sys::platform::VkPhysicalDevice,
) -> R {
    let _ = handle!(INSTANCES, instance);
    match pick_physical_device(vk_instance) {
        Ok(device) => {
            *out = device;
            R::SUCCESS
        }
        Err(e) => e,
    }
}

pub unsafe extern "system" fn create_vulkan_device(
    instance: XrInstance,
    info: *const openxr_sys::VulkanDeviceCreateInfoKHR,
    out: *mut openxr_sys::platform::VkDevice,
    vk_result: *mut openxr_sys::platform::VkResult,
) -> R {
    let _ = handle!(INSTANCES, instance);
    match crate::vk::create_device(&*info) {
        Ok((result, created)) => {
            *vk_result = result.as_raw();
            *out = created.as_raw() as usize as *const c_void;
            if result == vk::Result::SUCCESS {
                R::SUCCESS
            } else {
                R::ERROR_VALIDATION_FAILURE
            }
        }
        Err(e) => {
            log::error!("xrCreateVulkanDeviceKHR: {e}");
            R::ERROR_RUNTIME_FAILURE
        }
    }
}

// ---- session ---------------------------------------------------------------------------------

unsafe fn find_in_chain<T>(mut next: *const c_void, wanted: StructureType) -> Option<*const T> {
    while !next.is_null() {
        let header = &*(next as *const openxr_sys::BaseInStructure);
        if header.ty == wanted {
            return Some(next as *const T);
        }
        next = header.next as *const c_void;
    }
    None
}

pub unsafe extern "system" fn create_session(
    instance: XrInstance,
    info: *const openxr_sys::SessionCreateInfo,
    out: *mut XrSession,
) -> R {
    let instance = handle!(INSTANCES, instance);
    let info = &*info;
    if info.system_id.into_raw() != SYSTEM_ID {
        return R::ERROR_SYSTEM_INVALID;
    }
    let vulkan = find_in_chain::<openxr_sys::GraphicsBindingVulkanKHR>(
        info.next,
        StructureType::GRAPHICS_BINDING_VULKAN_KHR,
    );
    let opengl = find_in_chain::<c_void>(info.next, StructureType::GRAPHICS_BINDING_OPENGL_XLIB_KHR)
        .or_else(|| find_in_chain::<c_void>(info.next, StructureType::GRAPHICS_BINDING_OPENGL_WAYLAND_KHR));
    // A graphics binding is only taken from an application that has asked what its graphics API needs.
    if (vulkan.is_some() || opengl.is_some()) && !instance.requirements_asked.load(std::sync::atomic::Ordering::Relaxed) {
        return R::ERROR_GRAPHICS_REQUIREMENTS_CALL_MISSING;
    }
    let (vk, gl) = if let Some(binding) = vulkan {
        // A binding without its handles names no device to draw with.
        let b = &*binding;
        if b.instance as usize == 0 || b.physical_device as usize == 0 || b.device as usize == 0 {
            log::warn!("xrCreateSession with a Vulkan binding that has a null handle");
            return R::ERROR_GRAPHICS_DEVICE_INVALID;
        }
        match crate::vk::Vk::from_binding(&*binding) {
            Ok(v) => (v, None),
            Err(e) => {
                log::error!("could not use the application's Vulkan: {e}");
                return R::ERROR_GRAPHICS_DEVICE_INVALID;
            }
        }
    } else if opengl.is_some() {
        // The application's context is current on this thread, as OpenXR requires here.
        let gl = match crate::gl::Gl::load() {
            Ok(g) => g,
            Err(e) => {
                log::error!("could not use the application's OpenGL: {e}");
                return R::ERROR_GRAPHICS_DEVICE_INVALID;
            }
        };
        if !gl.can_share() {
            log::error!("this OpenGL cannot share memory with Vulkan (GL_EXT_memory_object_fd), which is how pictures leave it");
            return R::ERROR_GRAPHICS_DEVICE_INVALID;
        }
        match crate::vk::Vk::standalone(gl.device_uuid()) {
            Ok(v) => (v, Some(gl)),
            Err(e) => {
                log::error!("no Vulkan to compose OpenGL's pictures with: {e}");
                return R::ERROR_GRAPHICS_DEVICE_INVALID;
            }
        }
    } else {
        log::warn!("xrCreateSession without a Vulkan or OpenGL binding");
        return R::ERROR_GRAPHICS_DEVICE_INVALID;
    };
    // Poses and a window. Without a compositor to show them in, the session still runs -- the
    // application draws, nothing is shown -- which is how it is tested on a machine with no
    // headset, and a better answer than refusing to start.
    let title = format!("{} (OpenXR)", instance.app_name);
    let link = match crate::wl::Link::connect(&title) {
        Ok(l) => {
            log::info!("connected to the compositor");
            Some(l)
        }
        Err(e) => {
            log::warn!("running without a compositor: {e}");
            None
        }
    };
    if let Some(link) = &link {
        // The compositor answers `get_pose_channel` straight after the window is configured.
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while !link.has_poses() && link.unavailable().is_none() && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
        match link.unavailable() {
            Some(reason) => log::warn!("the compositor has no head tracking to give: {reason}"),
            None if link.has_poses() => log::info!("pose channel mapped"),
            None => log::warn!("the compositor did not send a pose channel in time"),
        }
    }
    let handle = new_handle();
    let session = Arc::new(Session {
        handle,
        instance,
        vk: Some(vk),
        gl,
        link,
        run: std::sync::Mutex::new(SessionRun {
            state: SessionState::Idle,
            begun: false,
            frame_open: false,
            waits: 0,
            last_index: 0,
            next_frame_ns: 0,
            frame_period_ns: 13_888_889,
            local_origin: None,
            warned_layers: false,
            quads: Vec::new(),
            frames: 0,
        }),
        output: std::sync::Mutex::new(None),
        input: std::sync::Mutex::new(InputState::default()),
    });
    SESSIONS.insert(handle, session.clone());
    session.push_state(SessionState::Idle);
    session.push_state(SessionState::Ready);
    *out = XrSession::from_raw(handle);
    R::SUCCESS
}

pub unsafe extern "system" fn destroy_session(session: XrSession) -> R {
    let s = handle!(SESSIONS, session);
    for space in SPACES.all() {
        let _ = space;
    }
    // Events about a session that is gone are not something to tell the application.
    s.instance.events.lock().unwrap().retain(|e| match e {
        Event::SessionState { session, .. } | Event::InteractionProfileChanged { session } => *session != s.handle,
    });
    SESSIONS.remove(s.handle);
    R::SUCCESS
}

pub unsafe extern "system" fn begin_session(session: XrSession, info: *const openxr_sys::SessionBeginInfo) -> R {
    let s = handle!(SESSIONS, session);
    if info.is_null() || (*info).ty != StructureType::SESSION_BEGIN_INFO {
        return R::ERROR_VALIDATION_FAILURE;
    }
    // The one view configuration this runtime has is two eyes.
    let wanted = (*info).primary_view_configuration_type;
    if wanted != ViewConfigurationType::PRIMARY_STEREO {
        return match wanted {
            // The one other the core specification has; a number from an extension that is not enabled
            // is not a view configuration at all.
            ViewConfigurationType::PRIMARY_MONO => R::ERROR_VIEW_CONFIGURATION_TYPE_UNSUPPORTED,
            _ => R::ERROR_VALIDATION_FAILURE,
        };
    }
    {
        let mut run = s.run.lock().unwrap();
        if run.begun {
            return R::ERROR_SESSION_RUNNING;
        }
        if run.state != SessionState::Ready {
            return R::ERROR_SESSION_NOT_READY;
        }
        run.begun = true;
    }
    // SYNCHRONIZED, VISIBLE and FOCUSED follow as frames are submitted -- see `xrEndFrame`.
    log::info!("session begun");
    R::SUCCESS
}

pub unsafe extern "system" fn end_session(session: XrSession) -> R {
    let s = handle!(SESSIONS, session);
    {
        let mut run = s.run.lock().unwrap();
        if !run.begun {
            return R::ERROR_SESSION_NOT_RUNNING;
        }
        if run.state != SessionState::Stopping {
            return R::ERROR_SESSION_NOT_STOPPING;
        }
        run.begun = false;
    }
    s.push_state(SessionState::Idle);
    s.push_state(SessionState::Exiting);
    log::info!("session ended");
    R::SUCCESS
}

pub unsafe extern "system" fn request_exit_session(session: XrSession) -> R {
    let s = handle!(SESSIONS, session);
    if !s.run.lock().unwrap().begun {
        return R::ERROR_SESSION_NOT_RUNNING;
    }
    s.push_stopping();
    R::SUCCESS
}

// ---- spaces ----------------------------------------------------------------------------------

pub unsafe extern "system" fn enumerate_reference_spaces(
    session: XrSession,
    capacity: u32,
    count_out: *mut u32,
    spaces: *mut openxr_sys::ReferenceSpaceType,
) -> R {
    let s = handle!(SESSIONS, session);
    // LOCAL_FLOOR is part of OpenXR 1.1.
    let mut kinds = vec![
        openxr_sys::ReferenceSpaceType::VIEW,
        openxr_sys::ReferenceSpaceType::LOCAL,
        openxr_sys::ReferenceSpaceType::STAGE,
    ];
    if s.instance.api_minor >= 1 {
        kinds.push(openxr_sys::ReferenceSpaceType::LOCAL_FLOOR);
    }
    two_call(capacity, count_out, &kinds, spaces)
}

pub unsafe extern "system" fn get_reference_space_bounds_rect(
    session: XrSession,
    _kind: openxr_sys::ReferenceSpaceType,
    bounds: *mut openxr_sys::Extent2Df,
) -> R {
    let s = handle!(SESSIONS, session);
    let known = matches!(
        _kind,
        openxr_sys::ReferenceSpaceType::VIEW | openxr_sys::ReferenceSpaceType::LOCAL | openxr_sys::ReferenceSpaceType::STAGE
    ) || (_kind == openxr_sys::ReferenceSpaceType::LOCAL_FLOOR && s.instance.api_minor >= 1);
    if !known {
        // Not a value of the enumeration at all, or one from an extension this does not have.
        return if _kind.into_raw() <= 0 || _kind.into_raw() == i32::MAX {
            R::ERROR_VALIDATION_FAILURE
        } else {
            R::ERROR_REFERENCE_SPACE_UNSUPPORTED
        };
    }
    *bounds = openxr_sys::Extent2Df { width: 0.0, height: 0.0 };
    R::SPACE_BOUNDS_UNAVAILABLE
}

pub unsafe extern "system" fn create_reference_space(
    session: XrSession,
    info: *const openxr_sys::ReferenceSpaceCreateInfo,
    out: *mut XrSpace,
) -> R {
    let s = handle!(SESSIONS, session);
    let info = &*info;
    // An orientation has to be a unit quaternion to be a pose at all.
    let q = info.pose_in_reference_space.orientation;
    let length_squared = q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w;
    if !((length_squared - 1.0).abs() <= 0.01) {
        return R::ERROR_POSE_INVALID;
    }
    let kind = match info.reference_space_type {
        openxr_sys::ReferenceSpaceType::VIEW => SpaceKind::View,
        openxr_sys::ReferenceSpaceType::LOCAL => SpaceKind::Local,
        openxr_sys::ReferenceSpaceType::STAGE => SpaceKind::Stage,
        // The floor under LOCAL: where STAGE puts it, as Spatiand has no room to measure.
        openxr_sys::ReferenceSpaceType::LOCAL_FLOOR if s.instance.api_minor >= 1 => SpaceKind::Stage,
        _ => return R::ERROR_REFERENCE_SPACE_UNSUPPORTED,
    };
    let handle = new_handle();
    SPACES.insert(
        handle,
        Arc::new(Space {
            session: s.handle,
            kind,
            offset: Pose::from_raw(&info.pose_in_reference_space),
            action: None,
        }),
    );
    *out = XrSpace::from_raw(handle);
    R::SUCCESS
}

pub unsafe extern "system" fn create_action_space(
    session: XrSession,
    info: *const openxr_sys::ActionSpaceCreateInfo,
    out: *mut XrSpace,
) -> R {
    let s = handle!(SESSIONS, session);
    let handle = new_handle();
    SPACES.insert(
        handle,
        Arc::new(Space {
            session: s.handle,
            kind: SpaceKind::Action,
            offset: Pose::from_raw(&(*info).pose_in_action_space),
            action: Some(((*info).action.into_raw(), (*info).subaction_path.into_raw())),
        }),
    );
    *out = XrSpace::from_raw(handle);
    R::SUCCESS
}

pub unsafe extern "system" fn destroy_space(space: XrSpace) -> R {
    let _ = handle!(SPACES, space);
    SPACES.remove(space.into_raw());
    R::SUCCESS
}

pub unsafe extern "system" fn locate_space(
    space: XrSpace,
    base: XrSpace,
    time: Time,
    location: *mut openxr_sys::SpaceLocation,
) -> R {
    use openxr_sys::SpaceLocationFlags as F;
    let space = handle!(SPACES, space);
    let base = handle!(SPACES, base);
    let Some(session) = SESSIONS.get(space.session) else {
        return R::ERROR_HANDLE_INVALID;
    };
    let out = &mut *location;
    let t = time.as_nanos();
    if t <= 0 {
        return R::ERROR_TIME_INVALID;
    }
    match (session.world_from(&space, t), session.world_from(&base, t)) {
        (Some(world), Some(base_world)) => {
            out.location_flags = F::POSITION_VALID | F::ORIENTATION_VALID | F::POSITION_TRACKED | F::ORIENTATION_TRACKED;
            out.pose = base_world.inverse().then(&world).to_raw();
        }
        _ => {
            // A hand or a controller: nothing here tracks one yet.
            out.location_flags = F::EMPTY;
            out.pose = Pose::IDENTITY.to_raw();
        }
    }
    R::SUCCESS
}

/// `xrLocateSpaces` (OpenXR 1.1 and `XR_KHR_locate_spaces`): several spaces against one base.
pub unsafe extern "system" fn locate_spaces(
    session: XrSession,
    info: *const openxr_sys::SpacesLocateInfo,
    locations: *mut openxr_sys::SpaceLocations,
) -> R {
    use openxr_sys::SpaceLocationFlags as F;
    let s = handle!(SESSIONS, session);
    if info.is_null() || locations.is_null() {
        return R::ERROR_VALIDATION_FAILURE;
    }
    let info = &*info;
    let out = &mut *locations;
    if info.ty != StructureType::SPACES_LOCATE_INFO || out.ty != StructureType::SPACE_LOCATIONS {
        return R::ERROR_VALIDATION_FAILURE;
    }
    if info.space_count == 0 {
        return R::ERROR_VALIDATION_FAILURE;
    }
    if out.location_count != info.space_count {
        return R::ERROR_VALIDATION_FAILURE;
    }
    // Velocities, if asked for, come in a structure chained on, one for each space.
    let mut velocities: Option<&mut openxr_sys::SpaceVelocities> = None;
    let mut next = out.next as *mut openxr_sys::BaseOutStructure;
    while !next.is_null() {
        if (*next).ty == StructureType::SPACE_VELOCITIES {
            velocities = Some(&mut *(next as *mut openxr_sys::SpaceVelocities));
            break;
        }
        next = (*next).next;
    }
    if let Some(v) = &velocities {
        if v.velocity_count != info.space_count {
            return R::ERROR_VALIDATION_FAILURE;
        }
    }
    if info.time.as_nanos() <= 0 {
        return R::ERROR_TIME_INVALID;
    }
    let base = handle!(SPACES, info.base_space);
    let t = info.time.as_nanos();
    let base_world = s.world_from(&base, t);
    let spaces = std::slice::from_raw_parts(info.spaces, info.space_count as usize);
    out.location_count = info.space_count;
    for (i, space) in spaces.iter().enumerate() {
        let space = handle!(SPACES, *space);
        // Nothing here moves relative to anything else the application can name that fast: said as
        // standing still, which is exact for spaces fixed to one another.
        if let Some(v) = &velocities {
            let slot = &mut *v.velocities.add(i);
            slot.velocity_flags = openxr_sys::SpaceVelocityFlags::LINEAR_VALID | openxr_sys::SpaceVelocityFlags::ANGULAR_VALID;
            slot.linear_velocity = openxr_sys::Vector3f { x: 0.0, y: 0.0, z: 0.0 };
            slot.angular_velocity = openxr_sys::Vector3f { x: 0.0, y: 0.0, z: 0.0 };
        }
        let slot = &mut *out.locations.add(i);
        match (s.world_from(&space, t), base_world) {
            (Some(world), Some(base_world)) => {
                slot.location_flags =
                    F::POSITION_VALID | F::ORIENTATION_VALID | F::POSITION_TRACKED | F::ORIENTATION_TRACKED;
                slot.pose = base_world.inverse().then(&world).to_raw();
            }
            _ => {
                slot.location_flags = F::EMPTY;
                slot.pose = Pose::IDENTITY.to_raw();
            }
        }
    }
    R::SUCCESS
}

pub unsafe extern "system" fn locate_views(
    session: XrSession,
    info: *const openxr_sys::ViewLocateInfo,
    state: *mut openxr_sys::ViewState,
    capacity: u32,
    count_out: *mut u32,
    views: *mut openxr_sys::View,
) -> R {
    use openxr_sys::ViewStateFlags as F;
    let s = handle!(SESSIONS, session);
    let info = &*info;
    if info.view_configuration_type != ViewConfigurationType::PRIMARY_STEREO {
        return R::ERROR_VIEW_CONFIGURATION_TYPE_UNSUPPORTED;
    }
    if info.display_time.as_nanos() <= 0 {
        return R::ERROR_TIME_INVALID;
    }
    let space = handle!(SPACES, info.space);
    *count_out = 2;
    if capacity == 0 {
        return R::SUCCESS;
    }
    if capacity < 2 {
        return R::ERROR_SIZE_INSUFFICIENT;
    }
    let t = info.display_time.as_nanos();
    let sample = s.sample_at(t);
    let Some(space_world) = s.world_from(&space, t) else {
        (*state).view_state_flags = F::EMPTY;
        return R::SUCCESS;
    };
    let from_world = space_world.inverse();
    for i in 0..2 {
        let v = &mut *views.add(i);
        v.ty = StructureType::VIEW;
        v.pose = from_world.then(&sample.eyes[i].pose).to_raw();
        let fov = sample.eyes[i].fov;
        v.fov = openxr_sys::Fovf {
            angle_left: fov[0],
            angle_right: fov[1],
            angle_up: fov[2],
            angle_down: fov[3],
        };
    }
    (*state).view_state_flags = F::POSITION_VALID | F::ORIENTATION_VALID | F::POSITION_TRACKED | F::ORIENTATION_TRACKED;
    R::SUCCESS
}

// ---- swapchains ------------------------------------------------------------------------------

pub unsafe extern "system" fn enumerate_swapchain_formats(
    session: XrSession,
    capacity: u32,
    count_out: *mut u32,
    formats: *mut i64,
) -> R {
    let s = handle!(SESSIONS, session);
    if s.gl.is_some() {
        let all: Vec<i64> = crate::gl::FORMATS.iter().map(|f| *f as i64).collect();
        return two_call(capacity, count_out, &all, formats);
    }
    let all: Vec<i64> = crate::vk::COLOR_FORMATS
        .iter()
        .chain(crate::vk::DEPTH_FORMATS.iter())
        .map(|f| f.as_raw() as i64)
        .collect();
    two_call(capacity, count_out, &all, formats)
}

pub unsafe extern "system" fn create_swapchain(
    session: XrSession,
    info: *const openxr_sys::SwapchainCreateInfo,
    out: *mut XrSwapchain,
) -> R {
    use openxr_sys::SwapchainUsageFlags as U;
    let s = handle!(SESSIONS, session);
    let info = &*info;
    let Some(vkd) = &s.vk else {
        return R::ERROR_GRAPHICS_DEVICE_INVALID;
    };
    if info.sample_count > 1 {
        return R::ERROR_FEATURE_UNSUPPORTED;
    }
    if let Some(gl) = &s.gl {
        return create_gl_swapchain(&s, vkd, gl, info, out);
    }
    let format = vk::Format::from_raw(info.format as i32);
    if !crate::vk::COLOR_FORMATS.contains(&format) && !crate::vk::is_depth(format) {
        return R::ERROR_SWAPCHAIN_FORMAT_UNSUPPORTED;
    }
    let mut usage = vk::ImageUsageFlags::empty();
    let flags = info.usage_flags;
    if flags.contains(U::COLOR_ATTACHMENT) {
        usage |= vk::ImageUsageFlags::COLOR_ATTACHMENT;
    }
    if flags.contains(U::DEPTH_STENCIL_ATTACHMENT) {
        usage |= vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT;
    }
    if flags.contains(U::UNORDERED_ACCESS) {
        usage |= vk::ImageUsageFlags::STORAGE;
    }
    if flags.contains(U::TRANSFER_SRC) {
        usage |= vk::ImageUsageFlags::TRANSFER_SRC;
    }
    if flags.contains(U::TRANSFER_DST) {
        usage |= vk::ImageUsageFlags::TRANSFER_DST;
    }
    if flags.contains(U::SAMPLED) {
        usage |= vk::ImageUsageFlags::SAMPLED;
    }
    let mut images = Vec::new();
    // A static swapchain is one image, drawn once.
    let image_count = if info.create_flags.contains(openxr_sys::SwapchainCreateFlags::STATIC_IMAGE) { 1 } else { 3 };
    for _ in 0..image_count {
        match vkd.create_image(
            format,
            info.width,
            info.height,
            info.array_size,
            info.mip_count,
            vk::SampleCountFlags::TYPE_1,
            usage,
        ) {
            Ok(image) => images.push(image),
            Err(e) => {
                log::error!("xrCreateSwapchain: {e}");
                for image in &images {
                    vkd.destroy_image(image);
                }
                return R::ERROR_RUNTIME_FAILURE;
            }
        }
    }
    let handle = new_handle();
    log::info!(
        "swapchain {:?} {}x{} x{} layers",
        format,
        info.width,
        info.height,
        info.array_size.max(1)
    );
    SWAPCHAINS.insert(
        handle,
        Arc::new(Swapchain {
            session: s.handle,
            format,
            width: info.width,
            height: info.height,
            array_size: info.array_size.max(1),
            images,
            state: std::sync::Mutex::new(SwapState {
                is_static: info.create_flags.contains(openxr_sys::SwapchainCreateFlags::STATIC_IMAGE),
                ..SwapState::default()
            }),
            gl: None,
        }),
    );
    *out = XrSwapchain::from_raw(handle);
    R::SUCCESS
}

/// A swapchain for an application drawing with OpenGL: ordinary textures for it to draw into, and
/// for each layer of each a linear staging image shared with Vulkan, which release copies into.
unsafe fn create_gl_swapchain(
    s: &Arc<Session>,
    vkd: &crate::vk::Vk,
    gl: &crate::gl::Gl,
    info: &openxr_sys::SwapchainCreateInfo,
    out: *mut XrSwapchain,
) -> R {
    let internal = info.format as u32;
    if !crate::gl::FORMATS.contains(&internal) {
        return R::ERROR_SWAPCHAIN_FORMAT_UNSUPPORTED;
    }
    let layers = info.array_size.max(1);
    let colour = crate::gl::colour_format(internal);
    let mut images = Vec::new();
    let mut textures = Vec::new();
    let mut staging: Vec<Vec<(u32, u32)>> = Vec::new();
    let mut failed = None;
    let image_count = if info.create_flags.contains(openxr_sys::SwapchainCreateFlags::STATIC_IMAGE) { 1 } else { 3 };
    'images: for _ in 0..image_count {
        match gl.plain_texture(internal, info.width, info.height, layers) {
            Ok(texture) => textures.push(texture),
            Err(e) => {
                failed = Some(e);
                break;
            }
        }
        let mut mine = Vec::new();
        if let Some(format) = colour {
            for _ in 0..layers {
                let made = vkd
                    .create_shared_image(format, info.width, info.height, 1, true)
                    .and_then(|shared| {
                        match gl.import_texture(internal, info.width, info.height, 1, shared.size, shared.fd, true) {
                            Ok(pair) => {
                                images.push(shared.image);
                                Ok(pair)
                            }
                            Err(e) => {
                                vkd.destroy_image(&shared.image);
                                Err(e)
                            }
                        }
                    });
                match made {
                    Ok(pair) => mine.push(pair),
                    Err(e) => {
                        failed = Some(e);
                        staging.push(mine);
                        break 'images;
                    }
                }
            }
        }
        staging.push(mine);
    }
    if let Some(e) = failed {
        log::error!("xrCreateSwapchain (OpenGL): {e}");
        for texture in &textures {
            gl.delete(*texture, None);
        }
        for (texture, memory) in staging.iter().flatten() {
            gl.delete(*texture, Some(*memory));
        }
        for image in &images {
            vkd.destroy_image(image);
        }
        return R::ERROR_RUNTIME_FAILURE;
    }
    let handle = new_handle();
    log::info!("swapchain (OpenGL {:#x}) {}x{} x{} layers", internal, info.width, info.height, layers);
    SWAPCHAINS.insert(
        handle,
        Arc::new(Swapchain {
            session: s.handle,
            format: colour.unwrap_or(vk::Format::UNDEFINED),
            width: info.width,
            height: info.height,
            array_size: layers,
            images,
            state: std::sync::Mutex::new(SwapState {
                is_static: info.create_flags.contains(openxr_sys::SwapchainCreateFlags::STATIC_IMAGE),
                ..SwapState::default()
            }),
            gl: Some(GlSwapchain { textures, array: layers > 1, layers, staging }),
        }),
    );
    *out = XrSwapchain::from_raw(handle);
    R::SUCCESS
}

pub unsafe extern "system" fn destroy_swapchain(swapchain: XrSwapchain) -> R {
    let sc = handle!(SWAPCHAINS, swapchain);
    if let Some(session) = SESSIONS.get(sc.session) {
        if let Some(vkd) = &session.vk {
            let _ = vkd.device.device_wait_idle();
            if let (Some(gl), Some(side)) = (&session.gl, &sc.gl) {
                for texture in &side.textures {
                    gl.delete(*texture, None);
                }
                for (texture, memory) in side.staging.iter().flatten() {
                    gl.delete(*texture, Some(*memory));
                }
            }
            for image in &sc.images {
                vkd.destroy_image(image);
            }
        }
    }
    SWAPCHAINS.remove(swapchain.into_raw());
    R::SUCCESS
}

pub unsafe extern "system" fn enumerate_swapchain_images(
    swapchain: XrSwapchain,
    capacity: u32,
    count_out: *mut u32,
    images: *mut openxr_sys::SwapchainImageBaseHeader,
) -> R {
    let sc = handle!(SWAPCHAINS, swapchain);
    *count_out = sc.count() as u32;
    if capacity == 0 {
        return R::SUCCESS;
    }
    if (capacity as usize) < sc.count() {
        return R::ERROR_SIZE_INSUFFICIENT;
    }
    if let Some(side) = &sc.gl {
        let out = images as *mut openxr_sys::SwapchainImageOpenGLKHR;
        for (i, texture) in side.textures.iter().enumerate() {
            let slot = &mut *out.add(i);
            if slot.ty != StructureType::SWAPCHAIN_IMAGE_OPENGL_KHR {
                return R::ERROR_VALIDATION_FAILURE;
            }
            slot.image = *texture;
        }
        return R::SUCCESS;
    }
    let out = images as *mut openxr_sys::SwapchainImageVulkanKHR;
    for (i, image) in sc.images.iter().enumerate() {
        let slot = &mut *out.add(i);
        if slot.ty != StructureType::SWAPCHAIN_IMAGE_VULKAN_KHR {
            return R::ERROR_VALIDATION_FAILURE;
        }
        slot.image = image.image.as_raw();
    }
    R::SUCCESS
}

pub unsafe extern "system" fn acquire_swapchain_image(
    swapchain: XrSwapchain,
    _info: *const openxr_sys::SwapchainImageAcquireInfo,
    index: *mut u32,
) -> R {
    let sc = handle!(SWAPCHAINS, swapchain);
    let mut state = sc.state.lock().unwrap();
    if state.acquired.len() >= sc.count() || (state.is_static && state.static_acquired) {
        return R::ERROR_CALL_ORDER_INVALID;
    }
    state.static_acquired = true;
    let i = state.next;
    state.next = (i + 1) % sc.count() as u32;
    state.acquired.push_back(i);
    *index = i;
    R::SUCCESS
}

pub unsafe extern "system" fn wait_swapchain_image(
    swapchain: XrSwapchain,
    _info: *const openxr_sys::SwapchainImageWaitInfo,
) -> R {
    let sc = handle!(SWAPCHAINS, swapchain);
    let mut state = sc.state.lock().unwrap();
    // Nothing acquired, or the oldest image already waited on and not yet released: one image at a
    // time is waited on.
    if state.acquired.is_empty() || state.waited {
        return R::ERROR_CALL_ORDER_INVALID;
    }
    // Every frame is finished with its images before `xrEndFrame` returns, so there is never
    // anything to wait for.
    state.waited = true;
    R::SUCCESS
}

pub unsafe extern "system" fn release_swapchain_image(
    swapchain: XrSwapchain,
    _info: *const openxr_sys::SwapchainImageReleaseInfo,
) -> R {
    let sc = handle!(SWAPCHAINS, swapchain);
    let mut state = sc.state.lock().unwrap();
    // Released only once waited on.
    if state.acquired.is_empty() || !state.waited {
        return R::ERROR_CALL_ORDER_INVALID;
    }
    let Some(i) = state.acquired.pop_front() else {
        return R::ERROR_CALL_ORDER_INVALID;
    };
    state.waited = false;
    state.released = Some(i);
    // An OpenGL application's picture goes into the staging images Vulkan reads, on the GPU and in
    // the order of its own commands, so it is whatever it had drawn by now.
    if let (Some(side), Some(session)) = (&sc.gl, SESSIONS.get(sc.session)) {
        if let Some(gl) = &session.gl {
            for (layer, (staging, _)) in side.staging[i as usize].iter().enumerate() {
                gl.copy_layer(side.textures[i as usize], side.array, layer as u32, *staging, sc.width, sc.height);
            }
        }
    }
    R::SUCCESS
}

// ---- the frame loop ----------------------------------------------------------------------------

pub unsafe extern "system" fn wait_frame(
    session: XrSession,
    info: *const openxr_sys::FrameWaitInfo,
    state: *mut openxr_sys::FrameState,
) -> R {
    let s = handle!(SESSIONS, session);
    if !info.is_null() && (*info).ty != StructureType::FRAME_WAIT_INFO {
        return R::ERROR_VALIDATION_FAILURE;
    }
    let (last_index, period, next_due) = {
        let run = s.run.lock().unwrap();
        if !run.begun {
            return R::ERROR_SESSION_NOT_RUNNING;
        }
        (run.last_index, run.frame_period_ns, run.next_frame_ns)
    };
    // Paced by the compositor: it writes a pose once per display frame, so a new one is the
    // signal that a frame is wanted. With no compositor, by the clock.
    let mut index = last_index;
    match &s.link {
        Some(link) if link.has_poses() => {
            let give_up = now_ns() + period * 3;
            loop {
                index = link.write_index();
                if index != last_index || now_ns() > give_up {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_micros(300));
            }
        }
        _ => {
            let wait = next_due - now_ns();
            if wait > 0 {
                std::thread::sleep(std::time::Duration::from_nanos(wait as u64));
            }
        }
    }
    let history = s.now_history();
    let period = estimate_period(&history).unwrap_or(period);
    let now = now_ns();
    let predicted = match history.newest() {
        Some(newest) if newest.predicted_ns > now => newest.predicted_ns,
        _ => now + period,
    };
    {
        let mut run = s.run.lock().unwrap();
        run.last_index = index;
        run.frame_period_ns = period;
        run.next_frame_ns = now.max(next_due) + period;
    }
    let out = &mut *state;
    out.predicted_display_time = Time::from_nanos(predicted);
    out.predicted_display_period = Duration::from_nanos(period);
    out.should_render = Bool32::from(true);
    s.run.lock().unwrap().waits += 1;
    R::SUCCESS
}

/// The time between the compositor's samples, if there are enough to tell.
fn estimate_period(history: &crate::pose::History) -> Option<i64> {
    let samples = history.samples();
    if samples.len() < 3 {
        return None;
    }
    let span = samples.last()?.sample_ns - samples.first()?.sample_ns;
    let period = span / (samples.len() as i64 - 1);
    // A display between 25 and 240 Hz; anything else is a stalled compositor, not a refresh rate.
    (4_000_000..=40_000_000).contains(&period).then_some(period)
}

pub unsafe extern "system" fn begin_frame(session: XrSession, info: *const openxr_sys::FrameBeginInfo) -> R {
    let s = handle!(SESSIONS, session);
    if !info.is_null() && (*info).ty != StructureType::FRAME_BEGIN_INFO {
        return R::ERROR_VALIDATION_FAILURE;
    }
    let mut run = s.run.lock().unwrap();
    if !run.begun {
        return R::ERROR_SESSION_NOT_RUNNING;
    }
    // A frame is begun for a wait that has not been begun yet; a second begin for the same wait is
    // out of order, and leaves the frame that is open alone.
    if run.waits == 0 {
        return R::ERROR_CALL_ORDER_INVALID;
    }
    run.waits -= 1;
    let was_open = run.frame_open;
    run.frame_open = true;
    if was_open {
        R::FRAME_DISCARDED
    } else {
        R::SUCCESS
    }
}

/// How many layers an application may submit in a frame.
pub const MAX_LAYERS: u32 = 16;

pub unsafe extern "system" fn end_frame(session: XrSession, info: *const openxr_sys::FrameEndInfo) -> R {
    let s = handle!(SESSIONS, session);
    if info.is_null() || (*info).ty != StructureType::FRAME_END_INFO {
        return R::ERROR_VALIDATION_FAILURE;
    }
    let info = &*info;
    {
        let mut run = s.run.lock().unwrap();
        if !run.begun {
            return R::ERROR_SESSION_NOT_RUNNING;
        }
        if !run.frame_open {
            return R::ERROR_CALL_ORDER_INVALID;
        }
        // A frame that fails here stays open: the next begin discards it.
        if info.display_time.as_nanos() <= 0 {
            return R::ERROR_TIME_INVALID;
        }
        if info.environment_blend_mode != EnvironmentBlendMode::OPAQUE {
            return R::ERROR_ENVIRONMENT_BLEND_MODE_UNSUPPORTED;
        }
    }
    let layers = if info.layer_count == 0 {
        &[][..]
    } else {
        std::slice::from_raw_parts(info.layers, info.layer_count as usize)
    };
    if layers.iter().any(|l| l.is_null()) {
        return R::ERROR_LAYER_INVALID;
    }
    if layers.len() > MAX_LAYERS as usize {
        return R::ERROR_LAYER_LIMIT_EXCEEDED;
    }
    let valid = crate::present::validate_layers(layers);
    if valid != R::SUCCESS {
        return valid;
    }
    let next = {
        let mut run = s.run.lock().unwrap();
        run.frame_open = false;
        run.frames += 1;
        // A session is synchronized once it has submitted a frame, visible when the next arrives and
        // focused after that: one step a frame, so each is a state the application is seen to be in.
        match run.state {
            SessionState::Ready => Some(SessionState::Synchronized),
            SessionState::Synchronized => Some(SessionState::Visible),
            SessionState::Visible => Some(SessionState::Focused),
            _ => None,
        }
    };
    if let Some(next) = next {
        s.push_state(next);
    }
    crate::present::submit(&s, layers, info.display_time.as_nanos())
}

// ---- time ------------------------------------------------------------------------------------

pub unsafe extern "system" fn convert_time_to_timespec(
    instance: XrInstance,
    time: Time,
    out: *mut libc::timespec,
) -> R {
    let _ = handle!(INSTANCES, instance);
    if time.as_nanos() <= 0 {
        return R::ERROR_TIME_INVALID;
    }
    *out = crate::time::to_timespec(time.as_nanos());
    R::SUCCESS
}

pub unsafe extern "system" fn convert_timespec_to_time(
    instance: XrInstance,
    ts: *const libc::timespec,
    out: *mut Time,
) -> R {
    let _ = handle!(INSTANCES, instance);
    let t = crate::time::from_timespec(&*ts);
    if t <= 0 || (*ts).tv_nsec < 0 || (*ts).tv_nsec >= 1_000_000_000 {
        return R::ERROR_TIME_INVALID;
    }
    *out = Time::from_nanos(t);
    R::SUCCESS
}

// ---- actions (accepted, bound to nothing yet) --------------------------------------------------

/// Whether an action set's or action's name is one: a component of a path, so lower-case letters,
/// digits, `-`, `.` and `_`. Empty is a bad name; anything else that is not allowed is a bad path.
fn check_name(name: &str) -> Result<(), R> {
    if name.is_empty() {
        return Err(R::ERROR_NAME_INVALID);
    }
    if name.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'.' | b'_')) {
        Ok(())
    } else {
        Err(R::ERROR_PATH_FORMAT_INVALID)
    }
}

pub unsafe extern "system" fn create_action_set(
    instance: XrInstance,
    info: *const openxr_sys::ActionSetCreateInfo,
    out: *mut openxr_sys::ActionSet,
) -> R {
    let instance = handle!(INSTANCES, instance);
    if info.is_null() || (*info).ty != StructureType::ACTION_SET_CREATE_INFO {
        return R::ERROR_VALIDATION_FAILURE;
    }
    let name = read_str(&(*info).action_set_name);
    let localized = read_str(&(*info).localized_action_set_name);
    if let Err(e) = check_name(&name) {
        return e;
    }
    if localized.is_empty() {
        return R::ERROR_LOCALIZED_NAME_INVALID;
    }
    for other in ACTION_SETS.all().iter().filter(|o| o.instance == instance.handle) {
        if other.name == name {
            return R::ERROR_NAME_DUPLICATED;
        }
        if other.localized == localized {
            return R::ERROR_LOCALIZED_NAME_DUPLICATED;
        }
    }
    let handle = new_handle();
    ACTION_SETS.insert(handle, Arc::new(ActionSet { instance: instance.handle, name, localized }));
    *out = openxr_sys::ActionSet::from_raw(handle);
    R::SUCCESS
}

pub unsafe extern "system" fn destroy_action_set(set: openxr_sys::ActionSet) -> R {
    let _ = handle!(ACTION_SETS, set);
    ACTION_SETS.remove(set.into_raw());
    R::SUCCESS
}

pub unsafe extern "system" fn create_action(
    set: openxr_sys::ActionSet,
    info: *const openxr_sys::ActionCreateInfo,
    out: *mut openxr_sys::Action,
) -> R {
    let owner = handle!(ACTION_SETS, set);
    if info.is_null() || (*info).ty != StructureType::ACTION_CREATE_INFO {
        return R::ERROR_VALIDATION_FAILURE;
    }
    let info = &*info;
    // Once the application has attached an action set nothing is added to it.
    if SESSIONS
        .all()
        .iter()
        .any(|s| s.instance.handle == owner.instance && s.input.lock().unwrap().attached_sets.contains(&set.into_raw()))
    {
        return R::ERROR_ACTIONSETS_ALREADY_ATTACHED;
    }
    let name = read_str(&info.action_name);
    let localized = read_str(&info.localized_action_name);
    if let Err(e) = check_name(&name) {
        return e;
    }
    if localized.is_empty() {
        return R::ERROR_LOCALIZED_NAME_INVALID;
    }
    use openxr_sys::ActionType as T;
    if !matches!(
        info.action_type,
        T::BOOLEAN_INPUT | T::FLOAT_INPUT | T::VECTOR2F_INPUT | T::POSE_INPUT | T::VIBRATION_OUTPUT
    ) {
        return R::ERROR_VALIDATION_FAILURE;
    }
    let subaction_paths: Vec<u64> = if info.count_subaction_paths > 0 && !info.subaction_paths.is_null() {
        std::slice::from_raw_parts(info.subaction_paths, info.count_subaction_paths as usize)
            .iter()
            .map(|p| p.into_raw())
            .collect()
    } else {
        Vec::new()
    };
    // A path named twice is not a path the action can be asked about.
    for (i, p) in subaction_paths.iter().enumerate() {
        if subaction_paths[..i].contains(p) {
            return R::ERROR_PATH_UNSUPPORTED;
        }
    }
    // The top-level paths a controller has, which are what an action can be asked about by hand.
    if let Some(instance) = INSTANCES.get(owner.instance) {
        for p in &subaction_paths {
            let ok = instance
                .path_text(*p)
                .is_some_and(|t| matches!(t.as_str(), "/user/hand/left" | "/user/hand/right" | "/user/head" | "/user/gamepad"));
            if !ok {
                return R::ERROR_PATH_UNSUPPORTED;
            }
        }
    }
    for other in ACTIONS.all().iter().filter(|o| o.set == set.into_raw()) {
        if other.name == name {
            return R::ERROR_NAME_DUPLICATED;
        }
        if other.localized == localized {
            return R::ERROR_LOCALIZED_NAME_DUPLICATED;
        }
    }
    let handle = new_handle();
    ACTIONS.insert(
        handle,
        Arc::new(Action { set: set.into_raw(), name, localized, kind: info.action_type, subaction_paths }),
    );
    *out = openxr_sys::Action::from_raw(handle);
    R::SUCCESS
}

pub unsafe extern "system" fn destroy_action(action: openxr_sys::Action) -> R {
    let _ = handle!(ACTIONS, action);
    ACTIONS.remove(action.into_raw());
    R::SUCCESS
}

pub unsafe extern "system" fn suggest_interaction_profile_bindings(
    instance: XrInstance,
    info: *const openxr_sys::InteractionProfileSuggestedBinding,
) -> R {
    let instance = handle!(INSTANCES, instance);
    let info = &*info;
    let profile = info.interaction_profile.into_raw();
    let bindings = if info.count_suggested_bindings == 0 {
        return R::ERROR_VALIDATION_FAILURE;
    } else {
        std::slice::from_raw_parts(info.suggested_bindings, info.count_suggested_bindings as usize)
    };
    // Once an application has attached its action sets the bindings are settled.
    if SESSIONS
        .all()
        .iter()
        .any(|s| s.instance.handle == instance.handle && s.input.lock().unwrap().attached)
    {
        return R::ERROR_ACTIONSETS_ALREADY_ATTACHED;
    }
    // Only what the specification has for a profile that is available, whether or not this machine
    // has such a controller: an application suggests for every kind it knows.
    let Some(table) = instance.path_text(profile).and_then(|text| crate::profiles::profile(&text)) else {
        log::warn!("bindings suggested for {:?}, which is not an interaction profile", instance.path_text(profile));
        return R::ERROR_PATH_UNSUPPORTED;
    };
    if !crate::profiles::profile_available(table, instance.api_minor, &instance.extensions) {
        log::warn!("bindings suggested for {}, which needs an extension that is not enabled", table.path);
        return R::ERROR_PATH_UNSUPPORTED;
    }
    for b in bindings {
        let known = instance
            .path_text(b.binding.into_raw())
            .is_some_and(|text| crate::profiles::component_available(table, &text, instance.api_minor, &instance.extensions));
        if !known {
            log::warn!(
                "{} suggested for {}: not a path of that profile; refusing all {} suggestions",
                instance.path_text(b.binding.into_raw()).unwrap_or_default(),
                table.path,
                bindings.len()
            );
            return R::ERROR_PATH_UNSUPPORTED;
        }
    }
    log::debug!(
        "{} bindings suggested for {}",
        bindings.len(),
        instance.path_text(profile).unwrap_or_default()
    );
    if log::log_enabled!(log::Level::Debug) {
        for b in bindings {
            let action = ACTIONS.get(b.action.into_raw()).map(|a| a.name.clone()).unwrap_or_default();
            log::debug!("  {action} <- {}", instance.path_text(b.binding.into_raw()).unwrap_or_default());
        }
    }
    // A later suggestion for the same profile replaces the earlier one, as the specification says.
    instance.bindings.lock().unwrap().insert(
        profile,
        bindings.iter().map(|b| (b.action.into_raw(), b.binding.into_raw())).collect(),
    );
    R::SUCCESS
}

pub unsafe extern "system" fn attach_session_action_sets(
    session: XrSession,
    info: *const openxr_sys::SessionActionSetsAttachInfo,
) -> R {
    let s = handle!(SESSIONS, session);
    if info.is_null() || (*info).ty != StructureType::SESSION_ACTION_SETS_ATTACH_INFO {
        return R::ERROR_VALIDATION_FAILURE;
    }
    let info = &*info;
    if info.count_action_sets == 0 {
        return R::ERROR_VALIDATION_FAILURE;
    }
    let sets = std::slice::from_raw_parts(info.action_sets, info.count_action_sets as usize);
    for set in sets {
        if ACTION_SETS.get(set.into_raw()).is_none() {
            return R::ERROR_HANDLE_INVALID;
        }
    }
    {
        let mut input = s.input.lock().unwrap();
        if input.attached {
            return R::ERROR_ACTIONSETS_ALREADY_ATTACHED;
        }
        input.attached = true;
        input.attached_sets = sets.iter().map(|set| set.into_raw()).collect();
    }
    s.poll_input();
    R::SUCCESS
}

pub unsafe extern "system" fn get_current_interaction_profile(
    session: XrSession,
    user_path: Path,
    profile: *mut openxr_sys::InteractionProfileState,
) -> R {
    let s = handle!(SESSIONS, session);
    let attached = s.input.lock().unwrap().attached;
    if !attached {
        return R::ERROR_ACTIONSET_NOT_ATTACHED;
    }
    let hand = s.instance.path_text(user_path.into_raw()).unwrap_or_default();
    let wanted = hand == "/user/hand/left" || hand == "/user/hand/right";
    (*profile).interaction_profile = match (attached && wanted, s.instance.active_profile()) {
        (true, Some(p)) => Path::from_raw(p),
        _ => Path::NULL,
    };
    R::SUCCESS
}

pub unsafe extern "system" fn sync_actions(session: XrSession, info: *const openxr_sys::ActionsSyncInfo) -> R {
    let s = handle!(SESSIONS, session);
    if info.is_null() || (*info).ty != StructureType::ACTIONS_SYNC_INFO {
        return R::ERROR_VALIDATION_FAILURE;
    }
    // Every action set asked for has to be one the application attached.
    let asked = (*info).count_active_action_sets;
    if asked > 0 && !(*info).active_action_sets.is_null() {
        let active = std::slice::from_raw_parts((*info).active_action_sets, asked as usize);
        let input = s.input.lock().unwrap();
        for a in active {
            if ACTION_SETS.get(a.action_set.into_raw()).is_none() {
                return R::ERROR_HANDLE_INVALID;
            }
            if !input.attached_sets.contains(&a.action_set.into_raw()) {
                return R::ERROR_ACTIONSET_NOT_ATTACHED;
            }
        }
    }
    s.poll_input();
    // Input is only for a session that has the focus.
    if s.run.lock().unwrap().state != SessionState::Focused {
        return R::SESSION_NOT_FOCUSED;
    }
    // The first sync after the action sets are attached is when the application is told which
    // controller it has: events about it are only queued while syncing.
    let announce = {
        let mut input = s.input.lock().unwrap();
        let first = input.attached && !input.profile_announced;
        if first {
            input.profile_announced = true;
        }
        first
    };
    if announce {
        if let Some(profile) = s.instance.active_profile() {
            log::info!("the application will have {}", s.instance.path_text(profile).unwrap_or_default());
            s.instance
                .events
                .lock()
                .unwrap()
                .push_back(Event::InteractionProfileChanged { session: s.handle });
        }
    }
    R::SUCCESS
}

/// What an action reads as, for a hand or (with the null path) for either: the largest of what its
/// bindings give, for the hands asked about.
unsafe fn action_value(s: &Session, info: &openxr_sys::ActionStateGetInfo) -> Option<(crate::input::Value, Option<crate::input::Hand>)> {
    use crate::input::{read, split, Value};
    let action = info.action.into_raw();
    let wanted = s
        .instance
        .path_text(info.subaction_path.into_raw())
        .as_deref()
        .and_then(|t| split(&format!("{t}/input/x")).map(|(h, _)| h));
    // A game that binds a mirrored set of actions for a left-handed player (X8's `inverted_*`:
    // the left stick turns and the right moves) reads both sets at once, since nothing here tells
    // it which hand is the player's -- and then each stick does both jobs. The right-handed set is
    // the one a pad is played with, so the mirrored one is left inactive.
    if let Some(name) = ACTIONS.get(action).map(|a| a.name.clone()) {
        if name.contains("inverted") {
            static SAID: std::sync::Once = std::sync::Once::new();
            SAID.call_once(|| log::info!("actions for the mirrored (left-handed) layout, such as \"{name}\", are left inactive"));
            return None;
        }
    }
    let mut input = s.input.lock().unwrap();
    if input.pad.is_none() && !input.has_pointer {
        return None;
    }
    let mut best: Option<Value> = None;
    for path in s.instance.bindings_of(action) {
        let Some((hand, _)) = split(&path) else { continue };
        if wanted.is_some_and(|w| w != hand) {
            continue;
        }
        let Some(value) = read(&path, &input.state) else { continue };
        best = Some(match best {
            Some(old) if old.as_float() >= value.as_float() => old,
            _ => value,
        });
    }
    let value = best?;
    let now = crate::time::now_ns();
    let key = (action, wanted);
    let changed = input.last.get(&key).map(|(old, _)| *old != value);
    if changed != Some(false) {
        // What the game is told, as it changes, for finding out which control does what: every
        // action, the hand it was asked about, and the value. Only a real step is said, since an
        // analogue stick reads a little differently every time.
        let stepped = match input.last.get(&key) {
            None => true,
            Some((old, _)) => match (old, &value) {
                (crate::input::Value::Vec2(a), crate::input::Value::Vec2(b)) => (a[0] - b[0]).abs().max((a[1] - b[1]).abs()) > 0.1,
                (crate::input::Value::Float(a), crate::input::Value::Float(b)) => (a - b).abs() > 0.1,
                (a, b) => a != b,
            },
        };
        if stepped {
            let name = ACTIONS.get(action).map(|a| a.name.clone()).unwrap_or_default();
            let hand = match wanted {
                Some(crate::input::Hand::Left) => "left",
                Some(crate::input::Hand::Right) => "right",
                None => "either",
            };
            log::debug!("input: {name} ({hand}) = {value:?}");
        }
        input.last.insert(key, (value, now));
    }
    Some((value, wanted))
}

unsafe fn changed_and_time(s: &Session, info: &openxr_sys::ActionStateGetInfo, wanted: Option<crate::input::Hand>) -> (bool, i64) {
    let input = s.input.lock().unwrap();
    input.last.get(&(info.action.into_raw(), wanted)).map_or((false, 0), |(_, t)| {
        // Changed within this sync's frame of reference: the last 50 ms is good enough for "since
        // the last sync" at a frame rate that syncs every frame.
        (crate::time::now_ns() - *t < 50_000_000, *t)
    })
}

/// What every `xrGetActionState*` checks first: the info, that the action is of the type asked for and
/// has been attached, and that the hand asked about is one it was made for.
unsafe fn check_state_get(
    s: &Session,
    info: *const openxr_sys::ActionStateGetInfo,
    expected: openxr_sys::ActionType,
) -> Option<R> {
    if info.is_null() || (*info).ty != StructureType::ACTION_STATE_GET_INFO {
        return Some(R::ERROR_VALIDATION_FAILURE);
    }
    let info = &*info;
    let Some(action) = ACTIONS.get(info.action.into_raw()) else {
        return Some(R::ERROR_HANDLE_INVALID);
    };
    if action.kind != expected {
        return Some(R::ERROR_ACTION_TYPE_MISMATCH);
    }
    let sub = info.subaction_path.into_raw();
    if sub != 0 && !action.subaction_paths.contains(&sub) {
        return Some(R::ERROR_PATH_UNSUPPORTED);
    }
    if !s.input.lock().unwrap().attached_sets.contains(&action.set) {
        return Some(R::ERROR_ACTIONSET_NOT_ATTACHED);
    }
    None
}

pub unsafe extern "system" fn get_action_state_boolean(
    session: XrSession,
    info: *const openxr_sys::ActionStateGetInfo,
    state: *mut openxr_sys::ActionStateBoolean,
) -> R {
    let s = handle!(SESSIONS, session);
    if let Some(e) = check_state_get(&s, info, openxr_sys::ActionType::BOOLEAN_INPUT) {
        return e;
    }
    let out = &mut *state;
    match action_value(&s, &*info) {
        Some((value, hand)) => {
            let (changed, time) = changed_and_time(&s, &*info, hand);
            out.current_state = Bool32::from(value.as_bool());
            out.changed_since_last_sync = Bool32::from(changed);
            out.last_change_time = Time::from_nanos(time);
            out.is_active = Bool32::from(true);
        }
        None => {
            out.current_state = Bool32::from(false);
            out.changed_since_last_sync = Bool32::from(false);
            out.last_change_time = Time::from_nanos(0);
            out.is_active = Bool32::from(false);
        }
    }
    R::SUCCESS
}

pub unsafe extern "system" fn get_action_state_float(
    session: XrSession,
    info: *const openxr_sys::ActionStateGetInfo,
    state: *mut openxr_sys::ActionStateFloat,
) -> R {
    let s = handle!(SESSIONS, session);
    if let Some(e) = check_state_get(&s, info, openxr_sys::ActionType::FLOAT_INPUT) {
        return e;
    }
    let out = &mut *state;
    match action_value(&s, &*info) {
        Some((value, hand)) => {
            let (changed, time) = changed_and_time(&s, &*info, hand);
            out.current_state = value.as_float();
            out.changed_since_last_sync = Bool32::from(changed);
            out.last_change_time = Time::from_nanos(time);
            out.is_active = Bool32::from(true);
        }
        None => {
            out.current_state = 0.0;
            out.changed_since_last_sync = Bool32::from(false);
            out.last_change_time = Time::from_nanos(0);
            out.is_active = Bool32::from(false);
        }
    }
    R::SUCCESS
}

pub unsafe extern "system" fn get_action_state_vector2f(
    session: XrSession,
    info: *const openxr_sys::ActionStateGetInfo,
    state: *mut openxr_sys::ActionStateVector2f,
) -> R {
    let s = handle!(SESSIONS, session);
    if let Some(e) = check_state_get(&s, info, openxr_sys::ActionType::VECTOR2F_INPUT) {
        return e;
    }
    let out = &mut *state;
    match action_value(&s, &*info) {
        Some((value, hand)) => {
            let (changed, time) = changed_and_time(&s, &*info, hand);
            let v = value.as_vec2();
            out.current_state = openxr_sys::Vector2f { x: v[0], y: v[1] };
            out.changed_since_last_sync = Bool32::from(changed);
            out.last_change_time = Time::from_nanos(time);
            out.is_active = Bool32::from(true);
        }
        None => {
            out.current_state = openxr_sys::Vector2f { x: 0.0, y: 0.0 };
            out.changed_since_last_sync = Bool32::from(false);
            out.last_change_time = Time::from_nanos(0);
            out.is_active = Bool32::from(false);
        }
    }
    R::SUCCESS
}

pub unsafe extern "system" fn get_action_state_pose(
    session: XrSession,
    info: *const openxr_sys::ActionStateGetInfo,
    state: *mut openxr_sys::ActionStatePose,
) -> R {
    let s = handle!(SESSIONS, session);
    if let Some(e) = check_state_get(&s, info, openxr_sys::ActionType::POSE_INPUT) {
        return e;
    }
    let info = &*info;
    // A pose is there when the application bound the action to a hand's grip or aim.
    let active = s.input.lock().unwrap().attached
        && s.hand_of(info.action.into_raw(), info.subaction_path.into_raw()).is_some();
    (*state).is_active = Bool32::from(active);
    R::SUCCESS
}

pub unsafe extern "system" fn enumerate_bound_sources_for_action(
    session: XrSession,
    info: *const openxr_sys::BoundSourcesForActionEnumerateInfo,
    _capacity: u32,
    count_out: *mut u32,
    _paths: *mut Path,
) -> R {
    let s = handle!(SESSIONS, session);
    if info.is_null() || (*info).ty != StructureType::BOUND_SOURCES_FOR_ACTION_ENUMERATE_INFO {
        return R::ERROR_VALIDATION_FAILURE;
    }
    let Some(action) = ACTIONS.get((*info).action.into_raw()) else {
        return R::ERROR_HANDLE_INVALID;
    };
    if !s.input.lock().unwrap().attached_sets.contains(&action.set) {
        return R::ERROR_ACTIONSET_NOT_ATTACHED;
    }
    *count_out = 0;
    R::SUCCESS
}

pub unsafe extern "system" fn get_input_source_localized_name(
    session: XrSession,
    _info: *const openxr_sys::InputSourceLocalizedNameGetInfo,
    _capacity: u32,
    count_out: *mut u32,
    _buffer: *mut c_char,
) -> R {
    let _ = handle!(SESSIONS, session);
    // Nothing is bound to a source yet, so there is no name to give.
    *count_out = 1;
    R::ERROR_PATH_UNSUPPORTED
}

/// What both haptic calls check: the info, that the action is an output, and the hand.
unsafe fn check_haptic(s: &Session, info: *const openxr_sys::HapticActionInfo) -> Option<R> {
    if info.is_null() || (*info).ty != StructureType::HAPTIC_ACTION_INFO {
        return Some(R::ERROR_VALIDATION_FAILURE);
    }
    let info = &*info;
    let Some(action) = ACTIONS.get(info.action.into_raw()) else {
        return Some(R::ERROR_HANDLE_INVALID);
    };
    if action.kind != openxr_sys::ActionType::VIBRATION_OUTPUT {
        return Some(R::ERROR_ACTION_TYPE_MISMATCH);
    }
    let sub = info.subaction_path.into_raw();
    if sub != 0 && !action.subaction_paths.contains(&sub) {
        return Some(if s.instance.path_text(sub).is_none() { R::ERROR_PATH_INVALID } else { R::ERROR_PATH_UNSUPPORTED });
    }
    if !s.input.lock().unwrap().attached_sets.contains(&action.set) {
        return Some(R::ERROR_ACTIONSET_NOT_ATTACHED);
    }
    None
}

pub unsafe extern "system" fn apply_haptic_feedback(
    session: XrSession,
    info: *const openxr_sys::HapticActionInfo,
    haptic: *const openxr_sys::HapticBaseHeader,
) -> R {
    let s = handle!(SESSIONS, session);
    if let Some(e) = check_haptic(&s, info) {
        return e;
    }
    if haptic.is_null() || (*haptic).ty != StructureType::HAPTIC_VIBRATION {
        return R::ERROR_VALIDATION_FAILURE;
    }
    if s.run.lock().unwrap().state != SessionState::Focused {
        return R::SESSION_NOT_FOCUSED;
    }
    R::SUCCESS
}

pub unsafe extern "system" fn stop_haptic_feedback(
    session: XrSession,
    info: *const openxr_sys::HapticActionInfo,
) -> R {
    let s = handle!(SESSIONS, session);
    if let Some(e) = check_haptic(&s, info) {
        return e;
    }
    if s.run.lock().unwrap().state != SessionState::Focused {
        return R::SESSION_NOT_FOCUSED;
    }
    R::SUCCESS
}
