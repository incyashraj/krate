//! A small APK v2/v3 signer for tests: ECDSA P-256 keys, a self-signed
//! certificate, and an optional rotation lineage. Not a signing tool --
//! real apps are signed with apksigner, and the committed fixtures prove
//! this verifier reads what apksigner writes.

use ring::rand::SystemRandom;
use ring::signature::{EcdsaKeyPair, KeyPair, ECDSA_P256_SHA256_ASN1_SIGNING};

/// A test signing key and its certificate.
pub struct TestKey {
    pair: EcdsaKeyPair,
    pub certificate: Vec<u8>,
}

fn tlv(tag: u8, value: &[u8]) -> Vec<u8> {
    let mut out = vec![tag];
    let len = value.len();
    if len < 0x80 {
        out.push(len as u8);
    } else if len < 0x100 {
        out.extend_from_slice(&[0x81, len as u8]);
    } else {
        out.extend_from_slice(&[0x82, (len >> 8) as u8, len as u8]);
    }
    out.extend_from_slice(value);
    out
}

fn seq(parts: &[&[u8]]) -> Vec<u8> {
    tlv(0x30, &parts.concat())
}

const ECDSA_WITH_SHA256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x04, 0x03, 0x02];
const EC_PUBLIC_KEY: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x02, 0x01];
const P256: &[u8] = &[0x2a, 0x86, 0x48, 0xce, 0x3d, 0x03, 0x01, 0x07];

impl TestKey {
    /// A new key with a certificate naming `name`.
    pub fn new(name: &str) -> TestKey {
        let rng = SystemRandom::new();
        let pkcs8 = EcdsaKeyPair::generate_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, &rng).unwrap();
        let pair = EcdsaKeyPair::from_pkcs8(&ECDSA_P256_SHA256_ASN1_SIGNING, pkcs8.as_ref(), &rng)
            .unwrap();
        let mut key = TestKey {
            pair,
            certificate: Vec::new(),
        };
        let alg = seq(&[&tlv(0x06, ECDSA_WITH_SHA256)]);
        let cn = seq(&[&tlv(
            0x31,
            &seq(&[&tlv(0x06, &[0x55, 0x04, 0x03]), &tlv(0x0c, name.as_bytes())]),
        )]);
        let validity = seq(&[&tlv(0x17, b"250101000000Z"), &tlv(0x17, b"491231000000Z")]);
        let tbs = seq(&[
            &tlv(0xa0, &tlv(0x02, &[2])),
            &tlv(0x02, &[1]),
            &alg,
            &cn,
            &validity,
            &cn,
            &key.spki(),
        ]);
        let signature = key.sign(&tbs);
        let mut bits = vec![0u8];
        bits.extend_from_slice(&signature);
        key.certificate = seq(&[&tbs, &alg, &tlv(0x03, &bits)]);
        key
    }

    /// The SubjectPublicKeyInfo.
    pub fn spki(&self) -> Vec<u8> {
        let mut bits = vec![0u8];
        bits.extend_from_slice(self.pair.public_key().as_ref());
        seq(&[
            &seq(&[&tlv(0x06, EC_PUBLIC_KEY), &tlv(0x06, P256)]),
            &tlv(0x03, &bits),
        ])
    }

    fn sign(&self, message: &[u8]) -> Vec<u8> {
        self.pair
            .sign(&SystemRandom::new(), message)
            .unwrap()
            .as_ref()
            .to_vec()
    }
}

fn prefixed(bytes: &[u8]) -> Vec<u8> {
    let mut out = (bytes.len() as u32).to_le_bytes().to_vec();
    out.extend_from_slice(bytes);
    out
}

fn sequence(items: &[Vec<u8>]) -> Vec<u8> {
    items.iter().flat_map(|i| prefixed(i)).collect()
}

const ECDSA_SHA256: u32 = 0x0201;

/// How to sign.
pub struct Signing<'a> {
    pub key: &'a TestKey,
    /// v3 (true) or v2 (false).
    pub v3: bool,
    /// Keys rotated from, oldest first; the signing key is the newest.
    pub rotated_from: &'a [&'a TestKey],
    /// Put the same signer in twice: an APK with two signers.
    pub twice: bool,
    /// Name this certificate instead of the key's own: a forgery.
    pub claim: Option<&'a [u8]>,
    /// Sign each rotation link with this key instead of the one before it:
    /// a lineage claiming a rotation the old key never made.
    pub forge_links_with: Option<&'a TestKey>,
}

/// Sign an unsigned APK (a zip with no signing block).
pub fn sign(unsigned: &[u8], how: Signing<'_>) -> Vec<u8> {
    let layout = crate::zip::layout(unsigned).expect("a zip");
    assert!(layout.signing_block.is_none(), "already signed");
    let digest = crate::content_digest(unsigned, &layout, &ring::digest::SHA256);
    let digests = sequence(&[[ECDSA_SHA256.to_le_bytes().to_vec(), prefixed(&digest)].concat()]);
    let certificates = sequence(&[how.claim.unwrap_or(&how.key.certificate).to_vec()]);
    let mut attributes = Vec::new();
    if !how.rotated_from.is_empty() {
        let mut chain: Vec<&TestKey> = how.rotated_from.to_vec();
        chain.push(how.key);
        let mut nodes = Vec::new();
        for (i, key) in chain.iter().enumerate() {
            let signed = [
                prefixed(&key.certificate),
                ECDSA_SHA256.to_le_bytes().to_vec(),
            ]
            .concat();
            let signature = match (i, how.forge_links_with) {
                (0, _) => Vec::new(),
                (_, Some(forger)) => forger.sign(&signed),
                (_, None) => chain[i - 1].sign(&signed),
            };
            nodes.push(
                [
                    prefixed(&signed),
                    0u32.to_le_bytes().to_vec(),
                    ECDSA_SHA256.to_le_bytes().to_vec(),
                    prefixed(&signature),
                ]
                .concat(),
            );
        }
        let lineage = [1u32.to_le_bytes().to_vec(), sequence(&nodes)].concat();
        attributes.push([0x3ba0_6f8cu32.to_le_bytes().to_vec(), lineage].concat());
    }
    let sdk = |v: u32| v.to_le_bytes().to_vec();
    let signed_data = if how.v3 {
        [
            prefixed(&digests),
            prefixed(&certificates),
            sdk(24),
            sdk(0x7fff_ffff),
            prefixed(&sequence(&attributes)),
        ]
        .concat()
    } else {
        [
            prefixed(&digests),
            prefixed(&certificates),
            prefixed(&sequence(&attributes)),
        ]
        .concat()
    };
    let signature = how.key.sign(&signed_data);
    let signatures =
        sequence(&[[ECDSA_SHA256.to_le_bytes().to_vec(), prefixed(&signature)].concat()]);
    let signer = if how.v3 {
        [
            prefixed(&signed_data),
            sdk(24),
            sdk(0x7fff_ffff),
            prefixed(&signatures),
            prefixed(&how.key.spki()),
        ]
        .concat()
    } else {
        [
            prefixed(&signed_data),
            prefixed(&signatures),
            prefixed(&how.key.spki()),
        ]
        .concat()
    };
    let signers = if how.twice {
        vec![signer.clone(), signer]
    } else {
        vec![signer]
    };
    let value = prefixed(&sequence(&signers));
    let id: u32 = if how.v3 { 0xf053_68c0 } else { 0x7109_871a };
    let pair = [
        ((4 + value.len()) as u64).to_le_bytes().to_vec(),
        id.to_le_bytes().to_vec(),
        value,
    ]
    .concat();
    let size = (pair.len() + 8 + 16) as u64;
    let block = [
        size.to_le_bytes().to_vec(),
        pair,
        size.to_le_bytes().to_vec(),
        b"APK Sig Block 42".to_vec(),
    ]
    .concat();
    let mut out = unsigned[..layout.cd_offset].to_vec();
    out.extend_from_slice(&block);
    out.extend_from_slice(&unsigned[layout.cd_offset..layout.eocd_offset]);
    let mut eocd = unsigned[layout.eocd_offset..].to_vec();
    let moved = (layout.cd_offset + block.len()) as u32;
    eocd[16..20].copy_from_slice(&moved.to_le_bytes());
    out.extend_from_slice(&eocd);
    out
}

/// A small unsigned APK: a manifest naming `package` and a `classes.dex`.
pub fn unsigned_apk(package: &str, dex: &[u8]) -> Vec<u8> {
    let manifest = crate::axml::manifest_for_tests(package);
    crate::zip::write_stored(&[("AndroidManifest.xml", &manifest), ("classes.dex", dex)])
}
