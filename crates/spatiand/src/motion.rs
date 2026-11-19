//! A motion made from words, by hemera (`spatiand_voice::motion`), played on the figure's own body
//! (the owner, 2026-11-19: hemera "now makes motion from words ... build the Spatiand side").
//!
//! The motion is SMPL's 22 body joints, turned from a T-pose. Each is laid onto the part of the
//! body the room finds in the figure (`spatiand_voice::figure::bodies`, as hemera's
//! docs/integrate-spatiand.md section 5 maps them): the figure's bone is first brought from where
//! it stands at rest to where it points in a T-pose of its own -- the way the motion's bone points
//! at its rest -- and is then turned in the world as the motion turns that bone. A hand turns with
//! its forearm's turn, as the motion has no fingers; the figure's own fingers, hair and face stay
//! as they are. The hips go where the motion's pelvis goes, by the figure's leg over the motion's.
//!
//! What comes of it is an ordinary animation of the figure (`Clip`), named as the assistant named
//! the motion: played once as a `move`, held or looped as a `pose`, faded in and out as the
//! figure's own are, and a pose of the assistant's own making laid over it.

use std::collections::HashMap;
use std::time::SystemTime;

use glam::{Mat4, Quat, Vec3};
use serde_json::Value;
use spatiand_voice::figure::Body;
use spatiand_voice::motion::Motion;

use crate::figure::{Channel, Clip, Interpolation, Skeleton, Track};

/// The motions the assistant named (`motions` in what it says of its figure: a name to a file),
/// as clips of the figure, after its own.
#[derive(Default)]
pub struct Motions {
    /// The figure's own clips, before any motion's.
    own: Option<usize>,
    /// What was named last, with when each file was written: read again only when that changes.
    named: Vec<(String, String, Option<SystemTime>)>,
}

/// The figure's clips brought up to what the assistant named: `true` when they changed.
pub fn take(skeleton: &mut Skeleton, said: &Value, motions: &mut Motions) -> bool {
    let own = *motions.own.get_or_insert(skeleton.clips.len());
    let mut named: Vec<(String, String, Option<SystemTime>)> = said
        .get("motions")
        .and_then(Value::as_object)
        .into_iter()
        .flatten()
        .filter_map(|(name, path)| {
            let path = path.as_str()?.trim();
            (!name.trim().is_empty() && !path.is_empty()).then(|| (name.trim().to_string(), path.to_string(), std::fs::metadata(path).and_then(|m| m.modified()).ok()))
        })
        .collect();
    named.sort();
    if named == motions.named {
        return false;
    }
    skeleton.clips.truncate(own);
    for (name, path, _) in &named {
        match read(path).and_then(|m| clip(skeleton, &m, name).ok_or_else(|| "the figure has no body to move".to_string())) {
            Ok(c) => skeleton.clips.push(c),
            Err(why) => log::warn!("avatar: the motion {name} ({path}) is not played: {why}"),
        }
    }
    log::info!("avatar: motions {:?}", skeleton.clips[own..].iter().map(|c| c.name.as_str()).collect::<Vec<_>>());
    motions.named = named;
    true
}

/// A motion from its file: hemera's own, or one kept with what it was made from (`motion` in it).
fn read(path: &str) -> Result<Motion, String> {
    let text = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    let v: Value = serde_json::from_slice(&text).map_err(|e| format!("not JSON: {e}"))?;
    let v = v.get("motion").cloned().unwrap_or(v);
    let m: Motion = serde_json::from_value(v).map_err(|e| format!("not a motion: {e}"))?;
    m.check()?;
    Ok(m)
}

/// A figure's node onto one of the motion's joints: `from`/`to` say which way the motion's bone
/// points at rest (from joint to joint), `start`/`end` the figure's (from node to node). Only a
/// limb is brought to the motion's way at rest (`limb`): the body and head stand upright in both,
/// and SMPL's own head joint, ahead of its neck, would have bowed the figure's head.
struct Part {
    node: usize,
    joint: usize,
    from: usize,
    to: usize,
    start: usize,
    end: usize,
    limb: bool,
}

/// The parts of `body` the motion moves, by hemera's names for its joints.
fn parts(body: &Body, m: &Motion) -> Option<Vec<Part>> {
    let j = |name: &str| m.joint(name);
    let mut out = Vec::new();
    let spine_joints = [j("Spine1")?, j("Spine2")?, j("Spine3")?];
    let (pelvis, neck, head) = (j("Pelvis")?, j("Neck")?, j("Head")?);
    let first_above = body.spine.first().or(body.neck.first()).copied();
    if let Some(above) = first_above {
        out.push(Part { node: body.hips, joint: pelvis, from: pelvis, to: spine_joints[0], start: body.hips, end: above, limb: false });
    }
    // The spine's joints shared out along the figure's, its top the motion's top.
    let n = body.spine.len();
    for (k, node) in body.spine.iter().enumerate() {
        let s = (((k + 1) * 3 + n / 2) / n.max(1)).clamp(1, 3) - 1;
        let joint = spine_joints[s];
        let to = if s == 2 { neck } else { spine_joints[s + 1] };
        let end = body.spine.get(k + 1).or(body.neck.first()).copied().unwrap_or(*node);
        out.push(Part { node: *node, joint, from: joint, to, start: *node, end, limb: false });
    }
    // The neck's joints, then the head: both point the way the neck does.
    for (k, node) in body.neck.iter().enumerate() {
        let joint = if k + 1 == body.neck.len() { head } else { neck };
        let (start, end) = if body.neck.len() >= 2 { (body.neck[body.neck.len() - 2], body.neck[body.neck.len() - 1]) } else { (*body.spine.last().unwrap_or(&body.hips), *node) };
        let (start, end) = if k + 1 < body.neck.len() { (*node, body.neck[k + 1]) } else { (start, end) };
        out.push(Part { node: *node, joint, from: neck, to: head, start, end, limb: false });
    }
    for (k, side) in ["L", "R"].into_iter().enumerate() {
        let (collar, shoulder, elbow, wrist) = (j(&format!("{side}_Collar"))?, j(&format!("{side}_Shoulder"))?, j(&format!("{side}_Elbow"))?, j(&format!("{side}_Wrist"))?);
        let arm = &body.arms[k];
        if let Some(root) = arm.root {
            out.push(Part { node: root, joint: collar, from: collar, to: shoulder, start: root, end: arm.upper, limb: true });
        }
        out.push(Part { node: arm.upper, joint: shoulder, from: shoulder, to: elbow, start: arm.upper, end: arm.lower, limb: true });
        out.push(Part { node: arm.lower, joint: elbow, from: elbow, to: wrist, start: arm.lower, end: arm.end, limb: true });
        // A hand turns as its forearm points: the motion has no fingers to say otherwise.
        out.push(Part { node: arm.end, joint: wrist, from: elbow, to: wrist, start: arm.lower, end: arm.end, limb: true });
        let (hip, knee, ankle, foot) = (j(&format!("{side}_Hip"))?, j(&format!("{side}_Knee"))?, j(&format!("{side}_Ankle"))?, j(&format!("{side}_Foot"))?);
        let leg = &body.legs[k];
        out.push(Part { node: leg.upper, joint: hip, from: hip, to: knee, start: leg.upper, end: leg.lower, limb: true });
        out.push(Part { node: leg.lower, joint: knee, from: knee, to: ankle, start: leg.lower, end: leg.end, limb: true });
        match leg.tip {
            Some(tip) => {
                out.push(Part { node: leg.end, joint: ankle, from: ankle, to: foot, start: leg.end, end: tip, limb: true });
                out.push(Part { node: tip, joint: foot, from: ankle, to: foot, start: leg.end, end: tip, limb: true });
            }
            // No toe to say which way the foot points: it turns as its shin does.
            None => out.push(Part { node: leg.end, joint: ankle, from: knee, to: ankle, start: leg.lower, end: leg.end, limb: true }),
        }
    }
    Some(out)
}

fn rotation(m: Mat4) -> Quat {
    m.to_scale_rotation_translation().1.normalize()
}

fn at(m: Mat4) -> Vec3 {
    m.w_axis.truncate()
}

fn quat(v: [f32; 4]) -> Quat {
    let q = Quat::from_xyzw(v[0], v[1], v[2], v[3]);
    if q.length_squared() > 1e-8 {
        q.normalize()
    } else {
        Quat::IDENTITY
    }
}

/// The motion as an animation of the figure, named `name`; `None` when the figure has no body
/// for it, or the motion lacks a joint the mapping needs.
pub fn clip(skeleton: &Skeleton, m: &Motion, name: &str) -> Option<Clip> {
    if skeleton.bodies.is_empty() {
        return None;
    }
    // The motion's frame (glTF's) in the figure's file's: a VRM 0.x faces -Z.
    let turned = skeleton.facing < 0.0;
    let flip = Quat::from_rotation_y(std::f32::consts::PI);
    let vec = |v: Vec3| if turned { Vec3::new(-v.x, v.y, -v.z) } else { v };
    let rot = |q: Quat| if turned { flip * q * flip.inverse() } else { q };

    // The motion's skeleton at rest: each joint's place in the world.
    let n = m.skeleton.joints.len();
    let mut rest_at = vec![Vec3::ZERO; n];
    for i in 0..n {
        let off = Vec3::from(m.skeleton.offsets[i]);
        rest_at[i] = match m.skeleton.parents[i] {
            p if p >= 0 => rest_at[p as usize] + off,
            _ => off,
        };
    }
    let smpl_dir = |from: usize, to: usize| vec(rest_at[to] - rest_at[from]).normalize_or_zero();

    let rest_local = skeleton.rest();
    let rest = skeleton.world(&rest_local);
    let fig_dir = |a: usize, b: usize| (at(rest[b]) - at(rest[a])).normalize_or_zero();

    // Each mapped node, with where it stands in its T-pose (a world turn).
    let mut mapped: HashMap<usize, (usize, Quat)> = HashMap::new();
    let mut hips = Vec::new();
    for body in &skeleton.bodies {
        let parts = parts(body, m)?;
        for p in parts {
            let (fd, sd) = (fig_dir(p.start, p.end), smpl_dir(p.from, p.to));
            let to_t = if !p.limb || fd == Vec3::ZERO || sd == Vec3::ZERO { Quat::IDENTITY } else { Quat::from_rotation_arc(fd, sd) };
            mapped.insert(p.node, (p.joint, (to_t * rotation(rest[p.node])).normalize()));
        }
        // The legs' length, the figure's over the motion's: how far the hips go for the motion's.
        let leg = |a: Vec3, b: Vec3, c: Vec3| (b - a).length() + (c - b).length();
        let l = &body.legs[0];
        let fig_leg = leg(at(rest[l.upper]), at(rest[l.lower]), at(rest[l.end]));
        let (hip, knee, ankle) = (m.joint("L_Hip")?, m.joint("L_Knee")?, m.joint("L_Ankle")?);
        let smpl_leg = leg(rest_at[hip], rest_at[knee], rest_at[ankle]);
        let k = if smpl_leg > 1e-4 { fig_leg / smpl_leg } else { 1.0 };
        hips.push((body.hips, k, fig_leg));
    }
    let root_rest_y = m.skeleton.offsets.first().map(|o| o[1]).unwrap_or(0.97);

    let frames = m.frames.len();
    let times: Vec<f32> = (0..frames).map(|f| f as f32 / m.fps.max(1.0)).collect();
    let mut rotations: HashMap<usize, Vec<f32>> = mapped.keys().map(|node| (*node, Vec::with_capacity(frames * 4))).collect();
    let mut places: HashMap<usize, Vec<f32>> = hips.iter().map(|(h, _, _)| (*h, Vec::with_capacity(frames * 3))).collect();
    let mut world_turn = vec![Quat::IDENTITY; n];
    let mut fig_turn = vec![Quat::IDENTITY; skeleton.nodes.len()];
    for frame in &m.frames {
        // The motion's joints turned in the world: R = R_parent * r.
        for i in 0..n {
            let r = quat(frame.rotations[i]);
            world_turn[i] = match m.skeleton.parents[i] {
                p if p >= 0 => (world_turn[p as usize] * r).normalize(),
                _ => r,
            };
        }
        // The figure's nodes, parents first: a mapped one turned as its joint is, any other as it is
        // at rest under its parent; each mapped one's own turn from its parent's kept.
        for &node in &skeleton.order {
            let parent = skeleton.nodes[node].parent.map(|p| fig_turn[p]).unwrap_or(Quat::IDENTITY);
            fig_turn[node] = match mapped.get(&node) {
                Some((joint, t_pose)) => (rot(world_turn[*joint]) * *t_pose).normalize(),
                None => (parent * rest_local[node].r).normalize(),
            };
            if mapped.contains_key(&node) {
                let local = (parent.inverse() * fig_turn[node]).normalize();
                rotations.get_mut(&node).unwrap().extend_from_slice(&local.to_array());
            }
        }
        // The hips where the motion's pelvis goes, from where they stand at rest; not wandering
        // off further than half a leg from there, the figure having its own place in the room.
        let t = vec(Vec3::from(frame.translation) - Vec3::new(0.0, root_rest_y, 0.0));
        for (node, k, fig_leg) in &hips {
            let mut moved = t * *k;
            let flat = Vec3::new(moved.x, 0.0, moved.z);
            let most = 0.5 * fig_leg;
            if flat.length() > most {
                let kept = flat.normalize() * most;
                moved = Vec3::new(kept.x, moved.y, kept.z);
            }
            let parent = skeleton.nodes[*node].parent.map(|p| rest[p]).unwrap_or(Mat4::IDENTITY);
            let place = parent.inverse().transform_point3(at(rest[*node]) + moved);
            places.get_mut(node).unwrap().extend_from_slice(&place.to_array());
        }
    }
    let mut tracks: Vec<Track> = rotations
        .into_iter()
        .map(|(node, values)| Track { node, channel: Channel::Rotation, interpolation: Interpolation::Linear, times: times.clone(), values })
        .collect();
    tracks.extend(places.into_iter().map(|(node, values)| Track { node, channel: Channel::Translation, interpolation: Interpolation::Linear, times: times.clone(), values }));
    tracks.sort_by_key(|t| (t.node, t.channel == Channel::Translation));
    Some(Clip { name: name.to_string(), duration: m.duration(), tracks })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::figure::{Node, Skin, Trs};
    use serde_json::json;

    /// SMPL's 22 joints as hemera has them at rest (a T-pose), from a motion it made.
    fn smpl(frames: Vec<(Vec3, Vec<(usize, Quat)>)>, fps: f32) -> Motion {
        let joints = ["Pelvis", "L_Hip", "R_Hip", "Spine1", "L_Knee", "R_Knee", "Spine2", "L_Ankle", "R_Ankle", "Spine3", "L_Foot", "R_Foot", "Neck", "L_Collar", "R_Collar", "Head", "L_Shoulder", "R_Shoulder", "L_Elbow", "R_Elbow", "L_Wrist", "R_Wrist"];
        let parents = [-1, 0, 0, 0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 9, 9, 12, 13, 14, 16, 17, 18, 19];
        let offsets: Vec<[f32; 3]> = vec![
            [-0.0018, 0.97017, 0.02822], [0.0695, -0.0914, -0.0043], [-0.0677, -0.0905, -0.0043], [-0.0025, 0.1089, -0.0267], [0.0343, -0.3755, -0.0045], [-0.0383, -0.3825, -0.0088],
            [0.0055, 0.1352, 0.0011], [-0.0136, -0.398, -0.0437], [0.0158, -0.3984, -0.0423], [0.0015, 0.0529, 0.0254], [0.0264, -0.0558, 0.1193], [-0.0254, -0.0482, 0.1233],
            [-0.0028, 0.2139, -0.0334], [0.0789, 0.1217, -0.0341], [-0.0818, 0.1188, -0.0386], [0.005, 0.065, 0.0513], [0.1225, 0.0452, -0.019], [-0.1137, 0.0473, -0.0085],
            [0.2553, -0.0156, -0.0229], [-0.2602, -0.0143, -0.0313], [0.2657, 0.0127, -0.0073], [-0.2691, 0.0068, -0.0063],
        ];
        let frames: Vec<Value> = frames
            .into_iter()
            .map(|(t, turns)| {
                let mut r = vec![[0.0f32, 0.0, 0.0, 1.0]; 22];
                for (j, q) in turns {
                    r[j] = q.to_array();
                }
                json!({"translation": [t.x, t.y, t.z], "rotations": r})
            })
            .collect();
        serde_json::from_value(json!({"fps": fps, "loop": false, "seed": 1, "skeleton": {"joints": joints, "parents": parents, "offsets": offsets}, "frames": frames})).unwrap()
    }

    /// A made-up person in an A-pose, as posing.rs's tests have one, facing +Z (-Z for -1).
    fn person(facing: f32) -> (Skeleton, Vec<&'static str>) {
        let mut all: Vec<(&'static str, Option<usize>, [f32; 3])> =
            vec![("hips", None, [0.0, 0.95, 0.0]), ("spine", Some(0), [0.0, 1.05, 0.0]), ("chest", Some(1), [0.0, 1.25, 0.0]), ("neck", Some(2), [0.0, 1.45, 0.0]), ("head", Some(3), [0.0, 1.55, 0.0])];
        for (side, names) in [(1.0, ["clavicle.L", "upper_arm.L", "forearm.L", "hand.L", "finger.L"]), (-1.0, ["clavicle.R", "upper_arm.R", "forearm.R", "hand.R", "finger.R"])] {
            let at = [[0.03, 1.40, 0.02], [0.17, 1.42, 0.0], [0.37, 1.22, -0.01], [0.55, 1.04, 0.0], [0.62, 0.97, 0.0]];
            let first = all.len();
            for (k, name) in names.iter().enumerate() {
                all.push((name, Some(if k == 0 { 2 } else { first + k - 1 }), [at[k][0] * side, at[k][1], at[k][2]]));
            }
        }
        for (side, names) in [(1.0, ["thigh.L", "shin.L", "foot.L", "toe.L"]), (-1.0, ["thigh.R", "shin.R", "foot.R", "toe.R"])] {
            let at = [[0.09, 0.90, 0.0], [0.10, 0.50, 0.01], [0.10, 0.08, -0.02], [0.10, 0.02, 0.12]];
            let first = all.len();
            for (k, name) in names.iter().enumerate() {
                all.push((name, Some(if k == 0 { 0 } else { first + k - 1 }), [at[k][0] * side, at[k][1], at[k][2]]));
            }
        }
        let file = |p: [f32; 3]| [p[0] * facing, p[1], p[2] * facing];
        let nodes: Vec<Value> = all
            .iter()
            .enumerate()
            .map(|(i, (name, parent, at))| {
                let (at, from) = (file(*at), parent.map(|p| file(all[p].2)).unwrap_or([0.0; 3]));
                let children: Vec<usize> = all.iter().enumerate().filter(|(_, j)| j.1 == Some(i)).map(|(k, _)| k).collect();
                json!({"name": name, "translation": [at[0] - from[0], at[1] - from[1], at[2] - from[2]], "children": children})
            })
            .collect();
        let extensions = if facing < 0.0 { json!({"VRM": {}}) } else { json!({}) };
        let root = json!({"scene": 0, "scenes": [{"nodes": [0]}], "nodes": nodes, "skins": [{"joints": (0..all.len()).collect::<Vec<_>>()}], "extensions": extensions});
        let n = all.len();
        let skeleton = Skeleton {
            nodes: all
                .iter()
                .map(|(name, parent, at)| {
                    let (at, from) = (file(*at), parent.map(|p| file(all[p].2)).unwrap_or([0.0; 3]));
                    Node { name: name.to_string(), parent: *parent, rest: Trs { t: Vec3::new(at[0] - from[0], at[1] - from[1], at[2] - from[2]), ..Default::default() } }
                })
                .collect(),
            order: spatiand_voice::figure::scene_order(&root),
            skins: vec![Skin { joints: (0..n).collect(), inverse_bind: vec![Mat4::IDENTITY; n] }],
            clips: Vec::new(),
            to_room: Mat4::IDENTITY,
            bodies: spatiand_voice::figure::bodies(&root),
            facing: spatiand_voice::figure::facing(&root),
        };
        (skeleton, all.iter().map(|a| a.0).collect())
    }

    /// Where each of the figure's joints is with the clip at `time`.
    fn played(s: &Skeleton, c: &Clip, time: f32, names: &[&str]) -> impl Fn(&str) -> Vec3 {
        let mut s2 = Skeleton { nodes: Vec::new(), order: s.order.clone(), skins: Vec::new(), clips: Vec::new(), to_room: s.to_room, bodies: Vec::new(), facing: s.facing };
        s2.nodes = s.nodes.iter().map(|n| Node { name: n.name.clone(), parent: n.parent, rest: n.rest }).collect();
        s2.clips = vec![Clip { name: c.name.clone(), duration: c.duration, tracks: c.tracks.iter().map(|t| Track { node: t.node, channel: t.channel, interpolation: t.interpolation, times: t.times.clone(), values: t.values.clone() }).collect() }];
        let mut local = s2.rest();
        s2.sample(0, time, &mut local);
        let world = s2.world(&local);
        let names: Vec<String> = names.iter().map(|n| n.to_string()).collect();
        move |name: &str| at(world[names.iter().position(|n| n == name).unwrap()])
    }

    #[test]
    fn the_motions_t_pose_is_the_figures_t_pose() {
        let (s, names) = person(1.0);
        let still = smpl(vec![(Vec3::new(0.0, 0.97, 0.0), vec![]); 2], 30.0);
        let c = clip(&s, &still, "still").unwrap();
        let at = played(&s, &c, 0.0, &names);
        // The arms out level, as the motion's are at its rest; the legs down, the head up.
        for (sh, el, side) in [("upper_arm.L", "forearm.L", 1.0), ("upper_arm.R", "forearm.R", -1.0)] {
            let d = (at(el) - at(sh)).normalize();
            assert!(d.x * side > 0.98, "{sh} {d}");
        }
        assert!((at("shin.L") - at("thigh.L")).normalize().y < -0.98);
        assert!((at("head") - at("neck")).normalize().y > 0.95);
        // Standing where it stood.
        assert!((at("hips") - Vec3::new(0.0, 0.95, 0.0)).length() < 0.01, "{}", at("hips"));
    }

    #[test]
    fn a_raised_arm_is_raised_on_the_figure() {
        let (s, names) = person(1.0);
        // The left shoulder turned about +Z by a quarter: the arm from out (+X) to up (+Y).
        let up = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let m = smpl(vec![(Vec3::new(0.0, 0.97, 0.0), vec![]), (Vec3::new(0.0, 0.97, 0.0), vec![(16, up)])], 30.0);
        let c = clip(&s, &m, "raise").unwrap();
        assert!((c.duration - 1.0 / 30.0).abs() < 1e-5);
        let at = played(&s, &c, c.duration, &names);
        let d = (at("forearm.L") - at("upper_arm.L")).normalize();
        assert!(d.y > 0.98, "{d}");
        // The hand stays in line with its forearm, and the right arm is level still.
        assert!((at("hand.L") - at("forearm.L")).normalize().y > 0.98);
        assert!((at("forearm.R") - at("upper_arm.R")).normalize().x < -0.98);
    }

    #[test]
    fn the_hips_go_by_the_figures_legs() {
        let (s, names) = person(1.0);
        // The pelvis down 10 cm (a crouch), and 2 m forwards -- which is held to half a leg.
        let m = smpl(vec![(Vec3::new(0.0, 0.97, 0.0), vec![]), (Vec3::new(0.0, 0.87, 2.0), vec![])], 30.0);
        let c = clip(&s, &m, "go").unwrap();
        let at = played(&s, &c, c.duration, &names);
        let leg_ratio = 0.82 / 0.78; // the made-up figure's leg over SMPL's, roughly
        assert!((at("hips").y - (0.95 - 0.10 * leg_ratio)).abs() < 0.02, "{}", at("hips"));
        assert!(at("hips").z > 0.3 && at("hips").z < 0.5, "{}", at("hips"));
    }

    #[test]
    fn a_vrm_0_figure_moves_its_own_left() {
        let (s, names) = person(-1.0);
        let up = Quat::from_rotation_z(std::f32::consts::FRAC_PI_2);
        let m = smpl(vec![(Vec3::new(0.0, 0.97, 0.0), vec![(16, up)]); 2], 30.0);
        let c = clip(&s, &m, "raise").unwrap();
        let at = played(&s, &c, 0.0, &names);
        // Its left arm up; its right out level, at -X... of a figure facing -Z, its right is +X.
        assert!((at("forearm.L") - at("upper_arm.L")).normalize().y > 0.98);
        assert!((at("forearm.R") - at("upper_arm.R")).normalize().x > 0.98);
    }

    #[test]
    fn what_is_named_is_played_and_what_is_not_is_let_go() {
        let (mut s, _) = person(1.0);
        let dir = std::env::temp_dir().join(format!("spatiand-motion-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let m = smpl(vec![(Vec3::new(0.0, 0.97, 0.0), vec![]); 3], 30.0);
        // Kept as SpatiPal keeps one: with the words it was made from.
        let path = dir.join("twirl.json");
        std::fs::write(&path, json!({"prompt": "a person spins around", "seed": 5, "motion": m}).to_string()).unwrap();
        let mut motions = Motions::default();
        let said = json!({"motions": {"twirl": path.to_str().unwrap(), "gone": "/nonexistent.json"}});
        assert!(take(&mut s, &said, &mut motions));
        assert_eq!(s.clips.iter().map(|c| c.name.as_str()).collect::<Vec<_>>(), ["twirl"]);
        assert!(s.clip("twirl").is_some());
        // One of the figure's own of that name: the motion is the one played.
        s.clips.insert(0, Clip { name: "twirl".into(), duration: 1.0, tracks: Vec::new() });
        motions.own = Some(1);
        assert_eq!(s.clip("Twirl"), Some(1));
        assert!(!take(&mut s, &said, &mut motions));
        assert!(take(&mut s, &json!({}), &mut motions));
        // The figure's own stays.
        assert_eq!(s.clips.len(), 1);
        assert!(s.clips[0].tracks.is_empty());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
