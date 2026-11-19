//! A small ZIP reader for the archive `PatientCCD` answers: the central
//! directory, stored and deflated entries, each checked against its CRC-32,
//! with bounds on the entry count and the bytes inflated. Encrypted, ZIP64
//! and multi-disk archives are refused: ICANotes sends none of them.

use std::io::Read;

use pk_cli_core::CliError;

/// The most entries read from one archive.
pub const MAX_ENTRIES: usize = 500;
/// The most one entry may inflate to.
pub const MAX_ENTRY_BYTES: u64 = 64 * 1024 * 1024;
/// The most a whole archive may inflate to.
pub const MAX_TOTAL_BYTES: u64 = 256 * 1024 * 1024;

const EOCD: u32 = 0x0605_4b50;
const CENTRAL: u32 = 0x0201_4b50;
const LOCAL: u32 = 0x0403_4b50;

/// One file in the archive.
#[derive(Clone, PartialEq, Eq)]
pub struct ZipEntry {
    /// Its name in the archive, as written (`/`-separated).
    pub name: String,
    pub bytes: Vec<u8>,
}

impl std::fmt::Debug for ZipEntry {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ZipEntry")
            .field("bytes", &self.bytes.len())
            .finish_non_exhaustive()
    }
}

fn bad(why: impl Into<String>) -> CliError {
    CliError::Upstream(format!("the C-CDA archive does not read: {}", why.into()))
}

fn u16_at(b: &[u8], at: usize) -> Result<u16, CliError> {
    b.get(at..at + 2)
        .map(|s| u16::from_le_bytes([s[0], s[1]]))
        .ok_or_else(|| bad("it ends early"))
}

fn u32_at(b: &[u8], at: usize) -> Result<u32, CliError> {
    b.get(at..at + 4)
        .map(|s| u32::from_le_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or_else(|| bad("it ends early"))
}

/// Every file entry (directories left out), in central-directory order.
pub fn read(archive: &[u8]) -> Result<Vec<ZipEntry>, CliError> {
    // The end record is the last 22 bytes plus a comment of up to 64 KiB.
    let floor = archive.len().saturating_sub(22 + 65_535);
    let eocd = (floor..=archive.len().saturating_sub(22))
        .rev()
        .find(|&i| u32_at(archive, i).ok() == Some(EOCD))
        .ok_or_else(|| bad("no end-of-archive record"))?;
    let disk = u16_at(archive, eocd + 4)?;
    let cd_disk = u16_at(archive, eocd + 6)?;
    let count = u16_at(archive, eocd + 10)? as usize;
    let cd_size = u32_at(archive, eocd + 12)? as usize;
    let cd_offset = u32_at(archive, eocd + 16)? as usize;
    if disk != 0 || cd_disk != 0 {
        return Err(bad("it spans several disks"));
    }
    if count == 0xFFFF || cd_offset == 0xFFFF_FFFF || cd_size == 0xFFFF_FFFF {
        return Err(bad("it is ZIP64"));
    }
    if count > MAX_ENTRIES {
        return Err(bad(format!("{count} entries, over {MAX_ENTRIES}")));
    }
    if cd_offset.checked_add(cd_size).is_none_or(|end| end > eocd) {
        return Err(bad("its directory lies outside it"));
    }
    let mut out = Vec::new();
    let mut total: u64 = 0;
    let mut at = cd_offset;
    for _ in 0..count {
        if u32_at(archive, at)? != CENTRAL {
            return Err(bad("a directory record is damaged"));
        }
        let flags = u16_at(archive, at + 8)?;
        let method = u16_at(archive, at + 10)?;
        let crc = u32_at(archive, at + 16)?;
        let compressed = u32_at(archive, at + 20)? as u64;
        let size = u32_at(archive, at + 24)? as u64;
        let name_len = u16_at(archive, at + 28)? as usize;
        let extra_len = u16_at(archive, at + 30)? as usize;
        let comment_len = u16_at(archive, at + 32)? as usize;
        let local = u32_at(archive, at + 42)? as usize;
        let name_bytes = archive
            .get(at + 46..at + 46 + name_len)
            .ok_or_else(|| bad("it ends early"))?;
        let name = String::from_utf8_lossy(name_bytes).into_owned();
        at += 46 + name_len + extra_len + comment_len;
        if name.ends_with('/') {
            continue;
        }
        if flags & 1 != 0 {
            return Err(bad("an entry is encrypted"));
        }
        if compressed == 0xFFFF_FFFF || size == 0xFFFF_FFFF || local == 0xFFFF_FFFF {
            return Err(bad("it is ZIP64"));
        }
        if size > MAX_ENTRY_BYTES {
            return Err(bad(format!(
                "an entry inflates to over {} MiB",
                MAX_ENTRY_BYTES / 1024 / 1024
            )));
        }
        total += size;
        if total > MAX_TOTAL_BYTES {
            return Err(bad(format!(
                "it inflates to over {} MiB",
                MAX_TOTAL_BYTES / 1024 / 1024
            )));
        }
        if u32_at(archive, local)? != LOCAL {
            return Err(bad("an entry's header is damaged"));
        }
        let start = local
            + 30
            + u16_at(archive, local + 26)? as usize
            + u16_at(archive, local + 28)? as usize;
        let data = archive
            .get(start..start + compressed as usize)
            .ok_or_else(|| bad("an entry runs past its end"))?;
        let bytes = match method {
            0 => data.to_vec(),
            8 => {
                let mut inflated = Vec::with_capacity(size as usize);
                flate2::read::DeflateDecoder::new(data)
                    .take(size + 1)
                    .read_to_end(&mut inflated)
                    .map_err(|e| bad(format!("an entry does not inflate: {e}")))?;
                inflated
            }
            m => return Err(bad(format!("an entry uses compression method {m}"))),
        };
        if bytes.len() as u64 != size {
            return Err(bad("an entry's length is not what its directory says"));
        }
        let mut sum = flate2::Crc::new();
        sum.update(&bytes);
        if sum.sum() != crc {
            return Err(bad("an entry fails its CRC-32"));
        }
        out.push(ZipEntry { name, bytes });
    }
    Ok(out)
}

/// A ZIP of `files`, deflated, for tests and the mock server.
pub fn write(files: &[(&str, &[u8])]) -> Vec<u8> {
    use std::io::Write;
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, content) in files {
        let mut enc =
            flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::default());
        enc.write_all(content).expect("in memory");
        let data = enc.finish().expect("in memory");
        let mut sum = flate2::Crc::new();
        sum.update(content);
        let offset = out.len() as u32;
        let header = |sig: u32, central: bool| {
            let mut h = Vec::new();
            h.extend_from_slice(&sig.to_le_bytes());
            if central {
                h.extend_from_slice(&20u16.to_le_bytes());
            }
            h.extend_from_slice(&20u16.to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes());
            h.extend_from_slice(&8u16.to_le_bytes());
            h.extend_from_slice(&0u32.to_le_bytes());
            h.extend_from_slice(&sum.sum().to_le_bytes());
            h.extend_from_slice(&(data.len() as u32).to_le_bytes());
            h.extend_from_slice(&(content.len() as u32).to_le_bytes());
            h.extend_from_slice(&(name.len() as u16).to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes());
            if central {
                // Comment length, disk, internal and external attributes.
                h.extend_from_slice(&[0u8; 10]);
                h.extend_from_slice(&offset.to_le_bytes());
            }
            h.extend_from_slice(name.as_bytes());
            h
        };
        out.extend(header(LOCAL, false));
        out.extend_from_slice(&data);
        central.extend(header(CENTRAL, true));
    }
    let cd_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&EOCD.to_le_bytes());
    out.extend_from_slice(&[0u8; 4]);
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn what_is_written_reads_back() {
        let zip = write(&[("a.xml", b"<a/>"), ("dir/b.xml", &[b'x'; 5000])]);
        let entries = read(&zip).unwrap();
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].name, "a.xml");
        assert_eq!(entries[0].bytes, b"<a/>");
        assert_eq!(entries[1].bytes.len(), 5000);
    }

    #[test]
    fn a_damaged_archive_is_an_upstream_error() {
        let mut zip = write(&[("a.xml", b"<ClinicalDocument/>")]);
        assert_eq!(read(&zip[..10]).unwrap_err().exit_code(), 5);
        // Flip a byte of the deflated data: the CRC or the inflate fails.
        zip[35] ^= 0xFF;
        assert_eq!(read(&zip).unwrap_err().exit_code(), 5);
        assert_eq!(
            read(b"not a zip at all, not even close")
                .unwrap_err()
                .exit_code(),
            5
        );
    }

    #[test]
    fn an_empty_archive_has_no_entries() {
        assert!(read(&write(&[])).unwrap().is_empty());
    }
}
