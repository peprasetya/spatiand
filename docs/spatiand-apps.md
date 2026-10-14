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

## Built so far (2026-10-13/14, branch `spatiand-apps`)

All five steps of the build order (step 5 is in its own section below). Steps 1 to 4: the SDK and toolkit, the launcher's entry, and SpatiReel with
local files and all four kinds of server. What each claim rests on is said, because most of it
has not been in front of anybody's eyes.

| | |
|---|---|
| `crates/spatiand-app` | The SDK: windows, input (keys through the compositor's keymap, with repeat), GL, eye layouts, layers, a window's shape apart from its picture's size (`wp_viewporter`), and `Mode` -- one picture or two -- as an event. `ui` is the room's look as widgets: list, poster grid, shelf (a row of tiles), tabs, text field (plain or secret), button; gradients and glows. `examples/gallery.rs` shows them. |
| `spatiand_xr_v1` version 6 | `display_mode` (flat / stereo), sent at bind and on change. |
| A picture without a copy | Mac: drawn into a *global* `IOSurface`, named by number in the window's placeholder (`spatiand_proto::handover`, `mac/remote_video.rs`). Deck: a texture exported as a dmabuf (`gl/linux.rs`). Anywhere it fails, and on Android for now, the picture is read back and sent as `wl_shm`. |
| Launcher | `Level::Own`, the "Spatiand" bubble always first; `crates/spatiand/src/own.rs` lists the programs found beside the compositor (or in `SPATIAND_APPS_DIR`) and starts them. `mac/make-app.sh` puts SpatiReel in the bundle. On a Mac, Control-Option-W closes the window that has the keyboard. |
| `crates/spatiand-media` | `vibrefy-media` moved in (`layout`, `hints`, `audio`, `mesh`), and an engine on FFmpeg in place of libmpv: `player` (reader, picture, sound and subtitle threads; the sound's clock; seeking; track choice; speed; a lip-sync offset), `sound` (cpal on a Mac; a `pw-cat` of its own on Linux, its channels named so the room's sink places a 5.1 or 7.1.4 film's channels), `screen` (planes to colour in a shader, HDR10/HLG brought down to SDR, a view from inside a sphere), `subs` (ASS text, picture subtitles, `.srt`), `stretch` (speed without a change of pitch), `still` (one frame, for a tile). Pictures stay on the GPU with VideoToolbox, and with VAAPI for H.264. |
| `crates/vibrefy-api` | Moved in unchanged, from vibrefyApp's working tree at `254e043` plus its uncommitted changes. `vibrefy-wl` and the egui screens were not moved: the SDK and the toolkit replace them. |
| `crates/spatireel` | **Home**: what is part watched, from every source at once, each with its picture and whose it is; then the sources. **Sources** (`source/`): this machine's folders (up to the root; `.strm` pointers played; where each film was left is kept), Jellyfin and Emby (sign in by name and password; a token is kept, never the password; posters; `static` streams so nothing is transcoded; the server's own word for a 3D film's packing; play, progress and stop reported), Vibrefy (paired by code; browse; streams; progress), DLNA (found by SSDP; browsed; played; no place kept). **A film**: every layout; a menu for layout, sound track, subtitles, speed and sync; resumed where it was left. Tiles are posters or frames by what the folder's pictures mostly are. Lists and pictures are fetched on threads and held in memory only. What is kept is `~/.config/spatiand/spatireel.json`. |
| `tools/fake-jellyfin.py`, `fake-vibrefy.py`, `fake-dlna.py` | Stand-ins for the servers, to look at SpatiReel without one. |

**Checked**, on an Intel Mac with the snapshot backend (`SPATIAND_CLIENT=... spatiand-mac`),
which runs the real compositor and the real application as two processes and draws one eye to
a PNG, driven by scripted clicks and keys:
the widgets; the launcher's levels; every layout, left eye and right; one-picture mode; the sky
at several headings; menus, seeking, pausing; the owner's own folders from `~/Movies` up to `/`
and a real twenty-minute film with far seeks; signing in to the stand-in Jellyfin by typing
into the form, and to the stand-in Emby; pairing with the stand-in Vibrefy; discovering,
browsing and playing from the stand-in DLNA server; each server being told where a film was
left, and the home screen then offering it; subtitles from an `.srt` in each eye of a 3D film
and from a film's own track; the place a film at 2x and 0.75x has got to. An HDR10 file made
from an ordinary picture comes out within a little of that picture. On deepmagpie (Linux,
Radeon): SpatiReel builds; one frame of an H.264 film decoded by VAAPI and drawn from its
dmabufs matches the same frame decoded in software. On the Deck, in a tree that is not the
session's: the compositor with the SDK's changes builds and the gallery draws through its
snapshot backend, pictures going over as dmabufs.

**Not checked**
* Anything on real glasses. The owner opened SpatiReel in the Mac room once, on the first
  build, and browsed (finding two crashes, since fixed); no film has been watched in glasses.
* **Any real server.** There is no Jellyfin, Emby, Vibrefy or DLNA server this session could
  reach; the stand-ins answer what the documentation says the real ones answer. Expect
  differences.
* Sound, by ear: lip-sync, the stretcher's quality, surround on the Deck (`pw-cat` with named
  channels has never run in a Spatiand session).
* SpatiReel on the Deck at all: built only on deepmagpie. Nothing is deployed to the Deck.
* Picture subtitles (PGS, VobSub) on a real disc's track: the code path has not had one.
* HEVC through VAAPI: **it is switched off on Linux**, because one HEVC file (encoded by a
  Mac) hung deepmagpie's GPU decoder when given to VAAPI from the test tool, and the kernel
  reset the GPU. HEVC is decoded in software there until that is understood
  (`SPATIAND_MEDIA_HARDWARE=all` overrides).

**Not done yet**
* The Mac without glasses: its 2D mode is the Swift app with no compositor, so there is nothing
  for an application to connect to, and nothing says "flat" when the glasses are unplugged.
  This meets the other session's work on `mac-compositor` and wants deciding with it.
* The Beam Pro: the programs in the APK (`android/build.sh`), FFmpeg built for Android,
  MediaCodec, an `AHardwareBuffer` handed over the socket, `SPATIAND_APPS_DIR` set from the
  native library directory, and the phantom-process monitor switched off.
* Subtitles while a film is the sky (they are drawn over a window, and the sky is not one).
* Dolby Vision that has no HDR10-compatible layer (profile 5) is shown with the wrong colours.
* Signing in to Vibrefy by any way but a code.
* On a Mac the room does not yet place an application's sound at its window.
* The pose channel is not in the SDK yet: SpatiLoupe's objects are drawn in stereo for an
  assumed window (1.3 m wide, 1.6 m away), not from the head's real place.

## Step 5, built 2026-10-14: the storage service, SpatiShelf, SpatiLoupe, SpatiQuill

| | |
|---|---|
| `storage/` (`spatiand-storage`) | Go, with **rclone v1.72 linked in as a library** (every backend). Remotes in Spatiand's own `~/.config/spatiand/rclone.conf`; the user's own rclone remotes are copied in only when asked ("From rclone"). Answers on 127.0.0.1 behind a token kept in `storage.token` beside the remotes: `/rc/<method>` is rclone's remote control (lists, copies, moves, deletes, `config/create` and its questions); `/dav/<remote>/...` is each remote as WebDAV, for streaming and for native applications. rclone's file cache is **off**: nothing is kept on disk. Where it is: `$XDG_RUNTIME_DIR/spatiand-storage.json`. Started by the first application that wants it, detached; a second start finds the first. `storage/build.sh mac|linux|android` builds it from the Mac for all three (no cgo). *The "fork" is upstream v1.72 as a Go module: no patch has been needed; a `replace` line in `go.mod` makes it a fork the day one is.* |
| `crates/spatiand-storage` | Its Rust client. `remote:path` is a **virtual place** to every application; files stream by URL (FFmpeg) or are read into memory. |
| `crates/spatishelf` | **Every tab a process** (`spatishelf --tab`): it lists, makes pictures (images, film frames, HEIF; remote pictures read into memory) and copies. Sidebar of places (per platform; `/sdcard` places on Android) and remotes; tiles or a list; several chosen (click with Control/Shift, Space, Control-A); path bar; back, forward, up; type to jump, Control-F to filter; copy, cut, paste (local by itself, anything with a remote through rclone, as a job with a progress bar); rename, new folder, trash (the Mac's, freedesktop's on Linux; remotes and Android delete for good, after asking); a menu on the right button; open with SpatiLoupe, SpatiReel, SpatiQuill (folders) or the platform's own (`open`, `xdg-open`; a remote's file by its WebDAV address). **Add remote**: every kind rclone has, searchable, its main options as a form, then rclone's own questions one at a time (OAuth kinds open the browser and wait). "Other apps": the WebDAV address, name and password, and a button that mounts it in Finder (Mac) or opens it in the file manager (Linux). |
| `crates/spatiloupe` | Pictures by the `image` crate, and HEIF (a phone's tile grid put together), AVIF and JPEG 2000 by FFmpeg; SVG by resvg; moving GIF, APNG, WebP; EXIF shown; zoom about the pointer, pan, rotate, a strip of the folder's other files, an info pane; controls on glass that fade. **Stereo photographs**: MPO pairs, side by side, crossed, over and under, told by name (`_sbs`, `.jps`...) or by the picture itself (the two halves' detail compared), squeezed pairs recognised by a film's shape; in the room each eye is given its view, flat the left one; the packing can be changed, eyes swapped, depth moved (`,` `.`). **PDF** by PDFium (`tools/fetch-pdfium.sh`, shipped beside the programs): pages down a column drawn at the size shown, search with the results marked, the outline. **glTF/GLB** by its own renderer: metallic-roughness PBR (base colour, metal-rough, normal, occlusion, emission, cut-out and see-through), lit by three lights and a studio sky, a soft shadow, 4x MSAA; in the room drawn for each eye with off-axis frusta; drag to turn, scroll to near. Remote files (`remote:path`) and folders open as local ones do. |
| `crates/spatiquill` | **Every tab a process** (`spatiquill --tab`) holding a pseudo-terminal and alacritty's emulator (`alacritty_terminal` 0.26), sending its screen to the window; one process holds every window, and a second start opens a window in it. Glyphs from the platform's monospaced face through cosmic-text into one atlas, the whole screen one draw: colour (16, 256, true), bold, italic, underline, emoji, wide CJK. Tabs named by the running program or the folder; dragged along the bar to reorder, **off the window into a window of its own, or onto another SpatiQuill window**. Selection copies (to the Mac's clipboard by `pbcopy`, to Wayland's by `wl-copy` if it is there); paste; scrollback with a marker; mouse to programs that ask for it; a crashed tab offers a new shell and the others go on. |
| SDK | `App::watch(fd)`: an application woken for its own files. `ui::look` (backdrop, rule, glass, chip) shared by all four. |
| Snapshot backend | `SPATIAND_STEPS`: clicks, drags (past the window's edge too), scrolls, keys with modifiers, typed text, waits, `window:<title>` and `dump:<png>` of the window's own picture at any point -- how all of the above was driven. |
| Packaging | **Mac**: `mac/make-app.sh` puts all four, the storage service and PDFium in the bundle. **Deck**: all four build in the `holo` container; PDFium and the storage service are copied beside them. **Beam Pro**: `android/build.sh` builds FFmpeg 7.1 for Android (`android/ffmpeg/`, MediaCodec), the four as `lib<name>.so`, PDFium and the storage service into the APK; the compositor finds them in the APK's library folder and gives them `LD_LIBRARY_PATH`; `android/setup.sh` grants all-files access and switches the phantom-process monitor off. |

**Checked** (Mac, snapshot backend with `SPATIAND_STEPS`, the real compositor and the
applications as their own processes): SpatiLoupe's folder grid, JPEG, HEIC, JPEG 2000, SVG,
GIF, a stereo pair to each eye and flat, zoom and pan, rotation, the info pane, a 36-megapixel
HEIC, PDF pages, search and its marks, three Khronos sample GLBs in both eyes, dragged and
zoomed, a remote's PDF and folder. SpatiQuill: zsh with colour, emoji and CJK, two and three
tabs, vim on the other screen, `kill -9` of one tab's process (the others went on), reorder,
tear-off into a window, a tab dropped onto another window, selection, scrollback.
SpatiShelf: home, tiles, list, menu, a remote (alias and WebDAV stand-ins) with pictures,
**a WebDAV remote added through the form** and browsed, the "other apps" pane, a file copied
from this machine to the remote by copy and paste in a second tab (it arrived); SpatiReel
playing a film from that remote by the WebDAV address SpatiShelf hands it. The storage
service by hand: remotes made, listed, range reads and PROPFIND over WebDAV, refused without
the token. The launcher's Spatiand bubble lists all four from the installed app.
**On the Deck** (snapshot backend, headless, outside the session): SpatiQuill with bash,
SpatiLoupe with a GLB in both eyes, HEIC and a PDF, SpatiShelf with pictures.

**Not checked**
* Anything in glasses, on any platform; sound by ear.
* **The Beam Pro at all**: it did not answer adb, so the APK is built and nothing in it has
  run. Unknowns there: the programs starting from the library folder, FFmpeg's MediaCodec,
  sound through cpal/oboe from a program with no Java, a pseudo-terminal under the app's uid,
  `/sdcard` with all-files access, HOME for the programs (the app's files folder).
* Stereo photographs from a real stereo camera or the Beam Pro's album: the pairs checked were
  made from an ordinary picture. The detection rule may need tuning on real ones.
* Any real cloud remote (Google Drive and the like, and their browser sign-in): only stand-ins.
  Native applications through the WebDAV folder (Finder's mount) were not tried.

**Not done yet**
* Clipboard on the Deck and the Beam Pro beyond `wl-copy` if present: the SDK has no
  `wl_data_device` yet.
* SpatiShelf: dragging files between tabs or onto remotes (copy and paste only); opening in
  Android's own applications.
* SpatiLoupe: Apple's spatial HEIC (two images in one file) shows its first eye only; glTF
  animation and skinning are not played.
* SpatiReel's own browsing of remotes (it opens what SpatiShelf hands it).
