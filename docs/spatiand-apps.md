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
* ~~The Mac without glasses~~: done 2026-10-17. In 2D mode the app starts the compositor for
  them and shows each window as a Mac window (docs/mac.md, "2D mode"); the mode is flat there.
  Unplugging the glasses still restarts the app, which ends the applications.
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
| `storage/` (`spatiand-storage`) | Go (the toolchain pinned by the `toolchain` line in `storage/go.mod`, which `go` fetches on any machine), with **rclone v1.76 linked in as a library** (every backend). Remotes in Spatiand's own `~/.config/spatiand/rclone.conf`; the user's own rclone remotes are copied in only when asked ("From rclone"). Answers on 127.0.0.1 behind a token kept in `storage.token` beside the remotes: `/rc/<method>` is rclone's remote control (lists, copies, moves, deletes, `config/create` and its questions); `/dav/<remote>/...` is each remote as WebDAV, for streaming and for native applications. rclone's file cache is **off**: nothing is kept on disk. Where it is: `$XDG_RUNTIME_DIR/spatiand-storage.json`. Started by the first application that wants it, detached; a second start finds the first. `storage/build.sh mac|linux|android` builds it from the Mac for all three (no cgo). *Upstream v1.76 as a Go module, not a fork: no patch has been needed; a `replace` line in `go.mod` makes it a fork the day one is.* |
| `crates/spatiand-storage` | Its Rust client. `remote:path` is a **virtual place** to every application; files stream by URL (FFmpeg) or are read into memory. |
| `crates/spatishelf` | **Every tab a process** (`spatishelf --tab`): it lists, makes pictures (images, film frames, HEIF; remote pictures read into memory) and copies. Sidebar of places (per platform; `/sdcard` places on Android) and remotes; tiles or a list; several chosen (click with Control/Shift, Space, Control-A); path bar; back, forward, up; type to jump, Control-F to filter; copy, cut, paste (local by itself, anything with a remote through rclone, as a job with a progress bar); rename, new folder, the trash (see "2026-11-15" below: each system's own, a Trash place, put back, empty; remotes delete after asking); a menu on the right button; open with SpatiLoupe, SpatiReel, SpatiQuill (folders) or any of the platform's applications ("Open with", 2026-11-15). **Add remote**: every kind rclone has, searchable, its main options as a form, then rclone's own questions one at a time (OAuth kinds open the browser and wait). "Other apps": the WebDAV address, name and password, and a button that mounts it in Finder (Mac) or opens it in the file manager (Linux). |
| `crates/spatiloupe` | Pictures by the `image` crate, and HEIF (a phone's tile grid put together), AVIF and JPEG 2000 by FFmpeg; SVG by resvg; moving GIF, APNG, WebP; EXIF shown; zoom about the pointer, pan, rotate, a strip of the folder's other files, an info pane; controls on glass that fade. **Stereo photographs**: MPO pairs, side by side, crossed, over and under, told by name (`_sbs`, `.jps`...) or by the picture itself (the two halves' detail compared), squeezed pairs recognised by a film's shape; in the room each eye is given its view, flat the left one; the packing can be changed, eyes swapped, depth moved (`,` `.`). **PDF** by PDFium (`tools/fetch-pdfium.sh`, shipped beside the programs): pages down a column drawn at the size shown, search with the results marked, the outline. **glTF/GLB** by its own renderer: metallic-roughness PBR (base colour, metal-rough, normal, occlusion, emission, cut-out and see-through), lit by three lights and a studio sky, a soft shadow, 4x MSAA; in the room drawn for each eye with off-axis frusta; drag to turn, scroll to near. Remote files (`remote:path`) and folders open as local ones do. |
| `crates/spatiquill` | **Every tab a process** (`spatiquill --tab`) holding a pseudo-terminal and alacritty's emulator (`alacritty_terminal` 0.26), sending its screen to the window; one process holds every window, and a second start opens a window in it. Glyphs from the platform's monospaced face through cosmic-text into one atlas, the whole screen one draw: colour (16, 256, true), bold, italic, underline, emoji, wide CJK. Tabs named by the running program or the folder; dragged along the bar to reorder, **off the window into a window of its own, or onto another SpatiQuill window**. Copy and paste by keys, through the room's clipboard (see "SpatiQuill's copy and paste" below); scrollback with a marker; mouse to programs that ask for it; a crashed tab offers a new shell and the others go on. |
| SDK | `App::watch(fd)`: an application woken for its own files. `ui::look` (backdrop, rule, glass, chip) shared by all four. |
| Snapshot backend | `SPATIAND_STEPS`: clicks, drags (past the window's edge too), clicks through the room's own pointer under a trembling aim (`shaky:u,v,degrees,clicks`), scrolls, keys with modifiers (`rctrl` the right Control, which is how a Mac's Command reaches the clients), typed text, waits, `window:<title>`, `dump:<png>` of the window's own picture at any point, and `clipboard`, which logs what is on the room's clipboard -- how all of the above was driven. |
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
* SpatiShelf: opening in Android's own applications.
* SpatiLoupe: Apple's spatial HEIC (two images in one file) shows its first eye only; glTF
  animation and skinning are not played.
* SpatiReel's own browsing of remotes (it opens what SpatiShelf hands it).

## 2026-10-27: SpatiShelf as a file manager; dragging between windows

**SpatiShelf.** Things are dragged onto a folder, a place along the side, a tab (which comes
forward when held over) or a step of the path: moved within a disk, copied to another disk or a
remote, Alt copying and Shift moving whatever. A band drawn from an empty part chooses what it
touches. A name is renamed where it is, as in Finder: a click on the name of the one thing
chosen, F2, or the menu; the name but its ending is chosen to be typed over. The right button's
menu is for what it is on (open in a new tab, paste into this folder, move to..., copy to...,
duplicate, rename, the trash, the path) or for the folder itself (new folder, paste, list or
tiles, sort, hidden files, SpatiQuill here), with icons and each one's keys; Control-click on a
Mac, the menu key, Shift-F10 and a press held still open it too. Sorting by name (numbers by
their value), kind, size or date, folders first; the list's heads sort.

**Dragging between windows, anywhere in the room.** An application starts a drag with what it
carries in as many forms as it likes and a picture (`App::start_drag`); the room draws the
picture on a card hanging from the pointer, a little in front of whatever window it is over, and
the window the ray is on hears it come, move and be let go of at the very point of it the ray
meets (`Event::DragEntered`, `DragMoved`, `Dropped`), says what it would do there
(`App::accept_drag`) and reads it (`App::drop_data`). SpatiShelf offers places (to another
SpatiShelf), `file:` addresses (to anything; a remote's by its WebDAV address) and text, and takes
them onto the folder under the point they are let go of; SpatiMesh takes files and links, opening
them in a new tab when let go of over its tabs and in the page when over the page. Checked in the
snapshot compositor: a file dragged from one SpatiShelf's window onto a folder in another's, and
from SpatiShelf onto SpatiMesh's tabs (Chromium's since 2026-11-10: the Rust window was removed;
`tools/spatiand-dnd-e2e.py` with `SPATIAND_CHROMIUM_BIN`). Not yet: dragging out of a page (WebKit's own drags), and
into or out of a Mac's or a host's own windows.

## 2026-11-14: SpatiShelf's list opens folders where they are (Finder's list)

The owner: "in list view, folder/sub-folder can be expanded like Apple Finder, so manage file/move
file can be drag and drop in one window."

**Opening.** Each folder's line has a triangle at its left (drawn: `Icon::Disclose` and
`Icon::Disclosed` of the SDK, not a font's). A click on it opens the folder where it is: what it holds
comes under it, one step in (30 px), to any depth; the other columns stay where they are. Right opens
the folder at the cursor, Left closes it (or, from inside an opened folder, goes to that folder); a
double click or Enter still goes into a folder. Closing a folder with the cursor inside it brings the
cursor to the folder. What is open is kept per tab by each folder's whole place: a folder opened
inside one that is closed is open again when that one is; gone into a folder and back, what was open
there is open again (for this run of the window, not after a restart). The tiles open nothing.

**Listing** (`crates/spatishelf/src/tree.rs`, plain logic with its tests). A folder is listed only
when it is opened and on show (`ToTab::List`, answered by `FromTab::Contents`, on a thread of the
tab's so it does not hold up the rest): never ahead and never all the way down, which matters on a
remote. Until it comes a "Loading..." line stands under the folder; one that cannot be listed says
why in red ("Could not be listed: This folder may not be opened."); an empty one says "Empty".
Closing a folder forgets what was in it and stops waiting for a listing on its way (it is dropped when
it comes). When the folder on show is listed again (Control-R, or after anything is done to files)
every open folder on show is listed again too, the old lines staying until the new come, and the
cursor, the chosen things and the scroll stay where they were. Sorting (the heads, the menu) orders
each folder's things under it, folders first. The filter typed (Control-F) is for the folder's own
things; hidden files are hidden all the way down. "N items" counts the folder's own. Several can be
chosen across folders, as before (Control or Shift click, Space, Control-A, a band). Duplicate puts
each copy beside its own original. Pictures of pictures and films are made for the folder's own
things only; things in opened folders show their kind's mark.

**Dragging within the window** (a press on a line and a move of 7 px, as before: the room's pointer
on every platform, the Beam Pro's screen included). Onto a folder's line: into it, the line lit. Onto
a file (or a note) in an opened folder: into that folder, its line and everything under it lit. Onto
a file at the top, the space below the last line or the heads: into the folder on show, the pane lit
-- how something is brought up out of an opened folder. A closed folder held over for a second opens
(Finder's spring-loading). Near the top or foot of the pane it scrolls. Onto itself, into the folder
it is already in, or into a folder inside itself is not taken and says so at the pointer ("Not onto
itself", "Not into itself", "Already in Alpha"); of several from different folders, those already
where they are let go of stay. Moved within a disk; **copied** to another disk or remote, or with
**Alt or Control** held (Control as the file managers of Linux have it, Alt as Finder's Option); Shift
(Command on a Mac) moves whatever -- one rule on every platform, and the same rule as dragging onto
the side, a tab or the path. A move between two places of one remote is rclone's move; between
storages, as the menu's Move to and Copy to do. Escape or the right button calls a drag off. Dragging
out to other windows and applications is as it was (`App::start_drag`).

**Checked** in the snapshot room on Linux (deepmagpie, scratch HOME/XDG_*/`SPATIAND_APPS_CONFIG`, a
scratch tree): a folder and a folder in it opened, an empty one, one that may not be opened (the
pictures showed the triangles, the steps in and the columns in line); a file dragged from an opened
folder onto a closed folder's line (it sprang open; moved on disk, both folders' lines updated, all
else still open); onto its own folder and a folder into a folder in itself (not taken, said so,
nothing changed on disk); two chosen in different folders dragged onto a third (both moved); a file
copied into an opened folder with Control held during the drag (both copies on disk); a file dragged
out of an opened folder onto the empty space (moved up); Left and Right closing and opening again with
what had changed. With the storage service on an alias remote of a scratch folder: two folders opened
and a file dragged from one onto the other (moved there on disk). **Not checked**: in glasses, the
Beam Pro, the Mac, a real cloud remote (slow listings), a folder of thousands opened.

## 2026-11-15: SpatiShelf's trash, folders that keep themselves up to date, renaming with the pointer, Open with

The owner: "add trash in places, and allow to clear the trash. Trash mechanism is follow the operating
system. On deck/linux, If there is no trash mechanism then make it"; "Operating between windows. May
not update the other window ... something mechanism to auto refresh the folder. That being opened";
"Renaming file. The pointer is useless. Cannot select, cannot unselect ... I want the pointer still
useful"; "Open with Mac app, Or Android app or Deck app ... I wish I can select the application that
will be used to open."

**The trash** (`crates/spatishelf/src/trash.rs`, plain file work with its tests; `trash_mac.rs`).
"Trash" is the last of the places along the side, on every platform. It lists what is in the trash,
each by its own name, **from** where (its folder, `~` for home) and when it was **deleted**, as a
list or tiles, and its pictures; a folder in it opens where it is and is counted whole. Its menu: Put
back (to where it was; a folder it was in that has gone is made again; something there by its name
already, it comes back beside it numbered "a 2.txt"), Open, Open with, Copy, Move to / Copy to (moved
out, what the trash knew of it goes too), Delete for good. "Empty" in the head (and "Empty the
Trash..." in the menus of the pane and of its place) asks first: "Empty the Trash? All 5 items in it
(26 bytes) will be deleted for good. This cannot be undone." Delete and the menu's "Move to the
Trash" put things in it (a drop onto Trash too); **Shift-Delete** (Shift- or Option-Command-Backspace
on a Mac) and the menu's "Delete for good..." delete after asking, "This cannot be undone."; in the
trash, Delete is for good, after asking. Nothing is pasted into it and nothing in it is renamed.
- **Linux, the Deck**: the freedesktop.org Trash specification, which Dolphin and every Linux file
  manager follow, so that it is one trash both ways: `$XDG_DATA_HOME/Trash` (`~/.local/share/Trash`)
  with `files/` and `info/<name>.trashinfo` (Path URL-escaped, DeletionDate in local time); the name
  claimed by making its info file first (`O_EXCL`), numbered on a clash. Another disk's things go into
  that disk's own `$topdir/.Trash/$uid` (an administrator's, sticky, not a link) or `$topdir/.Trash-$uid`,
  made when first wanted -- the spec's own rule for a machine with no trash yet; a disk where neither
  can be made has its things copied into the home trash and taken off it. Every disk's trash is
  listed (from `/proc/self/mounts`; network disks left alone, a look at one gone away can hang).
  `directorysizes` is not written (a cache; readers check its dates).
- **Android, the Beam Pro**: Android has no trash for files (MediaStore's is for its own media), so
  SpatiShelf keeps one in the same format at the top of each storage volume,
  **`/storage/emulated/0/.Trash-spatiand`** (what `/sdcard` is) and `/storage/XXXX-XXXX/.Trash-spatiand`
  on a card: on the volume, so that putting something in it is a rename and not a copy; in shared
  storage, so that it outlives the app being installed again (the app's own folders do not); named
  for Spatiand rather than for the uid, which Android changes on a new install; hidden, with a
  `.nomedia`, so that the gallery does not show what is in it. Paths in its info files are from the
  volume's top, as the spec has a disk's.
- **The Mac**: Finder's Trash. Put there by `NSFileManager trashItemAtURL` (what Finder does, so
  Finder's Put Back works for it: the system writes Finder's record for it too), listed from
  `~/.Trash` and each `/Volumes/*/.Trashes/<uid>`, emptied by deleting what is in them. **Where each
  thing came from is read from the Trash's `.DS_Store`**, as Finder wrote it: `ptbL`, the folder from
  the top of the disk (`private/tmp/sstest/files/`; `/` for the home Trash, `/Volumes/<disk>` for a
  disk's), and `ptbN`, the thing's own name when the Trash renamed it ("a.txt 17-56-07-829.txt" is
  shown and put back as "a.txt"). The reader (`ds_store.rs`) is SpatiShelf's own, read-only, from the
  format's notes (the `Bud1` buddy allocator, its `DSDB` B-tree of records keyed by file name, a
  four-letter code and a typed value), defensive -- every length checked, no node twice, no deeper
  than the tree says -- and tested on every system (Finder's own file rebuilt byte for byte, a tree
  with inner nodes and every kind of value, every byte of a file damaged in turn). SpatiShelf's own
  memory (`~/.config/spatiand/local/spatishelf-trash.json`) is kept too and comes first: it is known
  before Finder's record is written, and has when the thing was put there; each memory now holds the
  thing's inode, so that something else later in the Trash by the name is not taken for it (older
  memories without one are still read). **After SpatiShelf puts back something Finder put there,
  Finder's record of it stays in the `.DS_Store`** (SpatiShelf never writes the file): Finder tolerates
  that -- a record of nothing is ignored, and the next thing by the name gets a new one -- and so does
  this reader, which only looks up what is in the Trash. macOS keeps `~/.Trash` from programs without
  Full Disk Access: then the Trash says so ("Allow Spatiand in System Settings, Privacy & Security,
  Full Disk Access") rather than looking empty. Checked in the snapshot room (littleotter): a scratch
  file trashed by the system's trash call from outside SpatiShelf (so only Finder's record knew it)
  shown "from /private/tmp/sstest/files" and put back; a second by the same name (renamed in the
  Trash) shown by its own name and put back as "zz-finder-test 2.txt" beside the first; one trashed
  by SpatiShelf put back from its memory. Not checked: a disk's `.Trashes` (no other disk on that Mac).
- **Remotes** have no trash here: Delete asks first, and says where it goes -- "It goes to Google
  Drive's own trash, not to this machine's Trash." for the kinds rclone deletes into their own trash
  by default (Drive, OneDrive, Box, pCloud, MEGA, Dropbox), "for good? This cannot be undone." for the
  others. A remote's things dropped onto Trash are not deleted (said so): only from their menu.

**Folders that keep themselves up to date** (`watch.rs`). The folder on show and every folder
opened in place in the list are watched, by the tab's process, through one mechanism on every
system, the `notify` crate (inotify on Linux and Android, FSEvents on the Mac), each folder by
itself; a change from anywhere -- another SpatiShelf window, another program, the trash -- lists it
again a quarter of a second after the last change of a burst, at most once a second while one goes
on, keeping the cursor, what is chosen, the scroll and what is open, as any listing again does;
pictures of what did not change stay (no flicker). Watches go when a folder is closed, goes off show
or the tab goes elsewhere. The Trash place watches every trash's `files` and `info`. A remote cannot
be watched: the remote folder on show is listed again **every 15 seconds** (not under a drag, a band
or a name being typed), and at once after SpatiShelf itself did anything to it, as before.

**Renaming with the pointer** -- in the SDK, so every application's text field has it
(`spatiand_app::ui::Field`): a click puts the caret there and lets go of what was chosen, a drag
chooses (either way), two clicks choose a word, three everything, Shift-click chooses from what was
chosen (or the caret) to there. The click is put at its character when the field is next drawn
(only the painter can measure text; the nearest boundary is found by halving, a few measurements
for any length). `Input::Press` and `Event::Button` carry the keys held as the button went
(`modifiers`), for Shift-click; every application's patterns were given `..`. In SpatiShelf a click
outside the field ends the rename and keeps the name typed, as Finder does, and the click does what
it does; Escape puts the name back. The fields of SpatiShelf's remote form, SpatiReel's sign-in,
SpatiKey, SpatiPal and SpatiLoupe's find have it too, since they hand the field all their input;
SpatiChoose's field is given keys only.

**Open with** (`openwith.rs`, the menu `opening.rs`). "Open with..." in a thing's menu lists the
applications of this machine that open its kind, with their icons and names: the one a double click
uses first, marked "default" -- what was chosen to always open the kind, else Spatiand's own
(SpatiLoupe, SpatiReel, SpatiQuill for folders), else the system's own choice -- then Spatiand's
own, then the system's (its default first); "Other..." (every application); "Always open .jpg files
with the one chosen" (on, the next one chosen is remembered for the kind, **on this machine only**:
`~/.config/spatiand/local/spatishelf-open-with.json`; a double click and Enter then use it); and,
when one is remembered, "Forget X for .jpg files". Several chosen are all opened with the one chosen.
- **Linux, the Deck**: the freedesktop way (`openwith_linux.rs`): the MIME type from shared-mime-info's
  `globs2` (heaviest, longest glob; a file it does not know is `text/plain` if it reads as text) and
  its parents (`subclasses`, `aliases`, `text/*` is `text/plain`); the desktop entries under
  `$XDG_DATA_HOME` and `$XDG_DATA_DIRS` **and Flatpak's exports** (`~/.local/share/flatpak/exports/share`,
  `/var/lib/flatpak/exports/share`, where the Deck's apps are) listing it, minus Terminal ones and our
  own (`X-Spatiand`), localised names; `mimeapps.list` in the spec's order for the default and the
  added and removed. Started by the entry's `Exec` line **as written**, its field codes given the files
  (`%f`/`%u` once a file, `%F`/`%U` all, `%i %c %k`; Flatpak's `@@u %U @@` kept, so the Flatpak gets the
  file through its portal), Chromium and Electron programs given Wayland as the room gives them.
  Icons from the icon theme the room reads (`spatiand_platform::icons`), then Flatpak's exported icons;
  SVG by resvg.
- **The Mac** (`openwith_mac.rs`): Launch Services through `NSWorkspace` --
  `URLsForApplicationsToOpenURL`, `URLForApplicationToOpenURL` for the default, `iconForFile` for the
  icon -- started with `open -a <app> <files>`; "Other..." is what is in /Applications,
  /Applications/Utilities, ~/Applications, /System/Applications(/Utilities). Remembered by the ending.
  The icon is **drawn at the size shown** (64 px): into an `NSBitmapImageRep` of that size (8 bits,
  RGBA, device RGB) through `NSGraphicsContext graphicsContextWithBitmapImageRep`, AppKit choosing
  the icon's own picture for it, on SpatiShelf's worker thread (none of these classes is main-thread-only;
  the context is that thread's own, pushed and popped on its per-thread stack; the `NSImage` is only
  read; an autorelease pool per icon). Not by its `TIFFRepresentation`, as first: that renders every
  size, the first 1024x1024 in 16-bit floats (Preview's: 74 MB, 32 pictures), which the `image` crate
  cannot read ("Unhandled TIFF sample format 3"), so no icon showed, at ~130 ms and tens of MB each.
  Kept per application in memory for the run and as PNGs in `~/.cache/spatishelf/app-icons`, named by
  the bundle's path and its `Info.plist`'s time (an update draws it again). Checked in the snapshot
  room (littleotter): the menu's nine with their icons; "Other..." with an empty cache, its 72 others
  drawn and kept in 0.31 s (about 4 ms an icon), the first screenful there 0.3 s after the click, and
  the last of the list (scrolled to with End) with theirs.
- **Android, the Beam Pro** (`openwith_android.rs`): only the APK's Java may ask the PackageManager,
  so the room is asked, over its protocol: **`spatiand_xr_v1.ask_system`** (version 11), JSON in, JSON
  written into a descriptor -- `openers`, `all_openers`, `open_with` (the protocol's description says
  what each answers). Elsewhere the room answers that the application asks the system itself. On the
  Beam Pro `Shell.askSystem` queries `ACTION_VIEW` for the type (`queryIntentActivities`, the default by
  `resolveActivity` unless that is the chooser), draws the icons into the cache, and for `open_with`
  grants the app the file's `content://id.prasetya.spatiand.files/<path>` address to read
  (`OpenFiles.java`, a read-only provider of shared storage and Spatiand's own folders, not exported);
  the room then opens the app **in the room as from the launcher**, its display started with
  `ACTION_VIEW` of that address (`ShellHelper.launch`, helper version 2), or, already open, hands it
  the file where it is. Added to Chromium's copy of the protocol too (patch 0020).
- **A remote's file**: to an application that takes addresses (Linux's `%u`/`%U`) its WebDAV
  address; to any other, and always on the Mac and Android, it is fetched first into
  `~/.cache/spatishelf/opened/<remote>/...` (a job of the tab's, with its progress) and that file is
  opened.

**Also fixed on the way**: dates were written in Greenwich's hours ("Today 09:52" for a file made at
16:52 in Jakarta) -- now this machine's; a remote's dates written with their zone (a local disk's,
`+07:00`) were read as Greenwich's -- now with it. A thing's picture arriving after its folder was
listed again is put on the thing it is of (by its place, not its number).

**Checked** on deepmagpie in the snapshot room (`SPATIAND_BACKEND=snapshot`, scratch HOME, XDG_* and
`SPATIAND_APPS_CONFIG`, `SPATIAND_HMD=null SPATIAND_INPUT=off`), looking at each picture: Delete puts
a file in the home trash (`.trashinfo` as the spec has it); Shift-Delete asks and deletes; the Trash
place lists them with FROM and DELETED; Put back of a folder (back with what was in it) and of a file
whose folder had been deleted meanwhile (made again); two of one name (`notes 2.txt` in the trash,
both shown as `notes.txt`); Empty asks with the count and size and empties `files/` and `info/`. GLib
agrees: `gio trash` from a shell put a file where SpatiShelf listed it, and `gio trash --list` (gvfs, in
a private session bus) listed SpatiShelf's with their places. A file `gio trash`-ed from a shell
appeared in the Trash on show by itself. Two SpatiShelf windows (two processes) on Work and on
Work/Photos: a file dragged in the first into Photos showed in the second 1.5 s later; a file made by
a shell command showed in both, in the first inside Photos opened in place. With the storage service
on an alias remote: a file made in its folder showed by itself within the 15 s. Renaming: F2, a click
in the field (caret there, the choice let go of), a drag (chose "liday.jp"), a double click (chose
"jpeg"; `shaky:u,v,0,2`, as single steps are too slow apart to be a double click), Shift-click (from
the choice's far end), typing over it, a click outside (renamed on disk), Escape (not renamed). Open
with on Linux with made-up desktop entries (`%F`, `%u`, and a Flatpak-shaped `@@u %U @@` one in a
scratch `flatpak/exports`) and a scratch `mimeapps.list`: the menu with the default marked and an
entry's PNG icon, each started with the right arguments (`file://` addresses for `%U`, inside
`@@u ... @@`), "always" remembered and then used by Enter, "Other..." listing the machine's applications
with their theme icons; a remote's file given to a `%u` application as its WebDAV address and to a
`%F` one fetched first. Unit tests: the trash (escaping, info files, local times, clashes, restore to
a folder gone, purge, empty, sizes with links, a disk's trash and its copy, the shared `.Trash`'s
sticky rule, mounts, Android volumes), watching (bursts, which folder a change is news for, a real
inotify run: made, moved both ends, unwatched), the field (boundaries by halving, words, click,
drag, double and triple, Shift, a click and a drag that come whole before a draw), Open with (globs,
lineage, text sniffing, desktop entries, Exec field codes, a made-up machine's apps and defaults,
the menu's order, what is remembered). Built: `cargo build --release -p spatishelf`; the room and
every application that uses the SDK (`cargo check`); for Android with cargo-ndk (`spatiand-android`
and the apps; the Java with `javac` against android.jar, the manifest with aapt2); the Mac pieces
(`trash.rs`, `trash_mac.rs`, `openwith.rs`, `openwith_mac.rs`) type-checked for
`aarch64-apple-darwin` in a scratch crate with a stand-in for the SDK.

**Not checked**: anything in glasses; **the Mac at all** (Finder's trash and its Full Disk Access
question, Launch Services, `open -a`, the icons' TIFF; the rest of SpatiShelf and the room were not
compiled for macOS); **the Beam Pro at all** (the `.Trash-spatiand` trash, `ask_system` and
`Shell.askSystem`, the content provider, the helper's `ACTION_VIEW`, an app opening in the room
with a file, the room's apps window brought forward with a new file); Dolphin itself (GLib's
reading was checked, KDE's not); a disk's own `.Trash-$uid` on a real second disk (unit-tested on
scratch folders only); a real cloud remote's own trash; FSEvents (the Mac's watching); a folder
changing thousands of times a second.

## 2026-10-15: from the owner's first look

| | |
|---|---|
| Spatiand would not start again after the glasses were unplugged | Its single-instance lock was inherited by every program it started, and the storage service -- which outlives everything by design -- kept it. The lock is close-on-exec now, and the storage service is started holding nothing inherited. |
| Every Mac window came into the room at once | A hidden Mac window missing from one two-second scan was dropped, and found again as "new" (new windows come out at once). Now two scans running are needed, only a window number above any seen is new, and no more than two come out of their own accord a scan. *The input that stopped reaching Mac windows afterwards was not reproduced.* |
| SpatiReel's controls came back when the head moved | The pointer is cast from the head, so it crosses a window whenever the wearer looks about, and the application cannot tell. `spatiand_xr_v1` v7 adds `hands`: the room tells applications when a hand stirred (pad, mouse, button, key -- never the head). SpatiReel and SpatiLoupe wake their controls on that and on presses. |
| SpatiReel's sound bypassed the room | On the Mac and the Beam Pro an application of Spatiand's own now hands its sound to the session (`$XDG_RUNTIME_DIR/spatiand-sound`, `crates/spatiand/src/own.rs`), which places it at the window through the HRTF -- every channel of a 5.1 or 7.1 film -- with the window's own mute and volume. On the Deck it already went to the window's PipeWire sink. |
| Buttons were words | The room's icons (`spatiand_app::ui::Icon`), drawn for Spatiand, on every application's buttons, sidebars, tab bars and tiles. |
| SpatiShelf's Shift- and Control-click | Read from the keyboard's state at the click (`App::modifiers`), not from the last key event. |
| No rclone in SpatiShelf's words | Only in its About, with the licences. Its remotes were already its own list; "Import remotes" brings in the user's. |
| One folder for every application's settings | `~/.config/spatiand/apps` (shared) and `~/.config/spatiand/local` (this machine's alone: tokens, a device's name to a media server), files moved there from where they were. |
| Settings the same on every paired machine | `crates/spatiand-sync`: whenever a Spatiand is linked to a host (the Mac is both), they exchange what changed in `apps/`, newest version of each file winning, deletions remembered; so every machine ever linked to another ends with one set of settings. Spoken by the session (Deck, Mac, Beam Pro), the Linux `spatiand-host` and the Mac's own host. **Not run over a real link yet**: checked by tests (two machines converging, a deletion travelling, nothing outside the folder touched, the messages on the wire and as the Mac host's JSON) and built on the Mac and on deepmagpie; deepmagpie's running host is the old one until it is rebuilt and restarted. |

## 2026-10-16: from the owner's second look

| | |
|---|---|
| A new window came up anywhere | It was fanned out by how many were open. Now it opens in the middle of the view, nearer (by 0.3 m) than any window it would cover; with no room in front (1.3 m is the nearest), those windows are moved back instead (`WindowLayout::room_for_new`). This is also what made a tab torn out of SpatiQuill look as if its old window had gone. |
| SpatiQuill: tmux's Ctrl-b did nothing; keys came twice | With Control held the SDK says no text, and the chord was sent as nothing; it is sent as its letter now, which the terminal turns into the control byte. The room's key repeat started after 200 ms, shorter than a Mac's; it is the Mac's own now (`SPATIAND_KEY_REPEAT`, set by the app from the system's). |
| SpatiReel: ten seconds back and on | Buttons either side of play/pause, with icons of their own. |
| SpatiReel used the whole line while paused | The player read 512 packets or 24 MB ahead; from a server that was seconds of the line flat out after every pause and seek, and for every film open. Now 4 s of film ahead, nothing more while paused, and a network connection holds 1 MB (`recv_buffer_size`). Measured against a server that counts what it sends: paused, it stops within a second. Nothing is kept on disk. |
| Two accounts on one Vibrefy server | Kept by address *and* account now, each a source of its own, named by whose it is. *The account in the owner's settings had been signed out by the server* (it says NotAuthenticated), so it has to be paired again. |
| No way to remove a server | Remove on the server's own page, pressed twice; Delete on its tile asks the same way. |
| Two SpatiReels: heavy, stuttering, and the second lost from the HRTF | On the Mac and the Beam Pro an application's sound was paired with its windows by its id, so two SpatiReels wrote into one slot (the stutter) and the slot was aimed from one of the two windows. Each instance now gives its window id as `<id>@<process>`, which only the sound pairing sees (`State::instance_of`; everything else gets the plain id). And a film's picture is drawn no larger than the glasses show it (each eye at most 1920×1080): a 4K film was copied to the room at its own size every frame, per SpatiReel. |
| Sound drifting right after some films | The same shared slot: after a second film, both played from wherever the room aimed the slot, one of the two windows. |
| 5.1/7.1: back-left heard in the middle, sides and backs the same | The HRTF dataset and the whole chain were measured channel by channel (a click on each of FL…SR through placement, HRTF and mixer): exactly symmetric, right and left mirror images in level and timing, the channel order FFmpeg's. What made sides and backs alike was the plain stereo mixed in (35%), which puts a side and a back in the same ear: it is kept for the front channels only now, the room's channels heard through the head alone. Back-left in the middle was the shared slot above. **Needs the owner's ears.** |
| Local films kept as "continue watching" | Not any more: that is a server's. The old places are dropped from the settings file when it is next written. |
| SpatiLoupe: a PDF kept its shape when resized | It drew at a fixed size, which the room scaled. It draws at the size it is given now. A picture keeps its own shape inside the window, and around it the window is clear: `spatiand_xr_surface_v1.set_see_through` (v6) lets a window keep its alpha, which windows otherwise do not. |
| Pinching resized the window | `set_pinch_taken` (v6): a window that zooms its own content is sent `pinch` instead, and stays the size it is. SpatiLoupe zooms its picture, page or object. A `pinch:` step in the snapshot backend. |
| SpatiLoupe: swiping sideways was hard, not smooth (2026-11-09) | Two causes. SpatiLoupe threw sideways movement away in the fitted view and switched pictures at once: now the picture follows the hand 1:1 with its neighbour coming in as its folder tile, and settles on it past a quarter of the way or thrown, back if not (`spatiloupe/src/swipe.rs`; keys, Home/End and the strip slide the same way). And the room's pad leans every gesture towards up and down, as a page wants: a sideways swipe waited 0.12 of the pad and, a little off level, went up and down, which SpatiLoupe reads as zoom. `set_scroll_free` (v10): over a window that moves both ways under the hand the pad sends both axes from the thumb's first movement, and the window chooses (`PadScroll::set_free`). Not yet felt on the Beam Pro or the Deck. |
| Escape on the contents closed the document | A held Escape repeated; repeats of Escape are no longer input to any Spatiand application's controls (SpatiQuill's terminal, which reads keys itself, still gets them). |
| Hot keys: o, p, PgUp, PgDn; hints | `o` or `O` for the contents, `p` to go to a page, PageUp and PageDown turn pages. Hints are keycaps now (`look::keys`, written `"[O] Contents  [P] Page"`), in SpatiLoupe and SpatiReel. |

Also: SpatiLoupe asked for a file that is not in its folder's list panicked; it says the file would not open now.

Checked on the Mac in the snapshot backend (window placement, tear-off, bandwidth, accounts and Remove against `tools/fake-vibrefy.py` -- which takes `FAKE_VIBREFY_ACCOUNT` now -- the skip buttons, a 4K film's size, a portrait picture seen through, a PDF's contents, page field and held Escape, a pinch). **Not checked**: two SpatiReels' sound on the live Mac backend (the snapshot backend has no own-sound slots), the Deck (asleep), the Beam Pro.

## 2026-10-19: a Vibrefy pairing runs out

A Vibrefy session lasts about four hours (the owner's estimate). When the server answers the JSON
face with its 401 -- the one answer that cannot be a network failure; a picture's or a stream's
"signed out" can be, and is not acted on -- SpatiReel lets that server go without asking: it is
taken out of `spatireel.json`, and home says it was signed out. Its address is kept
(`vibrefy_last`, with `vibrefy_lapsed`), so home shows a **Vibrefy · host / Signed out ·
reconnect** tile: Enter on it asks that server for a code at once, and approving the code on a
signed-in device is all that is left. Delete on the tile puts it away. "Add a server → Vibrefy"
also starts with the last address filled in. Removing a server on purpose keeps the address for
the form but offers no reconnect tile.

`tools/fake-vibrefy.py` with `FAKE_VIBREFY_SESSIONS=1` gives each client its own session, signs
in only by pairing, and loses every session when it is restarted -- which is how this was
checked in the snapshot backend: paired, server restarted, home showed the tile and the store
had dropped the account; Reconnect paired and opened the server; the prefill; Delete on the
tile; and the session ending while inside a server's folder (back home, with the message).

## 2026-10-20: films from a server fetched as a browser fetches them

The owner: SpatiReel (and the old Vibrefy app) stutter playing normally from Vibrefy over the
internet, where Chrome is smooth; pfSense shows Vibrefy pulling from Google Drive but its
upload to the player flat. Vibrefy's `/stream` (its source is in the owner's
`~/eclipse/Projects/Vibrefy`: `StreamBean`, `content/stream.jsp`, `HttpTool.pipeRange`) costs two
Drive API calls per request, then relays Drive to the player through an 8 KB buffer in 8 MB
Drive range requests one after another -- nothing reaches the player while each next Drive
request starts. FFmpeg's `http:` reads at the film's pace with only the reader's 4 s of packets
(and, since 2026-10-16, a 1 MB socket buffer) in hand, so the whole chain runs at the film's
bitrate and any Drive pause longer than that is a stall; a stalled connection is also what
Jetty's idle timeout eventually cuts. Chrome reads ahead much further, as fast as the line
goes.

`spatiand-media::fetch` now does what a browser does, for every `http(s)` film with a known
length that a server sends in parts: each place the demuxer reads from is fetched about 30 s
of the film ahead (8-64 MB) by one exact `Range: bytes=a-b` at a time, read at full speed to
its end, asked again when half has been read; held in memory only, 16 MB kept behind; a seek
cuts the request it leaves behind. The player hands FFmpeg a custom `AVIOContext` over it.
Playlists, servers that do not answer ranges, and options other than user agent, referer,
cookies and headers still go to FFmpeg's own protocols; `SPATIAND_MEDIA_FETCH=ffmpeg` puts
everything back on them.

Checked on the Mac with `examples/stream.rs` (plays a URL with nobody watching and counts
stalls) against a stand-in server with a shared line, a delay before each first byte and
outages: open, seek and steady play no worse than FFmpeg's own, few large requests, none
left stalled; and SpatiReel playing from `tools/fake-vibrefy.py`. **Not checked**: the real
Vibrefy over the internet -- the owner's ears and pfSense. A server-side improvement worth
having regardless: `pipeRange` could ask Drive for the next 8 MB while relaying this one.

## 2026-10-27: applications installed into Spatiand, by dropping them into the room

Asked for by the owner for SpatiQuake (Quake in the room, peprasetya/SpatiQuake): an application
that is not one Spatiand carries is listed with Spatiand's own, behind the Spatiand bubble, and is
installed by dragging its package into the room -- not by an AppImage or a terminal.

* **An installed application** is a folder `~/.local/share/spatiand/apps/<id>/` with
  `spatiand-app.toml` at its top: `id`, `name`, `exec` (the program, inside the folder), `args`,
  `icon` (PNG or SVG, inside the folder), `categories`, `kind` (`window` or `vr`), `version`.
  `spatiand-platform::installed` reads, lists, installs and removes them; a folder whose manifest
  does not read, whose program is missing, or whose name is not its id is left out (and logged).
* **A package** is that folder as a gzipped tar, `.spatiapp`, the manifest at the top or inside one
  top folder. Installing unpacks it beside the others, checks the manifest and the program, makes
  the program executable, and puts it in place with one rename -- an earlier version of the same id
  is moved aside first and removed after, so the launcher never sees half of one. The unpacking is
  the `tar` crate's (`..` and absolute paths refused).
* **The launcher** (`own.rs`): Spatiand's own, then the installed ones by name. Read again each time
  the launcher opens and after an install, so one installed or removed meanwhile is there. An
  installed one is started as any program is (the X server named, the virtual pad the only
  controller); a `vr` one is told where Spatiand's OpenXR runtime is if the session has not said.
* **Dropping into the room** (`install.rs`, `ClientDndGrabHandler` in `state.rs`): when a window
  starts a drag that offers `text/uri-list`, Spatiand asks the source for it at once -- a drop on
  nothing cancels the source before the compositor hears of the drop, and a cancelled source
  answers nothing. A drag let go of over the room (no window under the pointer) with a
  `.spatiapp` in it: the package is read on a thread, and the notice card under the status bar
  asks "Install SpatiQuake 1.2? Press here to install it." Pressing the card (the right trigger's
  ray, as any press) installs it on a thread, and the card says it is installed and where to find
  it. An offer not taken goes after half a minute.
* **From a terminal**: `spatiand --install FILE` (a package or a folder), `--uninstall ID`,
  `--list-apps`.
* **The snapshot harness** draws the notice card, and has two more steps: `roomup` (the pointer
  off every window and the left button let go there: a drop onto the room) and `notice` (press
  the card).

**Checked**, in a container with no GPU (the snapshot backend in software): a GTK window as the
drag source (`gtk_drag_source_add_uri_targets`, one `file://` address), dragged out past its edge
and let go on the room -- GTK began the drag, was asked for the data, and Spatiand offered
"Install SpatiQuake test?" on the card; with `notice` after it, SpatiQuake was installed and the
card said so; `SPATIAND_VIEW=launcher SPATIAND_VIEW_PAGE=own` then showed it behind the Spatiand
bubble with its icon. Unit tests for the package rules (good package, one top folder, a second
install replacing the first, seven kinds of wrong package refused leaving nothing behind) and the
`text/uri-list` reading.

**Not done**: SpatiShelf drags files as `text/uri-list` (since the drag and drop between windows
landed on master), so a `.spatiapp` dragged out of it onto the room should be offered the same way;
that was merged, not tried. A drag from an X11 application (Xwayland) is not bridged. There is no way to remove an installed application in the room yet
(`spatiand --uninstall`). Not tried on the Deck.

## On the Deck's desktop, without the room

`tools/install-desktop-apps.sh [folder with the programs]` lists SpatiShelf, SpatiReel, SpatiLoupe,
SpatiQuill, SpatiPal, SpatiKey and SpatiMesh under a **Spatiand** category of Plasma's menu. They are
the same programs, ordinary Wayland windows of KWin: with no `spatiand_xr_v1` the SDK is in
`Mode::Flat`, asks the desktop for a title bar (`zxdg_decoration_manager_v1`) and its pointer
(`wp_cursor_shape_v1`). The room's launcher skips the entries' `X-Spatiand` category, as it has these
behind its own bubble.

`spatiand --spatikey-service` holds SpatiKey's socket on the desktop, started at login by an autostart
entry, and ends when the desktop's compositor does, leaving the socket to the room. Asking for a
finger on the Mac shows as a desktop notification, which stays its few seconds after the finger.

`spatiand --dictation-service` is dictation on the desktop, switched by the menu's Dictation entry
(`crates/spatiand/src/desktop_dictation.rs`, `docs/ai-integrations.md`).

What needs the room and is not there: stereo and the room's sound placement, typing into another
window from SpatiKey, SpatiPal's microphone and reading of windows, and the
settings and device-list sync the room runs (a program reads what was last synced). Only the Deck's
tree is covered: a release folder does not carry these programs yet.

## 2026-11-14: SpatiQuill's copy and paste

The owner: "use the cmd/ctrl c / v for copy paste, instead of select for auto copy or right click
for paste." Selecting no longer copies, and the right button no longer pastes. Copies go onto the
**room's clipboard** through the SDK (`App::copy_text`, `wl_data_device`), as every other Spatiand
application's do, so a Mac's pasteboard and a host's clipboard have them too; SpatiQuill no longer
runs `pbcopy`/`wl-copy`/`wl-paste`, and `SPATIQUILL_CLIPBOARD` is gone.

| Keys | Something selected | Nothing selected |
|---|---|---|
| Control-C (left Control) | copies, and the selection goes | the interrupt, as ever |
| Control-V (left Control) | pastes; to a full-screen program (the other screen: vim, nano, less) it is the program's Control-V | the same |
| Control-Shift-C | copies, the selection stays | nothing |
| Control-Shift-V | pastes | pastes |
| Right Control-C (Command-C on a Mac) | copies, the selection stays | nothing: never an interrupt |
| Right Control-V (Command-V on a Mac) | pastes, even into a full-screen program | pastes |

* **The two Controls**: the Mac's room hands either Command key to its clients as the right Control
  (evdev 97, `crates/spatiand-mac/src/mac/ffi.rs`), so SpatiQuill tells them apart by that key's
  press and release (`keys::RIGHT_CONTROL`, `keys::clip`), with nothing of the platform in it. On a
  Deck's or phone's keyboard the right Control is a Control whose C and V are copy and paste only:
  its C copies a selection as the left one does, and with nothing selected does nothing -- the
  left Control-C interrupts. Its other chords (Control-D, -R, tmux's -B) are the program's still.
  Its other Command-like shortcuts (T, W, N) were not changed: Control-Shift or a logo key still.
* **A literal Control-V** on the main screen (readline's quoted-insert) is not sent: Control-V
  pastes there. Readline has quoted-insert on Control-Q too, which reaches it once the terminal's
  flow control is off (`stty -ixon`); on the other screen Control-V is the program's. A literal
  Control-C with something selected is a second Control-C (the first copied and cleared it).
* Pastes are bracketed when the program asked for it (`keys::paste_bytes`; Escapes taken out), and
  bring the view down to the prompt. Whether something is selected is the tab's to say: Control-C
  goes to the tab as a key marked `copy_first`, and the tab copies or sends the key. Control, Shift
  or Command pressed on their own send nothing and no longer clear the selection (pressing Control
  for Control-C cleared what it was to copy).
* **The right button** opens a menu over a program that has not asked for the mouse: Copy (when
  something is selected), Paste, Select all (the history and the screen, to its last written line).
  A program that asked for the mouse gets the right button, as before. A double click selects a
  word and copies nothing.

**Checked** on deepmagpie in the snapshot room (`SPATIAND_BACKEND=snapshot`, a scratch home, bash),
with a raw-mode reader logging every byte the terminal sent and the new `clipboard` step: a drag
selected and copied nothing; Control-C copied it, sent nothing and cleared it; a second Control-C
sent 0x03; `sleep 100` was interrupted; Control-V pasted at bash's prompt (bracketed) and into `cat`;
bracketed paste around the text for a program that asked; on the other screen Control-V sent 0x16
and right Control-V and Control-Shift-V pasted; right Control-C copied with a selection (kept) and
sent nothing without one; Control-Shift-C/V; the right button opened the menu and pasted nothing,
its Copy, Paste and Select all did theirs; a program asking for the mouse got the right button as
SGR reports. **Not checked**: Command on a real Mac (the room's mapping is unit-tested there, not
driven here), the Beam Pro.

## 2026-11-16: new tab, close tab and quit, in every Spatiand app

The owner: "SpatiQuill... add cmd/ctrl T and W for new tab and close tab. and cmd/ctrl-Q to quit for
all spatiand-app". The same code on the Deck, other Linux, the Beam Pro and the Mac; Command is the
right Control there (evdev 97, as the Mac's room hands it to its clients), and on a Deck's or phone's
keyboard the right Control is that Command.

**In the SDK, once** (`crates/spatiand-app`): `Modifiers::command` -- Control held by the right
Control key -- is tracked by the SDK's keyboard (its press and release, `wl_keyboard.enter`'s keys,
cleared on leave), so an application no longer follows key 97 itself. **Quitting is the SDK's**:
`keys::quits` turns the chord into `Event::Quit { window }`, which comes instead of the key and is
not repeated when held:

| Keys | Quits |
|---|---|
| Command-Q (right Control-Q), logo-Q | always |
| Control-Q (left Control) | yes, unless the application keeps it (`App::keep_control_q`) |
| Control-Shift-Q | always |
| anything with Alt | never |

Each application says what quitting is for it (all its windows and tabs go; it asks first where
something would be lost). The question is the SDK's `ui::Confirm`: a pane over the dimmed window,
Return answers the lit button (the yes, to begin with), Escape is no, the arrows and Tab go between
them, the pointer presses either; Space does nothing (a space typed on into a terminal as the
question appeared was a yes). Another Quit or close while it is up is ignored.

| Application | Quit (and the window's close, the same) |
|---|---|
| SpatiQuill | Command-Q and Control-Shift-Q (its left Control-Q is the program's: XON, readline's quoted-insert). All windows and tabs; asks when a program runs in any tab |
| SpatiShelf | all its tabs (each a process) go; asks when a copy, move, deletion, trash or download is not done ("A copy is still going, and quitting stops it part way") |
| SpatiKey | locked and closed (the session ended with the service, as the window's close did); asks when the editor holds changes not saved |
| SpatiPal | its window closes; switched on as the room's assistant, the process stays, listening, as when its window is closed -- quitting the assistant is switching it off in its settings. Off, it ends |
| SpatiReel | ends; a film playing keeps its place first, as on closing |
| SpatiLoupe | ends |
| the chooser (`spatiand-choose`) | Quit is Cancel |

**New tab and close tab** (`crates/spatiquill/src/keys.rs`, `keys::tab_chord`):

| Keys | SpatiQuill |
|---|---|
| Command-T (right Control-T), Control-Shift-T, logo-T | a new tab, always |
| Control-T (left Control) | a new tab at a shell's prompt and whatever runs on the main screen; a full-screen program (the other screen: nano's ^T, vim's tag stack) gets its Control-T, as it gets Control-V |
| Command-W, Control-Shift-W, logo-W | closes the tab, always |
| Control-W (left Control) | **the program's**: the shell's word rubout (0x17), used all the time; never closes a tab |
| anything with Alt | the program's |

The left Control-T was taken for a new tab, as asked, and costs readline's transpose-chars and fzf's
Control-T file finder at a prompt; inside full-screen programs it stays theirs. Closing a tab -- by
key or by its close mark -- with a program running in it beyond the idle shell asks first, as
Terminal on a Mac does ("Close this tab? sleep is still running in it, and closing the tab ends
it."); so does closing a window (the room's close) whose tabs run programs, and quitting. What counts
is found from the processes under the tab's shell (`proc::running`: `/proc` on Linux and Android,
`proc_listchildpids` on the Mac, where `login` stands between the tab and the shell): any child of
the shell, or the shell replaced by a program (`exec`); a tmux or screen client and powerlevel10k's
`gitstatusd` do not count (the session goes on without them). A question about a tab whose shell
then exits goes with it. The last tab closed closes the window, and the last window SpatiQuill.

SpatiShelf had Control-T and Control-W already (either Control, so Command too; its keys are no
program's). Now Control-W asks when a job is still going in that tab, and on the last tab closes the
window -- SpatiShelf -- with the quit's question, as Finder's Command-W does. Its questions (delete,
empty the trash, forget a remote, and these) are the SDK's `Confirm` now: one look in every app.
SpatiShelf counts an operation as unfinished from when it is asked, before its first word of how far
it is, until the tab says it is done.

**Checked** on deepmagpie in the snapshot room (scratch HOME, XDG_* and `SPATIAND_APPS_CONFIG`,
`SPATIAND_HMD=null SPATIAND_INPUT=off`; every picture looked at): SpatiQuill -- Control-T,
Command-T, Control-Shift-T made four tabs, Control-W did not close one, Command-W and
Control-Shift-W closed two; `echo hello world` and Control-W at bash's prompt ran `echo hello`; a
raw-mode reader on the main screen got 0x17 for Control-W and 0x11 for Control-Q, and on the other
screen 0x14 for Control-T as well; Command-T opened a tab over it; an idle tab closed without asking;
with `sleep 100` running Command-W asked, Escape kept it, the close mark asked, Control-Shift-W and
Return closed it and, the last tab, the window and SpatiQuill, with no `sleep` left; Command-Q and
Control-Shift-Q asked with `sleep` running in another tab, Return quit; idle, Command-Q and
Control-Shift-Q quit at once and the left Control-Q did nothing. SpatiShelf -- Control-T, Command-T,
Control-W, Command-W; Control-Q and Command-Q quit; a copy held open (from a FIFO) made Control-Q ask
"A copy is still going", Control-W on its tab ask "Close this tab?" (an idle tab beside it closed
without asking), and Command-W on it as the last tab ask the quit's question; Return quit.
SpatiLoupe, SpatiReel, SpatiPal (assistant off) -- Control-Q and Command-Q each quit; SpatiPal
switched on -- Command-Q closed the window and the process stayed until the room went. SpatiKey
(`--demo`) -- in a new login with a title typed, Control-Q asked, Escape went back to the editor as
it was, Command-Q and Return quit; with nothing typed Command-Q quit at once. The chooser --
Control-Q and Command-Q each cancelled (exit status 1). **Not checked**: Command on a real Mac and
the Beam Pro (the same code; the apps type-checked for Android, SpatiQuill -- whose `proc_listchildpids`
is the Mac's own -- for aarch64-apple-darwin), a real copy of a large file (the FIFO stands in for one), SpatiKey
against the real service (the demo stand-in answers `close`).
