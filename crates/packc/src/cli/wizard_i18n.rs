#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use greentic_i18n_lib::normalize_tag;
use greentic_qa_lib::{I18nConfig, ResolvedI18nMap};

pub(crate) struct WizardI18n {
    locale: String,
    selected: BTreeMap<String, String>,
    fallback: BTreeMap<String, String>,
}

impl WizardI18n {
    pub(crate) fn new(requested_locale: Option<&str>) -> Self {
        let locale = select_locale(requested_locale);
        let fallback = parse_bundle(EN_GB_BUNDLE);
        let selected = bundle_for_locale(&locale)
            .map(parse_bundle)
            .unwrap_or_else(|| fallback.clone());
        Self {
            locale,
            selected,
            fallback,
        }
    }

    pub(crate) fn t(&self, key: &str) -> String {
        self.selected
            .get(key)
            .or_else(|| self.fallback.get(key))
            .cloned()
            .unwrap_or_else(|| format!("??{key}??"))
    }

    pub(crate) fn qa_i18n_config(&self) -> I18nConfig {
        I18nConfig {
            locale: Some(self.locale.clone()),
            resolved: Some(self.qa_resolved_map()),
            debug: false,
        }
    }

    fn qa_resolved_map(&self) -> ResolvedI18nMap {
        let mut resolved = BTreeMap::new();
        for (key, value) in &self.fallback {
            resolved.insert(key.clone(), value.clone());
            resolved.insert(format!("en-GB:{key}"), value.clone());
            resolved.insert(format!("en-GB/{key}"), value.clone());
        }
        for (key, value) in &self.selected {
            resolved.insert(key.clone(), value.clone());
            resolved.insert(format!("{}:{key}", self.locale), value.clone());
            resolved.insert(format!("{}/{key}", self.locale), value.clone());
        }
        resolved
    }
}

fn parse_bundle(raw: &str) -> BTreeMap<String, String> {
    serde_json::from_str(raw).unwrap_or_default()
}

pub(crate) fn detect_requested_locale() -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"]
        .iter()
        .find_map(|name| std::env::var(name).ok())
}

fn select_locale(requested_locale: Option<&str>) -> String {
    let requested = requested_locale
        .and_then(normalize_locale)
        .or_else(|| detect_requested_locale().and_then(|value| normalize_locale(&value)));

    let Some(requested) = requested else {
        return "en-GB".to_string();
    };

    if bundle_for_locale(&requested).is_some() {
        return requested;
    }

    let language = requested.split('-').next().unwrap_or_default();
    if let Some((locale, _)) = EMBEDDED_WIZARD_BUNDLES
        .iter()
        .find(|(locale, _)| locale.split('-').next() == Some(language))
    {
        return (*locale).to_string();
    }

    "en-GB".to_string()
}

fn bundle_for_locale(locale: &str) -> Option<&'static str> {
    EMBEDDED_WIZARD_BUNDLES
        .iter()
        .find(|(tag, _)| *tag == locale)
        .map(|(_, bundle)| *bundle)
}

fn normalize_locale(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }

    let without_suffix = trimmed
        .split_once('.')
        .map(|(base, _)| base)
        .unwrap_or(trimmed)
        .split_once('@')
        .map(|(base, _)| base)
        .unwrap_or(trimmed)
        .replace('_', "-");

    normalize_tag(&without_suffix)
        .ok()
        .map(|tag| tag.as_str().to_string())
}

const EN_GB_BUNDLE: &str = include_str!("../../i18n/pack_wizard/en-GB.json");
const EMBEDDED_WIZARD_BUNDLES: &[(&str, &str)] = &[
    ("ar", include_str!("../../i18n/pack_wizard/ar.json")),
    ("ar-AE", include_str!("../../i18n/pack_wizard/ar-AE.json")),
    ("ar-DZ", include_str!("../../i18n/pack_wizard/ar-DZ.json")),
    ("ar-EG", include_str!("../../i18n/pack_wizard/ar-EG.json")),
    ("ar-IQ", include_str!("../../i18n/pack_wizard/ar-IQ.json")),
    ("ar-MA", include_str!("../../i18n/pack_wizard/ar-MA.json")),
    ("ar-SA", include_str!("../../i18n/pack_wizard/ar-SA.json")),
    ("ar-SD", include_str!("../../i18n/pack_wizard/ar-SD.json")),
    ("ar-SY", include_str!("../../i18n/pack_wizard/ar-SY.json")),
    ("ar-TN", include_str!("../../i18n/pack_wizard/ar-TN.json")),
    ("ay", include_str!("../../i18n/pack_wizard/ay.json")),
    ("bg", include_str!("../../i18n/pack_wizard/bg.json")),
    ("bn", include_str!("../../i18n/pack_wizard/bn.json")),
    ("cs", include_str!("../../i18n/pack_wizard/cs.json")),
    ("da", include_str!("../../i18n/pack_wizard/da.json")),
    ("de", include_str!("../../i18n/pack_wizard/de.json")),
    ("el", include_str!("../../i18n/pack_wizard/el.json")),
    ("en-GB", include_str!("../../i18n/pack_wizard/en-GB.json")),
    ("es", include_str!("../../i18n/pack_wizard/es.json")),
    ("et", include_str!("../../i18n/pack_wizard/et.json")),
    ("fa", include_str!("../../i18n/pack_wizard/fa.json")),
    ("fi", include_str!("../../i18n/pack_wizard/fi.json")),
    ("fr", include_str!("../../i18n/pack_wizard/fr.json")),
    ("fr-FR", include_str!("../../i18n/pack_wizard/fr-FR.json")),
    ("gn", include_str!("../../i18n/pack_wizard/gn.json")),
    ("gu", include_str!("../../i18n/pack_wizard/gu.json")),
    ("hi", include_str!("../../i18n/pack_wizard/hi.json")),
    ("hr", include_str!("../../i18n/pack_wizard/hr.json")),
    ("ht", include_str!("../../i18n/pack_wizard/ht.json")),
    ("hu", include_str!("../../i18n/pack_wizard/hu.json")),
    ("id", include_str!("../../i18n/pack_wizard/id.json")),
    ("it", include_str!("../../i18n/pack_wizard/it.json")),
    ("ja", include_str!("../../i18n/pack_wizard/ja.json")),
    ("km", include_str!("../../i18n/pack_wizard/km.json")),
    ("kn", include_str!("../../i18n/pack_wizard/kn.json")),
    ("ko", include_str!("../../i18n/pack_wizard/ko.json")),
    ("lo", include_str!("../../i18n/pack_wizard/lo.json")),
    ("lt", include_str!("../../i18n/pack_wizard/lt.json")),
    ("lv", include_str!("../../i18n/pack_wizard/lv.json")),
    ("ml", include_str!("../../i18n/pack_wizard/ml.json")),
    ("mr", include_str!("../../i18n/pack_wizard/mr.json")),
    ("ms", include_str!("../../i18n/pack_wizard/ms.json")),
    ("my", include_str!("../../i18n/pack_wizard/my.json")),
    ("nah", include_str!("../../i18n/pack_wizard/nah.json")),
    ("ne", include_str!("../../i18n/pack_wizard/ne.json")),
    ("nl", include_str!("../../i18n/pack_wizard/nl.json")),
    ("nl-NL", include_str!("../../i18n/pack_wizard/nl-NL.json")),
    ("no", include_str!("../../i18n/pack_wizard/no.json")),
    ("pa", include_str!("../../i18n/pack_wizard/pa.json")),
    ("pl", include_str!("../../i18n/pack_wizard/pl.json")),
    ("pt", include_str!("../../i18n/pack_wizard/pt.json")),
    ("qu", include_str!("../../i18n/pack_wizard/qu.json")),
    ("ro", include_str!("../../i18n/pack_wizard/ro.json")),
    ("ru", include_str!("../../i18n/pack_wizard/ru.json")),
    ("si", include_str!("../../i18n/pack_wizard/si.json")),
    ("sk", include_str!("../../i18n/pack_wizard/sk.json")),
    ("sr", include_str!("../../i18n/pack_wizard/sr.json")),
    ("sv", include_str!("../../i18n/pack_wizard/sv.json")),
    ("ta", include_str!("../../i18n/pack_wizard/ta.json")),
    ("te", include_str!("../../i18n/pack_wizard/te.json")),
    ("th", include_str!("../../i18n/pack_wizard/th.json")),
    ("tl", include_str!("../../i18n/pack_wizard/tl.json")),
    ("tr", include_str!("../../i18n/pack_wizard/tr.json")),
    ("uk", include_str!("../../i18n/pack_wizard/uk.json")),
    ("ur", include_str!("../../i18n/pack_wizard/ur.json")),
    ("vi", include_str!("../../i18n/pack_wizard/vi.json")),
    ("zh", include_str!("../../i18n/pack_wizard/zh.json")),
];
