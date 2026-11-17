# SpatiMesh's patches on Chromium

All of them are Spatiand's own to keep and to carry from one Chromium to the next (the owner,
2026-11-13: "we are on our own patch now"). There is no Cromite checkout any more, and nothing
waits for one. Applied by `../build.sh <work> patch`, in this order:

| | |
|---|---|
| `base/` | The privacy and ad-blocking changes **taken over from Cromite** (its commit 3724efc, Chromium 153.0.8010.37, 2026-09-19; Cromite is GPL-3.0, and so are these). 276 of its 327: left out are the 44 that touch nothing a Linux-UI build compiles (its Android app's Java and resources, Windows), "Viewport Protection flag" (the owner, 2026-11-13: of no use in the room, and it drew black bars), and "Android Pixel Perfect Mode" and "Protect keyboard geometry on Android", which rest on it and do nothing off Android. From the ones kept, the files no Linux-UI build compiles are left out too (`*.java`, `chrome/android/`, `*_win.cc` ...), so that they need no carrying. Their names and numbers are Cromite's order. |
| `0001` ... | **Spatiand's own**, each described in docs/spatimesh.md. The numbers are kept for good: 0016 (bounce tracking and history) is gone since Chromium 157 removed what it mended, and 0029 (the switch against Viewport Protection) with that patch. |
| `sub/<name>/` | Those of `base/` that reach into a dependency which is a repository of its own (V8, Skia, DevTools' frontend, BoringSSL), applied and committed in the folder the `PATH` file names. Cromite folded those repositories into Chromium's own, after which `gclient` could never sync the tree again; kept apart, the same tree is synced to the next version. |

After them, for its target only: `../android-libc/` (Termux's changes and ours for Android's libc)
and `../mac/` (the Mac's two patches and its port, `mac/wip/`).

`../args.gn` is Cromite's GN arguments, likewise taken over.

**The files are made by a machine** (`git format-patch --zero-commit --no-signature` from the
tree, `../rebase/`), and applied to a clean Chromium of the pinned version they give the tree the
build was made from, exactly: checked at every update. Change a patch in the tree and write it out
again, not by hand here.

**Nothing is fetched from Cromite's pages any more (2026-11-14):** the ad blocker's lists come
from their maintainers' own addresses (eyeo's `easylist-downloads.adblockplus.org` for the EasyList
family and the anti-circumvention list, each other list from the address its maintainers register),
every one free-licensed; a profile that had the mirror's addresses is moved to them at its next
start, its own subscriptions and switches untouched. No longer offered at first run, a profile that
had them keeps them: the Bulgarian list (no licence stated) and Dandelion Sprout's Nordic (its own
licence, not a free one). The component updater asks nobody (its one source was Cromite's, and
served only a fonts pack no build of ours installed). In the same change: Google's model execution
is off by default, on the device and on its servers, with its pages' APIs and the Settings page gone
(SpatiPal is the assistant here), and Web Bluetooth is there on Linux and asks, as in Chrome.

**To drop at the next update (the owner, 2026-11-17):** base/0152 "Add setting to clear data on exit": its own flag, numbered by hand, became the history bit in Chromium 157 and every bounce-tracking clearing wiped all history; moved to bit 62 for now.

**Looked into, and nothing to change (2026-11-18):**

- *The log line `Message 1 rejected by interface blink.mojom.WidgetHost`* (`mojo/public/cpp/bindings/lib/interface_endpoint_client.cc:652`). Message 1 of `WidgetHost` (`third_party/blink/public/mojom/widget/platform_widget.mojom`) is `UpdateTooltipUnderCursor`. It is logged when a page that has a tooltip showing is replaced by one in another process (a link or an address to another site while the pointer rests on something with a `title`): the old page's widget sends its last tooltip update as it is torn down, and the browser's end of that widget's pipe has by then seen the other end close (`HandleValidatedMessage`'s `encountered_error_`, "a message is received for an interface after it has been disconnected"), so the message is dropped and the line written. Not a validation failure (no "bad Mojo message", no renderer killed), not the Mac's (the Deck's log has it 36 times, and messages 0 `SetCursor` and 5 `SelectionBoundsChanged` a few times, taken to be the same race) and not ours: no patch of ours touches it. Reproduced in the Mac's snapshot room, 157.0.8103.78: the pointer on a `title`d element, the tooltip up, then a navigation from `127.0.0.1` to `localhost` over DevTools -- the line once, the tooltip gone, the new page whole. Tooltips themselves are shown and placed rightly (an `xdg_popup` at the pointer). Nothing visible breaks; a patch to quiet it would be a patch to carry for a log line.
- *Video decoding on the Mac in 157* is the hardware's for H.264, HEVC, VP9 and AV1 (`VideoToolboxVideoDecoder`, which asks VideoToolbox to *require* its hardware decoder: `media/gpu/mac/video_toolbox_decompression_session_manager.mm`), with the switches the room starts it with and no more: docs/spatimesh-mac-port.md, *Chromium 157's decoders*. No switch and no patch is needed.

**Candidates to drop when they next cost work**, since they do nothing in a Linux-UI build:
"Experimental support for extensions on Android" (235 files, and it renames build flags across the
tree, which every new Chromium uses in new places), "Android fonts fingerprinting mitigation",
the "WIN ..." ones, "Bromite auto updater". Dropped only with the owner's word, one at a time, the
build compared before and after.
