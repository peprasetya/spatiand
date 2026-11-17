//! Where the wearer's head and eyes are: the room's pose channel.
//!
//! An application that draws its own two eye views -- [`crate::Layer::Projection`], with an eye
//! layout other than mono -- has to know where the head is at the last moment before it draws,
//! or what it draws swims against the room when the head moves. The room does not send poses
//! as events, which would be old on arrival: it writes them into a small ring of shared memory
//! (`spatiand_xr_pose_channel_v1`, whose description is the specification followed here) and
//! the application reads the newest whenever it likes. Here that is [`crate::App::pose`], a
//! read of memory and nothing sent.
//!
//! ```no_run
//! # use spatiand_app::{App, Event, EyeLayout, Layer};
//! # let mut app = App::connect("id.prasetya.spatiand.example")?;
//! # let window = app.open("Example", (1920, 540));
//! app.set_eyes(window, EyeLayout::SideBySide, false);
//! app.set_layer(window, Layer::Projection);
//! app.ask_poses();
//! // ... on each Event::Frame:
//! if let Some(pose) = app.pose() {
//!     app.set_frame_pose(window, Some(&pose));
//!     app.draw(window, |gl, size| {
//!         for (eye, _half) in pose.eyes.iter().zip(0..) {
//!             let _from_world = spatiand_app::pose::multiply(&eye.projection(0.05, 100.0), &eye.view());
//!             // ... draw the eye's half with it
//!         }
//!     })?;
//! }
//! # Ok::<(), String>(())
//! ```
//!
//! **The frame is OpenXR's**, as the protocol's is: +X right, +Y up, -Z forward, right-handed,
//! metres, quaternions in (x, y, z, w) order, fields of view as `XrFovf`. Times are
//! `CLOCK_MONOTONIC` nanoseconds ([`now_ns`]), the clock an [`crate::Event::Frame`]'s time
//! comes from.
//!
//! The same on the Deck, the Beam Pro and the Mac: the memory is the room's on all three (a
//! sealed memfd on the first two, POSIX shared memory on a Mac), handed over the connection as
//! a descriptor and mapped here for reading only.

use std::os::fd::OwnedFd;
use std::sync::atomic::{fence, Ordering};

use spatiand_proto::pose::{Header, Slot};

/// One eye at one moment: where it is, and what it sees.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Eye {
    /// x, y, z, w.
    pub orientation: [f32; 4],
    /// Metres, from the tracking origin.
    pub position: [f32; 3],
    /// The angles to the left, right, upper and lower edges of what the eye sees, in radians,
    /// signed: left and down are negative. `XrFovf` exactly, and not symmetric.
    pub fov: [f32; 4],
}

/// The head and both eyes at one moment, as the room wrote them.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HeadPose {
    /// A number the room's slot carried, to be handed back with the picture drawn from this
    /// pose ([`crate::App::set_frame_pose`] does): a room that only passes pictures on, to a
    /// headset across a network, names the head by it. Zero where the room has none to give.
    pub token: u32,
    /// When the pose was measured.
    pub sample_ns: i64,
    /// When a picture handed over now is expected to be shown.
    pub predicted_ns: i64,
    /// x, y, z, w.
    pub orientation: [f32; 4],
    /// Metres, from the tracking origin.
    pub position: [f32; 3],
    /// The left eye, then the right.
    pub eyes: [Eye; 2],
}

/// What the room has said of its poses.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum Poses {
    /// Not asked for ([`crate::App::ask_poses`]), or stopped.
    #[default]
    NotAsked,
    /// Asked for, and the room has not answered yet.
    Waiting,
    /// The room has none to give, and why (for a log, not for the wearer): no glasses are
    /// being tracked, or this is not Spatiand's room at all. Not an error, and not final: they
    /// come ([`crate::Event::Poses`]) if glasses appear. An application carries on as a flat
    /// window meanwhile.
    Unavailable(String),
    /// They are there to be read: [`crate::App::pose`].
    Here,
}

/// `CLOCK_MONOTONIC`, in nanoseconds: the clock a pose's times are on.
pub fn now_ns() -> i64 {
    let mut ts = libc::timespec { tv_sec: 0, tv_nsec: 0 };
    // SAFETY: a libc call filling a struct of ours.
    unsafe { libc::clock_gettime(libc::CLOCK_MONOTONIC, &mut ts) };
    ts.tv_sec as i64 * 1_000_000_000 + ts.tv_nsec as i64
}

impl HeadPose {
    fn from_slot(slot: &Slot) -> HeadPose {
        let eye = |i: usize| Eye { orientation: slot.eye[i].orientation, position: slot.eye[i].position, fov: slot.eye[i].fov };
        HeadPose {
            token: slot.reserved as u32,
            sample_ns: slot.sample_ns,
            predicted_ns: slot.predicted_ns,
            orientation: slot.head_orientation,
            position: slot.head_position,
            eyes: [eye(0), eye(1)],
        }
    }

    /// World to head: what turns a point of the room into the head's own frame. Column-major,
    /// as GL takes a matrix.
    pub fn view(&self) -> [f32; 16] {
        view(self.orientation, self.position)
    }
}

impl Eye {
    /// World to eye: what turns a point of the room into this eye's own frame, in which it
    /// looks down -Z with +Y up. Column-major, as GL takes a matrix.
    pub fn view(&self) -> [f32; 16] {
        view(self.orientation, self.position)
    }

    /// Eye to clip space for what this eye sees, between `near` and `far` metres, **for a
    /// picture drawn by [`crate::App::draw`]**, whose row 0 is the top: up in the room is
    /// towards row 0, which GL calls the bottom, so this turns the picture over where GL's
    /// usual projection would not -- and with it the winding of triangles, for whoever culls
    /// faces. Column-major.
    pub fn projection(&self, near: f32, far: f32) -> [f32; 16] {
        let [left, right, up, down] = self.fov.map(f32::tan);
        let (width, height) = (right - left, up - down);
        let mut m = [0.0f32; 16];
        m[0] = 2.0 / width;
        m[5] = -2.0 / height;
        m[8] = (right + left) / width;
        m[9] = -(up + down) / height;
        m[10] = -(far + near) / (far - near);
        m[11] = -1.0;
        m[14] = -2.0 * far * near / (far - near);
        m
    }
}

/// `a` after `b`, both column-major: what [`Eye::projection`] and [`Eye::view`] are joined with.
pub fn multiply(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut out = [0.0f32; 16];
    for column in 0..4 {
        for row in 0..4 {
            out[column * 4 + row] = (0..4).map(|k| a[k * 4 + row] * b[column * 4 + k]).sum();
        }
    }
    out
}

/// A direction turned by a quaternion (x, y, z, w).
pub fn rotate(q: [f32; 4], v: [f32; 3]) -> [f32; 3] {
    let (u, w) = ([q[0], q[1], q[2]], q[3]);
    let cross = |a: [f32; 3], b: [f32; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let t = cross(u, v).map(|c| 2.0 * c);
    let ut = cross(u, t);
    [v[0] + w * t[0] + ut[0], v[1] + w * t[1] + ut[1], v[2] + w * t[2] + ut[2]]
}

/// The inverse of a pose, as a matrix: the rotation's transpose, and the position taken back
/// through it.
fn view(orientation: [f32; 4], position: [f32; 3]) -> [f32; 16] {
    let length = orientation.iter().map(|c| c * c).sum::<f32>().sqrt();
    // A quaternion of nothing is a writer that had no head yet: no turn at all.
    let q = if length < 1e-6 { [0.0, 0.0, 0.0, 1.0] } else { orientation.map(|c| c / length) };
    let inverse = [-q[0], -q[1], -q[2], q[3]];
    let x = rotate(inverse, [1.0, 0.0, 0.0]);
    let y = rotate(inverse, [0.0, 1.0, 0.0]);
    let z = rotate(inverse, [0.0, 0.0, 1.0]);
    let t = rotate(inverse, position.map(|c| -c));
    [x[0], x[1], x[2], 0.0, y[0], y[1], y[2], 0.0, z[0], z[1], z[2], 0.0, t[0], t[1], t[2], 1.0]
}

/// The ring, mapped for reading. The memory is the room's and is being written while it is
/// read: nothing here ever makes a reference into it, only volatile reads, and what a read
/// returns is believed only when the slot's sequence says the writer was not in it.
pub(crate) struct Channel {
    map: memmap2::Mmap,
    /// The header's word on the ring, checked once against the bytes that are mapped.
    slots: usize,
    stride: usize,
    offset: usize,
}

impl Channel {
    /// Map what the room handed over (`channel`: a descriptor and the bytes to map), read-only
    /// and shared, as its description says to.
    pub(crate) fn map(fd: OwnedFd, size: u32) -> Result<Channel, String> {
        let size = size as usize;
        if size < std::mem::size_of::<Header>() {
            return Err(format!("the pose channel is {size} bytes, less than its own header"));
        }
        // Mapping past the end of what the descriptor holds maps, and then faults when read.
        // (A Mac says a shared memory's length rounded up to whole pages, which is no less.)
        let held = std::fs::File::from(fd);
        let length = held.metadata().map_err(|e| format!("the pose channel cannot be measured: {e}"))?.len();
        if length < size as u64 {
            return Err(format!("the pose channel is said to be {size} bytes and holds {length}"));
        }
        // SAFETY: a mapping of a descriptor the room sealed against shrinking (where the
        // system can: a Mac's cannot be, and is the room's own, never resized); only read
        // through volatile reads below.
        let map = unsafe { memmap2::MmapOptions::new().len(size).map(&held) }.map_err(|e| format!("the pose channel cannot be mapped: {e}"))?;
        let header = map.as_ptr() as *const Header;
        // SAFETY: the mapping is at least a header long, and page-aligned.
        let (version, slots, stride, offset) = unsafe {
            (
                std::ptr::addr_of!((*header).version).read_volatile(),
                std::ptr::addr_of!((*header).slot_count).read_volatile() as usize,
                std::ptr::addr_of!((*header).slot_stride).read_volatile() as usize,
                std::ptr::addr_of!((*header).slots_offset).read_volatile() as usize,
            )
        };
        // "Bumped if the layout ever changes. A client must check it."
        if version != spatiand_proto::pose::VERSION {
            return Err(format!("the pose channel's memory is version {version}, and this reads version {}", spatiand_proto::pose::VERSION));
        }
        // A ring that is not what the protocol says is one not to be read, rather than read
        // past: a count that is no power of two cannot be masked, a slot shorter than the one
        // known has fields missing, and slots must lie whole inside what is mapped, aligned
        // for their 64-bit fields.
        let whole = slots.checked_mul(stride).and_then(|ring| ring.checked_add(offset)).is_some_and(|end| end <= size);
        if !slots.is_power_of_two()
            || stride < std::mem::size_of::<Slot>()
            || stride % std::mem::align_of::<Slot>() != 0
            || offset < std::mem::size_of::<Header>()
            || offset % std::mem::align_of::<Slot>() != 0
            || !whole
        {
            return Err(format!("the pose channel's ring is not one this can read ({slots} slots of {stride} bytes at {offset}, in {size})"));
        }
        Ok(Channel { map, slots, stride, offset })
    }

    fn written(&self) -> u64 {
        let header = self.map.as_ptr() as *const Header;
        // SAFETY: inside the mapping, as checked when it was made.
        unsafe { std::ptr::addr_of!((*header).write_index).read_volatile() }
    }

    /// The slot written `back` before the next to be written, if the writer was not in it:
    /// its sequence read, its body read, its sequence read again, and the body believed only
    /// if both were the same even number.
    fn slot(&self, written: u64, back: u64) -> Option<Slot> {
        let index = (written.wrapping_sub(back) as usize) & (self.slots - 1);
        // SAFETY: `index` is masked into the ring, whose slots lie whole inside the mapping
        // and are aligned (`map`).
        unsafe {
            let at = self.map.as_ptr().add(self.offset + index * self.stride) as *const Slot;
            let before = std::ptr::addr_of!((*at).seq).read_volatile();
            fence(Ordering::Acquire);
            let body = std::ptr::read_volatile(at);
            fence(Ordering::Acquire);
            let after = std::ptr::addr_of!((*at).seq).read_volatile();
            (before % 2 == 0 && before == after).then_some(body)
        }
    }

    /// The newest pose written whole. None before the room has written any, or if the writer
    /// was in every slot tried, which an application treats alike: the pose it had.
    pub(crate) fn newest(&self) -> Option<HeadPose> {
        // The protocol: take the newest slot; if the writer was in it, "step back one slot or
        // try again". Both: the one before, then from the start, since by then there is a
        // newer.
        for _ in 0..4 {
            let written = self.written();
            if written == 0 {
                return None;
            }
            for back in 1..=written.min(2) {
                if let Some(slot) = self.slot(written, back) {
                    return Some(HeadPose::from_slot(&slot));
                }
            }
        }
        None
    }

    /// Every pose the ring still holds, the oldest first: a little history, to interpolate in.
    pub(crate) fn history(&self) -> Vec<HeadPose> {
        let written = self.written();
        let mut poses: Vec<HeadPose> =
            (1..=written.min(self.slots as u64)).filter_map(|back| self.slot(written, back)).map(|slot| HeadPose::from_slot(&slot)).collect();
        // Read newest first, and the writer may have gone round under the reading.
        poses.sort_by_key(|p| p.sample_ns);
        poses.dedup_by_key(|p| p.sample_ns);
        poses
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use spatiand_proto::pose::{channel_size, Ring, SLOTS};

    fn sample(n: i64) -> Slot {
        let mut slot = Slot { sample_ns: n, predicted_ns: n + 7, reserved: n + 1, head_orientation: [0.0, 0.0, 0.0, 1.0], ..Slot::default() };
        slot.head_position = [n as f32, 1.6, 0.0];
        for (i, eye) in slot.eye.iter_mut().enumerate() {
            eye.orientation = [0.0, 0.0, 0.0, 1.0];
            eye.position = [i as f32 * 0.064 - 0.032, 1.6, 0.0];
            eye.fov = [-0.4, 0.3, 0.2, -0.25];
        }
        slot
    }

    /// What a client is handed: the ring's read-only descriptor, and its size.
    fn handed(ring: &Ring) -> Channel {
        Channel::map(ring.read_only().expect("a read-only descriptor"), ring.size()).expect("mapped")
    }

    #[test]
    fn nothing_written_is_no_pose() {
        let ring = Ring::new(c"app-poses").expect("ring");
        let channel = handed(&ring);
        assert_eq!(channel.newest(), None);
        assert!(channel.history().is_empty());
    }

    #[test]
    fn the_newest_is_read_as_the_room_wrote_it_and_goes_on_being() {
        let mut ring = Ring::new(c"app-poses").expect("ring");
        let channel = handed(&ring);
        ring.write(sample(5));
        let pose = channel.newest().expect("written");
        assert_eq!((pose.sample_ns, pose.predicted_ns, pose.token), (5, 12, 6));
        assert_eq!(pose.position, [5.0, 1.6, 0.0]);
        assert_eq!(pose.eyes[1].position[0], 0.032);
        assert_eq!(pose.eyes[0].fov, [-0.4, 0.3, 0.2, -0.25]);
        // The mapping is the room's memory, not a copy of it.
        ring.write(sample(6));
        assert_eq!(channel.newest().expect("written").sample_ns, 6);
    }

    #[test]
    fn the_ring_goes_round_and_its_history_is_in_order() {
        let mut ring = Ring::new(c"app-poses").expect("ring");
        let channel = handed(&ring);
        for n in 0..3 {
            ring.write(sample(n));
        }
        assert_eq!(channel.history().iter().map(|p| p.sample_ns).collect::<Vec<_>>(), [0, 1, 2]);
        for n in 3..(SLOTS as i64 * 2 + 3) {
            ring.write(sample(n));
        }
        let history = channel.history();
        assert_eq!(history.len(), SLOTS);
        assert_eq!(history.last().unwrap().sample_ns, SLOTS as i64 * 2 + 2);
        assert!(history.windows(2).all(|w| w[0].sample_ns + 1 == w[1].sample_ns));
        assert_eq!(channel.newest().unwrap().sample_ns, SLOTS as i64 * 2 + 2);
    }

    #[test]
    fn a_pose_read_while_the_room_writes_is_never_half_of_one() {
        let mut ring = Ring::new(c"app-poses").expect("ring");
        let channel = handed(&ring);
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let writer = {
            let stop = stop.clone();
            std::thread::spawn(move || {
                let mut n = 0i64;
                while !stop.load(Ordering::Relaxed) {
                    ring.write(sample(n % 4096));
                    n += 1;
                }
                n
            })
        };
        let whole = |p: &HeadPose| p.predicted_ns == p.sample_ns + 7 && p.token as i64 == p.sample_ns + 1 && p.position[0] == p.sample_ns as f32;
        let (mut read, mut none) = (0u32, 0u32);
        let until = std::time::Instant::now() + std::time::Duration::from_millis(300);
        while std::time::Instant::now() < until {
            match channel.newest() {
                Some(pose) => {
                    assert!(whole(&pose), "half of a pose: {pose:?}");
                    read += 1;
                }
                None => none += 1,
            }
            assert!(channel.history().iter().all(whole));
        }
        stop.store(true, Ordering::Relaxed);
        let written = writer.join().expect("the writer");
        assert!(read > 100 && written > 100, "{read} read ({none} not), {written} written");
    }

    /// A ring in a file, to write what the room's writer never would.
    fn forged(change: impl FnOnce(&mut [u8])) -> Result<Channel, String> {
        let mut ring = Ring::new(c"app-poses").expect("ring");
        ring.write(sample(1));
        ring.write(sample(2));
        let size = channel_size();
        let mut bytes = vec![0u8; size];
        // SAFETY: the ring's own descriptor, mapped for as long as the copy takes.
        unsafe {
            use std::os::fd::AsRawFd;
            let from = libc::mmap(std::ptr::null_mut(), size, libc::PROT_READ, libc::MAP_SHARED, ring.fd().as_raw_fd(), 0);
            assert_ne!(from, libc::MAP_FAILED);
            std::ptr::copy_nonoverlapping(from as *const u8, bytes.as_mut_ptr(), size);
            libc::munmap(from, size);
        }
        change(&mut bytes);
        static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
        let path = std::env::temp_dir().join(format!("spatiand-app-pose-test-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        std::fs::write(&path, &bytes).expect("written");
        let file = std::fs::File::open(&path).expect("opened");
        let _ = std::fs::remove_file(&path);
        Channel::map(file.into(), size as u32)
    }

    #[test]
    fn a_slot_the_writer_is_in_is_stepped_back_from() {
        // The newest slot (the second written) with an odd sequence: being written.
        let slot_at = |i: usize| std::mem::size_of::<Header>() + i * std::mem::size_of::<Slot>();
        let channel = forged(|bytes| bytes[slot_at(1)..slot_at(1) + 8].copy_from_slice(&5u64.to_le_bytes())).expect("mapped");
        assert_eq!(channel.newest().expect("the one before").sample_ns, 1);
        assert_eq!(channel.history().len(), 1);
        // Both in the writer's hands: no pose, and no waiting for one.
        let channel = forged(|bytes| {
            bytes[slot_at(0)..slot_at(0) + 8].copy_from_slice(&1u64.to_le_bytes());
            bytes[slot_at(1)..slot_at(1) + 8].copy_from_slice(&3u64.to_le_bytes());
        })
        .expect("mapped");
        assert_eq!(channel.newest(), None);
    }

    #[test]
    fn a_ring_that_is_not_the_protocols_is_not_read() {
        // Another version of the memory.
        assert!(forged(|bytes| bytes[0..4].copy_from_slice(&2u32.to_le_bytes())).is_err());
        // A count that cannot be masked.
        assert!(forged(|bytes| bytes[4..8].copy_from_slice(&12u32.to_le_bytes())).is_err());
        // More slots than are mapped.
        assert!(forged(|bytes| bytes[4..8].copy_from_slice(&1024u32.to_le_bytes())).is_err());
        // Slots shorter than a slot.
        assert!(forged(|bytes| bytes[8..12].copy_from_slice(&64u32.to_le_bytes())).is_err());
        // Slots that begin inside the header, or off their alignment.
        assert!(forged(|bytes| bytes[12..16].copy_from_slice(&8u32.to_le_bytes())).is_err());
        assert!(forged(|bytes| bytes[12..16].copy_from_slice(&36u32.to_le_bytes())).is_err());
        // And the ring as the room writes it is read.
        assert_eq!(forged(|_| {}).expect("mapped").newest().expect("written").sample_ns, 2);
    }

    #[test]
    fn a_descriptor_shorter_than_it_is_said_to_be_is_not_mapped() {
        let ring = Ring::new(c"app-poses").expect("ring");
        let fd = ring.read_only().expect("a read-only descriptor");
        assert!(Channel::map(fd, ring.size() + (1 << 20)).is_err());
    }

    #[test]
    fn what_is_handed_over_cannot_be_written_through() {
        // The mapping is read-only by its type; and the descriptor would not have allowed more.
        let ring = Ring::new(c"app-poses").expect("ring");
        let fd = ring.read_only().expect("a read-only descriptor");
        assert!(unsafe { memmap2::MmapOptions::new().len(ring.size() as usize).map_mut(&std::fs::File::from(fd)) }.is_err());
    }

    fn apply(m: &[f32; 16], p: [f32; 3]) -> [f32; 4] {
        let mut out = [0.0f32; 4];
        for (row, value) in out.iter_mut().enumerate() {
            *value = m[row] * p[0] + m[4 + row] * p[1] + m[8 + row] * p[2] + m[12 + row];
        }
        out
    }

    #[test]
    fn a_view_puts_what_is_ahead_of_the_eye_down_its_minus_z() {
        // Turned a quarter to the left (about +Y), a metre up: what was a metre to its left
        // (-X) is now straight ahead.
        let half = std::f32::consts::FRAC_PI_4;
        let eye = Eye { orientation: [0.0, half.sin(), 0.0, half.cos()], position: [0.0, 1.0, 0.0], fov: [-0.5, 0.5, 0.4, -0.4] };
        let seen = apply(&eye.view(), [-1.0, 1.0, 0.0]);
        assert!(seen[0].abs() < 1e-5 && seen[1].abs() < 1e-5 && (seen[2] + 1.0).abs() < 1e-5, "{seen:?}");
    }

    #[test]
    fn a_projection_puts_the_edges_of_the_view_on_the_edges_of_the_picture_with_up_at_row_0() {
        let eye = Eye { orientation: [0.0, 0.0, 0.0, 1.0], position: [0.0; 3], fov: [-0.6, 0.5, 0.4, -0.3] };
        let m = eye.projection(0.1, 100.0);
        let ndc = |p: [f32; 3]| {
            let c = apply(&m, p);
            [c[0] / c[3], c[1] / c[3], c[2] / c[3]]
        };
        assert!((ndc([(-0.6f32).tan(), 0.0, -1.0])[0] + 1.0).abs() < 1e-5);
        assert!((ndc([(0.5f32).tan(), 0.0, -1.0])[0] - 1.0).abs() < 1e-5);
        // Up is GL's bottom, which is the picture's row 0.
        assert!((ndc([0.0, (0.4f32).tan(), -1.0])[1] + 1.0).abs() < 1e-5);
        assert!((ndc([0.0, (-0.3f32).tan(), -1.0])[1] - 1.0).abs() < 1e-5);
        // Near and far are the ends of the depth.
        assert!((ndc([0.0, 0.0, -0.1])[2] + 1.0).abs() < 1e-4);
        assert!((ndc([0.0, 0.0, -100.0])[2] - 1.0).abs() < 1e-4);
        // Behind the eye is clipped, not drawn mirrored.
        assert!(apply(&m, [0.0, 0.0, 1.0])[3] < 0.0);
    }

    #[test]
    fn matrices_multiply_in_the_order_they_are_named() {
        let eye = Eye { orientation: [0.0, 0.0, 0.0, 1.0], position: [1.0, 2.0, 3.0], fov: [-0.5, 0.5, 0.5, -0.5] };
        let joined = multiply(&eye.projection(0.1, 10.0), &eye.view());
        let point = [1.2, 2.1, 1.0];
        let seen = apply(&eye.view(), point);
        let expected = apply(&eye.projection(0.1, 10.0), [seen[0], seen[1], seen[2]]);
        let got = apply(&joined, point);
        assert!(expected.iter().zip(got).all(|(a, b)| (a - b).abs() < 1e-5), "{expected:?} {got:?}");
    }
}
