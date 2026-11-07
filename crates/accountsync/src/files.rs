//! Stage S3: an app's own files -- its sandbox -- on every device of the
//! person, by the same rule as its `store.kv` (F-321).
//!
//! A file is one entry, and a version is the whole file. The replica
//! ([`crate::Replica`]) holds, per path, a [`FileValue`] -- the SHA-256 of
//! the contents and their size -- in place of the value itself, so two
//! devices that wrote the same bytes agree, and the merge rule, the
//! conflicts and the four resolutions are exactly the ones `store.kv`
//! has. The bodies travel apart from the entries:
//!
//! - **the entry** (the hub's per-app bucket, in KV, like `store.kv`'s): its
//!   name an HMAC of the path, sealed inside it the path, the [`FileValue`],
//!   when the file was written, and which body holds the contents;
//! - **the body** (the hub's object store, R2, where it keeps every other
//!   large thing): the contents sealed under the app's file key, named by
//!   the SHA-256 of the sealed bytes, which says nothing about the file.
//!
//! The hub learns neither the app, nor the paths, nor the contents; it
//! sees how many files there are and roughly how large each is.
//!
//! A file larger than [`FILE_MAX`] stays on the device it is on, and the
//! person is told so ([`too_big_words`]).

use krate_orgkeys::{AppDataKey, KeyError, Keyring, RowKind};

use crate::replica::Version;
use crate::seal::Sealed;

/// The largest file that follows the person: 16 MiB. Larger ones stay on
/// the device they are on. The hub holds the same bound
/// (cloud/worker/src/account-sync.js `BLOB_MAX`).
pub const FILE_MAX: u64 = 16 * 1024 * 1024;

/// The most files one app's folder syncs; past this the rest stay on the
/// device they are on, first by path.
pub const MAX_FILES: usize = 2000;

/// The longest path that syncs, in bytes, and the longest one part of it.
pub const MAX_PATH_BYTES: usize = 1024;
pub const MAX_NAME_BYTES: usize = 255;

/// What sync writes while it puts a file in place, and so never syncs.
pub const TEMP_PREFIX: &str = ".krate-sync";

/// "Files over 16 MB stay on this device", for the person.
pub fn too_big_words() -> String {
    format!(
        "Files over {} MB stay on this device.",
        FILE_MAX / (1024 * 1024)
    )
}

/// One version of a file as the replica holds it: its contents' SHA-256
/// and size. Two versions with the same contents are the same value.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileValue {
    pub hash: String,
    pub size: u64,
}

impl FileValue {
    pub fn of(bytes: &[u8]) -> FileValue {
        FileValue {
            hash: crate::value_hash(bytes),
            size: bytes.len() as u64,
        }
    }

    /// `<sha256 hex>:<size>`: what the replica keeps as the value.
    pub fn encode(&self) -> Vec<u8> {
        format!("{}:{}", self.hash, self.size).into_bytes()
    }

    pub fn decode(bytes: &[u8]) -> Option<FileValue> {
        let text = std::str::from_utf8(bytes).ok()?;
        let (hash, size) = text.split_once(':')?;
        let good = hash.len() == 64
            && hash
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b));
        if !good {
            return None;
        }
        Some(FileValue {
            hash: hash.to_string(),
            size: size.parse().ok()?,
        })
    }
}

/// Whether `path` is one sync may read or write in an app's folder:
/// relative, `/` between its parts, no part empty, `.` or `..`, no
/// backslash, colon or control character, not sync's own temporary name,
/// and within [`MAX_PATH_BYTES`] and [`MAX_NAME_BYTES`]. A path from
/// another device that fails this is never written, whatever it says.
pub fn valid_path(path: &str) -> bool {
    if path.is_empty() || path.len() > MAX_PATH_BYTES {
        return false;
    }
    path.split('/').all(|part| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && part.len() <= MAX_NAME_BYTES
            && !part.starts_with(TEMP_PREFIX)
            && !part
                .chars()
                .any(|c| c.is_control() || c == '\\' || c == ':')
    })
}

/// The name "keep both" gives the other device's copy of `path`: the
/// device's name in brackets before the extension, in the same folder --
/// `notes/photo (Galaxy S22 Ultra).jpg` -- numbered when that is taken,
/// and always a [`valid_path`].
pub fn both_file_name(path: &str, device_name: &str, taken: &dyn Fn(&str) -> bool) -> String {
    let device: String = device_name
        .chars()
        .map(|c| {
            if c.is_control() || "/\\:*?\"<>|".contains(c) {
                ' '
            } else {
                c
            }
        })
        .collect();
    let mut device = device.replace("..", ".");
    while device.contains("..") {
        device = device.replace("..", ".");
    }
    let device = device.trim().trim_matches('.').trim().to_string();
    let device = if device.is_empty() {
        "other device".to_string()
    } else {
        device
    };
    let (folder, name) = match path.rsplit_once('/') {
        Some((folder, name)) => (Some(folder), name),
        None => (None, path),
    };
    // `photo.jpg` -> (`photo`, `.jpg`); `.profile` and `README` have none.
    let (stem, ext) = match name.rfind('.') {
        Some(dot) if dot > 0 => (&name[..dot], &name[dot..]),
        _ => (name, ""),
    };
    for n in 1.. {
        let suffix = if n == 1 {
            format!(" ({device})")
        } else {
            format!(" ({device} {n})")
        };
        let room = MAX_NAME_BYTES.saturating_sub(suffix.len() + ext.len());
        let mut base = stem.to_string();
        while base.len() > room {
            base.pop();
        }
        let file = format!("{base}{suffix}{ext}");
        let candidate = match folder {
            Some(folder) => format!("{folder}/{file}"),
            None => file,
        };
        if valid_path(&candidate) && !taken(&candidate) {
            return candidate;
        }
        if n > 10_000 {
            break;
        }
    }
    // Every name taken (or a device name nothing can be made of): a plain
    // numbered one, still beside the original.
    let file = format!("{} (other device).bin", crate::value_hash(path.as_bytes()));
    match folder {
        Some(folder) => format!("{folder}/{}", &file[..file.len().min(MAX_NAME_BYTES)]),
        None => file,
    }
}

/// Where a version's contents are: the body's name on the hub, the version
/// of the account key it was sealed with, and when the file was written
/// (seconds since 1970, the writer's clock: for the person, never for the
/// merge).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Body {
    pub blob: String,
    pub k: u32,
    pub at: Option<u64>,
}

/// One app's file keys, for one account.
pub struct FileKey {
    keyring: Keyring,
    scope: String,
    bucket: String,
}

impl std::fmt::Debug for FileKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "FileKey({})", self.bucket)
    }
}

/// What a body's name is: the SHA-256 of its sealed bytes, 64 hex.
pub fn blob_id(sealed: &[u8]) -> String {
    crate::value_hash(sealed)
}

const BODY_FIELD: &str = "file body";

impl FileKey {
    /// The file keys of the app `app_id` by `publisher`: a bucket and a key
    /// of their own, apart from the app's `store.kv`.
    pub fn derive(keyring: &Keyring, publisher: &str, app_id: &str) -> FileKey {
        let scope = format!("account files\n{publisher}\n{app_id}");
        let first = *keyring.versions().first().expect("a keyring has a version");
        let names = AppDataKey::derive(keyring, first, "account buckets")
            .expect("the keyring holds its own first version");
        let bucket = names.record_key("file bucket", &format!("{publisher}\n{app_id}"));
        FileKey {
            keyring: keyring.clone(),
            scope,
            bucket,
        }
    }

    pub fn bucket(&self) -> &str {
        &self.bucket
    }

    fn data(&self, version: u32) -> Option<AppDataKey> {
        AppDataKey::derive(&self.keyring, version, &self.scope)
    }

    fn current(&self) -> AppDataKey {
        self.data(self.keyring.current().0)
            .expect("the keyring holds its current version")
    }

    /// The name a file's entry goes by on the hub: 64 hex.
    pub fn name_of(&self, path: &str) -> String {
        let first = *self.keyring.versions().first().expect("a version");
        self.data(first)
            .expect("the keyring holds its first version")
            .record_key("file", path)
    }

    fn bound(version: &Version) -> String {
        format!("{}|{}", version.vv.canonical(), version.by)
    }

    /// Seal the contents of `path`: the bytes to send and the name they go
    /// by. The nonce is fresh each time, so the same file sealed twice is
    /// two unrelated bodies.
    pub fn seal_body(&self, path: &str, contents: &[u8]) -> Result<(Vec<u8>, Body), KeyError> {
        let data = self.current();
        let sealed = data.seal(&self.name_of(path), BODY_FIELD, RowKind::Value, contents)?;
        let raw = krate_orgkeys::b64_decode(&sealed).ok_or(KeyError::Malformed("sealed body"))?;
        let blob = blob_id(&raw);
        Ok((
            raw,
            Body {
                blob,
                k: data.version(),
                at: None,
            },
        ))
    }

    /// Open a body of `path`, refusing one whose contents are not `value`.
    pub fn open_body(
        &self,
        path: &str,
        body: &Body,
        raw: &[u8],
        value: &FileValue,
    ) -> Result<Vec<u8>, KeyError> {
        if blob_id(raw) != body.blob {
            return Err(KeyError::Open);
        }
        let data = self.data(body.k).ok_or(KeyError::Open)?;
        let contents = data.open(
            &self.name_of(path),
            BODY_FIELD,
            RowKind::Value,
            &krate_orgkeys::b64_encode(raw),
        )?;
        if FileValue::of(&contents) != *value {
            return Err(KeyError::Open);
        }
        Ok(contents)
    }

    /// Seal one version of `path`'s entry. `body` is where its contents are
    /// (`None` for a deletion).
    pub fn seal(
        &self,
        path: &str,
        version: &Version,
        body: Option<&Body>,
        at: Option<u64>,
    ) -> Result<Sealed, KeyError> {
        let data = self.current();
        let deleted = version.value.is_none();
        if deleted != body.is_none() {
            return Err(KeyError::Malformed("a file version names its body"));
        }
        let value = match &version.value {
            Some(v) => {
                Some(String::from_utf8(v.clone()).map_err(|_| KeyError::Malformed("file value"))?)
            }
            None => None,
        };
        let plain = serde_json::json!({
            "p": path,
            "v": value,
            "b": body.map(|b| b.blob.clone()),
            "bk": body.map(|b| b.k),
            "t": at,
        })
        .to_string();
        let c = data.seal(
            &self.name_of(path),
            &Self::bound(version),
            if deleted {
                RowKind::Tombstone
            } else {
                RowKind::Value
            },
            plain.as_bytes(),
        )?;
        Ok(Sealed {
            vv: version.vv.clone(),
            by: version.by.clone(),
            deleted,
            k: data.version(),
            c,
        })
    }

    /// Open one version the hub keeps under `name`: the path, the version,
    /// and where its contents are (`None` for a deletion) with when it was
    /// written.
    pub fn open(
        &self,
        name: &str,
        sealed: &Sealed,
    ) -> Result<(String, Version, Option<Body>, Option<u64>), KeyError> {
        let data = self.data(sealed.k).ok_or(KeyError::Open)?;
        let probe = Version {
            value: None,
            vv: sealed.vv.clone(),
            by: sealed.by.clone(),
        };
        let plain = data.open(
            name,
            &Self::bound(&probe),
            if sealed.deleted {
                RowKind::Tombstone
            } else {
                RowKind::Value
            },
            &sealed.c,
        )?;
        let bad = || KeyError::Malformed("sealed file entry");
        let entry: serde_json::Value = serde_json::from_slice(&plain).map_err(|_| bad())?;
        let path = entry["p"].as_str().ok_or_else(bad)?.to_string();
        if self.name_of(&path) != name || !valid_path(&path) {
            return Err(KeyError::Open);
        }
        let at = entry["t"].as_u64();
        let (value, body) = match (&entry["v"], sealed.deleted) {
            (serde_json::Value::Null, true) => (None, None),
            (serde_json::Value::String(text), false) => {
                FileValue::decode(text.as_bytes()).ok_or_else(bad)?;
                let blob = entry["b"].as_str().ok_or_else(bad)?;
                if blob.len() != 64 || !blob.bytes().all(|b| b.is_ascii_hexdigit()) {
                    return Err(bad());
                }
                let k = u32::try_from(entry["bk"].as_u64().ok_or_else(bad)?).map_err(|_| bad())?;
                (
                    Some(text.as_bytes().to_vec()),
                    Some(Body {
                        blob: blob.to_string(),
                        k,
                        at,
                    }),
                )
            }
            _ => return Err(bad()),
        };
        Ok((
            path,
            Version {
                value,
                vv: sealed.vv.clone(),
                by: sealed.by.clone(),
            },
            body,
            at,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::seal::new_account_key;
    use crate::vv::VersionVector;

    const A: &str = "aaaaaaaaaaaaaaaa";

    fn version(value: Option<&FileValue>, n: u64) -> Version {
        let mut vv = VersionVector::new();
        for _ in 0..n {
            vv.bump(A);
        }
        Version {
            value: value.map(FileValue::encode),
            vv,
            by: A.to_string(),
        }
    }

    #[test]
    fn a_file_value_is_its_contents_hash_and_size() {
        let v = FileValue::of(b"hello");
        assert_eq!(v.size, 5);
        assert_eq!(FileValue::decode(&v.encode()), Some(v.clone()));
        assert_eq!(FileValue::of(b"hello"), v, "same bytes, same value");
        assert_ne!(FileValue::of(b"hellO"), v);
        for bad in [&b"abc:1"[..], b"", b"x", &[0xff, 0x00]] {
            assert_eq!(FileValue::decode(bad), None);
        }
    }

    #[test]
    fn only_a_plain_path_inside_the_folder_is_one_sync_writes() {
        for good in ["a.txt", "notes/today.md", ".config", "a b/c (1).jpg"] {
            assert!(valid_path(good), "{good}");
        }
        for bad in [
            "",
            "/etc/passwd",
            "../outside",
            "a/../../b",
            "a//b",
            "./a",
            "a\\b",
            "c:\\x",
            "a/\u{0}b",
            ".krate-sync-1.tmp",
            "dir/.krate-sync.x",
        ] {
            assert!(!valid_path(bad), "{bad:?}");
        }
        assert!(!valid_path(&"a".repeat(MAX_NAME_BYTES + 1)));
        assert!(!valid_path(&"a/".repeat(600)));
    }

    #[test]
    fn keep_both_names_the_other_copy_after_its_device_before_the_extension() {
        let none = |_: &str| false;
        assert_eq!(
            both_file_name("photo.jpg", "Galaxy S22 Ultra", &none),
            "photo (Galaxy S22 Ultra).jpg"
        );
        assert_eq!(
            both_file_name("trips/2026/photo.jpg", "Mac", &none),
            "trips/2026/photo (Mac).jpg"
        );
        assert_eq!(both_file_name("README", "Mac", &none), "README (Mac)");
        assert_eq!(both_file_name(".profile", "Mac", &none), ".profile (Mac)");
        assert_eq!(
            both_file_name("a.tar.gz", "Mac", &none),
            "a.tar (Mac).gz",
            "the last extension"
        );
        let taken = |p: &str| p == "photo (Mac).jpg" || p == "photo (Mac 2).jpg";
        assert_eq!(
            both_file_name("photo.jpg", "Mac", &taken),
            "photo (Mac 3).jpg"
        );
        assert_eq!(
            both_file_name("photo.jpg", "a/b\\c:..d", &none),
            "photo (a b c .d).jpg"
        );
        assert_eq!(
            both_file_name("photo.jpg", "  ", &none),
            "photo (other device).jpg"
        );
        let long = format!("{}.jpg", "p".repeat(300));
        let name = both_file_name(&long, "Galaxy S22 Ultra", &none);
        assert!(name.len() <= MAX_NAME_BYTES && name.ends_with(" (Galaxy S22 Ultra).jpg"));
        assert!(valid_path(&name));
    }

    #[test]
    fn an_entry_and_its_body_open_with_the_accounts_key_and_nothing_else() {
        let ring = new_account_key("u1");
        let files = FileKey::derive(&ring, "unsigned", "dev.krate.notes");
        let contents = b"Dear diary, today the files learned to travel.";
        let value = FileValue::of(contents);
        let (raw, mut body) = files.seal_body("diary/today.txt", contents).unwrap();
        body.at = Some(1_700_000_000);
        let v = version(Some(&value), 1);
        let sealed = files
            .seal("diary/today.txt", &v, Some(&body), body.at)
            .unwrap();
        let name = files.name_of("diary/today.txt");
        let (path, opened, got_body, at) = files.open(&name, &sealed).unwrap();
        assert_eq!((path.as_str(), &opened), ("diary/today.txt", &v));
        assert_eq!(got_body.as_ref(), Some(&body));
        assert_eq!(at, Some(1_700_000_000));
        assert_eq!(
            files.open_body(&path, &body, &raw, &value).unwrap(),
            contents
        );

        // Nothing the hub holds says the path, the contents or the app.
        let wire = serde_json::json!({
            "bucket": files.bucket(), "name": name, "entry": sealed.to_json(), "blob": body.blob,
            "body": krate_orgkeys::b64_encode(&raw),
        })
        .to_string();
        for plain in [
            "diary",
            "today.txt",
            "Dear diary",
            "dev.krate.notes",
            "unsigned",
        ] {
            assert!(!wire.contains(plain), "{plain:?} in {wire}");
            let b64 = krate_orgkeys::b64_encode(plain.as_bytes());
            assert!(!wire.contains(&b64[..b64.len().saturating_sub(4)]));
        }
        assert!(!raw.windows(10).any(|w| w == &contents[..10]));

        // A body under another path, other contents, another account or a
        // changed byte does not open.
        assert!(files
            .open_body("diary/other.txt", &body, &raw, &value)
            .is_err());
        assert!(files
            .open_body(&path, &body, &raw, &FileValue::of(b"other"))
            .is_err());
        let mut flipped = raw.clone();
        flipped[30] ^= 1;
        assert!(files.open_body(&path, &body, &flipped, &value).is_err());
        let stranger = FileKey::derive(&new_account_key("u1"), "unsigned", "dev.krate.notes");
        assert_ne!(stranger.bucket(), files.bucket());
        assert!(stranger.open(&name, &sealed).is_err());
        assert!(stranger.open_body(&path, &body, &raw, &value).is_err());
        // The app's files are not its store.kv, nor another app's files.
        let kv = crate::seal::AppKey::derive(&ring, "unsigned", "dev.krate.notes");
        assert_ne!(kv.bucket(), files.bucket());
        assert_ne!(kv.name_of("diary/today.txt"), name);
        let other = FileKey::derive(&ring, "unsigned", "dev.krate.reminders");
        assert!(other.open(&name, &sealed).is_err());

        // A deletion carries no body, and a changed vector does not open.
        let gone = version(None, 2);
        let sealed = files.seal("diary/today.txt", &gone, None, None).unwrap();
        assert!(sealed.deleted);
        assert_eq!(files.open(&name, &sealed).unwrap().2, None);
        let mut inflated = sealed.clone();
        inflated.vv.bump(A);
        assert!(files.open(&name, &inflated).is_err());
        assert!(files.seal("x", &gone, Some(&body), None).is_err());
    }
}
