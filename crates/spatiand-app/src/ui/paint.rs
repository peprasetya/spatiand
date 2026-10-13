//! Drawing: rounded rectangles, pictures, and text.
//!
//! Three things, which is all the room's own cards are made of. One shader does all of them --
//! a rectangle with rounded corners, filled with a colour or with a texture -- and text is set
//! on the CPU by `spatiand-render`, at the picture's real resolution, and kept as textures for
//! as long as it goes on being drawn.

use std::collections::HashMap;

use glow::HasContext;
use spatiand_render::{TextImage, TextRenderer};

use super::{Color, Rect};
use crate::Gl;

const VERTEX: &str = r#"#version 300 es
precision highp float;
layout(location = 0) in vec2 a_corner;
// In logical pixels; `u_scale` is how many of the picture's pixels each is, each way, and
// `u_origin` where logical (0, 0) is in the picture.
uniform vec4 u_rect;
uniform vec2 u_scale;
uniform vec2 u_origin;
uniform vec2 u_view;
uniform vec4 u_uv;
out vec2 v_at;
out vec2 v_uv;
void main() {
    v_at = a_corner * u_rect.zw;
    v_uv = mix(u_uv.xy, u_uv.zw, a_corner);
    vec2 pixel = u_origin + (u_rect.xy + v_at) * u_scale;
    // Not flipped: row 0 of the picture is its top, and GL's y = -1 is row 0.
    gl_Position = vec4(pixel / u_view * 2.0 - 1.0, 0.0, 1.0);
}
"#;

const FRAGMENT: &str = r#"#version 300 es
precision highp float;
in vec2 v_at;
in vec2 v_uv;
uniform vec4 u_rect;
uniform vec2 u_scale;
uniform vec4 u_color;
uniform float u_radius;
// 0: the colour. 1: a texture with its own alpha, tinted. 2: a texture with none.
uniform int u_kind;
uniform sampler2D u_texture;
out vec4 f_color;
void main() {
    vec2 half_size = u_rect.zw * 0.5;
    // Distance to the rounded edge: the room's own corner (crates/spatiand/src/gl.rs). In
    // logical pixels, so a corner is round when the picture is shown and not merely in the
    // picture -- one eye of a film squeezed to half its width is drawn squeezed too.
    vec2 d = abs(v_at - half_size) - (half_size - vec2(u_radius));
    float dist = length(max(d, 0.0)) + min(max(d.x, d.y), 0.0) - u_radius;
    // One of the picture's pixels of softness either side of the edge.
    float soft = 0.7 / min(u_scale.x, u_scale.y);
    float cover = 1.0 - smoothstep(-soft, soft, dist);
    vec4 c = u_color;
    if (u_kind == 1) {
        c *= texture(u_texture, v_uv);
    } else if (u_kind == 2) {
        c *= vec4(texture(u_texture, v_uv).rgb, 1.0);
    }
    float a = c.a * cover;
    // Colour multiplied by alpha, which is what a window's picture is on the wire.
    f_color = vec4(c.rgb * a, a);
}
"#;

/// A picture on the GPU, to be drawn any number of times.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Texture {
    pub(crate) raw: glow::Texture,
    pub width: u32,
    pub height: u32,
}

impl Texture {
    /// Wrap a texture made elsewhere -- a decoder's picture, say.
    pub fn from_raw(raw: glow::Texture, width: u32, height: u32) -> Texture {
        Texture { raw, width, height }
    }

    pub fn raw(&self) -> glow::Texture {
        self.raw
    }
}

#[derive(Hash, PartialEq, Eq, Clone)]
struct TextKey {
    text: String,
    /// Em size and wrapping width in device pixels.
    size: u32,
    width: u32,
}

struct Set {
    texture: Texture,
    /// The frame it was last drawn in: text nobody draws any more is let go of.
    used: u64,
}

pub struct Painter {
    program: glow::Program,
    vao: glow::VertexArray,
    _corners: glow::Buffer,
    at: [Option<glow::UniformLocation>; 9],
    text: TextRenderer,
    set: HashMap<TextKey, Set>,
    frame: u64,
    view: (f32, f32),
    /// The picture's pixels to a logical one, across and down. The same, except where what
    /// is drawn will be stretched afterwards: one eye of a side-by-side pair.
    scale: (f32, f32),
    /// Where logical (0, 0) is in the picture, and the logical size being drawn into.
    origin: (f32, f32),
    logical: (f32, f32),
    clip: Vec<Rect>,
}

unsafe fn compile(g: &glow::Context, kind: u32, source: &str) -> Result<glow::Shader, String> {
    let shader = g.create_shader(kind)?;
    g.shader_source(shader, source);
    g.compile_shader(shader);
    if !g.get_shader_compile_status(shader) {
        return Err(format!("a shader would not compile: {}", g.get_shader_info_log(shader)));
    }
    Ok(shader)
}

impl Painter {
    pub fn new(gl: &Gl) -> Result<Painter, String> {
        let g = gl.api();
        unsafe {
            let program = g.create_program()?;
            let vertex = compile(g, glow::VERTEX_SHADER, VERTEX)?;
            let fragment = compile(g, glow::FRAGMENT_SHADER, FRAGMENT)?;
            g.attach_shader(program, vertex);
            g.attach_shader(program, fragment);
            g.link_program(program);
            if !g.get_program_link_status(program) {
                return Err(format!("the painter's shader would not link: {}", g.get_program_info_log(program)));
            }
            g.delete_shader(vertex);
            g.delete_shader(fragment);
            let vao = g.create_vertex_array()?;
            let corners = g.create_buffer()?;
            g.bind_vertex_array(Some(vao));
            g.bind_buffer(glow::ARRAY_BUFFER, Some(corners));
            let unit: [f32; 8] = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0];
            let bytes = std::slice::from_raw_parts(unit.as_ptr() as *const u8, std::mem::size_of_val(&unit));
            g.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes, glow::STATIC_DRAW);
            g.enable_vertex_attrib_array(0);
            g.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 0, 0);
            g.bind_vertex_array(None);
            let at = ["u_rect", "u_view", "u_uv", "u_color", "u_radius", "u_kind", "u_texture", "u_scale", "u_origin"]
                .map(|name| g.get_uniform_location(program, name));
            Ok(Painter {
                program,
                vao,
                _corners: corners,
                at,
                text: TextRenderer::new(),
                set: HashMap::new(),
                frame: 0,
                view: (1.0, 1.0),
                scale: (1.0, 1.0),
                origin: (0.0, 0.0),
                logical: (1.0, 1.0),
                clip: Vec::new(),
            })
        }
    }

    /// Start a picture of `size` device pixels, laid out at `scale` device pixels to a logical
    /// one, and clear it to `ground`.
    pub fn begin(&mut self, gl: &Gl, size: (u32, u32), scale: f32, ground: Color) {
        let g = gl.api();
        unsafe {
            g.disable(glow::SCISSOR_TEST);
            g.clear_color(ground[0] * ground[3], ground[1] * ground[3], ground[2] * ground[3], ground[3]);
            g.clear(glow::COLOR_BUFFER_BIT);
        }
        let logical = (size.0 as f32 / scale, size.1 as f32 / scale);
        self.begin_over(gl, size, Rect::new(0.0, 0.0, size.0 as f32, size.1 as f32), logical);
    }

    /// Start drawing over part of a picture that already has something in it: `part` of a
    /// picture `size` pixels in all, both in the picture's own pixels, laid out as `logical`
    /// pixels across and down. Nothing is cleared.
    ///
    /// The two scales need not agree. Controls over a film whose eyes are side by side are
    /// drawn once into each half, each as though it were the whole window -- and so squeezed
    /// by two, like the film under them, to come out right when each half is stretched over
    /// the window for its eye.
    pub fn begin_over(&mut self, gl: &Gl, size: (u32, u32), part: Rect, logical: (f32, f32)) {
        let g = gl.api();
        self.view = (size.0 as f32, size.1 as f32);
        self.origin = (part.x, part.y);
        self.logical = logical;
        self.scale = (part.w / logical.0.max(1.0), part.h / logical.1.max(1.0));
        self.frame += 1;
        self.clip.clear();
        unsafe {
            g.disable(glow::SCISSOR_TEST);
            g.disable(glow::DEPTH_TEST);
            g.disable(glow::CULL_FACE);
            g.enable(glow::BLEND);
            g.blend_func(glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
            g.use_program(Some(self.program));
            g.bind_vertex_array(Some(self.vao));
            g.uniform_2_f32(self.at[1].as_ref(), self.view.0, self.view.1);
            g.uniform_1_i32(self.at[6].as_ref(), 0);
            g.uniform_2_f32(self.at[7].as_ref(), self.scale.0, self.scale.1);
            g.uniform_2_f32(self.at[8].as_ref(), self.origin.0, self.origin.1);
            g.active_texture(glow::TEXTURE0);
        }
        // Text not drawn for a while is let go of, so a long list scrolled through does not
        // leave every label it ever showed on the GPU.
        let frame = self.frame;
        let stale: Vec<TextKey> = self.set.iter().filter(|(_, s)| s.used + 600 < frame).map(|(k, _)| k.clone()).collect();
        for key in stale {
            if let Some(set) = self.set.remove(&key) {
                unsafe { g.delete_texture(set.texture.raw) };
            }
        }
    }

    /// The picture's size in logical pixels.
    pub fn bounds(&self) -> Rect {
        Rect::new(0.0, 0.0, self.logical.0, self.logical.1)
    }

    /// The picture's pixels to a logical one, down the picture: what text is set at.
    pub fn scale(&self) -> f32 {
        self.scale.1
    }

    fn quad(&self, gl: &Gl, rect: Rect, radius: f32, color: Color, kind: i32, uv: [f32; 4]) {
        if rect.w <= 0.0 || rect.h <= 0.0 || color[3] <= 0.0 {
            return;
        }
        let g = gl.api();
        unsafe {
            g.uniform_4_f32(self.at[0].as_ref(), rect.x, rect.y, rect.w, rect.h);
            g.uniform_4_f32(self.at[2].as_ref(), uv[0], uv[1], uv[2], uv[3]);
            g.uniform_4_f32(self.at[3].as_ref(), color[0], color[1], color[2], color[3]);
            // Past half the shorter side the corner would fold back on itself.
            g.uniform_1_f32(self.at[4].as_ref(), radius.min(rect.w.min(rect.h) * 0.5));
            g.uniform_1_i32(self.at[5].as_ref(), kind);
            g.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
        }
    }

    /// A rectangle with rounded corners.
    pub fn fill(&mut self, gl: &Gl, rect: Rect, radius: f32, color: Color) {
        self.quad(gl, rect, radius, color, 0, [0.0, 0.0, 1.0, 1.0]);
    }

    /// A picture, stretched over `rect`. `uv` is the part of it to show: (left, top, right,
    /// bottom), each 0 to 1. `opaque` ignores the picture's own alpha, for one that has none
    /// worth the name -- a decoder's.
    #[allow(clippy::too_many_arguments)]
    pub fn image(&mut self, gl: &Gl, texture: Texture, rect: Rect, uv: [f32; 4], radius: f32, tint: Color, opaque: bool) {
        unsafe { gl.api().bind_texture(glow::TEXTURE_2D, Some(texture.raw)) };
        self.quad(gl, rect, radius, tint, if opaque { 2 } else { 1 }, uv);
    }

    /// A picture filling `rect` without being stretched: scaled until it covers, the overflow
    /// cut off equally on both sides.
    pub fn cover(&mut self, gl: &Gl, texture: Texture, rect: Rect, radius: f32, tint: Color) {
        let picture = texture.width as f32 / texture.height.max(1) as f32;
        let frame = rect.w / rect.h.max(1e-3);
        let uv = if picture > frame {
            let keep = frame / picture;
            [(1.0 - keep) * 0.5, 0.0, (1.0 + keep) * 0.5, 1.0]
        } else {
            let keep = picture / frame;
            [0.0, (1.0 - keep) * 0.5, 1.0, (1.0 + keep) * 0.5]
        };
        self.image(gl, texture, rect, uv, radius, tint, true);
    }

    /// Put a picture on the GPU: `rgba` is four bytes a pixel, alpha not multiplied in, top
    /// row first.
    pub fn texture(&mut self, gl: &Gl, rgba: &[u8], width: u32, height: u32) -> Result<Texture, String> {
        let g = gl.api();
        unsafe {
            let raw = g.create_texture()?;
            g.bind_texture(glow::TEXTURE_2D, Some(raw));
            for (name, value) in [
                (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
            ] {
                g.tex_parameter_i32(glow::TEXTURE_2D, name, value as i32);
            }
            g.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
            g.tex_image_2d(
                glow::TEXTURE_2D, 0, glow::RGBA8 as i32, width as i32, height as i32, 0, glow::RGBA, glow::UNSIGNED_BYTE,
                glow::PixelUnpackData::Slice(Some(rgba)),
            );
            Ok(Texture { raw, width, height })
        }
    }

    pub fn forget(&mut self, gl: &Gl, texture: Texture) {
        unsafe { gl.api().delete_texture(texture.raw) };
    }

    fn set(&mut self, gl: &Gl, text: &str, em: f32, width: f32) -> Option<Texture> {
        let key = TextKey {
            text: text.to_string(),
            size: (em * self.scale.1 * 16.0).round() as u32,
            width: (width * self.scale.1).round().max(1.0) as u32,
        };
        if let Some(set) = self.set.get_mut(&key) {
            set.used = self.frame;
            return Some(set.texture);
        }
        // White, and tinted when drawn: one texture serves a label in every state it has.
        let image: TextImage = self.text.render(text, em * self.scale.1, key.width, [255, 255, 255, 255]);
        let texture = self.texture(gl, &image.rgba, image.width, image.height).ok()?;
        self.set.insert(key, Set { texture, used: self.frame });
        Some(texture)
    }

    /// How much room `text` takes at `em`, wrapped to `width`: (width, height) in logical pixels.
    pub fn measure(&mut self, gl: &Gl, text: &str, em: f32, width: f32) -> (f32, f32) {
        match self.set(gl, text, em, width) {
            Some(texture) => (texture.width as f32 / self.scale.1, texture.height as f32 / self.scale.1),
            None => (0.0, 0.0),
        }
    }

    /// Set `text` with its top left at (`x`, `y`), wrapped to `width`. Returns the room it took.
    pub fn text(&mut self, gl: &Gl, text: &str, x: f32, y: f32, em: f32, width: f32, color: Color) -> (f32, f32) {
        if text.is_empty() {
            return (0.0, em * 1.4);
        }
        let Some(texture) = self.set(gl, text, em, width) else { return (0.0, 0.0) };
        let (w, h) = (texture.width as f32 / self.scale.1, texture.height as f32 / self.scale.1);
        // On whole device pixels, or the glyphs are resampled and go soft.
        let (sx, sy) = self.scale;
        self.image(gl, texture, Rect::new((x * sx).round() / sx, (y * sy).round() / sy, w, h), [0.0, 0.0, 1.0, 1.0], 0.0, color, false);
        (w, h)
    }

    /// One line of text in `rect`, middle to middle vertically, at its left edge, its right
    /// edge or centred, and cut short with an ellipsis if it does not fit.
    pub fn label(&mut self, gl: &Gl, text: &str, rect: Rect, em: f32, color: Color, align: f32) -> f32 {
        if text.is_empty() || rect.w <= 0.0 {
            return 0.0;
        }
        let fitted = self.fit(gl, text, em, rect.w);
        // Wide enough that it never wraps: this is a line.
        let wide = 8192.0 / self.scale.1;
        let (w, h) = self.measure(gl, &fitted, em, wide);
        let x = rect.x + (rect.w - w) * align;
        self.text(gl, &fitted, x, rect.y + (rect.h - h) * 0.5, em, wide, color);
        w
    }

    /// `text`, or as much of it as fits in `width` with an ellipsis after.
    fn fit(&mut self, gl: &Gl, text: &str, em: f32, width: f32) -> String {
        let wide = 8192.0 / self.scale.1;
        if self.measure(gl, text, em, wide).0 <= width {
            return text.to_string();
        }
        // The widths are near enough proportional to guess from, and a guess that is checked.
        let chars: Vec<char> = text.chars().collect();
        let mut keep = chars.len();
        while keep > 1 {
            keep = (keep * 9 / 10).min(keep - 1);
            let cut: String = chars[..keep].iter().collect::<String>().trim_end().to_string() + "\u{2026}";
            if self.measure(gl, &cut, em, wide).0 <= width {
                return cut;
            }
        }
        "\u{2026}".to_string()
    }

    /// Draw only inside `rect` until [`Painter::unclip`]: a list's rows scrolled under its edge.
    pub fn clip(&mut self, gl: &Gl, rect: Rect) {
        self.clip.push(rect);
        self.apply_clip(gl);
    }

    pub fn unclip(&mut self, gl: &Gl) {
        self.clip.pop();
        self.apply_clip(gl);
    }

    fn apply_clip(&self, gl: &Gl) {
        let g = gl.api();
        unsafe {
            match self.clip.last() {
                Some(rect) => {
                    let (sx, sy) = self.scale;
                    g.enable(glow::SCISSOR_TEST);
                    // Row 0 is the top here and the bottom to GL, which comes to the same thing.
                    g.scissor(
                        (self.origin.0 + rect.x * sx).floor() as i32,
                        (self.origin.1 + rect.y * sy).floor() as i32,
                        (rect.w * sx).ceil().max(0.0) as i32,
                        (rect.h * sy).ceil().max(0.0) as i32,
                    );
                }
                None => g.disable(glow::SCISSOR_TEST),
            }
        }
    }

    /// Finish the picture: leave GL as a caller that draws its own things would expect it.
    pub fn end(&mut self, gl: &Gl) {
        let g = gl.api();
        unsafe {
            g.disable(glow::SCISSOR_TEST);
            g.bind_vertex_array(None);
            g.use_program(None);
        }
    }
}
