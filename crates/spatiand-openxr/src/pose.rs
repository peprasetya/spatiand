//! Where the head is, as an OpenXR application wants to ask for it.
//!
//! The compositor (or a host) writes samples into a shared-memory ring -- see
//! `spatiand_proto::pose` -- and this reads the whole ring back, in order, so that a pose can be
//! answered for any time an application names. `xrLocateViews` and `xrLocateSpace` are given a
//! time, and the ring holds a short history; between two samples the answer is interpolated, and
//! beyond the newest one it is the newest one, because the compositor has already predicted
//! forward to the display time before it wrote the sample.
//!
//! Everything here is in OpenXR's frame already (+X right, +Y up, -Z forward), so there is no
//! conversion to do and nothing in this file is specific to Spatiand's own world.

use glam::{Quat, Vec3};
use spatiand_proto::pose::{Header, Slot, SLOTS};

/// A rigid transform: `XrPosef`, with maths.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pose {
    pub orientation: Quat,
    pub position: Vec3,
}

impl Pose {
    pub const IDENTITY: Pose = Pose {
        orientation: Quat::IDENTITY,
        position: Vec3::ZERO,
    };

    pub fn new(orientation: [f32; 4], position: [f32; 3]) -> Pose {
        Pose {
            orientation: Quat::from_array(orientation).normalize(),
            position: Vec3::from_array(position),
        }
    }

    /// The pose of something that is `child` relative to this, expressed in this pose's parent.
    pub fn then(&self, child: &Pose) -> Pose {
        Pose {
            orientation: (self.orientation * child.orientation).normalize(),
            position: self.position + self.orientation * child.position,
        }
    }

    pub fn inverse(&self) -> Pose {
        let inverse = self.orientation.inverse();
        Pose {
            orientation: inverse,
            position: inverse * -self.position,
        }
    }

    /// Part-way from `self` to `other`; `t` is clamped to 0..=1.
    pub fn lerp(&self, other: &Pose, t: f32) -> Pose {
        let t = t.clamp(0.0, 1.0);
        Pose {
            orientation: self.orientation.slerp(other.orientation, t).normalize(),
            position: self.position.lerp(other.position, t),
        }
    }

    /// Keep only the turn about the vertical axis, so a space made from a head that was tilted
    /// when the session began is still level. OpenXR's LOCAL space is gravity-aligned.
    pub fn level(&self) -> Pose {
        let forward = self.orientation * Vec3::NEG_Z;
        let flat = Vec3::new(forward.x, 0.0, forward.z);
        let orientation = if flat.length_squared() < 1e-6 {
            // Looking straight up or down: there is no yaw to keep.
            Quat::IDENTITY
        } else {
            let flat = flat.normalize();
            Quat::from_rotation_y((-flat.x).atan2(-flat.z))
        };
        Pose {
            orientation,
            position: self.position,
        }
    }

    pub fn to_raw(&self) -> openxr_sys::Posef {
        openxr_sys::Posef {
            orientation: openxr_sys::Quaternionf {
                x: self.orientation.x,
                y: self.orientation.y,
                z: self.orientation.z,
                w: self.orientation.w,
            },
            position: openxr_sys::Vector3f {
                x: self.position.x,
                y: self.position.y,
                z: self.position.z,
            },
        }
    }

    pub fn from_raw(raw: &openxr_sys::Posef) -> Pose {
        let q = raw.orientation;
        let length = (q.x * q.x + q.y * q.y + q.z * q.z + q.w * q.w).sqrt();
        // An application that passes a zero quaternion for "no rotation" is wrong, but it is
        // common enough that crashing a headset's whole picture over it is not the answer.
        let orientation = if length < 1e-6 {
            Quat::IDENTITY
        } else {
            Quat::from_xyzw(q.x, q.y, q.z, q.w).normalize()
        };
        Pose {
            orientation,
            position: Vec3::new(raw.position.x, raw.position.y, raw.position.z),
        }
    }
}

impl Pose {
    pub fn matrix(&self) -> glam::Mat4 {
        glam::Mat4::from_rotation_translation(self.orientation, self.position)
    }
}

/// Eye space to Vulkan clip space, for a field of view given as OpenXR gives it: `angleLeft`,
/// `angleRight`, `angleUp`, `angleDown`, signed.
///
/// The eye looks down -Z with +Y up, as OpenXR's views do, and clip space is Vulkan's: x right,
/// **y down**, depth 0 to 1. Depth is held at one half, since nothing is depth-tested here, so
/// the only clipping is the sides and what is behind the eye (negative w).
pub fn clip_from_eye(fov: [f32; 4]) -> glam::Mat4 {
    let [left, right, up, down] = [fov[0].tan(), fov[1].tan(), fov[2].tan(), fov[3].tan()];
    let a = 2.0 / (right - left);
    let c = (right + left) / (right - left);
    let b = 2.0 / (up - down);
    let e = (up + down) / (up - down);
    glam::Mat4::from_cols(
        glam::Vec4::new(a, 0.0, 0.0, 0.0),
        glam::Vec4::new(0.0, -b, 0.0, 0.0),
        glam::Vec4::new(c, -e, -0.5, -1.0),
        glam::Vec4::ZERO,
    )
}

/// One eye: where it is, and what it sees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Eye {
    pub pose: Pose,
    /// angleLeft, angleRight, angleUp, angleDown; radians, signed. `XrFovf` exactly.
    pub fov: [f32; 4],
}

/// The head and both eyes at one moment.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sample {
    /// What the slot's `reserved` field carried: on a host, which viewport the headset sent, plus
    /// one. Handed back with a frame (`set_frame_pose`) to say which head it was drawn for. Zero
    /// where the writer does not use it.
    pub token: u32,
    pub sample_ns: i64,
    pub predicted_ns: i64,
    pub head: Pose,
    pub eyes: [Eye; 2],
}

impl Sample {
    pub fn from_slot(slot: &Slot) -> Sample {
        let eye = |i: usize| Eye {
            pose: Pose::new(slot.eye[i].orientation, slot.eye[i].position),
            fov: slot.eye[i].fov,
        };
        Sample {
            token: slot.reserved as u32,
            sample_ns: slot.sample_ns,
            predicted_ns: slot.predicted_ns,
            head: Pose::new(slot.head_orientation, slot.head_position),
            eyes: [eye(0), eye(1)],
        }
    }

    fn lerp(&self, other: &Sample, t: f32) -> Sample {
        let eye = |i: usize| Eye {
            pose: self.eyes[i].pose.lerp(&other.eyes[i].pose, t),
            // A field of view does not change between two samples of one session, and
            // interpolating four angles would only add rounding.
            fov: other.eyes[i].fov,
        };
        Sample {
            token: if t < 0.5 { self.token } else { other.token },
            sample_ns: lerp_i64(self.sample_ns, other.sample_ns, t),
            predicted_ns: lerp_i64(self.predicted_ns, other.predicted_ns, t),
            head: self.head.lerp(&other.head, t),
            eyes: [eye(0), eye(1)],
        }
    }
}

fn lerp_i64(a: i64, b: i64, t: f32) -> i64 {
    a + ((b - a) as f64 * t as f64) as i64
}

/// Ordered samples, oldest first.
#[derive(Debug, Clone, Default)]
pub struct History {
    samples: Vec<Sample>,
}

impl History {
    pub fn new(mut samples: Vec<Sample>) -> History {
        samples.sort_by_key(|s| s.sample_ns);
        History { samples }
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn samples(&self) -> &[Sample] {
        &self.samples
    }

    pub fn newest(&self) -> Option<&Sample> {
        self.samples.last()
    }

    /// The head and eyes as they were at `time_ns`: interpolated between the two samples either
    /// side of it, and the nearest end of the history if it falls outside.
    pub fn at(&self, time_ns: i64) -> Option<Sample> {
        let (first, last) = (self.samples.first()?, self.samples.last()?);
        if time_ns <= first.sample_ns {
            return Some(*first);
        }
        if time_ns >= last.sample_ns {
            return Some(*last);
        }
        for pair in self.samples.windows(2) {
            let (a, b) = (&pair[0], &pair[1]);
            if time_ns >= a.sample_ns && time_ns <= b.sample_ns {
                let span = (b.sample_ns - a.sample_ns) as f64;
                let t = if span <= 0.0 {
                    1.0
                } else {
                    ((time_ns - a.sample_ns) as f64 / span) as f32
                };
                return Some(a.lerp(b, t));
            }
        }
        Some(*last)
    }
}

/// The reader's half of the pose channel, over a mapping the caller keeps alive.
///
/// # Safety
/// `memory` must point at a mapping of at least `spatiand_proto::pose::channel_size()` bytes laid
/// out as the protocol says, and stay mapped for the call.
pub unsafe fn read_history(memory: *const u8) -> History {
    use std::sync::atomic::{fence, Ordering};
    let header = memory as *const Header;
    if std::ptr::addr_of!((*header).version).read_volatile() != spatiand_proto::pose::VERSION {
        return History::default();
    }
    let written = std::ptr::addr_of!((*header).write_index).read_volatile();
    let stride = std::ptr::addr_of!((*header).slot_stride).read_volatile() as usize;
    let offset = std::ptr::addr_of!((*header).slots_offset).read_volatile() as usize;
    // A writer built against a different layout would have bumped the version; a stride smaller
    // than the slot we know is a mapping not to be trusted rather than one to read past.
    if stride < std::mem::size_of::<Slot>() {
        return History::default();
    }
    let mut out = Vec::new();
    let count = written.min(SLOTS as u64);
    for back in 1..=count {
        let index = ((written - back) as usize) & (SLOTS - 1);
        let at = memory.add(offset + index * stride) as *const Slot;
        let before = std::ptr::addr_of!((*at).seq).read_volatile();
        fence(Ordering::Acquire);
        let body = std::ptr::read_volatile(at);
        fence(Ordering::Acquire);
        let after = std::ptr::addr_of!((*at).seq).read_volatile();
        // The writer was in this slot; the others are enough.
        if before % 2 == 0 && before == after {
            out.push(Sample::from_slot(&body));
        }
    }
    History::new(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    use spatiand_proto::pose::Ring;

    fn at(ns: i64, x: f32) -> Sample {
        let mut slot = Slot::default();
        slot.sample_ns = ns;
        slot.predicted_ns = ns + 10;
        slot.head_orientation = [0.0, 0.0, 0.0, 1.0];
        slot.head_position = [x, 0.0, 0.0];
        for eye in &mut slot.eye {
            eye.orientation = [0.0, 0.0, 0.0, 1.0];
            eye.position = [x, 0.0, 0.0];
            eye.fov = [-0.5, 0.5, 0.5, -0.5];
        }
        Sample::from_slot(&slot)
    }

    #[test]
    fn straight_ahead_is_the_middle_of_the_picture_and_the_edges_are_the_edges() {
        let fov = [-0.6f32, 0.5, 0.4, -0.3];
        let m = clip_from_eye(fov);
        let ndc = |p: glam::Vec3| {
            let c = m * p.extend(1.0);
            (c.x / c.w, c.y / c.w, c.z / c.w)
        };
        // On the axis of a symmetric view: dead centre.
        let sym = clip_from_eye([-0.5, 0.5, 0.5, -0.5]);
        let c = sym * glam::Vec4::new(0.0, 0.0, -2.0, 1.0);
        assert!((c.x / c.w).abs() < 1e-6 && (c.y / c.w).abs() < 1e-6);
        // The left edge of the field of view at a metre out is the left edge of the picture,
        // the right edge the right, and "up" is the top of a Vulkan picture, which is y = -1.
        let (lx, _, _) = ndc(glam::Vec3::new(fov[0].tan(), 0.0, -1.0));
        let (rx, _, _) = ndc(glam::Vec3::new(fov[1].tan(), 0.0, -1.0));
        let (_, uy, _) = ndc(glam::Vec3::new(0.0, fov[2].tan(), -1.0));
        let (_, dy, _) = ndc(glam::Vec3::new(0.0, fov[3].tan(), -1.0));
        assert!((lx + 1.0).abs() < 1e-5 && (rx - 1.0).abs() < 1e-5, "{lx} {rx}");
        assert!((uy + 1.0).abs() < 1e-5 && (dy - 1.0).abs() < 1e-5, "{uy} {dy}");
        // Something behind the eye is clipped, not drawn mirrored.
        let behind = m * glam::Vec4::new(0.0, 0.0, 1.0, 1.0);
        assert!(behind.w < 0.0);
        // And depth stays inside 0..w for anything in front.
        let (_, _, z) = ndc(glam::Vec3::new(0.1, 0.0, -3.0));
        assert!((0.0..=1.0).contains(&z));
    }

    #[test]
    fn a_time_between_samples_is_interpolated() {
        let history = History::new(vec![at(0, 0.0), at(100, 1.0)]);
        let mid = history.at(50).unwrap();
        assert!((mid.head.position.x - 0.5).abs() < 1e-5);
        assert_eq!(mid.sample_ns, 50);
    }

    #[test]
    fn a_time_past_the_newest_is_the_newest() {
        let history = History::new(vec![at(0, 0.0), at(100, 1.0)]);
        assert_eq!(history.at(500).unwrap().head.position.x, 1.0);
        assert_eq!(history.at(-500).unwrap().head.position.x, 0.0);
    }

    #[test]
    fn an_empty_history_has_no_pose() {
        assert!(History::default().at(0).is_none());
    }

    #[test]
    fn samples_are_ordered_whatever_order_they_were_read_in() {
        let history = History::new(vec![at(100, 1.0), at(0, 0.0)]);
        assert_eq!(history.newest().unwrap().sample_ns, 100);
    }

    #[test]
    fn a_pose_composed_with_its_inverse_is_nothing() {
        let pose = Pose::new([0.0, 0.38268343, 0.0, 0.9238795], [1.0, 2.0, 3.0]);
        let back = pose.then(&pose.inverse());
        assert!(back.position.length() < 1e-5);
        assert!(back.orientation.dot(Quat::IDENTITY).abs() > 0.9999);
    }

    #[test]
    fn levelling_keeps_the_turn_and_drops_the_tilt() {
        // Yawed 90 degrees left, then pitched up 30: forward is no longer horizontal.
        let q = Quat::from_rotation_y(std::f32::consts::FRAC_PI_2) * Quat::from_rotation_x(0.5);
        let level = Pose { orientation: q, position: Vec3::ZERO }.level();
        let forward = level.orientation * Vec3::NEG_Z;
        assert!(forward.y.abs() < 1e-5, "level forward is horizontal");
        assert!((forward.x + 1.0).abs() < 1e-4, "still facing the way it was turned");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn the_ring_reads_back_in_order_with_its_whole_history() {
        let mut ring = Ring::new(c"test-openxr-poses").expect("ring");
        for n in 0..(SLOTS as i64 + 5) {
            let mut slot = Slot::default();
            slot.sample_ns = n * 1000;
            slot.head_orientation = [0.0, 0.0, 0.0, 1.0];
            slot.eye[0].orientation = [0.0, 0.0, 0.0, 1.0];
            slot.eye[1].orientation = [0.0, 0.0, 0.0, 1.0];
            ring.write(slot);
        }
        // SAFETY: a mapping of the ring made above, via the writer's own pointer.
        let history = unsafe { read_history(ring_memory(&ring)) };
        assert_eq!(history.samples.len(), SLOTS);
        assert_eq!(history.newest().unwrap().sample_ns, (SLOTS as i64 + 4) * 1000);
        assert!(history.samples.windows(2).all(|w| w[0].sample_ns < w[1].sample_ns));
    }

    /// Map the ring's own descriptor read-only, the way a client would.
    #[cfg(target_os = "linux")]
    fn ring_memory(ring: &Ring) -> *const u8 {
        use std::os::fd::AsRawFd;
        // SAFETY: mapping a descriptor that is alive for the whole test; never unmapped, which
        // is fine for a test process.
        let memory = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                spatiand_proto::pose::channel_size(),
                libc::PROT_READ,
                libc::MAP_SHARED,
                ring.fd().as_raw_fd(),
                0,
            )
        };
        assert_ne!(memory, libc::MAP_FAILED);
        memory as *const u8
    }
}
