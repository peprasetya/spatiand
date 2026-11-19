//! A figure the wearer chose for the assistant, read from a file: glTF binary (`.glb`) and VRM,
//! which is glTF with a humanoid and its expressions described (`docs/ai-integrations.md`,
//! `docs/spatipal-pals.md`).
//!
//! Read here, by a few hundred lines rather than a glTF library: what the room needs of a figure
//! is its meshes, their colours and pictures, the shapes its face can make -- the expressions VRM
//! names (`happy`, `blink`, the vowels its mouth makes), or in a plain GLB its morph targets by
//! their own names -- and how it moves (2026-11-19, the plan's "moods, animations, outfits"):
//!
//! - **Its skeleton**: glTF skins, each vertex hanging from up to four joints, posed here on the
//!   CPU -- a figure is a few thousand vertices, and the same code then runs on every GPU the room
//!   runs on. A figure with no animations stands as it was made.
//! - **Its animations, by name**: one named after what the assistant is doing (`idle`,
//!   `listening`, `talking`, `thinking`, ...; `idle` stands in for any it lacks), or a move or a
//!   pose the assistant asks for. Sampled here as glTF has them: step, linear, cubic.
//! - **Its options**: a node named `group=option` (`outfit=dress`, `hair=bob`; Blender's `.001`
//!   left off) is one of that group's choices, with everything under it, shown only while it is
//!   the one chosen. Several nodes may make up one option.
//! - **Its body**: which of its joints are its spine, its head, its arms and its legs -- a VRM's
//!   humanoid, or any person's skeleton found by its shape (`spatiand_voice::figure::bodies`) --
//!   for a pose of the assistant's own making (`crate::posing`). A figure with a body moves by
//!   its skeleton even with no animations.
//!
//! On a thread of its own, since a VRM is tens of megabytes and its pictures need decoding.
//!
//! The room's frame is +X forward, +Y left, +Z up; glTF's is +Y up and a figure facing +Z (VRM
//! 1.0; VRM 0.x faces -Z), +X to its left. So glTF's (x, y, z) is the room's (z, x, y), for a
//! figure facing the wearer once the room turns it to face them.

use std::collections::HashMap;
use std::path::Path;

use glam::{Mat3, Mat4, Quat, Vec3};
use serde_json::Value;

/// One primitive of one mesh, ready to draw: in the figure's own frame, at its height.
pub struct Part {
    /// Where it is at rest, room frame: as it is drawn while nothing moves it.
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub uvs: Vec<[f32; 2]>,
    /// Colours painted on its vertices, which tint the material's; empty when it has none.
    pub colors: Vec<[f32; 4]>,
    pub indices: Vec<u32>,
    pub material: usize,
    /// Each morph target's moves of the positions at rest, room frame.
    pub targets: Vec<Vec<[f32; 3]>>,
    /// Which mesh it belongs to, for the expressions, which name meshes.
    pub mesh: usize,
    /// How a pose moves it.
    pub moves: Moves,
    /// Which group's option it is (`("outfit", "dress")` under a node named `outfit=dress`), in
    /// small letters; `None` for a part that is always there.
    pub option: Option<(String, String)>,
}

/// How a part follows the figure's pose.
pub enum Moves {
    /// Nothing that is animated moves it: drawn as it was read.
    Still,
    /// Rigidly, with its node (a robot's arm, a thing held).
    Node { node: usize, local: Local },
    /// By a skin: each vertex's joints (places in the skin's list of them) and their weights.
    Skin { skin: usize, local: Local, joints: Vec<[u16; 4]>, weights: Vec<[f32; 4]> },
}

/// A part as its file has it, in its own mesh's frame: what a pose starts from.
pub struct Local {
    pub positions: Vec<[f32; 3]>,
    pub normals: Vec<[f32; 3]>,
    pub targets: Vec<Vec<[f32; 3]>>,
}

pub struct Material {
    /// Its name in small letters: a colour can be asked for a material by it (`hair`).
    pub name: String,
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
    /// Expressions by their VRM name (`happy`, `blink`, `aa`, `oh`, ...), or a plain GLB's morph
    /// targets by theirs: which mesh's which target, how far.
    pub expressions: HashMap<String, Vec<(usize, usize, f32)>>,
    /// The figure's lowest and highest points at rest, room frame, before it is made smaller.
    pub low: Vec3,
    pub high: Vec3,
    pub skeleton: Skeleton,
    /// Its groups of options, in small letters, each with its options as the file first has them.
    pub groups: Vec<(String, Vec<String>)>,
}

/// A node's own place: translation, rotation and scale, which is what animations move.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Trs {
    pub t: Vec3,
    pub r: Quat,
    pub s: Vec3,
}

impl Default for Trs {
    fn default() -> Self {
        Trs { t: Vec3::ZERO, r: Quat::IDENTITY, s: Vec3::ONE }
    }
}

impl Trs {
    pub fn matrix(&self) -> Mat4 {
        Mat4::from_scale_rotation_translation(self.s, self.r, self.t)
    }

    /// Part of the way from `self` to `to`: `0` is `self`, `1` is `to`.
    pub fn towards(&self, to: &Trs, k: f32) -> Trs {
        // The short way round: q and -q are the same turn.
        let r = if self.r.dot(to.r) < 0.0 { -to.r } else { to.r };
        Trs { t: self.t.lerp(to.t, k), r: self.r.slerp(r, k).normalize(), s: self.s.lerp(to.s, k) }
    }
}

pub struct Node {
    pub name: String,
    pub parent: Option<usize>,
    pub rest: Trs,
}

pub struct Skin {
    pub joints: Vec<usize>,
    pub inverse_bind: Vec<Mat4>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Channel {
    Translation,
    Rotation,
    Scale,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Interpolation {
    Step,
    Linear,
    CubicSpline,
}

/// One node's one channel through an animation.
pub struct Track {
    pub node: usize,
    pub channel: Channel,
    pub interpolation: Interpolation,
    pub times: Vec<f32>,
    /// Three or four to a key (a rotation's quaternion, x y z w); cubic splines have each key's
    /// in-tangent, value and out-tangent, in that order.
    pub values: Vec<f32>,
}

pub struct Clip {
    pub name: String,
    pub duration: f32,
    pub tracks: Vec<Track>,
}

/// What poses a figure: its nodes (each with its parent and its place at rest), its skins, its
/// animations.
pub struct Skeleton {
    pub nodes: Vec<Node>,
    /// The nodes of the figure's scene, parents before children.
    pub order: Vec<usize>,
    pub skins: Vec<Skin>,
    pub clips: Vec<Clip>,
    /// From the file's frame to the room's.
    pub to_room: Mat4,
    /// Its skeletons that are a person's, for a pose of the assistant's own making.
    pub bodies: Vec<spatiand_voice::figure::Body>,
    /// Which way it faces in its file: `1.0` +Z, `-1.0` -Z (a VRM 0.x); its own left is +X times it.
    pub facing: f32,
}

impl Skeleton {
    /// Every node at rest.
    pub fn rest(&self) -> Vec<Trs> {
        self.nodes.iter().map(|n| n.rest).collect()
    }

    /// An animation by its name, in any case. An exporter's `Armature|wave` is `wave` too.
    pub fn clip(&self, name: &str) -> Option<usize> {
        let name = name.trim();
        if name.is_empty() {
            return None;
        }
        let same = |a: &str| a.eq_ignore_ascii_case(name);
        self.clips.iter().position(|c| same(&c.name)).or_else(|| self.clips.iter().position(|c| c.name.rsplit('|').next().is_some_and(same)))
    }

    /// Lay animation `clip` at `time` seconds over `local` (each node's own place).
    pub fn sample(&self, clip: usize, time: f32, local: &mut [Trs]) {
        for track in self.clips.get(clip).map(|c| c.tracks.as_slice()).unwrap_or_default() {
            if let Some(place) = local.get_mut(track.node) {
                track.sample(time, place);
            }
        }
    }

    /// Each node's place in the figure (file frame), from each node's own.
    pub fn world(&self, local: &[Trs]) -> Vec<Mat4> {
        let mut world = vec![Mat4::IDENTITY; self.nodes.len()];
        for &i in &self.order {
            let own = local.get(i).map(Trs::matrix).unwrap_or(Mat4::IDENTITY);
            world[i] = match self.nodes[i].parent {
                Some(p) if p < world.len() => world[p] * own,
                _ => own,
            };
        }
        world
    }

    /// A skin's joints as matrices from the mesh's frame to the room's, in the pose `world`.
    fn joint_matrices(&self, skin: usize, world: &[Mat4]) -> Vec<Mat4> {
        let Some(skin) = self.skins.get(skin) else { return Vec::new() };
        skin.joints.iter().enumerate().map(|(k, j)| self.to_room * world.get(*j).copied().unwrap_or(Mat4::IDENTITY) * skin.inverse_bind.get(k).copied().unwrap_or(Mat4::IDENTITY)).collect()
    }

    /// Where a moving part is in the pose `world`, room frame: its positions and normals, its
    /// morph targets moved first by `morph` (target, weight). `None` for a part that does not move.
    pub fn place(&self, moves: &Moves, world: &[Mat4], morph: &[(usize, f32)]) -> Option<(Vec<[f32; 3]>, Vec<[f32; 3]>)> {
        let morphed = |local: &Local| -> Vec<[f32; 3]> {
            let mut out = local.positions.clone();
            for (target, w) in morph {
                let Some(moves) = local.targets.get(*target) else { continue };
                for (v, d) in out.iter_mut().zip(moves) {
                    v[0] += d[0] * w;
                    v[1] += d[1] * w;
                    v[2] += d[2] * w;
                }
            }
            out
        };
        match moves {
            Moves::Still => None,
            Moves::Node { node, local } => {
                let m = self.to_room * world.get(*node).copied().unwrap_or(Mat4::IDENTITY);
                let n = Mat3::from_mat4(m).inverse().transpose();
                let positions = morphed(local).iter().map(|v| m.transform_point3(Vec3::from(*v)).to_array()).collect();
                let normals = local.normals.iter().map(|v| (n * Vec3::from(*v)).normalize_or_zero().to_array()).collect();
                Some((positions, normals))
            }
            Moves::Skin { skin, local, joints, weights } => {
                let mats = self.joint_matrices(*skin, world);
                Some(skinned(&morphed(local), &local.normals, joints, weights, &mats))
            }
        }
    }
}

/// Points and normals moved by their joints: each by its joints' matrices, as far as each weighs.
fn skinned(positions: &[[f32; 3]], normals: &[[f32; 3]], joints: &[[u16; 4]], weights: &[[f32; 4]], mats: &[Mat4]) -> (Vec<[f32; 3]>, Vec<[f32; 3]>) {
    let mut out_p = Vec::with_capacity(positions.len());
    let mut out_n = Vec::with_capacity(positions.len());
    let fallback = mats.first().copied().unwrap_or(Mat4::IDENTITY);
    for (v, p) in positions.iter().enumerate() {
        let mut m = Mat4::ZERO;
        let mut total = 0.0;
        if let (Some(j), Some(w)) = (joints.get(v), weights.get(v)) {
            for k in 0..4 {
                if w[k] > 0.0 {
                    if let Some(joint) = mats.get(j[k] as usize) {
                        m += *joint * w[k];
                        total += w[k];
                    }
                }
            }
        }
        // Hung from nothing (a file's slip): it goes with the first joint.
        let m = if total > 1e-6 { m * (1.0 / total) } else { fallback };
        out_p.push(m.transform_point3(Vec3::from(*p)).to_array());
        let n = normals.get(v).copied().unwrap_or([0.0, 0.0, 1.0]);
        out_n.push((Mat3::from_mat4(m) * Vec3::from(n)).normalize_or_zero().to_array());
    }
    (out_p, out_n)
}

impl Channel {
    fn width(self) -> usize {
        if self == Channel::Rotation { 4 } else { 3 }
    }
}

impl Track {
    /// This track's value at `time`, written into `place`; held at its ends.
    fn sample(&self, time: f32, place: &mut Trs) {
        let n = self.times.len();
        if n == 0 {
            return;
        }
        let width = self.channel.width();
        let cubic = self.interpolation == Interpolation::CubicSpline;
        let stride = if cubic { width * 3 } else { width };
        // A key's value, and for a cubic spline its tangents (0 in, 2 out).
        let at = |key: usize, part: usize| -> [f32; 4] {
            let base = key * stride + if cubic { part * width } else { 0 };
            let mut v = [0.0; 4];
            v[..width].copy_from_slice(&self.values[base..base + width]);
            v
        };
        let value = if time <= self.times[0] || n == 1 {
            at(0, 1)
        } else if time >= self.times[n - 1] {
            at(n - 1, 1)
        } else {
            let k = self.times.partition_point(|t| *t <= time).saturating_sub(1).min(n - 2);
            let (t0, t1) = (self.times[k], self.times[k + 1]);
            let span = (t1 - t0).max(1e-6);
            let u = ((time - t0) / span).clamp(0.0, 1.0);
            match self.interpolation {
                Interpolation::Step => at(k, 1),
                Interpolation::Linear if self.channel == Channel::Rotation => {
                    let (a, b) = (quat(at(k, 1)), quat(at(k + 1, 1)));
                    let b = if a.dot(b) < 0.0 { -b } else { b };
                    a.slerp(b, u).to_array()
                }
                Interpolation::Linear => {
                    let (a, b) = (at(k, 1), at(k + 1, 1));
                    std::array::from_fn(|i| a[i] + (b[i] - a[i]) * u)
                }
                Interpolation::CubicSpline => {
                    let (p0, m0, p1, m1) = (at(k, 1), at(k, 2), at(k + 1, 1), at(k + 1, 0));
                    let (u2, u3) = (u * u, u * u * u);
                    let (h00, h10, h01, h11) = (2.0 * u3 - 3.0 * u2 + 1.0, u3 - 2.0 * u2 + u, -2.0 * u3 + 3.0 * u2, u3 - u2);
                    std::array::from_fn(|i| h00 * p0[i] + h10 * span * m0[i] + h01 * p1[i] + h11 * span * m1[i])
                }
            }
        };
        match self.channel {
            Channel::Translation => place.t = Vec3::new(value[0], value[1], value[2]),
            Channel::Scale => place.s = Vec3::new(value[0], value[1], value[2]),
            Channel::Rotation => place.r = quat(value).normalize(),
        }
    }
}

fn quat(v: [f32; 4]) -> Quat {
    Quat::from_xyzw(v[0], v[1], v[2], v[3])
}

/// Read a figure.
pub fn read(path: &Path) -> Result<Figure, String> {
    let bytes = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    parse(&bytes)
}

/// A node's group and option, from its name: the rule SpatiPal reads figures by too.
pub use spatiand_voice::figure::option_of;

pub fn parse(bytes: &[u8]) -> Result<Figure, String> {
    let (json, bin) = chunks(bytes)?;
    let root: Value = serde_json::from_slice(json).map_err(|e| format!("the figure's description is not JSON: {e}"))?;
    let gltf = Gltf { root: &root, bin };

    // VRM 0.x faces away from where VRM 1.0 and glTF face.
    let old_vrm = root.pointer("/extensions/VRM").is_some();
    let vrm = old_vrm || root.pointer("/extensions/VRMC_vrm").is_some();
    let turn = if old_vrm { Mat4::from_rotation_z(std::f32::consts::PI) } else { Mat4::IDENTITY };
    let to_room = turn * Mat4::from_cols_array(&[0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0]);

    // The nodes: each one's place at rest and its parent, and the scene's nodes parents first.
    let node_list = root.get("nodes").and_then(Value::as_array).cloned().unwrap_or_default();
    let children = |i: usize| -> Vec<usize> {
        node_list[i].get("children").and_then(Value::as_array).into_iter().flatten().filter_map(|c| c.as_u64().map(|c| c as usize)).filter(|c| *c < node_list.len()).collect()
    };
    let mut nodes: Vec<Node> = node_list.iter().map(|n| Node { name: n.get("name").and_then(Value::as_str).unwrap_or_default().to_string(), parent: None, rest: local(n) }).collect();
    for i in 0..node_list.len() {
        for c in children(i) {
            nodes[c].parent.get_or_insert(i);
        }
    }
    let order = spatiand_voice::figure::scene_order(&root);

    let skins = skins(&root, &gltf)?;
    let clips = clips(&root, &gltf, node_list.len());
    let bodies = spatiand_voice::figure::bodies(&root);
    let facing = spatiand_voice::figure::facing(&root);
    let skeleton = Skeleton { nodes, order, skins, clips, to_room, bodies, facing };
    let world = skeleton.world(&skeleton.rest());
    // What an animation or a pose moves: the nodes it names -- a body's, every joint of its skins
    // -- and everything under them.
    let mut moved = vec![false; node_list.len()];
    for clip in &skeleton.clips {
        for track in &clip.tracks {
            moved[track.node] = true;
        }
    }
    if !skeleton.bodies.is_empty() {
        for j in skeleton.skins.iter().flat_map(|s| &s.joints) {
            if let Some(m) = moved.get_mut(*j) {
                *m = true;
            }
        }
    }
    for &i in &skeleton.order {
        if let Some(p) = skeleton.nodes[i].parent {
            moved[i] |= moved[p];
        }
    }
    // Each node's option: its own name's, or the nearest above it with one.
    let mut options: Vec<Option<(String, String)>> = vec![None; node_list.len()];
    for &i in &skeleton.order {
        options[i] = option_of(&skeleton.nodes[i].name).or_else(|| skeleton.nodes[i].parent.and_then(|p| options[p].clone()));
    }
    let groups = spatiand_voice::figure::groups(&root);

    let meshes = root.get("meshes").and_then(Value::as_array).cloned().unwrap_or_default();
    let mut parts = Vec::new();
    for (n, node) in node_list.iter().enumerate() {
        let Some(mesh) = node.get("mesh").and_then(Value::as_u64).map(|m| m as usize) else { continue };
        let skin = node.get("skin").and_then(Value::as_u64).map(|s| s as usize).filter(|s| *s < skeleton.skins.len());
        let m = to_room * world[n];
        let normal_m = Mat3::from_mat4(m).inverse().transpose();
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
                targets.push(moves.chunks_exact(3).map(|v| [v[0], v[1], v[2]]).collect::<Vec<[f32; 3]>>());
            }
            let triples = |v: &[f32]| v.chunks_exact(3).map(|v| [v[0], v[1], v[2]]).collect::<Vec<[f32; 3]>>();
            let local = Local { positions: triples(&positions), normals: triples(&normals), targets };
            // A skin's joints and weights, when it has them both for every vertex.
            let skinning = skin.and_then(|s| {
                let joints = gltf.floats(attr("JOINTS_0")?, 4).ok()?;
                let weights = gltf.floats(attr("WEIGHTS_0")?, 4).ok()?;
                (joints.len() == count * 4 && weights.len() == count * 4).then(|| {
                    let joints: Vec<[u16; 4]> = joints.chunks_exact(4).map(|j| [j[0] as u16, j[1] as u16, j[2] as u16, j[3] as u16]).collect();
                    let weights: Vec<[f32; 4]> = weights.chunks_exact(4).map(|w| [w[0], w[1], w[2], w[3]]).collect();
                    (s, joints, weights)
                })
            });
            let (rest_positions, rest_normals, rest_targets, moves) = match skinning {
                // Skinned: at rest it is where its skeleton at rest puts it -- which is not always
                // where its vertices are (a mesh under a turned or scaled armature).
                Some((s, joints, weights)) => {
                    let mats = skeleton.joint_matrices(s, &world);
                    let (p, nn) = skinned(&local.positions, &local.normals, &joints, &weights, &mats);
                    let t = local.targets.iter().map(|moves| skinned_moves(moves, &joints, &weights, &mats)).collect();
                    let moves = if skeleton.clips.is_empty() && skeleton.bodies.is_empty() { Moves::Still } else { Moves::Skin { skin: s, local, joints, weights } };
                    (p, nn, t, moves)
                }
                None => {
                    let p = local.positions.iter().map(|v| m.transform_point3(Vec3::from(*v)).to_array()).collect();
                    let nn = local.normals.iter().map(|v| (normal_m * Vec3::from(*v)).normalize_or_zero().to_array()).collect();
                    let t = local.targets.iter().map(|moves| moves.iter().map(|v| (Mat3::from_mat4(m) * Vec3::from(*v)).to_array()).collect()).collect();
                    let moves = if moved[n] { Moves::Node { node: n, local } } else { Moves::Still };
                    (p, nn, t, moves)
                }
            };
            parts.push(Part {
                positions: rest_positions,
                normals: rest_normals,
                uvs: uvs.chunks_exact(2).map(|v| [v[0], v[1]]).collect(),
                colors,
                indices,
                material: primitive.get("material").and_then(Value::as_u64).unwrap_or(0) as usize,
                targets: rest_targets,
                mesh,
                moves,
                option: options[n].clone(),
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
                    let name = m.get("name").and_then(Value::as_str).unwrap_or_default().trim().to_lowercase();
                    Material { name, color, texture, cutoff }
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

    let mut expressions = expressions(&root);
    // A plain GLB's morph targets, by the names its exporter gave them (Blender's shape keys):
    // `aa`, `blink`, `smile`, whatever the figure's maker named. A VRM's own come first.
    for (mesh, m) in meshes.iter().enumerate() {
        for (index, name) in m.pointer("/extras/targetNames").and_then(Value::as_array).into_iter().flatten().enumerate() {
            let Some(name) = name.as_str().filter(|n| !n.trim().is_empty()) else { continue };
            if !vrm || !expressions.contains_key(name) {
                expressions.entry(name.to_string()).or_default().push((mesh, index, 1.0));
            }
        }
    }
    let mut low = Vec3::splat(f32::MAX);
    let mut high = Vec3::splat(f32::MIN);
    for p in &parts {
        for v in &p.positions {
            low = low.min(Vec3::from(*v));
            high = high.max(Vec3::from(*v));
        }
    }
    Ok(Figure { parts, materials, pictures, expressions, low, high, skeleton, groups })
}

/// A morph target's moves, turned as each vertex's joints turn it at rest.
fn skinned_moves(moves: &[[f32; 3]], joints: &[[u16; 4]], weights: &[[f32; 4]], mats: &[Mat4]) -> Vec<[f32; 3]> {
    moves
        .iter()
        .enumerate()
        .map(|(v, d)| {
            let mut m = Mat3::ZERO;
            if let (Some(j), Some(w)) = (joints.get(v), weights.get(v)) {
                for k in 0..4 {
                    if let Some(joint) = mats.get(j[k] as usize) {
                        m += Mat3::from_mat4(*joint) * w[k];
                    }
                }
            }
            (m * Vec3::from(*d)).to_array()
        })
        .collect()
}

/// The file's skins: each one's joints, and the matrices that take its mesh into each joint's
/// frame.
fn skins(root: &Value, gltf: &Gltf) -> Result<Vec<Skin>, String> {
    let mut out = Vec::new();
    for skin in root.get("skins").and_then(Value::as_array).into_iter().flatten() {
        let joints: Vec<usize> = skin.get("joints").and_then(Value::as_array).into_iter().flatten().filter_map(|j| j.as_u64().map(|j| j as usize)).collect();
        let mut inverse_bind: Vec<Mat4> = match skin.get("inverseBindMatrices").and_then(Value::as_u64) {
            Some(a) => gltf.floats(a as usize, 16)?.chunks_exact(16).map(Mat4::from_cols_slice).collect(),
            None => Vec::new(),
        };
        inverse_bind.resize(joints.len(), Mat4::IDENTITY);
        out.push(Skin { joints, inverse_bind });
    }
    Ok(out)
}

/// The file's animations, each by its name; a track that cannot be read is left out, and an
/// animation with none is no animation.
fn clips(root: &Value, gltf: &Gltf, nodes: usize) -> Vec<Clip> {
    let mut out = Vec::new();
    for (k, animation) in root.get("animations").and_then(Value::as_array).into_iter().flatten().enumerate() {
        let samplers = animation.get("samplers").and_then(Value::as_array).cloned().unwrap_or_default();
        let mut tracks = Vec::new();
        for channel in animation.get("channels").and_then(Value::as_array).into_iter().flatten() {
            let Some(node) = channel.pointer("/target/node").and_then(Value::as_u64).map(|n| n as usize).filter(|n| *n < nodes) else { continue };
            let kind = match channel.pointer("/target/path").and_then(Value::as_str) {
                Some("translation") => Channel::Translation,
                Some("rotation") => Channel::Rotation,
                Some("scale") => Channel::Scale,
                // Morph weights and anything else are not played.
                _ => continue,
            };
            let Some(sampler) = channel.get("sampler").and_then(Value::as_u64).and_then(|s| samplers.get(s as usize)) else { continue };
            let interpolation = match sampler.get("interpolation").and_then(Value::as_str) {
                Some("STEP") => Interpolation::Step,
                Some("CUBICSPLINE") => Interpolation::CubicSpline,
                _ => Interpolation::Linear,
            };
            let (Some(input), Some(output)) = (sampler.get("input").and_then(Value::as_u64), sampler.get("output").and_then(Value::as_u64)) else { continue };
            let (Ok(times), Ok(values)) = (gltf.floats(input as usize, 1), gltf.floats(output as usize, kind.width())) else { continue };
            let per_key = kind.width() * if interpolation == Interpolation::CubicSpline { 3 } else { 1 };
            if times.is_empty() || values.len() < times.len() * per_key || times.windows(2).any(|w| w[1] < w[0]) {
                continue;
            }
            tracks.push(Track { node, channel: kind, interpolation, times, values });
        }
        if tracks.is_empty() {
            continue;
        }
        let duration = tracks.iter().filter_map(|t| t.times.last().copied()).fold(0.0f32, f32::max);
        let name = animation.get("name").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("animation {k}"));
        out.push(Clip { name, duration, tracks });
    }
    out
}

/// A node's own place: its matrix, or its translation, rotation and scale.
fn local(node: &Value) -> Trs {
    let floats = |key: &str| node.get(key).and_then(Value::as_array).map(|a| a.iter().map(|v| v.as_f64().unwrap_or(0.0) as f32).collect::<Vec<f32>>());
    if let Some(m) = floats("matrix").filter(|m| m.len() == 16) {
        let (s, r, t) = Mat4::from_cols_slice(&m).to_scale_rotation_translation();
        return Trs { t, r: r.normalize(), s };
    }
    let t = floats("translation").filter(|t| t.len() == 3).map(|t| Vec3::new(t[0], t[1], t[2])).unwrap_or(Vec3::ZERO);
    let r = floats("rotation").filter(|r| r.len() == 4).map(|r| Quat::from_xyzw(r[0], r[1], r[2], r[3]).normalize()).unwrap_or(Quat::IDENTITY);
    let s = floats("scale").filter(|s| s.len() == 3).map(|s| Vec3::new(s[0], s[1], s[2])).unwrap_or(Vec3::ONE);
    Trs { t, r, s }
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

    /// An accessor's values as floats, `width` to an element: its buffer view's, or zeroes when
    /// it has none, with its sparse part laid over them -- which is how Blender writes a shape
    /// key, only the vertices it moves (glTF's `sparse`; before 2026-11-19 it was read as zeroes,
    /// and a figure's mouth never moved).
    fn floats(&self, accessor: usize, width: usize) -> Result<Vec<f32>, String> {
        let a = self.root.pointer(&format!("/accessors/{accessor}")).ok_or("an accessor that is not there")?;
        let count = a.get("count").and_then(Value::as_u64).unwrap_or(0) as usize;
        let kind = a.get("componentType").and_then(Value::as_u64).unwrap_or(5126);
        let normalized = a.get("normalized").and_then(Value::as_bool).unwrap_or(false);
        let size = component_size(kind);
        let mut out = match a.get("bufferView").and_then(Value::as_u64) {
            Some(view) => {
                let (bytes, stride) = self.view(view as usize)?;
                let offset = a.get("byteOffset").and_then(Value::as_u64).unwrap_or(0) as usize;
                let stride = if stride == 0 { size * width } else { stride };
                let mut out = Vec::with_capacity(count * width);
                for i in 0..count {
                    for c in 0..width {
                        let at = offset + i * stride + c * size;
                        out.push(component(kind, normalized, bytes.get(at..at + size).ok_or("an accessor runs past its data")?));
                    }
                }
                out
            }
            None => vec![0.0; count * width],
        };
        if let Some(sparse) = a.get("sparse") {
            let n = sparse.get("count").and_then(Value::as_u64).unwrap_or(0) as usize;
            let part = |key: &str| -> Result<(&[u8], usize), String> {
                let view = sparse.pointer(&format!("/{key}/bufferView")).and_then(Value::as_u64).ok_or("a sparse accessor with no data")? as usize;
                let (bytes, _) = self.view(view)?;
                Ok((bytes, sparse.pointer(&format!("/{key}/byteOffset")).and_then(Value::as_u64).unwrap_or(0) as usize))
            };
            let (index_bytes, index_offset) = part("indices")?;
            let (value_bytes, value_offset) = part("values")?;
            let index_kind = sparse.pointer("/indices/componentType").and_then(Value::as_u64).unwrap_or(5125);
            let index_size = component_size(index_kind);
            for k in 0..n {
                let at = index_offset + k * index_size;
                let i = component(index_kind, false, index_bytes.get(at..at + index_size).ok_or("sparse indices run past their data")?) as usize;
                if i >= count {
                    return Err("a sparse index past the accessor's end".into());
                }
                for c in 0..width {
                    let at = value_offset + (k * width + c) * size;
                    out[i * width + c] = component(kind, normalized, value_bytes.get(at..at + size).ok_or("sparse values run past their data")?);
                }
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

/// How many bytes a glTF component type takes.
fn component_size(kind: u64) -> usize {
    match kind {
        5126 | 5125 => 4,
        5123 | 5122 => 2,
        _ => 1,
    }
}

/// One component, as glTF stores it, as a float; a normalised integer as its fraction.
fn component(kind: u64, normalized: bool, b: &[u8]) -> f32 {
    match kind {
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

    /// A GLB of a description and its data.
    fn glb(json: &str, bin: &[u8]) -> Vec<u8> {
        let mut json = json.as_bytes().to_vec();
        while json.len() % 4 != 0 {
            json.push(b' ');
        }
        let mut bin = bin.to_vec();
        while bin.len() % 4 != 0 {
            bin.push(0);
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

    /// An arm of two bones, made here: a strip of three vertices up glTF's +Y, the lowest on the
    /// first bone, the other two on the second, which `wave` turns a quarter about +Z in a
    /// second. Under `outfit=dress`, skinned; the same mesh again, still, as `outfit=jeans` and
    /// `hair=bob.001`.
    fn arm() -> Vec<u8> {
        let mut bin: Vec<u8> = Vec::new();
        let f32s = |bin: &mut Vec<u8>, v: &[f32]| bin.extend(v.iter().flat_map(|f| f.to_le_bytes()));
        f32s(&mut bin, &[0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 2.0, 0.0]); // 0: positions, 36
        bin.extend([0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0]); // 36: joints, 12
        f32s(&mut bin, &[1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0]); // 48: weights, 48
        bin.extend([0, 0, 1, 0, 2, 0, 0, 0]); // 96: indices, 6 (+2)
        let mut ibm = Mat4::IDENTITY.to_cols_array().to_vec();
        ibm.extend(Mat4::from_translation(Vec3::new(0.0, -1.0, 0.0)).to_cols_array());
        f32s(&mut bin, &ibm); // 104: inverse binds, 128
        f32s(&mut bin, &[0.0, 1.0]); // 232: times, 8
        let half = std::f32::consts::FRAC_1_SQRT_2;
        f32s(&mut bin, &[0.0, 0.0, 0.0, 1.0, 0.0, 0.0, half, half]); // 240: rotations, 32
        let views = [(0, 36), (36, 12), (48, 48), (96, 6), (104, 128), (232, 8), (240, 32)]
            .iter()
            .map(|(o, l)| format!(r#"{{"buffer":0,"byteOffset":{o},"byteLength":{l}}}"#))
            .collect::<Vec<_>>()
            .join(",");
        let json = format!(
            r#"{{"asset":{{"version":"2.0"}},"scene":0,"scenes":[{{"nodes":[0,3,4,5]}}],
            "nodes":[{{"name":"root","children":[1]}},{{"name":"bone0","children":[2]}},{{"name":"bone1","translation":[0,1,0]}},
              {{"name":"outfit=dress","mesh":0,"skin":0}},{{"name":"outfit=jeans","mesh":0}},{{"name":"hair=bob.001","mesh":0}}],
            "skins":[{{"joints":[1,2],"inverseBindMatrices":4}}],
            "meshes":[{{"primitives":[{{"attributes":{{"POSITION":0,"JOINTS_0":1,"WEIGHTS_0":2}},"indices":3}}]}}],
            "animations":[{{"name":"Armature|wave","channels":[{{"sampler":0,"target":{{"node":2,"path":"rotation"}}}}],"samplers":[{{"input":5,"output":6}}]}}],
            "buffers":[{{"byteLength":{}}}],"bufferViews":[{views}],
            "accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"}},{{"bufferView":1,"componentType":5121,"count":3,"type":"VEC4"}},
              {{"bufferView":2,"componentType":5126,"count":3,"type":"VEC4"}},{{"bufferView":3,"componentType":5123,"count":3,"type":"SCALAR"}},
              {{"bufferView":4,"componentType":5126,"count":2,"type":"MAT4"}},{{"bufferView":5,"componentType":5126,"count":2,"type":"SCALAR"}},
              {{"bufferView":6,"componentType":5126,"count":2,"type":"VEC4"}}]}}"#,
            bin.len()
        );
        glb(&json, &bin)
    }

    fn near(a: [f32; 3], b: [f32; 3]) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() < 1e-4)
    }

    #[test]
    fn a_skinned_figure_is_posed_by_its_animation() {
        let f = parse(&arm()).unwrap();
        let dress = f.parts.iter().find(|p| p.option == Some(("outfit".into(), "dress".into()))).unwrap();
        // At rest, where its skeleton at rest puts it: glTF's (0, 2, 0) is the room's 2 up.
        assert!(near(dress.positions[2], [0.0, 0.0, 2.0]));
        assert!(matches!(dress.moves, Moves::Skin { .. }));
        let wave = f.skeleton.clip("wave").unwrap();
        let pose_at = |time: f32| {
            let mut local = f.skeleton.rest();
            f.skeleton.sample(wave, time, &mut local);
            let world = f.skeleton.world(&local);
            f.skeleton.place(&dress.moves, &world, &[]).unwrap().0
        };
        // A quarter turn about glTF's +Z at the elbow, 1 up: the hand, 1 above it, goes to glTF's
        // -X, which is the room's -Y; the shoulder's vertex does not move.
        let end = pose_at(1.0);
        assert!(near(end[0], [0.0, 0.0, 0.0]), "{:?}", end[0]);
        assert!(near(end[2], [0.0, -1.0, 1.0]), "{:?}", end[2]);
        // Half way, an eighth of a turn; held at the end after it.
        let half = std::f32::consts::FRAC_1_SQRT_2;
        assert!(near(pose_at(0.5)[2], [0.0, -half, 1.0 + half]), "{:?}", pose_at(0.5)[2]);
        assert!(near(pose_at(5.0)[2], [0.0, -1.0, 1.0]));
        assert!((f.skeleton.clips[wave].duration - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_part_with_no_skin_under_no_animated_node_stands_still() {
        let f = parse(&arm()).unwrap();
        let jeans = f.parts.iter().find(|p| p.option == Some(("outfit".into(), "jeans".into()))).unwrap();
        assert!(matches!(jeans.moves, Moves::Still));
    }

    #[test]
    fn options_are_named_group_equals_option() {
        let f = parse(&arm()).unwrap();
        assert_eq!(f.groups, vec![("outfit".to_string(), vec!["dress".to_string(), "jeans".to_string()]), ("hair".to_string(), vec!["bob".to_string()])]);
        assert_eq!(option_of("Hair=Bob.001"), Some(("hair".into(), "bob".into())));
        // Blender's numbering only: a dot inside the name stays, and a colon is not one.
        assert_eq!(option_of("outfit=v1.5"), Some(("outfit".into(), "v1.5".into())));
        assert_eq!(option_of("mixamorig:Hips"), None);
        assert_eq!(option_of("=dress"), None);
    }

    #[test]
    fn a_clip_is_found_by_its_name_in_any_case() {
        let f = parse(&arm()).unwrap();
        assert_eq!(f.skeleton.clip("WAVE"), Some(0));
        assert_eq!(f.skeleton.clip("armature|wave"), Some(0));
        assert_eq!(f.skeleton.clip("dance"), None);
        assert_eq!(f.skeleton.clip(""), None);
    }

    #[test]
    fn steps_and_cubic_splines_are_sampled_as_gltf_has_them() {
        let mut place = Trs::default();
        let step = Track { node: 0, channel: Channel::Translation, interpolation: Interpolation::Step, times: vec![0.0, 1.0], values: vec![0.0, 0.0, 0.0, 1.0, 1.0, 1.0] };
        step.sample(0.9, &mut place);
        assert_eq!(place.t, Vec3::ZERO);
        step.sample(1.0, &mut place);
        assert_eq!(place.t, Vec3::ONE);
        // A cubic spline with flat tangents eases: half way is half way, a quarter is less.
        let cubic = Track {
            node: 0,
            channel: Channel::Translation,
            interpolation: Interpolation::CubicSpline,
            times: vec![0.0, 1.0],
            values: vec![0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 2.0, 0.0, 0.0, 0.0, 0.0, 0.0],
        };
        cubic.sample(0.5, &mut place);
        assert!((place.t.x - 1.0).abs() < 1e-5);
        cubic.sample(0.25, &mut place);
        assert!((place.t.x - 2.0 * (3.0 * 0.0625 - 2.0 * 0.015625)).abs() < 1e-5);
    }

    #[test]
    fn a_plain_glbs_morph_targets_are_expressions_by_their_names() {
        let mut bin: Vec<u8> = Vec::new();
        bin.extend([0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0].iter().flat_map(|f| f.to_le_bytes()));
        bin.extend([0.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.5, 0.0].iter().flat_map(|f| f.to_le_bytes()));
        let json = format!(
            r#"{{"asset":{{"version":"2.0"}},"nodes":[{{"mesh":0}}],"meshes":[{{"extras":{{"targetNames":["smirk"]}},"primitives":[{{"attributes":{{"POSITION":0}},"targets":[{{"POSITION":1}}]}}]}}],
            "buffers":[{{"byteLength":{}}}],"bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":36}},{{"buffer":0,"byteOffset":36,"byteLength":36}}],
            "accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"}},{{"bufferView":1,"componentType":5126,"count":3,"type":"VEC3"}}]}}"#,
            bin.len()
        );
        let f = parse(&glb(&json, &bin)).unwrap();
        assert_eq!(f.expressions.get("smirk"), Some(&vec![(0, 0, 1.0)]));
    }

    #[test]
    fn a_sparse_morph_target_moves_only_the_vertices_it_names() {
        // Three vertices; the target, sparse with no data of its own, moves only the third.
        let mut bin: Vec<u8> = Vec::new();
        bin.extend([0.0f32, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0].iter().flat_map(|f| f.to_le_bytes()));
        bin.extend([2u8, 0, 0, 0]); // 36: the one index (u16), and padding
        bin.extend([0.0f32, 0.5, 0.0].iter().flat_map(|f| f.to_le_bytes())); // 40: its move
        let json = format!(
            r#"{{"asset":{{"version":"2.0"}},"nodes":[{{"mesh":0}}],"meshes":[{{"extras":{{"targetNames":["aa"]}},"primitives":[{{"attributes":{{"POSITION":0}},"targets":[{{"POSITION":1}}]}}]}}],
            "buffers":[{{"byteLength":{}}}],"bufferViews":[{{"buffer":0,"byteOffset":0,"byteLength":36}},{{"buffer":0,"byteOffset":36,"byteLength":2}},{{"buffer":0,"byteOffset":40,"byteLength":12}}],
            "accessors":[{{"bufferView":0,"componentType":5126,"count":3,"type":"VEC3"}},
              {{"componentType":5126,"count":3,"type":"VEC3","sparse":{{"count":1,"indices":{{"bufferView":1,"componentType":5123}},"values":{{"bufferView":2}}}}}}]}}"#,
            bin.len()
        );
        let f = parse(&glb(&json, &bin)).unwrap();
        // glTF's +Y half a metre is the room's +Z.
        assert_eq!(f.parts[0].targets[0], vec![[0.0, 0.0, 0.0], [0.0, 0.0, 0.0], [0.0, 0.0, 0.5]]);
    }

    #[test]
    fn turning_the_short_way_round() {
        let a = Trs::default();
        // The same quarter turn written the other way: halfway is an eighth, not seven eighths.
        let b = Trs { r: -Quat::from_rotation_z(std::f32::consts::FRAC_PI_2), ..Trs::default() };
        let mid = a.towards(&b, 0.5);
        assert!((mid.r.angle_between(Quat::from_rotation_z(std::f32::consts::FRAC_PI_4))).abs() < 1e-4);
    }
}
