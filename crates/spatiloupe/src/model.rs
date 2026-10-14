//! 3D objects: glTF and GLB, drawn with the materials they were made with.
//!
//! A small renderer of its own (docs/spatiand-apps.md): glTF's metallic-roughness materials --
//! base colour, metal and roughness, normal map, occlusion, emission, cut-out and see-through
//! surfaces -- lit by three lights and a soft sky, with a shadow under the object. Read on a
//! thread ([`Scene::read`]); put on the GPU and drawn here ([`Renderer`]).
//!
//! **Two eyes.** In the room each eye is drawn from where it is: two cameras 6.3 cm apart
//! looking at a window as wide as one is in the room, with the object's middle at the
//! window's own depth, so it stands half out of the window and half behind it, at its true
//! shape -- the frustum is cut off-axis for each eye, as a window is seen. Flat, one camera
//! between them.

use std::path::Path;

use glam::{Mat3, Mat4, Quat, Vec3, Vec4};
use spatiand_app::glow::{self, HasContext};
use spatiand_app::Gl;

/// How wide a window is in the room and how far away, near enough, in metres: what makes the
/// two eyes' cameras the wearer's own eyes, and the object its own size and shape.
const WINDOW_METRES: f32 = 1.3;
const VIEWER_METRES: f32 = 1.6;
const EYES_APART: f32 = 0.063;

/// One drawable piece, as read: everything in the GPU's own order.
pub struct Piece {
    /// Position, normal, texture coordinate, tangent (w its handedness), colour: 16 floats.
    vertices: Vec<f32>,
    indices: Vec<u32>,
    material: usize,
    world: Mat4,
    has_tangents: bool,
}

#[derive(Clone)]
pub struct Material {
    base: Vec4,
    base_texture: Option<usize>,
    metal: f32,
    rough: f32,
    metal_rough_texture: Option<usize>,
    normal_texture: Option<usize>,
    normal_scale: f32,
    occlusion_texture: Option<usize>,
    occlusion: f32,
    emissive: Vec3,
    emissive_texture: Option<usize>,
    /// 0 opaque, 1 cut out below `cutoff`, 2 see-through.
    alpha: u8,
    cutoff: f32,
    two_sided: bool,
    unlit: bool,
}

pub struct Image {
    width: u32,
    height: u32,
    rgba: Vec<u8>,
    /// Colour, decoded from sRGB when sampled; or data -- normals, roughness -- that is not.
    colour: bool,
    repeat: (u32, u32),
}

/// An object as read from its file, before any of it is on the GPU.
pub struct Scene {
    pieces: Vec<Piece>,
    materials: Vec<Material>,
    images: Vec<Image>,
    /// Where it is and how big, in its own units: the middle of its box and half the box's
    /// longest diagonal.
    centre: Vec3,
    radius: f32,
    floor: f32,
    pub triangles: usize,
    pub name: Option<String>,
}

fn wrap(mode: gltf::texture::WrappingMode) -> u32 {
    match mode {
        gltf::texture::WrappingMode::ClampToEdge => glow::CLAMP_TO_EDGE,
        gltf::texture::WrappingMode::MirroredRepeat => glow::MIRRORED_REPEAT,
        gltf::texture::WrappingMode::Repeat => glow::REPEAT,
    }
}

fn rgba_of(data: &gltf::image::Data) -> Option<Vec<u8>> {
    use gltf::image::Format;
    let pixels = (data.width * data.height) as usize;
    let p = &data.pixels;
    Some(match data.format {
        Format::R8G8B8A8 => p.clone(),
        Format::R8G8B8 => p.chunks_exact(3).flat_map(|c| [c[0], c[1], c[2], 255]).collect(),
        Format::R8G8 => p.chunks_exact(2).flat_map(|c| [c[0], c[1], 0, 255]).collect(),
        Format::R8 => p.iter().flat_map(|&v| [v, v, v, 255]).collect(),
        Format::R16G16B16A16 => p.chunks_exact(8).flat_map(|c| [c[1], c[3], c[5], c[7]]).collect(),
        Format::R16G16B16 => p.chunks_exact(6).flat_map(|c| [c[1], c[3], c[5], 255]).collect(),
        Format::R16G16 => p.chunks_exact(4).flat_map(|c| [c[1], c[3], 0, 255]).collect(),
        Format::R16 => p.chunks_exact(2).flat_map(|c| [c[1], c[1], c[1], 255]).collect(),
        _ => return None,
    })
    .filter(|rgba: &Vec<u8>| rgba.len() == pixels * 4)
}

impl Scene {
    pub fn read(path: &Path) -> Result<Scene, String> {
        let (document, buffers, images) = gltf::import(path).map_err(|e| match e {
            gltf::Error::UnsupportedScheme => "it points at files on the network".to_string(),
            gltf::Error::Validation(_) => "it is not a well-formed glTF file".to_string(),
            e => e.to_string(),
        })?;
        for required in document.extensions_required() {
            if !["KHR_materials_emissive_strength", "KHR_materials_unlit", "KHR_texture_transform", "KHR_mesh_quantization"].contains(&required) {
                return Err(format!("it needs {required}, which SpatiLoupe does not read"));
            }
        }
        // Which images are colour, which are data: decided by what materials use them for.
        let mut colour = vec![false; images.len()];
        let mut samplers: Vec<(u32, u32)> = vec![(glow::REPEAT, glow::REPEAT); images.len()];
        let mut materials = Vec::new();
        let source = |info: Option<gltf::texture::Texture>, colour_of: bool, colour: &mut Vec<bool>, samplers: &mut Vec<(u32, u32)>| -> Option<usize> {
            let texture = info?;
            let index = texture.source().index();
            if index < colour.len() {
                colour[index] |= colour_of;
                samplers[index] = (wrap(texture.sampler().wrap_s()), wrap(texture.sampler().wrap_t()));
                Some(index)
            } else {
                None
            }
        };
        for material in document.materials() {
            let pbr = material.pbr_metallic_roughness();
            let normal = material.normal_texture();
            let occlusion = material.occlusion_texture();
            let strength = material.emissive_strength().unwrap_or(1.0);
            materials.push(Material {
                base: Vec4::from(pbr.base_color_factor()),
                base_texture: source(pbr.base_color_texture().map(|t| t.texture()), true, &mut colour, &mut samplers),
                metal: pbr.metallic_factor(),
                rough: pbr.roughness_factor(),
                metal_rough_texture: source(pbr.metallic_roughness_texture().map(|t| t.texture()), false, &mut colour, &mut samplers),
                normal_scale: normal.as_ref().map_or(1.0, |n| n.scale()),
                normal_texture: source(normal.map(|t| t.texture()), false, &mut colour, &mut samplers),
                occlusion: occlusion.as_ref().map_or(1.0, |o| o.strength()),
                occlusion_texture: source(occlusion.map(|t| t.texture()), false, &mut colour, &mut samplers),
                emissive: Vec3::from(material.emissive_factor()) * strength,
                emissive_texture: source(material.emissive_texture().map(|t| t.texture()), true, &mut colour, &mut samplers),
                alpha: match material.alpha_mode() {
                    gltf::material::AlphaMode::Opaque => 0,
                    gltf::material::AlphaMode::Mask => 1,
                    gltf::material::AlphaMode::Blend => 2,
                },
                cutoff: material.alpha_cutoff().unwrap_or(0.5),
                two_sided: material.double_sided(),
                unlit: material.unlit(),
            });
        }
        // The default material, for a piece that names none: grey, a little rough.
        let plain = materials.len();
        materials.push(Material {
            base: Vec4::new(0.8, 0.8, 0.8, 1.0),
            base_texture: None,
            metal: 0.0,
            rough: 0.6,
            metal_rough_texture: None,
            normal_texture: None,
            normal_scale: 1.0,
            occlusion_texture: None,
            occlusion: 1.0,
            emissive: Vec3::ZERO,
            emissive_texture: None,
            alpha: 0,
            cutoff: 0.5,
            two_sided: false,
            unlit: false,
        });
        let images: Vec<Image> = images
            .iter()
            .enumerate()
            .map(|(index, data)| {
                let rgba = rgba_of(data).unwrap_or_else(|| vec![255; (data.width * data.height * 4) as usize]);
                Image { width: data.width, height: data.height, rgba, colour: colour[index], repeat: samplers[index] }
            })
            .collect();

        let mut pieces = Vec::new();
        let (mut low, mut high) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        let scene = document.default_scene().or_else(|| document.scenes().next()).ok_or("there is nothing in it")?;
        let mut stack: Vec<(gltf::Node, Mat4)> = scene.nodes().map(|n| (n, Mat4::IDENTITY)).collect();
        while let Some((node, parent)) = stack.pop() {
            let world = parent * Mat4::from_cols_array_2d(&node.transform().matrix());
            for child in node.children() {
                stack.push((child, world));
            }
            let Some(mesh) = node.mesh() else { continue };
            for primitive in mesh.primitives() {
                if primitive.mode() != gltf::mesh::Mode::Triangles {
                    continue;
                }
                let reader = primitive.reader(|buffer| buffers.get(buffer.index()).map(|b| &b.0[..]));
                let Some(positions) = reader.read_positions() else { continue };
                let positions: Vec<[f32; 3]> = positions.collect();
                let count = positions.len();
                let indices: Vec<u32> = match reader.read_indices() {
                    Some(indices) => indices.into_u32().filter(|&i| (i as usize) < count).collect(),
                    None => (0..count as u32).collect(),
                };
                let indices: Vec<u32> = indices.chunks_exact(3).flatten().copied().collect();
                let normals: Vec<[f32; 3]> = match reader.read_normals() {
                    Some(normals) => normals.collect(),
                    None => smooth_normals(&positions, &indices),
                };
                let uvs: Vec<[f32; 2]> = reader.read_tex_coords(0).map(|t| t.into_f32().collect()).unwrap_or_else(|| vec![[0.0, 0.0]; count]);
                let tangents: Option<Vec<[f32; 4]>> = reader.read_tangents().map(|t| t.collect());
                let colours: Vec<[f32; 4]> = reader.read_colors(0).map(|c| c.into_rgba_f32().collect()).unwrap_or_else(|| vec![[1.0; 4]; count]);
                let mut vertices = Vec::with_capacity(count * 16);
                for i in 0..count {
                    let p = positions[i];
                    let corner = world.transform_point3(Vec3::from(p));
                    low = low.min(corner);
                    high = high.max(corner);
                    vertices.extend_from_slice(&p);
                    vertices.extend_from_slice(normals.get(i).unwrap_or(&[0.0, 1.0, 0.0]));
                    vertices.extend_from_slice(uvs.get(i).unwrap_or(&[0.0, 0.0]));
                    vertices.extend_from_slice(tangents.as_ref().and_then(|t| t.get(i)).unwrap_or(&[1.0, 0.0, 0.0, 1.0]));
                    vertices.extend_from_slice(colours.get(i).unwrap_or(&[1.0; 4]));
                }
                let material = primitive.material().index().unwrap_or(plain);
                pieces.push(Piece { vertices, indices, material, world, has_tangents: tangents.is_some() });
            }
        }
        if pieces.is_empty() {
            return Err("there is nothing in it to draw".into());
        }
        let triangles = pieces.iter().map(|p| p.indices.len() / 3).sum();
        let centre = (low + high) * 0.5;
        let radius = ((high - low).length() * 0.5).max(1e-6);
        let name = scene.name().map(str::to_string).filter(|n| !n.is_empty() && n != "Scene");
        Ok(Scene { pieces, materials, images, centre, radius, floor: low.y, triangles, name })
    }
}

/// A normal for each corner, the average of its triangles', for a mesh that came without.
fn smooth_normals(positions: &[[f32; 3]], indices: &[u32]) -> Vec<[f32; 3]> {
    let mut sum = vec![Vec3::ZERO; positions.len()];
    for t in indices.chunks_exact(3) {
        let [a, b, c] = [t[0], t[1], t[2]].map(|i| Vec3::from(positions[i as usize]));
        let n = (b - a).cross(c - a);
        for &i in t {
            sum[i as usize] += n;
        }
    }
    sum.into_iter().map(|n| n.try_normalize().unwrap_or(Vec3::Y).to_array()).collect()
}

const VERTEX: &str = r#"#version 300 es
precision highp float;
layout(location = 0) in vec3 a_position;
layout(location = 1) in vec3 a_normal;
layout(location = 2) in vec2 a_uv;
layout(location = 3) in vec4 a_tangent;
layout(location = 4) in vec4 a_colour;
uniform mat4 u_model;
uniform mat4 u_view_projection;
uniform mat3 u_normal_matrix;
out vec3 v_world;
out vec3 v_normal;
out vec2 v_uv;
out vec4 v_tangent;
out vec4 v_colour;
void main() {
    vec4 world = u_model * vec4(a_position, 1.0);
    v_world = world.xyz;
    v_normal = u_normal_matrix * a_normal;
    v_tangent = vec4(mat3(u_model) * a_tangent.xyz, a_tangent.w);
    v_uv = a_uv;
    v_colour = a_colour;
    gl_Position = u_view_projection * world;
}
"#;

const FRAGMENT: &str = r#"#version 300 es
precision highp float;
in vec3 v_world;
in vec3 v_normal;
in vec2 v_uv;
in vec4 v_tangent;
in vec4 v_colour;
uniform vec3 u_eye;
uniform vec4 u_base;
uniform vec3 u_emissive;
uniform vec4 u_surface; // metal, rough, normal scale, occlusion strength
uniform ivec4 u_maps;    // base, metal-rough, normal, occlusion present
uniform ivec4 u_more;    // emissive present, alpha mode, unlit, tangents given
uniform float u_cutoff;
uniform sampler2D u_base_map;
uniform sampler2D u_metal_rough_map;
uniform sampler2D u_normal_map;
uniform sampler2D u_occlusion_map;
uniform sampler2D u_emissive_map;
out vec4 f_colour;

const float PI = 3.14159265;

// The sky the object is lit by: a studio's -- a bright overhead, a soft horizon, a dark
// floor -- in the room's own blues, so that what is shiny shows something worth seeing in it.
vec3 sky(vec3 d, float blur) {
    float up = d.y;
    vec3 zenith = vec3(0.80, 0.86, 1.00) * 1.4;
    vec3 horizon = vec3(0.42, 0.48, 0.62);
    vec3 floor_ = vec3(0.07, 0.075, 0.09);
    vec3 c = up > 0.0 ? mix(horizon, zenith, pow(up, mix(0.5, 1.0, blur))) : mix(horizon, floor_, pow(-up, mix(0.35, 1.0, blur)));
    // Two soft boxes, as a photographer's studio has, which give a shiny thing its highlights.
    float box1 = smoothstep(0.80 - blur * 0.5, 0.97, dot(d, normalize(vec3(-0.6, 0.7, 0.5))));
    float box2 = smoothstep(0.85 - blur * 0.5, 0.98, dot(d, normalize(vec3(0.7, 0.35, -0.6))));
    c += (box1 * 3.0 + box2 * 1.6) * (1.0 - blur * 0.85);
    return c;
}

vec3 env_brdf(vec3 f0, float rough, float nv) {
    // Karis's fit of the split-sum lookup, for want of a table.
    const vec4 c0 = vec4(-1.0, -0.0275, -0.572, 0.022);
    const vec4 c1 = vec4(1.0, 0.0425, 1.04, -0.04);
    vec4 r = rough * c0 + c1;
    float a004 = min(r.x * r.x, exp2(-9.28 * nv)) * r.x + r.y;
    vec2 ab = vec2(-1.04, 1.04) * a004 + r.zw;
    return f0 * ab.x + ab.y;
}

vec3 light(vec3 n, vec3 v, vec3 l, vec3 colour, vec3 albedo, vec3 f0, float metal, float rough) {
    vec3 h = normalize(v + l);
    float nl = max(dot(n, l), 0.0);
    float nv = max(dot(n, v), 1e-4);
    float nh = max(dot(n, h), 0.0);
    float a = rough * rough;
    float a2 = a * a;
    float d = a2 / (PI * pow(nh * nh * (a2 - 1.0) + 1.0, 2.0));
    float k = (rough + 1.0) * (rough + 1.0) / 8.0;
    float g = (nv / (nv * (1.0 - k) + k)) * (nl / (nl * (1.0 - k) + k));
    vec3 f = f0 + (1.0 - f0) * pow(1.0 - max(dot(h, v), 0.0), 5.0);
    vec3 specular = d * g * f / max(4.0 * nv * nl, 1e-4);
    vec3 diffuse = (1.0 - f) * (1.0 - metal) * albedo / PI;
    return (diffuse + specular) * colour * nl;
}

void main() {
    vec4 base = u_base * v_colour;
    if (u_maps.x == 1) base *= texture(u_base_map, v_uv);
    if (u_more.y == 1 && base.a < u_cutoff) discard;
    if (u_more.z == 1) {
        f_colour = vec4(pow(base.rgb, vec3(1.0 / 2.2)), u_more.y == 2 ? base.a : 1.0);
        return;
    }
    float metal = u_surface.x;
    float rough = u_surface.y;
    if (u_maps.y == 1) {
        vec4 mr = texture(u_metal_rough_map, v_uv);
        rough *= mr.g;
        metal *= mr.b;
    }
    rough = clamp(rough, 0.04, 1.0);
    vec3 n = normalize(v_normal);
    if (!gl_FrontFacing) n = -n;
    if (u_maps.z == 1) {
        vec3 t;
        vec3 b;
        if (u_more.w == 1) {
            t = normalize(v_tangent.xyz - n * dot(n, v_tangent.xyz));
            b = cross(n, t) * v_tangent.w;
        } else {
            // A frame from how the texture runs across the surface, for want of tangents.
            vec3 dp1 = dFdx(v_world);
            vec3 dp2 = dFdy(v_world);
            vec2 duv1 = dFdx(v_uv);
            vec2 duv2 = dFdy(v_uv);
            vec3 dp2perp = cross(dp2, n);
            vec3 dp1perp = cross(n, dp1);
            t = dp2perp * duv1.x + dp1perp * duv2.x;
            b = dp2perp * duv1.y + dp1perp * duv2.y;
            float scale = inversesqrt(max(dot(t, t), dot(b, b)) + 1e-12);
            t *= scale;
            b *= scale;
        }
        vec3 m = texture(u_normal_map, v_uv).xyz * 2.0 - 1.0;
        m.xy *= u_surface.z;
        n = normalize(mat3(t, b, n) * m);
    }
    vec3 v = normalize(u_eye - v_world);
    vec3 albedo = base.rgb;
    vec3 f0 = mix(vec3(0.04), albedo, metal);
    vec3 c = vec3(0.0);
    c += light(n, v, normalize(vec3(-0.5, 0.8, 0.6)), vec3(1.0, 0.96, 0.9) * 2.6, albedo, f0, metal, rough);
    c += light(n, v, normalize(vec3(0.8, 0.2, 0.4)), vec3(0.75, 0.85, 1.0) * 0.7, albedo, f0, metal, rough);
    c += light(n, v, normalize(vec3(0.1, 0.4, -0.9)), vec3(0.6, 0.75, 1.0) * 1.1, albedo, f0, metal, rough);
    float nv = max(dot(n, v), 1e-4);
    vec3 r = reflect(-v, n);
    vec3 diffuse_sky = sky(n, 1.0) * 0.5;
    vec3 brdf = env_brdf(f0, rough, nv);
    vec3 ambient = diffuse_sky * albedo * (1.0 - metal) + sky(r, rough) * brdf;
    if (u_maps.w == 1) ambient *= mix(1.0, texture(u_occlusion_map, v_uv).r, u_surface.w);
    c += ambient * 0.9;
    vec3 emissive = u_emissive;
    if (u_more.x == 1) emissive *= texture(u_emissive_map, v_uv).rgb;
    c += emissive;
    // Filmic, then into the window's sRGB.
    c = clamp((c * (2.51 * c + 0.03)) / (c * (2.43 * c + 0.59) + 0.14), 0.0, 1.0);
    f_colour = vec4(pow(c, vec3(1.0 / 2.2)), u_more.y == 2 ? base.a : 1.0);
}
"#;

/// The soft dark patch under the object, on the floor it would stand on.
const SHADOW_FRAGMENT: &str = r#"#version 300 es
precision highp float;
in vec2 v_uv;
out vec4 f_colour;
uniform float u_strength;
void main() {
    float d = length(v_uv * 2.0 - 1.0);
    float a = (1.0 - smoothstep(0.0, 1.0, d)) * u_strength;
    f_colour = vec4(0.0, 0.0, 0.0, a * a);
}
"#;

const SHADOW_VERTEX: &str = r#"#version 300 es
precision highp float;
layout(location = 0) in vec3 a_position;
layout(location = 2) in vec2 a_uv;
uniform mat4 u_model;
uniform mat4 u_view_projection;
out vec2 v_uv;
void main() {
    v_uv = a_uv;
    gl_Position = u_view_projection * u_model * vec4(a_position, 1.0);
}
"#;

struct GpuPiece {
    vao: glow::VertexArray,
    buffers: [glow::Buffer; 2],
    count: i32,
    material: usize,
    world: Mat4,
    has_tangents: bool,
    /// Where its middle is, for drawing see-through pieces back to front.
    middle: Vec3,
}

/// Where the object is looked at from: round it, above it, how far.
#[derive(Debug, Clone, Copy)]
pub struct Orbit {
    pub yaw: f32,
    pub pitch: f32,
    /// How much nearer than at the start: 1 is the whole object in view.
    pub zoom: f32,
    /// Moved off its middle, in the object's own radii, across and up.
    pub pan: (f32, f32),
}

impl Default for Orbit {
    fn default() -> Orbit {
        Orbit { yaw: -0.5, pitch: 0.28, zoom: 1.0, pan: (0.0, 0.0) }
    }
}

/// Which eye, for a picture with two.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Eye {
    Left,
    Right,
    Both,
}

/// What draws into, with a depth buffer and smoothed edges: drawn into several samples a
/// pixel, then made one, into a texture the window's picture is made from.
struct Canvas {
    size: (u32, u32),
    samples: glow::Framebuffer,
    colour_samples: glow::Renderbuffer,
    depth: glow::Renderbuffer,
    resolved: glow::Framebuffer,
    pub texture: glow::Texture,
}

pub struct Renderer {
    program: glow::Program,
    shadow_program: glow::Program,
    pieces: Vec<GpuPiece>,
    shadow: GpuPiece,
    textures: Vec<glow::Texture>,
    white: glow::Texture,
    materials: Vec<Material>,
    centre: Vec3,
    radius: f32,
    floor: f32,
    canvases: [Option<Canvas>; 2],
}

unsafe fn program(g: &glow::Context, vertex: &str, fragment: &str) -> Result<glow::Program, String> {
    let program = g.create_program()?;
    for (kind, source) in [(glow::VERTEX_SHADER, vertex), (glow::FRAGMENT_SHADER, fragment)] {
        let shader = g.create_shader(kind)?;
        g.shader_source(shader, source);
        g.compile_shader(shader);
        if !g.get_shader_compile_status(shader) {
            return Err(format!("a shader would not compile: {}", g.get_shader_info_log(shader)));
        }
        g.attach_shader(program, shader);
    }
    g.link_program(program);
    if !g.get_program_link_status(program) {
        return Err(format!("the shaders would not link: {}", g.get_program_info_log(program)));
    }
    Ok(program)
}

unsafe fn upload_piece(g: &glow::Context, vertices: &[f32], indices: &[u32]) -> Result<(glow::VertexArray, [glow::Buffer; 2]), String> {
    let vao = g.create_vertex_array()?;
    g.bind_vertex_array(Some(vao));
    let vbo = g.create_buffer()?;
    g.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
    g.buffer_data_u8_slice(glow::ARRAY_BUFFER, as_bytes(vertices), glow::STATIC_DRAW);
    let ibo = g.create_buffer()?;
    g.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ibo));
    g.buffer_data_u8_slice(glow::ELEMENT_ARRAY_BUFFER, as_bytes(indices), glow::STATIC_DRAW);
    let stride = 16 * 4;
    for (location, size, offset) in [(0u32, 3i32, 0i32), (1, 3, 3), (2, 2, 6), (3, 4, 8), (4, 4, 12)] {
        g.enable_vertex_attrib_array(location);
        g.vertex_attrib_pointer_f32(location, size, glow::FLOAT, false, stride, offset * 4);
    }
    g.bind_vertex_array(None);
    Ok((vao, [vbo, ibo]))
}

fn as_bytes<T: Copy>(data: &[T]) -> &[u8] {
    unsafe { std::slice::from_raw_parts(data.as_ptr() as *const u8, std::mem::size_of_val(data)) }
}

impl Renderer {
    pub fn new(gl: &Gl, scene: Scene) -> Result<Renderer, String> {
        let g = gl.api();
        unsafe {
            let program = program(g, VERTEX, FRAGMENT)?;
            let shadow_program = program_shadow(g)?;
            let mut textures = Vec::new();
            for image in &scene.images {
                let texture = g.create_texture()?;
                g.bind_texture(glow::TEXTURE_2D, Some(texture));
                let internal = if image.colour { glow::SRGB8_ALPHA8 } else { glow::RGBA8 };
                g.tex_image_2d(glow::TEXTURE_2D, 0, internal as i32, image.width as i32, image.height as i32, 0, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(Some(&image.rgba)));
                g.generate_mipmap(glow::TEXTURE_2D);
                g.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR_MIPMAP_LINEAR as i32);
                g.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
                g.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, image.repeat.0 as i32);
                g.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, image.repeat.1 as i32);
                textures.push(texture);
            }
            let white = g.create_texture()?;
            g.bind_texture(glow::TEXTURE_2D, Some(white));
            g.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, 1, 1, 0, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(Some(&[255, 255, 255, 255])));
            g.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
            let mut pieces = Vec::new();
            for piece in &scene.pieces {
                let (vao, buffers) = upload_piece(g, &piece.vertices, &piece.indices)?;
                let n = piece.vertices.len() / 16;
                let step = (n / 64).max(1);
                let (mut middle, mut counted) = (Vec3::ZERO, 0usize);
                for i in (0..n).step_by(step) {
                    middle += Vec3::new(piece.vertices[i * 16], piece.vertices[i * 16 + 1], piece.vertices[i * 16 + 2]);
                    counted += 1;
                }
                let middle = piece.world.transform_point3(middle / counted.max(1) as f32);
                pieces.push(GpuPiece { vao, buffers, count: piece.indices.len() as i32, material: piece.material, world: piece.world, has_tangents: piece.has_tangents, middle });
            }
            // A square on the floor, a little wider than the object.
            let mut square = Vec::new();
            for (x, z, u, v) in [(-1.0f32, -1.0f32, 0.0f32, 0.0f32), (1.0, -1.0, 1.0, 0.0), (1.0, 1.0, 1.0, 1.0), (-1.0, 1.0, 0.0, 1.0)] {
                square.extend_from_slice(&[x, 0.0, z, 0.0, 1.0, 0.0, u, v, 1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 1.0, 0.0][..16]);
            }
            let (vao, buffers) = upload_piece(g, &square, &[0, 1, 2, 0, 2, 3])?;
            let shadow = GpuPiece { vao, buffers, count: 6, material: 0, world: Mat4::IDENTITY, has_tangents: false, middle: Vec3::ZERO };
            Ok(Renderer {
                program,
                shadow_program,
                pieces,
                shadow,
                textures,
                white,
                materials: scene.materials,
                centre: scene.centre,
                radius: scene.radius,
                floor: scene.floor,
                canvases: [None, None],
            })
        }
    }

    pub fn forget(self, gl: &Gl) {
        let g = gl.api();
        unsafe {
            for piece in self.pieces.iter().chain(std::iter::once(&self.shadow)) {
                g.delete_vertex_array(piece.vao);
                for b in piece.buffers {
                    g.delete_buffer(b);
                }
            }
            for t in self.textures {
                g.delete_texture(t);
            }
            g.delete_texture(self.white);
            for canvas in self.canvases.into_iter().flatten() {
                canvas.forget(g);
            }
            g.delete_program(self.program);
            g.delete_program(self.shadow_program);
        }
    }

    /// Draw the object as `eye` sees it into a texture `size` pixels, over what `ground` draws
    /// first, and say which texture. The texture's row 0 is its foot, as GL draws.
    pub fn draw(&mut self, gl: &Gl, size: (u32, u32), orbit: &Orbit, eye: Eye, ground: impl FnOnce(&Gl)) -> Result<glow::Texture, String> {
        let g = gl.api();
        let slot = if eye == Eye::Right { 1 } else { 0 };
        unsafe {
            let previous = g.get_parameter_framebuffer(glow::FRAMEBUFFER_BINDING);
            if self.canvases[slot].as_ref().is_some_and(|c| c.size != size) {
                self.canvases[slot].take().unwrap().forget(g);
            }
            if self.canvases[slot].is_none() {
                self.canvases[slot] = Some(Canvas::new(g, size)?);
            }
            let canvas = self.canvases[slot].as_ref().unwrap();
            g.bind_framebuffer(glow::FRAMEBUFFER, Some(canvas.samples));
            g.viewport(0, 0, size.0 as i32, size.1 as i32);
            g.disable(glow::SCISSOR_TEST);
            g.clear_color(0.0, 0.0, 0.0, 1.0);
            g.clear_depth_f32(1.0);
            g.clear(glow::COLOR_BUFFER_BIT | glow::DEPTH_BUFFER_BIT);
            ground(gl);
            self.scene(g, size, orbit, eye);
            // Several samples to one, into the texture.
            g.bind_framebuffer(glow::READ_FRAMEBUFFER, Some(canvas.samples));
            g.bind_framebuffer(glow::DRAW_FRAMEBUFFER, Some(canvas.resolved));
            g.blit_framebuffer(0, 0, size.0 as i32, size.1 as i32, 0, 0, size.0 as i32, size.1 as i32, glow::COLOR_BUFFER_BIT, glow::NEAREST);
            g.bind_framebuffer(glow::FRAMEBUFFER, previous);
            Ok(canvas.texture)
        }
    }

    unsafe fn scene(&self, g: &glow::Context, size: (u32, u32), orbit: &Orbit, eye: Eye) {
        // The object made a radius of one, its middle at the origin.
        let fit = Mat4::from_scale(Vec3::splat(1.0 / self.radius)) * Mat4::from_translation(-self.centre);
        let aspect = size.0 as f32 / size.1.max(1) as f32;
        // As wide across as a window is seen from where the wearer stands: what makes two
        // cameras the two eyes.
        let across = 2.0 * (WINDOW_METRES * 0.5 / VIEWER_METRES).atan();
        let half_across = (across * 0.5).tan();
        // Far enough that a radius of one fills seven tenths of the narrower side.
        let half_narrow = if aspect >= 1.0 { half_across / aspect } else { half_across };
        let distance = 1.0 / (half_narrow * 0.7) / orbit.zoom.max(0.05);
        // The window is `distance` away; the eyes are as far apart, measured against it, as
        // the wearer's are against the window.
        let apart = EYES_APART * (2.0 * distance * half_across) / WINDOW_METRES;
        let offset = match eye {
            Eye::Left => -apart * 0.5,
            Eye::Right => apart * 0.5,
            Eye::Both => 0.0,
        };
        let turn = Quat::from_rotation_y(orbit.yaw) * Quat::from_rotation_x(-orbit.pitch);
        let back = turn * Vec3::Z;
        let right = turn * Vec3::X;
        let up = turn * Vec3::Y;
        let target = right * orbit.pan.0 + up * orbit.pan.1;
        let middle_eye = target + back * distance;
        let at = middle_eye + right * offset;
        let view = Mat4::look_to_rh(at, -back, up);
        let near = (distance - 2.0).max(distance * 0.05);
        let far = distance + 4.0;
        // Off-axis: the window's edges stay where they are and each eye looks through it.
        let half_w = distance * half_across;
        let half_h = half_w / aspect;
        let scale = near / distance;
        let projection = frustum((-half_w - offset) * scale, (half_w - offset) * scale, -half_h * scale, half_h * scale, near, far);
        let view_projection = projection * view;

        g.enable(glow::DEPTH_TEST);
        g.depth_func(glow::LEQUAL);
        g.depth_mask(true);
        g.disable(glow::BLEND);
        g.use_program(Some(self.program));
        let uniform = |name: &str| g.get_uniform_location(self.program, name);
        g.uniform_matrix_4_f32_slice(uniform("u_view_projection").as_ref(), false, &view_projection.to_cols_array());
        g.uniform_3_f32(uniform("u_eye").as_ref(), at.x, at.y, at.z);
        for (unit, name) in ["u_base_map", "u_metal_rough_map", "u_normal_map", "u_occlusion_map", "u_emissive_map"].iter().enumerate() {
            g.uniform_1_i32(uniform(name).as_ref(), unit as i32);
        }
        // Opaque first, then see-through from the back forwards.
        let mut order: Vec<&GpuPiece> = self.pieces.iter().filter(|p| self.materials[p.material].alpha != 2).collect();
        let mut clear: Vec<&GpuPiece> = self.pieces.iter().filter(|p| self.materials[p.material].alpha == 2).collect();
        clear.sort_by(|a, b| {
            let da = (fit.transform_point3(a.middle) - at).length_squared();
            let db = (fit.transform_point3(b.middle) - at).length_squared();
            db.partial_cmp(&da).unwrap_or(std::cmp::Ordering::Equal)
        });
        let opaque_count = order.len();
        order.extend(clear);
        for (index, piece) in order.into_iter().enumerate() {
            if index == opaque_count {
                g.enable(glow::BLEND);
                g.blend_func_separate(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA, glow::ONE, glow::ONE_MINUS_SRC_ALPHA);
                g.depth_mask(false);
            }
            let m = &self.materials[piece.material];
            let model = fit * piece.world;
            let normal = Mat3::from_mat4(model).inverse().transpose();
            g.uniform_matrix_4_f32_slice(uniform("u_model").as_ref(), false, &model.to_cols_array());
            g.uniform_matrix_3_f32_slice(uniform("u_normal_matrix").as_ref(), false, &normal.to_cols_array());
            g.uniform_4_f32(uniform("u_base").as_ref(), m.base.x, m.base.y, m.base.z, m.base.w);
            g.uniform_3_f32(uniform("u_emissive").as_ref(), m.emissive.x, m.emissive.y, m.emissive.z);
            g.uniform_4_f32(uniform("u_surface").as_ref(), m.metal, m.rough, m.normal_scale, m.occlusion);
            let maps = [m.base_texture, m.metal_rough_texture, m.normal_texture, m.occlusion_texture, m.emissive_texture];
            for (unit, map) in maps.iter().enumerate() {
                g.active_texture(glow::TEXTURE0 + unit as u32);
                g.bind_texture(glow::TEXTURE_2D, Some(map.and_then(|i| self.textures.get(i).copied()).unwrap_or(self.white)));
            }
            let has = |map: Option<usize>| map.is_some_and(|i| i < self.textures.len()) as i32;
            g.uniform_4_i32(uniform("u_maps").as_ref(), has(maps[0]), has(maps[1]), has(maps[2]), has(maps[3]));
            g.uniform_4_i32(uniform("u_more").as_ref(), has(maps[4]), m.alpha as i32, m.unlit as i32, piece.has_tangents as i32);
            g.uniform_1_f32(uniform("u_cutoff").as_ref(), m.cutoff);
            if m.two_sided {
                g.disable(glow::CULL_FACE);
            } else {
                g.enable(glow::CULL_FACE);
                g.cull_face(glow::BACK);
                // A mirrored node turns its triangles inside out.
                g.front_face(if model.determinant() < 0.0 { glow::CW } else { glow::CCW });
            }
            g.bind_vertex_array(Some(piece.vao));
            g.draw_elements(glow::TRIANGLES, piece.count, glow::UNSIGNED_INT, 0);
        }
        // The shadow: under the object, on the floor it stands on, drawn last so that it lies
        // under what is in front of it and over nothing.
        g.disable(glow::CULL_FACE);
        g.enable(glow::BLEND);
        g.blend_func(glow::SRC_ALPHA, glow::ONE_MINUS_SRC_ALPHA);
        g.depth_mask(false);
        g.use_program(Some(self.shadow_program));
        let floor = (self.floor - self.centre.y) / self.radius - 0.002;
        let shadow = Mat4::from_translation(Vec3::new(0.0, floor, 0.0)) * Mat4::from_scale(Vec3::new(0.85, 1.0, 0.85));
        let su = |name: &str| g.get_uniform_location(self.shadow_program, name);
        g.uniform_matrix_4_f32_slice(su("u_model").as_ref(), false, &shadow.to_cols_array());
        g.uniform_matrix_4_f32_slice(su("u_view_projection").as_ref(), false, &view_projection.to_cols_array());
        g.uniform_1_f32(su("u_strength").as_ref(), 0.75);
        g.bind_vertex_array(Some(self.shadow.vao));
        g.draw_elements(glow::TRIANGLES, 6, glow::UNSIGNED_INT, 0);
        g.bind_vertex_array(None);
        g.depth_mask(true);
        g.disable(glow::DEPTH_TEST);
        g.disable(glow::CULL_FACE);
        g.front_face(glow::CCW);
        g.active_texture(glow::TEXTURE0);
    }
}

unsafe fn program_shadow(g: &glow::Context) -> Result<glow::Program, String> {
    program(g, SHADOW_VERTEX, SHADOW_FRAGMENT)
}

fn frustum(left: f32, right: f32, bottom: f32, top: f32, near: f32, far: f32) -> Mat4 {
    let (rl, tb, fnr) = (right - left, top - bottom, far - near);
    Mat4::from_cols(
        Vec4::new(2.0 * near / rl, 0.0, 0.0, 0.0),
        Vec4::new(0.0, 2.0 * near / tb, 0.0, 0.0),
        Vec4::new((right + left) / rl, (top + bottom) / tb, -(far + near) / fnr, -1.0),
        Vec4::new(0.0, 0.0, -2.0 * far * near / fnr, 0.0),
    )
}

impl Canvas {
    unsafe fn new(g: &glow::Context, size: (u32, u32)) -> Result<Canvas, String> {
        let (w, h) = (size.0 as i32, size.1 as i32);
        let max_samples = g.get_parameter_i32(glow::MAX_SAMPLES).max(1);
        let samples_n = 4.min(max_samples);
        let samples = g.create_framebuffer()?;
        g.bind_framebuffer(glow::FRAMEBUFFER, Some(samples));
        let colour_samples = g.create_renderbuffer()?;
        g.bind_renderbuffer(glow::RENDERBUFFER, Some(colour_samples));
        g.renderbuffer_storage_multisample(glow::RENDERBUFFER, samples_n, glow::RGBA8, w, h);
        g.framebuffer_renderbuffer(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::RENDERBUFFER, Some(colour_samples));
        let depth = g.create_renderbuffer()?;
        g.bind_renderbuffer(glow::RENDERBUFFER, Some(depth));
        g.renderbuffer_storage_multisample(glow::RENDERBUFFER, samples_n, glow::DEPTH_COMPONENT24, w, h);
        g.framebuffer_renderbuffer(glow::FRAMEBUFFER, glow::DEPTH_ATTACHMENT, glow::RENDERBUFFER, Some(depth));
        if g.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
            return Err("the GPU would not give a picture with depth to draw into".into());
        }
        let texture = g.create_texture()?;
        g.bind_texture(glow::TEXTURE_2D, Some(texture));
        g.tex_image_2d(glow::TEXTURE_2D, 0, glow::RGBA8 as i32, w, h, 0, glow::RGBA, glow::UNSIGNED_BYTE, glow::PixelUnpackData::Slice(None));
        g.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MIN_FILTER, glow::LINEAR as i32);
        g.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_MAG_FILTER, glow::LINEAR as i32);
        g.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::CLAMP_TO_EDGE as i32);
        g.tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_T, glow::CLAMP_TO_EDGE as i32);
        let resolved = g.create_framebuffer()?;
        g.bind_framebuffer(glow::FRAMEBUFFER, Some(resolved));
        g.framebuffer_texture_2d(glow::FRAMEBUFFER, glow::COLOR_ATTACHMENT0, glow::TEXTURE_2D, Some(texture), 0);
        g.bind_renderbuffer(glow::RENDERBUFFER, None);
        Ok(Canvas { size, samples, colour_samples, depth, resolved, texture })
    }

    unsafe fn forget(self, g: &glow::Context) {
        g.delete_framebuffer(self.samples);
        g.delete_framebuffer(self.resolved);
        g.delete_renderbuffer(self.colour_samples);
        g.delete_renderbuffer(self.depth);
        g.delete_texture(self.texture);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mesh_without_normals_is_given_them_facing_out() {
        // One triangle in the floor's plane, wound anticlockwise from above.
        let normals = smooth_normals(&[[0.0, 0.0, 0.0], [0.0, 0.0, 1.0], [1.0, 0.0, 0.0]], &[0, 1, 2]);
        for n in normals {
            assert!((Vec3::from(n) - Vec3::Y).length() < 1e-5, "{n:?}");
        }
    }

    #[test]
    fn the_two_eyes_see_the_window_edges_in_the_same_place() {
        // At the window's own depth, a point is where it is for both eyes: zero parallax.
        let distance = 3.0;
        let half_w = 1.0;
        let offset = 0.05;
        for (eye, sign) in [(-offset, -1.0f32), (offset, 1.0)] {
            let _ = sign;
            let near = 1.0;
            let scale = near / distance;
            let p = frustum((-half_w - eye) * scale, (half_w - eye) * scale, -0.5 * scale, 0.5 * scale, near, 10.0);
            let view = Mat4::look_to_rh(Vec3::new(eye, 0.0, distance), -Vec3::Z, Vec3::Y);
            let edge = p * view * Vec4::new(half_w, 0.0, 0.0, 1.0);
            assert!((edge.x / edge.w - 1.0).abs() < 1e-5, "{}", edge.x / edge.w);
        }
    }
}
