//! Shared locale helpers for host adapters.

use std::path::Path;

/// BCP 47 locale identifier used by the Phase 2 host adapter layer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocaleId {
    pub bcp47: String,
}

/// Date formatting width requested by the guest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DateStyle {
    Short,
    Medium,
    Long,
    Full,
}

/// Number formatting style requested by the guest.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberStyle {
    Decimal,
    Percent,
    Currency,
}

/// Locale behavior shared by early local Phase 2 host adapters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostLocale {
    locale: LocaleId,
    timezone: String,
}

impl HostLocale {
    pub fn from_env() -> Self {
        Self::from_env_with_overrides(None, None)
    }

    pub fn from_env_with_overrides(
        locale_override: Option<&str>,
        timezone_override: Option<&str>,
    ) -> Self {
        Self::from_env_pairs_with_overrides(std::env::vars(), locale_override, timezone_override)
    }

    pub fn from_env_pairs(
        pairs: impl IntoIterator<Item = (impl AsRef<str>, impl AsRef<str>)>,
    ) -> Self {
        Self::from_env_pairs_with_overrides(pairs, None, None)
    }

    pub fn from_env_pairs_with_overrides(
        pairs: impl IntoIterator<Item = (impl AsRef<str>, impl AsRef<str>)>,
        locale_override: Option<&str>,
        timezone_override: Option<&str>,
    ) -> Self {
        let mut lc_all = None;
        let mut lang = None;
        let mut lc_messages = None;
        let mut language = None;
        let mut apple_locale = None;
        let mut timezone = None;

        for (key, value) in pairs {
            match key.as_ref() {
                "LC_ALL" => lc_all = Some(value.as_ref().to_string()),
                "LANG" => lang = Some(value.as_ref().to_string()),
                "LC_MESSAGES" => lc_messages = Some(value.as_ref().to_string()),
                "LANGUAGE" => language = Some(value.as_ref().to_string()),
                "AppleLocale" => apple_locale = Some(value.as_ref().to_string()),
                "TZ" => timezone = Some(value.as_ref().to_string()),
                _ => {}
            }
        }

        let language_first = language
            .as_deref()
            .and_then(|value| value.split(':').find(|part| !part.trim().is_empty()));
        let locale_raw = lc_all
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .or(lang.as_deref().filter(|value| !value.trim().is_empty()))
            .or(lc_messages
                .as_deref()
                .filter(|value| !value.trim().is_empty()))
            .or(language_first)
            .or(apple_locale
                .as_deref()
                .filter(|value| !value.trim().is_empty()));
        let timezone_raw = timezone_override
            .map(str::to_string)
            .or(timezone)
            .or_else(system_timezone_fallback);
        let locale_raw = locale_override.or(locale_raw);

        Self {
            locale: LocaleId {
                bcp47: normalize_locale_tag(locale_raw),
            },
            timezone: normalize_timezone(timezone_raw.as_deref()),
        }
    }

    pub fn current(&self) -> LocaleId {
        self.locale.clone()
    }

    pub fn timezone(&self) -> String {
        self.timezone.clone()
    }

    pub fn format_date(millis: u64, timezone: &str, style: DateStyle, locale: &LocaleId) -> String {
        let parts = utc_parts_from_unix_millis(millis);
        let timezone = normalize_timezone(Some(timezone));
        let locale = normalize_locale_tag(Some(&locale.bcp47));

        match style {
            DateStyle::Short => format!("{:04}-{:02}-{:02}", parts.year, parts.month, parts.day),
            DateStyle::Medium => format!(
                "{:04}-{:02}-{:02} {:02}:{:02}",
                parts.year, parts.month, parts.day, parts.hour, parts.minute
            ),
            DateStyle::Long => format!(
                "{:04}-{:02}-{:02} {:02}:{:02}:{:02} {}",
                parts.year,
                parts.month,
                parts.day,
                parts.hour,
                parts.minute,
                parts.second,
                timezone
            ),
            DateStyle::Full => format!(
                "{}, {:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:03} {} ({})",
                parts.weekday_name(),
                parts.year,
                parts.month,
                parts.day,
                parts.hour,
                parts.minute,
                parts.second,
                parts.millis,
                timezone,
                locale
            ),
        }
    }

    pub fn format_number(value: f64, style: NumberStyle, locale: &LocaleId) -> String {
        match style {
            NumberStyle::Decimal => decimal_text(value),
            NumberStyle::Percent => format!("{}%", decimal_text(value * 100.0)),
            NumberStyle::Currency => {
                format!("{}{}", currency_symbol(&locale.bcp47), decimal_text(value))
            }
        }
    }
}

fn system_timezone_fallback() -> Option<String> {
    #[cfg(unix)]
    {
        infer_unix_timezone_from_localtime()
    }

    #[cfg(not(unix))]
    {
        None
    }
}

#[cfg(unix)]
fn infer_unix_timezone_from_localtime() -> Option<String> {
    if let Ok(target) = std::fs::read_link("/etc/localtime") {
        if let Some(timezone) = timezone_from_localtime_link_target(&target) {
            return Some(timezone);
        }
    }

    let contents = std::fs::read_to_string("/etc/timezone").ok()?;
    timezone_from_etc_timezone_contents(&contents)
}

fn timezone_from_localtime_link_target(target: &Path) -> Option<String> {
    let portable = target.to_string_lossy().replace('\\', "/");
    let marker = "/zoneinfo/";
    let (_, suffix) = portable.split_once(marker)?;
    normalize_timezone_candidate(suffix.trim_matches('/'))
}

fn timezone_from_etc_timezone_contents(contents: &str) -> Option<String> {
    for raw_line in contents.lines() {
        let line = raw_line.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if let Some(timezone) = normalize_timezone_candidate(line) {
            return Some(timezone);
        }
    }

    None
}

fn normalize_timezone_candidate(candidate: &str) -> Option<String> {
    let candidate = candidate.trim_matches('/').trim();
    if candidate.is_empty() {
        return None;
    }

    let normalized = normalize_timezone(Some(candidate));
    if normalized == "UTC" && !candidate.eq_ignore_ascii_case("UTC") {
        return None;
    }

    Some(normalized)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct UtcParts {
    year: i32,
    month: u8,
    day: u8,
    hour: u8,
    minute: u8,
    second: u8,
    millis: u16,
    weekday_index: u8,
}

impl UtcParts {
    fn weekday_name(self) -> &'static str {
        const WEEKDAYS: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
        WEEKDAYS[self.weekday_index as usize]
    }
}

fn utc_parts_from_unix_millis(unix_millis: u64) -> UtcParts {
    const SECS_PER_DAY: u64 = 86_400;
    const SECS_PER_HOUR: u64 = 3_600;
    const SECS_PER_MINUTE: u64 = 60;

    let total_seconds = unix_millis / 1_000;
    let millis = (unix_millis % 1_000) as u16;
    let days = (total_seconds / SECS_PER_DAY) as i64;
    let day_seconds = total_seconds % SECS_PER_DAY;

    let hour = (day_seconds / SECS_PER_HOUR) as u8;
    let minute = ((day_seconds % SECS_PER_HOUR) / SECS_PER_MINUTE) as u8;
    let second = (day_seconds % SECS_PER_MINUTE) as u8;

    let (year, month, day) = civil_from_days(days);
    let weekday_index = (days + 4).rem_euclid(7) as u8;

    UtcParts {
        year,
        month,
        day,
        hour,
        minute,
        second,
        millis,
        weekday_index,
    }
}

fn civil_from_days(days_since_unix_epoch: i64) -> (i32, u8, u8) {
    let z = days_since_unix_epoch + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = mp + if mp < 10 { 3 } else { -9 };
    let year = y + if m <= 2 { 1 } else { 0 };

    (year as i32, m as u8, d as u8)
}

fn decimal_text(value: f64) -> String {
    if !value.is_finite() {
        return value.to_string();
    }

    let mut text = value.to_string();
    if text.contains('.') && !text.contains('e') && !text.contains('E') {
        while text.ends_with('0') {
            text.pop();
        }
        if text.ends_with('.') {
            text.pop();
        }
    }
    text
}

fn currency_symbol(locale: &str) -> &'static str {
    let locale = normalize_locale_tag(Some(locale));
    let lower = locale.to_ascii_lowercase();

    if lower == "en-gb" {
        "£"
    } else if lower.starts_with("en-") {
        "$"
    } else if lower.starts_with("ja-") {
        "¥"
    } else if lower.starts_with("de-")
        || lower.starts_with("fr-")
        || lower.starts_with("es-")
        || lower.starts_with("it-")
        || lower.starts_with("pt-")
        || lower.starts_with("nl-")
    {
        "€"
    } else {
        "¤"
    }
}

pub fn normalize_locale_tag(raw: Option<&str>) -> String {
    let Some(raw) = raw else {
        return "en-US".to_string();
    };

    let tag = raw
        .split('.')
        .next()
        .unwrap_or(raw)
        .split('@')
        .next()
        .unwrap_or(raw)
        .trim();

    if tag.is_empty() || tag == "C" || tag == "POSIX" {
        return "en-US".to_string();
    }

    canonicalize_locale_tag(tag).unwrap_or_else(|| "en-US".to_string())
}

pub fn normalize_timezone(raw: Option<&str>) -> String {
    const MAX_TIMEZONE_BYTES: usize = 128;
    raw.map(str::trim)
        .filter(|value| {
            !value.is_empty()
                && value.len() <= MAX_TIMEZONE_BYTES
                && value
                    .chars()
                    .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '/' | '_' | '+' | '-'))
        })
        .unwrap_or("UTC")
        .to_string()
}

fn canonicalize_locale_tag(tag: &str) -> Option<String> {
    let portable = tag.replace('_', "-");
    let mut pieces = portable.split('-');
    let language = pieces.next()?;

    if !is_valid_language_subtag(language) {
        return None;
    }

    let mut normalized = Vec::new();
    normalized.push(language.to_ascii_lowercase());

    for piece in pieces {
        if !is_valid_locale_subtag(piece) {
            return None;
        }

        let value = if piece.len() == 4 && piece.chars().all(|ch| ch.is_ascii_alphabetic()) {
            title_case_ascii(piece)
        } else if piece.len() == 2 && piece.chars().all(|ch| ch.is_ascii_alphabetic()) {
            piece.to_ascii_uppercase()
        } else if piece.len() == 3 && piece.chars().all(|ch| ch.is_ascii_digit()) {
            piece.to_string()
        } else {
            piece.to_ascii_lowercase()
        };

        normalized.push(value);
    }

    Some(normalized.join("-"))
}

fn is_ascii_alnum(value: &str) -> bool {
    value.chars().all(|ch| ch.is_ascii_alphanumeric())
}

fn is_valid_language_subtag(value: &str) -> bool {
    (2..=8).contains(&value.len()) && value.chars().all(|ch| ch.is_ascii_alphabetic())
}

fn is_valid_locale_subtag(value: &str) -> bool {
    (1..=8).contains(&value.len()) && is_ascii_alnum(value)
}

fn title_case_ascii(value: &str) -> String {
    let mut chars = value.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };

    let mut out = String::new();
    out.push(first.to_ascii_uppercase());
    for ch in chars {
        out.push(ch.to_ascii_lowercase());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locale_prefers_lc_all_over_lang() {
        let locale = HostLocale::from_env_pairs([
            ("LANG", "en_GB.UTF-8"),
            ("LC_ALL", "fr_CA.UTF-8"),
            ("TZ", "America/Toronto"),
        ]);

        assert_eq!(locale.current().bcp47, "fr-CA");
        assert_eq!(locale.timezone(), "America/Toronto");
    }

    #[test]
    fn locale_normalizes_lang_and_strips_modifier() {
        let locale = HostLocale::from_env_pairs([("LANG", "de_DE.UTF-8@euro"), ("TZ", "")]);

        assert_eq!(locale.current().bcp47, "de-DE");
        assert_eq!(locale.timezone(), "UTC");
    }

    #[test]
    fn empty_lc_all_falls_back_to_lang() {
        let locale = HostLocale::from_env_pairs([("LC_ALL", ""), ("LANG", "en_GB.UTF-8")]);

        assert_eq!(locale.current().bcp47, "en-GB");
    }

    #[test]
    fn locale_uses_default_for_posix_locale_and_empty_timezone() {
        let locale = HostLocale::from_env_pairs([("LC_ALL", "POSIX"), ("TZ", "")]);

        assert_eq!(locale.current().bcp47, "en-US");
        assert_eq!(locale.timezone(), "UTC");
    }

    #[test]
    fn deterministic_formatter_is_stable_without_host_bindings() {
        let locale = LocaleId {
            bcp47: "en-US".to_string(),
        };

        assert_eq!(
            HostLocale::format_date(1_777, "UTC", DateStyle::Medium, &locale),
            "1970-01-01 00:00"
        );
        assert_eq!(
            HostLocale::format_number(42.5, NumberStyle::Decimal, &locale),
            "42.5"
        );
        assert_eq!(
            HostLocale::format_number(0.125, NumberStyle::Percent, &locale),
            "12.5%"
        );
        assert_eq!(
            HostLocale::format_number(42.5, NumberStyle::Currency, &locale),
            "$42.5"
        );
    }

    #[test]
    fn locale_and_timezone_overrides_win_over_environment() {
        let locale = HostLocale::from_env_pairs_with_overrides(
            [("LC_ALL", "fr_CA.UTF-8"), ("TZ", "America/Toronto")],
            Some("en_GB.UTF-8"),
            Some("UTC"),
        );

        assert_eq!(locale.current().bcp47, "en-GB");
        assert_eq!(locale.timezone(), "UTC");
    }

    #[test]
    fn full_date_style_includes_weekday_timezone_and_locale() {
        let locale = LocaleId {
            bcp47: "de-DE".to_string(),
        };

        assert_eq!(
            HostLocale::format_date(1_234_567_890, "UTC", DateStyle::Full, &locale),
            "Thu, 1970-01-15 06:56:07.890 UTC (de-DE)"
        );
    }

    #[test]
    fn locale_canonicalization_normalizes_case_and_structure() {
        assert_eq!(normalize_locale_tag(Some("EN_us")), "en-US");
        assert_eq!(normalize_locale_tag(Some("zh_hant_tw")), "zh-Hant-TW");
        assert_eq!(
            normalize_locale_tag(Some("sr_latn_rs_revised")),
            "sr-Latn-RS-revised"
        );
    }

    #[test]
    fn malformed_locale_falls_back_to_default() {
        assert_eq!(normalize_locale_tag(Some("en--US")), "en-US");
        assert_eq!(normalize_locale_tag(Some("_")), "en-US");
        assert_eq!(normalize_locale_tag(Some("en_US@bad*mod")), "en-US");
        assert_eq!(normalize_locale_tag(Some("123")), "en-US");
        assert_eq!(normalize_locale_tag(Some("englishlong")), "en-US");
        assert_eq!(normalize_locale_tag(Some("en-superlongsubtag")), "en-US");
    }

    #[test]
    fn timezone_rejects_control_characters() {
        assert_eq!(normalize_timezone(Some("UTC\nInjected")), "UTC");
        assert_eq!(normalize_timezone(Some("Asia/Singapore")), "Asia/Singapore");
    }

    #[test]
    fn timezone_rejects_unsafe_characters_and_long_values() {
        assert_eq!(normalize_timezone(Some("America/New York")), "UTC");
        assert_eq!(normalize_timezone(Some("../UTC")), "UTC");
        assert_eq!(normalize_timezone(Some("UTC*")), "UTC");
        assert_eq!(normalize_timezone(Some(&"A".repeat(129))), "UTC");
        assert_eq!(normalize_timezone(Some("Etc/GMT+1")), "Etc/GMT+1");
    }

    #[test]
    fn locale_falls_back_to_lc_messages_and_language() {
        let locale = HostLocale::from_env_pairs([
            ("LC_ALL", ""),
            ("LANG", ""),
            ("LC_MESSAGES", "es_ES.UTF-8"),
            ("LANGUAGE", "fr_FR:de_DE"),
        ]);
        assert_eq!(locale.current().bcp47, "es-ES");

        let locale = HostLocale::from_env_pairs([
            ("LC_ALL", ""),
            ("LANG", ""),
            ("LC_MESSAGES", ""),
            ("LANGUAGE", "fr_FR:de_DE"),
        ]);
        assert_eq!(locale.current().bcp47, "fr-FR");
    }

    #[test]
    fn timezone_can_be_inferred_from_zoneinfo_symlink_target() {
        let tz =
            timezone_from_localtime_link_target(Path::new("/usr/share/zoneinfo/Asia/Singapore"))
                .expect("timezone should be inferred");
        assert_eq!(tz, "Asia/Singapore");

        let tz = timezone_from_localtime_link_target(Path::new(
            "/var/db/timezone/zoneinfo/America/Toronto",
        ))
        .expect("timezone should be inferred");
        assert_eq!(tz, "America/Toronto");
    }

    #[test]
    fn timezone_inference_rejects_non_zoneinfo_targets() {
        assert!(timezone_from_localtime_link_target(Path::new("/etc/localtime")).is_none());
        assert!(timezone_from_localtime_link_target(Path::new("/zoneinfo/")).is_none());
        assert!(timezone_from_localtime_link_target(Path::new(
            "/usr/share/zoneinfo/America/New York",
        ))
        .is_none());
    }

    #[test]
    fn timezone_can_be_inferred_from_etc_timezone_contents() {
        let tz = timezone_from_etc_timezone_contents("Asia/Singapore\n")
            .expect("timezone should be inferred");
        assert_eq!(tz, "Asia/Singapore");

        let tz = timezone_from_etc_timezone_contents("  # comment\nAmerica/Toronto # tz name\n")
            .expect("timezone should be inferred");
        assert_eq!(tz, "America/Toronto");
    }

    #[test]
    fn timezone_etc_timezone_parser_rejects_invalid_shapes() {
        assert!(timezone_from_etc_timezone_contents("America/New York\n").is_none());
        assert!(timezone_from_etc_timezone_contents("../UTC\n").is_none());
        assert!(timezone_from_etc_timezone_contents("\n# only comment\n").is_none());
    }
}
