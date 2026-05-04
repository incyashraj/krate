//! Shared locale helpers for host adapters.

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
        let mut timezone = None;

        for (key, value) in pairs {
            match key.as_ref() {
                "LC_ALL" => lc_all = Some(value.as_ref().to_string()),
                "LANG" => lang = Some(value.as_ref().to_string()),
                "TZ" => timezone = Some(value.as_ref().to_string()),
                _ => {}
            }
        }

        let locale_raw = lc_all
            .as_deref()
            .filter(|value| !value.trim().is_empty())
            .or(lang.as_deref());
        let timezone_raw = timezone_override.or(timezone.as_deref());
        let locale_raw = locale_override.or(locale_raw);

        Self {
            locale: LocaleId {
                bcp47: normalize_locale_tag(locale_raw),
            },
            timezone: normalize_timezone(timezone_raw),
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

    tag.replace('_', "-")
}

pub fn normalize_timezone(raw: Option<&str>) -> String {
    raw.map(str::trim)
        .filter(|value| !value.is_empty())
        .unwrap_or("UTC")
        .to_string()
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
        let locale = HostLocale::from_env_pairs([("LANG", "de_DE.UTF-8@euro")]);

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
}
