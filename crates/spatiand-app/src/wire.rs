//! The wire: an application's end of the cut-down Wayland the compositor speaks.
//!
//! Grown from `crates/spatiand/src/remote/client.rs`, which is the same client with a host's
//! decoder behind it instead of an application. Nothing here is in the crate's interface: an
//! application sees windows and events, and which protocol carried them is this file's
//! business alone.

use std::collections::HashMap;
use std::os::fd::{AsFd, AsRawFd, FromRawFd};
use std::os::unix::net::UnixStream;
use std::time::{Duration, Instant};

use wayland_client::globals::{registry_queue_init, GlobalList, GlobalListContents};
use wayland_client::protocol::{
    wl_buffer, wl_callback, wl_compositor, wl_data_device, wl_data_device_manager, wl_data_offer, wl_data_source,
    wl_keyboard, wl_pointer, wl_registry, wl_seat, wl_shm, wl_shm_pool, wl_surface,
};
use wayland_client::{event_created_child, Connection, Dispatch, EventQueue, Proxy, QueueHandle, WEnum};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{zwp_linux_buffer_params_v1, zwp_linux_dmabuf_v1};
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

use spatiand_proto::client::{spatiand_xr_surface_v1, spatiand_xr_v1};

use crate::keys::Keyboard;
use crate::{Anchor, Button, Event, EyeLayout, Layer, Mode, WindowId};

/// A buffer of shared memory, and whether the compositor still has it.
struct ShmBuffer {
    buffer: wl_buffer::WlBuffer,
    map: memmap2::MmapMut,
    size: (u32, u32),
    /// Its name in its `wl_buffer`'s user data, which outlives its place in the list.
    key: u32,
    /// Attached and not yet released: not to be drawn into.
    busy: bool,
}

/// What a drag this application started carries: each form it can be had in, and its bytes.
pub(crate) struct DragData {
    pub(crate) forms: Vec<(String, Vec<u8>)>,
}

/// A drag this application started, while it goes on.
struct Dragging {
    source: wl_data_source::WlDataSource,
    data: std::sync::Arc<DragData>,
    /// The picture that goes with it, as the compositor draws it at the pointer.
    icon: Option<(wl_surface::WlSurface, wl_buffer::WlBuffer, memmap2::MmapMut)>,
    /// What the window it is over would do with it: move it rather than copy it.
    moving: bool,
}

/// Something dragged over one of this application's windows, from here or anywhere.
struct Over {
    offer: Option<wl_data_offer::WlDataOffer>,
    window: WindowId,
    serial: u32,
    x: f32,
    y: f32,
    /// Let go of: kept until the window has had what it wanted of it ([`Wire::finish_drop`]).
    dropped: bool,
}

/// What stands in for a picture that is not in shared memory: see `gl::mac`.
pub(crate) struct Placeholder {
    buffer: wl_buffer::WlBuffer,
    map: memmap2::MmapMut,
    size: (u32, u32),
}

pub(crate) struct Window {
    surface: wl_surface::WlSurface,
    xdg: xdg_surface::XdgSurface,
    toplevel: xdg_toplevel::XdgToplevel,
    xr: Option<spatiand_xr_surface_v1::SpatiandXrSurfaceV1>,
    viewport: Option<wp_viewport::WpViewport>,
    /// The compositor has said its first word, before which nothing may be attached.
    configured: bool,
    /// The size the compositor last asked for, waiting for the configure that settles it.
    asked: Option<(i32, i32)>,
    shm: Vec<ShmBuffer>,
    placeholder: Option<Placeholder>,
    /// Buffers made from a GPU's pictures and still the compositor's: by key.
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    foreign: HashMap<u32, wl_buffer::WlBuffer>,
    next_key: u32,
    /// A frame callback is outstanding, so another need not be asked for.
    frame_asked: bool,
}

pub(crate) struct Wire {
    handle: QueueHandle<Wire>,
    compositor: wl_compositor::WlCompositor,
    shell: xdg_wm_base::XdgWmBase,
    shm: wl_shm::WlShm,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    dmabuf: Option<zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1>,
    xr: Option<spatiand_xr_v1::SpatiandXrV1>,
    viewporter: Option<wp_viewporter::WpViewporter>,
    pub(crate) mode: Mode,
    windows: HashMap<WindowId, Window>,
    ids: HashMap<wl_surface::WlSurface, WindowId>,
    next_window: u32,
    pub(crate) events: Vec<Event>,
    pub(crate) keyboard: Keyboard,
    pointer_on: Option<WindowId>,
    keyboard_on: Option<WindowId>,
    /// Buffers of a GPU's pictures the compositor has finished with: (window, key).
    pub(crate) released: Vec<(WindowId, u32)>,
    /// The room's clipboard: see `crate::clipboard`.
    data_devices: Option<wl_data_device_manager::WlDataDeviceManager>,
    data_device: Option<wl_data_device::WlDataDevice>,
    /// What each offer on the way says it can be, by the forms it lists before it is chosen.
    offers: HashMap<wl_data_offer::WlDataOffer, Vec<String>>,
    /// The offer that is the clipboard now, to be let go of when the next comes.
    selection: Option<wl_data_offer::WlDataOffer>,
    /// What this application put on the clipboard, while it is there.
    source: Option<wl_data_source::WlDataSource>,
    /// The newest input event's serial: a copy is made "in answer to" one.
    serial: u32,
    /// The last press of a button's serial: a drag is started in answer to it, while it is held.
    press_serial: u32,
    dragging: Option<Dragging>,
    over: Option<Over>,
}

/// Text's names, the best first.
const TEXT_FORMS: &[&str] = &["text/plain;charset=utf-8", "UTF8_STRING", "text/plain", "TEXT", "STRING"];

/// The buffer's part in a `wl_buffer`'s user data.
#[derive(Clone, Copy)]
enum Held {
    Shm(u32),
    Placeholder,
    #[cfg_attr(not(target_os = "linux"), allow(dead_code))]
    Foreign(u32),
}

impl Wire {
    /// Connect to the compositor: over a socket it handed down at launch, or the one its
    /// environment names.
    pub(crate) fn connect() -> Result<(Wire, EventQueue<Wire>), String> {
        let connection = match std::env::var("SPATIAND_APP_FD").ok().and_then(|fd| fd.parse::<i32>().ok()) {
            // SAFETY: the compositor passed this descriptor to us and to nobody else.
            Some(fd) => Connection::from_socket(unsafe { UnixStream::from_raw_fd(fd) })
                .map_err(|e| format!("could not speak to Spatiand: {e}"))?,
            None => Connection::connect_to_env().map_err(|e| format!("Spatiand is not running here: {e}"))?,
        };
        let (globals, queue): (GlobalList, EventQueue<Wire>) =
            registry_queue_init(&connection).map_err(|e| format!("could not read Spatiand's globals: {e}"))?;
        let handle = queue.handle();
        let compositor = globals.bind(&handle, 4..=6, ()).map_err(|e| format!("no wl_compositor: {e}"))?;
        let shell = globals.bind(&handle, 1..=6, ()).map_err(|e| format!("no xdg_wm_base: {e}"))?;
        let shm = globals.bind(&handle, 1..=2, ()).map_err(|e| format!("no wl_shm: {e}"))?;
        let seat: wl_seat::WlSeat = globals.bind(&handle, 5..=9, ()).map_err(|e| format!("no seat: {e}"))?;
        // The clipboard, where there is one to join: a compositor without it only means no
        // copying and pasting.
        let data_devices: Option<wl_data_device_manager::WlDataDeviceManager> = globals.bind(&handle, 1..=3, ()).ok();
        let data_device = data_devices.as_ref().map(|m| m.get_data_device(&seat, &handle, ()));
        let dmabuf = globals.bind(&handle, 3..=3, ()).ok();
        let xr: Option<spatiand_xr_v1::SpatiandXrV1> = globals.bind(&handle, 1..=8, ()).ok();
        let viewporter = globals.bind(&handle, 1..=1, ()).ok();
        // A compositor with the room's protocol and no word on the mode is an older Spatiand,
        // which has only ever had two eyes; one without the protocol is somebody's desktop.
        let mode = match &xr {
            Some(_) => Mode::Stereo,
            None => Mode::Flat,
        };
        let wire = Wire {
            handle,
            compositor,
            shell,
            shm,
            dmabuf,
            xr,
            viewporter,
            mode,
            windows: HashMap::new(),
            ids: HashMap::new(),
            next_window: 1,
            events: Vec::new(),
            keyboard: Keyboard::default(),
            pointer_on: None,
            keyboard_on: None,
            released: Vec::new(),
            data_devices,
            data_device,
            offers: HashMap::new(),
            selection: None,
            source: None,
            serial: 0,
            press_serial: 0,
            dragging: None,
            over: None,
        };
        Ok((wire, queue))
    }

    /// Whether the compositor is Spatiand, and so whether a layer or an eye layout means anything.
    pub(crate) fn in_the_room(&self) -> bool {
        self.xr.is_some()
    }

    pub(crate) fn open(&mut self, app_id: &str, title: &str) -> WindowId {
        let id = WindowId(self.next_window);
        self.next_window += 1;
        let surface = self.compositor.create_surface(&self.handle, ());
        let xdg = self.shell.get_xdg_surface(&surface, &self.handle, id);
        let toplevel = xdg.get_toplevel(&self.handle, id);
        toplevel.set_app_id(app_id.to_string());
        toplevel.set_title(title.to_string());
        // With nothing attached: this is what asks for the first configure.
        surface.commit();
        self.ids.insert(surface.clone(), id);
        self.windows.insert(
            id,
            Window {
                surface,
                xdg,
                toplevel,
                xr: None,
                viewport: None,
                configured: false,
                asked: None,
                shm: Vec::new(),
                placeholder: None,
                foreign: HashMap::new(),
                next_key: 0,
                frame_asked: false,
            },
        );
        id
    }

    pub(crate) fn close(&mut self, id: WindowId) {
        let Some(window) = self.windows.remove(&id) else { return };
        self.ids.remove(&window.surface);
        if self.pointer_on == Some(id) {
            self.pointer_on = None;
        }
        if self.keyboard_on == Some(id) {
            self.keyboard_on = None;
            self.keyboard.let_go();
        }
        if let Some(xr) = window.xr {
            xr.destroy();
        }
        if let Some(viewport) = window.viewport {
            viewport.destroy();
        }
        window.toplevel.destroy();
        window.xdg.destroy();
        window.surface.destroy();
        // Said in so many words: dropping a proxy does not destroy what it stands for, and a
        // compositor left holding a closed window's buffers holds their memory with them.
        for held in window.shm {
            held.buffer.destroy();
        }
        if let Some(placeholder) = window.placeholder {
            placeholder.buffer.destroy();
        }
        for (_, buffer) in window.foreign {
            buffer.destroy();
        }
    }

    pub(crate) fn is_ready(&self, id: WindowId) -> bool {
        self.windows.get(&id).is_some_and(|w| w.configured)
    }

    pub(crate) fn set_title(&mut self, id: WindowId, title: &str) {
        if let Some(window) = self.windows.get(&id) {
            window.toplevel.set_title(title.to_string());
        }
    }

    fn xr_of(&mut self, id: WindowId) -> Option<&spatiand_xr_surface_v1::SpatiandXrSurfaceV1> {
        let xr = self.xr.as_ref()?;
        let window = self.windows.get_mut(&id)?;
        Some(window.xr.get_or_insert_with(|| xr.get_xr_surface(&window.surface, &self.handle, id)))
    }

    /// Everything said through `spatiand_xr_v1` takes effect with the window's next picture,
    /// like everything else about a surface.
    pub(crate) fn set_eyes(&mut self, id: WindowId, eyes: EyeLayout, swapped: bool) {
        use spatiand_xr_surface_v1::EyeLayout as Wire;
        if let Some(xr) = self.xr_of(id) {
            xr.set_eye_layout(match eyes {
                EyeLayout::Mono => Wire::Mono,
                EyeLayout::SideBySide => Wire::SideBySide,
                EyeLayout::TopBottom => Wire::TopBottom,
            });
            xr.set_eye_swapped(swapped as u32);
        }
    }

    pub(crate) fn set_layer(&mut self, id: WindowId, layer: Layer) {
        if let Some(xr) = self.xr_of(id) {
            xr.set_layer(layer_to_wire(layer));
        }
    }

    pub(crate) fn set_yaw(&mut self, id: WindowId, radians: f32) {
        if let Some(xr) = self.xr_of(id) {
            xr.set_yaw_offset((radians * 1e6).round() as i32);
        }
    }

    pub(crate) fn set_idle_fade(&mut self, id: WindowId, after: Option<Duration>) {
        if let Some(xr) = self.xr_of(id) {
            if xr.version() < 2 {
                return;
            }
            xr.set_idle_fade(after.is_some() as u32);
            if let (Some(after), true) = (after, xr.version() >= 3) {
                xr.set_idle_after(after.as_millis().min(u32::MAX as u128) as u32);
            }
        }
    }

    pub(crate) fn set_anchor(&mut self, id: WindowId, anchor: Anchor) {
        use spatiand_xr_surface_v1::ResizeAnchor as Wire;
        if let Some(xr) = self.xr_of(id) {
            if xr.version() >= 3 {
                xr.set_resize_anchor(match anchor {
                    Anchor::Centre => Wire::Centre,
                    Anchor::Top => Wire::Top,
                    Anchor::Bottom => Wire::Bottom,
                });
            }
        }
    }

    pub(crate) fn set_pinch_taken(&mut self, id: WindowId, taken: bool) {
        if let Some(xr) = self.xr_of(id) {
            if xr.version() >= 6 {
                xr.set_pinch_taken(taken as u32);
            }
        }
    }

    pub(crate) fn set_see_through(&mut self, id: WindowId, clear: bool) {
        if let Some(xr) = self.xr_of(id) {
            if xr.version() >= 6 {
                xr.set_see_through(clear as u32);
            }
        }
    }

    pub(crate) fn set_sound_key(&mut self, id: WindowId, key: &str) {
        if let Some(xr) = self.xr_of(id) {
            // Not to a room older than this request: it would end the connection.
            if xr.version() >= 8 {
                xr.set_sound_key(key.to_string());
            }
        }
    }

    pub(crate) fn set_cursor_drawn(&mut self, id: WindowId, drawn: bool) {
        if let Some(xr) = self.xr_of(id) {
            if xr.version() >= 4 {
                xr.set_cursor_drawn(drawn as u32);
            }
        }
    }

    /// The size the window is, where that is not its picture's: a film whose two eyes are
    /// side by side in a picture twice as wide as either. `None` is the picture's own size.
    pub(crate) fn set_shape(&mut self, id: WindowId, shape: Option<(u32, u32)>) {
        let Some(viewporter) = self.viewporter.as_ref() else { return };
        let Some(window) = self.windows.get_mut(&id) else { return };
        match shape {
            Some((width, height)) => {
                let viewport = window.viewport.get_or_insert_with(|| viewporter.get_viewport(&window.surface, &self.handle, ()));
                viewport.set_destination(width.max(1) as i32, height.max(1) as i32);
            }
            None => {
                if let Some(viewport) = window.viewport.take() {
                    viewport.destroy();
                }
            }
        }
    }

    /// Ask to be told when the compositor would like the window's next picture.
    pub(crate) fn ask_frame(&mut self, id: WindowId) {
        let Some(window) = self.windows.get_mut(&id) else { return };
        if !std::mem::replace(&mut window.frame_asked, true) {
            window.surface.frame(&self.handle, id);
        }
    }

    /// Commit what has been said about a window without giving it a new picture.
    pub(crate) fn commit(&mut self, id: WindowId) {
        if let Some(window) = self.windows.get(&id) {
            if window.configured {
                window.surface.commit();
            }
        }
    }

    /// Show a picture from ordinary memory: `fill` is given the buffer to draw into, four bytes
    /// a pixel in the order blue, green, red, alpha, colour already multiplied by alpha, top
    /// row first.
    pub(crate) fn show_pixels(&mut self, id: WindowId, size: (u32, u32), fill: impl FnOnce(&mut [u8])) -> Result<(), String> {
        let handle = self.handle.clone();
        let shm = self.shm.clone();
        let Some(window) = self.windows.get_mut(&id) else { return Ok(()) };
        if !window.configured {
            return Ok(());
        }
        // Buffers of a size the window no longer is are of no use to it again.
        window.shm.retain(|held| {
            let keep = held.size == size || held.busy;
            if !keep {
                held.buffer.destroy();
            }
            keep
        });
        let free = window.shm.iter().position(|held| !held.busy && held.size == size);
        let index = match free {
            Some(index) => index,
            None => {
                let key = window.next_key;
                window.next_key = window.next_key.wrapping_add(1);
                let bytes = (size.0 * size.1 * 4) as usize;
                let file = memory(bytes)?;
                let map = unsafe { memmap2::MmapMut::map_mut(&file) }.map_err(|e| format!("could not map a buffer: {e}"))?;
                let pool = shm.create_pool(file.as_fd(), bytes as i32, &handle, ());
                let buffer = pool.create_buffer(
                    0,
                    size.0 as i32,
                    size.1 as i32,
                    size.0 as i32 * 4,
                    wl_shm::Format::Argb8888,
                    &handle,
                    (id, Held::Shm(key)),
                );
                pool.destroy();
                window.shm.push(ShmBuffer { buffer, map, size, key, busy: false });
                window.shm.len() - 1
            }
        };
        let held = &mut window.shm[index];
        fill(&mut held.map);
        held.busy = true;
        window.surface.attach(Some(&held.buffer), 0, 0);
        window.surface.damage_buffer(0, 0, size.0 as i32, size.1 as i32);
        window.surface.commit();
        Ok(())
    }

    /// Show a picture that lives somewhere the compositor can reach by name: the window is
    /// given a buffer of the picture's size whose first bytes are that name (`head`), and the
    /// compositor draws what the name finds. The buffer is made once for a size and its head
    /// rewritten for each picture after -- shared memory, so the compositor reads the new name
    /// without being sent anything but the commit.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub(crate) fn show_named(&mut self, id: WindowId, size: (u32, u32), head: &[u8]) -> Result<(), String> {
        let handle = self.handle.clone();
        let shm = self.shm.clone();
        let Some(window) = self.windows.get_mut(&id) else { return Ok(()) };
        if !window.configured {
            return Ok(());
        }
        if window.placeholder.as_ref().is_some_and(|p| p.size != size) {
            if let Some(old) = window.placeholder.take() {
                old.buffer.destroy();
            }
        }
        let fresh = window.placeholder.is_none();
        if fresh {
            let bytes = (size.0 * size.1 * 4) as usize;
            let file = memory(bytes)?;
            // Only the head is ever touched, so only the head is mapped.
            let map = unsafe { memmap2::MmapOptions::new().len(4096.min(bytes)).map_mut(&file) }
                .map_err(|e| format!("could not map a placeholder: {e}"))?;
            let pool = shm.create_pool(file.as_fd(), bytes as i32, &handle, ());
            let buffer = pool.create_buffer(
                0,
                size.0 as i32,
                size.1 as i32,
                size.0 as i32 * 4,
                wl_shm::Format::Xrgb8888,
                &handle,
                (id, Held::Placeholder),
            );
            pool.destroy();
            window.placeholder = Some(Placeholder { buffer, map, size });
        }
        let placeholder = window.placeholder.as_mut().unwrap();
        placeholder.map[..head.len()].copy_from_slice(head);
        if fresh {
            window.surface.attach(Some(&placeholder.buffer), 0, 0);
            window.surface.damage_buffer(0, 0, size.0 as i32, size.1 as i32);
        }
        // With no damage otherwise, so the compositor never reads the placeholder's memory as
        // a picture again: only its head, which it looks at every frame.
        window.surface.commit();
        Ok(())
    }

    /// Whether the compositor takes a GPU's pictures as dmabufs, in this format and tiling.
    #[cfg(target_os = "linux")]
    pub(crate) fn takes_dmabufs(&self) -> bool {
        self.dmabuf.is_some()
    }

    /// Show a GPU's picture without copying it. `key` comes back in [`Wire::released`] when
    /// the compositor has finished with it, before which it may not be drawn into again.
    #[cfg(target_os = "linux")]
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn show_dmabuf(
        &mut self,
        id: WindowId,
        key: u32,
        size: (u32, u32),
        fourcc: u32,
        modifier: u64,
        planes: &[(std::os::fd::BorrowedFd<'_>, u32, u32)],
    ) -> Result<(), String> {
        let handle = self.handle.clone();
        let Some(dmabuf) = self.dmabuf.clone() else { return Err("the compositor takes no dmabufs".into()) };
        let Some(window) = self.windows.get_mut(&id) else { return Ok(()) };
        if !window.configured {
            return Ok(());
        }
        let buffer = match window.foreign.get(&key) {
            Some(buffer) => buffer.clone(),
            None => {
                let params = dmabuf.create_params(&handle, ());
                for (plane, (fd, offset, stride)) in planes.iter().enumerate() {
                    params.add(*fd, plane as u32, *offset, *stride, (modifier >> 32) as u32, modifier as u32);
                }
                let buffer = params.create_immed(
                    size.0 as i32,
                    size.1 as i32,
                    fourcc,
                    zwp_linux_buffer_params_v1::Flags::empty(),
                    &handle,
                    (id, Held::Foreign(key)),
                );
                params.destroy();
                window.foreign.insert(key, buffer.clone());
                buffer
            }
        };
        window.surface.attach(Some(&buffer), 0, 0);
        window.surface.damage_buffer(0, 0, size.0 as i32, size.1 as i32);
        window.surface.commit();
        Ok(())
    }

    /// Forget a GPU picture's buffer: its picture is being thrown away.
    #[cfg(target_os = "linux")]
    pub(crate) fn forget_dmabuf(&mut self, id: WindowId, key: u32) {
        if let Some(buffer) = self.windows.get_mut(&id).and_then(|w| w.foreign.remove(&key)) {
            buffer.destroy();
        }
    }

    /// Put text on the clipboard: this application serves it while it is there.
    pub(crate) fn copy(&mut self, text: String) {
        let (Some(manager), Some(device)) = (self.data_devices.as_ref(), self.data_device.as_ref()) else {
            log::warn!("the compositor has no clipboard: nothing copied");
            return;
        };
        log::info!("copied {} characters to the clipboard", text.chars().count());
        let source = manager.create_data_source(&self.handle, std::sync::Arc::new(text));
        for form in TEXT_FORMS {
            source.offer(form.to_string());
        }
        device.set_selection(Some(&source), self.serial);
        if let Some(old) = self.source.replace(source) {
            old.destroy();
        }
    }

    /// Start dragging something out of `window`, while the button pressed in it is held: it can
    /// be had in each of `forms`; `icon` (width, height, and pixels as [`Wire::show_pixels`]
    /// takes them) is drawn at the pointer, by the compositor, wherever it goes in the room.
    /// Whether the compositor took it.
    pub(crate) fn start_drag(&mut self, window: WindowId, data: DragData, icon: Option<(u32, u32, Vec<u8>)>, can_move: bool) -> bool {
        let (Some(manager), Some(device)) = (self.data_devices.as_ref(), self.data_device.as_ref()) else { return false };
        let Some(origin) = self.windows.get(&window).map(|w| w.surface.clone()) else { return false };
        if let Some(old) = self.dragging.take() {
            old.source.destroy();
        }
        let data = std::sync::Arc::new(data);
        let source = manager.create_data_source(&self.handle, data.clone());
        for (form, _) in &data.forms {
            source.offer(form.clone());
        }
        if source.version() >= 3 {
            let mut actions = wl_data_device_manager::DndAction::Copy;
            if can_move {
                actions |= wl_data_device_manager::DndAction::Move;
            }
            source.set_actions(actions);
        }
        let icon = icon.and_then(|(width, height, pixels)| {
            let bytes = (width * height * 4) as usize;
            if width == 0 || height == 0 || pixels.len() < bytes {
                return None;
            }
            let file = memory(bytes).ok()?;
            let mut map = unsafe { memmap2::MmapMut::map_mut(&file) }.ok()?;
            map.copy_from_slice(&pixels[..bytes]);
            let pool = self.shm.create_pool(file.as_fd(), bytes as i32, &self.handle, ());
            let buffer = pool.create_buffer(0, width as i32, height as i32, width as i32 * 4, wl_shm::Format::Argb8888, &self.handle, (window, Held::Placeholder));
            pool.destroy();
            Some((self.compositor.create_surface(&self.handle, ()), buffer, map, (width, height)))
        });
        log::info!("dragging, in {} forms, from the press {}", data.forms.len(), self.press_serial);
        device.start_drag(Some(&source), &origin, icon.as_ref().map(|i| &i.0), self.press_serial);
        // Its picture is the icon's once it has the role, which starting the drag gives it.
        let icon = icon.map(|(surface, buffer, map, (width, height))| {
            surface.attach(Some(&buffer), 0, 0);
            surface.damage_buffer(0, 0, width as i32, height as i32);
            surface.commit();
            (surface, buffer, map)
        });
        self.dragging = Some(Dragging { source, data, icon, moving: false });
        true
    }

    pub(crate) fn dragging(&self) -> bool {
        self.dragging.is_some()
    }

    /// What the window under a drag would do with it: take it in `form` (none: not at all),
    /// moving it rather than copying it.
    pub(crate) fn accept_drag(&mut self, form: Option<&str>, moving: bool) {
        let Some(over) = self.over.as_ref() else { return };
        let Some(offer) = over.offer.as_ref() else { return };
        offer.accept(over.serial, form.map(str::to_string));
        if offer.version() >= 3 {
            use wl_data_device_manager::DndAction;
            match form {
                Some(_) => offer.set_actions(DndAction::Copy | DndAction::Move, if moving { DndAction::Move } else { DndAction::Copy }),
                None => offer.set_actions(DndAction::empty(), DndAction::empty()),
            }
        }
    }

    /// What is dragged over (or was let go of over) a window, in `form`: this application's
    /// own, at once; another's, to be read from what it writes.
    pub(crate) fn drop_data(&mut self, form: &str) -> DropRead {
        if let Some(dragging) = &self.dragging {
            if let Some((_, bytes)) = dragging.data.forms.iter().find(|(f, _)| f == form) {
                return DropRead::Here(bytes.clone());
            }
        }
        let Some(offer) = self.over.as_ref().and_then(|o| o.offer.as_ref()) else { return DropRead::Nothing };
        let mut fds = [0i32; 2];
        if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
            return DropRead::Nothing;
        }
        // SAFETY: both ends are new and ours alone.
        let (read, write) = unsafe { (std::fs::File::from_raw_fd(fds[0]), std::os::fd::OwnedFd::from_raw_fd(fds[1])) };
        offer.receive(form.to_string(), write.as_fd());
        drop(write);
        DropRead::Pipe(read)
    }

    /// The window has had what it wanted of what was let go of over it.
    pub(crate) fn finish_drop(&mut self) {
        let Some(over) = self.over.take() else { return };
        if let Some(offer) = over.offer {
            if over.dropped && offer.version() >= 3 {
                offer.finish();
            }
            self.offers.remove(&offer);
            offer.destroy();
        }
    }

    /// When a held key is next to repeat, if one is held.
    pub(crate) fn next_repeat(&self) -> Option<Instant> {
        self.keyboard.next_repeat()
    }

    /// Make the key events for the repeats that have come due.
    pub(crate) fn repeat(&mut self, now: Instant) {
        if let Some(window) = self.keyboard_on {
            self.keyboard.repeat(now, window, &mut self.events);
        }
    }
}

fn layer_to_wire(layer: Layer) -> spatiand_xr_surface_v1::Layer {
    use spatiand_xr_surface_v1::Layer as Wire;
    match layer {
        Layer::Window => Wire::Window,
        Layer::HeadLocked => Wire::HeadLocked,
        Layer::Projection => Wire::Projection,
        Layer::Sky180 => Wire::Equirect180,
        Layer::Sky360 => Wire::Equirect360,
    }
}

fn layer_from_wire(layer: WEnum<spatiand_xr_surface_v1::Layer>) -> Layer {
    use spatiand_xr_surface_v1::Layer as Wire;
    match layer {
        WEnum::Value(Wire::HeadLocked) => Layer::HeadLocked,
        WEnum::Value(Wire::Projection) => Layer::Projection,
        WEnum::Value(Wire::Equirect180) => Layer::Sky180,
        WEnum::Value(Wire::Equirect360) => Layer::Sky360,
        _ => Layer::Window,
    }
}

/// Memory with a descriptor, to share with the compositor.
fn memory(bytes: usize) -> Result<std::fs::File, String> {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    let file = {
        let fd = unsafe { libc::memfd_create(c"spatiand-app".as_ptr(), libc::MFD_CLOEXEC) };
        if fd < 0 {
            return Err(format!("could not make a buffer: {}", std::io::Error::last_os_error()));
        }
        unsafe { std::fs::File::from_raw_fd(fd) }
    };
    // A Mac has no anonymous memory with a descriptor, and does have a temporary directory.
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    let file = {
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!("spatiand-app-{}-{n}", std::process::id()));
        let file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create(true)
            .truncate(true)
            .open(&path)
            .map_err(|e| format!("could not make a buffer: {e}"))?;
        let _ = std::fs::remove_file(&path);
        file
    };
    file.set_len(bytes as u64).map_err(|e| format!("could not size a buffer: {e}"))?;
    Ok(file)
}

/// Send what is pending, and wait up to `timeout` for the compositor to say something.
pub(crate) fn pump(wire: &mut Wire, queue: &mut EventQueue<Wire>, timeout: Option<Duration>, watched: &[i32]) -> Result<(), String> {
    let lost = |e: &dyn std::fmt::Display| format!("lost Spatiand: {e}");
    queue.dispatch_pending(wire).map_err(|e| lost(&e))?;
    queue.flush().map_err(|e| lost(&e))?;
    if !wire.events.is_empty() {
        return Ok(());
    }
    let Some(guard) = queue.prepare_read() else {
        queue.dispatch_pending(wire).map_err(|e| lost(&e))?;
        return Ok(());
    };
    // The application's own files too -- a child's output -- so that it is woken for them.
    let mut fds = vec![libc::pollfd { fd: guard.connection_fd().as_raw_fd(), events: libc::POLLIN, revents: 0 }];
    fds.extend(watched.iter().map(|&fd| libc::pollfd { fd, events: libc::POLLIN, revents: 0 }));
    let wait = timeout.map_or(-1, |t| t.as_millis().min(i32::MAX as u128) as i32);
    let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, wait) };
    // A compositor that has gone leaves the socket hung up. Whatever a read of it then says, the
    // application is not wanted any more: it must end, not turn round the loop at a full core.
    if ready > 0 && fds[0].revents & (libc::POLLHUP | libc::POLLERR | libc::POLLNVAL) != 0 && fds[0].revents & libc::POLLIN == 0 {
        return Err(lost(&"the connection is closed"));
    }
    if ready > 0 && fds[0].revents != 0 {
        match guard.read() {
            Ok(_) => {}
            Err(wayland_client::backend::WaylandError::Io(e)) if e.kind() == std::io::ErrorKind::WouldBlock => {}
            Err(e) => return Err(lost(&e)),
        }
    }
    queue.dispatch_pending(wire).map_err(|e| lost(&e))?;
    Ok(())
}

// --- events ---

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for Wire {
    fn event(_: &mut Self, _: &wl_registry::WlRegistry, _: wl_registry::Event, _: &GlobalListContents, _: &Connection, _: &QueueHandle<Self>) {}
}

impl Dispatch<spatiand_xr_v1::SpatiandXrV1, ()> for Wire {
    fn event(state: &mut Self, _: &spatiand_xr_v1::SpatiandXrV1, event: spatiand_xr_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let spatiand_xr_v1::Event::Hands = event {
            state.events.push(Event::Hands);
            return;
        }
        if let spatiand_xr_v1::Event::DisplayMode { mode } = event {
            let mode = match mode {
                WEnum::Value(spatiand_xr_v1::DisplayMode::Flat) => Mode::Flat,
                _ => Mode::Stereo,
            };
            if std::mem::replace(&mut state.mode, mode) != mode {
                state.events.push(Event::Mode(mode));
            }
        }
    }
}

impl Dispatch<spatiand_xr_surface_v1::SpatiandXrSurfaceV1, WindowId> for Wire {
    fn event(
        state: &mut Self,
        _: &spatiand_xr_surface_v1::SpatiandXrSurfaceV1,
        event: spatiand_xr_surface_v1::Event,
        id: &WindowId,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            spatiand_xr_surface_v1::Event::LayerRefused { layer, reason } => {
                log::warn!("window {} was refused a layer: {reason}", id.0);
                state.events.push(Event::LayerRefused { window: *id, layer: layer_from_wire(layer) });
            }
            spatiand_xr_surface_v1::Event::LayerLost { layer } => {
                state.events.push(Event::LayerLost { window: *id, layer: layer_from_wire(layer) });
            }
            spatiand_xr_surface_v1::Event::Pinch { scale } => {
                state.events.push(Event::Pinch { window: *id, scale: (scale as f32 / 1e6).clamp(0.2, 5.0) });
            }
            _ => {}
        }
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for Wire {
    fn event(_: &mut Self, shell: &xdg_wm_base::XdgWmBase, event: xdg_wm_base::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        // Answering is what proves an application is alive.
        if let xdg_wm_base::Event::Ping { serial } = event {
            shell.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, WindowId> for Wire {
    fn event(state: &mut Self, surface: &xdg_surface::XdgSurface, event: xdg_surface::Event, id: &WindowId, _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_surface::Event::Configure { serial } = event {
            surface.ack_configure(serial);
            let Some(window) = state.windows.get_mut(id) else { return };
            let asked = window.asked.take().map(|(w, h)| (w as u32, h as u32));
            if !std::mem::replace(&mut window.configured, true) {
                // The first is only the compositor's opening suggestion for a window it knows
                // nothing about yet: passed on as that, for an application with no better idea.
                state.events.push(Event::Ready { window: *id, suggested: asked });
            } else if let Some((width, height)) = asked {
                state.events.push(Event::Resized { window: *id, width, height });
            }
        }
    }
}

impl Dispatch<xdg_toplevel::XdgToplevel, WindowId> for Wire {
    fn event(state: &mut Self, _: &xdg_toplevel::XdgToplevel, event: xdg_toplevel::Event, id: &WindowId, _: &Connection, _: &QueueHandle<Self>) {
        match event {
            xdg_toplevel::Event::Configure { width, height, .. } => {
                if width > 0 && height > 0 {
                    if let Some(window) = state.windows.get_mut(id) {
                        window.asked = Some((width, height));
                    }
                }
            }
            xdg_toplevel::Event::Close => state.events.push(Event::CloseRequested { window: *id }),
            _ => {}
        }
    }
}

impl Dispatch<wl_callback::WlCallback, WindowId> for Wire {
    fn event(state: &mut Self, _: &wl_callback::WlCallback, event: wl_callback::Event, id: &WindowId, _: &Connection, _: &QueueHandle<Self>) {
        if let wl_callback::Event::Done { callback_data } = event {
            if let Some(window) = state.windows.get_mut(id) {
                window.frame_asked = false;
                state.events.push(Event::Frame { window: *id, time_ms: callback_data });
            }
        }
    }
}

impl Dispatch<wl_buffer::WlBuffer, (WindowId, Held)> for Wire {
    fn event(state: &mut Self, _: &wl_buffer::WlBuffer, event: wl_buffer::Event, &(id, held): &(WindowId, Held), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_buffer::Event::Release = event {
            match held {
                Held::Shm(key) => {
                    if let Some(buffer) = state.windows.get_mut(&id).and_then(|w| w.shm.iter_mut().find(|b| b.key == key)) {
                        buffer.busy = false;
                    }
                }
                Held::Placeholder => {}
                Held::Foreign(key) => state.released.push((id, key)),
            }
        }
    }
}

macro_rules! ignore {
    ($($interface:path => $data:ty),* $(,)?) => {
        $(impl Dispatch<$interface, $data> for Wire {
            fn event(_: &mut Self, _: &$interface, _: <$interface as Proxy>::Event, _: &$data, _: &Connection, _: &QueueHandle<Self>) {}
        })*
    };
}

ignore! {
    wl_shm::WlShm => (),
    wl_shm_pool::WlShmPool => (),
    wl_compositor::WlCompositor => (),
    wl_surface::WlSurface => (),
    zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1 => (),
    zwp_linux_buffer_params_v1::ZwpLinuxBufferParamsV1 => (),
    wp_viewporter::WpViewporter => (),
    wp_viewport::WpViewport => (),
}

impl Dispatch<wl_seat::WlSeat, ()> for Wire {
    fn event(_: &mut Self, seat: &wl_seat::WlSeat, event: wl_seat::Event, _: &(), _: &Connection, handle: &QueueHandle<Self>) {
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(capabilities) } = event {
            if capabilities.contains(wl_seat::Capability::Pointer) {
                seat.get_pointer(handle, ());
            }
            if capabilities.contains(wl_seat::Capability::Keyboard) {
                seat.get_keyboard(handle, ());
            }
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for Wire {
    fn event(state: &mut Self, _: &wl_pointer::WlPointer, event: wl_pointer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_pointer::Event::Enter { surface, surface_x, surface_y, serial, .. } => {
                state.serial = serial;
                state.pointer_on = state.ids.get(&surface).copied();
                if let Some(window) = state.pointer_on {
                    state.events.push(Event::PointerMoved { window, x: surface_x as f32, y: surface_y as f32 });
                }
            }
            wl_pointer::Event::Leave { .. } => {
                if let Some(window) = state.pointer_on.take() {
                    state.events.push(Event::PointerLeft { window });
                }
            }
            wl_pointer::Event::Motion { surface_x, surface_y, .. } => {
                if let Some(window) = state.pointer_on {
                    state.events.push(Event::PointerMoved { window, x: surface_x as f32, y: surface_y as f32 });
                }
            }
            wl_pointer::Event::Button { button, state: pressed, serial, .. } => {
                state.serial = serial;
                if matches!(pressed, WEnum::Value(wl_pointer::ButtonState::Pressed)) {
                    state.press_serial = serial;
                }
                if let Some(window) = state.pointer_on {
                    let pressed = matches!(pressed, WEnum::Value(wl_pointer::ButtonState::Pressed));
                    // Linux's own numbers for a mouse's buttons, which is what the wire carries.
                    let button = match button {
                        0x110 => Button::Left,
                        0x111 => Button::Right,
                        0x112 => Button::Middle,
                        other => Button::Other(other),
                    };
                    state.events.push(Event::Button { window, button, pressed });
                }
            }
            wl_pointer::Event::Axis { axis, value, .. } => {
                if let Some(window) = state.pointer_on {
                    let (dx, dy) = match axis {
                        WEnum::Value(wl_pointer::Axis::HorizontalScroll) => (value as f32, 0.0),
                        _ => (0.0, value as f32),
                    };
                    state.events.push(Event::Scroll { window, dx, dy });
                }
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_keyboard::WlKeyboard, ()> for Wire {
    fn event(state: &mut Self, _: &wl_keyboard::WlKeyboard, event: wl_keyboard::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_keyboard::Event::Keymap { format: WEnum::Value(wl_keyboard::KeymapFormat::XkbV1), fd, size } => {
                state.keyboard.keymap(fd, size);
            }
            wl_keyboard::Event::Enter { surface, serial, .. } => {
                state.serial = serial;
                state.keyboard_on = state.ids.get(&surface).copied();
                if let Some(window) = state.keyboard_on {
                    state.events.push(Event::Focus { window, focused: true });
                }
            }
            wl_keyboard::Event::Leave { .. } => {
                state.keyboard.let_go();
                if let Some(window) = state.keyboard_on.take() {
                    state.events.push(Event::Focus { window, focused: false });
                }
            }
            wl_keyboard::Event::Modifiers { mods_depressed, mods_latched, mods_locked, group, .. } => {
                state.keyboard.modifiers(mods_depressed, mods_latched, mods_locked, group);
            }
            wl_keyboard::Event::RepeatInfo { rate, delay } => state.keyboard.repeat_info(rate, delay),
            wl_keyboard::Event::Key { key, state: pressed, serial, .. } => {
                state.serial = serial;
                if let Some(window) = state.keyboard_on {
                    let pressed = matches!(pressed, WEnum::Value(wl_keyboard::KeyState::Pressed));
                    state.keyboard.key(key, pressed, window, &mut state.events);
                }
            }
            _ => {}
        }
    }
}

// --- the clipboard ---

ignore! {
    wl_data_device_manager::WlDataDeviceManager => (),
}

impl Dispatch<wl_data_device::WlDataDevice, ()> for Wire {
    fn event(state: &mut Self, _: &wl_data_device::WlDataDevice, event: wl_data_device::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wl_data_device::Event::DataOffer { id } => {
                state.offers.insert(id, Vec::new());
            }
            // Something dragged: from this application or another, over one of its windows.
            wl_data_device::Event::Enter { serial, surface, x, y, id } => {
                if let Some(old) = state.over.take() {
                    if let Some(offer) = old.offer.filter(|o| Some(o) != id.as_ref()) {
                        state.offers.remove(&offer);
                        offer.destroy();
                    }
                }
                let Some(window) = state.ids.get(&surface).copied() else { return };
                let forms = id.as_ref().and_then(|o| state.offers.get(o)).cloned().unwrap_or_default();
                let (x, y) = (x as f32, y as f32);
                state.over = Some(Over { offer: id, window, serial, x, y, dropped: false });
                let own = state.dragging.is_some();
                state.events.push(Event::DragEntered { window, x, y, forms, own });
            }
            wl_data_device::Event::Motion { x, y, .. } => {
                if let Some(over) = state.over.as_mut() {
                    over.x = x as f32;
                    over.y = y as f32;
                    state.events.push(Event::DragMoved { window: over.window, x: over.x, y: over.y });
                }
            }
            wl_data_device::Event::Leave => {
                if let Some(over) = state.over.take_if(|o| !o.dropped) {
                    if let Some(offer) = over.offer {
                        state.offers.remove(&offer);
                        offer.destroy();
                    }
                    state.events.push(Event::DragLeft { window: over.window });
                }
            }
            wl_data_device::Event::Drop => {
                if let Some(over) = state.over.as_mut() {
                    over.dropped = true;
                    let forms = over.offer.as_ref().and_then(|o| state.offers.get(o)).cloned().unwrap_or_default();
                    let own = state.dragging.is_some();
                    state.events.push(Event::Dropped { window: over.window, x: over.x, y: over.y, forms, own });
                }
            }
            wl_data_device::Event::Selection { id } => {
                if let Some(old) = state.selection.take() {
                    if Some(&old) != id.as_ref() {
                        state.offers.remove(&old);
                        old.destroy();
                    }
                }
                let generation = crate::clipboard::changed();
                let Some(offer) = id else { return };
                let forms = state.offers.get(&offer).cloned().unwrap_or_default();
                state.selection = Some(offer.clone());
                let Some(form) = TEXT_FORMS.iter().find(|f| forms.iter().any(|o| o.eq_ignore_ascii_case(f))) else { return };
                let mut fds = [0i32; 2];
                if unsafe { libc::pipe(fds.as_mut_ptr()) } != 0 {
                    return;
                }
                // SAFETY: both ends are new and ours alone.
                let (read, write) = unsafe { (std::fs::File::from_raw_fd(fds[0]), std::os::fd::OwnedFd::from_raw_fd(fds[1])) };
                offer.receive(form.to_string(), write.as_fd());
                drop(write);
                // Read on a thread: whoever holds the clipboard may be slow, and may even be this
                // application, which serves it from this very loop.
                std::thread::Builder::new()
                    .name("clipboard-read".into())
                    .spawn(move || {
                        use std::io::Read;
                        let mut bytes = Vec::new();
                        let _ = read.take(16 << 20).read_to_end(&mut bytes);
                        crate::clipboard::read(generation, String::from_utf8(bytes).ok());
                    })
                    .ok();
            }
            _ => {}
        }
    }

    event_created_child!(Wire, wl_data_device::WlDataDevice, [
        wl_data_device::EVT_DATA_OFFER_OPCODE => (wl_data_offer::WlDataOffer, ()),
    ]);
}

impl Dispatch<wl_data_offer::WlDataOffer, ()> for Wire {
    fn event(state: &mut Self, offer: &wl_data_offer::WlDataOffer, event: wl_data_offer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wl_data_offer::Event::Offer { mime_type } = event {
            state.offers.entry(offer.clone()).or_default().push(mime_type);
        }
    }
}

/// What was let go of over a window, to be read: here at once, or from another application.
pub(crate) enum DropRead {
    Here(Vec<u8>),
    Pipe(std::fs::File),
    Nothing,
}

impl Dispatch<wl_data_source::WlDataSource, std::sync::Arc<DragData>> for Wire {
    fn event(
        state: &mut Self,
        source: &wl_data_source::WlDataSource,
        event: wl_data_source::Event,
        data: &std::sync::Arc<DragData>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        let ours = state.dragging.as_ref().is_some_and(|d| d.source == *source);
        match event {
            wl_data_source::Event::Send { mime_type, fd } => {
                let Some(bytes) = data.forms.iter().find(|(form, _)| *form == mime_type).map(|(_, b)| b.clone()) else { return };
                std::thread::Builder::new()
                    .name("drag-write".into())
                    .spawn(move || {
                        use std::io::Write;
                        let mut file = std::fs::File::from(fd);
                        let _ = file.write_all(&bytes);
                    })
                    .ok();
            }
            wl_data_source::Event::Action { dnd_action } => {
                if let Some(dragging) = state.dragging.as_mut().filter(|_| ours) {
                    dragging.moving = matches!(dnd_action, WEnum::Value(a) if a.contains(wl_data_device_manager::DndAction::Move));
                }
            }
            wl_data_source::Event::DndFinished | wl_data_source::Event::Cancelled => {
                let taken = matches!(event, wl_data_source::Event::DndFinished);
                if ours {
                    if let Some(dragging) = state.dragging.take() {
                        if let Some((surface, buffer, _)) = dragging.icon {
                            surface.destroy();
                            buffer.destroy();
                        }
                        state.events.push(Event::DragEnded { taken, moved: taken && dragging.moving });
                    }
                }
                source.destroy();
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_data_source::WlDataSource, std::sync::Arc<String>> for Wire {
    fn event(
        state: &mut Self,
        source: &wl_data_source::WlDataSource,
        event: wl_data_source::Event,
        text: &std::sync::Arc<String>,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
        match event {
            wl_data_source::Event::Send { fd, .. } => {
                // Written on a thread: a reader that is slow to read must not stop this loop.
                let text = text.clone();
                std::thread::Builder::new()
                    .name("clipboard-write".into())
                    .spawn(move || {
                        use std::io::Write;
                        let mut file = std::fs::File::from(fd);
                        let _ = file.write_all(text.as_bytes());
                    })
                    .ok();
            }
            wl_data_source::Event::Cancelled => {
                if state.source.as_ref() == Some(source) {
                    state.source = None;
                }
                source.destroy();
            }
            _ => {}
        }
    }
}
