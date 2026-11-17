//! SpatiKey's half on an Android device with a strong biometric: `KeyGuard.java`, asked through
//! JNI (docs/spatikey.md, "The holder's half").
//!
//! The service is spatikey-core's, as on every platform; what only Android can do -- a Keystore
//! key behind a class-3 fingerprint or face, BiometricPrompt, asking the owner about a device -- is
//! the app's, asked through spatikey-core's `platform` exactly as the Mac's `SpatiKeyGuard.swift`
//! is: a question with a number, answered later through [`Java_id_prasetya_spatiand_KeyGuard_answer`]
//! (or, carrying the vault key, `answerKey`, as bytes the app then overwrites). A device with no
//! such sensor -- the Beam Pro -- gets no guard, and its service asks a holder instead.

use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

use jni_sys::{jbyte, jbyteArray, jclass, jint, jlong, jmethodID, jstring, jvalue, JNIEnv, JNI_FALSE};
use spatikey_core::platform::{self, AppGuard, Asked};
use spatikey_core::service::Guard;

use super::jni::{java_string, new_string, with_shell};

/// `KeyGuard`, as a global reference, once found.
static CLASS: OnceLock<usize> = OnceLock::new();

/// How long the app may take to give its context before this device is taken to have no sensor.
const CONTEXT_WAIT: Duration = Duration::from_secs(30);

/// `KeyGuard`, found through `Shell`'s class loader: a thread the app did not start sees only
/// the system's classes.
///
/// # Safety
/// `env` and `shell` as [`with_shell`] hands them over.
unsafe fn key_guard(env: *mut JNIEnv, shell: jclass) -> Option<jclass> {
    if let Some(&class) = CLASS.get() {
        return Some(class as jclass);
    }
    let f = &**env;
    let failed = |env: *mut JNIEnv| {
        if (f.v1_2.ExceptionCheck)(env) != JNI_FALSE {
            (f.v1_1.ExceptionClear)(env);
        }
    };
    let class_class = (f.v1_1.FindClass)(env, c"java/lang/Class".as_ptr());
    let loader_class = (f.v1_1.FindClass)(env, c"java/lang/ClassLoader".as_ptr());
    if class_class.is_null() || loader_class.is_null() {
        failed(env);
        return None;
    }
    let get_loader = (f.v1_1.GetMethodID)(env, class_class, c"getClassLoader".as_ptr(), c"()Ljava/lang/ClassLoader;".as_ptr());
    let load = (f.v1_1.GetMethodID)(env, loader_class, c"loadClass".as_ptr(), c"(Ljava/lang/String;)Ljava/lang/Class;".as_ptr());
    if get_loader.is_null() || load.is_null() {
        failed(env);
        return None;
    }
    let loader = (f.v1_1.CallObjectMethodA)(env, shell, get_loader, std::ptr::null());
    if loader.is_null() {
        failed(env);
        return None;
    }
    let name = new_string(env, "id.prasetya.spatiand.KeyGuard");
    let found = (f.v1_1.CallObjectMethodA)(env, loader, load, [jvalue { l: name }].as_ptr());
    if found.is_null() || (f.v1_2.ExceptionCheck)(env) != JNI_FALSE {
        failed(env);
        log::warn!("SpatiKey: no KeyGuard class in the app");
        return None;
    }
    let global = (f.v1_1.NewGlobalRef)(env, found) as usize;
    Some(*CLASS.get_or_init(|| global) as jclass)
}

/// A static method of `KeyGuard`.
///
/// # Safety
/// `env` and `class` from [`with_shell`] and [`key_guard`].
unsafe fn method(env: *mut JNIEnv, class: jclass, name: &std::ffi::CStr, signature: &std::ffi::CStr) -> Option<jmethodID> {
    let method = ((**env).v1_1.GetStaticMethodID)(env, class, name.as_ptr(), signature.as_ptr());
    if method.is_null() {
        ((**env).v1_1.ExceptionClear)(env);
        log::warn!("SpatiKey: no KeyGuard.{}", name.to_string_lossy());
        return None;
    }
    Some(method)
}

/// `KeyGuard.sensor()`: -1 while the app has not given its context, 0 no biometric hardware, 1
/// hardware with nothing enrolled or not usable now, 2 a strong biometric ready. Asks nobody.
fn sensor() -> i32 {
    with_shell(|env, shell| unsafe {
        let Some(class) = key_guard(env, shell) else { return 0 };
        let Some(sensor) = method(env, class, c"sensor", c"()I") else { return 0 };
        ((**env).v1_1.CallStaticIntMethodA)(env, class, sensor, std::ptr::null())
    })
    .unwrap_or(-1)
}

/// One question to `KeyGuard.ask(id, what, json)`, which returns at once and answers later. One
/// that cannot be asked is answered here, at once, so that nobody waits for it.
fn send(id: u64, what: Asked, json: &str) {
    let sent = with_shell(|env, shell| unsafe {
        let Some(class) = key_guard(env, shell) else { return false };
        let Some(ask) = method(env, class, c"ask", c"(JILjava/lang/String;)V") else { return false };
        let text = new_string(env, json);
        let args = [jvalue { j: id as jlong }, jvalue { i: what as jint }, jvalue { l: text }];
        ((**env).v1_1.CallStaticVoidMethodA)(env, class, ask, args.as_ptr());
        ((**env).v1_2.ExceptionCheck)(env) == JNI_FALSE
    });
    if sent != Some(true) {
        platform::answer_text(id, r#"{"error":"touch_id_unavailable","message":"SpatiKey's half in the app is not there"}"#);
    }
}

/// SpatiKey's guard on this device, when it has biometric hardware: the service holds the vault
/// with it once a holder has added this device (or the recovery code has). None on a device with
/// none, whose service only asks. Waits for the app to give its context, up to half a minute.
pub fn guard() -> Option<Arc<dyn Guard>> {
    let since = Instant::now();
    let mut sensor = sensor();
    while sensor < 0 && since.elapsed() < CONTEXT_WAIT {
        std::thread::sleep(Duration::from_millis(250));
        sensor = self::sensor();
    }
    match sensor {
        s if s > 0 => {
            log::info!("SpatiKey: this device has a strong biometric ({}), and may hold the vault", if s == 2 { "ready" } else { "nothing enrolled now" });
            platform::set_app(Some(Arc::new(send)));
            Some(Arc::new(AppGuard::android()))
        }
        0 => {
            log::info!("SpatiKey: no strong biometric on this device: it asks a holder");
            None
        }
        _ => {
            log::warn!("SpatiKey: the app gave no context in {}s: asking a holder", CONTEXT_WAIT.as_secs());
            None
        }
    }
}

/// `KeyGuard.answer(id, json)`: the app's answer to question `id`.
#[no_mangle]
pub extern "system" fn Java_id_prasetya_spatiand_KeyGuard_answer(env: *mut JNIEnv, _class: jclass, id: jlong, json: jstring) {
    let text = unsafe { java_string(env, json) }.unwrap_or_default();
    platform::answer_text(id as u64, &text);
}

/// `KeyGuard.answerKey(id, vk)`: an unlock's vault key, as bytes; the app overwrites its array
/// once this returns.
#[no_mangle]
pub extern "system" fn Java_id_prasetya_spatiand_KeyGuard_answerKey(env: *mut JNIEnv, _class: jclass, id: jlong, vk: jbyteArray) {
    let mut bytes = [0u8; 32];
    let read = unsafe {
        let f = &**env;
        let length = (f.v1_1.GetArrayLength)(env, vk);
        if length == 32 {
            (f.v1_1.GetByteArrayRegion)(env, vk, 0, 32, bytes.as_mut_ptr() as *mut jbyte);
        }
        length == 32 && (f.v1_2.ExceptionCheck)(env) == JNI_FALSE
    };
    if read {
        platform::answer_key(id as u64, &bytes);
    } else {
        platform::answer_text(id as u64, r#"{"error":"failed","message":"the fingerprint sensor's key came back in a shape SpatiKey does not keep"}"#);
    }
    // The stack copy, overwritten in a way the compiler keeps.
    for byte in bytes.iter_mut() {
        unsafe { std::ptr::write_volatile(byte, 0) };
    }
}
