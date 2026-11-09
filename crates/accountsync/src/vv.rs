//! Version vectors: one counter per device.
//!
//! Each device counts its own writes of a key. A write carries the counts
//! of every device the writer had seen, so comparing two vectors answers
//! the one question a merge needs: was one of these writes made knowing the
//! other (take the later), or were they made apart (a conflict)? Clocks do
//! not enter into it.

use std::collections::BTreeMap;

/// How two versions of one key stand to each other.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Order {
    /// The same version.
    Equal,
    /// The left one is older: the right was made knowing it.
    Before,
    /// The left one is newer.
    After,
    /// Made apart, neither knowing the other.
    Concurrent,
}

/// `{device id: how many writes of this key that device made}`. A device
/// absent counts zero.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(transparent)]
pub struct VersionVector(pub BTreeMap<String, u64>);

/// The most devices one vector may name: far past anybody's devices, and a
/// bound the hub holds too.
pub const MAX_DEVICES: usize = 64;

impl VersionVector {
    pub fn new() -> VersionVector {
        VersionVector::default()
    }

    pub fn get(&self, device: &str) -> u64 {
        self.0.get(device).copied().unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.0.values().all(|n| *n == 0)
    }

    /// This device made one more write.
    pub fn bump(&mut self, device: &str) {
        *self.0.entry(device.to_string()).or_insert(0) += 1;
    }

    /// Everything either has seen.
    pub fn join(&mut self, other: &VersionVector) {
        for (device, n) in &other.0 {
            let mine = self.0.entry(device.clone()).or_insert(0);
            *mine = (*mine).max(*n);
        }
    }

    /// True when this vector has seen everything `other` has: `other` is
    /// the same version or an older one.
    pub fn covers(&self, other: &VersionVector) -> bool {
        other.0.iter().all(|(device, n)| self.get(device) >= *n)
    }

    pub fn compare(&self, other: &VersionVector) -> Order {
        match (self.covers(other), other.covers(self)) {
            (true, true) => Order::Equal,
            (true, false) => Order::After,
            (false, true) => Order::Before,
            (false, false) => Order::Concurrent,
        }
    }

    /// One line, the same on every device: `id:n,id:n`, sorted, zeros
    /// dropped. Sealed into each entry, so the hub cannot change a vector
    /// without the entry refusing to open.
    pub fn canonical(&self) -> String {
        self.0
            .iter()
            .filter(|(_, n)| **n > 0)
            .map(|(device, n)| format!("{device}:{n}"))
            .collect::<Vec<_>>()
            .join(",")
    }

    /// Read a vector as the hub sends it, refusing anything a device could
    /// not have written: device ids are 16 lower-case hex, counts positive
    /// and below 2^53 (a JavaScript number), at most [`MAX_DEVICES`].
    pub fn from_json(value: &serde_json::Value) -> Option<VersionVector> {
        let object = value.as_object()?;
        if object.len() > MAX_DEVICES {
            return None;
        }
        let mut out = BTreeMap::new();
        for (device, n) in object {
            let n = n.as_u64()?;
            if !valid_device_id(device) || n == 0 || n > (1u64 << 53) {
                return None;
            }
            out.insert(device.clone(), n);
        }
        Some(VersionVector(out))
    }

    pub fn to_json(&self) -> serde_json::Value {
        serde_json::Value::Object(
            self.0
                .iter()
                .filter(|(_, n)| **n > 0)
                .map(|(device, n)| (device.clone(), serde_json::Value::from(*n)))
                .collect(),
        )
    }
}

/// A device id as crates/orgkeys makes it: 16 lower-case hex.
pub fn valid_device_id(id: &str) -> bool {
    id.len() == 16
        && id
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vv(pairs: &[(&str, u64)]) -> VersionVector {
        VersionVector(pairs.iter().map(|(d, n)| (d.to_string(), *n)).collect())
    }

    #[test]
    fn comparing_tells_after_from_apart() {
        let a1 = vv(&[("a", 1)]);
        let a2 = vv(&[("a", 2)]);
        let a1b1 = vv(&[("a", 1), ("b", 1)]);
        let b1 = vv(&[("b", 1)]);
        assert_eq!(a1.compare(&a1), Order::Equal);
        assert_eq!(a1.compare(&a2), Order::Before);
        assert_eq!(a2.compare(&a1), Order::After);
        assert_eq!(a1.compare(&a1b1), Order::Before);
        assert_eq!(a2.compare(&a1b1), Order::Concurrent);
        assert_eq!(a1.compare(&b1), Order::Concurrent);
        assert_eq!(VersionVector::new().compare(&a1), Order::Before);
        // A zero entry is the same as no entry.
        assert_eq!(vv(&[("a", 1), ("b", 0)]).compare(&a1), Order::Equal);
    }

    #[test]
    fn join_and_bump() {
        let mut v = vv(&[("a", 2)]);
        v.join(&vv(&[("a", 1), ("b", 3)]));
        assert_eq!(v, vv(&[("a", 2), ("b", 3)]));
        v.bump("a");
        assert_eq!(v.get("a"), 3);
        assert_eq!(v.canonical(), "a:3,b:3");
    }

    #[test]
    fn the_wire_form_refuses_what_no_device_writes() {
        let id = "0123456789abcdef";
        let good = serde_json::json!({ id: 3 });
        assert_eq!(
            VersionVector::from_json(&good).unwrap(),
            vv(&[(id, 3)]),
            "round trip"
        );
        assert_eq!(VersionVector::from_json(&good).unwrap().to_json(), good);
        for bad in [
            serde_json::json!({ "not-hex-device!": 1 }),
            serde_json::json!({ id: 0 }),
            serde_json::json!({ id: -1 }),
            serde_json::json!({ id: "1" }),
            serde_json::json!([1]),
        ] {
            assert!(VersionVector::from_json(&bad).is_none(), "{bad}");
        }
    }
}
