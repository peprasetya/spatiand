//! Pictures on the GPU (IOSurfaces) handed from a process to its parent in private, on macOS:
//! a global IOSurface can be opened by any process that guesses its number. See
//! `native/surfaces.h`. A child [`post`]s a picture with a number; its parent, which
//! [`listen`]ed first, [`open`]s it by that number, and takes pictures only from its own children.

use std::ffi::c_void;

#[cfg(target_os = "macos")]
extern "C" {
    fn spatiand_surfaces_listen() -> i32;
    fn spatiand_surfaces_post(surface: *const c_void, number: u32) -> i32;
    fn spatiand_surfaces_open(number: u32) -> *const c_void;
}

/// Take pictures from this process's children (the ones started after this). Once is enough.
pub fn listen() -> bool {
    #[cfg(target_os = "macos")]
    return unsafe { spatiand_surfaces_listen() } != 0;
    #[cfg(not(target_os = "macos"))]
    false
}

/// Hand `surface` (an `IOSurfaceRef`) to this process's parent, as `number`.
///
/// # Safety
/// `surface` must be a live `IOSurfaceRef`.
pub unsafe fn post(surface: *const c_void, number: u32) -> bool {
    #[cfg(target_os = "macos")]
    return unsafe { spatiand_surfaces_post(surface, number) } != 0;
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (surface, number);
        false
    }
}

/// Hand this process's own picture with this ID (one it holds, global or not) to its parent, as
/// its ID.
pub fn post_own(id: u32) -> bool {
    #[cfg(target_os = "macos")]
    {
        #[link(name = "IOSurface", kind = "framework")]
        extern "C" {
            fn IOSurfaceLookup(id: u32) -> *const c_void;
        }
        #[link(name = "CoreFoundation", kind = "framework")]
        extern "C" {
            fn CFRelease(object: *const c_void);
        }
        unsafe {
            let surface = IOSurfaceLookup(id);
            if surface.is_null() {
                return false;
            }
            let posted = spatiand_surfaces_post(surface, id) != 0;
            CFRelease(surface);
            posted
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = id;
        false
    }
}

/// The picture a child handed over as `number`: an `IOSurfaceRef` this side owns (release it),
/// or null.
pub fn open(number: u32) -> *const c_void {
    #[cfg(target_os = "macos")]
    return unsafe { spatiand_surfaces_open(number) };
    #[cfg(not(target_os = "macos"))]
    {
        let _ = number;
        std::ptr::null()
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;

    extern "C" {
        fn IOSurfaceGetID(surface: *const c_void) -> u32;
        fn CFRelease(object: *const c_void);
    }

    /// A child's picture reaches its parent by the number it was handed over as, and only by it.
    #[test]
    fn a_child_hands_a_picture_over() {
        assert!(listen());
        let helper = concat!(env!("OUT_DIR"), "/surfaces-post-helper");
        let mut child = std::process::Command::new(helper).arg("4242").stdout(std::process::Stdio::piped()).spawn().unwrap();
        use std::io::BufRead;
        let mut line = String::new();
        std::io::BufReader::new(child.stdout.take().unwrap()).read_line(&mut line).unwrap();
        let id: u32 = line.trim().trim_start_matches("posted ").parse().unwrap_or_else(|_| panic!("the child said {line:?}"));
        let got = open(4242);
        assert!(!got.is_null(), "the child's picture did not arrive");
        assert_eq!(unsafe { IOSurfaceGetID(got) }, id);
        unsafe { CFRelease(got) };
        assert!(open(4243).is_null());
        let _ = child.kill();
        let _ = child.wait();
    }
}
