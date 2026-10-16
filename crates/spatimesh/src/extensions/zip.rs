//! Reading a zip: what a `.zip`, an `.xpi` (Firefox) and a `.crx` (Chrome, after its header)
//! carry an extension in. The central directory, stored and deflated entries, nothing else:
//! what packaged extensions use.

use std::io::Read;

/// One file in an archive.
pub struct Entry {
    pub name: String,
    pub bytes: Vec<u8>,
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

/// The most an archive's files may come to, unpacked: no extension is near it, and a crafted
/// archive does not fill the disk.
const MOST: usize = 512 << 20;

/// Every file of a zip archive (folders are not entries of their own).
pub fn read(archive: &[u8]) -> Result<Vec<Entry>, String> {
    // The end of the central directory: the last signature in the last 64 KiB and a bit.
    let floor = archive.len().saturating_sub(65_557);
    let end = (floor..archive.len().saturating_sub(21))
        .rev()
        .find(|&i| archive[i..i + 4] == [0x50, 0x4b, 0x05, 0x06])
        .ok_or("not a zip archive")?;
    let count = u16_at(archive, end + 10).ok_or("a broken zip archive")? as usize;
    let mut at = u32_at(archive, end + 16).ok_or("a broken zip archive")? as usize;
    let mut entries = Vec::new();
    let mut total = 0usize;
    for _ in 0..count {
        if u32_at(archive, at) != Some(0x0201_4b50) {
            return Err("a broken zip archive (its directory)".into());
        }
        let method = u16_at(archive, at + 10).ok_or("a broken zip archive")?;
        let packed = u32_at(archive, at + 20).ok_or("a broken zip archive")? as usize;
        let size = u32_at(archive, at + 24).ok_or("a broken zip archive")? as usize;
        let name_len = u16_at(archive, at + 28).ok_or("a broken zip archive")? as usize;
        let extra_len = u16_at(archive, at + 30).ok_or("a broken zip archive")? as usize;
        let comment_len = u16_at(archive, at + 32).ok_or("a broken zip archive")? as usize;
        let local = u32_at(archive, at + 42).ok_or("a broken zip archive")? as usize;
        let name = String::from_utf8_lossy(archive.get(at + 46..at + 46 + name_len).ok_or("a broken zip archive")?).into_owned();
        at += 46 + name_len + extra_len + comment_len;
        if name.ends_with('/') {
            continue;
        }
        total += size;
        if total > MOST {
            return Err("the archive is larger unpacked than any extension".into());
        }
        if u32_at(archive, local) != Some(0x0403_4b50) {
            return Err(format!("a broken zip archive ({name})"));
        }
        let local_name = u16_at(archive, local + 26).ok_or("a broken zip archive")? as usize;
        let local_extra = u16_at(archive, local + 28).ok_or("a broken zip archive")? as usize;
        let start = local + 30 + local_name + local_extra;
        let data = archive.get(start..start + packed).ok_or_else(|| format!("a broken zip archive ({name})"))?;
        let bytes = match method {
            0 => data.to_vec(),
            8 => {
                let mut out = Vec::with_capacity(size);
                flate2::read::DeflateDecoder::new(data)
                    .take(MOST as u64)
                    .read_to_end(&mut out)
                    .map_err(|e| format!("{name}: {e}"))?;
                out
            }
            other => return Err(format!("{name} is packed in a way this does not read ({other})")),
        };
        entries.push(Entry { name, bytes });
    }
    Ok(entries)
}

/// A `.crx`'s zip, after its header (CRX3, or CRX2 for an old one), and the extension's id
/// as its header gives it, if it does.
pub fn crx(file: &[u8]) -> Result<(&[u8], Option<String>), String> {
    if file.get(0..4) != Some(b"Cr24") {
        return Err("not a .crx".into());
    }
    match u32_at(file, 4) {
        Some(3) => {
            let header = u32_at(file, 8).ok_or("a broken .crx")? as usize;
            let proto = file.get(12..12 + header).ok_or("a broken .crx")?;
            let id = signed_id(proto);
            Ok((file.get(12 + header..).ok_or("a broken .crx")?, id))
        }
        Some(2) => {
            let key = u32_at(file, 8).ok_or("a broken .crx")? as usize;
            let signature = u32_at(file, 12).ok_or("a broken .crx")? as usize;
            let public_key = file.get(16..16 + key).ok_or("a broken .crx")?;
            let id = Some(super::id_of_key(public_key));
            Ok((file.get(16 + key + signature..).ok_or("a broken .crx")?, id))
        }
        _ => Err("a .crx of a version this does not read".into()),
    }
}

/// Protocol buffers' varint at `at`, and where the next field starts.
fn varint(b: &[u8], mut at: usize) -> Option<(u64, usize)> {
    let mut value = 0u64;
    for shift in (0..64).step_by(7) {
        let byte = *b.get(at)?;
        at += 1;
        value |= ((byte & 0x7f) as u64) << shift;
        if byte & 0x80 == 0 {
            return Some((value, at));
        }
    }
    None
}

/// The length-delimited fields of a protocol buffer message: (field number, bytes).
fn fields(b: &[u8]) -> Vec<(u64, &[u8])> {
    let mut out = Vec::new();
    let mut at = 0;
    while at < b.len() {
        let Some((key, next)) = varint(b, at) else { break };
        at = next;
        match key & 7 {
            0 => match varint(b, at) {
                Some((_, next)) => at = next,
                None => break,
            },
            1 => at += 8,
            5 => at += 4,
            2 => {
                let Some((len, next)) = varint(b, at) else { break };
                let Some(bytes) = b.get(next..next + len as usize) else { break };
                out.push((key >> 3, bytes));
                at = next + len as usize;
            }
            _ => break,
        }
    }
    out
}

/// CRX3: CrxFileHeader.signed_header_data (10000) is SignedData, whose crx_id (1) is the
/// extension's id as sixteen bytes.
fn signed_id(header: &[u8]) -> Option<String> {
    let signed = fields(header).into_iter().find(|(n, _)| *n == 10000)?.1;
    let crx_id = fields(signed).into_iter().find(|(n, _)| *n == 1)?.1;
    (crx_id.len() == 16).then(|| super::id_of_bytes(crx_id))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A zip of `files`, stored or deflated alternately: for tests.
    pub fn make(files: &[(&str, &[u8])]) -> Vec<u8> {
        use std::io::Write;
        let mut out = Vec::new();
        let mut directory = Vec::new();
        for (i, (name, bytes)) in files.iter().enumerate() {
            let (method, data) = if i % 2 == 0 {
                (0u16, bytes.to_vec())
            } else {
                let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                e.write_all(bytes).unwrap();
                (8u16, e.finish().unwrap())
            };
            let local = out.len() as u32;
            out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
            out.extend_from_slice(&[20, 0, 0, 0]);
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&[0; 8]);
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&[0, 0]);
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&data);
            directory.extend_from_slice(&0x0201_4b50u32.to_le_bytes());
            directory.extend_from_slice(&[20, 0, 20, 0, 0, 0]);
            directory.extend_from_slice(&method.to_le_bytes());
            directory.extend_from_slice(&[0; 8]);
            directory.extend_from_slice(&(data.len() as u32).to_le_bytes());
            directory.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
            directory.extend_from_slice(&(name.len() as u16).to_le_bytes());
            directory.extend_from_slice(&[0; 12]);
            directory.extend_from_slice(&local.to_le_bytes());
            directory.extend_from_slice(name.as_bytes());
        }
        let at = out.len() as u32;
        out.extend_from_slice(&directory);
        out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(files.len() as u16).to_le_bytes());
        out.extend_from_slice(&(directory.len() as u32).to_le_bytes());
        out.extend_from_slice(&at.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        out
    }

    #[test]
    fn stored_and_deflated_come_back() {
        let big = "ab".repeat(5000);
        let archive = make(&[("manifest.json", b"{}"), ("js/a.js", big.as_bytes()), ("b/", b"")]);
        let entries = read(&archive).unwrap();
        assert_eq!(entries.len(), 2, "folders are not files");
        assert_eq!(entries[0].name, "manifest.json");
        assert_eq!(entries[1].bytes, big.as_bytes());
        assert!(read(b"not a zip at all, not even close to one").is_err());
    }

    #[test]
    fn a_crx3_is_its_header_then_a_zip() {
        // SignedData { crx_id: 16 bytes } inside CrxFileHeader { signed_header_data (10000) }.
        let crx_id: Vec<u8> = (0..16).collect();
        let mut signed = vec![0x0a, 16];
        signed.extend_from_slice(&crx_id);
        let mut header = vec![0x82, 0xf1, 0x04, signed.len() as u8];
        header.extend_from_slice(&signed);
        let zip = make(&[("manifest.json", b"{\"name\":\"x\"}")]);
        let mut file = b"Cr24".to_vec();
        file.extend_from_slice(&3u32.to_le_bytes());
        file.extend_from_slice(&(header.len() as u32).to_le_bytes());
        file.extend_from_slice(&header);
        file.extend_from_slice(&zip);
        let (inner, id) = crx(&file).unwrap();
        assert_eq!(read(inner).unwrap()[0].name, "manifest.json");
        assert_eq!(id.as_deref(), Some("aaabacadaeafagahaiajakalamanaoap"));
    }
}
