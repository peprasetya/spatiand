//! One HID interface of the glasses on the Mac, through IOKit.
//!
//! macOS has no hidraw node to open. An interface is an `IOHIDDevice`, which is written with a
//! call and read by a callback on a run loop -- neither of which is something to `poll(2)` on,
//! and the driver waits on its two interfaces with exactly that. So each interface gets a
//! socket pair: the callback puts every report it is handed into one end, and the driver's
//! [`Port`] is the other. Datagrams, so a report is a report however fast they come.
//!
//! Two things macOS does differently from Linux, both learned the hard way in HoloFrame:
//!
//! * interfaces are told apart by `bInterfaceNumber`, which is a property of the
//!   `IOUSBHostInterface` *ancestor* of the HID device, not of the device. The usage page does
//!   not tell them apart: the glasses' interfaces 3, 4 and 5 all say 0x0041.
//! * a plain `IOHIDDeviceOpen` is all it takes. No seizing, no Input Monitoring permission.

use std::ffi::{c_char, c_void, CString};
use std::os::fd::{AsRawFd, RawFd};
use std::os::unix::net::UnixDatagram;
use std::sync::mpsc;
use std::time::Duration;

use crate::port::Port;
use crate::{device, DeviceSpec, HmdError, Result};

type Ref = *const c_void;

type ReportCallback = unsafe extern "C" fn(
    context: *mut c_void,
    result: i32,
    sender: *mut c_void,
    kind: u32,
    id: u32,
    report: *mut u8,
    length: isize,
);
type RemovalCallback = unsafe extern "C" fn(context: *mut c_void, result: i32, sender: *mut c_void);

#[link(name = "IOKit", kind = "framework")]
extern "C" {
    fn IOHIDManagerCreate(allocator: Ref, options: u32) -> Ref;
    fn IOHIDManagerSetDeviceMatching(manager: Ref, matching: Ref);
    fn IOHIDManagerCopyDevices(manager: Ref) -> Ref;
    fn IOHIDDeviceOpen(device: Ref, options: u32) -> i32;
    fn IOHIDDeviceClose(device: Ref, options: u32) -> i32;
    fn IOHIDDeviceGetService(device: Ref) -> u32;
    fn IOHIDDeviceGetProperty(device: Ref, key: Ref) -> Ref;
    fn IOHIDDeviceSetReport(device: Ref, kind: u32, id: isize, report: *const u8, length: isize) -> i32;
    fn IOHIDDeviceRegisterInputReportCallback(
        device: Ref,
        report: *mut u8,
        length: isize,
        callback: Option<ReportCallback>,
        context: *mut c_void,
    );
    fn IOHIDDeviceRegisterRemovalCallback(device: Ref, callback: Option<RemovalCallback>, context: *mut c_void);
    fn IOHIDDeviceScheduleWithRunLoop(device: Ref, run_loop: Ref, mode: Ref);
    fn IORegistryEntryCreateCFProperty(entry: u32, key: Ref, allocator: Ref, options: u32) -> Ref;
    fn IORegistryEntryGetParentEntry(entry: u32, plane: *const c_char, parent: *mut u32) -> i32;
    fn IOObjectRetain(object: u32) -> i32;
    fn IOObjectRelease(object: u32) -> i32;
}

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFRunLoopDefaultMode: Ref;
    fn CFRetain(object: Ref) -> Ref;
    fn CFRelease(object: Ref);
    fn CFStringCreateWithCString(allocator: Ref, text: *const c_char, encoding: u32) -> Ref;
    fn CFNumberGetValue(number: Ref, kind: isize, value: *mut c_void) -> u8;
    fn CFGetTypeID(object: Ref) -> usize;
    fn CFNumberGetTypeID() -> usize;
    fn CFSetGetCount(set: Ref) -> isize;
    fn CFSetGetValues(set: Ref, values: *mut Ref);
    fn CFRunLoopGetCurrent() -> Ref;
    fn CFRunLoopRun();
    fn CFRunLoopStop(run_loop: Ref);
}

const UTF8: u32 = 0x0800_0100;
const NUMBER_SINT32: isize = 3;
const REPORT_OUTPUT: u32 = 1;

unsafe fn string(text: &str) -> Ref {
    let text = CString::new(text).unwrap();
    CFStringCreateWithCString(std::ptr::null(), text.as_ptr(), UTF8)
}

unsafe fn number_of(value: Ref) -> Option<i32> {
    if value.is_null() || CFGetTypeID(value) != CFNumberGetTypeID() {
        return None;
    }
    let mut out = 0i32;
    (CFNumberGetValue(value, NUMBER_SINT32, &mut out as *mut i32 as *mut c_void) != 0).then_some(out)
}

unsafe fn property(device: Ref, key: &str) -> Option<i32> {
    let key = string(key);
    let value = number_of(IOHIDDeviceGetProperty(device, key));
    CFRelease(key);
    value
}

/// `bInterfaceNumber`, from whichever ancestor has it.
unsafe fn interface_number(device: Ref) -> Option<u8> {
    let mut current = IOHIDDeviceGetService(device);
    if current == 0 {
        return None;
    }
    IOObjectRetain(current);
    let key = string("bInterfaceNumber");
    let mut found = None;
    for _ in 0..6 {
        let value = IORegistryEntryCreateCFProperty(current, key, std::ptr::null(), 0);
        if !value.is_null() {
            found = number_of(value).map(|n| n as u8);
            CFRelease(value);
            if found.is_some() {
                break;
            }
        }
        let mut parent = 0u32;
        if IORegistryEntryGetParentEntry(current, c"IOService".as_ptr(), &mut parent) != 0 {
            break;
        }
        IOObjectRelease(current);
        current = parent;
    }
    IOObjectRelease(current);
    CFRelease(key);
    found
}

/// A device held, as something that can cross to the run loop's thread.
#[derive(Clone, Copy)]
struct Device(Ref);
unsafe impl Send for Device {}
// Only ever handed to calls that IOKit and Core Foundation allow from any thread.
unsafe impl Sync for Device {}

/// Every HID interface on the machine from a vendor a supported pair of glasses has, as
/// (device, product id, interface number). The devices are retained.
unsafe fn interfaces() -> Vec<(Device, &'static DeviceSpec, u8)> {
    let mut found = Vec::new();
    let manager = IOHIDManagerCreate(std::ptr::null(), 0);
    if manager.is_null() {
        return found;
    }
    // Everything, and the table picked from here: a matching dictionary takes one vendor, and
    // the table may one day hold more than one.
    IOHIDManagerSetDeviceMatching(manager, std::ptr::null());
    let set = IOHIDManagerCopyDevices(manager);
    if !set.is_null() {
        let count = CFSetGetCount(set).max(0) as usize;
        let mut devices: Vec<Ref> = vec![std::ptr::null(); count];
        CFSetGetValues(set, devices.as_mut_ptr());
        for device in devices {
            let (Some(vid), Some(pid)) = (property(device, "VendorID"), property(device, "ProductID")) else {
                continue;
            };
            let Some(spec) = device::lookup(vid as u16, pid as u16) else { continue };
            let Some(interface) = interface_number(device) else { continue };
            CFRetain(device);
            found.push((Device(device), spec, interface));
        }
        CFRelease(set);
    }
    CFRelease(manager);
    found
}

/// Whether a supported pair of glasses is plugged in. Opens nothing.
pub fn is_present() -> bool {
    unsafe {
        let found = interfaces();
        let any = !found.is_empty();
        for (device, _, _) in found {
            CFRelease(device.0);
        }
        any
    }
}

/// What a report callback is handed: where to put the report, and the buffer IOKit fills.
struct Feed {
    into: UnixDatagram,
    buffer: Vec<u8>,
}

unsafe extern "C" fn reported(
    context: *mut c_void,
    _result: i32,
    _sender: *mut c_void,
    _kind: u32,
    _id: u32,
    report: *mut u8,
    length: isize,
) {
    let feed = &*(context as *const Feed);
    if length > 0 {
        // Never waits: a reader that has fallen behind loses reports rather than stopping the
        // run loop, and there is another along in a millisecond.
        let _ = feed.into.send(std::slice::from_raw_parts(report, length as usize));
    }
}

unsafe extern "C" fn removed(context: *mut c_void, _result: i32, _sender: *mut c_void) {
    // An empty report is the glasses being unplugged; see `read_report`.
    let feed = &*(context as *const Feed);
    let _ = feed.into.send(&[]);
}

/// The run loop both interfaces' callbacks are called on, stopped when the last port goes.
struct Loop {
    run_loop: Device,
}

impl Drop for Loop {
    fn drop(&mut self) {
        unsafe { CFRunLoopStop(self.run_loop.0) };
    }
}

/// One interface: written with a call, read from the socket its callback fills.
pub struct IoHidPort {
    device: Device,
    reports: UnixDatagram,
    name: String,
    _loop: std::sync::Arc<Loop>,
}

impl Port for IoHidPort {
    fn fd(&self) -> RawFd {
        self.reports.as_raw_fd()
    }

    fn write_report(&mut self, payload: &[u8]) -> Result<()> {
        let status =
            unsafe { IOHIDDeviceSetReport(self.device.0, REPORT_OUTPUT, 0, payload.as_ptr(), payload.len() as isize) };
        if status != 0 {
            return Err(HmdError::Io {
                path: self.name.clone(),
                source: std::io::Error::other(format!("IOHIDDeviceSetReport: {status:#x}")),
            });
        }
        Ok(())
    }

    fn read_report(&mut self, buf: &mut [u8], timeout: Duration) -> Result<Option<usize>> {
        let mut fds = [libc::pollfd { fd: self.reports.as_raw_fd(), events: libc::POLLIN, revents: 0 }];
        let ms = timeout.as_millis().min(i32::MAX as u128) as i32;
        if unsafe { libc::poll(fds.as_mut_ptr(), 1, ms) } <= 0 {
            return Ok(None);
        }
        match self.reports.recv(buf) {
            Ok(0) => Err(HmdError::Io {
                path: self.name.clone(),
                source: std::io::Error::new(std::io::ErrorKind::BrokenPipe, "the glasses were unplugged"),
            }),
            Ok(n) => Ok(Some(n)),
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => Ok(None),
            Err(source) => Err(HmdError::Io { path: self.name.clone(), source }),
        }
    }

    fn describe(&self) -> String {
        self.name.clone()
    }
}

impl Drop for IoHidPort {
    fn drop(&mut self) {
        unsafe {
            IOHIDDeviceClose(self.device.0, 0);
            CFRelease(self.device.0);
        }
    }
}

/// Open the first supported pair of glasses: its table row, its IMU interface and its MCU one.
pub fn open_glasses(driver: &str) -> Result<(&'static DeviceSpec, IoHidPort, IoHidPort)> {
    unsafe {
        let found = interfaces();
        let spec = found.iter().map(|(_, spec, _)| *spec).find(|s| s.driver == driver);
        let pick = |interface: u8| {
            let spec = spec?;
            found
                .iter()
                .find(|(_, s, i)| s.vid == spec.vid && s.pid == spec.pid && *i == interface)
                .map(|(d, _, _)| *d)
        };
        let chosen = spec.map(|s| (s, pick(s.imu_interface), pick(s.mcu_interface)));
        // Everything not chosen is let go of; what is chosen keeps the reference it was found with.
        let keep: Vec<Ref> = match &chosen {
            Some((_, imu, mcu)) => imu.iter().chain(mcu.iter()).map(|d| d.0).collect(),
            None => Vec::new(),
        };
        for (device, _, _) in &found {
            if !keep.contains(&device.0) {
                CFRelease(device.0);
            }
        }
        let Some((spec, imu, mcu)) = chosen else { return Err(HmdError::NotFound) };
        let missing = |interface: u8| HmdError::InterfaceMissing { device: spec.name.clone(), interface };
        let (imu, mcu) = match (imu, mcu) {
            (Some(imu), Some(mcu)) => (imu, mcu),
            (imu, mcu) => {
                for d in imu.iter().chain(mcu.iter()) {
                    CFRelease(d.0);
                }
                return Err(missing(if imu.is_none() { spec.imu_interface } else { spec.mcu_interface }));
            }
        };

        let mut opened: Vec<(Device, UnixDatagram, UnixDatagram, String)> = Vec::new();
        for (device, interface) in [(imu, spec.imu_interface), (mcu, spec.mcu_interface)] {
            let name = format!("{} interface {interface}", spec.name);
            let status = IOHIDDeviceOpen(device.0, 0);
            if status != 0 {
                return Err(HmdError::Io {
                    path: name,
                    source: std::io::Error::other(format!(
                        "IOHIDDeviceOpen: {status:#x} (is another program -- Nebula, say -- holding the glasses?)"
                    )),
                });
            }
            let (ours, theirs) = UnixDatagram::pair().map_err(|source| HmdError::Io { path: name.clone(), source })?;
            let _ = ours.set_nonblocking(true);
            let _ = theirs.set_nonblocking(true);
            // Room for a quarter of a second of reports. macOS gives a datagram socket two
            // kilobytes, which is thirty milliseconds of a thousand reports a second: a frame
            // that took longer than that lost samples, and the head with them.
            for socket in [&ours, &theirs] {
                let size: libc::c_int = 256 * 1024;
                for option in [libc::SO_SNDBUF, libc::SO_RCVBUF] {
                    libc::setsockopt(
                        socket.as_raw_fd(),
                        libc::SOL_SOCKET,
                        option,
                        &size as *const libc::c_int as *const c_void,
                        std::mem::size_of::<libc::c_int>() as libc::socklen_t,
                    );
                }
            }
            opened.push((device, ours, theirs, name));
        }

        // One thread, one run loop, both interfaces' callbacks.
        let report_len = spec.imu_report_len.max(64);
        let feeds: Vec<(Device, UnixDatagram)> =
            opened.iter().map(|(d, _, theirs, _)| (*d, theirs.try_clone().unwrap())).collect();
        let (tx, rx) = mpsc::channel::<Device>();
        std::thread::Builder::new()
            .name("glasses-hid".into())
            .spawn(move || {
                let run_loop = CFRunLoopGetCurrent();
                let mut kept: Vec<Box<Feed>> = Vec::new();
                for (device, into) in feeds {
                    let mut feed = Box::new(Feed { into, buffer: vec![0u8; report_len] });
                    let context = &mut *feed as *mut Feed as *mut c_void;
                    IOHIDDeviceRegisterInputReportCallback(
                        device.0,
                        feed.buffer.as_mut_ptr(),
                        report_len as isize,
                        Some(reported),
                        context,
                    );
                    IOHIDDeviceRegisterRemovalCallback(device.0, Some(removed), context);
                    IOHIDDeviceScheduleWithRunLoop(device.0, run_loop, kCFRunLoopDefaultMode);
                    kept.push(feed);
                }
                let _ = tx.send(Device(run_loop));
                CFRunLoopRun();
                // Stopped: the ports are going. The callbacks are taken off before what they
                // point at is.
                // (The devices themselves are closed by their ports.)
                drop(kept);
            })
            .map_err(|source| HmdError::Io { path: "glasses-hid thread".into(), source })?;
        let run_loop = rx
            .recv_timeout(Duration::from_secs(2))
            .map_err(|_| HmdError::Protocol("the HID run loop did not start".into()))?;
        let shared = std::sync::Arc::new(Loop { run_loop });

        let mut ports = opened.into_iter().map(|(device, ours, _theirs, name)| IoHidPort {
            device,
            reports: ours,
            name,
            _loop: shared.clone(),
        });
        let imu = ports.next().unwrap();
        let mcu = ports.next().unwrap();
        Ok((spec, imu, mcu))
    }
}
