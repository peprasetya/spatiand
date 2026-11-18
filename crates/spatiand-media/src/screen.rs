//! Drawing what the engine decodes.
//!
//! A decoder's picture is brightness and two colour differences in separate planes, and stays
//! that way to the GPU: each plane is a texture, and one shader turns them into colour as it
//! draws -- so a picture decoded on the GPU is never brought back from it, and one decoded in
//! main memory is uploaded once and not converted there first.
//!
//! Two ways of drawing it. [`Screen::draw`] puts a part of the picture -- all of it, or one
//! eye's half -- into a rectangle. [`Screen::look`] is for a picture that is the inside of a
//! sphere, where there is no room to wrap it round: a view out from the centre in some
//! direction, as a camera would see it.

use std::sync::Arc;

use glam::{Mat3, Vec3};
use spatiand_app::glow::{self, HasContext};
use spatiand_app::Gl;

use crate::player::{Colour, Light, Picture, Pixels};

const VERTEX: &str = r#"#version 300 es
precision highp float;
layout(location = 0) in vec2 a_corner;
uniform vec4 u_rect;
uniform vec2 u_view;
out vec2 v_at;
void main() {
    v_at = a_corner;
    vec2 pixel = u_rect.xy + a_corner * u_rect.zw;
    // Row 0 is the top of the picture being drawn into: see spatiand_app::gl.
    gl_Position = vec4(pixel / u_view * 2.0 - 1.0, 0.0, 1.0);
}
"#;

/// The planes of a picture, made colour here.
const PLANES: &str = r#"#version 300 es
precision highp float;
uniform sampler2D u_y;
uniform sampler2D u_u;
uniform sampler2D u_v;
// 3: three planes. 2: two, the second holding both colour differences.
uniform int u_planes;
uniform mat3 u_matrix;
uniform vec3 u_offset;
uniform float u_deep;

vec3 decoded(vec2 uv) {
    vec3 yuv;
    yuv.x = texture(u_y, uv).r;
    if (u_planes == 2) {
        yuv.yz = texture(u_u, uv).rg;
    } else {
        yuv.y = texture(u_u, uv).r;
        yuv.z = texture(u_v, uv).r;
    }
    // Ten bits in sixteen read as a fraction of sixteen: `u_deep` makes them a fraction of ten.
    return u_matrix * (yuv * u_deep - u_offset);
}
"#;

/// A picture in an Android hardware buffer (`Pixels::HardwareBuffer`): the GPU makes it colour
/// as it samples it, by what the buffer says it is.
#[cfg_attr(not(target_os = "android"), allow(dead_code))]
const EXTERNAL: &str = r#"#version 300 es
#extension GL_OES_EGL_image_external_essl3 : require
precision highp float;
uniform samplerExternalOES u_ext;
// The part of the buffer that is picture: left, top, right, bottom.
uniform vec4 u_crop;

vec3 decoded(vec2 uv) {
    return texture(u_ext, mix(u_crop.xy, u_crop.zw, uv)).rgb;
}
"#;

/// The rest, the same for both: which part of the picture is shown, and HDR's light.
const FRAGMENT: &str = r#"
in vec2 v_at;
// 0: an ordinary film. 1: PQ. 2: HLG.
uniform int u_light;
// The part of the picture to show: left, top, right, bottom.
uniform vec4 u_uv;
// 0: flat. Otherwise how many radians of longitude the picture covers, looked at from inside.
uniform float u_span;
uniform mat3 u_gaze;
// The tangents of half the view's width and height.
uniform vec2 u_tan;
out vec4 f_color;

vec3 colour(vec2 uv) {
    vec3 rgb = decoded(uv);
    if (u_light == 0) {
        return clamp(rgb, 0.0, 1.0);
    }
    // An HDR film's numbers stand for far more light than these glasses give. They are turned
    // into light, the light is brought within what there is, and that is turned back into the
    // numbers an ordinary picture has.
    rgb = clamp(rgb, 0.0, 1.0);
    vec3 nits;
    if (u_light == 1) {
        // SMPTE 2084, backwards.
        vec3 p = pow(rgb, vec3(1.0 / 78.84375));
        nits = 10000.0 * pow(max(p - 0.8359375, 0.0) / (18.8515625 - 18.6875 * p), vec3(1.0 / 0.1593017578125));
    } else {
        // Hybrid log-gamma, backwards, and then as a thousand-nit screen would show it.
        vec3 scene = mix(rgb * rgb / 3.0, (exp((rgb - 0.55991073) / 0.17883277) + 0.28466892) / 12.0, step(0.5, rgb));
        float y = dot(scene, vec3(0.2627, 0.6780, 0.0593));
        nits = 1000.0 * scene * pow(max(y, 1e-6), 0.2);
    }
    // White on paper is 203 nits in an HDR film and all there is in an ordinary one. Above
    // it the light is rolled off, by its brightness and not channel by channel, so that a
    // bright colour keeps its colour.
    vec3 linear = nits / 203.0;
    float y = dot(linear, vec3(0.2627, 0.6780, 0.0593));
    // Up to three quarters of white it is left alone -- most of any picture, which is then
    // exactly as bright as it was meant -- and from there to a thousand nits is eased into
    // the quarter that is left.
    float knee = 0.75;
    float peak = 1000.0 / 203.0;
    float over = clamp((y - knee) / (peak - knee), 0.0, 1.0);
    float rolled = y <= knee ? y : knee + (1.0 - knee) * (1.0 - exp(-4.0 * over)) / (1.0 - exp(-4.0));
    linear *= rolled / max(y, 1e-6);
    // From HDR's wide colours to an ordinary screen's.
    linear = mat3(1.6605, -0.1246, -0.0182, -0.5876, 1.1329, -0.1006, -0.0728, -0.0083, 1.1187) * linear;
    return pow(clamp(linear, 0.0, 1.0), vec3(1.0 / 2.2));
}

void main() {
    vec2 at = v_at;
    if (u_span > 0.0) {
        // The ray through this pixel, turned to where the viewer is looking.
        vec3 ray = u_gaze * normalize(vec3((v_at.x * 2.0 - 1.0) * u_tan.x, (1.0 - v_at.y * 2.0) * u_tan.y, -1.0));
        float longitude = atan(ray.x, -ray.z);
        float latitude = asin(clamp(ray.y, -1.0, 1.0));
        if (abs(longitude) > u_span * 0.5) {
            // Behind a picture that is only the front half there is nothing, and nothing is
            // what is shown: not its edge, smeared round.
            f_color = vec4(0.0, 0.0, 0.0, 1.0);
            return;
        }
        at = vec2(longitude / u_span + 0.5, 0.5 - latitude / 3.14159265);
    }
    f_color = vec4(colour(mix(u_uv.xy, u_uv.zw, at)), 1.0);
}
"#;

/// The numbers that turn a picture's brightness and colour differences into red, green and
/// blue: `rgb = matrix * (yuv - offset)`.
pub fn conversion(colour: Colour) -> (Mat3, Vec3) {
    let (kr, kb) = if colour.wide {
        (0.2627, 0.0593)
    } else if colour.hd {
        (0.2126, 0.0722)
    } else {
        (0.299, 0.114)
    };
    let kg = 1.0 - kr - kb;
    // Broadcast range keeps brightness in 16..235 and the differences in 16..240.
    let (luma, chroma, floor) = if colour.full_range { (1.0, 1.0, 0.0) } else { (255.0 / 219.0, 255.0 / 224.0, 16.0 / 255.0) };
    let matrix = Mat3::from_cols(
        Vec3::new(luma, luma, luma),
        Vec3::new(0.0, -2.0 * kb * (1.0 - kb) / kg * chroma, 2.0 * (1.0 - kb) * chroma),
        Vec3::new(2.0 * (1.0 - kr) * chroma, -2.0 * kr * (1.0 - kr) / kg * chroma, 0.0),
    );
    (matrix, Vec3::new(floor, 128.0 / 255.0, 128.0 / 255.0))
}

/// Where a viewer inside a sphere is looking.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gaze {
    /// Radians, positive to the right.
    pub yaw: f32,
    /// Radians, positive upwards.
    pub pitch: f32,
    /// How much is seen from side to side, in radians.
    pub field: f32,
}

impl Default for Gaze {
    fn default() -> Self {
        Gaze { yaw: 0.0, pitch: 0.0, field: 100f32.to_radians() }
    }
}

/// A compiled shader and where its uniforms are.
struct Program {
    program: glow::Program,
    at: [Option<glow::UniformLocation>; 16],
}

impl Program {
    fn new(g: &glow::Context, decoding: &str) -> Result<Program, String> {
        unsafe {
            let program = g.create_program()?;
            let fragment = format!("{decoding}{FRAGMENT}");
            for (kind, source) in [(glow::VERTEX_SHADER, VERTEX), (glow::FRAGMENT_SHADER, fragment.as_str())] {
                let shader = g.create_shader(kind)?;
                g.shader_source(shader, source);
                g.compile_shader(shader);
                if !g.get_shader_compile_status(shader) {
                    return Err(format!("the film's shader would not compile: {}", g.get_shader_info_log(shader)));
                }
                g.attach_shader(program, shader);
                g.delete_shader(shader);
            }
            g.link_program(program);
            if !g.get_program_link_status(program) {
                return Err(format!("the film's shader would not link: {}", g.get_program_info_log(program)));
            }
            Ok(Program { program, at: NAMES.map(|name| g.get_uniform_location(program, name)) })
        }
    }
}

pub struct Screen {
    /// For pictures in planes, in memory or lent by the platform's decoder.
    planar: Program,
    /// For pictures in Android hardware buffers, which the GPU samples as they are.
    #[cfg(target_os = "android")]
    external: Option<Program>,
    vao: glow::VertexArray,
    _corners: glow::Buffer,
    /// Textures of its own, for a picture in main memory.
    planes: [glow::Texture; 3],
    /// A picture on the GPU already, as textures that are its own memory.
    #[cfg(target_os = "macos")]
    borrowed: Vec<spatiand_app::gl::SurfaceTexture>,
    #[cfg(target_os = "linux")]
    borrowed: Vec<spatiand_app::gl::DmabufTexture>,
    #[cfg(target_os = "android")]
    borrowed: Vec<spatiand_app::gl::HardwareBufferTexture>,
    /// The picture the textures hold, kept for as long as they do.
    loaded: Option<Arc<Picture>>,
    /// The textures hold ten bits in sixteen, to be scaled up as they are read.
    deep: bool,
}

const NAMES: [&str; 16] = [
    "u_rect", "u_view", "u_y", "u_u", "u_v", "u_planes", "u_matrix", "u_offset", "u_uv", "u_span", "u_gaze", "u_tan", "u_deep", "u_light",
    "u_ext", "u_crop",
];

impl Screen {
    pub fn new(gl: &Gl) -> Result<Screen, String> {
        let g = gl.api();
        let planar = Program::new(g, PLANES)?;
        // Every Android GPU samples external images from GLSL ES 3; one that would not could
        // show no decoder's picture, and says so.
        #[cfg(target_os = "android")]
        let external = Program::new(g, EXTERNAL).map_err(|e| log::warn!("no film decoded by MediaCodec can be shown: {e}")).ok();
        unsafe {
            let vao = g.create_vertex_array()?;
            let corners = g.create_buffer()?;
            g.bind_vertex_array(Some(vao));
            g.bind_buffer(glow::ARRAY_BUFFER, Some(corners));
            let unit: [f32; 8] = [0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0];
            g.buffer_data_u8_slice(glow::ARRAY_BUFFER, std::slice::from_raw_parts(unit.as_ptr() as *const u8, 32), glow::STATIC_DRAW);
            g.enable_vertex_attrib_array(0);
            g.vertex_attrib_pointer_f32(0, 2, glow::FLOAT, false, 0, 0);
            g.bind_vertex_array(None);
            let mut planes = [g.create_texture()?; 3];
            for plane in planes.iter_mut().skip(1) {
                *plane = g.create_texture()?;
            }
            for plane in planes {
                g.bind_texture(glow::TEXTURE_2D, Some(plane));
                for (name, value) in [
                    (glow::TEXTURE_MIN_FILTER, glow::LINEAR),
                    (glow::TEXTURE_MAG_FILTER, glow::LINEAR),
                    (glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE),
                    (glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE),
                ] {
                    g.tex_parameter_i32(glow::TEXTURE_2D, name, value as i32);
                }
            }
            Ok(Screen {
                planar,
                #[cfg(target_os = "android")]
                external,
                vao,
                _corners: corners,
                planes,
                #[cfg(any(target_os = "macos", target_os = "linux", target_os = "android"))]
                borrowed: Vec::new(),
                loaded: None,
                deep: false,
            })
        }
    }

    /// Whether there is a picture to draw.
    pub fn has_picture(&self) -> bool {
        self.loaded.is_some()
    }

    /// The size of the picture loaded.
    pub fn size(&self) -> (u32, u32) {
        self.loaded.as_ref().map_or((0, 0), |p| (p.width, p.height))
    }

    /// Let go of the picture, and of the decoder's memory with it.
    pub fn clear(&mut self, gl: &Gl) {
        #[cfg(target_os = "macos")]
        for held in self.borrowed.drain(..) {
            gl.release_surface_texture(held);
        }
        #[cfg(target_os = "linux")]
        for held in self.borrowed.drain(..) {
            gl.release_dmabuf_texture(held);
        }
        // The texture goes before the picture, whose dropping gives its buffer back to the
        // decoder. What was drawn from it is done by then: the window's picture was read back, or
        // finished, when it was handed over (spatiand_app::gl).
        #[cfg(target_os = "android")]
        for held in self.borrowed.drain(..) {
            gl.release_hardware_buffer_texture(held);
        }
        let _ = gl;
        self.loaded = None;
    }

    /// Let go of everything it made on the GPU -- its programs, its corners and the textures of
    /// its own -- and of the picture: for a screen that will not draw again. Dropped without
    /// this, every film watched left them behind for as long as the program ran.
    pub fn delete(mut self, gl: &Gl) {
        self.clear(gl);
        let g = gl.api();
        unsafe {
            g.delete_program(self.planar.program);
            #[cfg(target_os = "android")]
            if let Some(external) = self.external.as_ref() {
                g.delete_program(external.program);
            }
            g.delete_vertex_array(self.vao);
            g.delete_buffer(self._corners);
            for plane in self.planes {
                g.delete_texture(plane);
            }
        }
    }

    /// Make `picture` the one that is drawn. Costs nothing if it already is.
    pub fn load(&mut self, gl: &Gl, picture: &Arc<Picture>) {
        if self.loaded.as_ref().is_some_and(|loaded| Arc::ptr_eq(loaded, picture)) {
            return;
        }
        self.clear(gl);
        let g = gl.api();
        let (w, h) = (picture.width as i32, picture.height as i32);
        let half = ((w + 1) / 2, (h + 1) / 2);
        unsafe {
            match picture.pixels {
                Pixels::Planar420Ten => {
                    // Sixteen bits a sample where the GPU will have them as a texture it can
                    // smooth; brought down to eight where it will not.
                    let sixteen = g.supported_extensions().contains("GL_EXT_texture_norm16");
                    self.deep = sixteen;
                    g.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
                    for (index, (pw, ph)) in [(w, h), half, half].into_iter().enumerate() {
                        let (data, stride) = picture.plane(index);
                        g.bind_texture(glow::TEXTURE_2D, Some(self.planes[index]));
                        if sixteen {
                            const R16: u32 = 0x822A;
                            g.pixel_store_i32(glow::UNPACK_ROW_LENGTH, stride as i32 / 2);
                            g.tex_image_2d(glow::TEXTURE_2D, 0, R16 as i32, pw, ph, 0, glow::RED, glow::UNSIGNED_SHORT, glow::PixelUnpackData::Slice(Some(data)));
                        } else {
                            let mut eight = Vec::with_capacity((pw * ph) as usize);
                            for row in 0..ph as usize {
                                let line = &data[row * stride..row * stride + pw as usize * 2];
                                eight.extend(line.chunks_exact(2).map(|pair| (u16::from_le_bytes([pair[0], pair[1]]) >> 2) as u8));
                            }
                            g.pixel_store_i32(glow::UNPACK_ROW_LENGTH, 0);
                            g.tex_image_2d(glow::TEXTURE_2D, 0, glow::R8 as i32, pw, ph, 0, glow::RED, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(Some(&eight)));
                        }
                    }
                    g.pixel_store_i32(glow::UNPACK_ROW_LENGTH, 0);
                }
                Pixels::Planar420 | Pixels::Nv12 => {
                    let two = picture.pixels == Pixels::Nv12;
                    let shapes: &[(i32, i32, u32, u32, i32)] = if two {
                        &[(w, h, glow::R8, glow::RED, 1), (half.0, half.1, glow::RG8, glow::RG, 2)]
                    } else {
                        &[(w, h, glow::R8, glow::RED, 1), (half.0, half.1, glow::R8, glow::RED, 1), (half.0, half.1, glow::R8, glow::RED, 1)]
                    };
                    g.pixel_store_i32(glow::UNPACK_ALIGNMENT, 1);
                    for (index, (pw, ph, internal, format, bytes)) in shapes.iter().enumerate() {
                        let (data, stride) = picture.plane(index);
                        g.bind_texture(glow::TEXTURE_2D, Some(self.planes[index]));
                        // A row in memory is longer than a row of picture, padded for the
                        // decoder's convenience: GL is told how long, and skips the rest.
                        g.pixel_store_i32(glow::UNPACK_ROW_LENGTH, stride as i32 / bytes);
                        g.tex_image_2d(glow::TEXTURE_2D, 0, *internal as i32, *pw, *ph, 0, *format, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(Some(data)));
                    }
                    g.pixel_store_i32(glow::UNPACK_ROW_LENGTH, 0);
                }
                Pixels::Dmabuf | Pixels::Dmabuf10 => {
                    #[cfg(target_os = "linux")]
                    {
                        use std::os::fd::BorrowedFd;
                        // "R8  " and "GR88": a byte of brightness, two bytes of colour. Ten
                        // bits: "R16 " and "GR32" (GR1616), sixteen bits each.
                        const R8: u32 = u32::from_le_bytes(*b"R8  ");
                        const GR88: u32 = u32::from_le_bytes(*b"GR88");
                        const R16: u32 = u32::from_le_bytes(*b"R16 ");
                        const GR1616: u32 = u32::from_le_bytes(*b"GR32");
                        let (luma, chroma) = if picture.pixels == Pixels::Dmabuf10 { (R16, GR1616) } else { (R8, GR88) };
                        let planes = picture.dmabuf_planes();
                        if planes.len() < 2 {
                            log::warn!("a decoded picture was lent with {} planes", planes.len());
                            return;
                        }
                        for (plane, size, fourcc) in [(planes[0], (w as u32, h as u32), luma), (planes[1], (half.0 as u32, half.1 as u32), chroma)] {
                            // SAFETY: the descriptor is the picture's, which is held below
                            // for as long as the texture is.
                            let fd = BorrowedFd::borrow_raw(plane.fd);
                            match gl.dmabuf_texture(fd, plane.offset, plane.pitch, plane.modifier, size, fourcc) {
                                Ok(held) => self.borrowed.push(held),
                                Err(e) => {
                                    log::warn!("{e}");
                                    self.clear(gl);
                                    return;
                                }
                            }
                        }
                    }
                }
                Pixels::HardwareBuffer => {
                    #[cfg(target_os = "android")]
                    {
                        let Some((buffer, _)) = picture.hardware_buffer() else { return };
                        match gl.hardware_buffer_texture(buffer) {
                            Ok(held) => self.borrowed.push(held),
                            Err(e) => {
                                log::warn!("{e}");
                                return;
                            }
                        }
                    }
                }
                Pixels::VideoToolbox => {
                    #[cfg(target_os = "macos")]
                    {
                        let surface = CVPixelBufferGetIOSurface(picture.pixel_buffer());
                        if surface.is_null() {
                            log::warn!("a decoded picture has no surface behind it");
                            return;
                        }
                        for (plane, size, channels) in [(0, (w as u32, h as u32), 1), (1, (half.0 as u32, half.1 as u32), 2)] {
                            match gl.surface_texture(surface, plane, size, channels) {
                                Ok(held) => self.borrowed.push(held),
                                Err(e) => {
                                    log::warn!("{e}");
                                    self.clear(gl);
                                    return;
                                }
                            }
                        }
                    }
                }
            }
        }
        self.loaded = Some(picture.clone());
    }

    /// The program for the picture loaded, made current with the picture's textures and the
    /// uniforms every drawing has; none if there is nothing to draw.
    unsafe fn bind(&self, gl: &Gl, view: (u32, u32), rect: [f32; 4], uv: [f32; 4]) -> Option<&Program> {
        let picture = self.loaded.as_ref()?;
        let g = gl.api();
        #[cfg(any(target_os = "macos", target_os = "linux", target_os = "android"))]
        let borrowed: Vec<glow::Texture> = self.borrowed.iter().map(|held| held.texture).collect();
        #[cfg(not(any(target_os = "macos", target_os = "linux", target_os = "android")))]
        let borrowed: Vec<glow::Texture> = Vec::new();
        let program = if picture.pixels == Pixels::HardwareBuffer {
            #[cfg(target_os = "android")]
            {
                let program = self.external.as_ref()?;
                let texture = borrowed.first()?;
                let crop = picture.hardware_buffer().map_or([0.0, 0.0, 1.0, 1.0], |(_, crop)| crop);
                g.use_program(Some(program.program));
                g.active_texture(glow::TEXTURE0);
                g.bind_texture(spatiand_app::gl::TEXTURE_EXTERNAL_OES, Some(*texture));
                g.uniform_1_i32(program.at[14].as_ref(), 0);
                g.uniform_4_f32(program.at[15].as_ref(), crop[0], crop[1], crop[2], crop[3]);
                program
            }
            #[cfg(not(target_os = "android"))]
            return None;
        } else {
            let on_gpu = matches!(picture.pixels, Pixels::VideoToolbox | Pixels::Dmabuf | Pixels::Dmabuf10);
            if on_gpu && borrowed.len() < 2 {
                return None;
            }
            let textures: &[glow::Texture] = if on_gpu { &borrowed } else { &self.planes };
            let two = !matches!(picture.pixels, Pixels::Planar420 | Pixels::Planar420Ten);
            let program = &self.planar;
            g.use_program(Some(program.program));
            for (unit, texture) in textures.iter().enumerate().take(if two { 2 } else { 3 }) {
                g.active_texture(glow::TEXTURE0 + unit as u32);
                g.bind_texture(glow::TEXTURE_2D, Some(*texture));
            }
            g.active_texture(glow::TEXTURE0);
            let (matrix, offset) = conversion(picture.colour);
            g.uniform_1_i32(program.at[2].as_ref(), 0);
            g.uniform_1_i32(program.at[3].as_ref(), 1);
            g.uniform_1_i32(program.at[4].as_ref(), if two { 1 } else { 2 });
            g.uniform_1_i32(program.at[5].as_ref(), if two { 2 } else { 3 });
            g.uniform_matrix_3_f32_slice(program.at[6].as_ref(), false, &matrix.to_cols_array());
            g.uniform_3_f32(program.at[7].as_ref(), offset.x, offset.y, offset.z);
            // Ten bits in the bottom of sixteen are a fraction of sixteen read, made a fraction of
            // ten; ten in the top of sixteen (P010) want only the last 63 of 65535 back.
            let scale = match picture.pixels {
                Pixels::Planar420Ten if self.deep => 65535.0 / 1023.0,
                Pixels::Dmabuf10 => 65535.0 / 65472.0,
                _ => 1.0,
            };
            g.uniform_1_f32(program.at[12].as_ref(), scale);
            program
        };
        g.bind_vertex_array(Some(self.vao));
        g.disable(glow::BLEND);
        g.uniform_4_f32(program.at[0].as_ref(), rect[0], rect[1], rect[2], rect[3]);
        g.uniform_2_f32(program.at[1].as_ref(), view.0 as f32, view.1 as f32);
        g.uniform_4_f32(program.at[8].as_ref(), uv[0], uv[1], uv[2], uv[3]);
        g.uniform_1_i32(
            program.at[13].as_ref(),
            match picture.colour.light {
                Light::Ordinary => 0,
                Light::Pq => 1,
                Light::Hlg => 2,
            },
        );
        Some(program)
    }

    /// Draw the part `uv` of the picture (left, top, right, bottom, each 0 to 1) over `rect`
    /// (x, y, width, height in pixels) of a picture `view` pixels in size.
    pub fn draw(&self, gl: &Gl, view: (u32, u32), rect: [f32; 4], uv: [f32; 4]) {
        let g = gl.api();
        unsafe {
            let Some(program) = self.bind(gl, view, rect, uv) else { return };
            g.uniform_1_f32(program.at[9].as_ref(), 0.0);
            g.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            g.bind_vertex_array(None);
        }
    }

    /// Draw a view from inside a picture that is a sphere, or the front half of one (`half`),
    /// over `rect`. `uv` is the part of the picture that is the eye being shown.
    pub fn look(&self, gl: &Gl, view: (u32, u32), rect: [f32; 4], uv: [f32; 4], half: bool, gaze: Gaze) {
        let g = gl.api();
        unsafe {
            let Some(program) = self.bind(gl, view, rect, uv) else { return };
            let turn = Mat3::from_rotation_y(-gaze.yaw) * Mat3::from_rotation_x(gaze.pitch);
            let across = (gaze.field * 0.5).tan();
            g.uniform_1_f32(program.at[9].as_ref(), if half { std::f32::consts::PI } else { std::f32::consts::TAU });
            g.uniform_matrix_3_f32_slice(program.at[10].as_ref(), false, &turn.to_cols_array());
            g.uniform_2_f32(program.at[11].as_ref(), across, across * rect[3] / rect[2].max(1.0));
            g.draw_arrays(glow::TRIANGLE_STRIP, 0, 4);
            g.bind_vertex_array(None);
        }
    }
}

#[cfg(target_os = "macos")]
#[link(name = "CoreVideo", kind = "framework")]
extern "C" {
    fn CVPixelBufferGetIOSurface(buffer: *const std::ffi::c_void) -> *const std::ffi::c_void;
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rgb(colour: Colour, yuv: [f32; 3]) -> Vec3 {
        let (matrix, offset) = conversion(colour);
        matrix * (Vec3::from(yuv) - offset)
    }

    #[test]
    fn broadcast_black_and_white_come_out_black_and_white() {
        let colour = Colour { hd: true, wide: false, full_range: false, light: Light::Ordinary };
        let black = rgb(colour, [16.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0]);
        let white = rgb(colour, [235.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0]);
        assert!(black.abs().max_element() < 0.005, "{black}");
        assert!((white - Vec3::ONE).abs().max_element() < 0.005, "{white}");
    }

    #[test]
    fn a_full_range_picture_is_not_stretched_again() {
        let colour = Colour { hd: true, wide: false, full_range: true, light: Light::Ordinary };
        let grey = rgb(colour, [0.5, 128.0 / 255.0, 128.0 / 255.0]);
        assert!((grey - Vec3::splat(0.5)).abs().max_element() < 0.005, "{grey}");
    }

    /// Pure red in BT.709, as its three numbers, comes back as pure red -- and through
    /// BT.601's matrix it does not, which is the mistake this guards against.
    #[test]
    fn red_is_red_through_the_matrix_it_was_made_with() {
        let (kr, kb) = (0.2126f32, 0.0722f32);
        let y = kr;
        let (cb, cr) = ((0.0 - y) / (2.0 * (1.0 - kb)), (1.0 - y) / (2.0 * (1.0 - kr)));
        let yuv = [y, cb + 128.0 / 255.0, cr + 128.0 / 255.0];
        let right = rgb(Colour { hd: true, wide: false, full_range: true, light: Light::Ordinary }, yuv);
        assert!((right - Vec3::X).abs().max_element() < 0.01, "{right}");
        let wrong = rgb(Colour { hd: false, wide: false, full_range: true, light: Light::Ordinary }, yuv);
        assert!((wrong - Vec3::X).abs().max_element() > 0.03, "{wrong}");
    }
}
