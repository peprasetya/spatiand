# Spatiand's own applications

The plan for Spatiand's first-party applications, as agreed on 2026-10-13. Nothing here is
built yet. It is written so that a session which was not part of the discussion can build from
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

1. Android process model: bundled native executables run from the APK, or in-process — which
   decides how per-tab processes are done on the Beam Pro.
2. The browser, the music player, the Wi-Fi/Bluetooth manager.
3. The icons.

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
