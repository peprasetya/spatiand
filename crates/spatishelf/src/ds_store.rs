//! Reading a Mac's `.DS_Store`, for one thing: where Finder says each thing in a Trash came from
//! (its `ptbL` record, the folder, and `ptbN`, the name), so that Put back works for what Finder
//! put there as for what SpatiShelf did.
//!
//! The format, as Mozilla's and Wim Lewis's notes and the Python `ds_store` package describe it
//! (all big-endian):
//!
//! * The file: `00 00 00 01`, `Bud1`, then where the allocator's root block is (offset, size, the
//!   offset again), sixteen bytes unknown. Every offset in the file is from byte 4.
//! * The root block: how many blocks there are, four bytes, then each block's address -- its
//!   offset with the low five bits giving its size as a power of two -- the list padded with
//!   zeroes to a multiple of 256; then a table of contents (a byte's length, a name, a block
//!   number), whose `DSDB` is the B-tree; then the free lists, which a reader has no use for.
//! * `DSDB`: the root node's block, how many levels of internal nodes, records, nodes, the page
//!   size.
//! * A node: P, a count. P zero: a leaf of `count` records. Otherwise `count` pairs of a child's
//!   block and a record, P being the last child.
//! * A record: the file's name (a count of UTF-16 units, then those), a four-letter code, a
//!   four-letter type and its value: `long` and `shor` four bytes, `bool` one, `type` four,
//!   `comp` and `dutc` eight, `blob` a length and bytes, `ustr` a length in UTF-16 units and
//!   those.
//!
//! Only reading, and defensively: every length is checked against what is there, a node is not
//! gone into twice, the tree no deeper than it says (and never past 32), so that a damaged or
//! hostile file is an error, never a crash or a hang. Nothing is ever written.

use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

/// A record's value.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
    Long(u32),
    Short(u16),
    Bool(bool),
    Type([u8; 4]),
    Comp(u64),
    Dutc(u64),
    Blob(Vec<u8>),
    Ustr(String),
}

impl Value {
    /// As text, for what may be written either way (`ustr`, or a `blob` of UTF-8).
    pub fn text(&self) -> Option<String> {
        match self {
            Value::Ustr(s) => Some(s.clone()),
            Value::Blob(b) => std::str::from_utf8(b).ok().map(|s| s.trim_end_matches('\0').to_string()),
            _ => None,
        }
    }
}

/// One record: of which file, what, and its value.
#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub name: String,
    pub code: [u8; 4],
    pub value: Value,
}

/// Bytes read in order, every read checked.
struct Bytes<'a> {
    data: &'a [u8],
    at: usize,
}

impl<'a> Bytes<'a> {
    fn new(data: &'a [u8]) -> Self {
        Bytes { data, at: 0 }
    }

    fn take(&mut self, n: usize) -> Result<&'a [u8], String> {
        let end = self.at.checked_add(n).filter(|&e| e <= self.data.len()).ok_or("cut short")?;
        let out = &self.data[self.at..end];
        self.at = end;
        Ok(out)
    }

    fn u8(&mut self) -> Result<u8, String> {
        Ok(self.take(1)?[0])
    }

    fn u32(&mut self) -> Result<u32, String> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }

    fn u64(&mut self) -> Result<u64, String> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }

    fn four(&mut self) -> Result<[u8; 4], String> {
        Ok(self.take(4)?.try_into().unwrap())
    }

    /// `units` UTF-16 code units.
    fn utf16(&mut self, units: u32) -> Result<String, String> {
        let bytes = self.take((units as usize).checked_mul(2).ok_or("too long")?)?;
        let units: Vec<u16> = bytes.chunks_exact(2).map(|c| u16::from_be_bytes([c[0], c[1]])).collect();
        Ok(String::from_utf16_lossy(&units))
    }
}

/// The deepest a tree is gone into, whatever it says of itself.
const DEEPEST: u32 = 32;

/// Every record in a `.DS_Store`, in the tree's order.
pub fn read(file: &[u8]) -> Result<Vec<Record>, String> {
    let mut head = Bytes::new(file);
    if head.u32()? != 1 || head.take(4)? != b"Bud1" {
        return Err("not a .DS_Store".into());
    }
    let (root_at, root_size, again) = (head.u32()?, head.u32()?, head.u32()?);
    if root_at != again {
        return Err("its header disagrees with itself".into());
    }
    let root = region(file, root_at as usize, root_size as usize)?;
    let mut r = Bytes::new(root);
    let count = r.u32()?;
    r.u32()?;
    // Each address is four bytes; a count claiming more than the block holds is cut short here.
    let addresses: Vec<u32> = (0..count).map(|_| r.u32()).collect::<Result<_, _>>()?;
    let padding = (256 - count as usize % 256) % 256;
    r.take(padding * 4)?;
    let mut tree = None;
    for _ in 0..r.u32()? {
        let len = r.u8()? as usize;
        let name = r.take(len)?;
        let block = r.u32()?;
        if name == b"DSDB" {
            tree = Some(block);
        }
    }
    let tree = tree.ok_or("no DSDB in it")?;
    let blocks = Blocks { file, addresses };
    let mut db = Bytes::new(blocks.get(tree)?);
    let (top, levels) = (db.u32()?, db.u32()?);
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    walk(&blocks, top, 0, levels.min(DEEPEST), &mut seen, &mut out)?;
    Ok(out)
}

/// The allocator's blocks, by number.
struct Blocks<'a> {
    file: &'a [u8],
    addresses: Vec<u32>,
}

impl<'a> Blocks<'a> {
    fn get(&self, n: u32) -> Result<&'a [u8], String> {
        let address = *self.addresses.get(n as usize).ok_or("a block that is not there")?;
        let size = 1usize.checked_shl(address & 0x1f).ok_or("a block too big")?;
        region(self.file, (address & !0x1f) as usize, size)
    }
}

/// The part of the file a block's offset and size say (offsets from byte 4); a last block that
/// claims to run past the end is what there is of it.
fn region(file: &[u8], at: usize, size: usize) -> Result<&[u8], String> {
    let start = at.checked_add(4).filter(|&s| s <= file.len()).ok_or("a block past the end")?;
    let end = start.saturating_add(size).min(file.len());
    Ok(&file[start..end])
}

fn walk(blocks: &Blocks, node: u32, depth: u32, levels: u32, seen: &mut HashSet<u32>, out: &mut Vec<Record>) -> Result<(), String> {
    if !seen.insert(node) {
        return Err("a node met twice".into());
    }
    let mut b = Bytes::new(blocks.get(node)?);
    let (last, count) = (b.u32()?, b.u32()?);
    if last == 0 {
        for _ in 0..count {
            out.push(record(&mut b)?);
        }
        return Ok(());
    }
    if depth >= levels {
        return Err("deeper than it says".into());
    }
    for _ in 0..count {
        let child = b.u32()?;
        walk(blocks, child, depth + 1, levels, seen, out)?;
        out.push(record(&mut b)?);
    }
    walk(blocks, last, depth + 1, levels, seen, out)
}

fn record(b: &mut Bytes) -> Result<Record, String> {
    let units = b.u32()?;
    let name = b.utf16(units)?;
    let code = b.four()?;
    let kind = b.four()?;
    let value = match &kind {
        b"long" => Value::Long(b.u32()?),
        b"shor" => Value::Short(b.u32()? as u16),
        b"bool" => Value::Bool(b.u8()? != 0),
        b"type" => Value::Type(b.four()?),
        b"comp" => Value::Comp(b.u64()?),
        b"dutc" => Value::Dutc(b.u64()?),
        b"blob" => {
            let len = b.u32()?;
            Value::Blob(b.take(len as usize)?.to_vec())
        }
        b"ustr" => {
            let len = b.u32()?;
            Value::Ustr(b.utf16(len)?)
        }
        other => return Err(format!("a value of a kind not known ({})", String::from_utf8_lossy(other))),
    };
    Ok(Record { name, code, value })
}

/// Where Finder says a thing in its Trash came from: the folder (`ptbL`, from the top of the
/// disk, as `Users/me/Documents/`) and, when it was renamed in the Trash, its own name
/// (`ptbN`).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Origin {
    pub folder: String,
    pub name: Option<String>,
}

/// Each thing's origin in a Trash's `.DS_Store`, by its name in the Trash. A thing with a name
/// but no folder is left out: where it goes back to is not known.
pub fn put_back_records(file: &[u8]) -> Result<HashMap<String, Origin>, String> {
    let mut out: HashMap<String, Origin> = HashMap::new();
    let mut names: HashMap<String, String> = HashMap::new();
    for r in read(file)? {
        match &r.code {
            b"ptbL" => {
                if let Some(folder) = r.value.text() {
                    out.entry(r.name).or_default().folder = folder;
                }
            }
            b"ptbN" => {
                if let Some(name) = r.value.text() {
                    names.insert(r.name, name);
                }
            }
            _ => {}
        }
    }
    for (file, name) in names {
        if let Some(o) = out.get_mut(&file) {
            o.name = Some(name);
        }
    }
    Ok(out)
}

/// The whole path an origin is, on the disk whose top is `disk` (`/` for the home Trash).
pub fn origin_path(disk: &Path, origin: &Origin, trash_name: &str) -> PathBuf {
    let name = origin.name.as_deref().filter(|n| !n.is_empty() && !n.contains('/') && *n != "." && *n != "..").unwrap_or(trash_name);
    // The folder's own steps only: none up, none from elsewhere.
    let folder: PathBuf = Path::new(&origin.folder).components().filter(|c| matches!(c, std::path::Component::Normal(_))).collect();
    disk.join(folder).join(name)
}

#[cfg(test)]
pub mod tests {
    use super::*;

    fn utf16(s: &str) -> Vec<u8> {
        let units: Vec<u16> = s.encode_utf16().collect();
        let mut out = (units.len() as u32).to_be_bytes().to_vec();
        out.extend(units.iter().flat_map(|u| u.to_be_bytes()));
        out
    }

    /// A record as Finder writes one.
    pub fn record_bytes(name: &str, code: &[u8; 4], value: &Value) -> Vec<u8> {
        let mut out = utf16(name);
        out.extend_from_slice(code);
        match value {
            Value::Long(v) => out.extend(b"long".iter().copied().chain(v.to_be_bytes())),
            Value::Short(v) => out.extend(b"shor".iter().copied().chain((*v as u32).to_be_bytes())),
            Value::Bool(v) => out.extend(b"bool".iter().copied().chain([*v as u8])),
            Value::Type(t) => out.extend(b"type".iter().chain(t.iter()).copied()),
            Value::Comp(v) => out.extend(b"comp".iter().copied().chain(v.to_be_bytes())),
            Value::Dutc(v) => out.extend(b"dutc".iter().copied().chain(v.to_be_bytes())),
            Value::Blob(b) => out.extend(b"blob".iter().copied().chain((b.len() as u32).to_be_bytes()).chain(b.iter().copied())),
            Value::Ustr(s) => {
                out.extend_from_slice(b"ustr");
                out.extend(utf16(s));
            }
        }
        out
    }

    /// A node: a leaf of records, or an internal node of (child, record) pairs and its last
    /// child.
    pub enum Node {
        Leaf(Vec<Vec<u8>>),
        Inner(Vec<(u32, Vec<u8>)>, u32),
    }

    /// A whole `.DS_Store` of these nodes (blocks 2 on; block 1 is DSDB, 0 the root), laid out
    /// as the buddy allocator lays it: each block at an offset its own size divides.
    pub fn file_of(nodes: &[Node], top: u32, levels: u32) -> Vec<u8> {
        let mut blocks: Vec<Vec<u8>> = Vec::new();
        let mut db = Vec::new();
        for v in [top, levels, 0, nodes.len() as u32, 0x1000] {
            db.extend(v.to_be_bytes());
        }
        blocks.push(db);
        for node in nodes {
            let mut b = Vec::new();
            match node {
                Node::Leaf(records) => {
                    b.extend(0u32.to_be_bytes());
                    b.extend((records.len() as u32).to_be_bytes());
                    records.iter().for_each(|r| b.extend(r));
                }
                Node::Inner(pairs, last) => {
                    b.extend(last.to_be_bytes());
                    b.extend((pairs.len() as u32).to_be_bytes());
                    for (child, r) in pairs {
                        b.extend(child.to_be_bytes());
                        b.extend(r);
                    }
                }
            }
            blocks.push(b);
        }
        // The root block's size, known before it is placed: its addresses padded to 256, the
        // table of contents, 32 empty free lists.
        let root_len = 8 + 256 * 4 + 4 + 1 + 4 + 4 + 32 * 4;
        let all: Vec<usize> = std::iter::once(root_len).chain(blocks.iter().map(Vec::len)).collect();
        let mut file = vec![0u8; 4 + 32];
        let mut addresses = Vec::new();
        let mut at = 32usize;
        let mut placed = Vec::new();
        for len in &all {
            let shift = (len.max(&32).next_power_of_two()).trailing_zeros();
            let size = 1usize << shift;
            at = at.div_ceil(size) * size;
            addresses.push(at as u32 | shift);
            placed.push(at);
            at += size;
        }
        file.resize(4 + at, 0);
        let mut root = Vec::new();
        root.extend((addresses.len() as u32).to_be_bytes());
        root.extend(0u32.to_be_bytes());
        for a in &addresses {
            root.extend(a.to_be_bytes());
        }
        root.resize(8 + 256 * 4, 0);
        root.extend(1u32.to_be_bytes());
        root.push(4);
        root.extend_from_slice(b"DSDB");
        root.extend(1u32.to_be_bytes());
        root.resize(root_len, 0);
        let contents: Vec<&Vec<u8>> = std::iter::once(&root).chain(blocks.iter()).collect();
        for (b, at) in contents.iter().zip(&placed) {
            file[4 + at..4 + at + b.len()].copy_from_slice(b);
        }
        file[0..4].copy_from_slice(&1u32.to_be_bytes());
        file[4..8].copy_from_slice(b"Bud1");
        let root_address = addresses[0];
        let (root_at, root_size) = (root_address & !0x1f, 1u32 << (root_address & 0x1f));
        file[8..12].copy_from_slice(&root_at.to_be_bytes());
        file[12..16].copy_from_slice(&root_size.to_be_bytes());
        file[16..20].copy_from_slice(&root_at.to_be_bytes());
        file
    }

    /// The `.DS_Store` of a Mac's `~/.Trash` as Finder wrote it on 2026-11-15 (littleotter,
    /// macOS 27.2), byte for byte where it is not zero: one thing, trashed from
    /// `/tmp/sstest/files`. The root block at 0x1000 (2 KiB), DSDB at 0x40, the one leaf at 0x100.
    fn finders_own() -> Vec<u8> {
        let mut f = vec![0u8; 0x1804];
        let mut put = |at: usize, bytes: &[u8]| f[at..at + bytes.len()].copy_from_slice(bytes);
        put(0, &[0, 0, 0, 1]);
        put(4, b"Bud1");
        put(8, &[0, 0, 0x10, 0, 0, 0, 0x08, 0, 0, 0, 0x10, 0, 0, 0, 0x01, 0x08]);
        // DSDB: root node block 2, no internal levels, 2 records, 1 node, pages of 4 KiB.
        put(0x44, &[0, 0, 0, 2, 0, 0, 0, 0, 0, 0, 0, 2, 0, 0, 0, 1, 0, 0, 0x10, 0]);
        let mut leaf = vec![0, 0, 0, 0, 0, 0, 0, 2];
        leaf.extend(record_bytes("zz-spatishelf-test.txt", b"ptbL", &Value::Ustr("private/tmp/sstest/files/".into())));
        leaf.extend(record_bytes("zz-spatishelf-test.txt", b"ptbN", &Value::Ustr("zz-spatishelf-test.txt".into())));
        put(0x104, &leaf);
        // The root: three blocks (itself, DSDB, the leaf), then the table of contents.
        put(0x1004, &[0, 0, 0, 3, 0, 0, 0, 0, 0, 0, 0x10, 0x0b, 0, 0, 0, 0x45, 0, 0, 0x01, 0x08]);
        put(0x140c, &[0, 0, 0, 1, 4, b'D', b'S', b'D', b'B', 0, 0, 0, 1]);
        f
    }

    #[test]
    fn finders_own_trash_record_is_read() {
        let file = finders_own();
        assert_eq!(file.len(), 6148, "the size Finder's was");
        let records = read(&file).unwrap();
        assert_eq!(records.len(), 2);
        assert_eq!(records[0], Record { name: "zz-spatishelf-test.txt".into(), code: *b"ptbL", value: Value::Ustr("private/tmp/sstest/files/".into()) });
        let origins = put_back_records(&file).unwrap();
        let o = &origins["zz-spatishelf-test.txt"];
        assert_eq!(o.folder, "private/tmp/sstest/files/");
        assert_eq!(origin_path(Path::new("/"), o, "zz-spatishelf-test.txt"), PathBuf::from("/private/tmp/sstest/files/zz-spatishelf-test.txt"));
    }

    #[test]
    fn every_kind_of_value_and_a_tree_with_inner_nodes() {
        let r = record_bytes;
        // Block 2 the top (inner), its children leaves 3 and 4; the tree's order is 3's, the
        // pair's record, 4's.
        let nodes = [
            Node::Inner(vec![(3, r("b.txt", b"ptbL", &Value::Blob(b"Users/me/Desktop/".to_vec())))], 4),
            Node::Leaf(vec![r("a 2.txt", b"ptbL", &Value::Ustr("Users/me/Docs/".into())), r("a 2.txt", b"ptbN", &Value::Ustr("a.txt".into())), r("a 2.txt", b"Iloc", &Value::Blob(vec![0, 1, 2]))]),
            Node::Leaf(vec![
                r("c", b"lg1S", &Value::Comp(1 << 40)),
                r("c", b"moDD", &Value::Dutc(7)),
                r("c", b"dilc", &Value::Long(9)),
                r("c", b"fwvh", &Value::Short(300)),
                r("c", b"dscl", &Value::Bool(true)),
                r("c", b"vstl", &Value::Type(*b"icnv")),
                r("caf\u{e9} \u{1f600}", b"ptbL", &Value::Ustr("Volumes stuff/x/".into())),
            ]),
        ];
        let file = file_of(&nodes, 2, 1);
        let records = read(&file).unwrap();
        let names: Vec<_> = records.iter().map(|r| (r.name.as_str(), std::str::from_utf8(&r.code).unwrap())).collect();
        assert_eq!(names[..4], [("a 2.txt", "ptbL"), ("a 2.txt", "ptbN"), ("a 2.txt", "Iloc"), ("b.txt", "ptbL")]);
        assert_eq!(records.len(), 11);
        assert_eq!(records[6].value, Value::Long(9));
        assert_eq!(records[7].value, Value::Short(300));
        assert_eq!(records[9].value, Value::Type(*b"icnv"));
        let origins = put_back_records(&file).unwrap();
        assert_eq!(origins.len(), 3, "c has no ptbL: {origins:?}");
        assert_eq!(origin_path(Path::new("/"), &origins["a 2.txt"], "a 2.txt"), PathBuf::from("/Users/me/Docs/a.txt"), "back by its own name, not the Trash's");
        assert_eq!(origin_path(Path::new("/Volumes/Disk"), &origins["b.txt"], "b.txt"), PathBuf::from("/Volumes/Disk/Users/me/Desktop/b.txt"), "a blob's text, from its disk's top");
        assert_eq!(origins["caf\u{e9} \u{1f600}"].folder, "Volumes stuff/x/");
        // A name that would leave the folder is not trusted.
        let sly = Origin { folder: "Users/me/".into(), name: Some("../../etc/x".into()) };
        assert_eq!(origin_path(Path::new("/"), &sly, "x"), PathBuf::from("/Users/me/x"));
        let up = Origin { folder: "/../Users/../me/".into(), name: Some("..".into()) };
        assert_eq!(origin_path(Path::new("/Volumes/D"), &up, "y"), PathBuf::from("/Volumes/D/Users/me/y"));
    }

    #[test]
    fn damaged_files_are_errors_not_crashes() {
        let good = file_of(&[Node::Leaf(vec![record_bytes("a", b"ptbL", &Value::Ustr("x/".into()))])], 2, 0);
        assert!(read(&good).is_ok());
        assert!(read(b"").is_err());
        assert!(read(b"\0\0\0\x01Bud2").is_err());
        // Cut anywhere: an error (or, cut in the trailing free lists, still whole), never a panic.
        for n in 0..good.len() {
            let _ = read(&good[..n]);
        }
        // Every byte changed in turn: never a panic, never a hang.
        for i in 0..good.len().min(4096) {
            let mut bad = good.clone();
            bad[i] ^= 0xff;
            let _ = read(&bad);
        }
        // A node that is its own child.
        let looped = file_of(&[Node::Inner(vec![(2, record_bytes("a", b"ptbL", &Value::Ustr("x/".into())))], 2)], 2, 5);
        assert!(read(&looped).unwrap_err().contains("twice"));
        // Deeper than it says.
        let deep = file_of(&[Node::Inner(vec![], 3), Node::Leaf(vec![])], 2, 0);
        assert!(read(&deep).is_err());
        // A kind of value not known: the rest of the node cannot be read.
        let mut odd = record_bytes("a", b"ptbL", &Value::Long(1));
        odd[4 + 2 + 4..4 + 2 + 8].copy_from_slice(b"wxyz");
        assert!(read(&file_of(&[Node::Leaf(vec![odd])], 2, 0)).is_err());
        // A length claiming more than there is.
        let mut long = record_bytes("a", b"ptbL", &Value::Ustr("x/".into()));
        long[0..4].copy_from_slice(&u32::MAX.to_be_bytes());
        assert!(read(&file_of(&[Node::Leaf(vec![long])], 2, 0)).is_err());
    }
}
