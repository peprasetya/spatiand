//! A figure the wearer chose for the assistant, read from a file: glTF binary (`.glb`) and VRM,
//! which is glTF with a humanoid and its expressions described (`docs/ai-integrations.md`).
//!
//! Read here, by a few hundred lines rather than a glTF library: what the room needs of a figure
//! is its meshes as they stand, their colours and pictures, and the shapes its face can make --
//! the expressions VRM names (`happy`, `blink`, the vowels its mouth makes) and the morph targets
//! they move. Skeletons are not posed: a figure stands as it was made, and the room moves it
//! whole. On a thread of its own, since a VRM is tens of megabytes and its pictures need
//! decoding.
//!
//! The room's frame is +X forward, +Y left, +Z up; glTF's is +Y up and a figure facing +Z (VRM
//! 1.0; VRM 0.x faces -Z), +X to its left. So glTF's (x, y, z) is the room's (z, x, y), for a
//! figure facing the wearer once the room turns it to face them.

use std::collections::HashMap;
use std::path::Path;

use glam::{Mat4, Quat, Vec3};
use serde_json::Value;

/// One primitive of one mesh, ready to draw: in the figure's own frame, at its height.
pub struct Part {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Colours painted on its vertices, which tint the material's; empty when it has none.
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    pub material: usize,
    /// Each morph target's moves of the positions, as the file has them.
    pub targets: Vec<Vec<[f32; 3]>>,
    /// Which mesh it belongs to, for the expressions, which name meshes.
    pub mesh: usize,
}

pub struct Material {
    pub color: [f32; 4],
    pub texture: Option<usize>,
    /// Below this alpha, nothing is drawn: hair's ends, lashes.
    pub cutoff: f32,
}

/// A picture, decoded.
pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub rgba: Vec<u8>,
}

pub struct Figure {
    pub parts: Vec<Part>,
    pub materials: Vec<Material>,
    pub pictures: Vec<Picture>,
    /// Expressions by their VRM name (`happy`, `blink`, `aa`, `oh`, ...): which mesh's which
    /// target, how far.
    pub expressions: HashMap<String, Vec<(usize, usize, f32)>>,
    /// The figure's lowest and highest points, room frame, before it is made smaller.
    pub low: Vec3,
    pub high: Vec3,
}

/// Read a figure.
pub fn read(path: &Path) -> Result<Figure, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&bytes)
}

pub fn parse(bytes: &[u8]) -> Result<Figure, String> {
    let (json, bin) = chunks(bytes)?;
    let root: Value = serde_json::from_slice(json).map_err(|e| format!("the figure's description is not JSON: {e}"))?;
    let gltf = Gltf { root: &root, bin };

    // VRM 0.x faces away from where VRM 1.0 and glTF face.
    let old_vrm = root.pointer("/extensions/VRM").is_some();
    let vrm = old_vrm || root.pointer("/extensions/VRMC_vrm").is_some();
    let turn = if old_vrm { Mat4::from_rotation_z(std::f32::consts::PI) } else { Mat4::IDENTITY };
    let to_room = turn * Mat4::from_cols_array(&[0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]);

    // Every node's place in the figure.
    let nodes = root.get("nodes").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut world = vec![Mat4::IDENTITY; nodes.len()];
    let scene = root.get("scene").and_then(Value::as_u64).unwrap_or(0) as usize;
    let tops: Vec<usize> = root
        .pointer(&format!("/scenes/{scene}/nodes"))
        .and_then(Value::as_array)
        .map(|n| n.iter().filter_map(|v| v.as_u64().map(|i| i as usize)).collect())
        .unwrap_or_else(|| (0..nodes.len()).collect());
    let mut stack: Vec<(usize, Mat4)> = tops.iter().map(|i| (*i, Mat4::IDENTITY)).collect();
    let mut seen = vec![false; nodes.len()];
    while let Some((i, parent)) = stack.pop() {
        if i >= nodes.len() || seen[i] {
            continue;
        }
        seen[i] = true;
        let m = parent * local(&nodes[i]);
        world[i] = m;
        for child in nodes[i].get("children").and_then(Value::as_array).into_iter().flatten() {
            if let Some(c) = child.as_u64() {
                stack.push((c as usize, m));
            }
        }
    }

    let meshes = root.get("meshes").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut parts = Vec::new();
    for (n, node) in nodes.iter().enumerate() {
        let Some(mesh) = node.get("mesh").and_then(Value::as_u64).map(|m| m as usize) else { continue };
        // A skinned mesh is posed by its skeleton, which at rest is where its vertices already
        // are; its node's own place does not apply.
        let place = if node.get("skin").is_some() { Mat4::IDENTITY } else { world[n] };
        let m = to_room * place;
        let normal_m = glam::Mat3::from_mat4(m).inverse().transpose();
        for primitive in meshes.get(mesh).and_then(|m| m.get("primitives")).and_then(Value::as_array).into_iter().flatten() {
            if primitive.get("mode").and_then(Value::as_u64).unwrap_or(4) != 4 {
                continue;
            }
            let attr = |name: &str| primitive.pointer(&format!("/attributes/{name}")).and_then(Value::as_u64).map(|a| a as usize);
            let Some(positions) = attr("POSITION").map(|a| gltf.floats(a, 3)).transpose()? else { continue };
            let count = positions.len() / 3;
            let normals = match attr("NORMAL") {
                Some(a) => gltf.floats(a, 3)?,
                None => vec![0.0; count * 3],
            };
            let uvs = match attr("TEXCOORD_0") {
                Some(a) => gltf.floats(a, 2)?,
                None => vec![0.0; count * 2],
            };
            // Colours painted on the vertices tint the material's. A VRM's are left alone, as its
            // own renderer (MToon) leaves them; and a set that does not fit is no colour at all.
            let colors = attr("COLOR_0")
                .filter(|_| !vrm)
                .and_then(|a| {
                    let width = if gltf.kind(a) == Some("VEC4") { 4 } else { 3 };
                    let c = gltf.floats(a, width).ok()?;
                    Some(c.chunks_exact(width).map(|c| [c[0], c[1], c[2], if width == 4 { c[3] } else { 1.0 }]).collect::<Vec<_>>())
                })
                .filter(|c| c.len() == count)
                .unwrap_or_default();
            let indices = match primitive.get("indices").and_then(Value::as_u64) {
                Some(a) => gltf.indices(a as usize)?,
                None => (0..count as u32).collect(),
            };
            let mut targets = Vec::new();
            for target in primitive.get("targets").and_then(Value::as_array).into_iter().flatten() {
                let moves = match target.get("POSITION").and_then(Value::as_u64) {
                    Some(a) => gltf.floats(a as usize, 3).unwrap_or_else(|_| vec![0.0; count * 3]),
                    None => vec![0.0; count * 3],
                };
                targets.push(moves.chunks_exact(3).map(|v| (glam::Mat3::from_mat4(m) * Vec3::new(v[0], v[1], v[2])).to_array()).collect());
            }
            parts.push(Part {
                positions: positions.chunks_exact(3).map(|v| m.transform_point3(Vec3::new(v[0], v[1], v[2])).to_array()).collect(),
                normals: normals.chunks_exact(3).map(|v| (normal_m * Vec3::new(v[0], v[1], v[2])).normalize_or_zero().to_array()).collect(),
                uvs: uvs.chunks_exact(2).map(|v| [v[0], v[1]]).collect(),
                colors,
                indices,
                material: primitive.get("material").and_then(Value::as_u64).unwrap_or(0) as usize,
                targets,
                mesh,
            });
        }
    }
    if parts.is_empty() {
        return Err("the figure has nothing to draw".into());
    }

    let materials = root
        .get("materials")
        .and_then(Value::as_array)
        .map(|all| {
            all.iter()
                .map(|m| {
                    let pbr = m.get("pbrMetallicRoughness");
                    let factor = pbr.and_then(|p| p.get("baseColorFactor")).and_then(Value::as_array);
                    let color = factor
                        .map(|f| {
                            let c: Vec<f32> = f.iter().map(|x| x.as_f64().unwrap_or(1.0) as f32).collect();
                            [c.first().copied().unwrap_or(1.0), c.get(1).copied().unwrap_or(1.0), c.get(2).copied().unwrap_or(1.0), c.get(3).copied().unwrap_or(1.0)]
                        })
                        .unwrap_or([1.0; 4]);
                    let texture = pbr.and_then(|p| p.pointer("/baseColorTexture/index")).and_then(Value::as_u64).map(|t| t as usize);
                    let cutoff = match m.get("alphaMode").and_then(Value::as_str) {
                        Some("MASK") => m.get("alphaCutoff").and_then(Value::as_f64).unwrap_or(0.5) as f32,
                        // Blended parts are drawn as cut out: the figure's picture is one layer.
                        Some("BLEND") => 0.5,
                        _ => 0.0,
                    };
                    Material { color, texture, cutoff }
                })
                .collect()
        })
        .unwrap_or_default();

    // A texture names its image; the pictures are kept by texture.
    let textures = root.get("textures").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut pictures = Vec::new();
    for t in &textures {
        let image = t.get("source").and_then(Value::as_u64).map(|s| s as usize);
        let picture = image.and_then(|i| gltf.image(i).ok()).unwrap_or(Picture { width: 1, height: 1, rgba: vec![255; 4] });
        pictures.push(picture);
    }

    let expressions = expressions(&root);
    let mut low = Vec3::splat(f32::MAX);
    let mut high = Vec3::splat(f32::MIN);
    for p in &parts {
        for v in &p.positions {
            low = low.min(Vec3::from(*v));
            high = high.max(Vec3::from(*v));
        }
    }
    Ok(Figure { parts, materials, pictures, expressions, low, high })
}

/// A node's own place: its matrix, or its translation, rotation and scale.
fn local(node: &Value) -> Mat4 {
    let floats = |key: &str| node.get(key).and_then(Value::as_array).map(|a| a.iter().map(|v| v.as_f64().unwrap_or(0.0) as f32).collect::<Vec<f32>>());
    if let Some(m) = floats("matrix").filter(|m| m.len() == 16) {
        return Mat4::from_cols_slice(&m);
    }
    let t = floats("translation").filter(|t| t.len() == 3).map(|t| Vec3::new(t[0], t[1], t[2])).unwrap_or(Vec3::ZERO);
    let r = floats("rotation").filter(|r| r.len() == 4).map(|r| Quat::from_xyzw(r[0], r[1], r[2], r[3]).normalize()).unwrap_or(Quat::IDENTITY);
    let s = floats("scale").filter(|s| s.len() == 3).map(|s| Vec3::new(s[0], s[1], s[2])).unwrap_or(Vec3::ONE);
    Mat4::from_scale_rotation_translation(s, r, t)
}

/// The expressions a VRM names, as (mesh, target, weight 0..1), under VRM 1.0's names.
fn expressions(root: &Value) -> HashMap<String, Vec<(usize, usize, f32)>> {
    let mut out: HashMap<String, Vec<(usize, usize, f32)>> = HashMap::new();
    // VRM 1.0: by node, and the node's mesh.
    if let Some(presets) = root.pointer("/extensions/VRMC_vrm/expressions/preset").and_then(Value::as_object) {
        for (name, expression) in presets {
            for bind in expression.get("morphTargetBinds").and_then(Value::as_array).into_iter().flatten() {
                let node = bind.get("node").and_then(Value::as_u64).unwrap_or(u64::MAX) as usize;
                let Some(mesh) = root.pointer(&format!("/nodes/{node}/mesh")).and_then(Value::as_u64) else { continue };
                let index = bind.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let weight = bind.get("weight").and_then(Value::as_f64).unwrap_or(1.0) as f32;
                out.entry(name.clone()).or_default().push((mesh as usize, index, weight));
            }
        }
    }
    // VRM 0.x: by mesh, weights out of a hundred, and its own names.
    if let Some(groups) = root.pointer("/extensions/VRM/blendShapeMaster/blendShapeGroups").and_then(Value::as_array) {
        for group in groups {
            let preset = group.get("presetName").and_then(Value::as_str).unwrap_or_default();
            let name = match preset {
                "joy" => "happy",
                "sorrow" => "sad",
                "fun" => "relaxed",
                "a" => "aa",
                "i" => "ih",
                "u" => "ou",
                "e" => "ee",
                "o" => "oh",
                other => other,
            };
            for bind in group.get("binds").and_then(Value::as_array).into_iter().flatten() {
                let mesh = bind.get("mesh").and_then(Value::as_u64).unwrap_or(u64::MAX) as usize;
                let index = bind.get("index").and_then(Value::as_u64).unwrap_or(0) as usize;
                let weight = bind.get("weight").and_then(Value::as_f64).unwrap_or(100.0) as f32 / 100.0;
                out.entry(name.to_string()).or_default().push((mesh, index, weight));
            }
        }
    }
    out
}

/// The JSON and the binary chunk of a GLB.
fn chunks(bytes: &[u8]) -> Result<(&[u8], &[u8]), String> {
    let u32_at = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    if bytes.get(0..4) != Some(b"glTF") {
        return Err("not a glTF binary (.glb, .vrm)".into());
    }
    let mut at = 12;
    let (mut json, mut bin): (&[u8], &[u8]) = (&[], &[]);
    while at + 8 <= bytes.len() {
        let length = u32_at(at).ok_or("a broken chunk")? as usize;
        let kind = u32_at(at + 4).ok_or("a broken chunk")?;
        let body = bytes.get(at + 8..at + 8 + length).ok_or("a chunk runs past the end of the file")?;
        match kind {
            0x4E4F_534A => json = body,
            0x004E_4942 => bin = body,
            _ => {}
        }
        at += 8 + length;
    }
    if json.is_empty() {
        return Err("the figure has no description".into());
    }
    Ok((json, bin))
}

struct Gltf<'a> {
    root: &'a Value,
    bin: &'a [u8],
}

impl Gltf<'_> {
    /// The bytes of a buffer view: only the file's own binary chunk is read.
    fn view(&self, view: usize) -> Result<(&[u8], usize), String> {
        let v = self.root.pointer(&format!("/bufferViews/{view}")).ok_or("a buffer view that is not there")?;
        if v.get("buffer").and_then(Value::as_u64).unwrap_or(0) != 0 {
            return Err("a figure whose data is outside the file".into());
        }
        let offset = v.get("byteOffset").and_then(Value::as_u64).unwrap_or(0) as usize;
        let length = v.get("byteLength").and_then(Value::as_u64).unwrap_or(0) as usize;
        let stride = v.get("byteStride").and_then(Value::as_u64).unwrap_or(0) as usize;
        let bytes = self.bin.get(offset..offset + length).ok_or("a buffer view runs past the data")?;
        Ok((bytes, stride))
    }

    /// An accessor's values as floats, `width` to an element.
    /// An accessor's shape: `SCALAR`, `VEC3`, ...
    fn kind(&self, accessor: usize) -> Option<&str> {
        self.root.pointer(&format!("/accessors/{accessor}/type")).and_then(Value::as_str)
    }

    fn floats(&self, accessor: usize, width: usize) -> Result<Vec<f32>, String> {
        let a = self.root.pointer(&format!("/accessors/{accessor}")).ok_or("an accessor that is not there")?;
        let count = a.get("count").and_then(Value::as_u64).unwrap_or(0) as usize;
        let Some(view) = a.get("bufferView").and_then(Value::as_u64) else {
            // All zeroes, with only a sparse part set: taken as all zeroes.
            return Ok(vec![0.0; count * width]);
        };
        let (bytes, stride) = self.view(view as usize)?;
        let offset = a.get("byteOffset").and_then(Value::as_u64).unwrap_or(0) as usize;
        let kind = a.get("componentType").and_then(Value::as_u64).unwrap_or(5126);
        let normalized = a.get("normalized").and_then(Value::as_bool).unwrap_or(false);
        let size = match kind {
            5126 | 5125 => 4,
            5123 | 5122 => 2,
            _ => 1,
        };
        let stride = if stride == 0 { size * width } else { stride };
        let mut out = Vec::with_capacity(count * width);
        for i in 0..count {
            for c in 0..width {
                let at = offset + i * stride + c * size;
                let b = bytes.get(at..at + size).ok_or("an accessor runs past its data")?;
                let v = match kind {
                    5126 => f32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                    5123 => {
                        let v = u16::from_le_bytes([b[0], b[1]]) as f32;
                        if normalized { v / 65535.0 } else { v }
                    }
                    5122 => {
                        let v = i16::from_le_bytes([b[0], b[1]]) as f32;
                        if normalized { (v / 32767.0).max(-1.0) } else { v }
                    }
                    5121 => {
                        let v = b[0] as f32;
                        if normalized { v / 255.0 } else { v }
                    }
                    5120 => {
                        let v = b[0] as i8 as f32;
                        if normalized { (v / 127.0).max(-1.0) } else { v }
                    }
                    _ => u32::from_le_bytes([b[0], b[1], b[2], b[3]]) as f32,
                };
                out.push(v);
            }
        }
        Ok(out)
    }

    fn indices(&self, accessor: usize) -> Result<Vec<u32>, String> {
        let a = self.root.pointer(&format!("/accessors/{accessor}")).ok_or("an accessor that is not there")?;
        let count = a.get("count").and_then(Value::as_u64).unwrap_or(0) as usize;
        let view = a.get("bufferView").and_then(Value::as_u64).ok_or("indices with no data")? as usize;
        let (bytes, _) = self.view(view)?;
        let offset = a.get("byteOffset").and_then(Value::as_u64).unwrap_or(0) as usize;
        let kind = a.get("componentType").and_then(Value::as_u64).unwrap_or(5125);
        let size = match kind {
            5125 => 4,
            5123 => 2,
            _ => 1,
        };
        (0..count)
            .map(|i| {
                let at = offset + i * size;
                let b = bytes.get(at..at + size).ok_or_else(|| "indices run past their data".to_string())?;
                Ok(match size {
                    4 => u32::from_le_bytes([b[0], b[1], b[2], b[3]]),
                    2 => u16::from_le_bytes([b[0], b[1]]) as u32,
                    _ => b[0] as u32,
                })
            })
            .collect()
    }

    fn image(&self, image: usize) -> Result<Picture, String> {
        let i = self.root.pointer(&format!("/images/{image}")).ok_or("an image that is not there")?;
        let view = i.get("bufferView").and_then(Value::as_u64).ok_or("an image outside the file")? as usize;
        let (bytes, _) = self.view(view)?;
        let decoded = image::load_from_memory(bytes).map_err(|e| format!("a picture of the figure's would not read: {e}"))?;
        // Large enough to look at from a metre away, small enough not to fill the GPU.
        let decoded = if decoded.width() > 1024 || decoded.height() > 1024 { decoded.thumbnail(1024, 1024) } else { decoded };
        let rgba = decoded.to_rgba8();
        Ok(Picture { width: rgba.width(), height: rgba.height(), rgba: rgba.into_raw() })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A GLB of one triangle with one morph target and painted colours, made here byte by byte.
    fn triangle(vrm: bool) -> Vec<u8> {
        let positions: [f32; 9] = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 2.0, 0.0];
        let target: [f32; 9] = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0];
        let indices: [u16; 3] = [0, 1, 2];
        let mut bin: Vec<u8> = Vec::new();
        bin.extend(positions.iter().flat_map(|f| f.to_le_bytes()));
        bin.extend(target.iter().flat_map(|f| f.to_le_bytes()));
        bin.extend(indices.iter().flat_map(|i| i.to_le_bytes()));
        bin.extend([0, 0]);
        bin.extend([255, 255, 255, 255, 255, 0, 0, 255, 0, 0, 255, 128]);
        let extensions = if vrm {
            r#","extensions":{"VRMC_vrm":{"expressions":{"preset":{"aa":{"morphTargetBinds":[{"node":0,"index":0,"weight":1.0}]}}}}}"#
        } else {
            ""
        };
        let json = format!(
            r#"{{"asset":{{"version":"2.0"}},"scene":0,"scenes":[{{"nodes":[0]}}],"nodes":[{{"mesh":0,"translation":[0,1,0]}}],"meshes":[{{"primitives":[{{"attributes":{{"POSITION":0,"COLOR_0":3}},"indices":2,"targets":[{{"POSITION":1}}]}}]}}],"buffers":[{{"byteLength":{}}}],"bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":36}},{{"buffer":0,"byteOffset":36,"byteLength":36}},{{"buffer":0,"byteOffset":72,"byteLength":6}},{{"buffer":0,"byteOffset":80,"byteLength":12}}],"accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"}},{{"bufferView":1,"componentType":5126,"count":3,"type":"VEC3"}},{{"bufferView":2,"componentType":5123,"count":3,"type":"SCALAR"}},{{"bufferView":3,"componentType":5121,"normalized":true,"count":3,"type":"VEC4"}}]{extensions}}}"#,
            bin.len()
        );
        let mut json = json.into_bytes();
        while json.len() % 4 != 0 {
            json.push(b' ');
        }
        let mut out = Vec::new();
        out.extend(b"glTF");
        out.extend(2u32.to_le_bytes());
        out.extend(((12 + 8 + json.len() + 8 + bin.len()) as u32).to_le_bytes());
        out.extend((json.len() as u32).to_le_bytes());
        out.extend(0x4E4F_534Au32.to_le_bytes());
        out.extend(&json);
        out.extend((bin.len() as u32).to_le_bytes());
        out.extend(0x004E_4942u32.to_le_bytes());
        out.extend(&bin);
        out
    }

    #[test]
    fn a_glb_is_read_into_the_rooms_frame() {
        let f = parse(&triangle(false)).unwrap();
        assert_eq!(f.parts.len(), 1);
        let p = &f.parts[0];
        // glTF's (0, 2, 0), lifted by the node's (0, 1, 0), is 3 up in the room: its z.
        assert_eq!(p.positions[2], [0.0, 0.0, 3.0]);
        // glTF's +X is the room's +Y.
        assert_eq!(p.positions[1], [0.0, 1.0, 1.0]);
        assert_eq!(p.indices, vec![0, 1, 2]);
        assert_eq!(p.targets[0][2], [0.0, 0.0, 0.5]);
        assert!((f.high.z - 3.0).abs() < 1e-6 && (f.low.z - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_vrm_says_which_target_makes_which_face() {
        let f = parse(&triangle(true)).unwrap();
        assert_eq!(f.expressions.get("aa"), Some(&vec![(0, 0, 1.0)]));
    }

    #[test]
    fn painted_colours_tint_a_glb_but_not_a_vrm() {
        let glb = parse(&triangle(false)).unwrap();
        assert_eq!(glb.parts[0].colors[1], [1.0, 0.0, 0.0, 1.0]);
        assert!((glb.parts[0].colors[2][3] - 128.0 / 255.0).abs() < 1e-6);
        // MToon, a VRM's own renderer, paints none.
        assert!(parse(&triangle(true)).unwrap().parts[0].colors.is_empty());
    }

    #[test]
    fn what_is_not_a_figure_says_so() {
        assert!(parse(b"not a glb at all").is_err());
    }
}
