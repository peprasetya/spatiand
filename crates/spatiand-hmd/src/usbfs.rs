//! The glasses' HID interfaces through usbfs, for where there is no hidraw to open.
//!
//! That is Android. An app may not open `/dev/hidraw*`, but `UsbManager.openDevice` gives it
//! a usbfs descriptor for the whole device (`/dev/bus/usb/BBB/DDD`), and everything the
//! driver needs can be done through that one descriptor from native code in the same process
//! -- which is all Android's own `UsbDeviceConnection` does. Verified on the Beam Pro,
//! `docs/beam-pro.md`: interface 3 force-claimed, interrupt IN read with a bulk transfer,
//! 890-999 Hz of samples.
//!
//! The driver wants a descriptor per interface that is readable when a report is waiting and
//! hangs up on unplug ([`Port`]). A usbfs descriptor is neither: it is one for the whole
//! device, and a blocking transfer is how a report is read. So each claimed interface gets a
//! thread that does nothing but read its IN endpoint and hand every report across a
//! `SOCK_SEQPACKET` pair, which keeps report boundaries, is readable when one is waiting, and
//! hangs up when the thread stops because the glasses have gone. Writes go straight to the
//! OUT endpoint from whoever writes; usbfs takes transfers from several threads at once.
//!
//! Nothing here is Android-specific. It works on any Linux with a usbfs node, which is how it
//! is tested away from the Beam Pro.

use std::io;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

use crate::port::Port;
use crate::{HmdError, Result};

/// How long one read of an IN endpoint may block. Also how long closing an interface can take,
/// since its reader only looks at whether to stop between reads.
const READ_SLICE: Duration = Duration::from_millis(250);
/// How long a report written to the glasses may take to leave.
const WRITE_TIMEOUT: Duration = Duration::from_millis(500);

// --- usbfs ioctls, from <linux/usbdevice_fs.h> ---

#[repr(C)]
struct BulkTransfer {
    ep: libc::c_uint,
    len: libc::c_uint,
    /// Milliseconds.
    timeout: libc::c_uint,
    data: *mut libc::c_void,
}

#[repr(C)]
struct IoctlRequest {
    ifno: libc::c_int,
    ioctl_code: libc::c_int,
    data: *mut libc::c_void,
}

const fn ioc(dir: u32, nr: u32, size: usize) -> u32 {
    (dir << 30) | ((size as u32) << 16) | ((b'U' as u32) << 8) | nr
}
const IOC_WRITE: u32 = 1;
const IOC_READ: u32 = 2;
const USBDEVFS_BULK: u32 = ioc(IOC_READ | IOC_WRITE, 2, std::mem::size_of::<BulkTransfer>());
const USBDEVFS_CLAIMINTERFACE: u32 = ioc(IOC_READ, 15, std::mem::size_of::<libc::c_uint>());
const USBDEVFS_RELEASEINTERFACE: u32 = ioc(IOC_READ, 16, std::mem::size_of::<libc::c_uint>());
const USBDEVFS_IOCTL: u32 = ioc(IOC_READ | IOC_WRITE, 18, std::mem::size_of::<IoctlRequest>());
const USBDEVFS_DISCONNECT: u32 = ioc(0, 22, 0);

fn ioctl(fd: RawFd, request: u32, arg: *mut libc::c_void) -> io::Result<libc::c_int> {
    // The request type differs between libcs (c_ulong on glibc, c_int on bionic).
    let rc = unsafe { libc::ioctl(fd, request as _, arg) };
    if rc < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(rc)
    }
}

fn io_error(what: &str, source: io::Error) -> HmdError {
    HmdError::Io {
        path: format!("usbfs {what}"),
        source,
    }
}

/// One endpoint, as the configuration descriptor gives it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Endpoint {
    address: u8,
    max_packet: u16,
}

/// What the descriptors say about one interface's alternate setting 0.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Interface {
    number: u8,
    input: Option<Endpoint>,
    output: Option<Endpoint>,
}

/// What reading the device's descriptors gives.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Descriptors {
    vid: u16,
    pid: u16,
    interfaces: Vec<Interface>,
}

/// Parse what `read(2)` on a usbfs node returns: the device descriptor, then every
/// configuration's descriptors. Only the first configuration is read; these devices have one.
fn parse_descriptors(bytes: &[u8]) -> Option<Descriptors> {
    // Device descriptor: bLength 18, bDescriptorType 1, idVendor at 8, idProduct at 10.
    if bytes.len() < 18 || bytes[1] != 1 {
        return None;
    }
    let vid = u16::from_le_bytes([bytes[8], bytes[9]]);
    let pid = u16::from_le_bytes([bytes[10], bytes[11]]);
    let mut interfaces: Vec<Interface> = Vec::new();
    let mut configs = 0;
    // Endpoints belong to the interface descriptor before them, and only alternate setting 0
    // is wanted: the glasses' audio interfaces put their endpoints on setting 1.
    let mut current: Option<usize> = None;
    let mut at = bytes[0] as usize;
    while at + 2 <= bytes.len() {
        let len = bytes[at] as usize;
        if len < 2 || at + len > bytes.len() {
            break;
        }
        let d = &bytes[at..at + len];
        match d[1] {
            2 => {
                configs += 1;
                if configs > 1 {
                    break;
                }
            }
            4 if len >= 9 => {
                current = None;
                if d[3] == 0 {
                    interfaces.push(Interface {
                        number: d[2],
                        ..Default::default()
                    });
                    current = Some(interfaces.len() - 1);
                }
            }
            5 if len >= 7 => {
                if let Some(i) = current {
                    let ep = Endpoint {
                        address: d[2],
                        max_packet: u16::from_le_bytes([d[4], d[5]]) & 0x07FF,
                    };
                    let slot = if ep.address & 0x80 != 0 {
                        &mut interfaces[i].input
                    } else {
                        &mut interfaces[i].output
                    };
                    slot.get_or_insert(ep);
                }
            }
            _ => {}
        }
        at += len;
    }
    Some(Descriptors {
        vid,
        pid,
        interfaces,
    })
}

/// A USB device opened through usbfs.
pub struct UsbDevice {
    fd: Arc<OwnedFd>,
    descriptors: Descriptors,
}

impl UsbDevice {
    /// Take a usbfs descriptor for the whole device: one from `UsbManager.openDevice` on
    /// Android, or an opened `/dev/bus/usb/BBB/DDD` anywhere else.
    pub fn from_fd(fd: OwnedFd) -> Result<Self> {
        let mut buf = vec![0u8; 4096];
        // From offset 0 whatever has been read through this descriptor before: Java may
        // already have read the descriptors with `getRawDescriptors`.
        let n = unsafe { libc::pread(fd.as_raw_fd(), buf.as_mut_ptr().cast(), buf.len(), 0) };
        if n < 0 {
            return Err(io_error("descriptors", io::Error::last_os_error()));
        }
        let descriptors = parse_descriptors(&buf[..n as usize])
            .ok_or_else(|| HmdError::Protocol("usbfs descriptors did not parse".into()))?;
        Ok(Self {
            fd: Arc::new(fd),
            descriptors,
        })
    }

    pub fn vid(&self) -> u16 {
        self.descriptors.vid
    }

    pub fn pid(&self) -> u16 {
        self.descriptors.pid
    }

    /// Take one interface from whatever kernel driver has it and start reading it.
    pub fn claim(&self, number: u8) -> Result<UsbPort> {
        let iface = self
            .descriptors
            .interfaces
            .iter()
            .find(|i| i.number == number)
            .ok_or_else(|| HmdError::InterfaceMissing {
                device: format!("{:04x}:{:04x}", self.vid(), self.pid()),
                interface: number,
            })?;
        let input = iface.input.ok_or_else(|| {
            HmdError::Protocol(format!("interface {number} has no IN endpoint"))
        })?;
        let fd = self.fd.as_raw_fd();

        // The HID driver has every one of these interfaces, and a claim fails while it does.
        // Nothing bound is `ENODATA`, which is fine.
        let mut request = IoctlRequest {
            ifno: number as libc::c_int,
            ioctl_code: USBDEVFS_DISCONNECT as libc::c_int,
            data: std::ptr::null_mut(),
        };
        if let Err(e) = ioctl(fd, USBDEVFS_IOCTL, (&mut request as *mut IoctlRequest).cast()) {
            if e.raw_os_error() != Some(libc::ENODATA) {
                log::debug!("usbfs: detaching the driver from interface {number}: {e}");
            }
        }
        let mut ifno = number as libc::c_uint;
        ioctl(fd, USBDEVFS_CLAIMINTERFACE, (&mut ifno as *mut libc::c_uint).cast())
            .map_err(|e| io_error(&format!("claim interface {number}"), e))?;

        let mut pair = [0 as libc::c_int; 2];
        let rc = unsafe {
            libc::socketpair(
                libc::AF_UNIX,
                libc::SOCK_SEQPACKET | libc::SOCK_CLOEXEC,
                0,
                pair.as_mut_ptr(),
            )
        };
        if rc < 0 {
            let e = io::Error::last_os_error();
            let _ = ioctl(fd, USBDEVFS_RELEASEINTERFACE, (&mut ifno as *mut libc::c_uint).cast());
            return Err(io_error("socket pair", e));
        }
        let (ours, theirs) = unsafe { (OwnedFd::from_raw_fd(pair[0]), OwnedFd::from_raw_fd(pair[1])) };

        let stop = Arc::new(AtomicBool::new(false));
        let reader = Reader {
            device: self.fd.clone(),
            endpoint: input,
            socket: theirs,
            stop: stop.clone(),
        };
        let thread = std::thread::Builder::new()
            .name(format!("usb-if{number}"))
            .spawn(move || reader.run())
            .map_err(|e| io_error("reader thread", e))?;

        Ok(UsbPort {
            device: self.fd.clone(),
            number,
            output: iface.output,
            socket: ours,
            stop,
            thread: Some(thread),
        })
    }
}

/// Reads one interface's IN endpoint for as long as the glasses are there.
struct Reader {
    device: Arc<OwnedFd>,
    endpoint: Endpoint,
    socket: OwnedFd,
    stop: Arc<AtomicBool>,
}

impl Reader {
    fn run(self) {
        let mut buf = vec![0u8; self.endpoint.max_packet.max(64) as usize];
        while !self.stop.load(Ordering::Relaxed) {
            match bulk(&self.device, self.endpoint.address, &mut buf, READ_SLICE) {
                Ok(n) => {
                    // Not waited for: a reader that has fallen behind is better served by the
                    // newest reports than by the socket's backlog, and a full socket means
                    // exactly that.
                    let sent = unsafe {
                        libc::send(
                            self.socket.as_raw_fd(),
                            buf.as_ptr().cast(),
                            n,
                            libc::MSG_DONTWAIT | libc::MSG_NOSIGNAL,
                        )
                    };
                    if sent < 0 {
                        let e = io::Error::last_os_error();
                        if e.kind() != io::ErrorKind::WouldBlock {
                            // The port was dropped.
                            return;
                        }
                    }
                }
                Err(e) if e.raw_os_error() == Some(libc::ETIMEDOUT) => {}
                Err(e) if e.kind() == io::ErrorKind::Interrupted => {}
                // `ENODEV`, `ESHUTDOWN`, `EPROTO`: the glasses have gone. Returning drops the
                // socket, which the port's side sees as a hangup.
                Err(e) => {
                    log::info!("usbfs: endpoint {:#04x} stopped ({e})", self.endpoint.address);
                    return;
                }
            }
        }
    }
}

/// One transfer on an interrupt endpoint. usbfs's bulk call carries interrupt endpoints too,
/// which is what Android's `bulkTransfer` relies on.
fn bulk(device: &OwnedFd, endpoint: u8, buf: &mut [u8], timeout: Duration) -> io::Result<usize> {
    let mut request = BulkTransfer {
        ep: endpoint as libc::c_uint,
        len: buf.len() as libc::c_uint,
        timeout: timeout.as_millis().min(u32::MAX as u128) as libc::c_uint,
        data: buf.as_mut_ptr().cast(),
    };
    ioctl(
        device.as_raw_fd(),
        USBDEVFS_BULK,
        (&mut request as *mut BulkTransfer).cast(),
    )
    .map(|n| n as usize)
}

/// A claimed interface. See the module notes.
pub struct UsbPort {
    device: Arc<OwnedFd>,
    number: u8,
    output: Option<Endpoint>,
    socket: OwnedFd,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl Port for UsbPort {
    fn fd(&self) -> RawFd {
        self.socket.as_raw_fd()
    }

    fn write_report(&mut self, payload: &[u8]) -> Result<()> {
        let Some(out) = self.output else {
            // Every interface the driver writes to has one; hidapi's fallback, a SET_REPORT
            // control transfer, has not been needed.
            return Err(HmdError::Protocol(format!(
                "interface {} has no OUT endpoint",
                self.number
            )));
        };
        // Unlike hidraw, no report-id byte: this is the report exactly as it goes on the wire.
        let mut buf = payload.to_vec();
        let n = bulk(&self.device, out.address, &mut buf, WRITE_TIMEOUT)
            .map_err(|e| io_error(&format!("write interface {}", self.number), e))?;
        if n != payload.len() {
            return Err(HmdError::Protocol(format!(
                "interface {} took {n} of {} bytes",
                self.number,
                payload.len()
            )));
        }
        Ok(())
    }

    fn read_report(&mut self, buf: &mut [u8], timeout: Duration) -> Result<Option<usize>> {
        let mut fds = [libc::pollfd {
            fd: self.socket.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        }];
        let ms = timeout.as_millis().min(i32::MAX as u128) as i32;
        let rc = unsafe { libc::poll(fds.as_mut_ptr(), 1, ms) };
        if rc < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::Interrupted {
                return Ok(None);
            }
            return Err(io_error("poll", e));
        }
        if rc == 0 || fds[0].revents & libc::POLLIN == 0 {
            return Ok(None);
        }
        let n = unsafe {
            libc::recv(
                self.socket.as_raw_fd(),
                buf.as_mut_ptr().cast(),
                buf.len(),
                libc::MSG_DONTWAIT,
            )
        };
        if n < 0 {
            let e = io::Error::last_os_error();
            if e.kind() == io::ErrorKind::WouldBlock {
                return Ok(None);
            }
            return Err(io_error(&format!("read interface {}", self.number), e));
        }
        Ok(Some(n as usize))
    }

    fn describe(&self) -> String {
        format!("usbfs interface {}", self.number)
    }
}

impl Drop for UsbPort {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
        let mut ifno = self.number as libc::c_uint;
        let _ = ioctl(
            self.device.as_raw_fd(),
            USBDEVFS_RELEASEINTERFACE,
            (&mut ifno as *mut libc::c_uint).cast(),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The XREAL Air's descriptors as the Beam Pro enumerated them (`docs/xreal-air.md` §3):
    /// three audio interfaces with their endpoints on alternate setting 1, then four HID
    /// interfaces. Class-specific descriptors are left out; the parser skips them anyway.
    fn air() -> Vec<u8> {
        let mut d = vec![
            18, 1, 0x00, 0x02, 0, 0, 0, 64, 0x18, 0x33, 0x24, 0x04, 0, 1, 1, 2, 3, 1,
        ];
        d.extend_from_slice(&[9, 2, 0, 0, 7, 1, 0, 0x80, 250]);
        let iface = |n: u8, alt: u8, eps: u8| vec![9, 4, n, alt, eps, 3, 0, 0, 0];
        let ep = |addr: u8, max: u16| {
            let m = max.to_le_bytes();
            vec![7, 5, addr, 3, m[0], m[1], 1]
        };
        d.extend(iface(0, 0, 0));
        d.extend(iface(1, 0, 0));
        d.extend(iface(1, 1, 1));
        d.extend(ep(0x03, 192));
        d.extend(iface(2, 0, 0));
        d.extend(iface(2, 1, 1));
        d.extend(ep(0x82, 104));
        // A HID class descriptor between the interface and its endpoints, as on the wire.
        d.extend(iface(3, 0, 2));
        d.extend_from_slice(&[9, 0x21, 0x11, 0x01, 0, 1, 0x22, 0x20, 0]);
        d.extend(ep(0x84, 64));
        d.extend(ep(0x05, 64));
        d.extend(iface(4, 0, 2));
        d.extend(ep(0x86, 64));
        d.extend(ep(0x07, 64));
        d.extend(iface(6, 0, 1));
        d.extend(ep(0x8a, 3));
        d
    }

    #[test]
    fn finds_the_air_and_its_hid_endpoints() {
        let d = parse_descriptors(&air()).expect("should parse");
        assert_eq!((d.vid, d.pid), (0x3318, 0x0424));
        let imu = d.interfaces.iter().find(|i| i.number == 3).unwrap();
        assert_eq!(imu.input, Some(Endpoint { address: 0x84, max_packet: 64 }));
        assert_eq!(imu.output, Some(Endpoint { address: 0x05, max_packet: 64 }));
        let mcu = d.interfaces.iter().find(|i| i.number == 4).unwrap();
        assert_eq!(mcu.input.map(|e| e.address), Some(0x86));
        assert_eq!(mcu.output.map(|e| e.address), Some(0x07));
    }

    #[test]
    fn endpoints_on_an_alternate_setting_belong_to_nobody() {
        // The speakers' isochronous endpoint sits on interface 1, setting 1. Hung on the
        // setting-0 interface before it, it would be an OUT endpoint on an interface that has
        // none -- harmless for audio, and exactly the mistake that would put a write on the
        // wrong endpoint for a HID interface laid out the same way.
        let d = parse_descriptors(&air()).unwrap();
        let speakers = d.interfaces.iter().find(|i| i.number == 1).unwrap();
        assert_eq!((speakers.input, speakers.output), (None, None));
        assert_eq!(d.interfaces.iter().filter(|i| i.number == 1).count(), 1);
    }

    #[test]
    fn a_truncated_blob_stops_rather_than_reading_past_the_end() {
        let mut d = air();
        d.truncate(d.len() - 3);
        let parsed = parse_descriptors(&d).expect("the device descriptor is intact");
        let buttons = parsed.interfaces.iter().find(|i| i.number == 6).unwrap();
        assert_eq!(buttons.input, None);
    }

    #[test]
    fn not_a_device_descriptor_is_refused() {
        assert_eq!(parse_descriptors(&[9, 2, 0, 0]), None);
        assert_eq!(parse_descriptors(&[]), None);
    }

    #[test]
    fn the_ioctl_numbers_are_the_kernels() {
        // As <linux/usbdevice_fs.h> expands them on a 64-bit target.
        assert_eq!(USBDEVFS_BULK, 0xC018_5502);
        assert_eq!(USBDEVFS_CLAIMINTERFACE, 0x8004_550F);
        assert_eq!(USBDEVFS_RELEASEINTERFACE, 0x8004_5510);
        assert_eq!(USBDEVFS_IOCTL, 0xC010_5512);
        assert_eq!(USBDEVFS_DISCONNECT, 0x0000_5516);
    }
}
