//! An OpenXR runtime for Spatiand.
//!
//! The OpenXR loader finds this library through a manifest (`spatiand_openxr.json`, named by
//! `XR_RUNTIME_JSON` or installed as the active runtime), calls
//! [`xrNegotiateLoaderRuntimeInterface`], and from then on reaches everything through the
//! `xrGetInstanceProcAddr` that call hands back.
//!
//! What the runtime *is*: a Wayland client. The application's head pose comes out of the pose
//! channel `spatiand_xr_v1` gives (see [`wl`]), and its finished frames go back as a
//! side-by-side `projection` window (see [`present`]). It runs in the application's process, on
//! the application's GPU, and talks to whichever compositor `WAYLAND_DISPLAY` names -- Spatiand on
//! the Deck, `spatiand-host` on a machine streaming to a headset. The same library serves both,
//! which is the point of putting the protocol in the middle.
//!
//! It is not a conformant runtime and does not claim to be: it implements what applications use.
//! `docs/openxr.md` says what is and is not there.

pub mod api;
pub mod gl;
pub mod input;
pub mod logger;
pub mod object;
pub mod pose;
pub mod present;
pub mod profiles;
#[allow(clippy::all)]
pub mod profiles_table;
pub mod time;
pub mod vk;
pub mod wl;

use std::ffi::{c_char, CStr};

use openxr_sys::{pfn, Instance as XrInstance, Result as R, Handle};

/// Cast an entry point to the loader's untyped function pointer, checking its signature against
/// the specification's on the way: the cast to `$t` fails to compile if they disagree.
macro_rules! entry {
    ($t:ty, $f:expr) => {
        Some(unsafe { std::mem::transmute::<$t, pfn::VoidFunction>($f) })
    };
}

fn lookup(name: &str) -> Option<pfn::VoidFunction> {
    match name {
        "xrEnumerateInstanceExtensionProperties" => {
            entry!(pfn::EnumerateInstanceExtensionProperties, api::enumerate_instance_extension_properties)
        }
        "xrCreateInstance" => entry!(pfn::CreateInstance, api::create_instance),
        "xrDestroyInstance" => entry!(pfn::DestroyInstance, api::destroy_instance),
        "xrGetInstanceProperties" => entry!(pfn::GetInstanceProperties, api::get_instance_properties),
        "xrPollEvent" => entry!(pfn::PollEvent, api::poll_event),
        "xrResultToString" => entry!(pfn::ResultToString, api::result_to_string),
        "xrStructureTypeToString" => entry!(pfn::StructureTypeToString, api::structure_type_to_string),
        "xrStringToPath" => entry!(pfn::StringToPath, api::string_to_path),
        "xrPathToString" => entry!(pfn::PathToString, api::path_to_string),
        "xrGetSystem" => entry!(pfn::GetSystem, api::get_system),
        "xrGetSystemProperties" => entry!(pfn::GetSystemProperties, api::get_system_properties),
        "xrEnumerateEnvironmentBlendModes" => {
            entry!(pfn::EnumerateEnvironmentBlendModes, api::enumerate_environment_blend_modes)
        }
        "xrEnumerateViewConfigurations" => {
            entry!(pfn::EnumerateViewConfigurations, api::enumerate_view_configurations)
        }
        "xrGetViewConfigurationProperties" => {
            entry!(pfn::GetViewConfigurationProperties, api::get_view_configuration_properties)
        }
        "xrEnumerateViewConfigurationViews" => {
            entry!(pfn::EnumerateViewConfigurationViews, api::enumerate_view_configuration_views)
        }
        "xrGetVulkanGraphicsRequirements2KHR" => {
            entry!(pfn::GetVulkanGraphicsRequirements2KHR, api::get_vulkan_graphics_requirements2)
        }
        "xrGetVulkanGraphicsRequirementsKHR" => {
            entry!(pfn::GetVulkanGraphicsRequirementsKHR, api::get_vulkan_graphics_requirements2)
        }
        "xrGetVulkanInstanceExtensionsKHR" => {
            entry!(pfn::GetVulkanInstanceExtensionsKHR, api::get_vulkan_instance_extensions)
        }
        "xrGetVulkanDeviceExtensionsKHR" => {
            entry!(pfn::GetVulkanDeviceExtensionsKHR, api::get_vulkan_device_extensions)
        }
        "xrGetVulkanGraphicsDeviceKHR" => {
            entry!(pfn::GetVulkanGraphicsDeviceKHR, api::get_vulkan_graphics_device)
        }
        "xrGetOpenGLGraphicsRequirementsKHR" => {
            entry!(pfn::GetOpenGLGraphicsRequirementsKHR, api::get_opengl_graphics_requirements)
        }
        "xrCreateVulkanInstanceKHR" => entry!(pfn::CreateVulkanInstanceKHR, api::create_vulkan_instance),
        "xrGetVulkanGraphicsDevice2KHR" => {
            entry!(pfn::GetVulkanGraphicsDevice2KHR, api::get_vulkan_graphics_device2)
        }
        "xrCreateVulkanDeviceKHR" => entry!(pfn::CreateVulkanDeviceKHR, api::create_vulkan_device),
        "xrCreateSession" => entry!(pfn::CreateSession, api::create_session),
        "xrDestroySession" => entry!(pfn::DestroySession, api::destroy_session),
        "xrBeginSession" => entry!(pfn::BeginSession, api::begin_session),
        "xrEndSession" => entry!(pfn::EndSession, api::end_session),
        "xrRequestExitSession" => entry!(pfn::RequestExitSession, api::request_exit_session),
        "xrEnumerateReferenceSpaces" => entry!(pfn::EnumerateReferenceSpaces, api::enumerate_reference_spaces),
        "xrGetReferenceSpaceBoundsRect" => {
            entry!(pfn::GetReferenceSpaceBoundsRect, api::get_reference_space_bounds_rect)
        }
        "xrCreateReferenceSpace" => entry!(pfn::CreateReferenceSpace, api::create_reference_space),
        "xrCreateActionSpace" => entry!(pfn::CreateActionSpace, api::create_action_space),
        "xrDestroySpace" => entry!(pfn::DestroySpace, api::destroy_space),
        "xrLocateSpace" => entry!(pfn::LocateSpace, api::locate_space),
        "xrLocateSpaces" | "xrLocateSpacesKHR" => entry!(pfn::LocateSpaces, api::locate_spaces),
        "xrLocateViews" => entry!(pfn::LocateViews, api::locate_views),
        "xrEnumerateSwapchainFormats" => entry!(pfn::EnumerateSwapchainFormats, api::enumerate_swapchain_formats),
        "xrCreateSwapchain" => entry!(pfn::CreateSwapchain, api::create_swapchain),
        "xrDestroySwapchain" => entry!(pfn::DestroySwapchain, api::destroy_swapchain),
        "xrEnumerateSwapchainImages" => entry!(pfn::EnumerateSwapchainImages, api::enumerate_swapchain_images),
        "xrAcquireSwapchainImage" => entry!(pfn::AcquireSwapchainImage, api::acquire_swapchain_image),
        "xrWaitSwapchainImage" => entry!(pfn::WaitSwapchainImage, api::wait_swapchain_image),
        "xrReleaseSwapchainImage" => entry!(pfn::ReleaseSwapchainImage, api::release_swapchain_image),
        "xrWaitFrame" => entry!(pfn::WaitFrame, api::wait_frame),
        "xrBeginFrame" => entry!(pfn::BeginFrame, api::begin_frame),
        "xrEndFrame" => entry!(pfn::EndFrame, api::end_frame),
        "xrConvertTimeToTimespecTimeKHR" => {
            entry!(pfn::ConvertTimeToTimespecTimeKHR, api::convert_time_to_timespec)
        }
        "xrConvertTimespecTimeToTimeKHR" => {
            entry!(pfn::ConvertTimespecTimeToTimeKHR, api::convert_timespec_to_time)
        }
        "xrCreateActionSet" => entry!(pfn::CreateActionSet, api::create_action_set),
        "xrDestroyActionSet" => entry!(pfn::DestroyActionSet, api::destroy_action_set),
        "xrCreateAction" => entry!(pfn::CreateAction, api::create_action),
        "xrDestroyAction" => entry!(pfn::DestroyAction, api::destroy_action),
        "xrSuggestInteractionProfileBindings" => {
            entry!(pfn::SuggestInteractionProfileBindings, api::suggest_interaction_profile_bindings)
        }
        "xrAttachSessionActionSets" => entry!(pfn::AttachSessionActionSets, api::attach_session_action_sets),
        "xrGetCurrentInteractionProfile" => {
            entry!(pfn::GetCurrentInteractionProfile, api::get_current_interaction_profile)
        }
        "xrSyncActions" => entry!(pfn::SyncActions, api::sync_actions),
        "xrGetActionStateBoolean" => entry!(pfn::GetActionStateBoolean, api::get_action_state_boolean),
        "xrGetActionStateFloat" => entry!(pfn::GetActionStateFloat, api::get_action_state_float),
        "xrGetActionStateVector2f" => entry!(pfn::GetActionStateVector2f, api::get_action_state_vector2f),
        "xrGetActionStatePose" => entry!(pfn::GetActionStatePose, api::get_action_state_pose),
        "xrEnumerateBoundSourcesForAction" => {
            entry!(pfn::EnumerateBoundSourcesForAction, api::enumerate_bound_sources_for_action)
        }
        "xrGetInputSourceLocalizedName" => {
            entry!(pfn::GetInputSourceLocalizedName, api::get_input_source_localized_name)
        }
        "xrApplyHapticFeedback" => entry!(pfn::ApplyHapticFeedback, api::apply_haptic_feedback),
        "xrStopHapticFeedback" => entry!(pfn::StopHapticFeedback, api::stop_haptic_feedback),
        _ => None,
    }
}

/// `xrGetInstanceProcAddr`, as the runtime end of the loader's interface.
///
/// # Safety
/// Called by the loader with valid pointers.
pub unsafe extern "system" fn get_instance_proc_addr(
    instance: XrInstance,
    name: *const c_char,
    function: *mut Option<pfn::VoidFunction>,
) -> R {
    if name.is_null() || function.is_null() {
        return R::ERROR_VALIDATION_FAILURE;
    }
    let name = CStr::from_ptr(name).to_string_lossy();
    // Before an instance exists only the entry points that make one may be asked for.
    if instance.into_raw() == 0
        && !matches!(
            name.as_ref(),
            "xrEnumerateInstanceExtensionProperties" | "xrCreateInstance"
        )
    {
        *function = None;
        return R::ERROR_HANDLE_INVALID;
    }
    // `xrLocateSpaces` is OpenXR 1.1's, or the extension's, and not there for an instance without either.
    let withheld = match (name.as_ref(), crate::object::INSTANCES.get(instance.into_raw())) {
        ("xrLocateSpaces", Some(i)) => i.api_minor < 1 && !i.has_extension("XR_KHR_locate_spaces"),
        (name, Some(i)) => required_extension(name).is_some_and(|ext| !i.has_extension(ext)),
        _ => false,
    };
    *function = if withheld { None } else { lookup(&name) };
    if (*function).is_some() {
        R::SUCCESS
    } else {
        log::debug!("{name} was asked for and is not implemented");
        R::ERROR_FUNCTION_UNSUPPORTED
    }
}

/// The extension an entry point belongs to, for the ones that are an extension's and no core function:
/// an instance made without it is not given them.
fn required_extension(name: &str) -> Option<&'static str> {
    Some(match name {
        "xrGetVulkanInstanceExtensionsKHR" | "xrGetVulkanDeviceExtensionsKHR" | "xrGetVulkanGraphicsDeviceKHR"
        | "xrGetVulkanGraphicsRequirementsKHR" => "XR_KHR_vulkan_enable",
        "xrCreateVulkanInstanceKHR" | "xrCreateVulkanDeviceKHR" | "xrGetVulkanGraphicsDevice2KHR"
        | "xrGetVulkanGraphicsRequirements2KHR" => "XR_KHR_vulkan_enable2",
        "xrGetOpenGLGraphicsRequirementsKHR" => "XR_KHR_opengl_enable",
        "xrConvertTimespecTimeToTimeKHR" | "xrConvertTimeToTimespecTimeKHR" => "XR_KHR_convert_timespec_time",
        "xrLocateSpacesKHR" => "XR_KHR_locate_spaces",
        _ => return None,
    })
}

/// The loader's way in. Exported by name; the loader finds it with `dlsym`.
///
/// # Safety
/// Called by the loader with valid pointers.
#[no_mangle]
pub unsafe extern "system" fn xrNegotiateLoaderRuntimeInterface(
    loader: *const openxr_sys::NegotiateLoaderInfo,
    request: *mut openxr_sys::NegotiateRuntimeRequest,
) -> R {
    logger::init();
    if loader.is_null() || request.is_null() {
        return R::ERROR_INITIALIZATION_FAILED;
    }
    let (loader, request) = (&*loader, &mut *request);
    // Interface version 1 is the only one that has been defined for runtimes.
    if loader.min_interface_version > 1 || loader.max_interface_version < 1 {
        log::error!(
            "the loader wants interface versions {}..{}; this runtime speaks 1",
            loader.min_interface_version,
            loader.max_interface_version
        );
        return R::ERROR_INITIALIZATION_FAILED;
    }
    request.runtime_interface_version = 1;
    request.runtime_api_version = openxr_sys::CURRENT_API_VERSION;
    request.get_instance_proc_addr = Some(get_instance_proc_addr);
    log::info!("negotiated with the loader");
    R::SUCCESS
}
