//! The "merge" choice for a conflict whose two values are JSON: a reminders
//! list changed on the Mac and on the phone keeps the items of both.
//!
//! - Two objects: every field of either; a field both have is merged again
//!   when both sides are objects (or both lists), and otherwise the other
//!   device's value is taken -- the person chose to merge in what the other
//!   device did, so its fields win where the two disagree.
//! - Two lists: this device's items in their order, then each of the other's
//!   that this list does not already hold.
//! - Anything else is not offered: two numbers or two pieces of text have no
//!   merge that is not just picking one, which the other choices already do.

use serde_json::Value;

/// Merge `other` into `mine`, or `None` when the two are not both objects
/// or both lists.
pub fn merge(mine: &Value, other: &Value) -> Option<Value> {
    match (mine, other) {
        (Value::Object(_), Value::Object(_)) | (Value::Array(_), Value::Array(_)) => {
            Some(merge_inner(mine, other))
        }
        _ => None,
    }
}

fn merge_inner(mine: &Value, other: &Value) -> Value {
    match (mine, other) {
        (Value::Object(a), Value::Object(b)) => {
            let mut out = a.clone();
            for (field, theirs) in b {
                let merged = match out.get(field) {
                    Some(ours) => merge_inner(ours, theirs),
                    None => theirs.clone(),
                };
                out.insert(field.clone(), merged);
            }
            Value::Object(out)
        }
        (Value::Array(a), Value::Array(b)) => {
            let mut out = a.clone();
            for item in b {
                if !out.contains(item) {
                    out.push(item.clone());
                }
            }
            Value::Array(out)
        }
        // Two different kinds, or two plain values: the other device's.
        (_, theirs) => theirs.clone(),
    }
}

/// [`merge`] over bytes: both must be JSON text. The result is compact JSON.
pub fn merge_bytes(mine: &[u8], other: &[u8]) -> Option<Vec<u8>> {
    let mine: Value = serde_json::from_slice(mine).ok()?;
    let other: Value = serde_json::from_slice(other).ok()?;
    serde_json::to_vec(&merge(&mine, &other)?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn objects_keep_every_field_and_the_other_wins_a_clash() {
        let mine = json!({ "title": "Milk", "done": false, "list": "home" });
        let other = json!({ "title": "Oat milk", "due": "fri" });
        assert_eq!(
            merge(&mine, &other).unwrap(),
            json!({ "title": "Oat milk", "done": false, "list": "home", "due": "fri" })
        );
    }

    #[test]
    fn lists_are_a_union_in_this_devices_order() {
        let mine = json!(["milk", "eggs"]);
        let other = json!(["eggs", "bread", "milk", "jam"]);
        assert_eq!(
            merge(&mine, &other).unwrap(),
            json!(["milk", "eggs", "bread", "jam"])
        );
    }

    #[test]
    fn nested_values_merge_all_the_way_down() {
        let mine = json!({ "items": [{ "id": 1 }], "meta": { "a": 1, "b": 1 } });
        let other = json!({ "items": [{ "id": 2 }], "meta": { "b": 2, "c": 3 } });
        assert_eq!(
            merge(&mine, &other).unwrap(),
            json!({ "items": [{ "id": 1 }, { "id": 2 }], "meta": { "a": 1, "b": 2, "c": 3 } })
        );
    }

    #[test]
    fn plain_values_and_mixed_kinds_are_not_merged() {
        assert!(merge(&json!(1), &json!(2)).is_none());
        assert!(merge(&json!("a"), &json!("b")).is_none());
        assert!(merge(&json!({}), &json!([])).is_none());
        assert!(merge_bytes(b"not json", b"{}").is_none());
        assert_eq!(
            merge_bytes(br#"{"a":1}"#, br#"{"b":2}"#).unwrap(),
            br#"{"a":1,"b":2}"#.to_vec()
        );
    }
}
