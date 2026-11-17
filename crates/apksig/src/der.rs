//! Just enough DER to find a certificate's SubjectPublicKeyInfo and a
//! key's algorithm. Nothing here trusts a length it has not bounded.

/// One TLV: its tag, its contents, and the whole encoding.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Tlv<'a> {
    pub tag: u8,
    pub value: &'a [u8],
    pub whole: &'a [u8],
}

/// Read one TLV from the front of `input`, returning it and the rest.
pub(crate) fn read(input: &[u8]) -> Option<(Tlv<'_>, &[u8])> {
    let tag = *input.first()?;
    // High-tag-number form is not used by anything read here.
    if tag & 0x1f == 0x1f {
        return None;
    }
    let first = *input.get(1)? as usize;
    let (len, header) = if first < 0x80 {
        (first, 2)
    } else {
        let count = first & 0x7f;
        if count == 0 || count > 4 {
            return None;
        }
        let mut len = 0usize;
        for i in 0..count {
            len = (len << 8) | *input.get(2 + i)? as usize;
        }
        (len, 2 + count)
    };
    let end = header.checked_add(len)?;
    if end > input.len() {
        return None;
    }
    Some((
        Tlv {
            tag,
            value: &input[header..end],
            whole: &input[..end],
        },
        &input[end..],
    ))
}

/// Every TLV inside a constructed value.
pub(crate) fn children(mut input: &[u8]) -> Option<Vec<Tlv<'_>>> {
    let mut out = Vec::new();
    while !input.is_empty() {
        let (tlv, rest) = read(input)?;
        out.push(tlv);
        input = rest;
    }
    Some(out)
}

const SEQUENCE: u8 = 0x30;

/// A certificate's SubjectPublicKeyInfo, as encoded in it.
///
/// Certificate ::= SEQUENCE { tbsCertificate, signatureAlgorithm, signature }
/// TBSCertificate ::= SEQUENCE { [0] version OPTIONAL, serialNumber,
///   signature, issuer, validity, subject, subjectPublicKeyInfo, ... }
pub(crate) fn certificate_spki(cert: &[u8]) -> Option<&[u8]> {
    let (outer, rest) = read(cert)?;
    if outer.tag != SEQUENCE || !rest.is_empty() {
        return None;
    }
    let parts = children(outer.value)?;
    let tbs = parts.first().filter(|t| t.tag == SEQUENCE)?;
    let fields = children(tbs.value)?;
    // Skip the explicit [0] version when it is there.
    let skip = usize::from(fields.first()?.tag == 0xa0);
    let spki = fields.get(skip + 5)?;
    (spki.tag == SEQUENCE).then_some(spki.whole)
}

/// What a SubjectPublicKeyInfo holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum KeyKind {
    Rsa,
    EcP256,
    EcP384,
    Dsa,
    Other,
}

/// The algorithm and the raw key bits a SubjectPublicKeyInfo carries:
/// PKCS#1 RSAPublicKey for RSA, the uncompressed point for EC -- the forms
/// ring's verifier takes.
pub(crate) fn spki_key(spki: &[u8]) -> Option<(KeyKind, &[u8])> {
    const RSA: &[u8] = &[0x2a, 0x86, 0x48, 0x86, 0xf7, 0x0d, 0x01, 0x01, 0x01];
    const EC: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
    const P256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];
    const P384: &[u8] = &[0x2b, 0x81, 0x04, 0x00, 0x22];
    const DSA: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x38, 0x04, 0x01];
    let (outer, rest) = read(spki)?;
    if outer.tag != SEQUENCE || !rest.is_empty() {
        return None;
    }
    let parts = children(outer.value)?;
    let algorithm = children(parts.first().filter(|t| t.tag == SEQUENCE)?.value)?;
    let bits = parts.get(1).filter(|t| t.tag == 0x03)?;
    // A BIT STRING's first byte counts unused bits; keys use whole bytes.
    let (&unused, key) = bits.value.split_first()?;
    if unused != 0 {
        return None;
    }
    let oid = algorithm.first().filter(|t| t.tag == 0x06)?.value;
    let kind = if oid == RSA {
        KeyKind::Rsa
    } else if oid == EC {
        match algorithm.get(1).filter(|t| t.tag == 0x06).map(|t| t.value) {
            Some(curve) if curve == P256 => KeyKind::EcP256,
            Some(curve) if curve == P384 => KeyKind::EcP384,
            _ => KeyKind::Other,
        }
    } else if oid == DSA {
        KeyKind::Dsa
    } else {
        KeyKind::Other
    };
    Some((kind, key))
}

/// The modulus size of a PKCS#1 RSAPublicKey, in bits.
pub(crate) fn rsa_bits(pkcs1: &[u8]) -> Option<usize> {
    let (outer, _) = read(pkcs1)?;
    let parts = children(outer.value)?;
    let modulus = parts.first().filter(|t| t.tag == 0x02)?.value;
    let significant = modulus.iter().skip_while(|b| **b == 0).count();
    Some(significant * 8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_length_past_the_end_is_refused() {
        assert!(read(&[0x30, 0x05, 0x01]).is_none());
        assert!(read(&[0x30, 0x84, 0xff, 0xff, 0xff, 0xff]).is_none());
        assert!(
            read(&[0x30, 0x80]).is_none(),
            "indefinite length is not DER"
        );
        let (tlv, rest) = read(&[0x04, 0x02, 0xaa, 0xbb, 0x00]).unwrap();
        assert_eq!(
            (tlv.tag, tlv.value, rest),
            (0x04, &[0xaa, 0xbb][..], &[0x00][..])
        );
    }
}
