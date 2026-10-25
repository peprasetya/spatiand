# Upstream: -webkit-box-reflect is drawn without its mask (Skia, coordinated graphics)

To file at https://bugs.webkit.org (product WebKit, component "Layout and Rendering" or "WPE
WebKit", keyword `Skia`), then a pull request at https://github.com/WebKit/WebKit referencing
it, with `webkit-box-reflect-mask.patch` (in this folder) as the change. WebKit's process wants
a ChangeLog-style commit message (below) and, ideally, a layout test; neither the test nor a
check against WebKit's `main` has been done yet -- see "Before sending".

---

## Bug report

**Title:** [Skia][Coordinated Graphics] -webkit-box-reflect mask (fade gradient) is ignored on
composited layers

**Description:**

On ports that composite with Skia through coordinated graphics (WPE, GTK), a reflection with a
mask, such as

```css
.tile {
  transform: translateZ(0);
  -webkit-box-reflect: below 20px linear-gradient(transparent, transparent 40%, rgba(0,0,0,0.4));
}
```

is drawn at full opacity: the gradient that should fade it has no effect. Safari draws the
faded reflection. Seen in WPE WebKit 2.54.1.

There are three causes, all in how the replica layer's own mask layer is handled:

1. `GraphicsLayerCoordinated::recursiveCommitChanges()` commits the layer's mask and its
   replica, but not the replica's mask. The replica's mask layer never receives its size and
   stays 0x0.
2. `GraphicsLayerCoordinated::updateBackingStoresIfNeeded()` updates the layer's mask and the
   replica, but not the replica's mask, so that mask never gets a backing store or tiles.
3. `SkiaCompositingLayer::paintWithReplica()` draws the replica pass through
   `paintWithMaskAndBackdrop()`, which applies the layer's own `m_mask`, never the replica's.

With all three fixed, the reflection fades as in Safari.

**Steps to reproduce:** open a page with the CSS above on an element with some content, in WPE
(or GTK) with accelerated compositing. Expected: the reflection fades from transparent to 40%.
Actual: the reflection is fully opaque.

---

## Commit message (WebKit style)

```
[Skia][Coordinated Graphics] -webkit-box-reflect mask is ignored on composited layers
https://bugs.webkit.org/show_bug.cgi?id=NNNNNN

Reviewed by NOBODY (OOPS!).

The replica layer's own mask (the fade of -webkit-box-reflect) was never committed, never
given a backing store, and not used when the replica was painted, so reflections were drawn
unfaded.

* Source/WebCore/platform/graphics/texmap/coordinated/GraphicsLayerCoordinated.cpp:
(WebCore::GraphicsLayerCoordinated::recursiveCommitChanges): Commit the replica's mask layer.
(WebCore::GraphicsLayerCoordinated::updateBackingStoresIfNeeded): Update its backing store.
* Source/WebCore/platform/graphics/skia/SkiaCompositingLayer.h:
* Source/WebCore/platform/graphics/skia/SkiaCompositingLayer.cpp:
(WebCore::SkiaCompositingLayer::paintWithMaskAndBackdrop): Use the replica's mask while the
replica is being painted.
(WebCore::SkiaCompositingLayer::paintWithReplica): Set it for the replica pass.
```

---

## How it was found, said plainly (for the bug's first comment)

> I found this while porting WPE WebKit to macOS for a project of mine (Spatiand, a spatial
> desktop: WPE draws its browser's pages). The port is not upstream, but this bug is not in
> the port: the code involved is shared by WPE and GTK on every platform, and the change
> touches only that shared code.
>
> I used an AI coding assistant (Anthropic's Claude) to trace the cause and write the patch.
> I have reviewed it, built it, and checked the result against Safari on a real page; I take
> responsibility for it as my contribution and am happy to rework it as reviewers wish. If the
> project's policy on AI-assisted contributions requires something different, please tell me.

(Edit the wording as you like -- it is yours to send. It says what was done and who stands
behind it, and it does not hide the assistant, nor make it the author.)

---

## Before sending

- **Check against WebKit `main`.** The fix was made on the 2.54.1 release. The code around it
  may have moved; someone may already have fixed it. Search Bugzilla for "box-reflect Skia".
- **A layout test.** WebKit reviewers will ask for one: a ref test with a masked reflection
  compared against the same element drawn with an explicit gradient overlay, in
  `LayoutTests/compositing/reflections/`. Not written yet; I can write it once the patch is
  rebased on `main`.
- **Contributor setup.** WebKit takes changes as GitHub pull requests made with
  `Tools/Scripts/git-webkit`, linked to the Bugzilla bug; it needs a Bugzilla account in your
  name.
- **The Mac port itself** (`third_party/wpe/patches/webkit/0001-wpe-on-macos.patch`) is a
  separate, much larger conversation: WPE upstream targets Linux and embedded systems, so it
  is better raised on the webkit-wpe mailing list first, as a question, before any patches.
