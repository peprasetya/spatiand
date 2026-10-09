//! The Android backend: the Deck's session, with the glasses' Android display for a screen and
//! the phone for a controller.
//!
//! **The Mac runs this file too** (`crates/spatiand-mac` compiles it by path), since a Mac is the
//! same case: an app hosts the compositor, hands it a window to draw into, and tells it what its
//! pointer and keys did. Everything that differs between the two is behind a name in `super` --
//! the window and its EGL surface, the controller, the glasses' USB, the pads, the recorder --
//! which each platform's module supplies. What is written below about the phone and the Beam Pro
//! is Android's half of those; `crates/spatiand-mac/src/mac/mod.rs` says what the Mac's are.
//!
//! This is `crates/spatiand/src/backend_drm.rs`'s frame loop, kept as close to it as the
//! hardware allows, so that what happens in the room is what happens on the Deck: the same
//! menus, the same pointer, the same windows with the same frames, moved, resized and pushed
//! away the same way. Where it differs, it is because the hardware does:
//!
//! * **The picture.** No DRM: the app hands over the glasses' display as an `ANativeWindow`,
//!   and the scene is drawn straight into it through Smithay's own renderer (`super::egl`).
//!   The glasses' display goes away and comes back double-width when they switch to 3D, so a
//!   new window is a rebuild, as a hotplug is on the Deck.
//! * **The controller.** The phone, made into the Deck's controller state
//!   (`super::phone::PhoneController`): the touch area is the left pad, where the phone points
//!   the right, the orange key STEAM and the button under the touch area `⋯`. Two fingers
//!   pinching zoom the window being pointed at.
//! * **The glasses.** Opened through the usbfs descriptor the app was given, not hidraw.
//! * **The sidecar.** None: the phone's touch area is the pad and nothing else
//!   (`super::panel`); the audio pickers and the buttons are the app's own views around it.
//! * **Not here:** XWayland, local applications, PipeWire, BlueZ, the volume keys and the
//!   screen backlight. Every window on the Beam Pro is a remote one.

use std::time::{Duration, Instant};

use glam::{DQuat, DVec3, Mat4, Vec3};
use smithay::backend::egl::{EGLContext, EGLDisplay, EGLSurface};
use smithay::backend::renderer::gles::{ffi, GlesRenderer};
use smithay::backend::renderer::{Bind, Frame, Renderer};
use smithay::reexports::calloop::EventLoop;
use smithay::reexports::wayland_server::Display;
use smithay::utils::Transform;

use spatiand_hmd::{DisplayMode, Hmd, HmdEvent};
use spatiand_render::ray::{ray_from_pad, PointerConfig};
use spatiand_render::{EyeSide, StereoConfig, TextRenderer};
use spatiand_shell::{DesktopPanels, HudAction, Mode, Shell, ShellEvent};
use spatiand_track::{AxisMap, HeadTracker, TrackerConfig};

use super::controller::{Controller, Typed};
use super::{remote_video, Shared};
use crate::calib::Calibration;
use crate::environment::Environments;
use crate::gl::upload_rgba;
use crate::input_map::intent_for;
use crate::pointer::{self, Aim, Drag, PointerState, Zone, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT};
use crate::scene::{eye_centre, Cursor, Scene};
use crate::{Runtime, Spatiand};

const PANEL_DISTANCE: f32 = 1.4;
/// Scroll distance for a full sweep of the left pad, in wl_pointer units. The Deck's.
const SCROLL_SCALE: f64 = 260.0;
/// How long the headset may stay silent before it is let go of.
const IMU_SILENCE_TIMEOUT: Duration = Duration::from_secs(3);
/// See `backend_drm`'s.
const POLL_STALL_FORGIVENESS: Duration = Duration::from_millis(500);
/// How often the phone's touch area is redrawn: it only needs to be there.
const PANEL_EVERY: Duration = Duration::from_secs(1);
/// What the display the glasses are on is refreshed at. The Beam Pro draws every display at the
/// phone's 60 Hz.
const REFRESH_MHZ: i32 = super::REFRESH_MHZ;

/// The pad as the wire carries it. See `spatiand_stream::Pad`.
fn pad_state(report: &spatiand_input::virtual_pad::Report) -> spatiand_stream::Pad {
    use spatiand_stream::Pad;
    let bit = |on: bool, mask: u8| if on { mask } else { 0 };
    Pad {
        buttons: report.buttons,
        dpad: bit(report.dpad_up, Pad::UP)
            | bit(report.dpad_down, Pad::DOWN)
            | bit(report.dpad_left, Pad::LEFT)
            | bit(report.dpad_right, Pad::RIGHT),
        left: report.left,
        right: report.right,
        triggers: (report.left_trigger, report.right_trigger),
        extra: report.extra,
    }
}

/// An `EGLSurface` on one of the app's windows, and which of its windows it is.
struct Output {
    surface: EGLSurface,
    generation: u64,
    size: (i32, i32),
}

/// Make a surface on whatever window `slot` holds now, if it holds one and it is new.
fn output_for(
    slot: &super::Slot,
    display: &EGLDisplay,
    context: &EGLContext,
    current: Option<Output>,
) -> Option<Output> {
    let generation = slot.generation.load(std::sync::atomic::Ordering::SeqCst);
    if let Some(current) = current {
        if current.generation == generation {
            return Some(current);
        }
        // Dropped here: the window it was made on is gone or replaced.
    }
    let window = slot.window.lock().unwrap();
    let result = window.as_ref().and_then(|window| {
        let size = window.size();
        let native = super::egl::surface_of(window);
        let pixel_format = context.pixel_format()?;
        match unsafe { EGLSurface::new(display, pixel_format, context.config_id(), native) } {
            Ok(surface) => Some(Output { surface, generation, size }),
            Err(e) => {
                log::warn!("could not draw into a window of {}x{}: {e}", size.0, size.1);
                None
            }
        }
    });
    slot.seen.store(generation, std::sync::atomic::Ordering::SeqCst);
    result
}

pub fn run(
    event_loop: &mut EventLoop<'static, Runtime>,
    display: &mut Display<Spatiand>,
    runtime: &mut Runtime,
    shared: &Shared,
) -> Result<(), Box<dyn std::error::Error>> {
    // --- renderer ---
    let egl_display = super::egl::open_display()?;
    let egl_context = EGLContext::new_with_config(
        &egl_display,
        smithay::backend::egl::context::GlAttributes {
            version: (3, 0),
            profile: None,
            debug: false,
            vsync: true,
        },
        smithay::backend::egl::context::PixelFormatRequirements::_8_bit(),
    )?;
    let mut renderer = unsafe { GlesRenderer::new(egl_context)? };
    log::info!("renderer ready");

    let mut hmd: Option<Box<dyn Hmd>> = None;
    let mut had_headset = false;
    let mut text = TextRenderer::new();
    let mut panel: Option<(u32, f32)> = None;
    let mut last_prompt = String::new();
    let mut last_status_update = Instant::now();
    let mut last_poll_attempt = Instant::now();
    let mut status_text = String::new();
    let mut controller = Controller::new();
    let mut controls = crate::controls::Controls::new();
    let mut controls_focus: Option<usize> = None;
    let mut gesture = spatiand_input::TwoPadGesture::new();
    let pointer_config = PointerConfig::default();
    let mut pointers = PointerState::default();
    let started = Instant::now();
    let mut right_was_down = false;
    let mut left_was_down = false;
    let mut left_scroll = spatiand_input::PadScroll::default();
    let mut prefs = crate::prefs::Prefs::load();
    super::adopt_paired_hosts(&mut prefs);
    let mut remotes = crate::remote::Remotes::start(&mut runtime.display_handle, &prefs);
    let mut typed_event: Option<ShellEvent> = None;
    let mut keyboard_for_shell = false;
    // The click plays into the session's own output here (see `crate::click`).
    let clicks = crate::click::Clicks::new();
    let mut keyboard = spatiand_shell::Keyboard {
        click: prefs.keyboard_click,
        ..Default::default()
    };
    let mut spatial_audio = crate::audio::Audio::new(prefs.spatial_audio, prefs.directness());
    type KeyboardResize = (f32, f64);
    let mut keyboard_resize: Option<KeyboardResize> = None;
    let mut keyboard_border_hot = false;
    use spatiand_shell::keyboard::Key;
    let mut keyboard_hover: Vec<&'static Key> = Vec::new();
    const PRESS_SHOWN: Duration = Duration::from_millis(130);
    let mut keyboard_struck: Vec<(&'static Key, Instant)> = Vec::new();
    let mut nav_repeat = crate::input_map::NavRepeat::default();
    let mut menu_keys = crate::input_map::MenuKeys::default();
    let mut pointed_events: Vec<ShellEvent> = Vec::new();
    let mut menu_click_was = false;
    let mut keyboard_reach: [Option<spatiand_render::ray::Hit>; 2] = [None, None];
    let mut drag_left_y: Option<f32> = None;
    // Where a finger was when an open menu last moved a row for it. See the menus above.
    let mut menu_swipe: Option<(f32, f32)> = None;
    super::record::publish_leftovers();
    // How far a finger travels on the touch area, in pad units (2 across), for a menu row.
    const MENU_SWIPE_STEP: f32 = 0.18;
    // Where a thumb was on a pad's touchpad last frame, to move the pointer by the difference.
    let mut pad_touch_last: Option<(f32, f32)> = None;
    // The pointer's travel for a thumb's across the whole touchpad, which is 2 wide: about
    // three quarters of the view, so a corner is a slide and a half away and a word is easy.
    const PAD_TOUCH_GAIN: f32 = 0.75;
    let mut panel_drawn = Instant::now() - PANEL_EVERY;

    // The launcher has only remote applications here: Android's own are not windows yet.
    // On the Mac, the Mac's own.
    let shell_apps: Vec<spatiand_shell::AppEntry> = super::local_apps();
    let panels = super::PANELS;
    let calibrated = spatiand_track::config::load_axes().is_some();
    let mut shell = Shell::new(shell_apps, panels, calibrated);
    shell.set_local_name(&crate::system::machine_name());
    // Where windows pinned to the view go and how big: the wearer's, from last time.
    crate::pip::adopt(&mut runtime.state, &prefs, &mut shell);
    let mut environments = Environments::discover();
    shell.set_environments(environments.entries(), environments.choice());
    let mut browser = crate::environment::Browser::new();
    let sky_image = environments.current();
    let mut sky_loader = crate::environment::SkyLoader::start();
    let mut scene = Scene::new(&mut renderer, &sky_image)?;
    drop(sky_image);
    let stored = spatiand_track::config::load_axes();
    let mut calibration: Option<Calibration> = None;
    let mut tracker = HeadTracker::new(stored.unwrap_or(AxisMap::XREAL_AIR), TrackerConfig::default());
    let mut sensor_memory = spatiand_track::SensorMemory::new();
    let mut awaiting_app_id: Vec<(usize, Instant)> = Vec::new();
    // Asked once per pair of glasses, when they are opened: the display then goes away and
    // comes back double-width, which is a rebuild.
    let mut stereo_asked = false;

    crate::dmabuf::advertise(&mut runtime.state, &renderer);
    runtime.state.set_screen_refresh(REFRESH_MHZ);
    let mut pose_channel: Option<crate::pose::Channel> = None;
    let frame_ns: i64 = 1_000_000_000_000i64 / REFRESH_MHZ as i64;

    let mut glasses: Option<Output> = None;
    let mut phone: Option<Output> = None;

    // --- the session ---
    //
    // One pass per glasses window. The Wayland state -- clients, windows, the layout -- lives
    // in `runtime` and outlives every pass, as it outlives a rebuild on the Deck.
    while shared.running.load(std::sync::atomic::Ordering::Relaxed) {
        // The old glasses let go of first, then any new ones opened: replugged, the app says
        // both at once, and the other way round closed the glasses just opened -- the world
        // came back as "Plug in your XR glasses".
        if shared.usb_gone.swap(false, std::sync::atomic::Ordering::SeqCst) && hmd.is_some() {
            log::info!("the glasses were let go of");
            if let Some(x) = hmd.as_mut() {
                let _ = x.set_display_mode(DisplayMode::Mono);
            }
            hmd = None;
        }
        // New glasses on the USB side: open them, in 60 Hz side-by-side.
        if let Some(opened) = super::take_glasses(shared, &mut hmd) {
            match opened {
                Ok(x) => {
                    let info = x.info().clone();
                    log::info!("headset: {}", info.name);
                    settle_axes(&info, stored, &mut tracker, &mut calibration);
                    sensor_memory.restore(&info.name, &mut tracker);
                    had_headset = true;
                    stereo_asked = false;
                    hmd = Some(x);
                    *shared.status.lock().unwrap() = format!("{} connected", info.name);
                }
                Err(e) => {
                    log::warn!("could not open the glasses: {e}");
                    *shared.status.lock().unwrap() = format!("could not open the glasses: {e}");
                }
            }
        }

        glasses = output_for(&shared.glasses, &egl_display, renderer.egl_context(), glasses.take());
        phone = output_for(&shared.phone, &egl_display, renderer.egl_context(), phone.take());
        let Some(output) = glasses.as_ref() else {
            // Nowhere to draw the world. Keep the clients answered and the panel drawn.
            if let Some(p) = phone.as_mut() {
                draw_phone(&mut renderer, p);
            }
            display.dispatch_clients(&mut runtime.state)?;
            display.flush_clients()?;
            event_loop.dispatch(Some(Duration::from_millis(50)), runtime)?;
            continue;
        };
        let generation = output.generation;
        let (w, h) = output.size;
        let on_glasses = hmd.is_some() && w >= h * 3;
        log::info!("presenting at {w}x{h}, stereo: {on_glasses}");

        // Side by side, once the glasses are open and the window is not already double-width.
        if let Some(x) = hmd.as_mut() {
            if !on_glasses && !stereo_asked {
                stereo_asked = true;
                match x.set_display_mode(DisplayMode::Stereo) {
                    Ok(m) => log::info!("headset display mode -> {m:?}; waiting for its display"),
                    Err(e) => log::warn!("could not switch to stereo ({e}); staying mono"),
                }
            }
        }

        let stereo = StereoConfig {
            h_fov_deg: hmd.as_ref().map(|x| x.info().h_fov_deg).unwrap_or(40.0),
            ipd_m: hmd.as_ref().map(|x| x.info().default_ipd_mm).unwrap_or(63.0) / 1000.0,
            per_eye: if on_glasses {
                (w as u32 / 2, h as u32)
            } else {
                (w as u32, h as u32)
            },
            ..Default::default()
        };

        let mut frames = 0u32;
        let mut last_report = Instant::now();
        let mut in_flight: Vec<smithay::wayland::presentation::PresentationFeedbackCallback> = Vec::new();
        let mut presented_seq: u64 = 0;
        let mut last_hands = crate::attention::Hands::default();
        let mut last_tick = Instant::now();
        let mut last_imu = Instant::now();
        let mut screenshot_owed = false;
        // A video being recorded of this window. Belongs to it: rebuilt, it stops and the file
        // is finished.
        let mut recorder: Option<super::record::Recorder> = None;
        shell.set_recording(false);

        while shared.running.load(std::sync::atomic::Ordering::Relaxed) {
            // A new window, a window taken away, or new glasses: go round again.
            if shared.glasses.generation.load(std::sync::atomic::Ordering::SeqCst) != generation
                || super::glasses_waiting(shared, hmd.is_some())
                || shared.usb_gone.load(std::sync::atomic::Ordering::SeqCst)
            {
                break;
            }
            let mut shell_events: Vec<ShellEvent> = std::mem::take(&mut pointed_events);
            if super::recentre_requested().swap(false, std::sync::atomic::Ordering::SeqCst) {
                shell_events.push(ShellEvent::Hud(HudAction::Recentre));
            }
            let mut pads: Option<spatiand_input::ControllerState> = None;

            let missing = if hmd.is_none() {
                Some(crate::waiting::Missing::Headset)
            } else {
                None
            };
            let two_handed: Option<spatiand_input::GestureDelta> = None;
            let mut screenshot = super::screenshot_requested().swap(false, std::sync::atomic::Ordering::SeqCst);
            let mut record_toggle = false;

            // --- input ---
            controller.poll(tracker.orientation());
            {
                let c = &controller;
                if missing.is_none() {
                    let pointing = c.state().right_pad.touched && !shell.menu_is_open();
                    for control in c.pressed() {
                        if pointing
                            && matches!(
                                control,
                                spatiand_input::Control::A
                                    | spatiand_input::Control::B
                                    | spatiand_input::Control::X
                            )
                            && !matches!(control, spatiand_input::Control::B)
                        {
                            continue;
                        }
                        if *control == spatiand_input::Control::B && calibration.is_some() {
                            match calibration.as_mut() {
                                Some(c) if c.is_finished() => calibration = None,
                                Some(c) => c.cancel(),
                                None => {}
                            }
                            continue;
                        }
                        if let Some(intent) = intent_for(*control) {
                            if intent == spatiand_shell::Intent::ToggleSwitcher {
                                shell.set_windows(runtime.state.open_windows());
                            }
                            if let Some(event) = shell.handle(intent) {
                                shell_events.push(event);
                            }
                        }
                    }
                    let held = shell
                        .menu_is_open()
                        .then(|| {
                            crate::input_map::held_direction(c.state().buttons)
                                .or(controls.pad_held_direction())
                        })
                        .flatten();
                    if let Some(intent) = nav_repeat.tick(held, Instant::now()) {
                        if let Some(event) = shell.handle(intent) {
                            shell_events.push(event);
                        }
                    }
                    // **A finger sliding on the touch area moves through an open menu**, a row
                    // for every step of travel, the way a phone's list follows the thumb: up
                    // reveals what is below. The pointer alone could reach only the rows in
                    // view, and a turned phone is a clumsy way to get to the bottom of the HUD.
                    use spatiand_shell::grid::Direction;
                    let pad = c.state().left_pad;
                    match (shell.menu_is_open() && pad.touched, menu_swipe) {
                        (true, Some((x0, y0))) => {
                            let (dx, dy) = (pad.x - x0, pad.y - y0);
                            let step = if dy.abs() >= dx.abs() && dy.abs() >= MENU_SWIPE_STEP {
                                Some(if dy > 0.0 { Direction::Down } else { Direction::Up })
                            } else if dx.abs() > dy.abs() && dx.abs() >= MENU_SWIPE_STEP {
                                Some(if dx > 0.0 { Direction::Left } else { Direction::Right })
                            } else {
                                None
                            };
                            if let Some(direction) = step {
                                menu_swipe = Some((pad.x, pad.y));
                                c.pulse(spatiand_input::HapticPad::Left, spatiand_input::Feel::Tick);
                                if let Some(event) = shell.handle(spatiand_shell::Intent::Navigate(direction)) {
                                    shell_events.push(event);
                                }
                            }
                        }
                        (true, None) => menu_swipe = Some((pad.x, pad.y)),
                        (false, _) => menu_swipe = None,
                    }
                    let input = *c.state();
                    let _two_handed = gesture.update(&input.left_pad, &input.right_pad);
                }
            }

            runtime.state.settle_keyboard_focus();
            runtime.state.fit_screen_to_windows();

            // --- the controller layout ---
            {
                let focused = runtime
                    .state
                    .space
                    .elements()
                    .find(|w| runtime.state.layout.is_focused(w))
                    .cloned()
                    .or_else(|| crate::pointer::room_window(&runtime.state));
                let id = focused.as_ref().and_then(|w| runtime.state.layout.id_of(w));
                if id != controls_focus {
                    controls_focus = id;
                    match focused.as_ref() {
                        Some(window) => {
                            let key = crate::controls::app_key(
                                runtime.state.pid_of(window),
                                runtime.state.app_id_of(window).as_deref(),
                            );
                            let name = runtime
                                .state
                                .title_of(window)
                                .unwrap_or_else(|| "this application".into());
                            controls.focus(key, &name);
                        }
                        None => controls.focus(spatiand_mapper::AppKey::App(String::new()), "the desktop"),
                    }
                }
            }
            let mut deck_input = Some(*controller.state());
            let snapshot = controls.gather(deck_input.as_ref());
            super::pads::settle();
            // **A PlayStation pad's touchpad moves the pointer as a mouse does**: by how far the
            // thumb slides, held at the edge of the view, and it is the phone's pointer -- one
            // pointer, whichever moved it last. Absolute, the whole pad was the whole view, and
            // a millimetre of thumb was a degree. After `gather`, so the layout still sees the
            // phone exactly as it is.
            let touch = controls.touchpad().filter(|t| t.touched || t.clicked);
            match touch {
                Some(touch) if touch.touched => {
                    if let Some((x, y)) = pad_touch_last {
                        let (px, py) = controller.nudge(
                            (touch.x - x) * PAD_TOUCH_GAIN,
                            (touch.y - y) * PAD_TOUCH_GAIN,
                        );
                        if let Some(state) = deck_input.as_mut() {
                            state.right_pad.x = px;
                            state.right_pad.y = py;
                            state.right_pad.touched = true;
                        }
                    }
                    pad_touch_last = Some((touch.x, touch.y));
                }
                _ => pad_touch_last = None,
            }
            if let (Some(touch), Some(state)) = (touch, deck_input.as_mut()) {
                // Pressing the pad clicks where the pointer is, as the phone's tap does.
                state.right_pad.clicked |= touch.clicked;
                state.right_pad.touched = true;
            }
            let layout_resting = shell.menu_is_open() || missing.is_some() || calibration.is_some();
            let delivery = controls.step(&snapshot, layout_resting);
            let focused_app_id = runtime
                .state
                .space
                .elements()
                .find(|w| runtime.state.layout.is_focused(w))
                .cloned()
                .or_else(|| crate::pointer::room_window(&runtime.state))
                .and_then(|w| runtime.state.app_id_of(&w));
            remotes.pad(focused_app_id.as_deref(), pad_state(&controls.report()));
            // The motors are the pad's; the app plays what is asked for on it.
            if let Some((strong, weak)) = controls.rumble().or_else(|| remotes.rumble()) {
                super::pads::set_rumble(strong, weak);
            }
            for (fired, intent) in [
                (delivery.guide, spatiand_shell::Intent::ToggleHud),
                (delivery.guide_held, spatiand_shell::Intent::ToggleLauncher),
            ] {
                if fired {
                    if let Some(event) = shell.handle(intent) {
                        shell_events.push(event);
                    }
                }
            }
            if shell.menu_is_open() && calibration.is_none() {
                for control in &delivery.menu_presses {
                    if let Some(event) = intent_for(*control).and_then(|i| shell.handle(i)) {
                        shell_events.push(event);
                    }
                }
            }
            for command in &delivery.commands {
                use spatiand_mapper::Command;
                let intent = match command {
                    Command::Hud => Some(spatiand_shell::Intent::ToggleHud),
                    Command::Launcher => Some(spatiand_shell::Intent::ToggleLauncher),
                    Command::Keyboard => {
                        keyboard.open = !keyboard.open;
                        None
                    }
                    Command::Screenshot => {
                        screenshot = true;
                        None
                    }
                    Command::Recentre => {
                        shell_events.push(ShellEvent::Hud(HudAction::Recentre));
                        None
                    }
                };
                if let Some(event) = intent.and_then(|i| shell.handle(i)) {
                    shell_events.push(event);
                }
            }
            for (code, pressed) in &delivery.keys {
                let now = started.elapsed().as_millis() as u32;
                send_key_state(&mut runtime.state, *code, *pressed, now);
            }
            if let Some(mut input) = deck_input {
                if !shell.menu_is_open() {
                    if !delivery.pointer_clicks[0] {
                        input.left_trigger = 0.0;
                    }
                    if !delivery.pointer_clicks[1] {
                        input.right_trigger = 0.0;
                    }
                    pads = Some(input);
                }
            }

            // --- what was typed on the phone ---
            //
            // Keys go to whatever has focus, as a keyboard's do on the Deck; while the shell is
            // asking for text -- a computer's address -- they are that text instead.
            for typed in controller.take_keys() {
                let now = started.elapsed().as_millis() as u32;
                if shell.wants_text() {
                    use spatiand_shell::keyboard as kb;
                    match typed {
                        Typed::Char(c) if c == '\n' => typed_event = typed_event.or(shell.type_enter()),
                        Typed::Char(c) => shell.type_text(&c.to_string()),
                        Typed::Key { code: kb::KEY_BACKSPACE, pressed: true } => shell.type_backspace(),
                        Typed::Key { code: kb::KEY_ENTER, pressed: true } => {
                            typed_event = typed_event.or(shell.type_enter())
                        }
                        Typed::Key { .. } => {}
                    }
                    continue;
                }
                // A menu that is open has the keys: arrows, Enter, Escape, and in the launcher
                // the letters that narrow it.
                match typed {
                    Typed::Key { code, pressed } if menu_keys.key(&mut shell, code, pressed, &mut pointed_events) => continue,
                    Typed::Char(c) if shell.searches() => {
                        shell.type_query(&c.to_string());
                        continue;
                    }
                    _ => {}
                }
                match typed {
                    Typed::Key { code, pressed } => send_key_state(&mut runtime.state, code, pressed, now),
                    Typed::Char(c) => {
                        if let Some((code, shift)) = super::keys::from_char(c) {
                            send_stroke(
                                &mut runtime.state,
                                spatiand_shell::keyboard::Stroke {
                                    code,
                                    shift,
                                    ctrl: false,
                                    alt: false,
                                },
                                now,
                            );
                        }
                    }
                }
            }

            shell_events.extend(typed_event.take());
            remotes.tick(&mut runtime.display_handle, &mut prefs, &mut shell);
            runtime.state.quiet_hosts = remotes.quiet();
            crate::clipboard::exchange(&mut remotes, &mut runtime.state);
            if shell.wants_text() && !keyboard.open {
                keyboard.open = true;
                keyboard_for_shell = true;
            } else if !shell.wants_text() && keyboard_for_shell {
                keyboard.open = false;
                keyboard_for_shell = false;
            }

            for event in shell_events {
                match event {
                    ShellEvent::LaunchRemote { host, app } => remotes.launch(&host, &app),
                    ShellEvent::PairHost(address) => remotes.pair(address),
                    ShellEvent::ConfirmPairing => remotes.confirm(),
                    ShellEvent::CancelPairing => remotes.cancel(),
                    ShellEvent::ForgetHost(address) => remotes.forget(&address, &mut prefs),
                    ShellEvent::Bluetooth(_) => {}
                    ShellEvent::ModeChanged(mode) => {
                        if mode == Mode::World {
                            scene.forget_anchor();
                        } else {
                            scene.anchor_menu(mode, tracker.recentred_orientation());
                        }
                    }
                    ShellEvent::Launch(app) => super::launch_local(&app),
                    ShellEvent::ChooseEnvironment(choice) => {
                        environments.select(choice);
                        sky_loader.load(&environments);
                    }
                    ShellEvent::ListDirectory(name) => {
                        if let Some(name) = name {
                            browser.enter(&name);
                        }
                        shell.show_directory(browser.label(), browser.entries());
                    }
                    ShellEvent::AddEnvironment(name) => {
                        let path = browser.resolve(&name);
                        let choice = environments.add(&path);
                        environments.select(choice);
                        sky_loader.load(&environments);
                    }
                    ShellEvent::FocusWindow(id) => {
                        let window = runtime
                            .state
                            .space
                            .elements()
                            .find(|w| runtime.state.layout.id_of(w) == Some(id))
                            .cloned();
                        if let Some(window) = window {
                            // A pinned window has no place in the room to be brought from.
                            let in_room = !runtime.state.layout.is_pinned(&window);
                            if let Some(placement) =
                                runtime.state.layout.get(&window).filter(|_| in_room)
                            {
                                let placement =
                                    crate::window::brought_here(placement, tracker.recentred_orientation());
                                runtime.state.layout.set(&window, placement);
                            }
                            runtime.state.show_window(&window);
                            runtime.state.focus_window(&window);
                        }
                    }
                    ShellEvent::CloseWindow(id) => {
                        let window = runtime
                            .state
                            .space
                            .elements()
                            .find(|w| runtime.state.layout.id_of(w) == Some(id))
                            .cloned();
                        if let Some(window) = window {
                            log::info!("closing {}", runtime.state.display_title(&window));
                            runtime.state.close_window(&window);
                        }
                    }
                    // Pinning moves a window from the room to the corner of the view, or back,
                    // and changes nothing about it running. The list stays open, refreshed.
                    ShellEvent::PinWindow { id, pinned } => {
                        let window = runtime
                            .state
                            .space
                            .elements()
                            .find(|w| runtime.state.layout.id_of(w) == Some(id))
                            .cloned();
                        if let Some(window) = window {
                            runtime.state.pin_window(&window, pinned);
                            shell.refresh_windows(runtime.state.open_windows());
                        }
                    }
                    // Hiding keeps the application running and its sound going; it only stops
                    // being drawn or pointed at. The list stays open, refreshed in place.
                    ShellEvent::HideWindow { id, hidden } => {
                        let window = runtime
                            .state
                            .space
                            .elements()
                            .find(|w| runtime.state.layout.id_of(w) == Some(id))
                            .cloned();
                        if let Some(window) = window {
                            log::info!(
                                "{} {}",
                                if hidden { "hiding" } else { "showing" },
                                runtime.state.display_title(&window)
                            );
                            if hidden {
                                runtime.state.hide_window(&window);
                            } else {
                                runtime.state.show_window(&window);
                            }
                            shell.refresh_windows(runtime.state.open_windows());
                        }
                    }
                    ShellEvent::Controller(intent) => {
                        use spatiand_mapper::editor::Input;
                        use spatiand_shell::grid::Direction;
                        let input = match intent {
                            spatiand_shell::ControllerIntent::Navigate(Direction::Up) => Input::Up,
                            spatiand_shell::ControllerIntent::Navigate(Direction::Down) => Input::Down,
                            spatiand_shell::ControllerIntent::Navigate(Direction::Left) => Input::Left,
                            spatiand_shell::ControllerIntent::Navigate(Direction::Right) => Input::Right,
                            spatiand_shell::ControllerIntent::Accept => Input::Accept,
                            spatiand_shell::ControllerIntent::Back => Input::Back,
                        };
                        if !controls.editor_input(input) && shell.close_controller().is_some() {
                            scene.forget_anchor();
                        }
                    }
                    ShellEvent::Hud(action) => match action {
                        HudAction::Recentre => {
                            let anchor = runtime
                                .state
                                .layout
                                .focused_placement()
                                .map(|p| (p.yaw, p.pitch))
                                .unwrap_or_else(|| {
                                    let e = tracker.euler_degrees();
                                    (e.yaw.to_radians(), e.pitch.to_radians())
                                });
                            tracker.recenter();
                            runtime.state.layout.rotate_all(-anchor.0, -anchor.1);
                            crate::xr::rotate_sky_anchor(&mut runtime.state, -anchor.0);
                            // And the phone aims where the head now faces, from the next frame.
                            controller.reaim();
                            log::info!("recentred; brought the room round by {:.0} deg", (-anchor.0).to_degrees());
                        }
                        HudAction::Calibrate => {
                            log::info!("restarting axis calibration from the HUD");
                            calibration = Some(Calibration::new());
                        }
                        HudAction::OpenEnvironments => {
                            environments.refresh();
                            shell.set_environments(environments.entries(), environments.choice());
                        }
                        HudAction::Screenshot => screenshot = true,
                        HudAction::Record => record_toggle = true,
                        HudAction::PipCorner => {
                            crate::pip::next_corner(&mut runtime.state, &mut prefs, &mut shell)
                        }
                        HudAction::PipSize => {
                            crate::pip::toggle_size(&mut runtime.state, &mut prefs, &mut shell)
                        }
                        HudAction::OpenSwitcher => shell.set_windows(runtime.state.open_windows()),
                        HudAction::ControllerLayout => controls.open_editor(),
                        HudAction::ToggleKeyboard => {
                            keyboard.open = !keyboard.open;
                            log::info!("keyboard {}", if keyboard.open { "shown" } else { "hidden" });
                        }
                        HudAction::OpenHosts | HudAction::OpenPinned => {}
                        HudAction::OpenBluetooth => {}
                        HudAction::ReturnToDesktop => super::return_to_desktop(),
                        HudAction::OpenSystemSettings(panel) => super::system_settings(panel),
                        HudAction::Dismiss => {}
                    },
                }
            }

            // --- the glasses ---
            let stall = last_poll_attempt.elapsed();
            if stall > POLL_STALL_FORGIVENESS {
                last_imu = (last_imu + stall).min(Instant::now());
            }
            last_poll_attempt = Instant::now();
            if hmd.is_some() && last_imu.elapsed() >= IMU_SILENCE_TIMEOUT {
                log::warn!("no IMU samples for {IMU_SILENCE_TIMEOUT:?}; letting the glasses go until they are plugged in again");
                hmd = None;
                *shared.status.lock().unwrap() = "the glasses went quiet; unplug and replug them".into();
                break;
            }
            sensor_memory.tick(&tracker);
            let mut lost = false;
            if let Some(x) = hmd.as_mut() {
                while let Ok(Some(event)) = x.poll(Duration::ZERO) {
                    match event {
                        HmdEvent::Imu(sample) => {
                            last_imu = Instant::now();
                            if let Some(c) = calibration.as_mut() {
                                c.feed(&sample);
                            }
                            tracker.integrate(&sample);
                            controls.glasses_gyro(tracker.axes().apply(sample.gyro) - tracker.gyro_bias());
                        }
                        HmdEvent::Button { button, pressed: true } => match button {
                            spatiand_hmd::HmdButton::BrightnessUp => controls.glasses_button(true),
                            spatiand_hmd::HmdButton::BrightnessDown => controls.glasses_button(false),
                            spatiand_hmd::HmdButton::Unknown(_) => {}
                        },
                        HmdEvent::Disconnected => {
                            log::warn!("headset disconnected");
                            lost = true;
                            break;
                        }
                        _ => {}
                    }
                }
            }
            if lost {
                hmd = None;
                *shared.status.lock().unwrap() = "the glasses were unplugged".into();
                break;
            }
            if let Some(c) = calibration.as_mut() {
                c.tick();
                if c.is_finished() {
                    if let Some(map) = c.result() {
                        let known = hmd
                            .as_ref()
                            .and_then(|h| h.info().sensor_axes)
                            .and_then(AxisMap::from_mounting);
                        match known {
                            Some(known) if known != map => log::warn!(
                                "calibration measured {} but these glasses are built {}; keeping the hardware answer",
                                map.summary(),
                                known.summary()
                            ),
                            Some(_) => log::info!("calibration agrees with the hardware: {}", map.summary()),
                            None => {
                                log::info!("adopting measured axes: {}", map.summary());
                                tracker.set_axes(map);
                                if let Some(h) = hmd.as_ref() {
                                    sensor_memory.restore(&h.info().name, &mut tracker);
                                }
                            }
                        }
                    }
                    if matches!(c.stage(), crate::calib::Stage::Done | crate::calib::Stage::Cancelled) {
                        calibration = None;
                    }
                }
            }

            runtime.state.spawn_yaw = tracker.euler_degrees().yaw.to_radians();
            {
                let view_yaw = runtime.state.spawn_yaw;
                let locked: Vec<smithay::desktop::Window> = runtime
                    .state
                    .space
                    .elements()
                    .filter(|w| {
                        use smithay::wayland::seat::WaylandFocus;
                        w.wl_surface()
                            .map(|s| crate::xr::state_of(&s).layer == crate::xr::Layer::HeadLocked)
                            .unwrap_or(false)
                    })
                    .cloned()
                    .collect();
                for window in locked {
                    if let Some(mut placement) = runtime.state.layout.get(&window) {
                        placement.yaw = view_yaw;
                        placement.pitch = 0.0;
                        runtime.state.layout.set(&window, placement);
                    }
                }
            }

            // Two frames ahead: this one through Android's compositor on its next tick, then
            // scanout -- the Beam Pro's latency, measured as smoothest.
            let orientation = tracker.predicted_orientation(super::PREDICT_AHEAD_S, spatiand_track::DEFAULT_PREDICTION_MAX_DEGREES);

            {
                let dt = last_tick.elapsed();
                last_tick = Instant::now();
                runtime.state.attention.tick(hmd.is_some(), dt);
            }
            crate::window::apply_resize_anchors(&mut runtime.state);

            if !runtime.state.pose_clients.is_empty() {
                if hmd.is_none() {
                    for client in runtime.state.pose_clients.drain(..) {
                        client.unavailable("no headset is being tracked in this session".into());
                    }
                } else {
                    if pose_channel.is_none() {
                        match crate::pose::Channel::new() {
                            Ok(channel) => pose_channel = Some(channel),
                            Err(e) => log::warn!("could not make a pose channel: {e}"),
                        }
                    }
                    match pose_channel.as_ref() {
                        Some(channel) => {
                            for client in runtime.state.pose_clients.drain(..) {
                                client.channel(channel.fd(), channel.size());
                            }
                        }
                        None => {
                            for client in runtime.state.pose_clients.drain(..) {
                                client.unavailable("the pose channel could not be created".into());
                            }
                        }
                    }
                }
                runtime.state.pose_channels_to_open = false;
            }
            let sample = crate::pose::now_ns();
            let slot = crate::pose::wire_slot(orientation, DVec3::ZERO, &stereo, sample, sample + frame_ns);
            if let Some(channel) = pose_channel.as_mut() {
                channel.write_slot(slot);
            }
            if hmd.is_some() {
                remotes.viewport(&slot, (stereo.per_eye.0 * 2, stereo.per_eye.1));
            }
            let prompt_text = match calibration.as_ref() {
                _ if missing.is_some() => crate::waiting::message(
                    missing.unwrap_or(crate::waiting::Missing::Headset),
                    had_headset,
                    super::PLUG_HINT.into(),
                ),
                Some(c) => {
                    let p = c.prompt();
                    format!("{}\n\n{}\n\n{}", p.heading, p.body, p.status)
                }
                None => String::new(),
            };
            let waiting = missing.is_some();

            let ppd = TextRenderer::px_per_degree(stereo.per_eye.0, stereo.h_fov_deg);
            if prompt_text.is_empty() {
                if let Some((id, _)) = panel.take() {
                    renderer.with_context(|gl| unsafe { gl.DeleteTextures(1, &id) })?;
                }
                last_prompt.clear();
            } else if prompt_text != last_prompt {
                last_prompt = prompt_text.clone();
                let image = text.render(&prompt_text, ppd * 1.6, stereo.per_eye.0.saturating_sub(120).max(64), [235, 240, 255, 255]);
                let old = panel.take();
                panel = Some(renderer.with_context(|gl| unsafe {
                    if let Some((id, _)) = old {
                        gl.DeleteTextures(1, &id);
                    }
                    (upload_rgba(gl, &image), image.width as f32 / image.height.max(1) as f32)
                })?);
            }

            if let Some(image) = sky_loader.take() {
                renderer.with_context(|gl| unsafe { scene.set_sky(gl, &image) })?;
                log::info!("environment now {}", environments.describe());
            }
            if last_status_update.elapsed() >= Duration::from_secs(1) || status_text.is_empty() {
                status_text = crate::status::line();
                last_status_update = Instant::now();
            }
            scene.sync_status(&mut renderer, &mut text, &status_text, ppd)?;
            if keyboard.open {
                scene.sync_keyboard(&mut renderer, &mut text, &keyboard, ppd)?;
                keyboard_struck.retain(|(_, when)| when.elapsed() < PRESS_SHOWN);
                for key in &keyboard_hover {
                    scene.sync_key_cap(&mut renderer, &mut text, &keyboard, key, crate::keyboard_face::Lift::Hover)?;
                }
                for (key, _) in &keyboard_struck {
                    scene.sync_key_cap(&mut renderer, &mut text, &keyboard, key, crate::keyboard_face::Lift::Press)?;
                }
            }
            scene.sync_apps(&mut renderer, &mut text, &shell, ppd)?;
            if shell.mode() != Mode::Controller && controls.editor_open() {
                controls.close_editor();
            }
            let editor_view = if shell.mode() == Mode::Controller { controls.editor_view() } else { None };
            if shell.mode() == Mode::Controller && editor_view.is_none() && shell.close_controller().is_some() {
                scene.forget_anchor();
            }
            scene.set_compact_card(editor_view.is_some());
            let menu_model = match editor_view.as_ref() {
                Some(view) => Some(crate::menu::from_editor(view)),
                None => crate::menu::model(&shell),
            };
            scene.sync_menu(&mut renderer, &mut text, menu_model.as_ref(), ppd, (stereo.h_fov_deg, stereo.v_fov_deg()))?;
            scene.sync_diagram(
                &mut renderer,
                &mut text,
                editor_view.as_ref().map(|v| v.callouts.as_slice()).unwrap_or(&[]),
            )?;
            scene.sync_radial(&mut renderer, &mut text, delivery.radial.as_ref(), ppd)?;

            crate::dmabuf::settle(&mut runtime.state, &mut renderer);
            let sky_from_client = crate::scene::sky_surface(&mut renderer, &runtime.state);
            scene.set_sky_override(sky_from_client);
            let projection = crate::scene::projection_surface(&mut renderer, &runtime.state, crate::pose::eye_fovs(&stereo));
            scene.set_projection(projection);
            pointers.set_room(crate::pointer::room(&runtime.state, orientation, crate::pose::eye_fovs(&stereo)[0]));
            // Where pinned windows go is worked out from this frame's head, inside the collect.
            runtime.state.pip_head = Some((orientation, crate::pip::view_of(&stereo)));
            let mut windows = crate::scene::collect_windows(&mut renderer, &runtime.state);
            // A window that has just shown its first picture is given its size in the room,
            // where the platform has a view on that: a Mac's own windows.
            super::size_new_windows(&mut runtime.state, &mut windows);
            for quad in windows.iter_mut() {
                let title = runtime.state.display_title(&quad.window);
                quad.title = scene.title_texture(&mut renderer, &mut text, &title, ppd);
                if let Some(app_id) = runtime.state.app_id_of(&quad.window) {
                    quad.icon = scene.window_icon(&mut renderer, &app_id);
                }
            }

            if shell.mode() == Mode::Switcher
                && !(runtime.state.arrived_windows.is_empty() && runtime.state.departed_windows.is_empty())
            {
                shell.refresh_windows(runtime.state.open_windows());
            }
            let mut sinks_changed = false;
            for (id, pid, app_id) in std::mem::take(&mut runtime.state.arrived_windows) {
                match app_id {
                    Some(app_id) if app_id.starts_with("remote.") => {
                        spatial_audio.adopt_keyed(id, &app_id, crate::remote::sound_width(&app_id));
                        sinks_changed = true;
                    }
                    Some(_) => spatial_audio.adopt(id, pid),
                    None => {
                        spatial_audio.adopt(id, pid);
                        awaiting_app_id.push((id, Instant::now()));
                    }
                }
            }
            awaiting_app_id.retain(|(id, since)| match runtime.state.app_id_of_id(*id) {
                Some(app_id) => {
                    if app_id.starts_with("remote.") {
                        spatial_audio.adopt_keyed(*id, &app_id, crate::remote::sound_width(&app_id));
                        sinks_changed = true;
                    }
                    false
                }
                None => since.elapsed() < Duration::from_secs(10),
            });
            for id in std::mem::take(&mut runtime.state.departed_windows) {
                awaiting_app_id.retain(|(w, _)| *w != id);
                spatial_audio.forget(id);
                sinks_changed = true;
            }
            if sinks_changed {
                crate::remote::sound::set_sinks(spatial_audio.keyed_sinks());
            }

            // Where every window's sound is, now, and what it is doing. The head has moved
            // since the last frame even if nothing else has, so the aim is unconditional --
            // and cheap when the answer has not changed, because the renderer only fetches
            // new filters once a direction has moved further than anyone can hear.
            if spatial_audio.is_on() {
                let head = tracker.orientation();
                // Every window that could be what an app's sound is coming from -- including
                // the one that has become the room, which has no quad to be found among and
                // so was silently left out of this for as long as environments have existed.
                // Its sound stopped being pointed the moment it took the room, which meant it
                // stopped counter-rotating with the head as well.
                let mut sources: Vec<crate::audio::Source> = Vec::new();
                for window in runtime.state.space.elements() {
                    use smithay::wayland::seat::WaylandFocus;
                    let (Some(id), Some(surface)) =
                        (runtime.state.layout.id_of(window), window.wl_surface())
                    else {
                        continue;
                    };
                    let xr = crate::xr::state_of(&surface);
                    let kind = if xr.is_environment() {
                        crate::audio::Kind::Environment {
                            yaw: xr.sky_yaw_urad() as f64 * 1e-6,
                        }
                    } else if runtime.state.layout.is_pinned(window) {
                        crate::audio::Kind::glass(head)
                    } else if let Some(placement) = runtime.state.layout.get(window) {
                        crate::audio::Kind::Window(placement)
                    } else {
                        continue;
                    };
                    // The window's own picture, which is what decides whose sound this is;
                    // see `audio::Source::rank`.
                    let pixels = smithay::backend::renderer::utils::with_renderer_surface_state(
                        &surface,
                        |s| s.surface_size(),
                    )
                    .flatten()
                    .map(|size| (size.w.max(0) as u32, size.h.max(0) as u32))
                    .unwrap_or((0, 0));
                    sources.push(crate::audio::Source {
                        window: id,
                        pixels,
                        kind,
                        focused: runtime.state.layout.is_focused(window),
                        launcher: crate::audio::is_launcher(runtime.state.app_id_of(window).as_deref()),
                    });
                }
                spatial_audio.aim_all(&sources, head);
                for quad in windows.iter_mut() {
                    let Some(id) = runtime.state.layout.id_of(&quad.window) else {
                        continue;
                    };
                    // A window only grows a speaker once it has actually made a sound, and
                    // keeps it from then on: one that vanished between tracks would be a
                    // control that moved out from under a thumb reaching for it.
                    if let Some(status) = spatial_audio.status(id) {
                        let ever = quad.sound.is_some() || status.sounding.is_some();
                        quad.sound = ever.then_some(crate::scene::WindowSound {
                            muted: status.muted,
                            sounding: status.sounding,
                            peak: status.peak,
                        });
                    }
                }
            }

            // --- pointing and clicking ---
            let time_ms = started.elapsed().as_millis() as u32;
            let origin = eye_centre(orientation, &stereo);
            let aim_of = |pad: &spatiand_input::Pad| -> Option<Aim> {
                pad.touched
                    .then(|| pointer::aim(ray_from_pad(pad.x, pad.y, orientation, origin, &pointer_config), &windows))
            };
            let right_aim = pads.as_ref().and_then(|p| aim_of(&p.right_pad));
            // The phone's touch area is a wheel and not a second pointer: where a thumb rests on
            // it says nothing about where in the room it means.
            let _left_aim: Option<Aim> = None;

            let menu_aim = (shell.menu_is_open() && !shell.wants_text())
                .then_some(deck_input.as_ref())
                .flatten()
                .filter(|p| p.right_pad.touched)
                .map(|p| {
                    let ray = ray_from_pad(p.right_pad.x, p.right_pad.y, orientation, origin, &pointer_config);
                    let target = scene.menu_target(&shell, &ray, (stereo.h_fov_deg, stereo.v_fov_deg()));
                    if let Some((crate::scene::MenuTarget::Row(index), _)) = target {
                        // A tick per row crossed, as on the Deck.
                        if shell.point(index) {
                            controller.pulse(spatiand_input::HapticPad::Right, spatiand_input::Feel::Tick);
                        }
                    }
                    let aim = pointer::Aim {
                        ray,
                        hit: target.and_then(|(_, hit)| hit).map(|hit| (usize::MAX, hit)),
                        popup: None,
                        on_title: false,
                        zone: None,
                    };
                    (aim, target.map(|(t, _)| t))
                });
            let menu_pointed = menu_aim.as_ref().and_then(|(_, target)| *target);
            let menu_aim = menu_aim.map(|(aim, _)| aim);
            let pad_clicked = deck_input.is_some_and(|p| p.right_pad.clicked);
            let pad_click = pad_clicked && !menu_click_was;
            menu_click_was = pad_clicked;
            if let Some(target) = menu_pointed {
                if pad_click {
                    controller.pulse(spatiand_input::HapticPad::Right, spatiand_input::Feel::Click);
                    let intent = match target {
                        crate::scene::MenuTarget::Row(_) => spatiand_shell::Intent::Accept,
                        crate::scene::MenuTarget::Back => spatiand_shell::Intent::Back,
                    };
                    if let Some(event) = shell.handle(intent) {
                        pointed_events.push(event);
                    }
                }
            }
            {
                let touched = |pad: &spatiand_input::Pad| pad.touched.then_some((pad.x, pad.y));
                let hands = crate::attention::Hands {
                    right: pads.as_ref().and_then(|p| touched(&p.right_pad)),
                    left: pads.as_ref().and_then(|p| touched(&p.left_pad)),
                    mouse: None,
                };
                if hands.moved_from(&last_hands) {
                    runtime.state.attention.stir();
                }
                last_hands = hands;
            }

            for aim in [right_aim.as_ref()].into_iter().flatten() {
                let Some((index, _)) = aim.hit else { continue };
                let Some(quad) = windows.get_mut(index) else { continue };
                // Over the window at all: a pinned window shows its buttons only then.
                quad.aimed = true;
                match aim.zone {
                    Some(Zone::Pin) => quad.pin_hot = true,
                    Some(Zone::Close) => quad.close_hot = true,
                    Some(Zone::Hide) => quad.hide_hot = true,
                    Some(Zone::Mute) => quad.mute_hot = true,
                    _ => {}
                }
            }

            if shell.menu_is_open() || pointers.drag.is_some() {
                left_scroll.forget();
            }

            // Two fingers pinching zoom the window being pointed at, or the focused one.
            let pinch = controller.take_pinch();
            if (pinch - 1.0).abs() > 1e-4 && !shell.menu_is_open() {
                let target = right_aim
                    .as_ref()
                    .and_then(|a| a.hit)
                    .and_then(|(i, _)| windows.get(i))
                    .map(|q| q.window.clone())
                    .or_else(|| {
                        runtime
                            .state
                            .space
                            .elements()
                            .find(|w| runtime.state.layout.is_focused(w))
                            .cloned()
                    });
                if let Some(window) = target {
                    if let Some(mut placement) = runtime.state.layout.get(&window) {
                        placement.width = (placement.width * pinch as f64).clamp(0.3, 4.0);
                        runtime.state.layout.set(&window, placement);
                    }
                }
            }

            if shell.menu_is_open() && !shell.wants_text() {
                pointers.release_all(&mut runtime.state, time_ms);
            } else {
                match pointers.drag {
                    Some(Drag::Move { ref window, yaw_offset, pitch_offset }) => {
                        if let Some(a) = right_aim.as_ref() {
                            let d = a.ray.direction;
                            if let Some(mut placement) = runtime.state.layout.get(window) {
                                placement.yaw = d.y.atan2(d.x) + yaw_offset;
                                placement.pitch = crate::window::clamp_pitch(d.z.clamp(-1.0, 1.0).asin() + pitch_offset);
                                // The finger sliding on the touch area while holding sets the
                                // distance, as the left thumb does on the Deck.
                                if let Some(p) = pads.as_ref() {
                                    if p.left_pad.touched {
                                        if let Some(previous) = drag_left_y {
                                            let delta = (p.left_pad.y - previous) as f64;
                                            placement.radius = (placement.radius + delta * 2.5).clamp(0.8, 8.0);
                                        }
                                        drag_left_y = Some(p.left_pad.y);
                                    } else {
                                        drag_left_y = None;
                                    }
                                }
                                runtime.state.layout.set(window, placement);
                            }
                        }
                    }
                    Some(Drag::Resize { ref window, .. }) => {
                        if let Some(a) = right_aim.as_ref() {
                            if let Some(out) = pointers.drag.as_ref().and_then(|d| d.resized(&a.ray)) {
                                runtime.state.layout.set(window, out.placement);
                                runtime
                                    .state
                                    .request_size(window, (out.pixels.0 as i32, out.pixels.1 as i32).into());
                            }
                        }
                    }
                    None => {
                        let gesturing = two_handed.map(|d| !d.is_negligible()).unwrap_or(false);
                        if !gesturing {
                            if let Some(a) = right_aim.as_ref() {
                                pointers.motion(&mut runtime.state, a, &windows, time_ms);
                            }
                            // Not while the button is held: the finger that holds it is on the
                            // same glass, and a drag that also scrolled would fight itself.
                            if let Some(p) = pads.as_ref().filter(|p| !p.right_pad.clicked) {
                                match left_scroll.update(&p.left_pad) {
                                    spatiand_input::Scroll::By { dx, dy } => pointers.scroll(
                                        &mut runtime.state,
                                        -(dx as f64) * SCROLL_SCALE,
                                        dy as f64 * SCROLL_SCALE,
                                        time_ms,
                                    ),
                                    spatiand_input::Scroll::Fling => pointers.scroll_fling(&mut runtime.state, time_ms),
                                    spatiand_input::Scroll::Idle => {}
                                }
                            }
                        }
                    }
                }

                // A mouse's wheel scrolls what the pointer is on, as the Deck's pad does.
                let (wheel_right, wheel_up) = controller.take_wheel();
                if (wheel_right, wheel_up) != (0.0, 0.0) && pointers.drag.is_none() {
                    pointers.scroll(&mut runtime.state, wheel_right as f64 * 15.0, -(wheel_up as f64) * 15.0, time_ms);
                }

                if !shell.menu_is_open()
                    && (delivery.motion != (0, 0) || !delivery.mouse_buttons.is_empty() || delivery.wheel != (0, 0))
                {
                    if let Some(quad) = windows.iter().find(|w| w.focused) {
                        let surface = quad.surface.clone();
                        let size = (quad.pixels.0 as f64, quad.pixels.1 as f64);
                        pointers.nudge(&mut runtime.state, &surface, size, delivery.motion.0 as f64, delivery.motion.1 as f64, time_ms);
                        for (code, pressed) in &delivery.mouse_buttons {
                            pointers.button(&mut runtime.state, *code, *pressed, time_ms);
                        }
                        pointers.scroll(&mut runtime.state, delivery.wheel.0 as f64 * 15.0, -(delivery.wheel.1 as f64) * 15.0, time_ms);
                    }
                }

                if let Some(p) = pads.as_ref() {
                    let right_click = p.right_pad.clicked || controller.just_pressed(spatiand_input::Control::RPadClick);
                    let left_click = p.left_pad.clicked || controller.just_pressed(spatiand_input::Control::LPadClick);

                    // Felt under the thumb, as the Deck's pads are: the press registered.
                    if right_click && !right_was_down {
                        controller.pulse(spatiand_input::HapticPad::Right, spatiand_input::Feel::Click);
                    }
                    if left_click && !left_was_down {
                        controller.pulse(spatiand_input::HapticPad::Left, spatiand_input::Feel::Click);
                    }

                    let keyboard_quad = keyboard.open.then(|| {
                        let focus = windows.iter().find(|w| w.focused);
                        let (centre, facing, width, height) = scene.keyboard_placement(
                            focus.map(|w| &w.placement),
                            focus.map(|w| w.pixels).unwrap_or((16, 9)),
                            orientation,
                            (stereo.h_fov_deg, stereo.v_fov_deg()),
                            keyboard.scale,
                        );
                        spatiand_render::Quad {
                            centre: centre.as_dvec3(),
                            orientation: facing.as_dquat(),
                            width: width as f64,
                            height: height as f64,
                            bend: None,
                        }
                    });

                    keyboard_hover.clear();
                    keyboard_reach = [None, None];
                    keyboard_border_hot = keyboard_resize.is_some();
                    if let Some(q) = keyboard_quad.as_ref() {
                        if let Some(aim) = right_aim.as_ref() {
                            if let Some(hit) = spatiand_render::intersect_quad(&aim.ray, q) {
                                keyboard_reach[0] = Some(hit);
                                match keyboard.target_at(hit.u, hit.v) {
                                    Some(spatiand_shell::keyboard::Target::Key(k)) => keyboard_hover.push(k),
                                    Some(spatiand_shell::keyboard::Target::Border) => keyboard_border_hot = true,
                                    _ => {}
                                }
                            }
                        }
                    }

                    if let (Some(start), Some(q)) = (keyboard_resize, keyboard_quad.as_ref()) {
                        if right_click {
                            if let Some(a) = right_aim.as_ref() {
                                if let Some(hit) = spatiand_render::ray::intersect_plane(&a.ray, q) {
                                    let now = span(hit.u, hit.v);
                                    if start.1 > 1e-4 {
                                        keyboard.scale = (start.0 * (now / start.1) as f32).clamp(
                                            spatiand_shell::keyboard::MIN_SCALE,
                                            spatiand_shell::keyboard::MAX_SCALE,
                                        );
                                    }
                                }
                            }
                        } else {
                            log::info!("keyboard resized to {:.2}x", keyboard.scale);
                            keyboard_resize = None;
                        }
                    }

                    let mut typed = false;
                    if keyboard_resize.is_none() && right_click && !right_was_down {
                        if let (Some(a), Some(q)) = (right_aim.as_ref(), keyboard_quad.as_ref()) {
                            if let Some(hit) = spatiand_render::intersect_quad(&a.ray, q) {
                                match keyboard.target_at(hit.u, hit.v) {
                                    Some(spatiand_shell::keyboard::Target::Key(key)) => {
                                        typed = true;
                                        if keyboard.click {
                                            clicks.play();
                                        }
                                        if let Some(stroke) = keyboard.press(key) {
                                            if shell.wants_text() {
                                                typed_event = typed_event.or(type_into_shell(&mut shell, key, &stroke));
                                            } else if shell.searches() {
                                                typed_event = typed_event
                                                    .or(crate::input_map::stroke_in_launcher(&mut shell, key, &stroke));
                                            } else {
                                                send_stroke(&mut runtime.state, stroke, time_ms);
                                            }
                                        }
                                        keyboard.after_press(key);
                                        keyboard_struck.retain(|(k, _)| k.code != key.code);
                                        keyboard_struck.push((key, Instant::now()));
                                    }
                                    Some(spatiand_shell::keyboard::Target::SoundToggle) => {
                                        typed = true;
                                        let on = keyboard.toggle_click();
                                        prefs.keyboard_click = on;
                                        prefs.save();
                                        log::info!("keyboard click {}", if on { "on" } else { "off" });
                                        if on {
                                            clicks.play();
                                        }
                                    }
                                    Some(spatiand_shell::keyboard::Target::Border) => {
                                        typed = true;
                                        keyboard_resize = Some((keyboard.scale, span(hit.u, hit.v)));
                                    }
                                    None => {}
                                }
                            }
                        }
                    }

                    if !typed && !shell.wants_text() && right_click && pointers.drag.is_none() && !right_was_down {
                        if !right_aim.as_ref().map(|a| a.on_popup()).unwrap_or(false) {
                            pointers.dismiss_popups(&mut runtime.state);
                        }
                        match right_aim.as_ref() {
                            Some(a) if a.zone == Some(Zone::Mute) => {
                                if let Some(quad) = a.hit.and_then(|(i, _)| windows.get(i)) {
                                    if let Some(id) = runtime.state.layout.id_of(&quad.window) {
                                        let now = quad.sound.map(|s| s.muted).unwrap_or(false);
                                        spatial_audio.set_muted(id, !now);
                                    }
                                }
                            }
                            // Pinned to the glass or let go, whichever the window is not.
                            Some(a) if a.zone == Some(Zone::Pin) => {
                                if let Some(quad) = a.hit.and_then(|(i, _)| windows.get(i)) {
                                    let pinned = !quad.placement.pip;
                                    runtime.state.pin_window(&quad.window, pinned);
                                }
                            }
                            // Before the title bar, for the same reason as mute and close.
                            Some(a) if a.zone == Some(Zone::Hide) => {
                                if let Some(quad) = a.hit.and_then(|(i, _)| windows.get(i)) {
                                    log::info!(
                                        "hiding {}",
                                        runtime
                                            .state
                                            .title_of(&quad.window)
                                            .unwrap_or_else(|| "a window".into())
                                    );
                                    runtime.state.hide_window(&quad.window);
                                }
                            }
                            Some(a) if a.zone == Some(Zone::Close) => {
                                if let Some(quad) = a.hit.and_then(|(i, _)| windows.get(i)) {
                                    log::info!(
                                        "closing {}",
                                        runtime.state.title_of(&quad.window).unwrap_or_else(|| "a window".into())
                                    );
                                    runtime.state.close_window(&quad.window);
                                }
                            }
                            Some(a) if a.on_title => {
                                if let Some((index, _)) = a.hit {
                                    if let Some(quad) = windows.get(index) {
                                        runtime.state.focus_window(&quad.window);
                                        let d = a.ray.direction;
                                        let (ray_yaw, ray_pitch) = (d.y.atan2(d.x), d.z.clamp(-1.0, 1.0).asin());
                                        let (yaw_offset, pitch_offset) = runtime
                                            .state
                                            .layout
                                            .get(&quad.window)
                                            .map(|p| (p.yaw - ray_yaw, p.pitch - ray_pitch))
                                            .unwrap_or((0.0, 0.0));
                                        drag_left_y = None;
                                        pointers.drag = Some(Drag::Move {
                                            window: quad.window.clone(),
                                            yaw_offset,
                                            pitch_offset,
                                        });
                                    }
                                }
                            }
                            Some(a) if matches!(a.zone, Some(Zone::Resize(_))) => {
                                if let (Some((index, hit)), Some(Zone::Resize(edge))) = (a.hit, a.zone) {
                                    if let Some(quad) = windows.get(index) {
                                        runtime.state.focus_window(&quad.window);
                                        if let Some(placement) = runtime.state.layout.get(&quad.window) {
                                            pointers.drag = Some(Drag::Resize {
                                                window: quad.window.clone(),
                                                edge,
                                                start_quad: crate::pointer::quad_of(quad.pixels, &placement),
                                                start_u: hit.u,
                                                start_v: hit.v,
                                                start_placement: placement,
                                                start_pixels: quad.pixels,
                                            });
                                        }
                                    }
                                }
                            }
                            Some(a) => {
                                pointers.motion(&mut runtime.state, a, &windows, time_ms);
                                if let Some((index, _)) = a.hit {
                                    if let Some(quad) = windows.get(index) {
                                        runtime.state.focus_window(&quad.window);
                                    }
                                } else if keyboard_reach[0].is_none() && !a.on_popup() {
                                    if let Some(room) = crate::pointer::room_window(&runtime.state) {
                                        runtime.state.focus_window(&room);
                                    }
                                }
                                pointers.button(&mut runtime.state, BTN_LEFT, true, time_ms);
                            }
                            None => {}
                        }
                    } else if !right_click && right_was_down {
                        if matches!(pointers.drag, Some(Drag::Move { .. } | Drag::Resize { .. })) {
                            pointers.drag = None;
                        } else {
                            pointers.button(&mut runtime.state, BTN_LEFT, false, time_ms);
                        }
                    }

                    // A long press is the right button, where the phone points.
                    if left_click && !left_was_down {
                        if let Some(a) = right_aim.as_ref() {
                            if !a.on_popup() {
                                pointers.dismiss_popups(&mut runtime.state);
                            }
                            pointers.motion(&mut runtime.state, a, &windows, time_ms);
                            if let Some(quad) = a.hit.and_then(|(i, _)| windows.get(i)) {
                                runtime.state.focus_window(&quad.window);
                            } else if keyboard_reach.iter().all(Option::is_none) && !a.on_popup() {
                                if let Some(room) = crate::pointer::room_window(&runtime.state) {
                                    runtime.state.focus_window(&room);
                                }
                            }
                            pointers.button(&mut runtime.state, BTN_RIGHT, true, time_ms);
                        }
                    } else if !left_click && left_was_down {
                        pointers.button(&mut runtime.state, BTN_RIGHT, false, time_ms);
                    }
                    right_was_down = right_click;
                    left_was_down = left_click;
                    let _ = BTN_MIDDLE;
                }
            }

            // --- the recording ---
            if record_toggle {
                match recorder.take() {
                    Some(r) => {
                        r.stop(&mut renderer);
                        shell.set_recording(false);
                    }
                    None => match super::record::Recorder::start((w, h), &egl_display, renderer.egl_context()) {
                        Ok(r) => {
                            recorder = Some(r);
                            shell.set_recording(true);
                        }
                        Err(e) => log::warn!("could not start recording: {e}"),
                    },
                }
            }

            // --- draw, straight into the glasses' window ---
            let Some(output) = glasses.as_mut() else { break };
            {
                let scene = &scene;
                let shell = &shell;
                let windows = &windows;
                let right_aim = &right_aim;
                let menu_aim = &menu_aim;
                let keyboard_open = keyboard.open;
                let keyboard_state = &keyboard;
                let keyboard_hot = keyboard_border_hot;
                let keyboard_raised = &keyboard_hover;
                let pressed_now: Vec<&'static Key> = keyboard_struck.iter().map(|(key, _)| *key).collect();
                let keyboard_down = &pressed_now;
                let snapshot = panel;
                let drag_edge = match pointers.drag {
                    Some(Drag::Resize { edge, .. }) => Some(edge),
                    _ => None,
                };
                let room_draws_cursor = |ray: &spatiand_render::Ray| pointers.room_draws_cursor(ray);
                let record_now = recorder.as_mut().is_some_and(|r| r.due());
                let recorder_ref = &mut recorder;
                screenshot_owed |= screenshot;
                let take_shot = std::mem::take(&mut screenshot_owed);
                let mut shot: Option<Vec<u8>> = None;
                let mut target = renderer.bind(&mut output.surface)?;
                let mut frame = renderer.render(&mut target, (w, h).into(), Transform::Normal)?;
                frame.with_context(|gl| unsafe {
                    // The glasses' window is current here, which is when a driver that has to be
                    // told to wait for the display's refresh can be told.
                    super::egl::pace();
                    gl.BindFramebuffer(ffi::FRAMEBUFFER, 0);
                    gl.Disable(ffi::SCISSOR_TEST);
                    gl.ClearColor(0.02, 0.02, 0.05, 1.0);
                    gl.Viewport(0, 0, w, h);
                    gl.Clear(ffi::COLOR_BUFFER_BIT);
                    let views: &[(EyeSide, i32, i32)] = if on_glasses {
                        &[(EyeSide::Left, 0, w / 2), (EyeSide::Right, w / 2, w / 2)]
                    } else {
                        &[(EyeSide::Left, 0, w)]
                    };
                    // Straight into a window, which GL presents with its own convention: no flip,
                    // as the nested winit backend on the Deck.
                    for (side, x, vw) in views {
                        gl.Viewport(*x, 0, *vw, h);
                        let eye = spatiand_render::eye_for(*side, orientation, DVec3::ZERO, &stereo);
                        if !waiting {
                            scene.draw_sky(gl, &eye);
                            scene.draw_windows(gl, &eye, windows);
                        }
                        if keyboard_open {
                            let focus = windows.iter().find(|w| w.focused);
                            scene.draw_keyboard(
                                gl,
                                &eye,
                                focus.map(|w| &w.placement),
                                focus.map(|w| w.pixels).unwrap_or((16, 9)),
                                orientation,
                                (stereo.h_fov_deg, stereo.v_fov_deg()),
                                keyboard_state,
                                keyboard_hot,
                                keyboard_raised,
                                keyboard_down,
                            );
                        }
                        scene.draw_menu(gl, &eye, shell, (stereo.h_fov_deg, stereo.v_fov_deg()));
                        scene.draw_radial(gl, &eye, orientation);
                        if let Some(a) = right_aim.as_ref().or(menu_aim.as_ref()) {
                            if !(a.hit.is_none() && keyboard_reach[0].is_none() && room_draws_cursor(&a.ray)) {
                                let cursor = match (drag_edge, a.zone) {
                                    (Some(edge), _) => Cursor::for_edge(edge),
                                    (_, Some(Zone::Resize(edge))) => Cursor::for_edge(edge),
                                    _ => Cursor::Point,
                                };
                                scene.draw_pointer_ray(
                                    gl,
                                    &eye,
                                    orientation,
                                    &a.ray,
                                    nearer(a.hit.map(|(_, h)| h), keyboard_reach[0]),
                                    crate::scene::Pointing::RightThumb,
                                    cursor,
                                    1.0,
                                );
                            }
                        }
                        if let Some((tex, aspect)) = snapshot {
                            let (pw, ph) = fit_panel(aspect, stereo.h_fov_deg, stereo.v_fov_deg());
                            let model = head_locked_panel_sized(orientation, pw, ph);
                            scene.quads().draw(gl, tex, &(eye.view_projection() * model), [1.0, 1.0, 1.0, 1.0], (0.0, 1.0));
                        }
                    }
                    // The recording's copy of the frame, while it is still in the window.
                    if record_now {
                        if let Some(r) = recorder_ref.as_mut() {
                            r.copy(gl);
                        }
                    }
                    // What is on the glass, read back before it is presented.
                    if take_shot {
                        let mut pixels = vec![0u8; (w * h * 4) as usize];
                        gl.ReadPixels(0, 0, w, h, ffi::RGBA, ffi::UNSIGNED_BYTE, pixels.as_mut_ptr() as *mut _);
                        shot = Some(pixels);
                    }
                })?;
                // The swap below flushes; nothing here waits on the GPU.
                let _ = frame.finish()?;
                if let Some(pixels) = shot {
                    save_screenshot(pixels, w as u32, h as u32);
                }
            }
            // The swap waits for the display's next refresh: this is the frame clock.
            if let Err(e) = output.surface.swap_buffers(None) {
                log::warn!("the glasses' window would not present ({e:?}); rebuilding");
                glasses = None;
                break;
            }
            // The recording's frame, after the glasses have theirs.
            if let Some(r) = recorder.as_mut() {
                r.present(&mut renderer);
            }
            remote_video::end_frame(&mut renderer);

            // The phone's touch area, a few times a second.
            if panel_drawn.elapsed() >= PANEL_EVERY {
                panel_drawn = Instant::now();
                phone = output_for(&shared.phone, &egl_display, renderer.egl_context(), phone.take());
                if let Some(p) = phone.as_mut() {
                    draw_phone(&mut renderer, p);
                }
            }

            // Every callback that asked for this frame has now been shown.
            if !in_flight.is_empty() {
                let now = crate::pose::now_ns().max(0) as u64;
                let refresh = Duration::from_nanos(frame_ns.max(1) as u64);
                let screen = runtime.state.screen.clone();
                for feedback in in_flight.drain(..) {
                    feedback.presented(
                        &screen,
                        Duration::from_nanos(now),
                        smithay::wayland::presentation::Refresh::fixed(refresh),
                        presented_seq,
                        smithay::reexports::wayland_protocols::wp::presentation_time::server::wp_presentation_feedback::Kind::Vsync,
                    );
                }
            }
            presented_seq += 1;
            in_flight = runtime.state.take_presentation_feedback();

            frames += 1;
            if last_report.elapsed() >= Duration::from_secs(5) {
                let secs = last_report.elapsed().as_secs_f32();
                let window = runtime.state.cadence.take().map(|w| format!("; {w}")).unwrap_or_default();
                log::info!("presented {frames} frames in {secs:.1}s ({:.0} fps){window}", frames as f32 / secs);
                *shared.status.lock().unwrap() = format!(
                    "{}, {:.0} fps",
                    hmd.as_ref().map(|x| x.info().name.clone()).unwrap_or_else(|| "no glasses".into()),
                    frames as f32 / secs
                );
                frames = 0;
                last_report = Instant::now();
            }

            let screen = runtime.state.screen.clone();
            runtime.state.send_frames(&screen, Duration::ZERO);
            runtime.state.space.refresh();
            display.dispatch_clients(&mut runtime.state)?;
            display.flush_clients()?;
            event_loop.dispatch(Some(Duration::ZERO), runtime)?;
        }
    }

    if let Some(x) = hmd.as_mut() {
        let _ = x.set_display_mode(DisplayMode::Mono);
    }
    Ok(())
}

/// Draw the phone's touch area.
fn draw_phone(renderer: &mut GlesRenderer, output: &mut Output) {
    let size = output.size;
    let drawn = (|| -> Result<(), Box<dyn std::error::Error>> {
        let mut target = renderer.bind(&mut output.surface)?;
        let mut frame = renderer.render(&mut target, size.into(), Transform::Normal)?;
        frame.with_context(|gl| unsafe {
            gl.BindFramebuffer(ffi::FRAMEBUFFER, 0);
            gl.Disable(ffi::SCISSOR_TEST);
            super::panel::draw(gl, (size.0 as u32, size.1 as u32));
        })?;
        let _ = frame.finish()?;
        Ok(())
    })();
    match drawn {
        // The panel is not what paces anything: never wait for its refresh.
        Ok(()) => {
            let _ = output.surface.swap_buffers(None);
        }
        Err(e) => log::debug!("the phone's panel could not be drawn: {e}"),
    }
}

/// See `backend_drm::settle_axes`: a headset that states its IMU mounting wins.
fn settle_axes(
    info: &spatiand_hmd::HmdInfo,
    stored: Option<AxisMap>,
    tracker: &mut HeadTracker,
    calibration: &mut Option<Calibration>,
) {
    if let Some(map) = info.sensor_axes.and_then(AxisMap::from_mounting) {
        log::info!("axes from {} hardware: {}", info.name, map.summary());
        tracker.set_axes(map);
        *calibration = None;
        return;
    }
    match stored {
        Some(map) => tracker.set_axes(map),
        None if calibration.is_none() => *calibration = Some(Calibration::new()),
        None => {}
    }
}

/// See `backend_drm::span`.
fn span(u: f64, v: f64) -> f64 {
    let (du, dv) = (u - 0.5, v - 0.5);
    (du * du + dv * dv).sqrt()
}

/// See `backend_drm::nearer`.
fn nearer(a: Option<spatiand_render::ray::Hit>, b: Option<spatiand_render::ray::Hit>) -> Option<spatiand_render::ray::Hit> {
    match (a, b) {
        (Some(a), Some(b)) => Some(if b.distance < a.distance { b } else { a }),
        (a, b) => a.or(b),
    }
}

/// See `backend_drm::type_into_shell`.
fn type_into_shell(
    shell: &mut Shell,
    key: &spatiand_shell::Key,
    stroke: &spatiand_shell::keyboard::Stroke,
) -> Option<ShellEvent> {
    use spatiand_shell::keyboard as kb;
    match stroke.code {
        kb::KEY_BACKSPACE => shell.type_backspace(),
        kb::KEY_ENTER => return shell.type_enter(),
        _ => {
            let face = key.face(stroke.shift);
            if face.chars().count() == 1 {
                shell.type_text(face);
            }
        }
    }
    None
}

/// See `backend_drm::send_stroke`.
fn send_stroke(state: &mut Spatiand, stroke: spatiand_shell::keyboard::Stroke, time_ms: u32) {
    use spatiand_shell::keyboard as kb;
    let mut held = Vec::new();
    if stroke.ctrl {
        held.push(kb::KEY_LEFTCTRL);
    }
    if stroke.alt {
        held.push(kb::KEY_LEFTALT);
    }
    if stroke.shift {
        held.push(kb::KEY_LEFTSHIFT);
    }
    for code in &held {
        send_key_state(state, *code, true, time_ms);
    }
    send_key_state(state, stroke.code, true, time_ms);
    send_key_state(state, stroke.code, false, time_ms);
    for code in held.iter().rev() {
        send_key_state(state, *code, false, time_ms);
    }
}

/// See `backend_drm::send_key_state`.
fn send_key_state(state: &mut Spatiand, evdev_code: u32, pressed: bool, time_ms: u32) {
    state.attention.stir();
    let Some(keyboard) = state.seat.get_keyboard() else { return };
    let code = smithay::input::keyboard::Keycode::new(evdev_code + 8);
    let key_state = if pressed {
        smithay::backend::input::KeyState::Pressed
    } else {
        smithay::backend::input::KeyState::Released
    };
    keyboard.input::<(), _>(
        state,
        code,
        key_state,
        smithay::utils::SERIAL_COUNTER.next_serial(),
        time_ms,
        |_, _, _| smithay::input::keyboard::FilterResult::Forward,
    );
}

/// See `backend_drm::fit_panel`; the glasses are never portrait.
fn fit_panel(aspect: f32, h_fov_deg: f64, v_fov_deg: f64) -> (f32, f32) {
    let usable = 0.68;
    let extent = |fov: f64| 2.0 * PANEL_DISTANCE * ((fov * usable / 2.0).to_radians().tan() as f32);
    let (max_w, max_h) = (extent(h_fov_deg), extent(v_fov_deg));
    let aspect = aspect.max(0.01);
    let width = max_w.min(max_h * aspect);
    (width, width / aspect)
}

/// See `backend_drm::head_locked_panel_sized`.
fn head_locked_panel_sized(orientation: DQuat, width: f32, height: f32) -> Mat4 {
    let cfg = StereoConfig::default();
    let centre = Vec3::new((cfg.neck_forward_m as f32) + PANEL_DISTANCE, 0.0, cfg.neck_up_m as f32);
    let basis = Mat4::from_cols(
        (-Vec3::Y * width).extend(0.0),
        (Vec3::Z * height).extend(0.0),
        Vec3::X.extend(0.0),
        centre.extend(1.0),
    );
    Mat4::from_quat(orientation.as_quat()) * basis
}

/// Save a frame read back from the glasses' window: see `backend_drm::capture`. A window is
/// read bottom row first, so the rows are turned the right way up on the way to the file, on a
/// thread of its own because encoding a PNG this size takes longer than a frame.
fn save_screenshot(pixels: Vec<u8>, width: u32, height: u32) {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let _ = std::thread::Builder::new().name("screenshot".into()).spawn(move || {
        let row = (width * 4) as usize;
        let flipped: Vec<u8> = pixels.chunks(row).rev().flatten().copied().collect();
        let dir = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| "/tmp".into())).join("screenshots");
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join(format!("spatiand-{stamp}.png"));
        match image::save_buffer(&path, &flipped, width, height, image::ColorType::Rgba8) {
            Ok(()) => {
                log::info!("screenshot saved to {}", path.display());
                // To Pictures/Spatiand, where the gallery finds it.
                super::record::publish(path.display().to_string());
            }
            Err(e) => log::warn!("could not save a screenshot: {e}"),
        }
    });
}
