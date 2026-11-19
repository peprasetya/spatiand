//! What an assistant's figure offers, read from its file's description (a GLB's or a VRM's JSON,
//! none of its data): its **groups of options** -- a node named `group=option` (`outfit=dress`,
//! `hair=bob`) is one of that group's choices, with everything under it -- its **animations** by
//! name, its **expressions** (VRM's, or a plain GLB's morph targets by their own names), its
//! **materials** by name, and its **body**, for poses of its own making: which of its skeleton's
//! joints are its head, its arms and its legs, and the words a pose names them by
//! (`docs/spatipal-pals.md`).
//!
//! Here because two programs must read a figure the same way: SpatiPal tells its model what it
//! can choose from, and the room draws what was chosen (`crates/spatiand/src/figure.rs`). The
//! rule for a node's name, which option of a group is its first -- shown until another is
//! chosen -- which joint is which, and what a pose's words mean, are this file's, for both.

use std::collections::HashSet;
use std::io::Read;
use std::path::Path;

use glam::{Mat4, Quat, Vec3};
use serde_json::Value;

/// What a figure can be asked to wear, do and look like.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Offers {
    /// Each group with its options, in small letters, as the scene first has them: the first is
    /// the one shown until another is chosen.
    pub groups: Vec<(String, Vec<String>)>,
    pub animations: Vec<String>,
    pub expressions: Vec<String>,
    /// Its materials' names, in small letters.
    pub materials: Vec<String>,
    /// The parts of its body a pose of its own can turn ([`BODY_PARTS`]): none for a figure
    /// without a person's skeleton.
    pub body: Vec<String>,
}

/// A node's group and option, from its name, in small letters: `outfit=dress` is the group
/// `outfit`'s option `dress`. Blender's way of keeping names apart (`outfit=dress.001`, always
/// three digits) is the same option; `mixamorig:Hips` and `Body` are no option at all.
pub fn option_of(name: &str) -> Option<(String, String)> {
    let (group, option) = name.split_once('=')?;
    let option = match option.rsplit_once('.') {
        Some((before, digits)) if !before.is_empty() && digits.len() == 3 && digits.bytes().all(|b| b.is_ascii_digit()) => before,
        _ => option,
    };
    let (group, option) = (group.trim().to_lowercase(), option.trim().to_lowercase());
    (!group.is_empty() && !option.is_empty()).then_some((group, option))
}

/// The scene's nodes, parents before children, each once: the order a figure is posed and drawn
/// in. A file with no scene has its nodes that are no one's child as its scene.
pub fn scene_order(root: &Value) -> Vec<usize> {
    let nodes = root.get("nodes").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let children = |i: usize| -> Vec<usize> {
        nodes[i].get("children").and_then(Value::as_array).into_iter().flatten().filter_map(|c| c.as_u64().map(|c| c as usize)).filter(|c| *c < nodes.len()).collect()
    };
    let scene = root.get("scene").and_then(Value::as_u64).unwrap_or(0) as usize;
    let tops: Vec<usize> = match root.pointer(&format!("/scenes/{scene}/nodes")).and_then(Value::as_array) {
        Some(tops) => tops.iter().filter_map(|v| v.as_u64().map(|i| i as usize)).collect(),
        None => {
            let mut child = vec![false; nodes.len()];
            for i in 0..nodes.len() {
                for c in children(i) {
                    child[c] = true;
                }
            }
            (0..nodes.len()).filter(|i| !child[*i]).collect()
        }
    };
    let mut order = Vec::new();
    let mut seen = vec![false; nodes.len()];
    let mut stack: Vec<usize> = tops.into_iter().rev().collect();
    while let Some(i) = stack.pop() {
        if i >= nodes.len() || seen[i] {
            continue;
        }
        seen[i] = true;
        order.push(i);
        stack.extend(children(i).into_iter().rev());
    }
    order
}

/// The figure's groups of options, each with its options in scene order.
pub fn groups(root: &Value) -> Vec<(String, Vec<String>)> {
    let mut groups: Vec<(String, Vec<String>)> = Vec::new();
    for i in scene_order(root) {
        let Some((group, option)) = root.pointer(&format!("/nodes/{i}/name")).and_then(Value::as_str).and_then(option_of) else { continue };
        match groups.iter_mut().find(|(g, _)| *g == group) {
            Some((_, all)) if !all.contains(&option) => all.push(option),
            Some(_) => {}
            None => groups.push((group, vec![option])),
        }
    }
    groups
}

/// Everything a figure's description offers.
pub fn offers_of(root: &Value) -> Offers {
    let names = |list: &str| -> Vec<String> { root.get(list).and_then(Value::as_array).into_iter().flatten().filter_map(|a| a.get("name").and_then(Value::as_str)).map(str::to_string).collect() };
    let animations = root
        .get("animations")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(k, a)| a.get("name").and_then(Value::as_str).map(str::to_string).unwrap_or_else(|| format!("animation {k}")))
        .collect();
    let mut expressions: Vec<String> = Vec::new();
    let mut add = |name: &str| {
        if !name.trim().is_empty() && !expressions.iter().any(|e| e == name) {
            expressions.push(name.to_string());
        }
    };
    // VRM 1.0's, then 0.x's under 1.0's names (as the room reads them), then a plain GLB's targets.
    for kind in ["preset", "custom"] {
        if let Some(all) = root.pointer(&format!("/extensions/VRMC_vrm/expressions/{kind}")).and_then(Value::as_object) {
            all.keys().for_each(|k| add(k));
        }
    }
    for group in root.pointer("/extensions/VRM/blendShapeMaster/blendShapeGroups").and_then(Value::as_array).into_iter().flatten() {
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
            "" | "unknown" => group.get("name").and_then(Value::as_str).unwrap_or_default(),
            other => other,
        };
        add(name);
    }
    for mesh in root.get("meshes").and_then(Value::as_array).into_iter().flatten() {
        for name in mesh.pointer("/extras/targetNames").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
            add(name);
        }
    }
    let mut materials: Vec<String> = Vec::new();
    for m in names("materials").into_iter().map(|m| material_name(&m).to_lowercase()).filter(|m| !m.is_empty()) {
        if !materials.contains(&m) {
            materials.push(m);
        }
    }
    let body = bodies(root).first().map(|b| b.parts().into_iter().map(str::to_string).collect()).unwrap_or_default();
    Offers { groups: groups(root), animations, expressions, materials, body }
}

/// A material's name as its maker gave it: Blender names the second of a name "hair.001", the
/// third "hair.002" -- a doll's three hairstyles, each its own picture, are all its hair, given
/// one colour by that name.
pub fn material_name(name: &str) -> &str {
    let name = name.trim();
    match name.rsplit_once('.') {
        Some((base, n)) if !base.is_empty() && n.len() == 3 && n.bytes().all(|b| b.is_ascii_digit()) => base,
        _ => name,
    }
}

/// The parts of a body a pose names, as a figure's model names them: its left its own.
pub const BODY_PARTS: &[&str] = &[
    "head",
    "chest",
    "left_shoulder",
    "right_shoulder",
    "left_upper_arm",
    "right_upper_arm",
    "left_forearm",
    "right_forearm",
    "left_hand",
    "right_hand",
    "left_thigh",
    "right_thigh",
    "left_shin",
    "right_shin",
];

/// An arm or a leg, by the nodes that turn it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Limb {
    /// The collarbone before an arm, when the skeleton has one; none for a leg.
    pub root: Option<usize>,
    /// The shoulder or the hip: turns the upper arm or the thigh.
    pub upper: usize,
    /// The elbow or the knee: turns the forearm or the shin.
    pub lower: usize,
    /// The wrist or the ankle: turns the hand or the foot.
    pub end: usize,
    /// A node beyond it -- a finger, a toe -- that says which way the hand or the foot points.
    pub tip: Option<usize>,
}

/// A figure's skeleton as a body: which of its nodes a pose turns for each of its parts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Body {
    pub hips: usize,
    /// Above the hips, up to where the arms join: bent together, to lean.
    pub spine: Vec<usize>,
    /// Above that, up to the head, which is the last: turned together, to look.
    pub neck: Vec<usize>,
    /// Its own left first.
    pub arms: [Limb; 2],
    pub legs: [Limb; 2],
}

impl Body {
    /// The parts of [`BODY_PARTS`] a pose can turn on it.
    pub fn parts(&self) -> Vec<&'static str> {
        BODY_PARTS
            .iter()
            .copied()
            .filter(|p| match *p {
                "chest" => !self.spine.is_empty(),
                "head" => !self.neck.is_empty(),
                "left_shoulder" => self.arms[0].root.is_some(),
                "right_shoulder" => self.arms[1].root.is_some(),
                _ => true,
            })
            .collect()
    }
}

/// Which way a figure faces in its file: `1.0` for glTF's +Z (and VRM 1.0's), `-1.0` for VRM
/// 0.x's -Z. Its own left is +X times this, its front +Z times it.
pub fn facing(root: &Value) -> f32 {
    if root.pointer("/extensions/VRM").is_some() {
        -1.0
    } else {
        1.0
    }
}

/// The bodies a pose can move: one for each of the figure's skeletons that is a person's (skins
/// that share their joints share one) -- a VRM's humanoid as it names its bones, any other found
/// by its shape at rest, whatever its joints are called (`shaped`). None for a figure with no
/// such skeleton: a fox, a robot's arm.
pub fn bodies(root: &Value) -> Vec<Body> {
    let (parent, at) = rest_places(root);
    if let Some(body) = vrm_body(root, at.len()) {
        return vec![body];
    }
    let names: Vec<String> = root.get("nodes").and_then(Value::as_array).into_iter().flatten().map(|n| n.get("name").and_then(Value::as_str).unwrap_or_default().to_lowercase()).collect();
    if let Some(body) = sl_body(&names, &parent) {
        return vec![body];
    }
    let mut out: Vec<Body> = Vec::new();
    for skin in root.get("skins").and_then(Value::as_array).into_iter().flatten() {
        let joints: Vec<usize> = skin.get("joints").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_u64).map(|j| j as usize).filter(|j| *j < at.len()).collect();
        if let Some(body) = shaped(&joints, &parent, &at, &names) {
            if !out.iter().any(|b| b.hips == body.hips) {
                out.push(body);
            }
        }
    }
    out
}

/// A person's body in any skeleton, by its shape at rest as a figure's is found (`shaped`): each
/// joint's parent, its place at rest -- +X its own left, +Y up, +Z its front -- and its name in
/// small letters. A BVH's, when its names say nothing (`crate::bvh`).
pub fn body_of(parent: &[Option<usize>], at: &[Vec3], names: &[String]) -> Option<Body> {
    let all: Vec<usize> = (0..at.len().min(parent.len()).min(names.len())).collect();
    shaped(&all, parent, at, names)
}

/// A node's own place, as its description has it: a matrix, or a translation, rotation and scale.
fn node_matrix(node: &Value) -> Mat4 {
    let floats = |key: &str| node.get(key).and_then(Value::as_array).map(|a| a.iter().map(|v| v.as_f64().unwrap_or(0.0) as f32).collect::<Vec<f32>>());
    if let Some(m) = floats("matrix").filter(|m| m.len() == 16) {
        return Mat4::from_cols_slice(&m);
    }
    let t = floats("translation").filter(|t| t.len() == 3).map(|t| Vec3::new(t[0], t[1], t[2])).unwrap_or(Vec3::ZERO);
    let r = floats("rotation").filter(|r| r.len() == 4).map(|r| Quat::from_xyzw(r[0], r[1], r[2], r[3]).normalize()).unwrap_or(Quat::IDENTITY);
    let s = floats("scale").filter(|s| s.len() == 3).map(|s| Vec3::new(s[0], s[1], s[2])).unwrap_or(Vec3::ONE);
    Mat4::from_scale_rotation_translation(s, r, t)
}

/// Each node's parent, and where each node is at rest in the figure's own frame: x towards its
/// left, y up, z towards its front.
fn rest_places(root: &Value) -> (Vec<Option<usize>>, Vec<Vec3>) {
    let nodes = root.get("nodes").and_then(Value::as_array).map(Vec::as_slice).unwrap_or_default();
    let mut parent = vec![None; nodes.len()];
    for (i, node) in nodes.iter().enumerate() {
        for c in node.get("children").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_u64).map(|c| c as usize) {
            if c < nodes.len() && parent[c].is_none() && c != i {
                parent[c] = Some(i);
            }
        }
    }
    let mut world = vec![Mat4::IDENTITY; nodes.len()];
    for i in scene_order(root) {
        let own = node_matrix(&nodes[i]);
        world[i] = match parent[i] {
            Some(p) => world[p] * own,
            None => own,
        };
    }
    let k = facing(root);
    let places = world
        .iter()
        .map(|m| {
            let p = m.w_axis.truncate();
            Vec3::new(p.x * k, p.y, p.z * k)
        })
        .collect();
    (parent, places)
}

/// A VRM's humanoid, as it names its bones (VRM 1.0's `humanBones` by name, 0.x's as a list).
fn vrm_body(root: &Value, nodes: usize) -> Option<Body> {
    let bone = |name: &str| -> Option<usize> {
        let node = match root.pointer("/extensions/VRMC_vrm/humanoid/humanBones") {
            Some(bones) => bones.get(name)?.get("node")?.as_u64()?,
            None => root.pointer("/extensions/VRM/humanoid/humanBones")?.as_array()?.iter().find(|b| b.get("bone").and_then(Value::as_str) == Some(name))?.get("node")?.as_u64()?,
        } as usize;
        (node < nodes).then_some(node)
    };
    let hips = bone("hips")?;
    bone("head")?;
    let spine = ["spine", "chest", "upperChest"].iter().filter_map(|b| bone(b)).collect();
    let neck = ["neck", "head"].iter().filter_map(|b| bone(b)).collect();
    let arm = |side: &str| {
        Some(Limb {
            root: bone(&format!("{side}Shoulder")),
            upper: bone(&format!("{side}UpperArm"))?,
            lower: bone(&format!("{side}LowerArm"))?,
            end: bone(&format!("{side}Hand"))?,
            tip: bone(&format!("{side}MiddleProximal")).or_else(|| bone(&format!("{side}IndexProximal"))),
        })
    };
    let leg = |side: &str| Some(Limb { root: None, upper: bone(&format!("{side}UpperLeg"))?, lower: bone(&format!("{side}LowerLeg"))?, end: bone(&format!("{side}Foot"))?, tip: bone(&format!("{side}Toes")) });
    Some(Body { hips, spine, neck, arms: [arm("left")?, arm("right")?], legs: [leg("left")?, leg("right")?] })
}

/// Second Life's skeleton, by its bones' names (`mPelvis`, `mShoulderLeft`...): a fitted mesh's,
/// Ruth2's, Max's -- whose collision volumes (`L_UPPER_ARM`, `BELLY`), Bento's extra spine joints
/// and its fingers stand among the bones, where the shape alone would be read amiss.
fn sl_body(names: &[String], parent: &[Option<usize>]) -> Option<Body> {
    let bone = |name: &str| names.iter().position(|n| n == &name.to_lowercase());
    let hips = bone("mPelvis")?;
    let head = bone("mHead")?;
    let neck = bone("mNeck")?;
    // The spine: from the pelvis up to the neck, Bento's mSpine joints among mTorso and mChest.
    let mut up = Vec::new();
    let mut k = neck;
    while let Some(p) = parent[k] {
        if p == hips {
            break;
        }
        up.push(p);
        k = p;
        if up.len() > names.len() {
            return None;
        }
    }
    parent[k]?;
    up.reverse();
    let arm = |side: &str| {
        Some(Limb {
            root: bone(&format!("mCollar{side}")),
            upper: bone(&format!("mShoulder{side}"))?,
            lower: bone(&format!("mElbow{side}"))?,
            end: bone(&format!("mWrist{side}"))?,
            tip: bone(&format!("mHandMiddle1{side}")).or_else(|| bone(&format!("mHandIndex1{side}"))),
        })
    };
    let leg = |side: &str| Some(Limb { root: None, upper: bone(&format!("mHip{side}"))?, lower: bone(&format!("mKnee{side}"))?, end: bone(&format!("mAnkle{side}"))?, tip: bone(&format!("mFoot{side}")).or_else(|| bone(&format!("mToe{side}"))) });
    Some(Body { hips, spine: up, neck: vec![neck, head], arms: [arm("Left")?, arm("Right")?], legs: [leg("Left")?, leg("Right")?] })
}

/// A skin's joints found to be a person's by their shape at rest, whatever they are named: its
/// feet the lowest joints on either side, its hips where their legs meet, its spine the way up
/// from there, each time to the joint reaching highest, its arms the branches off that reaching
/// farthest out on either side. Of each limb, the upper and lower bones are its two longest in a
/// row -- an upper arm and a forearm, past a collarbone and before a hand; a thigh and a shin
/// before a foot. A head's end that only says where its top is (`HeadTop_End`) turns nothing.
fn shaped(joints: &[usize], parent: &[Option<usize>], at: &[Vec3], names: &[String]) -> Option<Body> {
    let set: HashSet<usize> = joints.iter().copied().collect();
    if set.len() < 13 {
        return None;
    }
    let low = joints.iter().map(|j| at[*j].y).fold(f32::MAX, f32::min);
    let high = joints.iter().map(|j| at[*j].y).fold(f32::MIN, f32::max);
    let tall = high - low;
    if tall.is_nan() || tall < 1e-4 {
        return None;
    }
    let kids = |j: usize| joints.iter().copied().filter(move |c| parent[*c] == Some(j));
    // A joint and those above it, to the top.
    let above = |j: usize| {
        let mut all = vec![j];
        let mut k = j;
        while let Some(p) = parent[k] {
            if all.contains(&p) {
                break;
            }
            all.push(p);
            k = p;
        }
        all
    };
    // The way down from `from` to `j`, `from` left out.
    let down_to = |from: usize, j: usize| -> Option<Vec<usize>> {
        let mut path = Vec::new();
        let mut k = j;
        while k != from {
            path.push(k);
            k = parent[k]?;
            if path.len() > joints.len() {
                return None;
            }
        }
        path.reverse();
        Some(path)
    };
    let under = |j: usize| -> Vec<usize> { joints.iter().copied().filter(|c| above(*c).contains(&j)).collect() };

    let foot = |side: f32| joints.iter().copied().filter(|j| at[*j].x * side > 0.03 * tall).min_by(|a, b| at[*a].y.total_cmp(&at[*b].y));
    let (foot_l, foot_r) = (foot(1.0)?, foot(-1.0)?);
    if at[foot_l].y > low + 0.25 * tall || at[foot_r].y > low + 0.25 * tall {
        return None;
    }
    let theirs = above(foot_r);
    let hips = above(foot_l).into_iter().find(|j| theirs.contains(j)).filter(|j| set.contains(j))?;
    let legs = [down_to(hips, foot_l)?, down_to(hips, foot_r)?];
    let on_legs: HashSet<usize> = legs.iter().flatten().copied().collect();

    let reach_up = |j: usize| under(j).iter().map(|c| at[*c].y).fold(f32::MIN, f32::max);
    let mut up = vec![hips];
    while let Some(next) = kids(up[up.len() - 1]).filter(|c| !on_legs.contains(c)).max_by(|a, b| reach_up(*a).total_cmp(&reach_up(*b))) {
        up.push(next);
        if up.len() > joints.len() {
            return None;
        }
    }
    let chain: HashSet<usize> = up.iter().copied().collect();
    // On either side, the branch off the spine that reaches farthest out: where it leaves the
    // spine, and the way out to its joint farthest from there.
    let arm = |side: f32| -> Option<(usize, Vec<usize>)> {
        let mut best: Option<(f32, usize, usize)> = None;
        for &u in &up {
            for c in kids(u).filter(|c| !chain.contains(c) && !on_legs.contains(c)) {
                let reach = under(c).iter().map(|j| at[*j].x * side).fold(f32::MIN, f32::max);
                if best.is_none_or(|(r, _, _)| reach > r) {
                    best = Some((reach, u, c));
                }
            }
        }
        let (reach, from, branch) = best?;
        if reach < 0.12 * tall {
            return None;
        }
        let far = under(branch).into_iter().max_by(|a, b| at[*a].distance(at[from]).total_cmp(&at[*b].distance(at[from])))?;
        Some((from, down_to(from, far)?))
    };
    let (from_l, arm_l) = arm(1.0)?;
    let (from_r, arm_r) = arm(-1.0)?;
    if arm_l.first() == arm_r.first() {
        return None;
    }
    let place = |j: usize| up.iter().position(|u| *u == j);
    let chest = place(from_l)?.max(place(from_r)?);
    let spine = up[1..=chest].to_vec();
    let mut neck = up[chest + 1..].to_vec();
    while neck.len() > 1 && neck.last().is_some_and(|j| is_end(&names[*j])) {
        neck.pop();
    }
    let arms = [limb(from_l, &arm_l, at, true)?, limb(from_r, &arm_r, at, true)?];
    let legs = [limb(hips, &legs[0], at, false)?, limb(hips, &legs[1], at, false)?];
    // As long as a person's bones are, for their height.
    let long = |a: usize, b: usize, least: f32| at[a].distance(at[b]) >= least * tall;
    if !arms.iter().all(|l| long(l.upper, l.lower, 0.06) && long(l.lower, l.end, 0.06)) || !legs.iter().all(|l| long(l.upper, l.lower, 0.12) && long(l.lower, l.end, 0.12)) {
        return None;
    }
    Some(Body { hips, spine, neck, arms, legs })
}

/// A node that only marks where a bone ends: `HeadTop_End`, `head_end`, `Bip01 HeadNub`.
fn is_end(name: &str) -> bool {
    name.ends_with("end") || name.contains("top") || name.contains("nub") || name.ends_with("tip")
}

/// A limb, from where it leaves the body (`from`) along its joints (`path`): its upper and lower
/// bones the two longest in a row -- of which the upper is the limb's own, not the body's.
fn limb(from: usize, path: &[usize], at: &[Vec3], arm: bool) -> Option<Limb> {
    // The bone of path[i - 1] runs from it to path[i]; the first, from `from`, is the body's.
    let bone = |i: usize| at[path[i]].distance(at[if i == 0 { from } else { path[i - 1] }]);
    let i = (1..path.len().saturating_sub(1)).max_by(|a, b| (bone(*a) + bone(*a + 1)).total_cmp(&(bone(*b) + bone(*b + 1))))?;
    Some(Limb { root: (arm && i >= 2).then(|| path[i - 2]), upper: path[i - 1], lower: path[i], end: path[i + 1], tip: path.get(i + 2).copied() })
}

/// Words a pose's words may have that say nothing of where a part points.
const FILLER: &[&str] = &[
    "a", "an", "the", "and", "to", "of", "your", "my", "her", "his", "its", "their", "with", "toward", "towards", "pointing", "point", "points", "pointed", "facing", "face", "faces",
    "then", "also", "very", "far", "fully", "all", "way", "bent", "bend", "bending", "is", "are", "at", "from", "by", "on", "over", "under", "more", "much", "just", "quite", "rather",
    "angle", "angled", "diagonally", "lean", "leaning", "leans", "look", "looking", "looks", "hand", "hands", "arm", "arms", "elbow", "elbows", "knee", "knees", "foot", "feet",
    "fingers", "palm", "palms", "head", "chest", "shoulder", "shoulders", "it", "them", "each", "both", "body", "you", "me", "held", "hold", "holding", "keep", "kept", "pose",
];

/// Where a part of a body is asked to point, from a model's words for it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Aim {
    /// Towards the figure's own left, up, and towards its front: 1 for a word, 0.45 for "a
    /// little", negative for the opposite, summed over the words and kept within -1 to 1.
    pub left: f32,
    pub up: f32,
    pub front: f32,
    /// A head tilted towards its own left shoulder (+) or its right (-).
    pub tilt: f32,
    /// A head or the chest turned towards its own left (+) or its right (-).
    pub turn: f32,
    /// A forearm or a shin straightened, in line with the bone above it.
    pub straight: bool,
}

/// What a part of a body is asked to do, from the words a model gave it -- `None` for a part
/// left to move as it does ("relaxed") -- or why they say nothing, in words the model can mend
/// them by. `part` is one of [`BODY_PARTS`]:
///
/// - a limb's bone points where its words say, from the figure's own point of view: `front`,
///   `back`, `up`, `down`, `left`, `right`, `out` (away from its middle), `in` (across it), any
///   of them `a little`; an upper arm where its elbow goes, a forearm where its hand goes (or
///   `straight`), a hand where its fingers go, a thigh where its knee goes, a shin where its foot
///   goes (or `straight`);
/// - the head looks `front`, `up`, `down`, `left`, `right`, and tilts (`tilt left`);
/// - the chest leans `front`, `back`, `left`, `right`, and turns (`turn right`);
/// - a shoulder goes `up`, `down`, `front`, `back`.
pub fn aim(part: &str, words: &str) -> Result<Option<Aim>, String> {
    let part = part.trim().to_lowercase();
    if !BODY_PARTS.contains(&part.as_str()) {
        return Err(format!("there is no part {part}: the parts are {}", BODY_PARTS.join(", ")));
    }
    let side = if part.starts_with("left_") {
        1.0
    } else if part.starts_with("right_") {
        -1.0
    } else {
        0.0
    };
    let central = side == 0.0;
    let shoulder = part.ends_with("_shoulder");
    let bends = part.ends_with("_forearm") || part.ends_with("_shin");
    let lower = words.to_lowercase();
    let mut aim = Aim::default();
    let (mut weight, mut about, mut said, mut strange) = (1.0f32, None::<&str>, false, Vec::new());
    for word in lower.split(|c: char| !c.is_alphabetic()).filter(|w| !w.is_empty()) {
        let w = weight;
        let (mut left, mut up, mut front) = (0.0, 0.0, 0.0);
        match word {
            "little" | "slightly" | "bit" | "somewhat" | "slight" | "lightly" | "gently" => {
                weight = 0.45;
                continue;
            }
            "tilt" | "tilted" | "tilting" | "tilts" | "cock" | "cocked" => {
                about = Some("tilt");
                continue;
            }
            "turn" | "turned" | "turning" | "turns" | "twist" | "twisted" | "twisting" => {
                about = Some("turn");
                continue;
            }
            "relaxed" | "relax" | "rest" | "resting" | "natural" | "normal" | "neutral" | "none" | "default" | "loose" | "free" => return Ok(None),
            "straight" | "straighten" | "straightened" | "extended" | "outstretched" | "upright" | "level" => {
                aim.straight |= bends;
                said = true;
                continue;
            }
            "left" | "right" => {
                let s = if word == "left" { w } else { -w };
                match about.take() {
                    Some("tilt") if part == "head" => aim.tilt += s,
                    Some("turn") if central => aim.turn += s,
                    _ => left = s,
                }
            }
            "front" | "forward" | "forwards" | "ahead" | "frontward" | "frontwards" => front = w,
            "back" | "backward" | "backwards" | "behind" | "rear" => front = -w,
            "up" | "upward" | "upwards" | "high" | "above" | "overhead" | "raised" | "raise" | "skyward" | "shrug" | "shrugged" | "shrugging" => up = w,
            "down" | "downward" | "downwards" | "low" | "below" | "lowered" | "floor" | "ground" => up = -w,
            "out" | "outward" | "outwards" | "away" | "side" | "sides" | "sideways" | "wide" | "outside" | "apart" | "spread" if !central => left = side * w,
            "in" | "inward" | "inwards" | "across" | "inside" | "together" | "middle" | "center" | "centre" | "crossed" if !central => left = -side * w,
            "hunch" | "hunched" if shoulder => front = w,
            _ => {
                if !FILLER.contains(&word) {
                    strange.push(word);
                }
                continue;
            }
        }
        weight = 1.0;
        aim.left += left;
        aim.up += up;
        aim.front += front;
        said = true;
    }
    // Words that say something else -- a model thinking aloud in its call -- are not a pose.
    if strange.len() > 3 {
        return Err(format!("{part}: \"{}\" is more than where it points -- give only ways: front, back, up, down, left, right, out, in, each with \"a little\" for only a little", words.trim()));
    }
    if !said {
        let ways = match part.as_str() {
            "head" => "front, up, down, left or right, and tilt left or right",
            "chest" => "front (to lean forward), back, left or right, and turn left or right",
            _ if shoulder => "up, down, front or back",
            _ if bends => "front, back, up, down, left, right, out (away from your middle), in (across it), or straight",
            _ => "front, back, up, down, left, right, out (away from your middle) or in (across it)",
        };
        return Err(format!("{part}: \"{}\" says no way to point -- say {ways}, each with \"a little\" for only a little", words.trim()));
    }
    let keep = |v: f32| v.clamp(-1.0, 1.0);
    let aim = Aim { left: keep(aim.left), up: keep(aim.up), front: keep(aim.front), tilt: keep(aim.tilt), turn: keep(aim.turn), straight: aim.straight };
    if !central && !shoulder && !aim.straight && aim.left.abs() + aim.up.abs() + aim.front.abs() < 1e-3 {
        return Err(format!("{part}: \"{}\" points nowhere: its ways cancel out", words.trim()));
    }
    Ok(Some(aim))
}

/// The JSON description of a GLB (or VRM), from its first chunk.
pub fn description(bytes: &[u8]) -> Result<Value, String> {
    let u32_at = |at: usize| bytes.get(at..at + 4).map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]));
    if bytes.get(0..4) != Some(b"glTF") {
        return Err("not a glTF binary (.glb, .vrm)".into());
    }
    let length = u32_at(12).ok_or("a broken chunk")? as usize;
    if u32_at(16) != Some(0x4E4F_534A) {
        return Err("the figure's first chunk is not its description".into());
    }
    let json = bytes.get(20..20 + length).ok_or("the description runs past the end of the file")?;
    serde_json::from_slice(json).map_err(|e| format!("the figure's description is not JSON: {e}"))
}

/// What the figure in a file offers, reading only as much of it as its description takes.
pub fn read(path: &Path) -> Result<Offers, String> {
    let mut file = std::fs::File::open(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut head = [0u8; 20];
    file.read_exact(&mut head).map_err(|e| format!("{}: {e}", path.display()))?;
    let length = u32::from_le_bytes([head[12], head[13], head[14], head[15]]) as usize;
    // A description is kilobytes; a figure of tens of megabytes is not read for it.
    if length > 64 << 20 {
        return Err(format!("{}: a description of {length} bytes is not a figure's", path.display()));
    }
    let mut bytes = head.to_vec();
    bytes.resize(20 + length, 0);
    file.read_exact(&mut bytes[20..]).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(offers_of(&description(&bytes)?))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn a_node_named_group_equals_option_is_one() {
        assert_eq!(option_of("Outfit=Dress"), Some(("outfit".into(), "dress".into())));
        assert_eq!(option_of("hair=bob.001"), Some(("hair".into(), "bob".into())));
        assert_eq!(option_of("outfit=v1.5"), Some(("outfit".into(), "v1.5".into())));
        assert_eq!(option_of("mixamorig:Hips"), None);
        assert_eq!(option_of("Body"), None);
        assert_eq!(option_of("=dress"), None);
        assert_eq!(option_of("outfit="), None);
    }

    #[test]
    fn options_come_in_scene_order_each_once() {
        let root = json!({
            "scene": 0,
            "scenes": [{"nodes": [3, 0]}],
            "nodes": [
                {"name": "Armature", "children": [1, 2]},
                {"name": "outfit=dress"},
                {"name": "outfit=casual"},
                {"name": "outfit=dress.001", "children": [4]},
                {"name": "hair=bob"},
                {"name": "outfit=unused"}
            ]
        });
        // Node 3 comes first in the scene; node 5 is in none.
        assert_eq!(groups(&root), vec![("outfit".to_string(), vec!["dress".to_string(), "casual".to_string()]), ("hair".to_string(), vec!["bob".to_string()])]);
    }

    #[test]
    fn what_a_figure_offers() {
        let root = json!({
            "nodes": [{"name": "look=jeans"}, {"name": "look=dress"}],
            "animations": [{"name": "idle"}, {"name": "wave"}, {}],
            "meshes": [{"extras": {"targetNames": ["aa", "smirk"]}}, {"extras": {"targetNames": ["aa"]}}],
            "materials": [{"name": "Hair"}, {"name": "skin"}, {}, {"name": "hair.001"}, {"name": "Dress.002"}]
        });
        let offers = offers_of(&root);
        assert_eq!(offers.groups, vec![("look".to_string(), vec!["jeans".to_string(), "dress".to_string()])]);
        assert_eq!(offers.animations, vec!["idle", "wave", "animation 2"]);
        assert_eq!(offers.expressions, vec!["aa", "smirk"]);
        // Blender's ".001" of a second material of a name is the same name.
        assert_eq!(offers.materials, vec!["hair", "skin", "dress"]);
        assert_eq!((material_name("hair.001"), material_name("Material.Skin"), material_name("v.1"), material_name(".001")), ("hair", "Material.Skin", "v.1", ".001"));
    }

    #[test]
    fn a_files_description_is_read_without_its_data() {
        let json = br#"{"asset":{"version":"2.0"},"nodes":[{"name":"outfit=dress"}]}  "#;
        let mut bytes = b"glTF".to_vec();
        bytes.extend(2u32.to_le_bytes());
        bytes.extend(((20 + json.len() + 8 + 4) as u32).to_le_bytes());
        bytes.extend((json.len() as u32).to_le_bytes());
        bytes.extend(0x4E4F_534Au32.to_le_bytes());
        bytes.extend(json);
        bytes.extend(4u32.to_le_bytes());
        bytes.extend(0x004E_4942u32.to_le_bytes());
        bytes.extend([0, 0, 0, 0]);
        let path = std::env::temp_dir().join(format!("spatiand-figure-offers-{}.glb", std::process::id()));
        std::fs::write(&path, &bytes).unwrap();
        let offers = read(&path).unwrap();
        let _ = std::fs::remove_file(&path);
        assert_eq!(offers.groups, vec![("outfit".to_string(), vec!["dress".to_string()])]);
        assert!(read(Path::new("/nonexistent/figure.glb")).is_err());
        assert!(description(b"not a figure at all, just words").is_err());
    }

    /// A made-up person's skeleton at rest, 1.6 m tall, arms down at 45 degrees: (name, parent,
    /// where it is in its own frame -- x its left, y up, z its front).
    pub(crate) fn person() -> Vec<(&'static str, Option<usize>, [f32; 3])> {
        let mut all: Vec<(&'static str, Option<usize>, [f32; 3])> = vec![
            ("hips", None, [0.0, 0.95, 0.0]),
            ("spine", Some(0), [0.0, 1.05, 0.0]),
            ("chest", Some(1), [0.0, 1.25, 0.0]),
            ("neck", Some(2), [0.0, 1.45, 0.0]),
            ("head", Some(3), [0.0, 1.55, 0.0]),
        ];
        for (side, names) in [(1.0, ["clavicle.L", "upper_arm.L", "forearm.L", "hand.L", "finger.L"]), (-1.0, ["clavicle.R", "upper_arm.R", "forearm.R", "hand.R", "finger.R"])] {
            let at = [[0.03, 1.40, 0.02], [0.17, 1.42, 0.0], [0.37, 1.22, -0.01], [0.55, 1.04, 0.0], [0.62, 0.97, 0.0]];
            let first = all.len();
            for (k, name) in names.iter().enumerate() {
                let parent = if k == 0 { 2 } else { first + k - 1 };
                all.push((name, Some(parent), [at[k][0] * side, at[k][1], at[k][2]]));
            }
        }
        for (side, names) in [(1.0, ["thigh.L", "shin.L", "foot.L", "toe.L"]), (-1.0, ["thigh.R", "shin.R", "foot.R", "toe.R"])] {
            let at = [[0.09, 0.90, 0.0], [0.10, 0.50, 0.01], [0.10, 0.08, -0.02], [0.10, 0.02, 0.12]];
            let first = all.len();
            for (k, name) in names.iter().enumerate() {
                let parent = if k == 0 { 0 } else { first + k - 1 };
                all.push((name, Some(parent), [at[k][0] * side, at[k][1], at[k][2]]));
            }
        }
        all
    }

    /// A file's description of a skeleton: its nodes' places made relative to their parents,
    /// the whole turned to face -Z when `facing` is -1 (as a VRM 0.x is).
    pub(crate) fn rig(joints: &[(&str, Option<usize>, [f32; 3])], facing: f32, extensions: Value) -> Value {
        let file = |p: [f32; 3]| [p[0] * facing, p[1], p[2] * facing];
        let nodes: Vec<Value> = joints
            .iter()
            .enumerate()
            .map(|(i, (name, parent, at))| {
                let (at, from) = (file(*at), parent.map(|p| file(joints[p].2)).unwrap_or([0.0; 3]));
                let children: Vec<usize> = joints.iter().enumerate().filter(|(_, j)| j.1 == Some(i)).map(|(k, _)| k).collect();
                let mut node = json!({"name": name, "translation": [at[0] - from[0], at[1] - from[1], at[2] - from[2]]});
                if !children.is_empty() {
                    node["children"] = json!(children);
                }
                node
            })
            .collect();
        json!({"scene": 0, "scenes": [{"nodes": [0]}], "nodes": nodes, "skins": [{"joints": (0..joints.len()).collect::<Vec<_>>()}], "extensions": extensions})
    }

    fn index(joints: &[(&str, Option<usize>, [f32; 3])], name: &str) -> usize {
        joints.iter().position(|j| j.0 == name).unwrap()
    }

    #[test]
    fn a_persons_skeleton_is_found_by_its_shape() {
        let joints = person();
        let i = |name: &str| index(&joints, name);
        let bodies = bodies(&rig(&joints, 1.0, json!({})));
        assert_eq!(bodies.len(), 1);
        let body = &bodies[0];
        assert_eq!(body.hips, i("hips"));
        assert_eq!(body.spine, vec![i("spine"), i("chest")]);
        assert_eq!(body.neck, vec![i("neck"), i("head")]);
        assert_eq!(body.arms[0], Limb { root: Some(i("clavicle.L")), upper: i("upper_arm.L"), lower: i("forearm.L"), end: i("hand.L"), tip: Some(i("finger.L")) });
        assert_eq!(body.arms[1].upper, i("upper_arm.R"));
        assert_eq!(body.legs[0], Limb { root: None, upper: i("thigh.L"), lower: i("shin.L"), end: i("foot.L"), tip: Some(i("toe.L")) });
        assert_eq!(body.legs[1].end, i("foot.R"));
        assert_eq!(body.parts(), BODY_PARTS.to_vec());
        // Its names say nothing: the same skeleton named otherwise is the same body.
        let renamed: Vec<(&str, Option<usize>, [f32; 3])> = joints.iter().map(|(_, p, at)| ("joint", *p, *at)).collect();
        assert_eq!(super::bodies(&rig(&renamed, 1.0, json!({})))[0], *body);
    }

    #[test]
    fn second_lifes_skeleton_is_found_by_its_bones_names() {
        // SL's bones (Bento's extra spine joints among them) and a fitted mesh's collision
        // volumes, which stand along the bones and would read as branches of their own.
        let mut j: Vec<(&str, Option<usize>, [f32; 3])> = vec![
            ("mPelvis", None, [0.0, 1.05, 0.0]),
            ("mSpine1", Some(0), [0.0, 1.05, 0.0]),
            ("mSpine2", Some(1), [0.0, 1.05, 0.0]),
            ("mTorso", Some(2), [0.0, 1.13, 0.0]),
            ("mSpine3", Some(3), [0.0, 1.13, 0.0]),
            ("mSpine4", Some(4), [0.0, 1.13, 0.0]),
            ("mChest", Some(5), [0.0, 1.33, 0.0]),
            ("mNeck", Some(6), [0.0, 1.58, 0.0]),
            ("mHead", Some(7), [0.0, 1.66, 0.0]),
            ("BELLY", Some(3), [0.0, 1.1, 0.12]),
            ("UPPER_BACK", Some(6), [0.0, 1.4, -0.15]),
        ];
        for (side, s) in [("Left", 1.0f32), ("Right", -1.0)] {
            let chest = 6;
            let at = j.len();
            j.push((if side == "Left" { "mCollarLeft" } else { "mCollarRight" }, Some(chest), [0.08 * s, 1.5, 0.0]));
            j.push((if side == "Left" { "mShoulderLeft" } else { "mShoulderRight" }, Some(at), [0.2 * s, 1.5, 0.0]));
            j.push((if side == "Left" { "L_UPPER_ARM" } else { "R_UPPER_ARM" }, Some(at + 1), [0.35 * s, 1.5, 0.0]));
            j.push((if side == "Left" { "mElbowLeft" } else { "mElbowRight" }, Some(at + 1), [0.5 * s, 1.5, 0.0]));
            j.push((if side == "Left" { "mWristLeft" } else { "mWristRight" }, Some(at + 3), [0.75 * s, 1.5, 0.0]));
            j.push((if side == "Left" { "mHandMiddle1Left" } else { "mHandMiddle1Right" }, Some(at + 4), [0.85 * s, 1.5, 0.0]));
            let at = j.len();
            j.push((if side == "Left" { "mHipLeft" } else { "mHipRight" }, Some(0), [0.1 * s, 1.0, 0.0]));
            j.push((if side == "Left" { "mKneeLeft" } else { "mKneeRight" }, Some(at), [0.1 * s, 0.55, 0.0]));
            j.push((if side == "Left" { "mAnkleLeft" } else { "mAnkleRight" }, Some(at + 1), [0.1 * s, 0.1, 0.0]));
            j.push((if side == "Left" { "mFootLeft" } else { "mFootRight" }, Some(at + 2), [0.1 * s, 0.02, 0.12]));
        }
        let i = |name: &str| index(&j, name);
        let body = &bodies(&rig(&j, 1.0, json!({})))[0];
        assert_eq!(body.hips, i("mPelvis"));
        assert_eq!(body.spine, vec![i("mSpine1"), i("mSpine2"), i("mTorso"), i("mSpine3"), i("mSpine4"), i("mChest")]);
        assert_eq!(body.neck, vec![i("mNeck"), i("mHead")]);
        assert_eq!(body.arms[0], Limb { root: Some(i("mCollarLeft")), upper: i("mShoulderLeft"), lower: i("mElbowLeft"), end: i("mWristLeft"), tip: Some(i("mHandMiddle1Left")) });
        assert_eq!(body.legs[1], Limb { root: None, upper: i("mHipRight"), lower: i("mKneeRight"), end: i("mAnkleRight"), tip: Some(i("mFootRight")) });
    }

    #[test]
    fn a_vrm_0_facing_away_has_its_own_left_on_the_other_side() {
        let joints = person();
        let body = &bodies(&rig(&joints, -1.0, json!({"VRM": {}})))[0];
        assert_eq!(body.arms[0].upper, index(&joints, "upper_arm.L"));
        assert_eq!(body.legs[1].upper, index(&joints, "thigh.R"));
    }

    #[test]
    fn a_heads_end_turns_nothing_and_a_t_pose_is_a_person_too() {
        let mut joints = person();
        // Arms straight out, and a node marking the top of the head.
        for (name, _, at) in joints.iter_mut() {
            let x = match *name {
                "upper_arm.L" | "upper_arm.R" => 0.17,
                "forearm.L" | "forearm.R" => 0.45,
                "hand.L" | "hand.R" => 0.70,
                "finger.L" | "finger.R" => 0.80,
                _ => continue,
            };
            *at = [x * at[0].signum(), 1.42, 0.0];
        }
        joints.push(("HeadTop_End", Some(4), [0.0, 1.70, 0.0]));
        let body = &bodies(&rig(&joints, 1.0, json!({})))[0];
        assert_eq!(body.neck, vec![index(&joints, "neck"), index(&joints, "head")]);
        assert_eq!(body.arms[1].lower, index(&joints, "forearm.R"));
    }

    #[test]
    fn a_vrm_says_which_bone_is_which() {
        let joints = person();
        let i = |name: &str| index(&joints, name);
        let bones = json!({
            "hips": {"node": i("hips")}, "spine": {"node": i("spine")}, "chest": {"node": i("chest")},
            "neck": {"node": i("neck")}, "head": {"node": i("head")},
            "leftUpperArm": {"node": i("upper_arm.L")}, "leftLowerArm": {"node": i("forearm.L")}, "leftHand": {"node": i("hand.L")},
            "rightUpperArm": {"node": i("upper_arm.R")}, "rightLowerArm": {"node": i("forearm.R")}, "rightHand": {"node": i("hand.R")},
            "leftUpperLeg": {"node": i("thigh.L")}, "leftLowerLeg": {"node": i("shin.L")}, "leftFoot": {"node": i("foot.L")},
            "rightUpperLeg": {"node": i("thigh.R")}, "rightLowerLeg": {"node": i("shin.R")}, "rightFoot": {"node": i("foot.R")}
        });
        let body = &bodies(&rig(&joints, 1.0, json!({"VRMC_vrm": {"humanoid": {"humanBones": bones}}})))[0];
        assert_eq!(body.arms[0], Limb { root: None, upper: i("upper_arm.L"), lower: i("forearm.L"), end: i("hand.L"), tip: None });
        // No shoulders named: none to shrug.
        assert!(!body.parts().contains(&"left_shoulder"));
        assert!(body.parts().contains(&"left_forearm"));
    }

    #[test]
    fn what_is_not_a_persons_skeleton_is_no_body() {
        let chain: Vec<(&str, Option<usize>, [f32; 3])> = (0..16usize).map(|k| ("tail", k.checked_sub(1), [0.0, 0.1 * k as f32, -0.1 * k as f32])).collect();
        assert!(bodies(&rig(&chain, 1.0, json!({}))).is_empty());
        assert!(bodies(&json!({"nodes": [{"name": "Body"}]})).is_empty());
        // One arm short of a person.
        let joints: Vec<_> = person().into_iter().filter(|j| !j.0.ends_with(".R") || j.0.starts_with("thigh") || j.0.starts_with("shin") || j.0.starts_with("foot") || j.0.starts_with("toe")).collect();
        let kept: Vec<&str> = joints.iter().map(|j| j.0).collect();
        let mut renumbered = Vec::new();
        let all = person();
        for (name, parent, at) in &joints {
            renumbered.push((*name, parent.map(|p| kept.iter().position(|k| *k == all[p].0).unwrap()), *at));
        }
        assert!(bodies(&rig(&renumbered, 1.0, json!({}))).is_empty());
    }

    #[test]
    fn a_poses_words_say_where_each_part_points() {
        let a = aim("left_upper_arm", "Down, a little out").unwrap().unwrap();
        assert_eq!((a.left, a.up, a.front), (0.45, -1.0, 0.0));
        // Out is away from the middle: the other way for a right arm.
        let a = aim("right_upper_arm", "out and slightly forward").unwrap().unwrap();
        assert_eq!((a.left, a.up, a.front), (-1.0, 0.0, 0.45));
        let a = aim("left_forearm", "in, across your chest").unwrap().unwrap();
        assert_eq!(a.left, -1.0);
        assert!(aim("right_shin", "straight").unwrap().unwrap().straight);
        let a = aim("head", "a little down, tilt right").unwrap().unwrap();
        assert_eq!((a.up, a.tilt, a.left), (-0.45, -1.0, 0.0));
        let a = aim("chest", "lean forward, turn left").unwrap().unwrap();
        assert_eq!((a.front, a.turn), (1.0, 1.0));
        assert!(aim("chest", "upright").unwrap().unwrap() == Aim::default());
        // Left as it moves.
        assert_eq!(aim("left_hand", "relaxed").unwrap(), None);
        // What says nothing, says how to say it.
        let why = aim("left_hand", "on my hip").unwrap_err();
        assert!(why.contains("front, back, up"), "{why}");
        // A model thinking aloud in its call (neohorse-1:4b, 2026-11-19).
        assert!(aim("right_upper_arm", "down, a little front, a little in (wait, hips means upper arm down, forearm... hand on hip). Let me phrase: hand on hip.").is_err());
        assert!(aim("left_upper_arm", "pointing down towards the floor, with the elbow bent").unwrap().is_some());
        assert!(aim("head", "out").is_err());
        assert!(aim("left_thigh", "up down").is_err());
        assert!(aim("tail", "up").unwrap_err().contains("left_upper_arm"));
    }
}
