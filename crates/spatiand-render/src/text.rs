//! Text rasterisation.
//!
//! Produces plain RGBA images on the CPU; uploading them is the GL layer's job. Keeping the
//! split there means text can be tested without a GPU, a display or a headset — and the
//! things worth testing about text (does it wrap, does it fit, is it actually opaque where
//! glyphs are) are all CPU-side properties.
//!
//! Sizing is in **degrees of field of view**, not pixels. Each eye gets 1920 px across about
//! 40°, so roughly 48 px per degree — but that ratio changes with the headset, and a UI
//! specified in pixels would silently become unreadable on different optics. Asking for "1.5°
//! tall" is a statement about legibility that survives a hardware change.

use cosmic_text::{Align, Attrs, Buffer, Family, FontSystem, Metrics, Shaping, SwashCache};

/// Where a line sits within the width it was given.
///
/// Centring is right for something floating in space with nothing around it — a prompt, a
/// heading — where a ragged left edge reads as a fault because there is no margin for the eye
/// to measure it against. Inside a panel there *is* an edge, and then centring is the thing
/// that looks wrong: a wrapped explanation under a column of left-aligned rows wants to start
/// where they start.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TextAlign {
    Left,
    Center,
}

/// An RGBA8 image, straight (non-premultiplied) alpha.
#[derive(Debug, Clone, PartialEq)]
pub struct TextImage {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

impl TextImage {
    pub fn is_empty(&self) -> bool {
        self.width == 0 || self.height == 0
    }

    /// Fraction of pixels with any coverage. Used by tests to tell "drew something" from
    /// "silently drew nothing", which is otherwise an easy failure to miss.
    pub fn ink_fraction(&self) -> f32 {
        if self.rgba.is_empty() {
            return 0.0;
        }
        let inked = self.rgba.chunks_exact(4).filter(|p| p[3] > 8).count();
        inked as f32 / (self.rgba.len() / 4) as f32
    }

    /// The ink's own top and bottom, as fractions of the image's height.
    ///
    /// `render` crops a line to its ink *horizontally* but keeps the full line-height box
    /// vertically -- see its own doc comment. That box is where a font's metrics say a
    /// descender could reach, not where this particular glyph's ink actually sits, and the two
    /// agree only by accident. They are close enough for a word of lowercase letters, whose
    /// x-height fills most of the box either side of the baseline, and visibly wrong for a
    /// single glyph that has no descender at all -- an arrow, a speaker -- which then sits high
    /// in the box with all the spare room left below it. This is what lets a caller ask "where
    /// is the ink" instead of assuming the box's own centre.
    ///
    /// `(0.0, 1.0)` for an empty image: the caller's fallback is then the box's own centre,
    /// which is what happened before this existed.
    pub fn ink_vertical_extent(&self) -> (f32, f32) {
        if self.height == 0 {
            return (0.0, 1.0);
        }
        let mut min_y = self.height;
        let mut max_y = 0u32;
        for y in 0..self.height {
            for x in 0..self.width {
                if self.rgba[((y * self.width + x) * 4 + 3) as usize] > 8 {
                    min_y = min_y.min(y);
                    max_y = max_y.max(y);
                }
            }
        }
        if min_y > max_y {
            return (0.0, 1.0);
        }
        (
            min_y as f32 / self.height as f32,
            (max_y + 1) as f32 / self.height as f32,
        )
    }

    /// Where the middle of the ink sits, as a fraction of the image's height. `0.5` — the
    /// box's own centre — for an empty image, which is the answer a caller already assumed
    /// before this existed.
    ///
    /// How far from `0.5` this gets is worth knowing before deciding it does not matter.
    /// Measured on the Deck's own fonts: the word "space" inks at **0.65**, a sixth of the box
    /// below its middle. Ordinary lettering was never centred either — it only looked it,
    /// because every letter was wrong by the same amount and had nothing to be wrong against.
    /// A symbol among letters is what made it visible.
    pub fn ink_vertical_center(&self) -> f32 {
        let (top, bottom) = self.ink_vertical_extent();
        (top + bottom) * 0.5
    }

    /// The tightest rectangle that still contains every inked pixel.
    ///
    /// For a caller that draws this image's own pixels rather than stretching them into a box
    /// of some other size -- see [`Self::ink_vertical_center`] for the alternative, needed
    /// wherever stretching is unavoidable and cropping would silently change the glyph's size
    /// along with its position.
    pub fn crop_to_ink_vertically(&self) -> TextImage {
        let (top, bottom) = self.ink_vertical_extent();
        let min_y = (top * self.height as f32).round() as u32;
        let max_y = ((bottom * self.height as f32).round() as u32)
            .max(min_y + 1)
            .min(self.height);
        let new_h = max_y - min_y;
        let row = (self.width * 4) as usize;
        let mut out = vec![0u8; row * new_h as usize];
        for y in 0..new_h {
            let src = ((y + min_y) as usize) * row;
            let dst = (y as usize) * row;
            out[dst..dst + row].copy_from_slice(&self.rgba[src..src + row]);
        }
        TextImage {
            width: self.width,
            height: new_h,
            rgba: out,
        }
    }
}

/// The machine's fonts. For everything of Spatiand's that sets text with cosmic-text: an
/// application that asked cosmic-text itself found none on Android and stopped at its first
/// letter (SpatiQuill on the Beam Pro, "no default font found", 2026-11-14).
#[cfg(not(target_os = "android"))]
pub fn font_system() -> FontSystem {
    FontSystem::new()
}

/// Android's fonts, which fontdb does not look for by itself: they are all in `/system/fonts`,
/// and the sans-serif is Roboto.
///
/// All but one. `NotoColorEmoji.ttf` is a COLRv1 font, whose pictures swash cannot paint:
/// loaded, it answers for every emoji and draws nothing, which on the Beam Pro was every emoji
/// in a window's title gone (measured 2026-11-09). `NotoColorEmojiLegacy.ttf` beside it is the
/// same set as bitmaps (CBDT), which swash draws, in colour, as a Mac's are.
#[cfg(target_os = "android")]
pub fn font_system() -> FontSystem {
    const FONTS: &str = "/system/fonts";
    let mut db = cosmic_text::fontdb::Database::new();
    let bitmaps = std::path::Path::new(FONTS).join("NotoColorEmojiLegacy.ttf").exists();
    match std::fs::read_dir(FONTS) {
        Ok(entries) => {
            for path in entries.flatten().map(|e| e.path()) {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                let font = [".ttf", ".otf", ".ttc", ".otc"].iter().any(|e| name.to_ascii_lowercase().ends_with(e));
                if !font || (bitmaps && name == "NotoColorEmoji.ttf") {
                    continue;
                }
                if let Err(e) = db.load_font_file(&path) {
                    log::warn!("font {name}: {e}");
                }
            }
        }
        Err(_) => db.load_fonts_dir(FONTS),
    }
    db.set_sans_serif_family("Roboto");
    db.set_serif_family("Noto Serif");
    db.set_monospace_family("Droid Sans Mono");
    FontSystem::new_with_locale_and_db("en-US".into(), db)
}

/// Composite one straight-alpha pixel over what is already at `(x, y)`, source-over, so that
/// overlapping glyphs (accents, tight scripts) composite instead of punching holes. Off the
/// image is nothing.
fn blend(rgba: &mut [u8], (width, height): (u32, u32), (x, y): (i32, i32), c: [u8; 4]) {
    if c[3] == 0 || x < 0 || y < 0 || x >= width as i32 || y >= height as i32 {
        return;
    }
    let i = ((y as u32 * width + x as u32) * 4) as usize;
    let sa = c[3] as u32;
    let da = rgba[i + 3] as u32;
    let out_a = sa + da * (255 - sa) / 255;
    if out_a == 0 {
        return;
    }
    for k in 0..3 {
        let sc = c[k] as u32;
        let dc = rgba[i + k] as u32;
        rgba[i + k] = ((sc * sa + dc * da * (255 - sa) / 255) / out_a).min(255) as u8;
    }
    rgba[i + 3] = out_a.min(255) as u8;
}

pub struct TextRenderer {
    font_system: FontSystem,
    swash_cache: SwashCache,
}

impl TextRenderer {
    pub fn new() -> Self {
        Self {
            font_system: font_system(),
            swash_cache: SwashCache::new(),
        }
    }

    /// Pixels per degree for a given eye. See the module note on why sizing is angular.
    pub fn px_per_degree(eye_width_px: u32, h_fov_deg: f64) -> f32 {
        (eye_width_px as f64 / h_fov_deg) as f32
    }

    /// Rasterise `text` into an image `max_width` px wide, wrapping as needed.
    ///
    /// `size_px` is the em size; `color` is straight RGBA.
    pub fn render(
        &mut self,
        text: &str,
        size_px: f32,
        max_width: u32,
        color: [u8; 4],
    ) -> TextImage {
        self.render_inner(text, size_px, max_width, color, false, TextAlign::Center)
    }

    /// As [`Self::render`], but the image keeps the full `max_width` instead of being cropped
    /// to the ink.
    ///
    /// For anything whose *size* must not depend on its contents. A menu is the case that
    /// forced this: the selected row carries a marker and the others carry spaces, so the
    /// widest line — and with it the cropped image, and with it the panel fitted to that
    /// image's aspect — changed every time the cursor moved. The panel visibly grew and shrank
    /// as you moved down the list, which reads as the layout being unstable rather than as a
    /// consequence of one glyph.
    pub fn render_padded(
        &mut self,
        text: &str,
        size_px: f32,
        max_width: u32,
        color: [u8; 4],
    ) -> TextImage {
        self.render_inner(text, size_px, max_width, color, true, TextAlign::Center)
    }

    /// A block of text that keeps its full width and is aligned as asked.
    ///
    /// The alignment is the whole reason this exists alongside [`Self::render_padded`]: a
    /// panel's explanation is set left, under rows that are also set left, and cropping it
    /// would leave the caller unable to tell where the left edge of the text is.
    pub fn render_aligned(
        &mut self,
        text: &str,
        size_px: f32,
        max_width: u32,
        color: [u8; 4],
        align: TextAlign,
    ) -> TextImage {
        self.render_inner(text, size_px, max_width, color, true, align)
    }

    fn render_inner(
        &mut self,
        text: &str,
        size_px: f32,
        max_width: u32,
        color: [u8; 4],
        keep_full_width: bool,
        align: TextAlign,
    ) -> TextImage {
        // Generous line spacing: at a 40 degree field the eye travels a long way between
        // lines, and tight leading reads as cramped in a way it does not on a monitor.
        let metrics = Metrics::new(size_px, size_px * 1.4);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        let attrs = Attrs::new().family(Family::SansSerif);
        // The room's own symbols are shaped as a stand-in and painted where it lands, so they
        // are the same picture on every platform: see [`crate::symbols`].
        let stand_ins = crate::symbols::stand_ins(text);
        let shaped = stand_ins.as_ref().map_or(text, |(s, _)| s.as_str());
        {
            // 0.19 moved Buffer's mutating methods behind `borrow_with`, which pairs the
            // buffer with its font system for the duration rather than threading it through
            // every call.
            let mut b = buffer.borrow_with(&mut self.font_system);
            b.set_size(Some(max_width as f32), None);
            // See [`TextAlign`] for why this is a choice rather than always centred.
            let align = match align {
                TextAlign::Left => Align::Left,
                TextAlign::Center => Align::Center,
            };
            b.set_text(shaped, &attrs, Shaping::Advanced, Some(align));
            b.shape_until_scroll(false);
        }

        // Measure before allocating, so a short string does not carry a full-width bitmap
        // around with it.
        let mut used_width = 0.0f32;
        let mut lines = 0usize;
        for run in buffer.layout_runs() {
            used_width = used_width.max(run.line_w);
            lines += 1;
        }
        // Draw at full width, then crop to the ink.
        //
        // Cropping to a *measured* width and shifting by half the slack is almost right and
        // visibly wrong: `line_w` excludes side bearings, so the shift is a pixel or two
        // short and every line drifts the same way. Finding the actual painted extent
        // afterwards is exact, and costs one pass over an image we have already built.
        let width = max_width;
        let height = ((lines.max(1) as f32) * metrics.line_height).ceil() as u32;
        let _ = used_width;

        let mut rgba = vec![0u8; (width * height * 4) as usize];
        let text_color = cosmic_text::Color::rgba(color[0], color[1], color[2], color[3]);
        // Where each stand-in went, as cosmic-text counts it: by line, and by byte within the
        // line.
        let marks: Vec<(usize, usize, crate::symbols::Symbol)> = match &stand_ins {
            Some((shaped, marks)) => {
                let lines: Vec<_> = cosmic_text::LineIter::new(shaped).map(|(r, _)| r).collect();
                marks
                    .iter()
                    .filter_map(|&(at, symbol)| {
                        let line = lines.iter().position(|r| r.start <= at && at < r.end)?;
                        Some((line, at - lines[line].start, symbol))
                    })
                    .collect()
            }
            None => Vec::new(),
        };
        // Glyph by glyph, as `Buffer::draw` does it, so that a stand-in can be left out.
        let mut symbols = Vec::new();
        for run in buffer.layout_runs() {
            for glyph in run.glyphs.iter() {
                if let Some(&(_, _, symbol)) = marks
                    .iter()
                    .find(|(line, at, _)| *line == run.line_i && *at == glyph.start)
                {
                    symbols.push((symbol, glyph.x + glyph.w * 0.5, run.line_y, glyph.font_size));
                    continue;
                }
                let physical = glyph.physical((0.0, run.line_y), 1.0);
                let colour = glyph.color_opt.unwrap_or(text_color);
                self.swash_cache.with_pixels(
                    &mut self.font_system,
                    physical.cache_key,
                    colour,
                    |x, y, c| {
                        blend(
                            &mut rgba,
                            (width, height),
                            (physical.x + x, physical.y + y),
                            [c.r(), c.g(), c.b(), c.a()],
                        )
                    },
                );
            }
        }
        for (symbol, centre_x, baseline, em) in symbols {
            let side = em.ceil().max(1.0) as u32;
            let Some(coverage) = symbol.coverage(side) else {
                continue;
            };
            let left = (centre_x - side as f32 * 0.5).round() as i32;
            let top = (baseline - em * crate::symbols::MIDDLE - side as f32 * 0.5).round() as i32;
            for (i, &cover) in coverage.iter().enumerate() {
                if cover == 0 {
                    continue;
                }
                let (dx, dy) = ((i as u32 % side) as i32, (i as u32 / side) as i32);
                let alpha = (cover as u32 * color[3] as u32 / 255) as u8;
                blend(
                    &mut rgba,
                    (width, height),
                    (left + dx, top + dy),
                    [color[0], color[1], color[2], alpha],
                );
            }
        }

        // Keeping the full width is the whole point for a padded render: the text is already
        // centred within it, so there is nothing further to do.
        if keep_full_width {
            return TextImage {
                width,
                height,
                rgba,
            };
        }

        // Crop to the painted area.
        let mut min_x = width;
        let mut max_x = 0u32;
        for y in 0..height {
            for x in 0..width {
                if rgba[((y * width + x) * 4 + 3) as usize] > 8 {
                    min_x = min_x.min(x);
                    max_x = max_x.max(x);
                }
            }
        }
        if min_x > max_x {
            // Nothing was drawn - an empty string, or glyphs the font could not supply.
            // Return a valid 1x1 rather than a zero-sized texture, which GL rejects.
            return TextImage {
                width: 1,
                height: 1,
                rgba: vec![0; 4],
            };
        }
        let cropped_w = max_x - min_x + 1;
        let mut out = vec![0u8; (cropped_w * height * 4) as usize];
        for y in 0..height {
            let src = ((y * width + min_x) * 4) as usize;
            let dst = (y * cropped_w * 4) as usize;
            out[dst..dst + (cropped_w * 4) as usize]
                .copy_from_slice(&rgba[src..src + (cropped_w * 4) as usize]);
        }

        TextImage {
            width: cropped_w,
            height,
            rgba: out,
        }
    }
}

impl Default for TextRenderer {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod padded_tests {
    use super::*;

    /// A padded render's width must not depend on what it contains.
    ///
    /// This is the property the settings panel needs: it is fitted to the rendered image's
    /// aspect, so a width that varies with the text makes the panel grow and shrink as the
    /// selection moves. Cropping is right for a label and wrong for anything whose size is
    /// part of the layout.
    #[test]
    fn a_padded_render_is_always_the_width_it_was_asked_for() {
        let mut t = TextRenderer::new();
        let short = t.render_padded("i", 24.0, 512, [255, 255, 255, 255]);
        let long = t.render_padded(
            "a much longer line of text",
            24.0,
            512,
            [255, 255, 255, 255],
        );
        assert_eq!(short.width, 512);
        assert_eq!(long.width, 512);
    }

    /// And the ordinary render must still shrink to fit, or every label carries a full-width
    /// bitmap around with it.
    #[test]
    fn an_ordinary_render_still_crops_to_the_ink() {
        let mut t = TextRenderer::new();
        let short = t.render("i", 24.0, 512, [255, 255, 255, 255]);
        assert!(short.width < 512, "cropped width was {}", short.width);
    }

    /// The marker is what made the panel move, so it is worth stating that it is wider than
    /// the spaces that stand in for it. If this ever stops being true the padding is harmless,
    /// but the reason for it is gone.
    #[test]
    fn the_selection_marker_is_wider_than_the_blank_that_replaces_it() {
        let mut t = TextRenderer::new();
        let marked = t.render("\u{25b8} Calibrate head tracking", 24.0, 1024, [255; 4]);
        let blank = t.render("   Calibrate head tracking", 24.0, 1024, [255; 4]);
        assert_ne!(
            marked.width, blank.width,
            "if these are equal the panel never moved and this fix is unnecessary"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn px_per_degree_matches_the_air() {
        // 1920 px across 40 degrees is the number every UI size is derived from.
        let ppd = TextRenderer::px_per_degree(1920, 40.0);
        assert!((ppd - 48.0).abs() < 0.1, "expected ~48 px/deg, got {ppd}");
    }

    #[test]
    fn renders_visible_glyphs() {
        let mut r = TextRenderer::new();
        let img = r.render("Turn LEFT", 48.0, 800, [255, 255, 255, 255]);
        assert!(!img.is_empty(), "produced no image");
        assert_eq!(img.rgba.len(), (img.width * img.height * 4) as usize);
        // The failure this guards against is a correct-looking image full of zeros, which
        // renders as an invisible prompt and looks like the compositor having hung.
        assert!(
            img.ink_fraction() > 0.02,
            "almost nothing was drawn (ink {:.4})",
            img.ink_fraction()
        );
    }

    #[test]
    fn empty_text_is_survivable() {
        let mut r = TextRenderer::new();
        let img = r.render("", 48.0, 800, [255, 255, 255, 255]);
        assert_eq!(img.ink_fraction(), 0.0);
        // Must still be a valid 1x1-or-larger buffer rather than a zero-sized texture, which
        // GL rejects.
        assert!(img.width >= 1 && img.height >= 1);
    }

    #[test]
    fn narrow_width_wraps_into_more_lines() {
        let mut r = TextRenderer::new();
        let text = "Slowly turn your head to the left, like saying no, and hold.";
        let wide = r.render(text, 32.0, 1600, [255, 255, 255, 255]);
        let narrow = r.render(text, 32.0, 400, [255, 255, 255, 255]);
        assert!(
            narrow.height > wide.height,
            "wrapping should add lines: {} vs {}",
            narrow.height,
            wide.height
        );
        assert!(narrow.width <= 400);
    }

    #[test]
    fn the_rooms_symbols_are_drawn_in_the_ink_and_nothing_else() {
        // The Beam Pro drew these as nothing and a Mac drew the speakers as coloured emoji
        // (crate::symbols). Here every one of them inks, and only in the colour asked for.
        let mut r = TextRenderer::new();
        for symbol in crate::symbols::Symbol::ALL {
            let img = r.render(&symbol.char().to_string(), 64.0, 256, [255, 40, 0, 255]);
            assert!(img.ink_fraction() > 0.05, "{symbol:?} drew {:.3}", img.ink_fraction());
            for p in img.rgba.chunks_exact(4).filter(|p| p[3] > 0) {
                assert!(p[0] > 200 && p[1] < 80 && p[2] < 40, "{symbol:?} has a pixel {p:?}");
            }
        }
    }

    #[test]
    fn a_symbol_sits_where_it_was_set_and_as_wide_as_its_stand_in() {
        let mut r = TextRenderer::new();
        let white = [255, 255, 255, 255];
        let symbol = r.render_padded("A\u{1F50A}B", 64.0, 400, white);
        let stand_in = r.render_padded("A@B", 64.0, 400, white);
        assert_eq!((symbol.width, symbol.height), (stand_in.width, stand_in.height));
        // The A and the B are where they are with the stand-in, so the symbol took exactly its
        // room; and between them there is ink, which is the symbol and not the stand-in.
        let columns = |img: &TextImage| -> Vec<bool> {
            (0..img.width)
                .map(|x| (0..img.height).any(|y| img.rgba[((y * img.width + x) * 4 + 3) as usize] > 8))
                .collect()
        };
        let (a, b) = (columns(&symbol), columns(&stand_in));
        let first = |c: &[bool]| c.iter().position(|&x| x).unwrap();
        let last = |c: &[bool]| c.iter().rposition(|&x| x).unwrap();
        assert_eq!(first(&a), first(&b), "the A moved");
        assert_eq!(last(&a), last(&b), "the B moved");
        let speaker = r.render("\u{1F50A}", 64.0, 400, white);
        let at = r.render("@", 64.0, 400, white);
        assert_ne!(speaker, at, "the stand-in was drawn instead of the symbol");
    }

    #[test]
    fn a_symbol_is_level_with_the_capitals_beside_it() {
        // Before a title, a speaker sits halfway up its first letter: an emoji's own box
        // drops below the baseline, which is how it looks pasted in rather than set.
        let mut r = TextRenderer::new();
        let white = [255, 255, 255, 255];
        let line = r.render_padded("\u{1F50A}H", 64.0, 400, white);
        let h = r.render_padded("@H", 64.0, 400, white);
        // The H's own columns, from the stand-in render: the right half of the ink.
        let ink_rows = |img: &TextImage, from: u32, to: u32| -> (u32, u32) {
            let rows: Vec<u32> = (0..img.height)
                .filter(|&y| (from..to).any(|x| img.rgba[((y * img.width + x) * 4 + 3) as usize] > 8))
                .collect();
            (*rows.first().unwrap(), *rows.last().unwrap())
        };
        let cols: Vec<u32> = (0..h.width)
            .filter(|&x| (0..h.height).any(|y| h.rgba[((y * h.width + x) * 4 + 3) as usize] > 8))
            .collect();
        // The gap between the @ and the H splits the two.
        let split = cols.windows(2).find(|w| w[1] > w[0] + 1).map(|w| w[0] + 1).unwrap();
        let (h_top, h_bottom) = ink_rows(&line, split, line.width);
        let (s_top, s_bottom) = ink_rows(&line, 0, split);
        let (h_mid, s_mid) = ((h_top + h_bottom) as f32 / 2.0, (s_top + s_bottom) as f32 / 2.0);
        assert!((h_mid - s_mid).abs() <= 64.0 * 0.06, "speaker centred at {s_mid}, H at {h_mid}");
    }

    #[test]
    fn colour_is_respected() {
        let mut r = TextRenderer::new();
        let img = r.render("X", 64.0, 200, [255, 0, 0, 255]);
        let reddest = img
            .rgba
            .chunks_exact(4)
            .filter(|p| p[3] > 200)
            .map(|p| (p[0], p[1], p[2]))
            .next()
            .expect("should have an opaque pixel");
        assert!(
            reddest.0 > 200 && reddest.1 < 80 && reddest.2 < 80,
            "expected red, got {reddest:?}"
        );
    }

    #[test]
    fn a_single_glyph_does_not_fill_its_own_line_height_box() {
        // The premise the rest of this file's centring bug rests on. `render`'s box is a
        // font-metrics line height -- room for the tallest ascender and the deepest descender
        // a *font* can produce, not what any one glyph actually used. A short glyph with no
        // descender, alone on its line, leaves real margin on at least one side; which side,
        // and by how much, is a property of the font actually installed and is deliberately
        // not asserted here -- see the sidecar's own glyph tests for that, checked against the
        // Deck's real font stack through the snapshot backend rather than against whatever
        // fontconfig resolves to on the machine running `cargo test`.
        let mut r = TextRenderer::new();
        let img = r.render("\u{2190}", 64.0, 200, [255, 255, 255, 255]);
        let (top, bottom) = img.ink_vertical_extent();
        assert!(
            top > 0.0 || bottom < 1.0,
            "the arrow's ink fills the box with no margin at all"
        );
    }

    #[test]
    fn cropping_vertically_leaves_no_transparent_margin() {
        let mut r = TextRenderer::new();
        let img = r
            .render("\u{2190}", 64.0, 200, [255, 255, 255, 255])
            .crop_to_ink_vertically();
        let (top, bottom) = img.ink_vertical_extent();
        assert!(top < 1e-3, "a blank row survived at the top: {top}");
        assert!(
            bottom > 1.0 - 1e-3,
            "a blank row survived at the bottom: {bottom}"
        );
    }

    #[test]
    fn cropping_vertically_does_not_touch_width() {
        // The width was already cropped to the ink by `render`; this must not redo, and
        // definitely must not undo, that.
        let mut r = TextRenderer::new();
        let img = r.render("\u{2190}", 64.0, 200, [255, 255, 255, 255]);
        let width_before = img.width;
        assert_eq!(img.crop_to_ink_vertically().width, width_before);
    }

    #[test]
    fn an_empty_image_reports_the_box_centre_rather_than_panicking() {
        let mut r = TextRenderer::new();
        let img = r.render("", 48.0, 200, [255, 255, 255, 255]);
        assert_eq!(img.ink_vertical_center(), 0.5);
        // And cropping it must not divide by, or index, anything that is not there.
        let cropped = img.crop_to_ink_vertically();
        assert!(cropped.height >= 1);
    }

    #[test]
    fn what_the_ink_centre_is_depends_on_the_glyphs_and_not_only_on_the_size() {
        // The property that makes a hard-coded 0.5 wrong. Where the ink sits inside the box is
        // a fact about *which glyphs* were set, not about the size they were set at -- so two
        // labels at one size can want two different centres, and a caller cannot pick one
        // number for both. Asserted as "the same string at two sizes agrees, and that agreement
        // is not what makes it right", which holds whatever font is installed.
        let mut r = TextRenderer::new();
        let small = r
            .render("space", 32.0, 400, [255, 255, 255, 255])
            .ink_vertical_center();
        let large = r
            .render("space", 64.0, 800, [255, 255, 255, 255])
            .ink_vertical_center();
        assert!(
            (small - large).abs() < 0.05,
            "the same word gave two centres at two sizes: {small} and {large}"
        );
        assert!(
            (0.0..=1.0).contains(&small),
            "centre outside the image: {small}"
        );
    }
}
