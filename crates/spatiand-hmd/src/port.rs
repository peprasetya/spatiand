//! One HID interface of the glasses, whatever carries it.
//!
//! On the Deck an interface is a hidraw node. On the Beam Pro there is no hidraw an app may
//! open: Android hands over the whole USB device as one usbfs descriptor, and the interfaces
//! are reached through that (see [`crate::usbfs`]). The driver does not care which. It needs
//! a report written, a report read, and something to `poll(2)` on that becomes readable when
//! a report is waiting and hangs up when the glasses are unplugged.

use std::os::fd::RawFd;
use std::time::Duration;

use crate::hid::HidDevice;
use crate::Result;

/// One interface's end of the wire: a hidraw node, a usbfs interface, a socket in the tests.
pub trait Port: Send {
    /// Readable while a report is waiting; hung up once the device has gone.
    fn fd(&self) -> RawFd;
    /// Write one report. Just the report: whatever framing the transport needs is its own.
    fn write_report(&mut self, payload: &[u8]) -> Result<()>;
    /// One report, waiting at most `timeout`. `Ok(None)` is the timeout expiring.
    fn read_report(&mut self, buf: &mut [u8], timeout: Duration) -> Result<Option<usize>>;
    /// What to call it in the log.
    fn describe(&self) -> String;
}

impl<P: Port + ?Sized> Port for Box<P> {
    fn fd(&self) -> RawFd {
        (**self).fd()
    }

    fn write_report(&mut self, payload: &[u8]) -> Result<()> {
        (**self).write_report(payload)
    }

    fn read_report(&mut self, buf: &mut [u8], timeout: Duration) -> Result<Option<usize>> {
        (**self).read_report(buf, timeout)
    }

    fn describe(&self) -> String {
        (**self).describe()
    }
}

impl Port for HidDevice {
    fn fd(&self) -> RawFd {
        self.as_raw_fd()
    }

    fn write_report(&mut self, payload: &[u8]) -> Result<()> {
        HidDevice::write_report(self, payload)
    }

    fn read_report(&mut self, buf: &mut [u8], timeout: Duration) -> Result<Option<usize>> {
        HidDevice::read_report(self, buf, timeout)
    }

    fn describe(&self) -> String {
        self.path().display().to_string()
    }
}
