//! The runtime's connection to whatever is showing its pictures.
//!
//! On the Deck that is Spatiand itself; on a host it is `spatiand-host`. Either way it is a
//! Wayland compositor that speaks `spatiand_xr_v1`, and this is a client of it. What the runtime
//! needs from the connection is three things:
//!
//!   * the **pose channel** -- shared memory the compositor writes the head into, which is all
//!     `xrLocateViews` and `xrWaitFrame` need;
//!   * a **window** -- an `xdg_toplevel` whose surface is declared a side-by-side `projection`
//!     layer, so the compositor draws the two halves of whatever is attached to it as the two
//!     eyes' views of the room;
//!   * a way to **attach a dmabuf** to that window, which is how a finished frame leaves the
//!     application's GPU memory without ever touching the CPU.
//!
//! One thread reads the connection (a Wayland client has to, or the compositor's pings go
//! unanswered and it decides the window has hung). Requests are made from whichever thread
//! needs them; proxies are safe to use that way.

use std::os::fd::{AsFd, OwnedFd};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use spatiand_proto::client::spatiand_xr_pose_channel_v1::{self, SpatiandXrPoseChannelV1};
use spatiand_proto::client::spatiand_xr_surface_v1::{self, EyeLayout, Layer, SpatiandXrSurfaceV1};
use spatiand_proto::client::spatiand_xr_v1::SpatiandXrV1;
use wayland_client::globals::{registry_queue_init, GlobalListContents};
use wayland_client::protocol::{wl_buffer, wl_compositor, wl_pointer, wl_registry, wl_seat, wl_surface};
use wayland_client::WEnum;
use wayland_client::{delegate_noop, Connection, Dispatch, EventQueue, Proxy, QueueHandle};
use wayland_protocols::wp::linux_dmabuf::zv1::client::{
    zwp_linux_buffer_params_v1::{self, ZwpLinuxBufferParamsV1},
    zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1,
};
use wayland_protocols::xdg::shell::client::{
    xdg_surface::{self, XdgSurface},
    xdg_toplevel::{self, XdgToplevel},
    xdg_wm_base::{self, XdgWmBase},
};

use crate::pose::{read_history, History};

/// The newest version of `spatiand_xr_v1` this was written against.
const XR_VERSION: u32 = 5;

/// Shared memory the compositor writes poses into, mapped read-only.
struct Mapping {
    memory: *const u8,
    size: usize,
}

// The mapping is read only through volatile reads; nothing else holds the pointer.
unsafe impl Send for Mapping {}
unsafe impl Sync for Mapping {}

impl Drop for Mapping {
    fn drop(&mut self) {
        // SAFETY: unmapping exactly what `map` mapped.
        unsafe { libc::munmap(self.memory as *mut libc::c_void, self.size) };
    }
}

fn map(fd: &OwnedFd, size: usize) -> Result<Mapping, String> {
    use std::os::fd::AsRawFd;
    // SAFETY: mapping a descriptor we hold, read-only, at the size the compositor stated.
    let memory = unsafe {
        libc::mmap(
            std::ptr::null_mut(),
            size,
            libc::PROT_READ,
            libc::MAP_SHARED,
            fd.as_raw_fd(),
            0,
        )
    };
    if memory == libc::MAP_FAILED {
        return Err(format!("mmap of the pose channel: {}", std::io::Error::last_os_error()));
    }
    Ok(Mapping {
        memory: memory as *const u8,
        size,
    })
}

/// Where the wearer's pointer is on the picture, as the compositor last said.
///
/// The compositor sends pointer motion to the surface under the ray in the surface's own pixels,
/// and for a room that is the left eye's half of the picture -- the half an application's own
/// interface is laid out in. It is the only pointer there is: a game that wants a laser from a
/// hand is given this one, as the hand's aim.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct PointerSample {
    /// The ray is on the picture.
    pub inside: bool,
    pub x: f64,
    pub y: f64,
    /// Left and right buttons.
    pub buttons: [bool; 2],
    /// When it last moved, or a button last changed, in the runtime's clock. Zero if never.
    pub active_ns: i64,
}

/// What the dispatching thread learns, for the others to read.
#[derive(Default)]
struct Shared {
    pointer: Mutex<PointerSample>,
    /// The size of the picture last shown, `width << 32 | height`, both eyes.
    picture: AtomicU64,
    has_pointer: AtomicBool,
    poses: Mutex<Option<Mapping>>,
    /// Why there are no poses, if the compositor said so.
    unavailable: Mutex<Option<String>>,
    configured: AtomicBool,
    closed: AtomicBool,
    /// Frames the compositor has finished with and the application may draw again.
    frames_done: AtomicU64,
    /// What the viewer wants one eye drawn at, `width << 32 | height`, zero until it has said.
    render_size: AtomicU64,
}

struct State {
    shared: Arc<Shared>,
    pointer: Option<wl_pointer::WlPointer>,
}

/// Which of the window's buffers a `wl_buffer` is, so its release can be told to the owner.
pub struct BufferTag {
    released: Arc<AtomicBool>,
}

/// A picture the compositor can be shown.
pub struct Buffer {
    wl: wl_buffer::WlBuffer,
    released: Arc<AtomicBool>,
}

impl Buffer {
    /// The compositor is no longer reading from this one.
    pub fn is_free(&self) -> bool {
        self.released.load(Ordering::Acquire)
    }
}

impl Drop for Buffer {
    fn drop(&mut self) {
        self.wl.destroy();
    }
}

/// One plane of a dmabuf.
pub struct Plane {
    pub fd: OwnedFd,
    pub offset: u32,
    pub stride: u32,
}

pub struct Link {
    conn: Connection,
    qh: QueueHandle<State>,
    shared: Arc<Shared>,
    surface: wl_surface::WlSurface,
    dmabuf: ZwpLinuxDmabufV1,
    // Held so the objects live as long as the window does.
    _toplevel: XdgToplevel,
    _xdg_surface: XdgSurface,
    _xr: SpatiandXrV1,
    _seat: Option<wl_seat::WlSeat>,
    xr_surface: SpatiandXrSurfaceV1,
    _pose_channel: SpatiandXrPoseChannelV1,
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Link {
    /// Connect to the compositor named by `WAYLAND_DISPLAY`, make the window and ask for poses.
    pub fn connect(title: &str) -> Result<Link, String> {
        let conn = Connection::connect_to_env().map_err(|e| format!("no Wayland compositor: {e}"))?;
        let (globals, mut queue): (_, EventQueue<State>) =
            registry_queue_init(&conn).map_err(|e| format!("reading the Wayland globals: {e}"))?;
        let qh = queue.handle();
        let shared = Arc::new(Shared::default());
        let mut state = State {
            shared: shared.clone(),
            pointer: None,
        };

        let compositor: wl_compositor::WlCompositor = globals
            .bind(&qh, 1..=4, ())
            .map_err(|e| format!("no wl_compositor: {e}"))?;
        let wm_base: XdgWmBase = globals
            .bind(&qh, 1..=3, ())
            .map_err(|e| format!("no xdg_wm_base: {e}"))?;
        let dmabuf: ZwpLinuxDmabufV1 = globals
            .bind(&qh, 3..=4, ())
            .map_err(|e| format!("no zwp_linux_dmabuf_v1: {e}"))?;
        let xr: SpatiandXrV1 = globals
            .bind(&qh, 1..=XR_VERSION, ())
            .map_err(|_| "this compositor does not speak spatiand_xr_v1".to_string())?;

        // A seat, if there is one: it is how the wearer's pointer reaches the application.
        let seat: Option<wl_seat::WlSeat> = globals.bind(&qh, 1..=5, ()).ok();

        let surface = compositor.create_surface(&qh, ());
        let xdg_surface = wm_base.get_xdg_surface(&surface, &qh, ());
        let toplevel = xdg_surface.get_toplevel(&qh, ());
        toplevel.set_title(title.to_string());
        toplevel.set_app_id("spatiand-openxr".to_string());

        let xr_surface = xr.get_xr_surface(&surface, &qh, ());
        xr_surface.set_eye_layout(EyeLayout::SideBySide);
        xr_surface.set_layer(Layer::Projection);
        let pose_channel = xr.get_pose_channel(&qh, ());
        // The application draws its own pointer -- a laser from each hand -- so the compositor's
        // reticle would be a second one, at the wrong depth. Where the pointer is still arrives as
        // ordinary motion.
        if xr.version() >= 4 && std::env::var("SPATIAND_OPENXR_CURSOR").as_deref() != Ok("compositor") {
            xr_surface.set_cursor_drawn(1);
        }

        // The first commit has no buffer: it asks the compositor to configure the window.
        surface.commit();
        let deadline = Instant::now() + Duration::from_secs(5);
        while !shared.configured.load(Ordering::Acquire) {
            if Instant::now() > deadline {
                return Err("the compositor never configured the window".into());
            }
            queue
                .roundtrip(&mut state)
                .map_err(|e| format!("Wayland roundtrip: {e}"))?;
        }

        let stop = Arc::new(AtomicBool::new(false));
        let thread = {
            let stop = stop.clone();
            let conn = conn.clone();
            std::thread::Builder::new()
                .name("spatiand-openxr-wayland".into())
                .spawn(move || dispatch(conn, queue, state, stop))
                .map_err(|e| format!("could not start the Wayland thread: {e}"))?
        };

        Ok(Link {
            conn,
            qh,
            shared,
            surface,
            dmabuf,
            _toplevel: toplevel,
            _xdg_surface: xdg_surface,
            _xr: xr,
            _seat: seat,
            xr_surface,
            _pose_channel: pose_channel,
            stop,
            thread: Some(thread),
        })
    }

    /// Why there are no poses, if the compositor has said there are none.
    pub fn unavailable(&self) -> Option<String> {
        self.shared.unavailable.lock().unwrap().clone()
    }

    /// Has the compositor sent the pose channel?
    pub fn has_poses(&self) -> bool {
        self.shared.poses.lock().unwrap().is_some()
    }

    /// The wearer closed the window, or the compositor went away.
    pub fn is_closed(&self) -> bool {
        self.shared.closed.load(Ordering::Acquire)
    }

    /// Everything in the pose ring, oldest first.
    pub fn history(&self) -> History {
        match self.shared.poses.lock().unwrap().as_ref() {
            // SAFETY: the mapping is `channel_size()` bytes as the compositor stated, and lives
            // as long as the guard.
            Some(m) if m.size >= spatiand_proto::pose::channel_size() => unsafe { read_history(m.memory) },
            _ => History::default(),
        }
    }

    /// How many samples the compositor has ever written. A change is a new frame.
    pub fn write_index(&self) -> u64 {
        use spatiand_proto::pose::Header;
        match self.shared.poses.lock().unwrap().as_ref() {
            // SAFETY: as above; the header is at offset zero.
            Some(m) => unsafe {
                std::ptr::addr_of!((*(m.memory as *const Header)).write_index).read_volatile()
            },
            None => 0,
        }
    }

    /// Where the wearer's pointer is on the picture.
    pub fn pointer(&self) -> PointerSample {
        *self.shared.pointer.lock().unwrap()
    }

    /// Whether the compositor has a pointer to give at all.
    pub fn has_pointer(&self) -> bool {
        self.shared.has_pointer.load(Ordering::Acquire)
    }

    /// The size of the picture last shown, both eyes side by side.
    pub fn picture_size(&self) -> Option<(u32, u32)> {
        unpack(self.shared.picture.load(Ordering::Acquire))
    }

    /// Make a `wl_buffer` from a dmabuf. `width` and `height` are the whole picture, both eyes.
    pub fn make_buffer(
        &self,
        width: u32,
        height: u32,
        fourcc: u32,
        modifier: u64,
        plane: Plane,
    ) -> Result<Buffer, String> {
        let released = Arc::new(AtomicBool::new(true));
        let params = self.dmabuf.create_params(&self.qh, ());
        params.add(
            plane.fd.as_fd(),
            0,
            plane.offset,
            plane.stride,
            (modifier >> 32) as u32,
            modifier as u32,
        );
        let wl = params.create_immed(
            width as i32,
            height as i32,
            fourcc,
            zwp_linux_buffer_params_v1::Flags::empty(),
            &self.qh,
            BufferTag {
                released: released.clone(),
            },
        );
        params.destroy();
        self.conn
            .flush()
            .map_err(|e| format!("could not send the buffer: {e}"))?;
        Ok(Buffer { wl, released })
    }

    /// Show this picture. It is not to be drawn into again until [`Buffer::is_free`].
    pub fn present(&self, buffer: &Buffer, width: u32, height: u32) {
        buffer.released.store(false, Ordering::Release);
        self.shared.picture.store(((width as u64) << 32) | height as u64, Ordering::Release);
        self.surface.attach(Some(&buffer.wl), 0, 0);
        self.surface.damage_buffer(0, 0, width as i32, height as i32);
        self.surface.commit();
        let _ = self.conn.flush();
    }

    /// Which head the next picture was drawn for, in the protocol's frame, so the compositor can turn
    /// it by how far the head has moved since. Only a compositor that is version 5 or later listens;
    /// to an older one it is not sent, which is the same as never having said.
    pub fn set_frame_pose(&self, token: u32, orientation: [f32; 4]) {
        if self.xr_surface.version() >= 5 {
            let [x, y, z, w] = orientation.map(|c| (c * 1e6).round() as i32);
            self.xr_surface.set_frame_pose(token, x, y, z, w);
        }
    }

    /// Say whether the picture is for the room (a projection layer) or a panel in it.
    pub fn set_layer(&self, projection: bool) {
        self.xr_surface
            .set_layer(if projection { Layer::Projection } else { Layer::Window });
        let _ = self.conn.flush();
    }

    /// What the viewer has asked one eye be drawn at, if it has. Only a viewer across a network
    /// has an opinion; the headset's own compositor does not send one.
    pub fn render_size(&self) -> Option<(u32, u32)> {
        unpack(self.shared.render_size.load(Ordering::Acquire))
    }

    pub fn frames_done(&self) -> u64 {
        self.shared.frames_done.load(Ordering::Acquire)
    }
}

impl Drop for Link {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        // Waking the thread out of its blocking read: a roundtrip-less request is enough.
        let _ = self.conn.flush();
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn dispatch(conn: Connection, mut queue: EventQueue<State>, mut state: State, stop: Arc<AtomicBool>) {
    use std::os::fd::AsRawFd;
    while !stop.load(Ordering::Acquire) {
        if queue.dispatch_pending(&mut state).is_err() {
            state.shared.closed.store(true, Ordering::Release);
            return;
        }
        let _ = conn.flush();
        let Some(guard) = queue.prepare_read() else {
            continue;
        };
        // Poll with a timeout so `stop` is noticed; a plain blocking read could not be woken.
        let mut fd = libc::pollfd {
            fd: guard.connection_fd().as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        };
        // SAFETY: a libc call on a struct we own.
        let ready = unsafe { libc::poll(&mut fd, 1, 100) };
        if ready > 0 {
            if guard.read().is_err() {
                state.shared.closed.store(true, Ordering::Release);
                return;
            }
        } else {
            drop(guard);
        }
    }
}

delegate_noop!(State: ignore wl_compositor::WlCompositor);
delegate_noop!(State: ignore wl_surface::WlSurface);
delegate_noop!(State: ignore ZwpLinuxDmabufV1);
delegate_noop!(State: ignore ZwpLinuxBufferParamsV1);
delegate_noop!(State: ignore SpatiandXrV1);

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for State {
    fn event(
        _: &mut Self,
        _: &wl_registry::WlRegistry,
        _: wl_registry::Event,
        _: &GlobalListContents,
        _: &Connection,
        _: &QueueHandle<Self>,
    ) {
    }
}

impl Dispatch<wl_seat::WlSeat, ()> for State {
    fn event(state: &mut Self, seat: &wl_seat::WlSeat, event: wl_seat::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_seat::Event::Capabilities { capabilities: WEnum::Value(caps) } = event {
            if caps.contains(wl_seat::Capability::Pointer) && state.pointer.is_none() {
                state.pointer = Some(seat.get_pointer(qh, ()));
                state.shared.has_pointer.store(true, Ordering::Release);
            }
        }
    }
}

impl Dispatch<wl_pointer::WlPointer, ()> for State {
    fn event(state: &mut Self, _: &wl_pointer::WlPointer, event: wl_pointer::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        let now = crate::time::now_ns();
        let mut p = state.shared.pointer.lock().unwrap();
        match event {
            wl_pointer::Event::Enter { surface_x, surface_y, .. } => {
                log::info!("the wearer's pointer is on the picture at ({surface_x:.0}, {surface_y:.0})");
                p.inside = true;
                p.x = surface_x;
                p.y = surface_y;
                p.active_ns = now;
            }
            wl_pointer::Event::Leave { .. } => {
                log::info!("the wearer's pointer left the picture");
                p.inside = false;
                p.buttons = [false; 2];
            }
            wl_pointer::Event::Motion { surface_x, surface_y, .. } => {
                if (p.x - surface_x).abs() + (p.y - surface_y).abs() > 0.5 {
                    p.active_ns = now;
                }
                p.inside = true;
                p.x = surface_x;
                p.y = surface_y;
            }
            wl_pointer::Event::Button { button, state: WEnum::Value(pressed), .. } => {
                // BTN_LEFT and BTN_RIGHT.
                let down = pressed == wl_pointer::ButtonState::Pressed;
                match button {
                    0x110 => p.buttons[0] = down,
                    0x111 => p.buttons[1] = down,
                    _ => {}
                }
                p.active_ns = now;
            }
            _ => {}
        }
    }
}

impl Dispatch<XdgWmBase, ()> for State {
    fn event(_: &mut Self, base: &XdgWmBase, event: xdg_wm_base::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            base.pong(serial);
        }
    }
}

impl Dispatch<XdgSurface, ()> for State {
    fn event(state: &mut Self, surface: &XdgSurface, event: xdg_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_surface::Event::Configure { serial } = event {
            surface.ack_configure(serial);
            state.shared.configured.store(true, Ordering::Release);
        }
    }
}

impl Dispatch<XdgToplevel, ()> for State {
    fn event(state: &mut Self, _: &XdgToplevel, event: xdg_toplevel::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_toplevel::Event::Close = event {
            state.shared.closed.store(true, Ordering::Release);
        }
    }
}

impl Dispatch<SpatiandXrSurfaceV1, ()> for State {
    fn event(_: &mut Self, _: &SpatiandXrSurfaceV1, event: spatiand_xr_surface_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        // A refused layer leaves the picture a window, which is still a valid thing to show.
        if let spatiand_xr_surface_v1::Event::LayerRefused { .. } = event {
            log::warn!("the compositor refused the projection layer; the picture will be a window");
        }
    }
}

impl Dispatch<SpatiandXrPoseChannelV1, ()> for State {
    fn event(state: &mut Self, _: &SpatiandXrPoseChannelV1, event: spatiand_xr_pose_channel_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            spatiand_xr_pose_channel_v1::Event::Channel { fd, size } => match map(&fd, size as usize) {
                Ok(mapping) => *state.shared.poses.lock().unwrap() = Some(mapping),
                Err(e) => log::warn!("{e}"),
            },
            spatiand_xr_pose_channel_v1::Event::RenderSize { width, height } => {
                state.shared.render_size.store(((width as u64) << 32) | height as u64, Ordering::Release);
            }
            spatiand_xr_pose_channel_v1::Event::Unavailable { reason } => {
                *state.shared.unavailable.lock().unwrap() = Some(reason);
            }
            _ => {}
        }
    }
}

impl Dispatch<wl_buffer::WlBuffer, BufferTag> for State {
    fn event(_: &mut Self, _: &wl_buffer::WlBuffer, event: wl_buffer::Event, tag: &BufferTag, _: &Connection, _: &QueueHandle<Self>) {
        if let wl_buffer::Event::Release = event {
            tag.released.store(true, Ordering::Release);
        }
    }
}

fn unpack(packed: u64) -> Option<(u32, u32)> {
    let (w, h) = ((packed >> 32) as u32, packed as u32);
    (w > 0 && h > 0).then_some((w, h))
}

/// Ask the compositor what size it wants an eye drawn at, without making a window.
///
/// Applications ask how big their swapchains should be before they make a session, and the answer
/// has to come from somewhere that has not yet got a connection. This opens one, asks for the pose
/// channel -- which is where the size is said -- waits briefly for it, and goes away.
pub fn probe_render_size() -> Option<(u32, u32)> {
    let conn = Connection::connect_to_env().ok()?;
    let (globals, mut queue): (_, EventQueue<State>) = registry_queue_init(&conn).ok()?;
    let qh = queue.handle();
    let shared = Arc::new(Shared::default());
    let mut state = State { shared: shared.clone(), pointer: None };
    let xr: SpatiandXrV1 = globals.bind(&qh, 1..=XR_VERSION, ()).ok()?;
    let _channel = xr.get_pose_channel(&qh, ());
    let deadline = Instant::now() + Duration::from_millis(400);
    let mut answered: Option<Instant> = None;
    while Instant::now() < deadline {
        queue.roundtrip(&mut state).ok()?;
        if let Some(size) = unpack(shared.render_size.load(Ordering::Acquire)) {
            return Some(size);
        }
        // The channel (or the news there is none) comes with the size or not at all, so a little
        // after it has come there is nothing more to wait for: this compositor has no opinion.
        if shared.unavailable.lock().unwrap().is_some() || shared.poses.lock().unwrap().is_some() {
            let since = *answered.get_or_insert_with(Instant::now);
            if since.elapsed() > Duration::from_millis(80) {
                return None;
            }
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    unpack(shared.render_size.load(Ordering::Acquire))
}
