//! Drawing a terminal's screen: every cell at once, in one draw.
//!
//! Glyphs are set once each by cosmic-text -- the room's own text engine -- in the
//! platform's monospaced face, kept in an atlas on the GPU, and drawn as quads in a single
//! call with the cells' backgrounds, underlines and the cursor. A character the face does not
//! have comes from whatever face does, as everywhere else in the room.

use std::collections::HashMap;

use cosmic_text::{Attrs, Buffer, Family, FontSystem, Metrics, Shaping, Style, SwashCache, SwashContent, Weight};
use spatiand_app::glow::{self, HasContext};
use spatiand_app::Gl;

use crate::wire::{flags, Cursor, Screen};

const ATLAS: i32 = 2048;

const VERTEX: &str = r#"#version 300 es
precision highp float;
layout(location = 0) in vec2 a_at;
layout(location = 1) in vec2 a_uv;
layout(location = 2) in vec4 a_colour;
layout(location = 3) in float a_kind;
uniform vec2 u_view;
out vec2 v_uv;
out vec4 v_colour;
out float v_kind;
void main() {
    v_uv = a_uv;
    v_colour = a_colour;
    v_kind = a_kind;
    gl_Position = vec4(a_at / u_view * 2.0 - 1.0, 0.0, 1.0);
}
"#;

const FRAGMENT: &str = r#"#version 300 es
precision highp float;
in vec2 v_uv;
in vec4 v_colour;
in float v_kind;
uniform sampler2D u_atlas;
out vec4 f_colour;
void main() {
    vec4 c = v_colour;
    if (v_kind > 1.5) {
        // A glyph in colour of its own: an emoji.
        c = texture(u_atlas, v_uv);
        c.a *= v_colour.a;
        f_colour = vec4(c.rgb * c.a, c.a);
        return;
    }
    if (v_kind > 0.5) {
        c.a *= texture(u_atlas, v_uv).a;
    }
    f_colour = vec4(c.rgb * c.a, c.a);
}
"#;

/// Glyph images, each at its top left from a common origin, made one: drawn each over the
/// ones before it. In colour if any of them is.
fn composed(parts: Vec<(i32, i32, i32, i32, Vec<u8>, bool)>) -> (i32, i32, i32, i32, Vec<u8>, bool) {
    let left = parts.iter().map(|p| p.0).min().unwrap_or(0);
    let top = parts.iter().map(|p| p.1).min().unwrap_or(0);
    let right = parts.iter().map(|p| p.0 + p.2).max().unwrap_or(0);
    let bottom = parts.iter().map(|p| p.1 + p.3).max().unwrap_or(0);
    let (w, h) = (right - left, bottom - top);
    let mut out = vec![0u8; (w * h * 4) as usize];
    for (x, y, pw, ph, rgba, _) in &parts {
        for row in 0..*ph {
            for col in 0..*pw {
                let s = ((row * pw + col) * 4) as usize;
                let d = (((y - top + row) * w + (x - left + col)) * 4) as usize;
                let (sa, da) = (rgba[s + 3] as f32 / 255.0, out[d + 3] as f32 / 255.0);
                let a = sa + da * (1.0 - sa);
                if a <= 0.0 {
                    continue;
                }
                for k in 0..3 {
                    out[d + k] = ((rgba[s + k] as f32 * sa + out[d + k] as f32 * da * (1.0 - sa)) / a).round() as u8;
                }
                out[d + 3] = (a * 255.0).round() as u8;
            }
        }
    }
    let colour = parts.iter().any(|p| p.5);
    (left, top, w, h, out, colour)
}

#[cfg(test)]
mod tests {
    use super::composed;

    #[test]
    fn a_letter_and_its_mark_make_one_image() {
        // A 2x2 letter at (0, 0) and a 1x1 mark above it at (1, -1).
        let letter = (0, 0, 2, 2, [255, 255, 255, 255].repeat(4), false);
        let mark = (1, -1, 1, 1, vec![255, 255, 255, 128], false);
        let (left, top, w, h, rgba, colour) = composed(vec![letter, mark]);
        assert_eq!((left, top, w, h, colour), (0, -1, 2, 3, false));
        let alpha: Vec<u8> = rgba.chunks(4).map(|p| p[3]).collect();
        assert_eq!(alpha, [0, 128, 255, 255, 255, 255]);
    }
}

#[derive(Clone, Copy)]
struct Glyph {
    /// Where it is in the atlas, in its pixels.
    uv: [f32; 4],
    /// Where its top left is from the cell's top left, and its size, in pixels.
    at: (f32, f32),
    size: (f32, f32),
    colour: bool,
}

pub struct Grid {
    program: glow::Program,
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    atlas: glow::Texture,
    fonts: FontSystem,
    swash: SwashCache,
    family: String,
    glyphs: HashMap<(char, u8), Option<Glyph>>,
    /// The same for a character with marks of no width (an accent, a selector): by all of it.
    clusters: HashMap<(String, u8), Option<Glyph>>,
    /// Where the next glyph goes in the atlas: a row at a time.
    pen: (i32, i32, i32),
    pub px: f32,
    pub cell: (f32, f32),
    baseline: f32,
    vertices: Vec<f32>,
}

/// The monospaced faces worth having, best first; the first this machine has is used.
const FACES: &[&str] = &["SF Mono", "JetBrains Mono", "Cascadia Code", "Fira Code", "Menlo", "DejaVu Sans Mono", "Noto Sans Mono", "Liberation Mono", "Droid Sans Mono", "Cutive Mono", "Monaco", "Courier New"];

unsafe fn compile(g: &glow::Context) -> Result<glow::Program, String> {
    let program = g.create_program()?;
    for (kind, source) in [(glow::VERTEX_SHADER, VERTEX), (glow::FRAGMENT_SHADER, FRAGMENT)] {
        let shader = g.create_shader(kind)?;
        g.shader_source(shader, source);
        g.compile_shader(shader);
        if !g.get_shader_compile_status(shader) {
            return Err(g.get_shader_info_log(shader));
        }
        g.attach_shader(program, shader);
    }
    g.link_program(program);
    if !g.get_program_link_status(program) {
        return Err(g.get_program_info_log(program));
    }
    Ok(program)
}

impl Grid {
    pub fn new(gl: &Gl, px: f32) -> Result<Grid, String> {
        let g = gl.api();
        // The room's own way of finding the machine's fonts: cosmic-text's finds none on Android.
        let fonts = spatiand_render::text::font_system();
        let family = {
            let db = fonts.db();
            FACES.iter().find(|face| db.faces().any(|f| f.families.iter().any(|(name, _)| name == *face))).map(|s| s.to_string())
        };
        let family = family.unwrap_or_else(|| "monospace".into());
        log::info!("terminal text in {family}");
        unsafe {
            let program = compile(g)?;
            let vao = g.create_vertex_array()?;
            let vbo = g.create_buffer()?;
            g.bind_vertex_array(Some(vao));
            g.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
            let stride = 9 * 4;
            for (location, size, offset) in [(0u32, 2i32, 0i32), (1, 2, 2), (2, 4, 4), (3, 1, 8)] {
                g.enable_vertex_attrib_array(location);
                g.vertex_attrib_pointer_f32(location, size, glow::FLOAT, false, stride, offset * 4);
            }
            g.bind_vertex_array(None);
            let atlas = g.create_texture()?;
            g.bind_texture(glow::TEXTURE_2D, Some(atlas));
            g.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, ATLAS, ATLAS, 0, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(None));
            g.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
            g.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
            g.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
            g.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
            let mut grid = Grid {
                program,
                vao,
                vbo,
                atlas,
                fonts,
                swash: SwashCache::new(),
                family,
                glyphs: HashMap::new(),
                clusters: HashMap::new(),
                pen: (1, 1, 0),
                px,
                cell: (px * 0.6, px * 1.3),
                baseline: px,
                vertices: Vec::new(),
            };
            grid.set_size(gl, px);
            Ok(grid)
        }
    }

fn attrs(family: &str, style: u8) -> Attrs<'_> {
        let mut attrs = Attrs::new().family(Family::Name(family));
        if style & 1 != 0 {
            attrs = attrs.weight(Weight::BOLD);
        }
        if style & 2 != 0 {
            attrs = attrs.style(Style::Italic);
        }
        attrs
    }

    /// Text this many pixels high, and the cells measured again for it.
    pub fn set_size(&mut self, gl: &Gl, px: f32) {
        self.px = px.clamp(8.0, 96.0);
        let line = (self.px * 1.28).round();
        let metrics = Metrics::new(self.px, line);
        let family = self.family.clone();
        let attrs = Self::attrs(&family, 0);
        let mut buffer = Buffer::new(&mut self.fonts, metrics);
        {
            let mut b = buffer.borrow_with(&mut self.fonts);
            b.set_size(Some(4096.0), Some(line * 2.0));
            b.set_text("MMMMMMMMMM", &attrs, Shaping::Advanced, None);
            b.shape_until_scroll(false);
        }
        let (mut width, mut baseline) = (self.px * 0.6, self.px);
        if let Some(run) = buffer.layout_runs().next() {
            width = run.line_w / 10.0;
            baseline = run.line_y;
        }
        self.cell = (width.round().max(1.0), line);
        self.baseline = baseline.round();
        self.forget_glyphs(gl);
    }

    fn forget_glyphs(&mut self, gl: &Gl) {
        self.glyphs.clear();
        self.clusters.clear();
        self.pen = (1, 1, 0);
        unsafe {
            let g = gl.api();
            g.bind_texture(glow::TEXTURE_2D, Some(self.atlas));
            let clear = vec![0u8; (ATLAS * 64 * 4) as usize];
            for y in (0..ATLAS).step_by(64) {
                g.tex_sub_image_2d(glow::TEXTURE_2D, 0, 0, y, ATLAS, 64, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(Some(&clear)));
            }
        }
    }

    /// The glyph for a cell: its character, or with `marks` the character and its marks, set
    /// together so that an accent sits on its letter and a selector picks the emoji.
    fn glyph(&mut self, gl: &Gl, c: char, marks: Option<&str>, style: u8) -> Option<Glyph> {
        let cluster = marks.map(|m| {
            let mut text = String::with_capacity(4 + m.len());
            text.push(c);
            text.push_str(m);
            text
        });
        let known = match &cluster {
            None => self.glyphs.get(&(c, style)),
            Some(text) => self.clusters.get(&(text.clone(), style)),
        };
        if let Some(glyph) = known {
            return *glyph;
        }
        let mut buffer = [0u8; 4];
        let text = cluster.as_deref().unwrap_or_else(|| c.encode_utf8(&mut buffer));
        let mut made = self.make(gl, text, style);
        // The atlas is full: start it again, and set this one first.
        if made.is_none() && self.pen.1 >= ATLAS - 4 {
            self.forget_glyphs(gl);
            made = self.make(gl, text, style);
        }
        match cluster {
            None => self.glyphs.insert((c, style), made),
            Some(text) => self.clusters.insert((text, style), made),
        };
        made
    }

    fn make(&mut self, gl: &Gl, text: &str, style: u8) -> Option<Glyph> {
        let metrics = Metrics::new(self.px, self.cell.1);
        let family = self.family.clone();
        let attrs = Self::attrs(&family, style);
        let mut buffer = Buffer::new(&mut self.fonts, metrics);
        {
            let mut b = buffer.borrow_with(&mut self.fonts);
            b.set_size(Some(self.cell.0 * 4.0), Some(self.cell.1 * 2.0));
            b.set_text(text, &attrs, Shaping::Advanced, None);
            b.shape_until_scroll(false);
        }
        // Every glyph the text is set as: one for a character; for a character with marks, one
        // if the face has them made up together, the letter and each mark if not, each where
        // the face puts it.
        let run = buffer.layout_runs().next()?;
        let mut parts = Vec::new();
        for glyph in run.glyphs.iter() {
            let physical = glyph.physical((0.0, 0.0), 1.0);
            let Some(image) = self.swash.get_image_uncached(&mut self.fonts, physical.cache_key) else { continue };
            let (w, h) = (image.placement.width as i32, image.placement.height as i32);
            if w == 0 || h == 0 {
                continue;
            }
            let colour = matches!(image.content, SwashContent::Color);
            let rgba: Vec<u8> = match image.content {
                SwashContent::Mask => image.data.iter().flat_map(|&a| [255, 255, 255, a]).collect(),
                SwashContent::Color => image.data.clone(),
                SwashContent::SubpixelMask => image.data.chunks_exact(4).flat_map(|p| [255, 255, 255, p[0].max(p[1]).max(p[2])]).collect(),
            };
            // Its top left from the pen at the baseline.
            parts.push((physical.x + image.placement.left, physical.y - image.placement.top, w, h, rgba, colour));
        }
        let (left, top, w, h, rgba, colour) = match parts.len() {
            0 => return None,
            1 => parts.pop()?,
            _ => composed(parts),
        };
        if self.pen.0 + w + 1 > ATLAS {
            self.pen = (1, self.pen.1 + self.pen.2 + 1, 0);
        }
        if self.pen.1 + h + 1 > ATLAS {
            self.pen.1 = ATLAS;
            return None;
        }
        let (x, y) = (self.pen.0, self.pen.1);
        unsafe {
            let g = gl.api();
            g.bind_texture(glow::TEXTURE_2D, Some(self.atlas));
            g.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            g.tex_sub_image_2d(glow::TEXTURE_2D, 0, x, y, w, h, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(Some(&rgba)));
        }
        self.pen.0 += w + 1;
        self.pen.2 = self.pen.2.max(h);
        let a = ATLAS as f32;
        Some(Glyph {
            uv: [x as f32 / a, y as f32 / a, (x + w) as f32 / a, (y + h) as f32 / a],
            at: (left as f32, self.baseline + top as f32),
            size: (w as f32, h as f32),
            colour,
        })
    }

    fn quad(&mut self, x: f32, y: f32, w: f32, h: f32, uv: [f32; 4], colour: [f32; 4], kind: f32) {
        let corners = [(x, y, uv[0], uv[1]), (x + w, y, uv[2], uv[1]), (x, y + h, uv[0], uv[3]), (x + w, y, uv[2], uv[1]), (x + w, y + h, uv[2], uv[3]), (x, y + h, uv[0], uv[3])];
        for (px, py, u, v) in corners {
            self.vertices.extend_from_slice(&[px, py, u, v, colour[0], colour[1], colour[2], colour[3], kind]);
        }
    }

    /// Draw `screen` with its top left at (`x`, `y`) in a picture `view` pixels big. `focused`
    /// draws the cursor solid and `blink_on` says whether it is in the lit half of its blink.
    #[allow(clippy::too_many_arguments)]
    pub fn draw(&mut self, gl: &Gl, view: (u32, u32), x: f32, y: f32, screen: &Screen, focused: bool, blink_on: bool, cursor_colour: [f32; 4], selection: [f32; 4]) {
        self.vertices.clear();
        let (cw, ch) = self.cell;
        let cols = screen.cols as usize;
        let rgb = |c: [u8; 3], a: f32| [c[0] as f32 / 255.0, c[1] as f32 / 255.0, c[2] as f32 / 255.0, a];
        // Backgrounds first, a run of one colour at a time.
        for row in 0..screen.rows as usize {
            let line = &screen.cells[row * cols..(row + 1) * cols];
            let mut col = 0;
            while col < cols {
                let cell = line[col];
                let selected = cell.flags & flags::SELECTED != 0;
                if cell.flags & flags::PLAIN_BG != 0 && !selected {
                    col += 1;
                    continue;
                }
                let start = col;
                while col < cols && line[col].bg == cell.bg && (line[col].flags & flags::PLAIN_BG) == (cell.flags & flags::PLAIN_BG) && (line[col].flags & flags::SELECTED) == (cell.flags & flags::SELECTED) {
                    col += 1;
                }
                let colour = if selected { selection } else { rgb(cell.bg, 1.0) };
                self.quad(x + start as f32 * cw, y + row as f32 * ch, (col - start) as f32 * cw, ch, [0.0; 4], colour, 0.0);
            }
        }
        // The cursor under its character, when it is a block.
        let (ccol, crow) = (screen.cursor.0 as usize, screen.cursor.1 as usize);
        let cursor_here = screen.cursor_shape != Cursor::Hidden && ccol < cols && crow < screen.rows as usize && (blink_on || !focused);
        let cursor_wide = cursor_here && screen.cells[crow * cols + ccol].flags & flags::WIDE != 0;
        let (cx, cy) = (x + ccol as f32 * cw, y + crow as f32 * ch);
        let cursor_w = if cursor_wide { cw * 2.0 } else { cw };
        if cursor_here && screen.cursor_shape == Cursor::Block && focused {
            self.quad(cx, cy, cursor_w, ch, [0.0; 4], cursor_colour, 0.0);
        }
        // Then the characters.
        for row in 0..screen.rows as usize {
            for col in 0..cols {
                let cell = screen.cells[row * cols + col];
                let marks = if screen.marks.is_empty() { None } else { screen.marks_at(row * cols + col) };
                if (cell.c == ' ' || cell.c == '\0') && marks.is_none() || cell.flags & (flags::SPACER | flags::HIDDEN) != 0 {
                    continue;
                }
                let style = (cell.flags & flags::BOLD != 0) as u8 | ((cell.flags & flags::ITALIC != 0) as u8) << 1;
                let Some(glyph) = self.glyph(gl, cell.c, marks, style) else { continue };
                let on_cursor = cursor_here && focused && screen.cursor_shape == Cursor::Block && row == crow && col == ccol;
                let mut colour = if on_cursor { rgb(crate::tab::BACKGROUND, 1.0) } else { rgb(cell.fg, 1.0) };
                if cell.flags & flags::DIM != 0 {
                    colour[3] = 0.6;
                }
                let (gx, gy) = (x + col as f32 * cw + glyph.at.0, y + row as f32 * ch + glyph.at.1);
                // A glyph wider than its cells (an emoji) is made to fit them.
                let room = if cell.flags & flags::WIDE != 0 { cw * 2.0 } else { cw };
                let (gw, gh, gx, gy) = if glyph.colour && glyph.size.0 > room * 1.05 {
                    let s = room / glyph.size.0;
                    (glyph.size.0 * s, glyph.size.1 * s, x + col as f32 * cw, y + row as f32 * ch + (ch - glyph.size.1 * s) * 0.5)
                } else {
                    (glyph.size.0, glyph.size.1, gx, gy)
                };
                self.quad(gx, gy, gw, gh, glyph.uv, colour, if glyph.colour { 2.0 } else { 1.0 });
            }
            // Lines under and through, a run at a time.
            for col in 0..cols {
                let cell = screen.cells[row * cols + col];
                let colour = rgb(cell.fg, 1.0);
                let thick = (self.px / 14.0).max(1.0).round();
                if cell.flags & flags::UNDERLINE != 0 {
                    self.quad(x + col as f32 * cw, y + row as f32 * ch + self.baseline + thick * 1.5, cw, thick, [0.0; 4], colour, 0.0);
                }
                if cell.flags & flags::STRIKE != 0 {
                    self.quad(x + col as f32 * cw, y + row as f32 * ch + self.baseline - self.px * 0.3, cw, thick, [0.0; 4], colour, 0.0);
                }
            }
        }
        if cursor_here {
            let thin = (self.px / 9.0).max(2.0).round();
            match (screen.cursor_shape, focused) {
                (Cursor::Beam, _) => self.quad(cx, cy, thin, ch, [0.0; 4], cursor_colour, 0.0),
                (Cursor::Underline, _) => self.quad(cx, cy + ch - thin, cursor_w, thin, [0.0; 4], cursor_colour, 0.0),
                // Not focused: an outline, as every terminal shows where the cursor is without
                // claiming the keys.
                (_, false) => {
                    self.quad(cx, cy, cursor_w, thin * 0.6, [0.0; 4], cursor_colour, 0.0);
                    self.quad(cx, cy + ch - thin * 0.6, cursor_w, thin * 0.6, [0.0; 4], cursor_colour, 0.0);
                    self.quad(cx, cy, thin * 0.6, ch, [0.0; 4], cursor_colour, 0.0);
                    self.quad(cx + cursor_w - thin * 0.6, cy, thin * 0.6, ch, [0.0; 4], cursor_colour, 0.0);
                }
                _ => {}
            }
        }
        if self.vertices.is_empty() {
            return;
        }
        let g = gl.api();
        unsafe {
            g.use_program(Some(self.program));
            g.uniform_2_f32(g.get_uniform_location(self.program, "u_view").as_ref(), view.0 as f32, view.1 as f32);
            g.uniform_1_i32(g.get_uniform_location(self.program, "u_atlas").as_ref(), 0);
            g.active_texture(glow::TEXTURE0);
            g.bind_texture(glow::TEXTURE_2D, Some(self.atlas));
            g.enable(glow::BLEND);
            g.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            g.bind_vertex_array(Some(self.vao));
            g.bind_buffer(glow::ARRAY_BUFFER, Some(self.vbo));
            let bytes = std::slice::from_raw_parts(self.vertices.as_ptr() as *const u8, self.vertices.len() * 4);
            g.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STREAM_DRAW);
            g.draw_arrays(glow::TRIANGLES, 0, (self.vertices.len() / 9) as i32);
            g.bind_vertex_array(None);
            g.use_program(None);
        }
    }
}
