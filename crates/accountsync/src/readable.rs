//! An app's stored value, and a time, as a person reads them: what a
//! conflict card shows for "this device's" and "the other's", and "last
//! synced 2 minutes ago". The same on a phone's home and on a computer's
//! `krate account sync conflicts`.

use serde_json::Value;

/// `value` in a few words: text as it is, a record as `title: Milk, done:
/// yes`, a list as its items, anything else by its size. At most `max`
/// characters, cut with an ellipsis.
pub fn preview(value: &[u8], max: usize) -> String {
    let text = match std::str::from_utf8(value) {
        Ok(text) => text,
        Err(_) => return format!("{} of data", size(value.len())),
    };
    if text.trim().is_empty() {
        return "Empty".to_string();
    }
    let said = match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(fields)) if !fields.is_empty() => {
            // In name order, whichever order the JSON library keeps.
            let mut pairs: Vec<(&String, &Value)> = fields.iter().collect();
            pairs.sort_by(|a, b| a.0.cmp(b.0));
            pairs
                .iter()
                .map(|(k, v)| format!("{k}: {}", scalar(v)))
                .collect::<Vec<_>>()
                .join(", ")
        }
        Ok(Value::Array(items)) if items.is_empty() => "An empty list".to_string(),
        Ok(Value::Array(items)) => {
            if items
                .iter()
                .all(|v| !matches!(v, Value::Object(_) | Value::Array(_)))
            {
                items.iter().map(scalar).collect::<Vec<_>>().join(", ")
            } else {
                let first = items.first().map(|v| match v {
                    Value::Object(_) => preview(v.to_string().as_bytes(), max),
                    other => scalar(other),
                });
                format!(
                    "{} {}: {}",
                    items.len(),
                    if items.len() == 1 { "item" } else { "items" },
                    first.unwrap_or_default()
                )
            }
        }
        Ok(other @ (Value::String(_) | Value::Bool(_) | Value::Number(_))) => scalar(&other),
        _ => text.split_whitespace().collect::<Vec<_>>().join(" "),
    };
    cut(&said, max)
}

fn scalar(value: &Value) -> String {
    match value {
        Value::String(s) => s.split_whitespace().collect::<Vec<_>>().join(" "),
        Value::Bool(true) => "yes".to_string(),
        Value::Bool(false) => "no".to_string(),
        Value::Null => "nothing".to_string(),
        Value::Number(n) => n.to_string(),
        Value::Array(items) => format!(
            "{} {}",
            items.len(),
            if items.len() == 1 { "item" } else { "items" }
        ),
        Value::Object(fields) => format!(
            "{} {}",
            fields.len(),
            if fields.len() == 1 { "field" } else { "fields" }
        ),
    }
}

fn cut(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out = out.trim_end().to_string();
    out.push('\u{2026}');
    out
}

fn size(bytes: usize) -> String {
    if bytes >= 1024 * 1024 {
        format!("{:.1} MB", bytes as f64 / (1024.0 * 1024.0))
    } else {
        format!("{} KB", bytes.div_ceil(1024).max(1))
    }
}

/// How long ago `then` was, at `now` (both seconds since 1970): "just now",
/// "5 minutes ago", "yesterday". A clock that runs behind reads as just now.
pub fn ago(now: u64, then: u64) -> String {
    let gone = now.saturating_sub(then);
    let plural = |n: u64, one: &str| {
        if n == 1 {
            format!("1 {one} ago")
        } else {
            format!("{n} {one}s ago")
        }
    };
    match gone {
        0..=59 => "just now".to_string(),
        60..=3599 => plural(gone / 60, "minute"),
        3600..=86_399 => plural(gone / 3600, "hour"),
        86_400..=172_799 => "yesterday".to_string(),
        _ => plural(gone / 86_400, "day"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_value_reads_as_words() {
        assert_eq!(preview(b"Buy milk", 80), "Buy milk");
        assert_eq!(preview(b"  Buy\n milk  ", 80), "Buy milk");
        assert_eq!(
            preview(br#"{"title":"Milk","done":true,"due":null}"#, 80),
            "done: yes, due: nothing, title: Milk"
        );
        assert_eq!(preview(br#"["milk","eggs",2]"#, 80), "milk, eggs, 2");
        assert_eq!(
            preview(br#"[{"title":"Milk"},{"title":"Eggs"}]"#, 80),
            "2 items: title: Milk"
        );
        assert_eq!(preview(br#""quoted""#, 80), "quoted");
        assert_eq!(preview(b"42", 80), "42");
        assert_eq!(preview(b"[]", 80), "An empty list");
        assert_eq!(preview(b"", 80), "Empty");
        assert_eq!(preview(&[0xff, 0xfe, 0x00], 80), "1 KB of data");
        let long = preview("word ".repeat(60).as_bytes(), 20);
        assert!(long.chars().count() <= 20 && long.ends_with('\u{2026}'));
        let nested = preview(br#"{"tags":["a","b"],"who":{"n":1}}"#, 80);
        assert_eq!(nested, "tags: 2 items, who: 1 field");
    }

    #[test]
    fn a_time_reads_as_how_long_ago() {
        assert_eq!(ago(1000, 1000), "just now");
        assert_eq!(ago(1000, 2000), "just now", "a clock behind");
        assert_eq!(ago(1000 + 60, 1000), "1 minute ago");
        assert_eq!(ago(1000 + 300, 1000), "5 minutes ago");
        assert_eq!(ago(1000 + 7200, 1000), "2 hours ago");
        assert_eq!(ago(1000 + 90_000, 1000), "yesterday");
        assert_eq!(ago(1000 + 3 * 86_400, 1000), "3 days ago");
    }
}
