//! The zip container .xlsx and .ods files are made of: read, and write.
//!
//! Reading takes the central directory at the end of the file as the truth
//! (sizes there are right even when a streaming writer left the local
//! headers blank), understands Zip64, inflates with `miniz_oxide` and
//! checks every member's CRC. Writing produces plain deflated members with
//! a fixed timestamp, so the same workbook always gives the same bytes.

use super::SheetError;
use alloc::{string::String, vec::Vec};

/// The most a single member may inflate to. A real sheet of a million rows
/// is well under this; a zip bomb is not.
const MAX_MEMBER: u64 = 512 * 1024 * 1024;

pub(crate) struct Entry {
    name: String,
    method: u16,
    crc: u32,
    compressed: u64,
    size: u64,
    local_offset: u64,
}

pub(crate) struct Archive<'a> {
    data: &'a [u8],
    entries: Vec<Entry>,
}

fn u16_at(d: &[u8], i: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*d.get(i)?, *d.get(i + 1)?]))
}

fn u32_at(d: &[u8], i: usize) -> Option<u32> {
    Some(u32::from_le_bytes([
        *d.get(i)?,
        *d.get(i + 1)?,
        *d.get(i + 2)?,
        *d.get(i + 3)?,
    ]))
}

fn u64_at(d: &[u8], i: usize) -> Option<u64> {
    Some(u64::from(u32_at(d, i)?) | (u64::from(u32_at(d, i + 4)?) << 32))
}

pub(crate) fn is_zip(data: &[u8]) -> bool {
    data.starts_with(b"PK\x03\x04") || data.starts_with(b"PK\x05\x06")
}

impl<'a> Archive<'a> {
    pub(crate) fn open(data: &'a [u8]) -> Result<Self, SheetError> {
        Self::parse(data).ok_or(SheetError::Damaged)
    }

    fn parse(data: &'a [u8]) -> Option<Self> {
        // End of central directory: 22 bytes plus a comment of up to 64 KiB.
        let lowest = data.len().saturating_sub(22 + 0xffff);
        let mut eocd = None;
        let mut at = data.len().checked_sub(22)?;
        loop {
            if u32_at(data, at) == Some(0x0605_4b50) {
                eocd = Some(at);
                break;
            }
            if at == lowest {
                break;
            }
            at -= 1;
        }
        let eocd = eocd?;
        let mut count = u64::from(u16_at(data, eocd + 10)?);
        let mut cd_offset = u64::from(u32_at(data, eocd + 16)?);
        if cd_offset == 0xffff_ffff || count == 0xffff {
            // Zip64: the locator sits just before the classic record.
            let locator = eocd.checked_sub(20)?;
            if u32_at(data, locator)? != 0x0706_4b50 {
                return None;
            }
            let z64 = usize::try_from(u64_at(data, locator + 8)?).ok()?;
            if u32_at(data, z64)? != 0x0606_4b50 {
                return None;
            }
            count = u64_at(data, z64 + 32)?;
            cd_offset = u64_at(data, z64 + 48)?;
        }
        let mut pos = usize::try_from(cd_offset).ok()?;
        let mut entries = Vec::new();
        for _ in 0..count {
            if u32_at(data, pos)? != 0x0201_4b50 {
                return None;
            }
            let method = u16_at(data, pos + 10)?;
            let crc = u32_at(data, pos + 16)?;
            let mut compressed = u64::from(u32_at(data, pos + 20)?);
            let mut size = u64::from(u32_at(data, pos + 24)?);
            let name_len = usize::from(u16_at(data, pos + 28)?);
            let extra_len = usize::from(u16_at(data, pos + 30)?);
            let comment_len = usize::from(u16_at(data, pos + 32)?);
            let mut local_offset = u64::from(u32_at(data, pos + 42)?);
            let name_bytes = data.get(pos + 46..pos + 46 + name_len)?;
            let name = String::from_utf8_lossy(name_bytes).into_owned();
            // Zip64 extra field: only the values that overflowed are there,
            // in this order.
            let extra = data.get(pos + 46 + name_len..pos + 46 + name_len + extra_len)?;
            let mut e = 0;
            while e + 4 <= extra.len() {
                let id = u16_at(extra, e)?;
                let len = usize::from(u16_at(extra, e + 2)?);
                if id == 0x0001 {
                    let mut f = e + 4;
                    if size == 0xffff_ffff {
                        size = u64_at(extra, f)?;
                        f += 8;
                    }
                    if compressed == 0xffff_ffff {
                        compressed = u64_at(extra, f)?;
                        f += 8;
                    }
                    if local_offset == 0xffff_ffff {
                        local_offset = u64_at(extra, f)?;
                    }
                }
                e += 4 + len;
            }
            entries.push(Entry {
                name,
                method,
                crc,
                compressed,
                size,
                local_offset,
            });
            pos += 46 + name_len + extra_len + comment_len;
        }
        Some(Archive { data, entries })
    }

    /// The member called `name`, compared the way OPC part names are:
    /// without a leading slash and ignoring ASCII case.
    pub(crate) fn find(&self, name: &str) -> Option<&Entry> {
        let name = name.trim_start_matches('/');
        self.entries
            .iter()
            .find(|e| e.name.trim_start_matches('/').eq_ignore_ascii_case(name))
    }

    pub(crate) fn has(&self, name: &str) -> bool {
        self.find(name).is_some()
    }

    /// The bytes of `name`, inflated and checked.
    pub(crate) fn read(&self, name: &str) -> Result<Option<Vec<u8>>, SheetError> {
        let Some(entry) = self.find(name) else {
            return Ok(None);
        };
        self.read_entry(entry).map(Some)
    }

    /// The text of `name`, which must be UTF-8 (every workbook part is).
    pub(crate) fn read_text(&self, name: &str) -> Result<Option<String>, SheetError> {
        match self.read(name)? {
            Some(bytes) => String::from_utf8(bytes)
                .map(Some)
                .map_err(|_| SheetError::Damaged),
            None => Ok(None),
        }
    }

    fn read_entry(&self, e: &Entry) -> Result<Vec<u8>, SheetError> {
        if e.size > MAX_MEMBER {
            return Err(SheetError::TooLarge);
        }
        let data = self.data;
        let local = usize::try_from(e.local_offset).map_err(|_| SheetError::Damaged)?;
        if u32_at(data, local) != Some(0x0403_4b50) {
            return Err(SheetError::Damaged);
        }
        let name_len = usize::from(u16_at(data, local + 26).ok_or(SheetError::Damaged)?);
        let extra_len = usize::from(u16_at(data, local + 28).ok_or(SheetError::Damaged)?);
        let start = local + 30 + name_len + extra_len;
        let len = usize::try_from(e.compressed).map_err(|_| SheetError::Damaged)?;
        let raw = data
            .get(start..start.saturating_add(len))
            .ok_or(SheetError::Damaged)?;
        let size = usize::try_from(e.size).map_err(|_| SheetError::TooLarge)?;
        let out = match e.method {
            0 => raw.to_vec(),
            8 => miniz_oxide::inflate::decompress_to_vec_with_limit(raw, size)
                .map_err(|_| SheetError::Damaged)?,
            _ => return Err(SheetError::Damaged),
        };
        if out.len() != size || crc32(&out) != e.crc {
            return Err(SheetError::Damaged);
        }
        Ok(out)
    }
}

const CRC_TABLE: [u32; 256] = {
    let mut table = [0u32; 256];
    let mut n = 0;
    while n < 256 {
        let mut c = n as u32;
        let mut k = 0;
        while k < 8 {
            c = if c & 1 != 0 {
                0xedb8_8320 ^ (c >> 1)
            } else {
                c >> 1
            };
            k += 1;
        }
        table[n] = c;
        n += 1;
    }
    table
};

pub(crate) fn crc32(bytes: &[u8]) -> u32 {
    let mut c = 0xffff_ffffu32;
    for &b in bytes {
        c = CRC_TABLE[((c ^ u32::from(b)) & 0xff) as usize] ^ (c >> 8);
    }
    !c
}

/// Builds a zip in memory, one member at a time.
pub(crate) struct Writer {
    out: Vec<u8>,
    central: Vec<u8>,
    count: u16,
}

// 1980-01-01 00:00, the earliest time a zip can say: a fixed stamp keeps
// the output byte-for-byte reproducible.
const DOS_TIME: u16 = 0;
const DOS_DATE: u16 = (1 << 5) | 1;

impl Writer {
    pub(crate) fn new() -> Self {
        Writer {
            out: Vec::new(),
            central: Vec::new(),
            count: 0,
        }
    }

    pub(crate) fn add(&mut self, name: &str, data: &[u8]) {
        let packed = miniz_oxide::deflate::compress_to_vec(data, 6);
        let crc = crc32(data);
        let offset = self.out.len() as u32;
        let name = name.as_bytes();
        let fixed = |buf: &mut Vec<u8>| {
            buf.extend_from_slice(&20u16.to_le_bytes()); // version needed
            buf.extend_from_slice(&0x0800u16.to_le_bytes()); // names are UTF-8
            buf.extend_from_slice(&8u16.to_le_bytes()); // deflate
            buf.extend_from_slice(&DOS_TIME.to_le_bytes());
            buf.extend_from_slice(&DOS_DATE.to_le_bytes());
            buf.extend_from_slice(&crc.to_le_bytes());
            buf.extend_from_slice(&(packed.len() as u32).to_le_bytes());
            buf.extend_from_slice(&(data.len() as u32).to_le_bytes());
            buf.extend_from_slice(&(name.len() as u16).to_le_bytes());
            buf.extend_from_slice(&0u16.to_le_bytes()); // extra length
        };
        self.out.extend_from_slice(&0x0403_4b50u32.to_le_bytes());
        fixed(&mut self.out);
        self.out.extend_from_slice(name);
        self.out.extend_from_slice(&packed);

        self.central
            .extend_from_slice(&0x0201_4b50u32.to_le_bytes());
        self.central.extend_from_slice(&20u16.to_le_bytes()); // made by
        fixed(&mut self.central);
        self.central.extend_from_slice(&0u16.to_le_bytes()); // comment
        self.central.extend_from_slice(&0u16.to_le_bytes()); // disk
        self.central.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
        self.central.extend_from_slice(&0u32.to_le_bytes()); // external attrs
        self.central.extend_from_slice(&offset.to_le_bytes());
        self.central.extend_from_slice(name);
        self.count += 1;
    }

    pub(crate) fn finish(mut self) -> Vec<u8> {
        let cd_offset = self.out.len() as u32;
        let cd_len = self.central.len() as u32;
        self.out.extend_from_slice(&self.central);
        self.out.extend_from_slice(&0x0605_4b50u32.to_le_bytes());
        self.out.extend_from_slice(&0u16.to_le_bytes());
        self.out.extend_from_slice(&0u16.to_le_bytes());
        self.out.extend_from_slice(&self.count.to_le_bytes());
        self.out.extend_from_slice(&self.count.to_le_bytes());
        self.out.extend_from_slice(&cd_len.to_le_bytes());
        self.out.extend_from_slice(&cd_offset.to_le_bytes());
        self.out.extend_from_slice(&0u16.to_le_bytes());
        self.out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crc_matches_the_standard_check_value() {
        assert_eq!(crc32(b"123456789"), 0xcbf4_3926);
    }

    #[test]
    fn written_members_read_back() {
        let mut w = Writer::new();
        w.add("a.txt", b"hello hello hello hello");
        w.add("dir/b.xml", b"");
        let bytes = w.finish();
        let zip = Archive::open(&bytes).unwrap();
        assert_eq!(
            zip.read("A.TXT").unwrap().unwrap(),
            b"hello hello hello hello"
        );
        assert_eq!(zip.read("/dir/b.xml").unwrap().unwrap(), b"");
        assert!(zip.read("missing").unwrap().is_none());
    }

    #[test]
    fn a_flipped_byte_is_damage_not_a_panic() {
        let mut w = Writer::new();
        w.add("a.txt", b"some text that deflates some text");
        let good = w.finish();
        for i in 0..good.len() {
            let mut bad = good.clone();
            bad[i] ^= 0x55;
            if let Ok(zip) = Archive::open(&bad) {
                let _ = zip.read("a.txt");
            }
        }
        for cut in 0..good.len() {
            let _ = Archive::open(&good[..cut]).map(|z| z.read("a.txt").is_ok());
        }
    }
}
