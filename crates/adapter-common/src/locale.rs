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
        Self::from_env_pairs(std::env::vars())
    }

    pub fn from_env_pairs(
        pairs: impl IntoIterator<Item = (impl AsRef<str>, impl AsRef<str>)>,
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

        Self {
            locale: LocaleId {
                bcp47: normalize_locale_tag(locale_raw),
            },
            timezone: normalize_timezone(timezone.as_deref()),
        }
    }

    pub fn current(&self) -> LocaleId {
        self.locale.clone()
    }

    pub fn timezone(&self) -> String {
        self.timezone.clone()
    }

    pub fn format_date(millis: u64, timezone: &str, style: DateStyle, locale: &LocaleId) -> String {
        format!("{millis}:{timezone}:{style:?}:{}", locale.bcp47)
    }

    pub fn format_number(value: f64, style: NumberStyle, locale: &LocaleId) -> String {
        format!("{value}:{style:?}:{}", locale.bcp47)
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
    fn formatting_placeholder_is_stable_until_icu_slice() {
        let locale = LocaleId {
            bcp47: "en-US".to_string(),
        };

        assert_eq!(
            HostLocale::format_date(1_777, "UTC", DateStyle::Medium, &locale),
            "1777:UTC:Medium:en-US"
        );
        assert_eq!(
            HostLocale::format_number(42.5, NumberStyle::Decimal, &locale),
            "42.5:Decimal:en-US"
        );
    }
}
