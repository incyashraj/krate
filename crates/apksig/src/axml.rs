//! The package name out of a compiled (binary XML) AndroidManifest.xml.
//!
//! Only what is needed: the string pool and the first start tag, which is
//! `<manifest package="...">`. Every offset is bounded before it is read.

fn u16_at(b: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(b.get(at..at + 2)?.try_into().ok()?))
}

fn u32_at(b: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(at..at + 4)?.try_into().ok()?))
}

const RES_XML_TYPE: u16 = 0x0003;
const RES_STRING_POOL_TYPE: u16 = 0x0001;
const RES_XML_START_ELEMENT_TYPE: u16 = 0x0102;
const UTF8_FLAG: u32 = 1 << 8;

/// The manifest's `package` attribute.
pub(crate) fn package_name(manifest: &[u8]) -> Option<String> {
    if u16_at(manifest, 0)? != RES_XML_TYPE {
        return None;
    }
    let mut at = usize::from(u16_at(manifest, 2)?);
    let mut pool: Option<Vec<String>> = None;
    while at + 8 <= manifest.len() {
        let kind = u16_at(manifest, at)?;
        let size = u32_at(manifest, at + 4)? as usize;
        if size < 8 || at + size > manifest.len() {
            return None;
        }
        let chunk = &manifest[at..at + size];
        match kind {
            RES_STRING_POOL_TYPE => pool = Some(strings(chunk)?),
            RES_XML_START_ELEMENT_TYPE => {
                let pool = pool.as_ref()?;
                let name = pool.get(u32_at(chunk, 20)? as usize)?;
                if name != "manifest" {
                    return None;
                }
                let attr_start = usize::from(u16_at(chunk, 24)?);
                let attr_size = usize::from(u16_at(chunk, 26)?);
                let count = usize::from(u16_at(chunk, 28)?);
                // attributeStart counts from the end of the 16-byte node header.
                for i in 0..count {
                    let a = 16 + attr_start + i * attr_size;
                    let attr_name = pool.get(u32_at(chunk, a + 4)? as usize)?;
                    if attr_name == "package" {
                        let raw = u32_at(chunk, a + 8)?;
                        return pool.get(raw as usize).cloned();
                    }
                }
                return None;
            }
            _ => {}
        }
        at += size;
    }
    None
}

fn strings(chunk: &[u8]) -> Option<Vec<String>> {
    let count = u32_at(chunk, 8)? as usize;
    let flags = u32_at(chunk, 16)?;
    let start = u32_at(chunk, 20)? as usize;
    let header = usize::from(u16_at(chunk, 2)?);
    if count > 1_000_000 {
        return None;
    }
    let mut out = Vec::with_capacity(count);
    for i in 0..count {
        let offset = u32_at(chunk, header + i * 4)? as usize;
        let at = start.checked_add(offset)?;
        out.push(if flags & UTF8_FLAG != 0 {
            utf8_string(chunk, at)?
        } else {
            utf16_string(chunk, at)?
        });
    }
    Some(out)
}

fn utf8_string(chunk: &[u8], mut at: usize) -> Option<String> {
    // Two lengths, each one or two bytes: UTF-16 units, then UTF-8 bytes.
    let skip = |at: &mut usize| -> Option<usize> {
        let first = *chunk.get(*at)? as usize;
        *at += 1;
        if first & 0x80 != 0 {
            let second = *chunk.get(*at)? as usize;
            *at += 1;
            Some(((first & 0x7f) << 8) | second)
        } else {
            Some(first)
        }
    };
    skip(&mut at)?;
    let len = skip(&mut at)?;
    let bytes = chunk.get(at..at.checked_add(len)?)?;
    Some(String::from_utf8_lossy(bytes).into_owned())
}

fn utf16_string(chunk: &[u8], mut at: usize) -> Option<String> {
    let mut len = usize::from(u16_at(chunk, at)?);
    at += 2;
    if len & 0x8000 != 0 {
        len = ((len & 0x7fff) << 16) | usize::from(u16_at(chunk, at)?);
        at += 2;
    }
    let units: Option<Vec<u16>> = (0..len).map(|i| u16_at(chunk, at + i * 2)).collect();
    Some(String::from_utf16_lossy(&units?))
}

/// A minimal binary manifest naming `package`: what the test signer puts
/// in its APKs, and what the format reader is checked against.
#[cfg(any(test, feature = "test-signer"))]
pub fn manifest_for_tests(package: &str) -> Vec<u8> {
    // String pool: 0 "manifest", 1 "package", 2 the package name (UTF-16).
    let words = ["manifest", "package", package];
    let mut data = Vec::new();
    let mut offsets = Vec::new();
    for w in words {
        offsets.push(data.len() as u32);
        let units: Vec<u16> = w.encode_utf16().collect();
        data.extend_from_slice(&(units.len() as u16).to_le_bytes());
        for u in units {
            data.extend_from_slice(&u.to_le_bytes());
        }
        data.extend_from_slice(&[0, 0]);
    }
    while data.len() % 4 != 0 {
        data.push(0);
    }
    let header = 28u16;
    let strings_start = header as u32 + 4 * words.len() as u32;
    let mut pool = Vec::new();
    pool.extend_from_slice(&RES_STRING_POOL_TYPE.to_le_bytes());
    pool.extend_from_slice(&header.to_le_bytes());
    pool.extend_from_slice(&(strings_start + data.len() as u32).to_le_bytes());
    pool.extend_from_slice(&(words.len() as u32).to_le_bytes());
    pool.extend_from_slice(&0u32.to_le_bytes()); // styles
    pool.extend_from_slice(&0u32.to_le_bytes()); // flags: UTF-16
    pool.extend_from_slice(&strings_start.to_le_bytes());
    pool.extend_from_slice(&0u32.to_le_bytes()); // styles start
    for o in offsets {
        pool.extend_from_slice(&o.to_le_bytes());
    }
    pool.extend_from_slice(&data);
    // <manifest package="..."> with one attribute.
    let mut element = Vec::new();
    element.extend_from_slice(&RES_XML_START_ELEMENT_TYPE.to_le_bytes());
    element.extend_from_slice(&16u16.to_le_bytes());
    element.extend_from_slice(&(36u32 + 20).to_le_bytes());
    element.extend_from_slice(&1u32.to_le_bytes()); // line
    element.extend_from_slice(&u32::MAX.to_le_bytes()); // comment
    element.extend_from_slice(&u32::MAX.to_le_bytes()); // ns
    element.extend_from_slice(&0u32.to_le_bytes()); // name: "manifest"
    element.extend_from_slice(&20u16.to_le_bytes()); // attributeStart
    element.extend_from_slice(&20u16.to_le_bytes()); // attributeSize
    element.extend_from_slice(&1u16.to_le_bytes()); // count
    element.extend_from_slice(&[0; 6]); // id, class, style
    element.extend_from_slice(&u32::MAX.to_le_bytes()); // attr ns
    element.extend_from_slice(&1u32.to_le_bytes()); // attr name: "package"
    element.extend_from_slice(&2u32.to_le_bytes()); // raw value
    element.extend_from_slice(&8u16.to_le_bytes());
    element.push(0);
    element.push(0x03); // string
    element.extend_from_slice(&2u32.to_le_bytes());
    let mut out = Vec::new();
    out.extend_from_slice(&RES_XML_TYPE.to_le_bytes());
    out.extend_from_slice(&8u16.to_le_bytes());
    out.extend_from_slice(&((8 + pool.len() + element.len()) as u32).to_le_bytes());
    out.extend_from_slice(&pool);
    out.extend_from_slice(&element);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_package_name_reads_out_of_a_binary_manifest() {
        let manifest = manifest_for_tests("com.example.owned");
        assert_eq!(
            package_name(&manifest).as_deref(),
            Some("com.example.owned")
        );
        assert_eq!(
            package_name(b"<manifest/>"),
            None,
            "text XML is not compiled XML"
        );
        assert_eq!(
            package_name(&manifest[..manifest.len() - 4]),
            None,
            "cut short"
        );
    }
}
