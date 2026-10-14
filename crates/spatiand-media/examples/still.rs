//! One picture out of a file by `still::grab`, as a library's tile is made: `still <file> <width>`.
fn main() {
    let mut args = std::env::args().skip(1);
    let (path, width) = (args.next().expect("which file?"), args.next().and_then(|w| w.parse().ok()).unwrap_or(320));
    match spatiand_media::still::grab(&path, width) {
        Some(still) => println!("{}x{}", still.width, still.height),
        None => println!("nothing"),
    }
}
