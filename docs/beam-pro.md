# Spatiand on the XREAL Beam Pro

Can spatiand take the place of Nebula ("My Glass") on the Beam Pro, the way it takes the place
of SteamOS's desktop and game modes on the Deck? The constraint is **no root, ever**: an
ordinary app, plus shell (ADB) privilege where the app alone cannot do something.

Device: Beam Pro X4000, build `X4000_X502_260518_ROW`, Android 14, Snapdragon `parrot`.
Glasses: XREAL Air `0x3318:0x0424`. Checked 2026-09-26. The spike is `tools/beam-spike/`
(`build.sh` needs only the SDK's build-tools, no Gradle).

---

## How Nebula owns the glasses

This comes from decompiling Nebula 2.2.1 and the framework's own `services.jar`. **[verified]**

- **Nebula is an app, not a platform.** `com.xreal.evapro.nebula` runs as uid 1000 (system).
  - Its renderer is an ordinary activity started on the glasses' display. That display
    appears as `HDMI Screen`, category PRESENTATION, with PnP id `MRG`.
  - It shows Android apps on virtual displays. `LaunchManager.createVirtualDisplay` uses
    flags 1485 (PUBLIC, SECURE, OWN_CONTENT_ONLY, SUPPORTS_TOUCH, ROTATES_WITH_CONTENT,
    DESTROY_CONTENT_ON_REMOVAL, TRUSTED), followed by `setLaunchDisplayId`, with input
    delivered by `injectInputEvent`.
  - Its video player is inside Nebula itself, which is why it can switch 2D/3D.
- **The framework hands it the glasses, whatever else is installed.** XREAL's `UsbHostManager`
  matches `xreal_vids` (with `persist.sys.lc_default_xreal_usbhost`, true by default) and
  starts Nebula's `SplashActivity` as a fixed handler (`deviceAttachedForFixedHandler`).
  Android never asks which app should have the glasses, so there is no "Always" to tick. With
  Nebula disabled the start fails quietly (result -92) and nobody is asked at all. The
  `USB_DEVICE_ATTACHED` *broadcast* still goes to every app, before that start.
- **The "static placeholder" in [xreal-air.md §8](xreal-air.md) is a framework window.**
  - When an `MRG` display is added, `DisplayManagerService` sets
    `Settings.System xreal_priview_show=1`. `PhoneWindowManager.showPresent()` then shows an
    `ArLauncherPresentation` (the NebulaOS jpeg) over the display.
  - `settings put system xreal_preview_update stop` dismisses it.
  - It comes back on **every** display add, including the re-add caused by a 2D/3D switch.
- `live_app_xreal` / `live_app_thridpart` (Settings.System) only feed the OomAdjuster's
  keep-alive whitelist. They do not decide what may appear on the glasses.

## What the shell user may do

`dumpsys package com.android.shell` shows these as granted: `ADD_TRUSTED_DISPLAY`,
`INJECT_EVENTS`, `CAPTURE_VIDEO_OUTPUT`, `MANAGE_USB`, `INTERNAL_SYSTEM_WINDOW`,
`WRITE_SECURE_SETTINGS`, `ADD_ALWAYS_UNLOCKED_DISPLAY`, `MANAGE_ACTIVITY_TASKS`,
`START_ACTIVITIES_FROM_BACKGROUND`. **[verified]**

**Not granted: `CAPTURE_SECURE_VIDEO_OUTPUT`.** A SECURE virtual display is refused with
`SecurityException: Requires CAPTURE_SECURE_VIDEO_OUTPUT`. So protected apps (DRM video,
`FLAG_SECURE` windows) render black inside the sphere. Only a system app can avoid that. The
way around it is to launch such an app directly on the glasses display, flat, where nothing
has to be captured.

## Phase 0 results — 2026-09-26

Nebula was disabled for these checks with `pm disable-user --user 0 com.xreal.evapro.nebula`.
`pm enable com.xreal.evapro.nebula` restores it.

| Check | Result |
|---|---|
| (a) An ordinary app's `Presentation` on the glasses | **Pass.** After the placeholder was dismissed, `screencap` of the physical display shows the spike's pattern. It renders at 60 fps, although the display advertises 90 Hz at 1920×1080. |
| (b) 2D → side-by-side 3D from an app | **Pass.** `W_DISP_MODE 0x3` on interface 4 made the display re-add as 3840×1080 @ 60, with a new display id. The placeholder returns, so it must be dismissed again and the Presentation re-shown on the new display. |
| (c) IMU through `UsbManager` | **Pass.** It needs one USB permission prompt. Interface 3 was force-claimed and gave 890–999 Hz, \|a\| = 1.008 g, \|m\| = 0.36 G, and it kept streaming through the mode switch. |
| (c′) Axes | **Confirmed**, by rate peaks per axis in a wearer's roll/nod/turn test: yaw = sensor Z, pitch = sensor X, roll = sensor Y, with peaks of 100–220 deg/s on the axis moved. That is `AxisMap::XREAL_AIR`, the same map the Deck uses. Still-head bias at the time was (+0.57, −0.84, −0.75) deg/s. Roll is never pure; it shows up partly on X as well. |
| (d) Another app on a shell-created virtual display | **Pass, without SECURE.** The shell process (`app_process`) created a TRUSTED display. `am start --display` put vibrefy on it as the top resumed activity, and `input -d` swipes made it redraw. The `ImageReader` counted 17 fps while it drew and 0 while it was idle. |
| (e) GPU cost of several windows plus a decode | Not yet measured. |

To reproduce, with the glasses plugged in:

```sh
tools/beam-spike/build.sh && adb install -r tools/beam-spike/spike.apk
adb shell pm disable-user --user 0 com.xreal.evapro.nebula
adb shell settings put system xreal_preview_update stop
adb shell am start -n id.prasetya.spatiand.spike/.Main --es cmd show
adb shell am start -n id.prasetya.spatiand.spike/.Main --es cmd imu      # tap Allow on the Beam Pro
adb shell am start -n id.prasetya.spatiand.spike/.Main --es cmd mode --ei mode 3
adb logcat -s spike
```

## Taking the glasses from Nebula, once — verified 2026-09-26

`android/setup.sh`, run once from a computer with adb, with the glasses plugged in. Each of
its three steps survives reboots:

1. **Nebula disabled** (`pm disable-user`). While it is enabled, the fixed handler gives it
   the glasses on every plug-in. `android/setup.sh nebula` gives them back.
2. **"Display over other apps"** for Spatiand (`appops set … SYSTEM_ALERT_WINDOW allow`).
   - Its picture is a `TYPE_APPLICATION_OVERLAY` window on the glasses' display, which sits
     above the NebulaOS placeholder, so the placeholder needs no shell to dismiss.
   - The window must not be `FLAG_NOT_TOUCHABLE`, or Android caps it at alpha 0.8 and the
     placeholder shows through.
   - The permission also lets the app start its activity from the background.
3. **A persistent USB permission.**
   - `Setup.java` runs as the shell through `app_process` and calls
     `IUsbManager.setDevicePersistentPermission` (shell holds `MANAGE_USB`).
   - `dumpsys usb` then lists `uid_permission … is_granted=true` for the glasses' serial.

After that, plugging in opens Spatiand. `Plugged.java` receives the attach broadcast, starts
the activity (`BAL_ALLOW_SAW_PERMISSION`), and the activity takes the USB with no question.
This works whether or not the app is running.

The one exception is a **force-stopped** app. Android sends no broadcasts to one until it
has been opened by hand once.

## The Deck's session on the Beam Pro — running 2026-09-27

The app runs **the Deck's compositor itself**. `crates/spatiand-android` compiles
`crates/spatiand/src`'s own modules by path, so the room is the Deck's:
- the windows have the same frames and title bars, and are moved, resized and pushed away the
  same way;
- the HUD, launcher, menus and on-screen keyboard are the same.

Only what the Deck gets from its hardware is Android's (`crates/spatiand-android/src/android/`):

| on the Deck | on the Beam Pro |
|---|---|
| DRM scanout | smithay's `GlesRenderer` drawing straight into the glasses' `ANativeWindow` (`backend.rs`, adapted from `backend_drm.rs`) |
| the Deck's controller | the phone, made into the Deck's controller state (`phone.rs`) |
| hidraw | the glasses' usbfs descriptor |
| the sidecar panel | the phone's touch area, with the monitors drawn dim under the thumb (`panel.rs`) |
| VAAPI dmabufs | MediaCodec into `AHardwareBuffer`s; a placeholder shm buffer carries a token, and the compositor draws the decoder's picture in its place (`spatiand_video::android`, `remote_video.rs`) |
| PipeWire sinks | the same `Binaural` renderer and measured head, on AAudio (`spatiand_audio`'s `server_android.rs`) |

**The phone as a controller:**
- The **touch area** is the left pad: one finger scrolls, a pinch zooms the window pointed at.
- Clicks:
  - **tap:** click;
  - **tap then hold:** hold the click, to drag a title bar or select, with finger movement setting a dragged window's distance;
  - **long press:** right click.
- **Where the phone points** is the right pad. It is aimed at the world, not the head, through
  `spatiand_render::ray::pad_for_direction`. Recentring aims it where the head faces.
- Buttons:
  - the **orange key** is STEAM (the framework rebroadcasts it as `XREAL.switchMode.down/up`);
  - the on-screen **⋯** and **B** are the Deck's;
  - **⌨** brings up Android's keyboard, which types into the focused window;
  - **◎** recentres.
- At the top, the sound output picker chooses where the placed sound plays.

**What Android needed** (all built from upstream source by scripts in `android/`, pinned):
- `xkbcommon/`: libxkbcommon and the keyboard layouts, since smithay's keyboard needs them.
- `eglshim/`: a `libEGL.so.1` that is Android's `libEGL.so`, since smithay opens EGL by the
  Linux name. The display itself is opened with `eglGetDisplay` and adopted with
  `EGLDisplay::from_raw`: Android returns an unusable display from
  `eglGetPlatformDisplayEXT`.
- `mysofa/`: libmysofa and the MIT KEMAR dataset (558 taps, the head SteamOS installs).
- Fonts come from `/system/fonts`, which fontdb does not search on Android.

**Measured on the device:**
- 59–60 fps at 3840×1080 stereo, with GPU about 50%.
- Remote windows from a host arrive with their Deck title bars and pictures, and come back
  on reconnect.
- The measured head loads.

**Not yet:**
- Android's own apps as windows.
- The microphone to a host (the input picker is for show).
- The Deck's own `crates/spatiand` changes (cfg gates only) have **not been compiled on the
  Deck**.

## Consequences for the design

- **Without shell privilege**, one app gets the sphere, head tracking and remote Linux apps:
  it owns the glasses display (Presentation) and the glasses' USB.
- **Shell privilege is needed only for** hosting other Android apps (TRUSTED virtual
  displays), injecting input into them and toggling Nebula. The placeholder is drawn
  over instead (above).
  Android revokes it at every reboot. Getting it back on the device itself (Shizuku) uses
  wireless debugging, which Android enables only while connected to Wi-Fi.
