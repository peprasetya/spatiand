//! The glasses' thread: owns the driver, feeds the tracker.
//!
//! The same `XrealGlasses` the Deck runs, reached through the usbfs descriptor Android hands
//! over instead of hidraw. It is the only thing that talks to the glasses, and the drawing
//! only ever reads the tracker it fills.

use std::os::fd::OwnedFd;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use spatiand_hmd::{DisplayMode, Hmd, HmdEvent, XrealGlasses};
use spatiand_track::{AxisMap, SensorMemory};

use super::Shared;

pub struct Glasses {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Glasses {
    pub fn start(fd: OwnedFd, shared: Arc<Shared>) -> Self {
        let stop = Arc::new(AtomicBool::new(false));
        let quit = stop.clone();
        let thread = std::thread::Builder::new()
            .name("glasses".into())
            .spawn(move || run(fd, &shared, &quit))
            .ok();
        Self { stop, thread }
    }
}

impl Drop for Glasses {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn say(shared: &Shared, line: String) {
    log::info!("{line}");
    *shared.glasses_state.lock().unwrap() = line;
}

fn run(fd: OwnedFd, shared: &Shared, stop: &AtomicBool) {
    let mut hmd = match XrealGlasses::open_usb(fd) {
        Ok(hmd) => hmd,
        Err(e) => {
            say(shared, format!("could not open the glasses: {e}"));
            return;
        }
    };
    let info = hmd.info().clone();
    let axes = info
        .sensor_axes
        .and_then(AxisMap::from_mounting)
        .unwrap_or(AxisMap::XREAL_AIR);
    // What was learned about these glasses last time -- the gyro's resting offset and the
    // magnetometer's own field -- after the axes, which a remembered calibration belongs to.
    // Without it every start begins by guessing the bias, and the world drifts until it has.
    let mut memory = SensorMemory::new();
    {
        let mut tracker = shared.tracker.lock().unwrap();
        tracker.set_axes(axes);
        memory.restore(&info.name, &mut tracker);
    }
    *shared.h_fov_deg.lock().unwrap() = info.h_fov_deg;

    // The Beam Pro composites every display on the phone screen's 60 Hz clock. A panel at the
    // glasses' preferred 72 would show one frame in five twice, which is a judder on every turn.
    hmd.prefer_refresh(60);

    // Side-by-side from the start. The glasses' display then goes away and comes back at
    // double width, which the app answers by putting its Presentation on the new one.
    let mode = match hmd.set_display_mode(DisplayMode::Stereo) {
        Ok(_) => "3D",
        Err(e) => {
            log::warn!("the glasses stayed in 2D: {e}");
            "2D"
        }
    };
    say(shared, format!("{} in {mode}", info.name));

    let mut samples = 0u32;
    let mut since = Instant::now();
    while !stop.load(Ordering::Relaxed) {
        match hmd.poll(Duration::from_millis(20)) {
            Ok(Some(HmdEvent::Imu(sample))) => {
                let mut tracker = shared.tracker.lock().unwrap();
                tracker.integrate(&sample);
                samples += 1;
                // Logs every half minute and saves at most once a minute; cheap otherwise.
                if samples % 100 == 0 {
                    memory.tick(&tracker);
                }
            }
            Ok(Some(HmdEvent::Disconnected)) => {
                say(shared, format!("{} unplugged", info.name));
                return;
            }
            Ok(Some(HmdEvent::DisplayModeChanged(mode))) => {
                log::info!("the glasses switched themselves to {mode:?}");
            }
            Ok(Some(HmdEvent::Button { button, pressed })) => {
                log::info!("glasses button {button:?} pressed={pressed}");
            }
            Ok(_) => {}
            Err(e) => {
                say(shared, format!("lost the glasses: {e}"));
                return;
            }
        }
        if since.elapsed() >= Duration::from_secs(1) {
            shared.imu_hz.store(samples, Ordering::Relaxed);
            samples = 0;
            since = Instant::now();
        }
    }
    // Dropping `hmd` puts the glasses back in 2D and stops the IMU.
}
