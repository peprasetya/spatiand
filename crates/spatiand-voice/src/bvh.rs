//! Motions from BVH files, the format animations are passed around in outside Second Life (the
//! owner, 2026-11-20: animations "whose licence allows it" -- CMU's motion capture, packs sold
//! with a licence for use outside SL): a BVH's skeleton and frames made a [`Motion`], which the
//! room plays on a pal's own figure as it plays one made by hemera (`crates/spatiand/src/motion.rs`).
//!
//! The skeleton's joints are found by their names -- CMU's and Mixamo's (`LeftUpLeg`), Second
//! Life's own (`mHipLeft`) and its BVHs' (`lThigh`), Unreal's (`thigh_l`), 3ds Max's biped (`Bip01
//! L Thigh`), Rigify's (`thigh.L`) -- and, when their names say nothing, by its shape at rest
//! (`figure::body_of`), as a figure's are; then named as hemera's motions name theirs (SMPL's
//! `L_Hip`, `Spine1`...). A part the skeleton lacks (a toe, a third spine joint, a collarbone) is
//! added, turning with the joint above it. The frames are brought into a motion's own frame: +Y up,
//! facing +Z at its first frame, its own left at +X, standing on y = 0 where its feet are lowest,
//! and starting where it stands. More than thirty frames a second are thinned to thirty; a first
//! frame that is only the skeleton at rest (as an SL BVH's often is: its importer's reference) is
//! left out.

use glam::{Mat3, Quat, Vec3};

use crate::figure;
use crate::motion::{Frame, Motion, Skeleton};

/// More frames a second than this are thinned to about this: CMU's capture is at 120.
const MOST_FPS: f32 = 30.0;

/// What a BVH may not exceed, so that a broken or hostile file costs nothing.
const MOST_JOINTS: usize = 1024;
const MOST_VALUES: usize = 64 << 20;

#[derive(Clone, Copy, Debug, PartialEq)]
enum Channel {
    Position(usize),
    Rotation(usize),
}

#[derive(Debug)]
struct Joint {
    name: String,
    parent: Option<usize>,
    offset: Vec3,
    channels: Vec<Channel>,
    /// An `End Site`: where a bone ends, with no turn of its own.
    end: bool,
}

const AXES: [Vec3; 3] = [Vec3::X, Vec3::Y, Vec3::Z];

/// A BVH file's text made a motion, or why not.
pub fn read(text: &str) -> Result<Motion, String> {
    let (joints, frame_time, values) = parse(text)?;
    let per_frame: usize = joints.iter().map(|j| j.channels.len()).sum();
    if per_frame == 0 {
        return Err("the BVH's skeleton moves nothing (it has no channels)".into());
    }
    let frames = values.len() / per_frame;
    if frames == 0 {
        return Err("the BVH has no frames".into());
    }
    let fps = if frame_time > 0.0 { 1.0 / frame_time } else { MOST_FPS };
    let n = joints.len();

    // Each frame as the file has it: the root's place, each joint's turn from its parent's.
    let mut raw: Vec<(Vec3, Vec<Quat>)> = Vec::with_capacity(frames);
    for f in 0..frames {
        let mut values = values[f * per_frame..(f + 1) * per_frame].iter();
        // A root with no place in its channels stands where its offset puts it.
        let mut root = if joints[0].channels.iter().any(|c| matches!(c, Channel::Position(_))) { Vec3::ZERO } else { joints[0].offset };
        let mut turns = vec![Quat::IDENTITY; n];
        for (j, joint) in joints.iter().enumerate() {
            for channel in &joint.channels {
                let v = *values.next().unwrap_or(&0.0);
                match *channel {
                    Channel::Position(axis) if joint.parent.is_none() => root[axis] = v,
                    Channel::Position(_) => {}
                    // In the order listed: "Zrotation Xrotation Yrotation" is Rz * Rx * Ry.
                    Channel::Rotation(axis) => turns[j] = turns[j] * Quat::from_axis_angle(AXES[axis], v.to_radians()),
                }
            }
        }
        raw.push((root, turns));
    }

    // Where each joint is at rest, in the file's own frame.
    let rest = |joints: &[Joint]| {
        let mut at = vec![Vec3::ZERO; joints.len()];
        for (j, joint) in joints.iter().enumerate() {
            at[j] = joint.parent.map(|p| at[p]).unwrap_or(Vec3::ZERO) + joint.offset;
        }
        at
    };
    let file_at = rest(&joints);

    // Up: a BVH is +Y up, but some are written +Z up (Blender's frame).
    let extent = |axis: usize| file_at.iter().map(|p| p[axis]).fold(f32::MIN, f32::max) - file_at.iter().map(|p| p[axis]).fold(f32::MAX, f32::min);
    let up_fix = if extent(2) > 1.5 * extent(1) { Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2) } else { Quat::IDENTITY };

    // Its own left, from the joints its names put on either side; facing +Z with it at +X.
    let named: Vec<(i32, String)> = joints.iter().map(|j| if j.end { (0, String::new()) } else { side_and_base(&j.name) }).collect();
    let side_mean = |side: i32| {
        let at: Vec<Vec3> = named.iter().enumerate().filter(|(_, (s, _))| *s == side).map(|(j, _)| up_fix * file_at[j]).collect();
        (!at.is_empty()).then(|| at.iter().copied().sum::<Vec3>() / at.len() as f32)
    };
    let left = match (side_mean(1), side_mean(-1)) {
        (Some(l), Some(r)) => Vec3::new(l.x - r.x, 0.0, l.z - r.z),
        _ => Vec3::X,
    };
    let left = if left.length() > 1e-6 { left.normalize() } else { Vec3::X };
    let front = left.cross(Vec3::Y).normalize();
    // Taking its left to +X, up to +Y, its front to +Z.
    let face = Quat::from_mat3(&Mat3::from_cols(left, Vec3::Y, front)).inverse();
    let to_motion = (face * up_fix).normalize();
    let motion_at: Vec<Vec3> = file_at.iter().map(|p| to_motion * *p).collect();

    // Which joint is which part of a person's body.
    let parents: Vec<Option<usize>> = joints.iter().map(|j| j.parent).collect();
    let names: Vec<String> = joints.iter().map(|j| j.name.to_lowercase()).collect();
    let body = by_names(&joints, &named, &parents).or_else(|| figure::body_of(&parents, &motion_at, &names)).ok_or("no person's body was found in the BVH's skeleton")?;

    // The motion's skeleton: the file's joints, those of a body by SMPL's names, then any it lacks.
    let mut skeleton_names: Vec<String> = joints.iter().enumerate().map(|(j, joint)| format!("~{j}:{}", joint.name)).collect();
    let mut skeleton_parents: Vec<i32> = parents.iter().map(|p| p.map(|p| p as i32).unwrap_or(-1)).collect();
    let mut offsets: Vec<Vec3> = joints.iter().map(|j| to_motion * j.offset).collect();
    let name = |j: usize, as_name: &str, skeleton_names: &mut Vec<String>| skeleton_names[j] = as_name.to_string();
    // A joint added under `parent`, `offset` from it, turning as it does.
    let add = |parent: usize, offset: Vec3, as_name: &str, skeleton_names: &mut Vec<String>, skeleton_parents: &mut Vec<i32>, offsets: &mut Vec<Vec3>| {
        skeleton_names.push(as_name.to_string());
        skeleton_parents.push(parent as i32);
        offsets.push(offset);
        skeleton_names.len() - 1
    };

    let leg_length = (motion_at[body.legs[0].upper] - motion_at[body.legs[0].lower]).length() + (motion_at[body.legs[0].lower] - motion_at[body.legs[0].end]).length();
    name(body.hips, "Pelvis", &mut skeleton_names);
    // The spine: three joints, the first, middle and last of the file's; any missing added on top.
    let mut spine: Vec<usize> = match body.spine.len() {
        0 => Vec::new(),
        1 => vec![body.spine[0]],
        2 => vec![body.spine[0], body.spine[1]],
        k => vec![body.spine[0], body.spine[k / 2], body.spine[k - 1]],
    };
    while spine.len() < 3 {
        let parent = spine.last().copied().unwrap_or(body.hips);
        let j = add(parent, Vec3::ZERO, "", &mut skeleton_names, &mut skeleton_parents, &mut offsets);
        spine.push(j);
    }
    for (k, j) in spine.iter().enumerate() {
        name(*j, &format!("Spine{}", k + 1), &mut skeleton_names);
    }
    // The neck and the head: the first above the spine and the last (the head).
    let (neck, head) = match body.neck.len() {
        0 => {
            let neck = add(spine[2], Vec3::ZERO, "", &mut skeleton_names, &mut skeleton_parents, &mut offsets);
            (neck, add(neck, Vec3::Y * 0.1 * leg_length, "", &mut skeleton_names, &mut skeleton_parents, &mut offsets))
        }
        1 => (add(spine[2], Vec3::ZERO, "", &mut skeleton_names, &mut skeleton_parents, &mut offsets), body.neck[0]),
        k => (body.neck[0], body.neck[k - 1]),
    };
    name(neck, "Neck", &mut skeleton_names);
    name(head, "Head", &mut skeleton_names);
    for (k, side) in ["L", "R"].into_iter().enumerate() {
        let arm = body.arms[k];
        let collar = match arm.root {
            Some(c) => c,
            // Where the arm joins: its collarbone turning with the chest.
            None => add(parents[arm.upper].unwrap_or(spine[2]), Vec3::ZERO, "", &mut skeleton_names, &mut skeleton_parents, &mut offsets),
        };
        name(collar, &format!("{side}_Collar"), &mut skeleton_names);
        name(arm.upper, &format!("{side}_Shoulder"), &mut skeleton_names);
        name(arm.lower, &format!("{side}_Elbow"), &mut skeleton_names);
        name(arm.end, &format!("{side}_Wrist"), &mut skeleton_names);
        let leg = body.legs[k];
        name(leg.upper, &format!("{side}_Hip"), &mut skeleton_names);
        name(leg.lower, &format!("{side}_Knee"), &mut skeleton_names);
        name(leg.end, &format!("{side}_Ankle"), &mut skeleton_names);
        let toe = match leg.tip {
            Some(t) => t,
            // No toe: one ahead of the ankle and a little down, turning with it.
            None => add(leg.end, Vec3::new(0.0, -0.03, 0.12) * leg_length, "", &mut skeleton_names, &mut skeleton_parents, &mut offsets),
        };
        name(toe, &format!("{side}_Foot"), &mut skeleton_names);
    }
    let all = skeleton_names.len();
    for (j, n) in skeleton_names.iter_mut().enumerate() {
        if n.is_empty() {
            *n = format!("~added{j}");
        }
    }

    // The frames: the file's turns in the motion's frame (each turn is about the same axes the
    // skeleton's offsets are in, so turned as they are), thinned to MOST_FPS.
    let step = ((fps / MOST_FPS).round() as usize).max(1);
    // A first frame of the skeleton at rest only (an SL BVH's reference) is no part of the move.
    let at_rest = |(_, turns): &(Vec3, Vec<Quat>)| turns.iter().all(|q| q.angle_between(Quat::IDENTITY) < 0.5_f32.to_radians());
    let first = usize::from(raw.len() > 2 && at_rest(&raw[0]) && !at_rest(&raw[1]));
    let kept: Vec<&(Vec3, Vec<Quat>)> = raw[first..].iter().step_by(step).collect();
    let conj = |q: Quat| (to_motion * q * to_motion.inverse()).normalize();

    // Facing +Z at the first frame: the root's turn then, about the vertical, taken off all of them.
    let first_root = conj(kept[0].1[0]);
    let faces = first_root * Vec3::Z;
    let heading = if Vec3::new(faces.x, 0.0, faces.z).length() > 1e-4 { faces.x.atan2(faces.z) } else { 0.0 };
    let unturn = Quat::from_rotation_y(-heading);

    // World places of the feet, frame by frame: where its ground is.
    let feet: Vec<usize> = [body.legs[0].end, body.legs[1].end].into_iter().chain(body.legs.iter().filter_map(|l| l.tip)).chain(feet_ends(&joints, &body)).collect();
    let mut frames_out = Vec::with_capacity(kept.len());
    let mut ground = f32::MAX;
    let mut world = vec![(Vec3::ZERO, Quat::IDENTITY); n];
    let mut roots = Vec::with_capacity(kept.len());
    for (root, turns) in &kept {
        let mut rotations = Vec::with_capacity(all);
        for j in 0..n {
            let mut q = conj(turns[j]);
            if j == 0 {
                q = (unturn * q).normalize();
            }
            rotations.push(q);
            let (p, w) = match parents[j] {
                Some(parent) => {
                    let (pp, pw) = world[parent];
                    (pp + pw * offsets[j], (pw * q).normalize())
                }
                None => (unturn * (to_motion * *root), q),
            };
            world[j] = (p, w);
        }
        for &f in &feet {
            ground = ground.min(world[f].0.y);
        }
        roots.push(unturn * (to_motion * *root));
        // Added joints turn as their parents: no turn of their own.
        rotations.resize(all, Quat::IDENTITY);
        frames_out.push(rotations);
    }
    // Standing at rest: the root's height over its lowest foot joint, as the motion's ground is.
    let rest_ground = feet.iter().map(|f| motion_at[*f].y).fold(f32::MAX, f32::min);
    let rest_height = motion_at[0].y - rest_ground;
    let (x0, z0) = (roots[0].x, roots[0].z);
    let frames: Vec<Frame> = roots
        .iter()
        .zip(frames_out)
        .map(|(r, rotations)| Frame { translation: [r.x - x0, r.y - ground, r.z - z0], rotations: rotations.into_iter().map(|q| q.to_array()).collect() })
        .collect();
    offsets[0] = Vec3::new(0.0, rest_height, 0.0);
    let motion = Motion {
        fps: fps / step as f32,
        looping: false,
        seed: 0,
        skeleton: Skeleton { joints: skeleton_names, parents: skeleton_parents, offsets: offsets.into_iter().map(|o| o.to_array()).collect() },
        frames,
    };
    motion.check()?;
    Ok(motion)
}

/// The `End Site`s under the feet: their tips, the lowest a foot reaches.
fn feet_ends(joints: &[Joint], body: &figure::Body) -> Vec<usize> {
    let feet: Vec<usize> = body.legs.iter().flat_map(|l| [Some(l.end), l.tip]).flatten().collect();
    joints.iter().enumerate().filter(|(_, j)| j.end && j.parent.is_some_and(|p| feet.contains(&p))).map(|(k, _)| k).collect()
}

/// The skeleton and the frames' values: (joints, seconds a frame, values).
fn parse(text: &str) -> Result<(Vec<Joint>, f32, Vec<f32>), String> {
    let mut lines = text.lines().map(str::trim).enumerate().filter(|(_, l)| !l.is_empty());
    match lines.next() {
        Some((_, l)) if l.eq_ignore_ascii_case("HIERARCHY") => {}
        _ => return Err("not a BVH file: it does not begin with HIERARCHY".into()),
    }
    let mut joints: Vec<Joint> = Vec::new();
    // The joint each open brace belongs to.
    let mut open: Vec<usize> = Vec::new();
    let mut pending: Option<usize> = None;
    let mut roots = 0;
    let mut motion = None;
    for (number, line) in lines.by_ref() {
        let bad = |what: &str| format!("line {}: {what}", number + 1);
        let (head, rest) = line.split_once(char::is_whitespace).map(|(h, r)| (h, r.trim())).unwrap_or((line, ""));
        match head.to_ascii_uppercase().as_str() {
            "ROOT" | "JOINT" | "END" => {
                let end = head.eq_ignore_ascii_case("END");
                if head.eq_ignore_ascii_case("ROOT") {
                    roots += 1;
                    if roots > 1 {
                        return Err(bad("a second skeleton: only one is read"));
                    }
                }
                let parent = open.last().copied();
                if parent.is_none() && !head.eq_ignore_ascii_case("ROOT") {
                    return Err(bad("a joint outside the skeleton"));
                }
                let name = rest.trim_end_matches('{').trim();
                let name = if end { format!("{}_end", parent.map(|p| joints[p].name.as_str()).unwrap_or("")) } else { name.to_string() };
                if joints.len() >= MOST_JOINTS {
                    return Err(format!("more than {MOST_JOINTS} joints"));
                }
                joints.push(Joint { name, parent, offset: Vec3::ZERO, channels: Vec::new(), end });
                pending = Some(joints.len() - 1);
                if line.ends_with('{') {
                    open.push(joints.len() - 1);
                    pending = None;
                }
            }
            "{" => open.push(pending.take().ok_or_else(|| bad("a brace with no joint"))?),
            "}" => {
                open.pop().ok_or_else(|| bad("a brace closed that was not opened"))?;
            }
            "OFFSET" => {
                let j = *open.last().ok_or_else(|| bad("an offset outside a joint"))?;
                let v: Vec<f32> = rest.split_whitespace().map(str::parse).collect::<Result<_, _>>().map_err(|_| bad("an offset that is not three numbers"))?;
                if v.len() != 3 || v.iter().any(|x| !x.is_finite()) {
                    return Err(bad("an offset that is not three numbers"));
                }
                joints[j].offset = Vec3::new(v[0], v[1], v[2]);
            }
            "CHANNELS" => {
                let j = *open.last().ok_or_else(|| bad("channels outside a joint"))?;
                let mut words = rest.split_whitespace();
                let count: usize = words.next().and_then(|c| c.parse().ok()).ok_or_else(|| bad("channels with no count"))?;
                let channels: Vec<Channel> = words
                    .map(|w| {
                        let axis = match w.chars().next().map(|c| c.to_ascii_lowercase()) {
                            Some('x') => 0,
                            Some('y') => 1,
                            Some('z') => 2,
                            _ => return Err(bad("a channel that is not an axis")),
                        };
                        match w.get(1..).map(str::to_ascii_lowercase).as_deref() {
                            Some("position") => Ok(Channel::Position(axis)),
                            Some("rotation") => Ok(Channel::Rotation(axis)),
                            _ => Err(bad("a channel that is not a position or a rotation")),
                        }
                    })
                    .collect::<Result<_, _>>()?;
                if channels.len() != count {
                    return Err(bad("channels that do not number as many as they say"));
                }
                joints[j].channels = channels;
            }
            "MOTION" => {
                if !open.is_empty() {
                    return Err(bad("the skeleton is not closed before its motion"));
                }
                motion = Some(number);
                break;
            }
            _ => return Err(bad(&format!("{head} is not part of a BVH's skeleton"))),
        }
    }
    motion.ok_or("the BVH has no MOTION")?;
    if joints.is_empty() {
        return Err("the BVH has no skeleton".into());
    }
    // "Frames: N", "Frame Time: t", then the values, however they are laid out in lines.
    let mut frame_time = 0.0;
    let mut frames: Option<usize> = None;
    let mut values = Vec::new();
    for (_, line) in lines {
        let lower = line.to_ascii_lowercase();
        if let Some(n) = lower.strip_prefix("frames:") {
            frames = n.trim().parse().ok();
            continue;
        }
        if let Some(t) = lower.strip_prefix("frame time:") {
            frame_time = t.trim().parse().map_err(|_| "a frame time that is not a number")?;
            continue;
        }
        for word in line.split_whitespace() {
            let v: f32 = word.parse().map_err(|_| format!("{word} is not a number in the BVH's frames"))?;
            values.push(if v.is_finite() { v } else { 0.0 });
            if values.len() > MOST_VALUES {
                return Err("the BVH's frames are too many to read".into());
            }
        }
    }
    let per_frame: usize = joints.iter().map(|j| j.channels.len()).sum();
    if let Some(f) = frames {
        // As many frames as it says, of those it has whole.
        values.truncate(f.saturating_mul(per_frame).min(values.len()));
    }
    Ok((joints, frame_time, values))
}

/// A joint's side by its name (1 its own left, -1 its right, 0 neither), and its name without the
/// side or a rig's prefix, in small letters and without numbers at its end: `LeftUpLeg` (1,
/// "upleg"), `mHipLeft` (1, "mhip"), `lThigh` (1, "thigh"), `thigh_l` (1, "thigh"), `Bip01 R
/// Calf` (-1, "calf"), `mixamorig:Neck` (0, "neck"), `Spine1` (0, "spine").
fn side_and_base(name: &str) -> (i32, String) {
    // Words: split at what is not a letter or a digit, and where a capital starts a word.
    let mut words: Vec<String> = Vec::new();
    let chars: Vec<char> = name.chars().collect();
    let mut word = String::new();
    for (i, c) in chars.iter().enumerate() {
        if !c.is_alphanumeric() {
            if !word.is_empty() {
                words.push(std::mem::take(&mut word));
            }
            continue;
        }
        let starts = c.is_uppercase() && !word.is_empty() && (chars.get(i.wrapping_sub(1)).is_some_and(|p| p.is_lowercase() || p.is_ascii_digit()) || chars.get(i + 1).is_some_and(|n| n.is_lowercase()));
        if starts {
            words.push(std::mem::take(&mut word));
        }
        word.push(c.to_ascii_lowercase());
    }
    if !word.is_empty() {
        words.push(word);
    }
    let mut side = 0;
    words.retain(|w| match w.as_str() {
        "left" | "l" | "lft" => {
            side = 1;
            false
        }
        "right" | "r" | "rt" | "rgt" => {
            side = -1;
            false
        }
        "mixamorig" | "bip01" | "bip001" | "bip" | "def" | "jnt" | "characternode" => false,
        _ => true,
    });
    let base: String = words.concat();
    (side, base.trim_end_matches(|c: char| c.is_ascii_digit()).to_string())
}

/// The parts of a body by the joints' names, both sides; `None` if any is missing.
fn by_names(joints: &[Joint], named: &[(i32, String)], parents: &[Option<usize>]) -> Option<figure::Body> {
    let find = |side: i32, bases: &[&str]| named.iter().enumerate().position(|(j, (s, b))| !joints[j].end && *s == side && bases.contains(&b.as_str()));
    const PELVIS: &[&str] = &["hips", "hip", "pelvis", "mpelvis"];
    const HEAD: &[&str] = &["head", "mhead"];
    const NECK: &[&str] = &["neck", "mneck"];
    const COLLAR: &[&str] = &["shoulder", "collar", "clavicle", "mcollar"];
    const UPPER_ARM: &[&str] = &["arm", "upperarm", "uparm", "shldr", "mshoulder", "humerus"];
    const FOREARM: &[&str] = &["forearm", "lowerarm", "elbow", "melbow"];
    const HAND: &[&str] = &["hand", "wrist", "mwrist"];
    const THIGH: &[&str] = &["upleg", "upperleg", "thigh", "hip", "mhip", "femur"];
    const SHIN: &[&str] = &["leg", "lowerleg", "shin", "calf", "knee", "mknee"];
    const ANKLE: &[&str] = &["foot", "ankle", "mankle"];
    const TOE: &[&str] = &["toebase", "toe", "toes", "ball", "mfoot", "mtoe"];
    let head = find(0, HEAD)?;
    // The pelvis: one named so above the legs, or the root.
    let hips = find(0, PELVIS).unwrap_or(0);
    let chain = |j: usize| {
        let mut all = vec![j];
        let mut k = j;
        while let Some(p) = parents[k] {
            all.push(p);
            k = p;
        }
        all.reverse();
        all
    };
    // From the pelvis up to the head: its spine, then its neck.
    let up = chain(head);
    let from = up.iter().position(|j| *j == hips)?;
    let up = &up[from + 1..];
    let neck_at = up.iter().position(|j| NECK.contains(&named[*j].1.as_str())).unwrap_or(up.len().saturating_sub(1));
    let spine: Vec<usize> = up[..neck_at].to_vec();
    let neck: Vec<usize> = up[neck_at..].to_vec();
    let limb = |side: i32, upper: &[&str], lower: &[&str], end: &[&str], tip: &[&str], root: Option<&[&str]>| -> Option<figure::Limb> {
        let upper = find(side, upper)?;
        let lower = find(side, lower)?;
        let end_j = find(side, end)?;
        // Each below the one before it, as a limb's joints are.
        let below = |j: usize, of: usize| chain(j).contains(&of);
        if !below(lower, upper) || !below(end_j, lower) {
            return None;
        }
        let tip = find(side, tip).filter(|t| below(*t, end_j)).or_else(|| parents.iter().position(|p| *p == Some(end_j)).filter(|t| !joints[*t].end));
        let root = root.and_then(|r| find(side, r)).filter(|c| below(upper, *c) && *c != upper);
        Some(figure::Limb { root, upper, lower, end: end_j, tip })
    };
    let arm = |side| limb(side, UPPER_ARM, FOREARM, HAND, &["fingerbase", "handindex", "middle", "index"], Some(COLLAR));
    let leg = |side| limb(side, THIGH, SHIN, ANKLE, TOE, None);
    Some(figure::Body { hips, spine, neck, arms: [arm(1)?, arm(-1)?], legs: [leg(1)?, leg(-1)?] })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A small person in a T-pose, CMU's names and layout (+Y up, facing +Z, its left at +X):
    /// `frames` lines of values after it.
    fn cmu(frames: &[&str], time: f32) -> String {
        let mut s = String::from(
            "HIERARCHY
ROOT Hips
{
  OFFSET 0 0 0
  CHANNELS 6 Xposition Yposition Zposition Zrotation Yrotation Xrotation
  JOINT LeftUpLeg
  {
    OFFSET 1 -1 0
    CHANNELS 3 Zrotation Yrotation Xrotation
    JOINT LeftLeg
    {
      OFFSET 0 -4 0
      CHANNELS 3 Zrotation Yrotation Xrotation
      JOINT LeftFoot
      {
        OFFSET 0 -4 0
        CHANNELS 3 Zrotation Yrotation Xrotation
        JOINT LeftToeBase
        {
          OFFSET 0 -0.5 1
          CHANNELS 3 Zrotation Yrotation Xrotation
          End Site
          {
            OFFSET 0 0 0.5
          }
        }
      }
    }
  }
  JOINT RightUpLeg
  {
    OFFSET -1 -1 0
    CHANNELS 3 Zrotation Yrotation Xrotation
    JOINT RightLeg
    {
      OFFSET 0 -4 0
      CHANNELS 3 Zrotation Yrotation Xrotation
      JOINT RightFoot
      {
        OFFSET 0 -4 0
        CHANNELS 3 Zrotation Yrotation Xrotation
        JOINT RightToeBase
        {
          OFFSET 0 -0.5 1
          CHANNELS 3 Zrotation Yrotation Xrotation
          End Site
          {
            OFFSET 0 0 0.5
          }
        }
      }
    }
  }
  JOINT Spine
  {
    OFFSET 0 2 0
    CHANNELS 3 Zrotation Yrotation Xrotation
    JOINT Spine1
    {
      OFFSET 0 2 0
      CHANNELS 3 Zrotation Yrotation Xrotation
      JOINT Neck
      {
        OFFSET 0 1.5 0
        CHANNELS 3 Zrotation Yrotation Xrotation
        JOINT Head
        {
          OFFSET 0 1 0
          CHANNELS 3 Zrotation Yrotation Xrotation
          End Site
          {
            OFFSET 0 1.5 0
          }
        }
      }
      JOINT LeftShoulder
      {
        OFFSET 0.5 1 0
        CHANNELS 3 Zrotation Yrotation Xrotation
        JOINT LeftArm
        {
          OFFSET 3.5 0 0
          CHANNELS 3 Zrotation Yrotation Xrotation
          JOINT LeftForeArm
          {
            OFFSET 5 0 0
            CHANNELS 3 Zrotation Yrotation Xrotation
            JOINT LeftHand
            {
              OFFSET 2.5 0 0
              CHANNELS 3 Zrotation Yrotation Xrotation
              End Site
              {
                OFFSET 1 0 0
              }
            }
          }
        }
      }
      JOINT RightShoulder
      {
        OFFSET -0.5 1 0
        CHANNELS 3 Zrotation Yrotation Xrotation
        JOINT RightArm
        {
          OFFSET -3.5 0 0
          CHANNELS 3 Zrotation Yrotation Xrotation
          JOINT RightForeArm
          {
            OFFSET -5 0 0
            CHANNELS 3 Zrotation Yrotation Xrotation
            JOINT RightHand
            {
              OFFSET -2.5 0 0
              CHANNELS 3 Zrotation Yrotation Xrotation
              End Site
              {
                OFFSET -1 0 0
              }
            }
          }
        }
      }
    }
  }
}
MOTION
",
        );
        s.push_str(&format!("Frames: {}\nFrame Time: {time}\n", frames.len()));
        for f in frames {
            s.push_str(f);
            s.push('\n');
        }
        s
    }

    /// 6 + 20 joints x 3 values: the root at (x, y, z), and a turn for one joint (counted from 1
    /// after the root, in the file's order), the rest zero.
    fn frame(root: [f32; 3], joint: usize, zyx: [f32; 3]) -> String {
        let mut v = vec![root[0], root[1], root[2], 0.0, 0.0, 0.0];
        v.extend(std::iter::repeat_n(0.0, 20 * 3));
        let at = if joint == 0 { 3 } else { 6 + (joint - 1) * 3 };
        v[at..at + 3].copy_from_slice(&zyx);
        v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(" ")
    }

    /// Where each joint of a motion is, frame by frame, as the room turns them: R = R_parent * r.
    fn places(m: &Motion, f: usize) -> Vec<Vec3> {
        let n = m.skeleton.joints.len();
        let mut world = vec![(Vec3::ZERO, Quat::IDENTITY); n];
        for j in 0..n {
            let q = Quat::from_array(m.frames[f].rotations[j]);
            world[j] = match m.skeleton.parents[j] {
                p if p >= 0 => {
                    let (pp, pw) = world[p as usize];
                    (pp + pw * Vec3::from(m.skeleton.offsets[j]), pw * q)
                }
                _ => (Vec3::from(m.frames[f].translation), q),
            };
        }
        world.into_iter().map(|(p, _)| p).collect()
    }

    #[test]
    fn a_cmu_skeleton_is_named_as_hemera_names_its_joints() {
        let m = read(&cmu(&[&frame([0.0, 9.5, 0.0], 0, [0.0; 3]), &frame([0.0, 9.5, 0.0], 0, [0.0; 3])], 0.0333)).unwrap();
        for (smpl, file) in [("Pelvis", "Hips"), ("L_Hip", "LeftUpLeg"), ("R_Knee", "RightLeg"), ("L_Ankle", "LeftFoot"), ("L_Foot", "LeftToeBase"), ("L_Collar", "LeftShoulder"), ("L_Shoulder", "LeftArm"), ("L_Elbow", "LeftForeArm"), ("R_Wrist", "RightHand"), ("Neck", "Neck"), ("Head", "Head")] {
            let j = m.joint(smpl).unwrap_or_else(|| panic!("no {smpl} in {:?}", m.skeleton.joints));
            // Its place is the file's joint's: the same distance from the pelvis.
            let file_j = m.skeleton.joints.iter().position(|n| n.ends_with(&format!(":{file}")));
            assert!(file_j.is_none(), "{file} kept its own name beside {smpl}");
            let _ = j;
        }
        // Two spine joints in the file: a third added, at the second's place.
        let (s2, s3) = (m.joint("Spine2").unwrap(), m.joint("Spine3").unwrap());
        assert_eq!(m.skeleton.parents[s3], s2 as i32);
        assert_eq!(m.skeleton.offsets[s3], [0.0, 0.0, 0.0]);
        assert_eq!((m.frames.len(), m.fps.round()), (2, 30.0));
        // Standing on the ground: the pelvis 9.5 over its lowest foot joint, at rest and in the frames.
        let at = places(&m, 0);
        let lowest = at.iter().map(|p| p.y).fold(f32::MAX, f32::min);
        assert!(lowest.abs() < 1e-4, "{lowest}");
        assert!((m.skeleton.offsets[0][1] - 9.5).abs() < 1e-4, "{:?}", m.skeleton.offsets[0]);
    }

    #[test]
    fn a_turn_in_the_file_is_the_same_turn_in_the_motion() {
        // LeftArm (the 14th joint after the root) turned 90 degrees about Z: the arm, out along +X
        // at rest, then points up -- the forearm's joint above the shoulder's.
        let m = read(&cmu(&[&frame([0.0, 9.5, 0.0], 14, [90.0, 0.0, 0.0]), &frame([0.0, 9.5, 0.0], 14, [90.0, 0.0, 0.0])], 0.0333)).unwrap();
        let at = places(&m, 0);
        let (shoulder, elbow) = (at[m.joint("L_Shoulder").unwrap()], at[m.joint("L_Elbow").unwrap()]);
        let way = (elbow - shoulder).normalize();
        assert!(way.y > 0.99, "{way}");
        // The other arm as it was: out to its own right, -X.
        let (shoulder, elbow) = (at[m.joint("R_Shoulder").unwrap()], at[m.joint("R_Elbow").unwrap()]);
        assert!((elbow - shoulder).normalize().x < -0.99);
    }

    #[test]
    fn a_skeleton_facing_away_or_written_z_up_is_brought_round() {
        // The same person turned to face -Z: its left at -X in the file.
        let away = cmu(&[&frame([0.0, 9.5, 0.0], 0, [0.0; 3]), &frame([0.0, 9.5, 0.0], 0, [0.0; 3])], 0.0333)
            .lines()
            .map(|l| match l.trim().strip_prefix("OFFSET ") {
                Some(v) => {
                    let v: Vec<f32> = v.split_whitespace().map(|x| x.parse().unwrap()).collect();
                    format!("OFFSET {} {} {}", -v[0], v[1], -v[2])
                }
                None => l.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        let m = read(&away).unwrap();
        let at = places(&m, 0);
        // Its own left at +X again, its toes ahead (+Z).
        assert!(at[m.joint("L_Hip").unwrap()].x > 0.5, "{:?}", at[m.joint("L_Hip").unwrap()]);
        let (ankle, toe) = (at[m.joint("L_Ankle").unwrap()], at[m.joint("L_Foot").unwrap()]);
        assert!(toe.z > ankle.z);
        // Written with +Z up (y and z swapped as Blender's frame has them): upright again.
        let z_up = cmu(&[&frame([0.0, 9.5, 0.0], 0, [0.0; 3]), &frame([0.0, 9.5, 0.0], 0, [0.0; 3])], 0.0333)
            .lines()
            .map(|l| match l.trim().strip_prefix("OFFSET ") {
                Some(v) => {
                    let v: Vec<f32> = v.split_whitespace().map(|x| x.parse().unwrap()).collect();
                    format!("OFFSET {} {} {}", v[0], -v[2], v[1])
                }
                None => l.to_string(),
            })
            .collect::<Vec<_>>()
            .join("\n");
        let m = read(&z_up).unwrap();
        let at = places(&m, 0);
        assert!(at[m.joint("Head").unwrap()].y > at[m.joint("Pelvis").unwrap()].y + 4.0);
        assert!(at[m.joint("L_Hip").unwrap()].x > 0.5);
    }

    #[test]
    fn it_starts_facing_the_wearer_where_it_stands_and_thinned_to_thirty_a_second() {
        // Turned 90 degrees about the vertical, 40 units off, at 120 frames a second.
        let frames: Vec<String> = (0..12).map(|k| {
            let mut f = frame([40.0 + k as f32, 9.5, -7.0], 0, [0.0; 3]);
            // The root's Y rotation is its fifth value.
            let mut v: Vec<String> = f.split(' ').map(str::to_string).collect();
            v[4] = "90".into();
            f = v.join(" ");
            f
        }).collect();
        let refs: Vec<&str> = frames.iter().map(String::as_str).collect();
        let m = read(&cmu(&refs, 1.0 / 120.0)).unwrap();
        assert_eq!((m.frames.len(), m.fps.round()), (3, 30.0));
        let q = Quat::from_array(m.frames[0].rotations[0]);
        assert!((q * Vec3::Z).z > 0.99, "{:?}", q * Vec3::Z);
        assert_eq!((m.frames[0].translation[0], m.frames[0].translation[2]), (0.0, 0.0));
        // Walking along the file's +X, turned with it: now ahead, along +Z... or to the side, as the
        // turn says; either way 4 frames (one kept) on, it has moved 4 units.
        let moved = Vec3::from(m.frames[1].translation);
        assert!((Vec3::new(moved.x, 0.0, moved.z).length() - 4.0).abs() < 1e-3, "{moved}");
    }

    #[test]
    fn an_sl_bvh_by_its_poser_names_with_a_reference_frame_first() {
        // SL's BVHs: hip, abdomen, chest, neck, head, lCollar, lShldr..., no toes; frame 1 at rest.
        let joint = |name: &str, offset: &str, inner: &str| format!("JOINT {name}\n{{\nOFFSET {offset}\nCHANNELS 3 Xrotation Zrotation Yrotation\n{inner}}}\n");
        let end = |offset: &str| format!("End Site\n{{\nOFFSET {offset}\n}}\n");
        let arm = |s: &str, x: f32| joint(&format!("{s}Collar"), &format!("{} 3 0", x), &joint(&format!("{s}Shldr"), &format!("{} 0 0", 3.0 * x), &joint(&format!("{s}ForeArm"), &format!("{} 0 0", 10.0 * x), &joint(&format!("{s}Hand"), &format!("{} 0 0", 9.0 * x), &end(&format!("{} 0 0", 6.0 * x))))));
        let leg = |s: &str, x: f32| joint(&format!("{s}Thigh"), &format!("{} -2 0", 4.0 * x), &joint(&format!("{s}Shin"), "0 -18 0", &joint(&format!("{s}Foot"), "0 -17 0", &end("0 -3 4"))));
        let upper = joint("abdomen", "0 3 0", &joint("chest", "0 4 0", &format!("{}{}{}", joint("neck", "0 4 0", &joint("head", "0 2 0", &end("0 3 0"))), arm("l", 1.0), arm("r", -1.0))));
        let text = format!(
            "HIERARCHY\nROOT hip\n{{\nOFFSET 0 0 0\nCHANNELS 6 Xposition Yposition Zposition Xrotation Zrotation Yrotation\n{upper}{}{}}}\nMOTION\nFrames: 3\nFrame Time: 0.0333\n",
            leg("l", 1.0),
            leg("r", -1.0)
        );
        // At rest, then the root 40 up and lShldr (the 6th after the root) turned 60 about Z.
        let rest = vec!["0"; 6 + 18 * 3].join(" ");
        let mut moving = vec!["0".to_string(); 6 + 18 * 3];
        moving[1] = "40".into();
        moving[6 + 5 * 3 + 1] = "60".into();
        let moving = moving.join(" ");
        let text = format!("{text}{rest}\n{moving}\n{moving}\n");
        let m = read(&text).unwrap_or_else(|e| panic!("{e}\n{text}"));
        // The reference frame left out: two frames.
        assert_eq!(m.frames.len(), 2);
        for smpl in ["Pelvis", "Spine1", "Spine2", "Spine3", "Neck", "Head", "L_Collar", "L_Shoulder", "L_Elbow", "L_Wrist", "R_Hip", "R_Knee", "R_Ankle", "R_Foot"] {
            assert!(m.joint(smpl).is_some(), "no {smpl}: {:?}", m.skeleton.joints);
        }
        // lShldr is the upper arm, not lCollar: turned up 60 degrees, its elbow above it.
        let at = places(&m, 0);
        let way = (at[m.joint("L_Elbow").unwrap()] - at[m.joint("L_Shoulder").unwrap()]).normalize();
        assert!((way.y - 60f32.to_radians().sin()).abs() < 1e-3, "{way}");
        // No toe in the file: one added under the ankle, ahead of it.
        let toe = m.joint("L_Foot").unwrap();
        assert_eq!(m.skeleton.parents[toe], m.joint("L_Ankle").unwrap() as i32);
        assert!(m.skeleton.offsets[toe][2] > 0.0);
        // Standing 40 over its lowest point, its toes' ends.
        assert!((m.skeleton.offsets[0][1] - 40.0).abs() < 1e-3, "{:?}", m.skeleton.offsets[0]);
    }

    #[test]
    fn names_say_their_side_and_their_part() {
        for (name, side, base) in [
            ("LeftUpLeg", 1, "upleg"),
            ("mHipLeft", 1, "mhip"),
            ("lThigh", 1, "thigh"),
            ("thigh_l", 1, "thigh"),
            ("thigh.L", 1, "thigh"),
            ("Bip01 R Calf", -1, "calf"),
            ("mixamorig:Neck", 0, "neck"),
            ("Spine1", 0, "spine"),
            ("LHipJoint", 1, "hipjoint"),
            ("RightForeArm", -1, "forearm"),
            ("mShoulderRight", -1, "mshoulder"),
            ("LowerBack", 0, "lowerback"),
            ("Root", 0, "root"),
        ] {
            assert_eq!(side_and_base(name), (side, base.to_string()), "{name}");
        }
    }

    #[test]
    fn what_is_not_a_bvh_says_so() {
        assert!(read("hello").unwrap_err().contains("HIERARCHY"));
        assert!(read("HIERARCHY\nROOT Hips\n{\nOFFSET 0 0\n}\nMOTION\nFrames: 0\nFrame Time: 0.1\n").unwrap_err().contains("three numbers"));
        let no_frames = cmu(&[], 0.0333);
        assert!(read(&no_frames).unwrap_err().contains("no frames"));
        assert!(read("HIERARCHY\nROOT a\n{\nOFFSET 0 0 0\nCHANNELS 3 Xrotation Yrotation Zrotation\n}\nMOTION\nFrames: 1\nFrame Time: 0.1\n0 0 0\n").unwrap_err().contains("body"));
    }

    /// Every BVH in `SPATIAND_BVH_DIR` read as a motion: `SPATIAND_BVH_DIR=<folder> cargo test -p
    /// spatiand-voice -- --ignored bvh_files`.
    #[test]
    #[ignore]
    fn bvh_files() {
        let dir = std::env::var("SPATIAND_BVH_DIR").expect("SPATIAND_BVH_DIR");
        let mut read_any = false;
        for entry in std::fs::read_dir(&dir).unwrap().flatten() {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("bvh")) {
                let text = std::fs::read_to_string(&path).unwrap();
                let m = read(&text).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
                let at = places(&m, 0);
                let (l, r) = (at[m.joint("L_Hip").unwrap()], at[m.joint("R_Hip").unwrap()]);
                eprintln!("{}: {} frames at {:.0} fps, {:.1} s; pelvis {:.2} up; left hip {:+.2} x", path.display(), m.frames.len(), m.fps, m.duration(), m.skeleton.offsets[0][1], l.x - r.x);
                assert!(l.x > r.x, "{}: its left on its right", path.display());
                read_any = true;
            }
        }
        assert!(read_any);
    }
}
