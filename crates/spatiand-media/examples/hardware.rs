//! Which video codecs this machine decodes in hardware, as pages will be told.
fn main() {
    for codec in ["avc1.64001e", "hvc1.1.6.L93.B0", "vp09.00.40.08", "av01.0.08M.08"] {
        println!("{codec}: {}", if spatiand_media::hardware::decoded_in_hardware(codec) { "hardware" } else { "software only" });
    }
}
