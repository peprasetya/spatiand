//! Spatiand on Android, as `libspatiand.so` inside the `id.prasetya.spatiand` app.
//!
//! The app is the Beam Pro's glasses shell in place of XREAL's Nebula (`docs/beam-pro.md`).
//! Java owns what only Java may: the phone's activity, the `Presentation` on the glasses'
//! display, and asking `UsbManager` for the glasses. Everything after that is here -- the
//! glasses through the same driver the Deck uses (`XrealGlasses::open_usb`), the same tracker,
//! and the drawing.
//!
//! Only built for Android. Anywhere else this library is empty, so a workspace build on the
//! Mac or the Deck does not have to know it exists.

#[cfg(target_os = "android")]
mod android;
