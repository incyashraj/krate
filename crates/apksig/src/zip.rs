//! The zip structure an APK signature covers, and reading one entry.
//!
//! An APK is a zip with an APK Signing Block between the last entry and the
//! central directory. The v2/v3 digest covers three sections -- the entries,
//! the central directory, and the end record with its directory offset
//! pointing where the signing block starts -- so the bytes are walked here
//! by hand rather than through a zip library that hides those offsets.

use crate::Refused;

/// Where the parts of an APK are.
pub(crate) struct Layout {
    /// Where the central directory starts (and the signing block ends).
    pub cd_offset: usize,
    pub cd_size: usize,
    /// Where the end-of-central-directory record starts.
    pub eocd_offset: usize,
    /// The signing block, when there is one: (start, end).
    pub signing_block: Option<(usize, usize)>,
}

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

fn u64_at(b: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(b.get(at..at + 8)?.try_into().ok()?))
}

const EOCD_MAGIC: u32 = 0x0605_4b50;
const BLOCK_MAGIC: &[u8; 16] = b"APK Sig Block 42";

pub(crate) fn layout(apk: &[u8]) -> Result<Layout, Refused> {
    let not_a_zip = || Refused::NotAnApk("it is not a zip archive".into());
    if apk.len() < 22 {
        return Err(not_a_zip());
    }
    // The end record is 22 bytes plus a comment of at most 65,535.
    let lowest = apk.len().saturating_sub(22 + 0xffff);
    let eocd_offset = (lowest..=apk.len() - 22)
        .rev()
        .find(|&at| {
            u32_at(apk, at) == Some(EOCD_MAGIC)
                && u16_at(apk, at + 20).map(usize::from) == Some(apk.len() - at - 22)
        })
        .ok_or_else(not_a_zip)?;
    let cd_size = u32_at(apk, eocd_offset + 12).ok_or_else(not_a_zip)? as usize;
    let cd_offset = u32_at(apk, eocd_offset + 16).ok_or_else(not_a_zip)? as usize;
    if cd_offset == 0xffff_ffff || cd_size == 0xffff_ffff {
        return Err(Refused::Unsupported("a ZIP64 APK".into()));
    }
    if cd_offset.checked_add(cd_size) != Some(eocd_offset) {
        return Err(Refused::NotAnApk(
            "its central directory does not end where its end record starts".into(),
        ));
    }
    let signing_block = signing_block(apk, cd_offset)?;
    Ok(Layout {
        cd_offset,
        cd_size,
        eocd_offset,
        signing_block,
    })
}

fn signing_block(apk: &[u8], cd_offset: usize) -> Result<Option<(usize, usize)>, Refused> {
    if cd_offset < 32 || apk.get(cd_offset - 16..cd_offset) != Some(&BLOCK_MAGIC[..]) {
        return Ok(None);
    }
    let bad = || Refused::Invalid("its APK Signing Block is malformed".into());
    let size = u64_at(apk, cd_offset - 24).ok_or_else(bad)? as usize;
    let start = cd_offset
        .checked_sub(size)
        .and_then(|s| s.checked_sub(8))
        .ok_or_else(bad)?;
    if u64_at(apk, start).map(|s| s as usize) != Some(size) {
        return Err(bad());
    }
    Ok(Some((start, cd_offset)))
}

/// The id-value pairs inside a signing block.
pub(crate) fn block_pairs(apk: &[u8], block: (usize, usize)) -> Result<Vec<(u32, &[u8])>, Refused> {
    let bad = || Refused::Invalid("its APK Signing Block is malformed".into());
    let mut at = block.0 + 8;
    let end = block.1 - 24;
    let mut out = Vec::new();
    while at < end {
        let len = u64_at(apk, at).ok_or_else(bad)? as usize;
        if len < 4 || at + 8 + len > end {
            return Err(bad());
        }
        let id = u32_at(apk, at + 8).ok_or_else(bad)?;
        out.push((id, &apk[at + 12..at + 8 + len]));
        at += 8 + len;
    }
    Ok(out)
}

/// Largest entry read out of an APK: a manifest or a challenge statement.
const MAX_ENTRY: usize = 4 * 1024 * 1024;

/// Every entry name in the central directory.
pub(crate) fn names(apk: &[u8], layout: &Layout) -> Result<Vec<String>, Refused> {
    Ok(entries(apk, layout)?.into_iter().map(|e| e.name).collect())
}

struct Entry {
    name: String,
    method: u16,
    compressed: usize,
    size: usize,
    local: usize,
}

fn entries(apk: &[u8], layout: &Layout) -> Result<Vec<Entry>, Refused> {
    let bad = || Refused::NotAnApk("its central directory is malformed".into());
    let mut out = Vec::new();
    let mut at = layout.cd_offset;
    let end = layout.cd_offset + layout.cd_size;
    while at < end {
        if u32_at(apk, at) != Some(0x0201_4b50) {
            return Err(bad());
        }
        let method = u16_at(apk, at + 10).ok_or_else(bad)?;
        let compressed = u32_at(apk, at + 20).ok_or_else(bad)? as usize;
        let size = u32_at(apk, at + 24).ok_or_else(bad)? as usize;
        let name_len = usize::from(u16_at(apk, at + 28).ok_or_else(bad)?);
        let extra = usize::from(u16_at(apk, at + 30).ok_or_else(bad)?);
        let comment = usize::from(u16_at(apk, at + 32).ok_or_else(bad)?);
        let local = u32_at(apk, at + 42).ok_or_else(bad)? as usize;
        let name = apk.get(at + 46..at + 46 + name_len).ok_or_else(bad)?;
        out.push(Entry {
            name: String::from_utf8_lossy(name).into_owned(),
            method,
            compressed,
            size,
            local,
        });
        at += 46 + name_len + extra + comment;
    }
    Ok(out)
}

/// One entry's bytes, inflated, or `None` when it is not there.
pub(crate) fn read_entry(
    apk: &[u8],
    layout: &Layout,
    name: &str,
) -> Result<Option<Vec<u8>>, Refused> {
    let bad = || Refused::NotAnApk(format!("its {name} entry is malformed"));
    let Some(entry) = entries(apk, layout)?.into_iter().find(|e| e.name == name) else {
        return Ok(None);
    };
    if entry.size > MAX_ENTRY || entry.compressed > MAX_ENTRY {
        return Err(Refused::Unsupported(format!("a {name} larger than 4 MB")));
    }
    let at = entry.local;
    if u32_at(apk, at) != Some(0x0403_4b50) {
        return Err(bad());
    }
    let name_len = usize::from(u16_at(apk, at + 26).ok_or_else(bad)?);
    let extra = usize::from(u16_at(apk, at + 28).ok_or_else(bad)?);
    let data_at = at + 30 + name_len + extra;
    // Never past the entries section: a local header pointing into the
    // signing block or the directory is not an entry.
    let limit = layout
        .signing_block
        .map(|b| b.0)
        .unwrap_or(layout.cd_offset);
    if data_at + entry.compressed > limit {
        return Err(bad());
    }
    let data = &apk[data_at..data_at + entry.compressed];
    let bytes = match entry.method {
        0 => data.to_vec(),
        // Never more than the size the directory gave: a bomb stops there.
        8 => miniz_oxide::inflate::decompress_to_vec_with_limit(data, entry.size)
            .map_err(|_| bad())?,
        _ => {
            return Err(Refused::Unsupported(format!(
                "{name} compressed another way"
            )))
        }
    };
    if bytes.len() != entry.size {
        return Err(bad());
    }
    Ok(Some(bytes))
}

/// Write a stored (uncompressed) zip of these entries: the challenge APK.
pub(crate) fn write_stored(files: &[(&str, &[u8])]) -> Vec<u8> {
    let mut out = Vec::new();
    let mut central = Vec::new();
    for (name, data) in files {
        let crc = crc32fast::hash(data);
        let offset = out.len() as u32;
        let header = |sig: u32, central_form: bool| {
            let mut h = Vec::new();
            h.extend_from_slice(&sig.to_le_bytes());
            if central_form {
                h.extend_from_slice(&20u16.to_le_bytes()); // made by
            }
            h.extend_from_slice(&20u16.to_le_bytes()); // needed
            h.extend_from_slice(&0u16.to_le_bytes()); // flags
            h.extend_from_slice(&0u16.to_le_bytes()); // stored
            h.extend_from_slice(&0u16.to_le_bytes()); // time
            h.extend_from_slice(&0x21u16.to_le_bytes()); // date: 1980-01-01
            h.extend_from_slice(&crc.to_le_bytes());
            h.extend_from_slice(&(data.len() as u32).to_le_bytes());
            h.extend_from_slice(&(data.len() as u32).to_le_bytes());
            h.extend_from_slice(&(name.len() as u16).to_le_bytes());
            h.extend_from_slice(&0u16.to_le_bytes()); // extra
            if central_form {
                h.extend_from_slice(&0u16.to_le_bytes()); // comment
                h.extend_from_slice(&0u16.to_le_bytes()); // disk
                h.extend_from_slice(&0u16.to_le_bytes()); // internal
                h.extend_from_slice(&0u32.to_le_bytes()); // external
                h.extend_from_slice(&offset.to_le_bytes());
            }
            h.extend_from_slice(name.as_bytes());
            h
        };
        out.extend_from_slice(&header(0x0403_4b50, false));
        out.extend_from_slice(data);
        central.extend_from_slice(&header(0x0201_4b50, true));
    }
    let cd_offset = out.len() as u32;
    out.extend_from_slice(&central);
    out.extend_from_slice(&EOCD_MAGIC.to_le_bytes());
    out.extend_from_slice(&[0; 4]);
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(files.len() as u16).to_le_bytes());
    out.extend_from_slice(&(central.len() as u32).to_le_bytes());
    out.extend_from_slice(&cd_offset.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out
}
