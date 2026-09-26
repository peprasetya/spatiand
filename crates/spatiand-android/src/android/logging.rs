//! `log` into logcat, under the tag `spatiand`, so `adb logcat -s spatiand` shows it all.

use std::ffi::CString;

struct Logcat;

impl log::Log for Logcat {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        metadata.level() <= log::Level::Info
    }

    fn log(&self, record: &log::Record) {
        if !self.enabled(record.metadata()) {
            return;
        }
        let priority = match record.level() {
            log::Level::Error => ndk_sys::android_LogPriority::ANDROID_LOG_ERROR,
            log::Level::Warn => ndk_sys::android_LogPriority::ANDROID_LOG_WARN,
            log::Level::Info => ndk_sys::android_LogPriority::ANDROID_LOG_INFO,
            _ => ndk_sys::android_LogPriority::ANDROID_LOG_DEBUG,
        };
        let text = CString::new(format!("{}", record.args())).unwrap_or_default();
        unsafe {
            ndk_sys::__android_log_write(priority.0 as i32, c"spatiand".as_ptr(), text.as_ptr());
        }
    }

    fn flush(&self) {}
}

pub fn init() {
    static LOGCAT: Logcat = Logcat;
    if log::set_logger(&LOGCAT).is_ok() {
        log::set_max_level(log::LevelFilter::Info);
    }
    std::panic::set_hook(Box::new(|info| log::error!("panic: {info}")));
}
