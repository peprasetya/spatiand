//! The device table.
//!
//! Parsed once from a compiled-in `devices.toml`, so adding hardware needs no runtime file
//! but stays a data edit rather than a code edit.

use serde::Deserialize;
use std::sync::OnceLock;

/// A direction in the wearer's head frame, used to say where a sensor axis points.
///
/// The head frame is the canonical one: forward is the nose, left is the left ear, up is the
/// top of the head.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum HeadDirection {
    Forward,
    Back,
    Left,
    Right,
    Up,
    Down,
}

/// Which head direction each of the IMU's three axes points along, in order X, Y, Z.
///
/// This is the IMU's physical mounting inside the glasses — where the chip sits on the board
/// and which way round the board is. It is a property of the product, identical across every
/// unit of it, and it cannot change between wearers or between sessions.
///
/// Stating it this way rather than as a ready-made axis map is deliberate. A map is six
/// numbers with a sign convention folded in, which nobody can check by looking; a mounting is
/// a claim about the physical world that anyone can verify in a minute with an accelerometer
/// and a flat table. `AxisMap::from_mounting` turns the checkable statement into the
/// convention-laden one, and is tested against the measurement this row came from.
pub type Mounting = [HeadDirection; 3];

#[derive(Debug, Clone, PartialEq, Deserialize)]
pub struct DeviceSpec {
    pub name: String,
    pub driver: String,
    pub vid: u16,
    pub pid: u16,
    /// Whether this row has been exercised on real hardware. Unverified rows still work,
    /// but the driver logs a warning so a wrong table entry is visible rather than baffling.
    pub verified: bool,
    pub imu_interface: u8,
    pub mcu_interface: u8,
    pub imu_report_len: usize,
    pub per_eye: (u32, u32),
    pub h_fov_deg: f64,
    pub default_ipd_mm: f64,
    pub mode_mono: u8,
    pub mode_stereo: u8,
    pub mode_stereo_fallback: u8,
    /// Where the IMU's X, Y and Z axes point in the head frame, if it has been measured on
    /// real hardware.
    ///
    /// `None` means nobody has held one of these level and looked at the accelerometer, so
    /// the wearer has to calibrate. Present means the answer is known and calibration is not
    /// a per-user question at all — see [`Mounting`].
    #[serde(default)]
    pub sensor_axes: Option<Mounting>,
    /// The display's EDID manufacturer ID and product code, if read off real hardware: how a
    /// platform that sees several screens picks out the glasses' (see devices.toml).
    #[serde(default)]
    pub edid: Option<(u16, u16)>,
}

#[derive(Debug, Deserialize)]
struct DeviceFile {
    device: Vec<DeviceSpec>,
}

static DB: OnceLock<Vec<DeviceSpec>> = OnceLock::new();

/// Every known device.
pub fn all() -> &'static [DeviceSpec] {
    DB.get_or_init(|| {
        let raw = include_str!("devices.toml");
        // A malformed table is a build-time authoring error, not a runtime condition worth
        // degrading over — the message names the file so it is obvious what to fix.
        let parsed: DeviceFile =
            toml::from_str(raw).expect("devices.toml is malformed; fix the table");
        parsed.device
    })
}

pub fn lookup(vid: u16, pid: u16) -> Option<&'static DeviceSpec> {
    all().iter().find(|d| d.vid == vid && d.pid == pid)
}

/// The headset whose display says it is this EDID manufacturer and product.
pub fn by_display(edid_vendor: u16, edid_product: u16) -> Option<&'static DeviceSpec> {
    all().iter().find(|d| d.edid == Some((edid_vendor, edid_product)))
}

/// Every driver the table names, each once, in the table's order.
pub fn drivers() -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for d in all() {
        if !out.contains(&d.driver.as_str()) {
            out.push(d.driver.as_str());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Mac's and the Beam Pro's shells have the table as generated files
    /// (tools/gen-headsets.py): each row there, the same as here.
    #[test]
    fn generated_shell_tables_match() {
        let swift = include_str!("../../../mac/Sources/Spatiand/Headsets.swift");
        let java = include_str!("../../../android/src/id/prasetya/spatiand/Headsets.java");
        let xml = include_str!("../../../android/res/xml/glasses.xml");
        let rerun = "devices.toml changed: run tools/gen-headsets.py";
        for d in all() {
            let usb = format!("(vendor: 0x{:04x}, product: 0x{:04x}, name: \"{}\")", d.vid, d.pid, d.name);
            assert!(swift.contains(&usb), "{rerun} ({} in Headsets.swift)", d.name);
            assert!(java.contains(&format!("{{0x{:04x}, 0x{:04x}}}, // {}", d.vid, d.pid, d.name)), "{rerun} ({} in Headsets.java)", d.name);
            assert!(xml.contains(&format!("vendor-id=\"{}\" product-id=\"{}\"", d.vid, d.pid)), "{rerun} ({} in glasses.xml)", d.name);
            if let Some((vendor, product)) = d.edid {
                let display = format!("(vendor: 0x{vendor:04x}, product: 0x{product:04x}, name: \"{}\")", d.name);
                assert!(swift.matches(&display).count() == 1, "{rerun} ({}'s display in Headsets.swift)", d.name);
            }
        }
        assert_eq!(swift.matches("(vendor: 0x").count(), all().len() + all().iter().filter(|d| d.edid.is_some()).count(), "{rerun}");
        assert_eq!(java.matches(", // ").count(), all().len(), "{rerun}");
    }

    #[test]
    fn the_air_s_display_is_known() {
        assert_eq!(by_display(0x3647, 0x3132).map(|d| d.name.as_str()), Some("XREAL Air"));
        assert_eq!(drivers(), vec!["xreal"]);
    }

    #[test]
    fn table_parses_and_has_the_verified_device() {
        let air = lookup(0x3318, 0x0424).expect("XREAL Air must be in the table");
        assert_eq!(air.name, "XREAL Air");
        assert!(
            air.verified,
            "the Air gen 1 is the one we confirmed on hardware"
        );
        assert_eq!((air.imu_interface, air.mcu_interface), (3, 4));
        // 0x04 is 3840x1080@72 — confirmed working on the Deck. Guard against a careless
        // edit silently downgrading everyone to 60 Hz.
        assert_eq!(air.mode_stereo, 0x04);
    }

    #[test]
    fn no_duplicate_ids() {
        let mut seen = std::collections::HashSet::new();
        for d in all() {
            assert!(seen.insert((d.vid, d.pid)), "duplicate id for {}", d.name);
        }
    }
}
