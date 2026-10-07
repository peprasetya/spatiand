//! Vulkan, in the application's own device.
//!
//! `XR_KHR_vulkan_enable2` makes the runtime the party that creates the application's
//! `VkInstance` and `VkDevice` -- the application hands over its create info and the runtime adds
//! what it needs. What it needs here is the ability to hand a finished picture to another process
//! without copying it through the CPU: external memory, exported as a dmabuf file descriptor.
//!
//! Everything drawn lives in images the runtime allocates in that device. At `xrEndFrame` the two
//! eyes' images are copied side by side into one **linear** image whose memory is exported, and
//! the descriptor goes to the compositor in a `wl_buffer`. Linear is deliberate for now: it needs
//! no modifier negotiation, every importer understands it, and a copy is all the application's
//! GPU does with it. A tiled, modifier-negotiated path is an optimisation for later.

use std::ffi::{c_char, CStr, CString};

use ash::vk::{self, Handle};

const QUAD_VERT: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/quad.vert.spv"));
const QUAD_FRAG: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/quad.frag.spv"));

/// What an application's swapchain may be made in. Colour first: the first is what a runtime
/// recommends, and sRGB is what the picture should be drawn in.
pub const COLOR_FORMATS: [vk::Format; 4] = [
    vk::Format::B8G8R8A8_SRGB,
    vk::Format::R8G8B8A8_SRGB,
    vk::Format::B8G8R8A8_UNORM,
    vk::Format::R8G8B8A8_UNORM,
];
pub const DEPTH_FORMATS: [vk::Format; 4] = [
    vk::Format::D32_SFLOAT,
    vk::Format::D16_UNORM,
    vk::Format::D24_UNORM_S8_UINT,
    vk::Format::D32_SFLOAT_S8_UINT,
];

/// `DRM_FORMAT_XRGB8888` and `DRM_FORMAT_XBGR8888`: the byte orders of the two colour layouts, with
/// the alpha byte ignored -- an application's alpha is its own business, not the room's.
pub const DRM_XRGB8888: u32 = 0x3432_5258;
pub const DRM_XBGR8888: u32 = 0x3432_4258;
pub const DRM_MODIFIER_LINEAR: u64 = 0;

pub fn drm_fourcc(format: vk::Format) -> u32 {
    match format {
        vk::Format::R8G8B8A8_SRGB | vk::Format::R8G8B8A8_UNORM => DRM_XBGR8888,
        _ => DRM_XRGB8888,
    }
}

pub fn is_depth(format: vk::Format) -> bool {
    DEPTH_FORMATS.contains(&format)
}

/// The extensions an application's instance needs for a dmabuf to leave it.
const INSTANCE_EXTENSIONS: [&CStr; 3] = [
    c"VK_KHR_get_physical_device_properties2",
    c"VK_KHR_external_memory_capabilities",
    c"VK_KHR_external_semaphore_capabilities",
];

/// ...and its device.
const DEVICE_EXTENSIONS: [&CStr; 5] = [
    c"VK_KHR_external_memory",
    c"VK_KHR_external_memory_fd",
    c"VK_EXT_external_memory_dma_buf",
    c"VK_KHR_dedicated_allocation",
    c"VK_KHR_get_memory_requirements2",
];

/// The application's `vkCreateInstance`, with the extensions this runtime needs added.
///
/// # Safety
/// `info` must be a valid `XrVulkanInstanceCreateInfoKHR` whose pointers are live.
pub unsafe fn create_instance(
    info: &openxr_sys::VulkanInstanceCreateInfoKHR,
) -> Result<(vk::Result, vk::Instance), String> {
    let get_proc = info.pfn_get_instance_proc_addr.ok_or("no vkGetInstanceProcAddr")?;
    let create: vk::PFN_vkCreateInstance = std::mem::transmute(
        get_proc(std::ptr::null(), c"vkCreateInstance".as_ptr()).ok_or("no vkCreateInstance")?,
    );
    let theirs = &*(info.vulkan_create_info as *const vk::InstanceCreateInfo);
    let mut names = copy_names(theirs.pp_enabled_extension_names, theirs.enabled_extension_count);
    for wanted in INSTANCE_EXTENSIONS {
        add_name(&mut names, wanted);
    }
    let pointers: Vec<*const c_char> = names.iter().map(|n| n.as_ptr()).collect();
    let mut ours = *theirs;
    ours.enabled_extension_count = pointers.len() as u32;
    ours.pp_enabled_extension_names = pointers.as_ptr();
    let mut instance = vk::Instance::null();
    let result = create(&ours, info.vulkan_allocator as *const vk::AllocationCallbacks, &mut instance);
    Ok((result, instance))
}

/// The application's `vkCreateDevice`, with this runtime's extensions added -- those the
/// physical device has, because an extension that is missing makes the whole call fail.
///
/// # Safety
/// `info` must be a valid `XrVulkanDeviceCreateInfoKHR` whose pointers are live.
pub unsafe fn create_device(
    info: &openxr_sys::VulkanDeviceCreateInfoKHR,
) -> Result<(vk::Result, vk::Device), String> {
    let physical = vk::PhysicalDevice::from_raw(info.vulkan_physical_device as u64);
    // Neither command is reachable through vkGetInstanceProcAddr(NULL, ...), and the structure
    // does not name the instance. The loader's own exported trampolines find the right driver
    // from the physical device handle, which is all that is needed.
    let enumerate: vk::PFN_vkEnumerateDeviceExtensionProperties = std::mem::transmute(
        loader_fn(c"vkEnumerateDeviceExtensionProperties").ok_or("no vkEnumerateDeviceExtensionProperties")?,
    );
    let mut count = 0u32;
    let _ = enumerate(physical, std::ptr::null(), &mut count, std::ptr::null_mut());
    let mut available = vec![vk::ExtensionProperties::default(); count as usize];
    let _ = enumerate(physical, std::ptr::null(), &mut count, available.as_mut_ptr());
    available.truncate(count as usize);
    let has = |name: &CStr| {
        available
            .iter()
            .any(|e| CStr::from_ptr(e.extension_name.as_ptr()) == name)
    };

    let create: vk::PFN_vkCreateDevice =
        std::mem::transmute(loader_fn(c"vkCreateDevice").ok_or("no vkCreateDevice")?);
    let theirs = &*(info.vulkan_create_info as *const vk::DeviceCreateInfo);
    let mut names = copy_names(theirs.pp_enabled_extension_names, theirs.enabled_extension_count);
    for wanted in DEVICE_EXTENSIONS {
        if has(wanted) {
            add_name(&mut names, wanted);
        } else {
            log::warn!("the GPU has no {}", wanted.to_string_lossy());
        }
    }
    let pointers: Vec<*const c_char> = names.iter().map(|n| n.as_ptr()).collect();
    let mut ours = *theirs;
    ours.enabled_extension_count = pointers.len() as u32;
    ours.pp_enabled_extension_names = pointers.as_ptr();
    let mut device = vk::Device::null();
    let result = create(physical, &ours, info.vulkan_allocator as *const vk::AllocationCallbacks, &mut device);
    Ok((result, device))
}

/// A function the Vulkan loader exports by name.
unsafe fn loader_fn(name: &CStr) -> Option<unsafe extern "system" fn()> {
    let lib = libc::dlopen(c"libvulkan.so.1".as_ptr(), libc::RTLD_NOW | libc::RTLD_GLOBAL);
    if lib.is_null() {
        return None;
    }
    let symbol = libc::dlsym(lib, name.as_ptr());
    if symbol.is_null() {
        None
    } else {
        Some(std::mem::transmute(symbol))
    }
}

unsafe fn copy_names(names: *const *const c_char, count: u32) -> Vec<CString> {
    (0..count as usize)
        .map(|i| CStr::from_ptr(*names.add(i)).to_owned())
        .collect()
}

fn add_name(names: &mut Vec<CString>, name: &CStr) {
    if !names.iter().any(|n| n.as_c_str() == name) {
        names.push(name.to_owned());
    }
}

/// The application's Vulkan, as `xrCreateSession` was handed it.
pub struct Vk {
    _entry: ash::Entry,
    pub instance: ash::Instance,
    pub physical: vk::PhysicalDevice,
    pub device: ash::Device,
    pub queue: vk::Queue,
    pub queue_family: u32,
    memory: vk::PhysicalDeviceMemoryProperties,
    external_fd: ash::khr::external_memory_fd::Device,
    pool: vk::CommandPool,
    fence: vk::Fence,
    /// What quad layers are drawn with, made when the first one is seen.
    quads: std::sync::Mutex<Option<QuadState>>,
    /// This runtime made the instance and device (an OpenGL session has none of the application's
    /// to use), and so destroys them.
    owned: bool,
}

// Handles and function tables; the application is responsible for the external synchronisation
// of the queue, as the extension requires of it.
unsafe impl Send for Vk {}
unsafe impl Sync for Vk {}

impl Vk {
    /// # Safety
    /// The handles must be those of a live instance, physical device and device.
    pub unsafe fn from_binding(binding: &openxr_sys::GraphicsBindingVulkanKHR) -> Result<Vk, String> {
        let entry = ash::Entry::load().map_err(|e| format!("no Vulkan loader: {e}"))?;
        let instance = ash::Instance::load(
            entry.static_fn(),
            vk::Instance::from_raw(binding.instance as u64),
        );
        let physical = vk::PhysicalDevice::from_raw(binding.physical_device as u64);
        let device = ash::Device::load(instance.fp_v1_0(), vk::Device::from_raw(binding.device as u64));
        Self::assemble(entry, instance, physical, device, binding.queue_family_index, binding.queue_index, false)
    }

    unsafe fn assemble(
        entry: ash::Entry,
        instance: ash::Instance,
        physical: vk::PhysicalDevice,
        device: ash::Device,
        queue_family: u32,
        queue_index: u32,
        owned: bool,
    ) -> Result<Vk, String> {
        let queue = device.get_device_queue(queue_family, queue_index);
        let memory = instance.get_physical_device_memory_properties(physical);
        let external_fd = ash::khr::external_memory_fd::Device::new(&instance, &device);
        let pool = device
            .create_command_pool(
                &vk::CommandPoolCreateInfo::default()
                    .queue_family_index(queue_family)
                    .flags(vk::CommandPoolCreateFlags::RESET_COMMAND_BUFFER),
                None,
            )
            .map_err(|e| format!("vkCreateCommandPool: {e}"))?;
        let fence = device
            .create_fence(&vk::FenceCreateInfo::default(), None)
            .map_err(|e| format!("vkCreateFence: {e}"))?;
        Ok(Vk {
            _entry: entry,
            instance,
            physical,
            device,
            queue,
            queue_family,
            memory,
            external_fd,
            pool,
            fence,
            quads: std::sync::Mutex::new(None),
            owned,
        })
    }

    /// A Vulkan of this runtime's own, for a session whose application draws with OpenGL.
    ///
    /// On the GPU OpenGL is using -- picked by the device UUID the GL driver reports, since a
    /// machine with two GPUs would otherwise be shown a picture drawn on one and composed on the
    /// other -- or the first real one if the driver will not say.
    pub unsafe fn standalone(uuid: Option<[u8; 16]>) -> Result<Vk, String> {
        let entry = ash::Entry::load().map_err(|e| format!("no Vulkan loader: {e}"))?;
        let app = vk::ApplicationInfo::default()
            .application_name(c"spatiand-openxr")
            .api_version(vk::make_api_version(0, 1, 1, 0));
        let instance = entry
            .create_instance(&vk::InstanceCreateInfo::default().application_info(&app), None)
            .map_err(|e| format!("vkCreateInstance: {e}"))?;
        let devices = instance
            .enumerate_physical_devices()
            .map_err(|e| format!("vkEnumeratePhysicalDevices: {e}"))?;
        let id_of = |d: vk::PhysicalDevice| {
            let mut id = vk::PhysicalDeviceIDProperties::default();
            let mut props = vk::PhysicalDeviceProperties2::default().push_next(&mut id);
            instance.get_physical_device_properties2(d, &mut props);
            id.device_uuid
        };
        let by_uuid = uuid.and_then(|u| devices.iter().copied().find(|d| id_of(*d) == u));
        let physical = by_uuid
            .or_else(|| {
                devices.iter().copied().find(|d| {
                    let t = instance.get_physical_device_properties(*d).device_type;
                    t == vk::PhysicalDeviceType::DISCRETE_GPU || t == vk::PhysicalDeviceType::INTEGRATED_GPU
                })
            })
            .or_else(|| devices.first().copied())
            .ok_or("no Vulkan device")?;
        if by_uuid.is_none() && uuid.is_some() {
            log::warn!("OpenGL's GPU is not one Vulkan can see; using another, and sharing will probably fail");
        }
        let family = instance
            .get_physical_device_queue_family_properties(physical)
            .iter()
            .position(|f| f.queue_flags.contains(vk::QueueFlags::GRAPHICS | vk::QueueFlags::TRANSFER))
            .ok_or("no graphics queue")? as u32;
        let available = instance
            .enumerate_device_extension_properties(physical)
            .map_err(|e| format!("vkEnumerateDeviceExtensionProperties: {e}"))?;
        let has = |name: &CStr| available.iter().any(|e| CStr::from_ptr(e.extension_name.as_ptr()) == name);
        let names: Vec<*const c_char> = DEVICE_EXTENSIONS.iter().filter(|n| has(n)).map(|n| n.as_ptr()).collect();
        let priorities = [1.0f32];
        let queues = [vk::DeviceQueueCreateInfo::default().queue_family_index(family).queue_priorities(&priorities)];
        let device = instance
            .create_device(
                physical,
                &vk::DeviceCreateInfo::default().queue_create_infos(&queues).enabled_extension_names(&names),
                None,
            )
            .map_err(|e| format!("vkCreateDevice: {e}"))?;
        Self::assemble(entry, instance, physical, device, family, 0, true)
    }

    /// An image whose memory is also an OpenGL texture's: made here, exported as a file descriptor,
    /// and imported on the GL side. The application draws into the texture; this reads the image.
    /// Left in `GENERAL`, which is what both sides can agree on.
    pub unsafe fn create_shared_image(
        &self,
        format: vk::Format,
        width: u32,
        height: u32,
        layers: u32,
        linear: bool,
    ) -> Result<Shared, String> {
        use std::os::fd::FromRawFd;
        let mut external = vk::ExternalMemoryImageCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D { width, height, depth: 1 })
            .mip_levels(1)
            .array_layers(layers.max(1))
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(if linear { vk::ImageTiling::LINEAR } else { vk::ImageTiling::OPTIMAL })
            .usage(
                vk::ImageUsageFlags::COLOR_ATTACHMENT
                    | vk::ImageUsageFlags::SAMPLED
                    | vk::ImageUsageFlags::TRANSFER_SRC
                    | vk::ImageUsageFlags::TRANSFER_DST,
            )
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .push_next(&mut external);
        let image = self.device.create_image(&info, None).map_err(|e| format!("vkCreateImage (shared): {e}"))?;
        let need = self.device.get_image_memory_requirements(image);
        let kind = self
            .memory_type(need.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL)
            .or_else(|| self.memory_type(need.memory_type_bits, vk::MemoryPropertyFlags::empty()))
            .ok_or("no memory type for a shared image")?;
        let mut export = vk::ExportMemoryAllocateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD);
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
        let memory = self
            .device
            .allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(need.size)
                    .memory_type_index(kind)
                    .push_next(&mut export)
                    .push_next(&mut dedicated),
                None,
            )
            .map_err(|e| format!("vkAllocateMemory (shared): {e}"))?;
        self.device
            .bind_image_memory(image, memory, 0)
            .map_err(|e| format!("vkBindImageMemory (shared): {e}"))?;
        let fd = self
            .external_fd
            .get_memory_fd(
                &vk::MemoryGetFdInfoKHR::default()
                    .memory(memory)
                    .handle_type(vk::ExternalMemoryHandleTypeFlags::OPAQUE_FD),
            )
            .map_err(|e| format!("vkGetMemoryFdKHR (shared): {e}"))?;
        self.to_general(image, layers.max(1))?;
        Ok(Shared {
            image: Image { image, memory },
            fd: std::os::fd::OwnedFd::from_raw_fd(fd),
            size: need.size,
        })
    }

    /// Put an image in `GENERAL` once, with nothing else going on.
    unsafe fn to_general(&self, image: vk::Image, layers: u32) -> Result<(), String> {
        let cb = self
            .device
            .allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(self.pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )
            .map_err(|e| format!("vkAllocateCommandBuffers: {e}"))?[0];
        self.device
            .begin_command_buffer(
                cb,
                &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
            .map_err(|e| format!("vkBeginCommandBuffer: {e}"))?;
        let barrier = vk::ImageMemoryBarrier::default()
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::GENERAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(vk::ImageSubresourceRange {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                base_mip_level: 0,
                level_count: 1,
                base_array_layer: 0,
                layer_count: layers,
            });
        self.device.cmd_pipeline_barrier(
            cb,
            vk::PipelineStageFlags::TOP_OF_PIPE,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::DependencyFlags::empty(),
            &[],
            &[],
            &[barrier],
        );
        self.device.end_command_buffer(cb).map_err(|e| format!("vkEndCommandBuffer: {e}"))?;
        let buffers = [cb];
        self.device
            .queue_submit(self.queue, &[vk::SubmitInfo::default().command_buffers(&buffers)], self.fence)
            .map_err(|e| format!("vkQueueSubmit: {e}"))?;
        let waited = self.device.wait_for_fences(&[self.fence], true, 1_000_000_000);
        let _ = self.device.reset_fences(&[self.fence]);
        self.device.free_command_buffers(self.pool, &buffers);
        waited.map_err(|e| format!("putting an image in GENERAL: {e}"))
    }

    fn memory_type(&self, bits: u32, wanted: vk::MemoryPropertyFlags) -> Option<u32> {
        (0..self.memory.memory_type_count).find(|&i| {
            bits & (1 << i) != 0
                && self.memory.memory_types[i as usize]
                    .property_flags
                    .contains(wanted)
        })
    }

    /// An image the application will draw into and the runtime will read from.
    pub unsafe fn create_image(
        &self,
        format: vk::Format,
        width: u32,
        height: u32,
        array_layers: u32,
        mip_levels: u32,
        samples: vk::SampleCountFlags,
        usage: vk::ImageUsageFlags,
    ) -> Result<Image, String> {
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D { width, height, depth: 1 })
            .mip_levels(mip_levels.max(1))
            .array_layers(array_layers.max(1))
            .samples(samples)
            .tiling(vk::ImageTiling::OPTIMAL)
            // The runtime copies out of it, so it must be readable as a transfer source whatever
            // the application said it would use it for.
            .usage(usage | vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::SAMPLED)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED);
        let image = self
            .device
            .create_image(&info, None)
            .map_err(|e| format!("vkCreateImage: {e}"))?;
        let need = self.device.get_image_memory_requirements(image);
        let kind = self
            .memory_type(need.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL)
            .or_else(|| self.memory_type(need.memory_type_bits, vk::MemoryPropertyFlags::empty()))
            .ok_or("no memory type for a swapchain image")?;
        let memory = self
            .device
            .allocate_memory(
                &vk::MemoryAllocateInfo::default().allocation_size(need.size).memory_type_index(kind),
                None,
            )
            .map_err(|e| format!("vkAllocateMemory: {e}"))?;
        self.device
            .bind_image_memory(image, memory, 0)
            .map_err(|e| format!("vkBindImageMemory: {e}"))?;
        Ok(Image { image, memory })
    }

    pub unsafe fn destroy_image(&self, image: &Image) {
        self.device.destroy_image(image.image, None);
        self.device.free_memory(image.memory, None);
    }

    /// An image whose memory can be handed to another process as a dmabuf.
    pub unsafe fn create_exportable(
        &self,
        format: vk::Format,
        width: u32,
        height: u32,
    ) -> Result<Exportable, String> {
        let mut external = vk::ExternalMemoryImageCreateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        let info = vk::ImageCreateInfo::default()
            .image_type(vk::ImageType::TYPE_2D)
            .format(format)
            .extent(vk::Extent3D { width, height, depth: 1 })
            .mip_levels(1)
            .array_layers(1)
            .samples(vk::SampleCountFlags::TYPE_1)
            .tiling(vk::ImageTiling::LINEAR)
            .usage(vk::ImageUsageFlags::TRANSFER_DST | vk::ImageUsageFlags::TRANSFER_SRC)
            .sharing_mode(vk::SharingMode::EXCLUSIVE)
            .initial_layout(vk::ImageLayout::UNDEFINED)
            .push_next(&mut external);
        let image = self
            .device
            .create_image(&info, None)
            .map_err(|e| format!("vkCreateImage (exportable): {e}"))?;
        let need = self.device.get_image_memory_requirements(image);
        let kind = self
            .memory_type(need.memory_type_bits, vk::MemoryPropertyFlags::DEVICE_LOCAL)
            .or_else(|| self.memory_type(need.memory_type_bits, vk::MemoryPropertyFlags::empty()))
            .ok_or("no memory type for an exportable image")?;
        let mut export = vk::ExportMemoryAllocateInfo::default()
            .handle_types(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT);
        // Mesa wants an exported image's memory dedicated to it.
        let mut dedicated = vk::MemoryDedicatedAllocateInfo::default().image(image);
        let memory = self
            .device
            .allocate_memory(
                &vk::MemoryAllocateInfo::default()
                    .allocation_size(need.size)
                    .memory_type_index(kind)
                    .push_next(&mut export)
                    .push_next(&mut dedicated),
                None,
            )
            .map_err(|e| format!("vkAllocateMemory (exportable): {e}"))?;
        self.device
            .bind_image_memory(image, memory, 0)
            .map_err(|e| format!("vkBindImageMemory (exportable): {e}"))?;
        let layout = self.device.get_image_subresource_layout(
            image,
            vk::ImageSubresource {
                aspect_mask: vk::ImageAspectFlags::COLOR,
                mip_level: 0,
                array_layer: 0,
            },
        );
        Ok(Exportable {
            image: Image { image, memory },
            stride: layout.row_pitch as u32,
            offset: layout.offset as u32,
            width,
            height,
            format,
            first_use: true,
        })
    }

    /// A descriptor onto the exportable image's memory; each call makes a new one, which the
    /// receiver closes.
    pub unsafe fn export_fd(&self, target: &Exportable) -> Result<std::os::fd::OwnedFd, String> {
        use std::os::fd::FromRawFd;
        let fd = self
            .external_fd
            .get_memory_fd(
                &vk::MemoryGetFdInfoKHR::default()
                    .memory(target.image.memory)
                    .handle_type(vk::ExternalMemoryHandleTypeFlags::DMA_BUF_EXT),
            )
            .map_err(|e| format!("vkGetMemoryFdKHR: {e}"))?;
        Ok(std::os::fd::OwnedFd::from_raw_fd(fd))
    }

    /// Copy the eyes of a frame side by side into `target` and wait until it is done.
    ///
    /// Each copy is `(source image, source layer, source rectangle, eye)`: `eye` 0 goes to the
    /// left half, 1 to the right. The sources are in `COLOR_ATTACHMENT_OPTIMAL`, which is where
    /// `xrReleaseSwapchainImage` leaves an image, and are put back there.
    pub unsafe fn compose(
        &self,
        target: &mut Exportable,
        copies: &[EyeCopy],
        quads: &[QuadDraw],
        eye_size: (u32, u32),
    ) -> Result<(), String> {
        if quads.is_empty() {
            return self.compose_direct(target, copies, eye_size);
        }
        self.compose_with_quads(target, copies, quads, eye_size)
    }

    /// The eyes' pictures copied straight into the frame, which is all most frames are.
    unsafe fn compose_direct(
        &self,
        target: &mut Exportable,
        copies: &[EyeCopy],
        eye_size: (u32, u32),
    ) -> Result<(), String> {
        let cb = self
            .device
            .allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(self.pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )
            .map_err(|e| format!("vkAllocateCommandBuffers: {e}"))?[0];
        self.device
            .begin_command_buffer(
                cb,
                &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
            )
            .map_err(|e| format!("vkBeginCommandBuffer: {e}"))?;

        let color = |layer: u32| vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: layer,
            layer_count: 1,
        };
        let barrier = |image: vk::Image, from: vk::ImageLayout, to: vk::ImageLayout, layer: u32,
                       src: vk::AccessFlags, dst: vk::AccessFlags| {
            vk::ImageMemoryBarrier::default()
                .src_access_mask(src)
                .dst_access_mask(dst)
                .old_layout(from)
                .new_layout(to)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(image)
                .subresource_range(color(layer))
        };
        let stages = |cb, src, dst, barriers: &[vk::ImageMemoryBarrier]| {
            self.device
                .cmd_pipeline_barrier(cb, src, dst, vk::DependencyFlags::empty(), &[], &[], barriers)
        };

        let was = if target.first_use {
            vk::ImageLayout::UNDEFINED
        } else {
            vk::ImageLayout::GENERAL
        };
        stages(
            cb,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::PipelineStageFlags::TRANSFER,
            &[barrier(
                target.image.image,
                was,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                0,
                vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE,
                vk::AccessFlags::TRANSFER_WRITE,
            )],
        );
        target.first_use = false;

        for copy in copies {
            stages(
                cb,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT | vk::PipelineStageFlags::ALL_COMMANDS,
                vk::PipelineStageFlags::TRANSFER,
                &[barrier(
                    copy.image,
                    copy.layout,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    copy.layer,
                    vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                    vk::AccessFlags::TRANSFER_READ,
                )],
            );
            let x = copy.eye as i32 * eye_size.0 as i32;
            let region = vk::ImageBlit::default()
                .src_subresource(vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: 0,
                    base_array_layer: copy.layer,
                    layer_count: 1,
                })
                .src_offsets(src_offsets(copy))
                .dst_subresource(vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .dst_offsets([
                    vk::Offset3D { x, y: 0, z: 0 },
                    vk::Offset3D { x: x + eye_size.0 as i32, y: eye_size.1 as i32, z: 1 },
                ]);
            self.device.cmd_blit_image(
                cb,
                copy.image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                target.image.image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[region],
                vk::Filter::LINEAR,
            );
            stages(
                cb,
                vk::PipelineStageFlags::TRANSFER,
                vk::PipelineStageFlags::ALL_COMMANDS,
                &[barrier(
                    copy.image,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    copy.layout,
                    copy.layer,
                    vk::AccessFlags::TRANSFER_READ,
                    vk::AccessFlags::COLOR_ATTACHMENT_WRITE | vk::AccessFlags::COLOR_ATTACHMENT_READ,
                )],
            );
        }

        // GENERAL is where an external reader finds it; the compositor is not a Vulkan queue
        // that could be told about a layout, and expects the memory to be what was written.
        stages(
            cb,
            vk::PipelineStageFlags::TRANSFER,
            vk::PipelineStageFlags::ALL_COMMANDS,
            &[barrier(
                target.image.image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::ImageLayout::GENERAL,
                0,
                vk::AccessFlags::TRANSFER_WRITE,
                vk::AccessFlags::MEMORY_READ,
            )],
        );
        self.device
            .end_command_buffer(cb)
            .map_err(|e| format!("vkEndCommandBuffer: {e}"))?;
        let buffers = [cb];
        self.device
            .queue_submit(
                self.queue,
                &[vk::SubmitInfo::default().command_buffers(&buffers)],
                self.fence,
            )
            .map_err(|e| format!("vkQueueSubmit: {e}"))?;
        let waited = self.device.wait_for_fences(&[self.fence], true, 1_000_000_000);
        let _ = self.device.reset_fences(&[self.fence]);
        self.device.free_command_buffers(self.pool, &buffers);
        waited.map_err(|e| format!("waiting for the composed frame: {e}"))
    }
}

impl Vk {
    /// The eyes' pictures, then quad layers drawn over them, then all of it into the frame.
    ///
    /// Drawn into a canvas of the frame's size first: the frame itself is linear, exportable memory,
    /// and neither a blit target nor a render target is something every driver will make of one.
    unsafe fn compose_with_quads(
        &self,
        target: &mut Exportable,
        copies: &[EyeCopy],
        quads: &[QuadDraw],
        eye_size: (u32, u32),
    ) -> Result<(), String> {
        let (width, height) = (eye_size.0 * 2, eye_size.1);
        let mut guard = self.quads.lock().unwrap();
        let format = target.format;
        self.prepare_quads(&mut guard, format, width, height)?;
        let state = guard.as_ref().unwrap();
        let canvas = state.canvas.as_ref().unwrap();
        let d = &self.device;

        let cb = d
            .allocate_command_buffers(
                &vk::CommandBufferAllocateInfo::default()
                    .command_pool(self.pool)
                    .level(vk::CommandBufferLevel::PRIMARY)
                    .command_buffer_count(1),
            )
            .map_err(|e| format!("vkAllocateCommandBuffers: {e}"))?[0];
        d.begin_command_buffer(
            cb,
            &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT),
        )
        .map_err(|e| format!("vkBeginCommandBuffer: {e}"))?;

        let range = |layer: u32| vk::ImageSubresourceRange {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            base_mip_level: 0,
            level_count: 1,
            base_array_layer: layer,
            layer_count: 1,
        };
        let barrier = |image: vk::Image, from: vk::ImageLayout, to: vk::ImageLayout, layer: u32,
                       src: vk::AccessFlags, dst: vk::AccessFlags| {
            vk::ImageMemoryBarrier::default()
                .src_access_mask(src)
                .dst_access_mask(dst)
                .old_layout(from)
                .new_layout(to)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(image)
                .subresource_range(range(layer))
        };
        let wait = |src: vk::PipelineStageFlags, dst: vk::PipelineStageFlags, b: &[vk::ImageMemoryBarrier]| {
            d.cmd_pipeline_barrier(cb, src, dst, vk::DependencyFlags::empty(), &[], &[], b)
        };
        let all = vk::PipelineStageFlags::ALL_COMMANDS;
        let transfer = vk::PipelineStageFlags::TRANSFER;

        // The canvas starts empty, every frame: black, then the eyes' pictures over it.
        wait(
            all,
            transfer,
            &[barrier(
                canvas.image.image,
                vk::ImageLayout::UNDEFINED,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                0,
                vk::AccessFlags::empty(),
                vk::AccessFlags::TRANSFER_WRITE,
            )],
        );
        d.cmd_clear_color_image(
            cb,
            canvas.image.image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &vk::ClearColorValue { float32: [0.0, 0.0, 0.0, 1.0] },
            &[range(0)],
        );
        for copy in copies {
            wait(
                all,
                transfer,
                &[barrier(
                    copy.image,
                    copy.layout,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    copy.layer,
                    vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                    vk::AccessFlags::TRANSFER_READ,
                )],
            );
            let x = copy.eye as i32 * eye_size.0 as i32;
            let region = vk::ImageBlit::default()
                .src_subresource(vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: 0,
                    base_array_layer: copy.layer,
                    layer_count: 1,
                })
                .src_offsets(src_offsets(copy))
                .dst_subresource(vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: 0,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .dst_offsets([
                    vk::Offset3D { x, y: 0, z: 0 },
                    vk::Offset3D { x: x + eye_size.0 as i32, y: eye_size.1 as i32, z: 1 },
                ]);
            d.cmd_blit_image(
                cb,
                copy.image,
                vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                canvas.image.image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                &[region],
                vk::Filter::LINEAR,
            );
            wait(
                transfer,
                all,
                &[barrier(
                    copy.image,
                    vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
                    copy.layout,
                    copy.layer,
                    vk::AccessFlags::TRANSFER_READ,
                    vk::AccessFlags::COLOR_ATTACHMENT_WRITE | vk::AccessFlags::COLOR_ATTACHMENT_READ,
                )],
            );
        }
        wait(
            transfer,
            vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
            &[barrier(
                canvas.image.image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL,
                0,
                vk::AccessFlags::TRANSFER_WRITE,
                vk::AccessFlags::COLOR_ATTACHMENT_READ | vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
            )],
        );

        // The quads' own images are read as textures for a while.
        let drawn = &quads[..quads.len().min(MAX_QUADS as usize)];
        for quad in drawn {
            wait(
                all,
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                &[barrier(
                    quad.image,
                    quad.layout,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    quad.layer,
                    vk::AccessFlags::COLOR_ATTACHMENT_WRITE,
                    vk::AccessFlags::SHADER_READ,
                )],
            );
        }

        d.reset_descriptor_pool(state.pool, vk::DescriptorPoolResetFlags::empty())
            .map_err(|e| format!("vkResetDescriptorPool: {e}"))?;
        let mut views = Vec::new();
        let pass_info = vk::RenderPassBeginInfo::default()
            .render_pass(state.render_pass)
            .framebuffer(canvas.framebuffer)
            .render_area(vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent: vk::Extent2D { width, height } });
        d.cmd_begin_render_pass(cb, &pass_info, vk::SubpassContents::INLINE);
        d.cmd_bind_pipeline(cb, vk::PipelineBindPoint::GRAPHICS, state.pipeline);
        for quad in drawn {
            let view = d
                .create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(quad.image)
                        .view_type(vk::ImageViewType::TYPE_2D)
                        .format(quad.format)
                        .subresource_range(range(quad.layer)),
                    None,
                )
                .map_err(|e| format!("vkCreateImageView (quad): {e}"))?;
            views.push(view);
            let layouts = [state.set_layout];
            let set = d
                .allocate_descriptor_sets(
                    &vk::DescriptorSetAllocateInfo::default().descriptor_pool(state.pool).set_layouts(&layouts),
                )
                .map_err(|e| format!("vkAllocateDescriptorSets: {e}"))?[0];
            let image_info = [vk::DescriptorImageInfo::default()
                .image_view(view)
                .image_layout(vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL)];
            let sampler_info = [vk::DescriptorImageInfo::default().sampler(state.sampler)];
            d.update_descriptor_sets(
                &[
                    vk::WriteDescriptorSet::default()
                        .dst_set(set)
                        .dst_binding(0)
                        .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                        .image_info(&image_info),
                    vk::WriteDescriptorSet::default()
                        .dst_set(set)
                        .dst_binding(1)
                        .descriptor_type(vk::DescriptorType::SAMPLER)
                        .image_info(&sampler_info),
                ],
                &[],
            );
            d.cmd_bind_descriptor_sets(cb, vk::PipelineBindPoint::GRAPHICS, state.layout, 0, &[set], &[]);
            for eye in 0..2usize {
                if !quad.visible[eye] {
                    continue;
                }
                let x = eye as f32 * eye_size.0 as f32;
                d.cmd_set_viewport(
                    cb,
                    0,
                    &[vk::Viewport {
                        x,
                        y: 0.0,
                        width: eye_size.0 as f32,
                        height: eye_size.1 as f32,
                        min_depth: 0.0,
                        max_depth: 1.0,
                    }],
                );
                d.cmd_set_scissor(
                    cb,
                    0,
                    &[vk::Rect2D {
                        offset: vk::Offset2D { x: x as i32, y: 0 },
                        extent: vk::Extent2D { width: eye_size.0, height: eye_size.1 },
                    }],
                );
                let mut constants = [0f32; 24];
                constants[..16].copy_from_slice(&quad.mvp[eye]);
                constants[16..20].copy_from_slice(&quad.uv_rect);
                constants[20] = quad.use_alpha as u32 as f32;
                constants[21] = quad.unpremultiplied as u32 as f32;
                let bytes = std::slice::from_raw_parts(constants.as_ptr() as *const u8, 96);
                d.cmd_push_constants(
                    cb,
                    state.layout,
                    vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT,
                    0,
                    bytes,
                );
                d.cmd_draw(cb, 6, 1, 0, 0);
            }
        }
        d.cmd_end_render_pass(cb);

        for quad in drawn {
            wait(
                vk::PipelineStageFlags::FRAGMENT_SHADER,
                all,
                &[barrier(
                    quad.image,
                    vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL,
                    quad.layout,
                    quad.layer,
                    vk::AccessFlags::SHADER_READ,
                    vk::AccessFlags::COLOR_ATTACHMENT_WRITE | vk::AccessFlags::COLOR_ATTACHMENT_READ,
                )],
            );
        }

        // Out to the frame the compositor reads.
        let was = if target.first_use { vk::ImageLayout::UNDEFINED } else { vk::ImageLayout::GENERAL };
        wait(
            all,
            transfer,
            &[barrier(
                target.image.image,
                was,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                0,
                vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE,
                vk::AccessFlags::TRANSFER_WRITE,
            )],
        );
        target.first_use = false;
        let whole = vk::ImageSubresourceLayers {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            mip_level: 0,
            base_array_layer: 0,
            layer_count: 1,
        };
        let extent = [
            vk::Offset3D { x: 0, y: 0, z: 0 },
            vk::Offset3D { x: width as i32, y: height as i32, z: 1 },
        ];
        d.cmd_blit_image(
            cb,
            canvas.image.image,
            vk::ImageLayout::TRANSFER_SRC_OPTIMAL,
            target.image.image,
            vk::ImageLayout::TRANSFER_DST_OPTIMAL,
            &[vk::ImageBlit::default()
                .src_subresource(whole)
                .src_offsets(extent)
                .dst_subresource(whole)
                .dst_offsets(extent)],
            vk::Filter::NEAREST,
        );
        wait(
            transfer,
            all,
            &[barrier(
                target.image.image,
                vk::ImageLayout::TRANSFER_DST_OPTIMAL,
                vk::ImageLayout::GENERAL,
                0,
                vk::AccessFlags::TRANSFER_WRITE,
                vk::AccessFlags::MEMORY_READ,
            )],
        );
        d.end_command_buffer(cb).map_err(|e| format!("vkEndCommandBuffer: {e}"))?;
        let buffers = [cb];
        d.queue_submit(self.queue, &[vk::SubmitInfo::default().command_buffers(&buffers)], self.fence)
            .map_err(|e| format!("vkQueueSubmit: {e}"))?;
        let waited = d.wait_for_fences(&[self.fence], true, 1_000_000_000);
        let _ = d.reset_fences(&[self.fence]);
        d.free_command_buffers(self.pool, &buffers);
        for view in views {
            d.destroy_image_view(view, None);
        }
        waited.map_err(|e| format!("waiting for the composed frame: {e}"))
    }
}

impl Drop for Vk {
    fn drop(&mut self) {
        // SAFETY: the pool and fence were made by this device and nothing else uses them.
        unsafe {
            let _ = self.device.device_wait_idle();
            if let Some(state) = self.quads.lock().unwrap().take() {
                self.destroy_quad_state(state);
            }
            self.device.destroy_fence(self.fence, None);
            self.device.destroy_command_pool(self.pool, None);
            if self.owned {
                self.device.destroy_device(None);
                self.instance.destroy_instance(None);
            }
        }
    }
}

/// An image and the memory behind it.
pub struct Image {
    pub image: vk::Image,
    pub memory: vk::DeviceMemory,
}

/// A frame buffer the compositor can read.
pub struct Exportable {
    pub image: Image,
    pub stride: u32,
    pub offset: u32,
    pub width: u32,
    pub height: u32,
    pub format: vk::Format,
    first_use: bool,
}

/// One eye's picture to place in the frame.
pub struct EyeCopy {
    pub image: vk::Image,
    pub layer: u32,
    /// `(x, y, width, height)` within the source.
    pub rect: (i32, i32, u32, u32),
    pub eye: u32,
    /// The layout the application leaves the image in: `COLOR_ATTACHMENT_OPTIMAL` from Vulkan,
    /// `GENERAL` from an OpenGL texture that Vulkan only shares.
    pub layout: vk::ImageLayout,
    /// The image's rows run bottom to top, as OpenGL's do.
    pub flip_y: bool,
}

/// An OpenGL texture's memory, made here.
pub struct Shared {
    pub image: Image,
    /// For the GL side to import; it takes ownership.
    pub fd: std::os::fd::OwnedFd,
    pub size: u64,
}

/// A quad layer, ready to draw: where it lands in each eye, and what of the image to show.
pub struct QuadDraw {
    pub image: vk::Image,
    pub layer: u32,
    pub layout: vk::ImageLayout,
    pub format: vk::Format,
    /// `u0, v0, u1, v1` of the part of the image to show.
    pub uv_rect: [f32; 4],
    /// Model-view-projection for the left and right eye, column-major.
    pub mvp: [[f32; 16]; 2],
    pub visible: [bool; 2],
    /// Use the image's alpha at all; otherwise it is opaque.
    pub use_alpha: bool,
    /// The colour has not been multiplied by the alpha.
    pub unpremultiplied: bool,
}

/// How many quads one frame may hold. A frame with more draws the first of them.
const MAX_QUADS: u32 = 16;

struct QuadState {
    format: vk::Format,
    render_pass: vk::RenderPass,
    set_layout: vk::DescriptorSetLayout,
    layout: vk::PipelineLayout,
    pipeline: vk::Pipeline,
    sampler: vk::Sampler,
    vert: vk::ShaderModule,
    frag: vk::ShaderModule,
    pool: vk::DescriptorPool,
    canvas: Option<Canvas>,
}

/// The two eyes' picture before it is copied out: an ordinary optimal image, because a linear
/// one that can be exported is not something a render pass may be told to draw into.
struct Canvas {
    image: Image,
    view: vk::ImageView,
    framebuffer: vk::Framebuffer,
    width: u32,
    height: u32,
}

fn words(bytes: &[u8]) -> Vec<u32> {
    bytes.chunks_exact(4).map(|c| u32::from_le_bytes([c[0], c[1], c[2], c[3]])).collect()
}

impl Vk {
    unsafe fn make_quad_state(&self, format: vk::Format) -> Result<QuadState, String> {
        let d = &self.device;
        let attachment = vk::AttachmentDescription::default()
            .format(format)
            .samples(vk::SampleCountFlags::TYPE_1)
            .load_op(vk::AttachmentLoadOp::LOAD)
            .store_op(vk::AttachmentStoreOp::STORE)
            .initial_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .final_layout(vk::ImageLayout::TRANSFER_SRC_OPTIMAL);
        let colour = [vk::AttachmentReference::default()
            .attachment(0)
            .layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)];
        let subpass = [vk::SubpassDescription::default()
            .pipeline_bind_point(vk::PipelineBindPoint::GRAPHICS)
            .color_attachments(&colour)];
        let dependencies = [
            vk::SubpassDependency::default()
                .src_subpass(vk::SUBPASS_EXTERNAL)
                .dst_subpass(0)
                .src_stage_mask(vk::PipelineStageFlags::TRANSFER)
                .dst_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
                .src_access_mask(vk::AccessFlags::TRANSFER_WRITE)
                .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_READ | vk::AccessFlags::COLOR_ATTACHMENT_WRITE),
            vk::SubpassDependency::default()
                .src_subpass(0)
                .dst_subpass(vk::SUBPASS_EXTERNAL)
                .src_stage_mask(vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT)
                .dst_stage_mask(vk::PipelineStageFlags::TRANSFER)
                .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
                .dst_access_mask(vk::AccessFlags::TRANSFER_READ),
        ];
        let attachments = [attachment];
        let render_pass = d
            .create_render_pass(
                &vk::RenderPassCreateInfo::default()
                    .attachments(&attachments)
                    .subpasses(&subpass)
                    .dependencies(&dependencies),
                None,
            )
            .map_err(|e| format!("vkCreateRenderPass: {e}"))?;
        let bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(1)
                .descriptor_type(vk::DescriptorType::SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
        ];
        let set_layout = d
            .create_descriptor_set_layout(&vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings), None)
            .map_err(|e| format!("vkCreateDescriptorSetLayout: {e}"))?;
        let ranges = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX | vk::ShaderStageFlags::FRAGMENT)
            .offset(0)
            .size(96)];
        let set_layouts = [set_layout];
        let layout = d
            .create_pipeline_layout(
                &vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts).push_constant_ranges(&ranges),
                None,
            )
            .map_err(|e| format!("vkCreatePipelineLayout: {e}"))?;
        let vert_words = words(QUAD_VERT);
        let frag_words = words(QUAD_FRAG);
        let vert = d
            .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&vert_words), None)
            .map_err(|e| format!("vkCreateShaderModule (vertex): {e}"))?;
        let frag = d
            .create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&frag_words), None)
            .map_err(|e| format!("vkCreateShaderModule (fragment): {e}"))?;
        let stages = [
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::VERTEX)
                .module(vert)
                .name(c"main"),
            vk::PipelineShaderStageCreateInfo::default()
                .stage(vk::ShaderStageFlags::FRAGMENT)
                .module(frag)
                .name(c"main"),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
        let assembly = vk::PipelineInputAssemblyStateCreateInfo::default().topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport = vk::PipelineViewportStateCreateInfo::default().viewport_count(1).scissor_count(1);
        let raster = vk::PipelineRasterizationStateCreateInfo::default()
            .polygon_mode(vk::PolygonMode::FILL)
            .cull_mode(vk::CullModeFlags::NONE)
            .front_face(vk::FrontFace::COUNTER_CLOCKWISE)
            .line_width(1.0);
        let multisample =
            vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(vk::SampleCountFlags::TYPE_1);
        let blend_attachment = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(true)
            .src_color_blend_factor(vk::BlendFactor::ONE)
            .dst_color_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ONE)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE_MINUS_SRC_ALPHA)
            .alpha_blend_op(vk::BlendOp::ADD)
            .color_write_mask(vk::ColorComponentFlags::RGBA)];
        let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&blend_attachment);
        let dynamic_states = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic_states);
        let info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&assembly)
            .viewport_state(&viewport)
            .rasterization_state(&raster)
            .multisample_state(&multisample)
            .color_blend_state(&blend)
            .dynamic_state(&dynamic)
            .layout(layout)
            .render_pass(render_pass)
            .subpass(0);
        let pipeline = d
            .create_graphics_pipelines(vk::PipelineCache::null(), &[info], None)
            .map_err(|(_, e)| format!("vkCreateGraphicsPipelines: {e}"))?[0];
        let sampler = d
            .create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                None,
            )
            .map_err(|e| format!("vkCreateSampler: {e}"))?;
        let sizes = [
            vk::DescriptorPoolSize::default().ty(vk::DescriptorType::SAMPLED_IMAGE).descriptor_count(MAX_QUADS),
            vk::DescriptorPoolSize::default().ty(vk::DescriptorType::SAMPLER).descriptor_count(MAX_QUADS),
        ];
        let pool = d
            .create_descriptor_pool(
                &vk::DescriptorPoolCreateInfo::default().max_sets(MAX_QUADS).pool_sizes(&sizes),
                None,
            )
            .map_err(|e| format!("vkCreateDescriptorPool: {e}"))?;
        Ok(QuadState {
            format,
            render_pass,
            set_layout,
            layout,
            pipeline,
            sampler,
            vert,
            frag,
            pool,
            canvas: None,
        })
    }

    unsafe fn destroy_quad_state(&self, state: QuadState) {
        let d = &self.device;
        let _ = d.device_wait_idle();
        if let Some(canvas) = &state.canvas {
            d.destroy_framebuffer(canvas.framebuffer, None);
            d.destroy_image_view(canvas.view, None);
            self.destroy_image(&canvas.image);
        }
        d.destroy_descriptor_pool(state.pool, None);
        d.destroy_sampler(state.sampler, None);
        d.destroy_pipeline(state.pipeline, None);
        d.destroy_shader_module(state.vert, None);
        d.destroy_shader_module(state.frag, None);
        d.destroy_pipeline_layout(state.layout, None);
        d.destroy_descriptor_set_layout(state.set_layout, None);
        d.destroy_render_pass(state.render_pass, None);
    }

    /// Everything ready to draw `quads` into a canvas of this size and format.
    unsafe fn prepare_quads(
        &self,
        guard: &mut Option<QuadState>,
        format: vk::Format,
        width: u32,
        height: u32,
    ) -> Result<(), String> {
        if guard.as_ref().is_some_and(|s| s.format != format) {
            if let Some(old) = guard.take() {
                self.destroy_quad_state(old);
            }
        }
        if guard.is_none() {
            *guard = Some(self.make_quad_state(format)?);
        }
        let state = guard.as_mut().unwrap();
        if state.canvas.as_ref().is_some_and(|c| c.width != width || c.height != height) {
            let _ = self.device.device_wait_idle();
            if let Some(c) = state.canvas.take() {
                self.device.destroy_framebuffer(c.framebuffer, None);
                self.device.destroy_image_view(c.view, None);
                self.destroy_image(&c.image);
            }
        }
        if state.canvas.is_none() {
            let image = self.create_image(
                format,
                width,
                height,
                1,
                1,
                vk::SampleCountFlags::TYPE_1,
                vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_DST,
            )?;
            let view = self
                .device
                .create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(image.image)
                        .view_type(vk::ImageViewType::TYPE_2D)
                        .format(format)
                        .subresource_range(vk::ImageSubresourceRange {
                            aspect_mask: vk::ImageAspectFlags::COLOR,
                            base_mip_level: 0,
                            level_count: 1,
                            base_array_layer: 0,
                            layer_count: 1,
                        }),
                    None,
                )
                .map_err(|e| format!("vkCreateImageView (canvas): {e}"))?;
            let views = [view];
            let framebuffer = self
                .device
                .create_framebuffer(
                    &vk::FramebufferCreateInfo::default()
                        .render_pass(state.render_pass)
                        .attachments(&views)
                        .width(width)
                        .height(height)
                        .layers(1),
                    None,
                )
                .map_err(|e| format!("vkCreateFramebuffer: {e}"))?;
            state.canvas = Some(Canvas { image, view, framebuffer, width, height });
        }
        Ok(())
    }
}

/// The part of an eye's source image to copy, top to bottom -- or, for an image whose rows run the
/// other way, bottom to top, which makes the blit turn it over on the way.
fn src_offsets(copy: &EyeCopy) -> [vk::Offset3D; 2] {
    let (x, y, w, h) = (copy.rect.0, copy.rect.1, copy.rect.2 as i32, copy.rect.3 as i32);
    if copy.flip_y {
        [vk::Offset3D { x, y: y + h, z: 0 }, vk::Offset3D { x: x + w, y, z: 1 }]
    } else {
        [vk::Offset3D { x, y, z: 0 }, vk::Offset3D { x: x + w, y: y + h, z: 1 }]
    }
}
