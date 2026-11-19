//! A pose of the figure's own making (2026-11-19, the owner: "if animation too complex, then
//! pose"): the assistant names parts of its body and where each points -- `{"left_upper_arm":
//! "down, out, a little back", "left_forearm": "in, a little down, a little front"}` -- and the
//! room turns the figure's joints so that they do, over whatever its animation is doing with the
//! rest. Which joints are which, and what the words mean, are `spatiand_voice::figure`'s:
//! SpatiPal reads them the same way (`docs/spatipal-pals.md`).
//!
//! Each bone is turned the short way from where it points now to where it is asked to. An elbow
//! or a knee bends as a person's does, one way only and only so far: the upper bone is first
//! turned about itself (as far as a shoulder or a hip turns) so that its hinge bends towards
//! where the lower bone is asked to point. A lean or a turn of the chest or the head is shared
//! out along the spine or the neck. Every direction is the figure's own -- its front, up, its own
//! left -- and an arm or a leg is kept where a body can put it.

use glam::{Mat4, Quat, Vec3};
use spatiand_voice::figure::{Aim, Body, Limb};

use crate::figure::{Skeleton, Trs};

const DEG: f32 = std::f32::consts::PI / 180.0;

/// How far a part turns for a word of it, in degrees.
const LEAN_FRONT: f32 = 35.0;
const LEAN_BACK: f32 = 25.0;
const LEAN_SIDE: f32 = 20.0;
const CHEST_TURN: f32 = 40.0;
const HEAD_TURN: f32 = 60.0;
const HEAD_UP: f32 = 35.0;
const HEAD_DOWN: f32 = 45.0;
const HEAD_TILT: f32 = 25.0;
const SHRUG_UP: f32 = 25.0;
const SHRUG_FRONT: f32 = 20.0;
/// How far an upper arm or a thigh turns about itself, and an elbow or a knee bends.
const ARM_TWIST: f32 = 100.0;
const LEG_TWIST: f32 = 45.0;
const ELBOW_BEND: f32 = 150.0;
const KNEE_BEND: f32 = 150.0;
/// How far a wrist bends the hand from the forearm's line.
const WRIST_BEND: f32 = 75.0;

/// The parts asked for and where, from what the assistant said (`pose` as an object). A part of
/// no such name, and words that say nothing, are left out.
pub fn read(pose: &serde_json::Value) -> Vec<(String, Aim)> {
    pose.as_object()
        .into_iter()
        .flatten()
        .filter_map(|(part, words)| {
            let part = part.trim().to_lowercase();
            spatiand_voice::figure::aim(&part, words.as_str()?).ok().flatten().map(|a| (part, a))
        })
        .collect()
}

/// The figure's own frame in its file's: towards its left, up, towards its front.
#[derive(Clone, Copy)]
struct Frame {
    left: Vec3,
    up: Vec3,
    front: Vec3,
}

impl Frame {
    /// Where a part's words point, in the file's frame; `None` when they point nowhere.
    fn of(&self, a: &Aim) -> Option<Vec3> {
        let v = self.left * a.left + self.up * a.up + self.front * a.front;
        (v.length() > 1e-3).then(|| v.normalize())
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Hinge {
    Elbow,
    Knee,
}

/// Lay the pose `aims` over `local` -- each node's own place, as the figure's animation has it
/// for the moment -- on each of the figure's bodies.
pub fn apply(skeleton: &Skeleton, aims: &[(String, Aim)], local: &mut [Trs]) {
    if aims.is_empty() || skeleton.bodies.is_empty() {
        return;
    }
    let f = Frame { left: Vec3::X * skeleton.facing, up: Vec3::Y, front: Vec3::Z * skeleton.facing };
    let asked = |part: &str| aims.iter().find(|(p, _)| p == part).map(|(_, a)| *a);
    let rest = skeleton.world(&skeleton.rest());
    for body in &skeleton.bodies {
        let world = skeleton.world(local);
        let mut p = Posing { skeleton, local: &mut *local, world };
        pose_body(&mut p, &rest, body, f, &asked);
    }
}

fn pose_body(p: &mut Posing, rest: &[Mat4], body: &Body, f: Frame, asked: &dyn Fn(&str) -> Option<Aim>) {
    if let Some(a) = asked("chest").filter(|_| !body.spine.is_empty()) {
        let k = body.spine.len();
        p.spread(&body.spine, chest(f, &a), &vec![1.0 / k as f32; k]);
    }
    if let Some(a) = asked("head").filter(|_| !body.neck.is_empty()) {
        // Most of a look is the head's own; the rest is the neck's.
        let k = body.neck.len();
        let shares: Vec<f32> = (0..k).map(|i| if k == 1 { 1.0 } else if i + 1 == k { 0.6 } else { 0.4 / (k - 1) as f32 }).collect();
        p.spread(&body.neck, head(f, &a), &shares);
    }
    for (k, side) in ["left", "right"].into_iter().enumerate() {
        let s = if k == 0 { 1.0 } else { -1.0 };
        let arm = &body.arms[k];
        if let (Some(root), Some(a)) = (arm.root, asked(&format!("{side}_shoulder"))) {
            shrug(p, root, arm.upper, f, &a);
        }
        limb(p, rest, arm, asked(&format!("{side}_upper_arm")), asked(&format!("{side}_forearm")), f, s, Hinge::Elbow);
        if let Some(a) = asked(&format!("{side}_hand")) {
            hand(p, arm, f, &a);
        }
        limb(p, rest, &body.legs[k], asked(&format!("{side}_thigh")), asked(&format!("{side}_shin")), f, s, Hinge::Knee);
    }
}

/// The nodes as they are being posed: their own places, and where each is in the figure.
struct Posing<'a> {
    skeleton: &'a Skeleton,
    local: &'a mut [Trs],
    world: Vec<Mat4>,
}

fn rotation(m: Mat4) -> Quat {
    m.to_scale_rotation_translation().1.normalize()
}

impl Posing<'_> {
    fn at(&self, n: usize) -> Vec3 {
        self.world[n].w_axis.truncate()
    }

    /// Which way the bone from `a` to `b` points.
    fn dir(&self, a: usize, b: usize) -> Vec3 {
        (self.at(b) - self.at(a)).normalize_or_zero()
    }

    fn rotation(&self, n: usize) -> Quat {
        rotation(self.world[n])
    }

    /// Turn node `n` by `q`, a turn in the file's frame, and everything under it with it.
    fn turn(&mut self, n: usize, q: Quat) {
        let own = self.rotation(n);
        let parent = self.skeleton.nodes[n].parent.map(|p| self.rotation(p)).unwrap_or(Quat::IDENTITY);
        self.local[n].r = (parent.inverse() * q * own).normalize();
        self.world = self.skeleton.world(self.local);
    }

    /// Turn a chain of nodes by `q` in all, `shares` of it at each.
    fn spread(&mut self, nodes: &[usize], q: Quat, shares: &[f32]) {
        for (n, share) in nodes.iter().zip(shares) {
            self.turn(*n, Quat::IDENTITY.slerp(q, *share));
        }
    }
}

/// The chest's lean, towards where its top goes, and its turn.
fn chest(f: Frame, a: &Aim) -> Quat {
    let forward = if a.front >= 0.0 { a.front * LEAN_FRONT } else { a.front * LEAN_BACK };
    let lean = f.front * forward + f.left * (a.left * LEAN_SIDE);
    let lean = if lean.length() > 1e-3 { Quat::from_axis_angle(f.up.cross(lean).normalize(), lean.length() * DEG) } else { Quat::IDENTITY };
    Quat::from_axis_angle(f.up, a.turn * CHEST_TURN * DEG) * lean
}

/// The head's look: turned towards its left or right, up or down (a head "back" looks up), and
/// tilted towards a shoulder.
fn head(f: Frame, a: &Aim) -> Quat {
    let up = (a.up - a.front.min(0.0) * 0.6).clamp(-1.0, 1.0);
    let down = if up >= 0.0 { -up * HEAD_UP } else { -up * HEAD_DOWN };
    Quat::from_axis_angle(f.up, (a.left + a.turn).clamp(-1.0, 1.0) * HEAD_TURN * DEG) * Quat::from_axis_angle(f.left, down * DEG) * Quat::from_axis_angle(f.front, -a.tilt * HEAD_TILT * DEG)
}

/// A shoulder raised (a shrug), or brought forward or back, by its collarbone.
fn shrug(p: &mut Posing, root: usize, upper: usize, f: Frame, a: &Aim) {
    let now = p.dir(root, upper);
    let want = (now + f.up * (a.up * (SHRUG_UP * DEG).tan()) + f.front * (a.front * (SHRUG_FRONT * DEG).tan())).normalize_or_zero();
    if now != Vec3::ZERO && want != Vec3::ZERO {
        p.turn(root, Quat::from_rotation_arc(now, want));
    }
}

/// An arm or a leg: its upper bone pointed as asked, turned about itself so that its hinge bends
/// towards where the lower bone is asked to point, and the lower bent there.
#[allow(clippy::too_many_arguments)]
fn limb(p: &mut Posing, rest: &[Mat4], limb: &Limb, upper: Option<Aim>, lower: Option<Aim>, f: Frame, side: f32, hinge: Hinge) {
    if upper.is_none() && lower.is_none() {
        return;
    }
    let (u_node, l_node, e_node) = (limb.upper, limb.lower, limb.end);
    // Which way it bends: as it is bent at rest, or -- straight at rest -- as a person's does, an
    // elbow forwards and a knee backwards; carried along as the upper bone is turned now.
    let at_rest = |a: usize, b: usize| (rest[b].w_axis.truncate() - rest[a].w_axis.truncate()).normalize_or_zero();
    let (ru, rl) = (at_rest(u_node, l_node), at_rest(l_node, e_node));
    let mut bend_rest = rl - ru * rl.dot(ru);
    if bend_rest.length() < 0.17 {
        let towards = if hinge == Hinge::Elbow { f.front } else { -f.front };
        bend_rest = towards - ru * towards.dot(ru);
    }
    let carry = p.rotation(u_node) * rotation(rest[u_node]).inverse();
    let u0 = p.dir(u_node, l_node);
    if u0 == Vec3::ZERO {
        return;
    }
    let bend0 = {
        let b = carry * bend_rest;
        (b - u0 * b.dot(u0)).normalize_or_zero()
    };
    let u = match upper.and_then(|a| f.of(&a)) {
        Some(v) => within(v, f, side, hinge),
        None => u0,
    };
    let q1 = Quat::from_rotation_arc(u0, u);
    let bend1 = q1 * bend0;
    let mut twist = Quat::IDENTITY;
    let mut angle = None;
    if let Some(a) = lower {
        if let Some(want) = if a.straight { Some(u) } else { f.of(&a) } {
            let across = want - u * want.dot(u);
            if across.length() > 1e-3 && bend1 != Vec3::ZERO {
                let across = across.normalize();
                let most = if hinge == Hinge::Elbow { ARM_TWIST } else { LEG_TWIST } * DEG;
                let by = bend1.cross(across).dot(u).atan2(bend1.dot(across)).clamp(-most, most);
                twist = Quat::from_axis_angle(u, by);
            }
            let most = if hinge == Hinge::Elbow { ELBOW_BEND } else { KNEE_BEND } * DEG;
            angle = Some(u.dot(want).clamp(-1.0, 1.0).acos().min(most));
        }
    }
    p.turn(u_node, twist * q1);
    if let Some(angle) = angle {
        let u_now = p.dir(u_node, l_node);
        let bend_now = {
            let b = twist * bend1;
            (b - u_now * b.dot(u_now)).normalize_or_zero()
        };
        let want = (u_now * angle.cos() + bend_now * angle.sin()).normalize_or_zero();
        let now = p.dir(l_node, e_node);
        if want != Vec3::ZERO && now != Vec3::ZERO {
            p.turn(l_node, Quat::from_rotation_arc(now, want));
        }
    }
}

/// `v` with no more of it along `axis` than `most`, the rest of it kept as it was (or `instead`,
/// when it had none) and the whole still a direction.
fn at_most(v: Vec3, axis: Vec3, most: f32, instead: Vec3) -> Vec3 {
    let along = v.dot(axis);
    if along <= most {
        return v;
    }
    let rest = v - axis * along;
    let rest = if rest.length() > 1e-3 { rest.normalize() } else { instead };
    axis * most + rest * (1.0 - most * most).max(0.0).sqrt()
}

/// An upper arm or a thigh kept where a body can put it: an arm pointed across the chest goes in
/// front of it, not into it, and not far behind the back; a thigh lifts its knee no higher than
/// a little above level, goes back and out only as far as a hip lets it, and crosses the other
/// leg only a little unless it is lifted in front.
fn within(v: Vec3, f: Frame, side: f32, hinge: Hinge) -> Vec3 {
    let out = f.left * side;
    match hinge {
        Hinge::Elbow => {
            let inward = -v.dot(out);
            let v = if inward > 0.0 { (v + f.front * (0.9 * inward - v.dot(f.front)).max(0.0)).normalize() } else { v };
            at_most(v, -f.front, 0.75, -f.up)
        }
        Hinge::Knee => {
            let v = at_most(v, f.up, 0.5, f.front);
            let v = at_most(v, -f.front, 0.5, -f.up);
            let v = at_most(v, out, 0.85, -f.up);
            at_most(v, -out, 0.35 + 0.6 * v.dot(f.front).max(0.0), -f.up)
        }
    }
}

/// A hand pointed where its fingers are asked to go, as far as a wrist bends.
fn hand(p: &mut Posing, arm: &Limb, f: Frame, a: &Aim) {
    let Some(want) = f.of(a) else { return };
    let fore = p.dir(arm.lower, arm.end);
    let now = arm.tip.map(|t| p.dir(arm.end, t)).filter(|d| *d != Vec3::ZERO).unwrap_or(fore);
    if fore == Vec3::ZERO || now == Vec3::ZERO {
        return;
    }
    let most = WRIST_BEND * DEG;
    let angle = fore.angle_between(want);
    let want = if angle > most { Quat::IDENTITY.slerp(Quat::from_rotation_arc(fore, want), most / angle) * fore } else { want };
    p.turn(arm.end, Quat::from_rotation_arc(now, want));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::figure::{Node, Skin};
    use serde_json::{json, Value};

    /// A made-up person, 1.6 m tall, arms down at 45 degrees, facing +Z (x its left): the names
    /// are for the tests only -- the room finds its body by its shape.
    fn person() -> Vec<(&'static str, Option<usize>, [f32; 3])> {
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
        all
    }

    /// Its skeleton as the room has it, turned to face -Z as a VRM 0.x when `facing` is -1.
    fn skeleton(facing: f32) -> Skeleton {
        let joints = person();
        let file = |p: [f32; 3]| [p[0] * facing, p[1], p[2] * facing];
        let nodes: Vec<Value> = joints
            .iter()
            .enumerate()
            .map(|(i, (name, parent, at))| {
                let (at, from) = (file(*at), parent.map(|p| file(joints[p].2)).unwrap_or([0.0; 3]));
                let children: Vec<usize> = joints.iter().enumerate().filter(|(_, j)| j.1 == Some(i)).map(|(k, _)| k).collect();
                json!({"name": name, "translation": [at[0] - from[0], at[1] - from[1], at[2] - from[2]], "children": children})
            })
            .collect();
        let extensions = if facing < 0.0 { json!({"VRM": {}}) } else { json!({}) };
        let root = json!({"scene": 0, "scenes": [{"nodes": [0]}], "nodes": nodes, "skins": [{"joints": (0..joints.len()).collect::<Vec<_>>()}], "extensions": extensions});
        let n = joints.len();
        Skeleton {
            nodes: joints
                .iter()
                .map(|(name, parent, at)| {
                    let (at, from) = (file(*at), parent.map(|p| file(joints[p].2)).unwrap_or([0.0; 3]));
                    Node { name: name.to_string(), parent: *parent, rest: Trs { t: Vec3::new(at[0] - from[0], at[1] - from[1], at[2] - from[2]), ..Default::default() } }
                })
                .collect(),
            order: spatiand_voice::figure::scene_order(&root),
            skins: vec![Skin { joints: (0..n).collect(), inverse_bind: vec![Mat4::IDENTITY; n] }],
            clips: Vec::new(),
            to_room: Mat4::IDENTITY,
            bodies: spatiand_voice::figure::bodies(&root),
            facing: spatiand_voice::figure::facing(&root),
        }
    }

    /// Where each joint is, posed by `pose`, by its name.
    fn posed(facing: f32, pose: Value) -> impl Fn(&str) -> Vec3 {
        let s = skeleton(facing);
        assert_eq!(s.bodies.len(), 1, "the made-up person is a body");
        let mut local = s.rest();
        apply(&s, &read(&pose), &mut local);
        let world = s.world(&local);
        let names: Vec<String> = s.nodes.iter().map(|n| n.name.clone()).collect();
        move |name: &str| world[names.iter().position(|n| n == name).unwrap()].w_axis.truncate()
    }

    #[test]
    fn an_upper_arm_points_where_it_is_told() {
        let at = posed(1.0, json!({"left_upper_arm": "up"}));
        let (shoulder, elbow) = (at("upper_arm.L"), at("forearm.L"));
        assert!((elbow - shoulder).normalize().dot(Vec3::Y) > 0.99, "{shoulder} {elbow}");
        // Out is away from the middle; the right arm is left as it was.
        let at = posed(1.0, json!({"right_upper_arm": "out"}));
        assert!((at("forearm.R") - at("upper_arm.R")).normalize().dot(-Vec3::X) > 0.99);
        assert!((at("forearm.L") - Vec3::new(0.37, 1.22, -0.01)).length() < 1e-4);
    }

    #[test]
    fn an_elbow_bends_forwards() {
        let at = posed(1.0, json!({"left_upper_arm": "down", "left_forearm": "front"}));
        let (elbow, wrist) = (at("forearm.L"), at("hand.L"));
        assert!((wrist - elbow).normalize().dot(Vec3::Z) > 0.99, "{elbow} {wrist}");
        // Out and up: a wave's arm, its elbow bent upwards by turning the upper arm about itself.
        let at = posed(1.0, json!({"left_upper_arm": "out", "left_forearm": "up"}));
        assert!((at("forearm.L") - at("upper_arm.L")).normalize().dot(Vec3::X) > 0.99);
        assert!((at("hand.L") - at("forearm.L")).normalize().dot(Vec3::Y) > 0.99);
        // Straightened, in line with the upper arm.
        let at = posed(1.0, json!({"left_upper_arm": "front", "left_forearm": "straight"}));
        assert!((at("hand.L") - at("forearm.L")).normalize().dot(Vec3::Z) > 0.99);
    }

    #[test]
    fn hands_on_hips_as_the_tool_says_it() {
        let at = posed(1.0, json!({"left_upper_arm": "down, out, a little back", "left_forearm": "in, a little down, a little front", "right_upper_arm": "down, out, a little back", "right_forearm": "in, a little down, a little front"}));
        for (wrist, side) in [(at("hand.L"), 1.0), (at("hand.R"), -1.0)] {
            // At the side of the waist, between the hips (0.95) and the chest (1.25).
            assert!((0.10..0.22).contains(&(wrist.x * side)), "{wrist}");
            assert!((1.0..1.2).contains(&wrist.y), "{wrist}");
            assert!(wrist.z.abs() < 0.08, "{wrist}");
        }
    }

    #[test]
    fn an_arm_across_the_chest_goes_in_front_of_it() {
        let at = posed(1.0, json!({"left_upper_arm": "in"}));
        let d = (at("forearm.L") - at("upper_arm.L")).normalize();
        assert!(d.z > 0.5 && d.x < 0.0, "{d}");
    }

    #[test]
    fn the_head_and_chest_turn_and_lean() {
        let s = skeleton(1.0);
        let head = s.nodes.iter().position(|n| n.name == "head").unwrap();
        let mut local = s.rest();
        apply(&s, &read(&json!({"head": "left"})), &mut local);
        let face = s.world(&local)[head].transform_vector3(Vec3::Z);
        assert!(face.x > 0.8, "{face}");
        let at = posed(1.0, json!({"chest": "front"}));
        assert!(at("neck").z > 0.1, "{}", at("neck"));
        let at = posed(1.0, json!({"chest": "a little left"}));
        assert!(at("neck").x > 0.02 && at("neck").x < 0.1, "{}", at("neck"));
        let at = posed(1.0, json!({"left_shoulder": "up"}));
        assert!(at("upper_arm.L").y > 1.45, "{}", at("upper_arm.L"));
    }

    #[test]
    fn a_knee_bends_backwards() {
        let at = posed(1.0, json!({"left_thigh": "front", "left_shin": "down"}));
        let (hip, knee, ankle) = (at("thigh.L"), at("shin.L"), at("foot.L"));
        assert!((knee - hip).normalize().dot(Vec3::Z) > 0.99, "{hip} {knee}");
        assert!((ankle - knee).normalize().dot(-Vec3::Y) > 0.99, "{knee} {ankle}");
        let at = posed(1.0, json!({"left_shin": "back"}));
        assert!(at("foot.L").z < at("shin.L").z - 0.3, "{}", at("foot.L"));
        // A knee is not lifted over the hip.
        let at = posed(1.0, json!({"left_thigh": "up"}));
        assert!((at("shin.L") - at("thigh.L")).normalize().y <= 0.51);
    }

    #[test]
    fn a_vrm_0_figure_is_posed_by_its_own_left() {
        // Facing -Z, its own left is -X.
        let at = posed(-1.0, json!({"left_upper_arm": "out", "head": "down"}));
        assert!((at("forearm.L") - at("upper_arm.L")).normalize().dot(-Vec3::X) > 0.99);
    }

    #[test]
    fn what_is_not_a_part_or_says_nothing_is_left_out() {
        let aims = read(&json!({"tail": "up", "left_hand": "on my hip", "head": "left", "chest": 3}));
        assert_eq!(aims.len(), 1);
        assert_eq!(aims[0].0, "head");
    }
}
