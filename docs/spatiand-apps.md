# Spatiand's own applications

The plan for Spatiand's first-party applications, as agreed on 2026-10-13, and at its end what
has been built of it. It is written so that a session which was not part of the discussion can build from
it: what was decided, what each application must do, what already exists to build on, what is
still open, and the order to build in.

## Context

On the Deck, Spatiand is the compositor and any Linux app is a window. On the Mac and the
Beam Pro it is a guest: Mac windows are captured and clicked by proxy, Android's are not
windows at all. So the everyday things done in the glasses — files, video, pictures, a
terminal — need applications that are Spatiand's own: drawn straight into the room, the same on
all three platforms, and aware of 2D versus two eyes. One day this set may be the seed of a
SpatiandOS; that is far off and not a goal here.

## Decided

| | |
|---|---|
| The apps | **SpatiShelf** (files), **SpatiReel** (video), **SpatiLoupe** (viewer), **SpatiQuill** (terminal). Each has its own icon from the start. |
| Where apps run | **Always local**, where the glasses are. Data is local too, or reached by the app's own means: Jellyfin/Emby/Vibrefy servers, rclone remotes, ssh typed in a terminal. A paired spatiand-host plays no part in the apps. |
| Launcher | A **"Spatiand" entry, always first** (not in alphabetical order), which opens to the list of Spatiand apps. The machines after it — this one by its name (darkphoenix, steamdeck) and each host (deepmagpie) — behave exactly as now. |
| Wire | The **cut-down Wayland subset** the compositor already implements on all three platforms (core, xdg-toplevel, one buffer extension per platform, `spatiand_xr_v1`), hidden behind one SDK crate. Apps never see Wayland. |
| UI | **The room's own look, grown into a toolkit**: the renderer that draws Settings, the launcher and cards becomes a small widget set, stereo-aware, text tuned for the glasses. |
| 2D ⇄ stereo | The SDK reports the mode as it changes (the Mac goes 2D when the glasses are unplugged). Flat UI is unaffected; content with depth switches between one eye and two, live. |
| Crash isolation | Each app is its own process. **Tabs are processes too**: every SpatiQuill tab and every SpatiShelf tab runs in its own process, so one crashing does not take the others. |
| Media engine | **FFmpeg + each platform's hardware decoder** (VAAPI, VideoToolbox, MediaCodec). |
| Remote storage | **A storage service inside Spatiand** holds an rclone **fork, kept easy to rebase and linked in** (not a separate rclone driven from outside) and the list of remotes. Every app asks it for files; it also serves the WebDAV proxy for native apps. |
| No cache | **Nothing on disk.** Folder listings and thumbnails already fetched are held in memory while the app is open, with a refresh, and gone when it closes. File contents are always streamed from the remote. |
| SpatiReel's code | **The Vibrefy Rust port moves into the Spatiand repository** and becomes SpatiReel; Vibrefy is one server type among Jellyfin, Emby, DLNA and local. vibrefyApp keeps its Android app. |
| Viewer engine | **PDFium** for PDF; **our own small glTF PBR renderer** for 3D objects. |
| Beam Pro storage | All shared storage, through the "all files access" permission granted once. No root. |
| First app | **SpatiReel.** |
| Packaging | All apps ship inside Spatiand on each platform. SpatiWorld stays separate. |
| Android processes | **Bundled native executables**, shipped in the APK's library directory (as `lib<name>.so`) and started by the compositor, as on the Deck and the Mac: one way of starting an app everywhere, and a tab that crashes takes nothing with it. Android counts them as "phantom" child processes, so its monitor of those is to be switched off once over adb (no root). |

## Requirements per app

### SpatiReel — video
- Layouts: 2D, SBS and half-SBS, over-under and half-OU, VR180, VR360, VR180 SBS, and VR360
  stereo if a file ever turns up.
- Sources: **Vibrefy** (kept in step with the server; fix what the current app misses — random
  suggestions and the bugs around them), **Jellyfin** and **Emby** (both report play progress
  back), **local files**, **DLNA** browse and play (no progress saving).
- Servers are independent of any spatiand-host; several of any kind can be added.
- UI need not follow vibrefyApp: common and modern. Poster shape is **chosen from what the
  library or folder mostly has** (tall posters or 16:9 covers), not from the server type.
- Subtitles, text and image (PGS, VobSub), at a comfortable depth in 3D films.
- Surround beds placed in the room through Spatiand's HRTF, head-tracked.
- HDR10 and Dolby Vision's compatible layer tone-mapped for the glasses.
- Audio-track choice, playback speed, sync offsets for sound and subtitles.

### SpatiLoupe — media viewer
- Like Preview on a Mac: PDF, JPEG, PNG, HEIF, JPEG 2000, GLB — any still media that is not
  video or audio.
- Stereo photographs (samples to come from the Beam Pro's album).
- 3D objects with textures and PBR, in true stereo in the 3D room and flat in 2D.

### SpatiQuill — terminal
- A full terminal emulator, drawn directly in Spatiand, running the platform's standard shell.
- Tabs, each its own process.
- A tab can be dragged out into a new window, into another SpatiQuill window, or along the bar
  to reorder.

### SpatiShelf — file manager
- Works like Finder/Explorer, **with tabs, each its own process**.
- Opens a file with a Spatiand app or with a native app of the platform.
- rclone built in: a UI for configuring remotes, and remotes that feel local with no mounting
  step. Mounts are **virtual** — understood by Spatiand apps, not OS mounts.
- For native apps, a remote is reached through the **local WebDAV proxy**.

### Later / optional
- Browser: discussed after the others exist.
- Music player. A common Wi-Fi and Bluetooth manager.

## What already exists and should be reused

- **The in-process client path**: `crates/spatiand/src/remote/client.rs` (xdg toplevel +
  `spatiand_xr_v1`, zero-copy pictures), already run inside the compositor on the Mac and
  Android (`crates/spatiand-mac/src/mac/local.rs`). The SDK starts from this.
- **`spatiand_xr_v1`** (`crates/spatiand-proto/protocol/spatiand-xr-v1.xml`): eye layouts,
  equirect and projection layers, pose channel, idle fade.
- **The Vibrefy Rust port**, `/Users/paulus/Android/vibrefyApp/linux/crates/`: `vibrefy-media`
  (engine, mesh, layout, render, audio), `vibrefy-wl` (with `spatiand.rs`), `vibrefy-api`
  (client, progress, episodes, pairing), `vibrefy` (screens, tiles, nav).
- **The room's card and list renderer**: `spatiand-render::panel`, `crates/spatiand/src/scene.rs`,
  `crates/spatiand/src/menu.rs`, `spatiand-shell`.
- **The launcher's machine-first list** (`spatiand-shell` launcher; `local_apps()` per platform).
- **Per-window sound placement** (`spatiand-audio`), for SpatiReel's surround.

## Still open

1. The browser, the music player, the Wi-Fi/Bluetooth manager.
2. The icons: `assets/apps/*.svg` are stand-ins in the Spatiand icon's colours.

## Build order (for the session that builds)

1. `spatiand-app` SDK (window, input, GL, the 2D/stereo event, eye layouts) and the toolkit's
   first widgets (list, poster grid, tabs, text field, buttons).
2. The launcher's fixed "Spatiand" first entry and its list of apps.
3. SpatiReel on local files with every layout, from the Vibrefy port moved in.
4. Servers: Vibrefy, then Jellyfin and Emby with progress, then DLNA.
5. The storage service and SpatiShelf; then SpatiLoupe; then SpatiQuill.

## Verification

Each app runs unchanged on the Deck, the Mac (2D and glasses, switching live when the glasses
are unplugged) and the Beam Pro; snapshot pictures per screen; SpatiReel against real Vibrefy,
Jellyfin and Emby servers, a local file and a DLNA server, with progress checked on the server
afterwards.

## Built so far (2026-10-13, branch `spatiand-apps`)

Steps 1 to 3 of the build order, on the Mac. What each claim rests on is said, because most of
it has not been in front of anybody's eyes.

| | |
|---|---|
| `crates/spatiand-app` | The SDK: windows, input (keys through the compositor's keymap, with repeat), GL, eye layouts, layers, a window's shape apart from its picture's size (`wp_viewporter`), and `Mode` — one picture or two — as an event. `ui` is the room's look as widgets: list, poster grid, tabs, text field, button. `examples/gallery.rs` shows all of them. |
| `spatiand_xr_v1` version 6 | `display_mode` (flat / stereo), sent at bind and on change. The Deck's and the Beam Pro's backends say what they are presenting; **nothing yet says "flat" on a Mac when the glasses are unplugged**, because the Mac's 2D mode does not run the compositor at all (below). |
| A picture without a copy | Mac: drawn into a *global* `IOSurface`, named by number in the window's placeholder (`spatiand_proto::handover`, `mac/remote_video.rs`). Deck: a texture exported as a dmabuf (`gl/linux.rs`). Anywhere it fails, and on Android for now, the picture is read back and sent as `wl_shm`. |
| Launcher | `Level::Own`, the "Spatiand" bubble always first; `crates/spatiand/src/own.rs` lists the programs found beside the compositor (or in `SPATIAND_APPS_DIR`) and starts them. `mac/make-app.sh` puts SpatiReel in the bundle. |
| `crates/spatiand-media` | `vibrefy-media` moved in: `layout`, `hints`, `audio`, `mesh` as they were (hints now read FFmpeg's stereo and spherical side data), and a new engine on FFmpeg in place of libmpv — `player` (reader, picture and sound threads, the sound's clock, seeking, sound-track choice), `sound` (cpal), `screen` (planes to colour in a shader, and a view from inside a sphere). VideoToolbox pictures stay on the GPU. |
| `crates/vibrefy-api` | Moved in unchanged, for step 4. Taken from vibrefyApp's working tree at `254e043` plus its uncommitted changes. `vibrefy-wl` and the egui screens were not moved: the SDK and the toolkit replace them. |
| `crates/spatireel` | A library of this machine's folders and films -- each film's tile a frame of it, made on a thread and held in memory only -- and a film playing: flat, side by side and over-under (full and half), VR180 and VR360 (one eye, side by side, stacked). Flat films are a window whose eyes the room samples from the file's own frame; spheres are the room's sky with the window as controls; with one picture, or the sky refused, one eye in the window and a sphere looked about in by dragging. A menu corrects what a film was taken to be and chooses its sound track. |

**Checked**, all on an Intel Mac with the snapshot backend (`SPATIAND_CLIENT=… spatiand-mac`),
which runs the real compositor and the real application as two processes and draws one eye to
a PNG: the gallery's widgets, clicks and keys; the launcher's top level and the Spatiand level;
every layout above, left eye and right; one-picture mode (`SPATIAND_SNAPSHOT_MODE=flat`); the
sky at several headings; the menu, seeking, pausing, going back to the library; software and
VideoToolbox decoding. Unit tests for the launcher, the layouts and hints, the colour matrix,
the library and the staging table.

**Not checked**: anything on real glasses, on any platform. Sound, beyond its clock running
(tests are muted with `SPATIAND_MEDIA_MUTE=1`); lip-sync by ear. The Deck beyond this: the
compositor with these changes was built there (in `/home/deck/spatiand-apps`, not the tree the
session runs) and the gallery drawn through its snapshot backend, pictures going over as
dmabufs -- but SpatiReel and `spatiand-media` have not been built on the Deck, and nothing of
this is deployed to it. The Beam Pro: nothing built.

**Not done yet, of steps 1 to 3**
* The Mac without glasses: its 2D mode is the Swift app with no compositor, so there is nothing
  for an application to connect to. Either the compositor runs there too with its windows shown
  as Mac windows, or the apps get a second way in; this meets the other session's work on
  `mac-compositor` and wants deciding with it.
* Android: the programs in the APK (`android/build.sh`), FFmpeg built for it, MediaCodec, an
  `AHardwareBuffer` handed over the socket, and `SPATIAND_APPS_DIR` set from the native library
  directory.
* The Deck: VAAPI pictures kept on the GPU (they are decoded in software now); sound through
  cpal's ALSA has not been tried against Spatiand's per-window sinks; installing the programs
  beside the compositor.
* SpatiReel: subtitles; HDR; surround placed in the room (sound is whatever the device's default layout is);
  playback speed and sync offsets; VR360 looked about in with the keys; on a Mac the room does
  not yet place an application's sound at its window.
* The pose channel is not in the SDK yet (SpatiLoupe's 3D objects will want it).
