#![forbid(unsafe_code)]

use std::collections::HashMap;
use std::env;
use std::path::PathBuf;
use std::sync::OnceLock;
use unic_langid::LanguageIdentifier;

static REQUESTED_LOCALE: OnceLock<String> = OnceLock::new();
static I18N: OnceLock<CliI18n> = OnceLock::new();

pub fn init_locale(cli_locale: Option<&str>) {
    let supported = supported_locales();
    let selected = select_locale(cli_locale, &supported);
    let _ = REQUESTED_LOCALE.set(selected);
}

pub fn t(key: &str) -> String {
    i18n().t(key)
}

pub fn tf(key: &str, args: &[&str]) -> String {
    i18n().tf(key, args)
}

pub fn has(key: &str) -> bool {
    i18n().has(key)
}

fn i18n() -> &'static CliI18n {
    I18N.get_or_init(|| {
        let locale = REQUESTED_LOCALE.get().map_or("en", String::as_str);
        CliI18n::from_request(locale)
    })
}

struct CliI18n {
    catalog: HashMap<String, String>,
    fallback: HashMap<String, String>,
}

impl CliI18n {
    fn from_request(requested: &str) -> Self {
        let fallback = parse_map(include_str!("../i18n/en.json"));
        if requested == "en" {
            return Self {
                catalog: fallback.clone(),
                fallback,
            };
        }
        let catalog = load_locale_file(requested).unwrap_or_else(|| fallback.clone());
        Self { catalog, fallback }
    }

    fn t(&self, key: &str) -> String {
        if let Some(v) = self.catalog.get(key) {
            return v.clone();
        }
        if let Some(v) = self.fallback.get(key) {
            return v.clone();
        }
        key.to_string()
    }

    fn tf(&self, key: &str, args: &[&str]) -> String {
        format_template(&self.t(key), args)
    }

    fn has(&self, key: &str) -> bool {
        self.catalog.contains_key(key) || self.fallback.contains_key(key)
    }
}

fn load_locale_file(locale: &str) -> Option<HashMap<String, String>> {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("i18n");
    path.push(format!("{locale}.json"));
    let raw = std::fs::read_to_string(path).ok()?;
    Some(parse_map(&raw))
}

fn supported_locales() -> Vec<String> {
    let mut out = vec!["en".to_string()];
    let mut dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    dir.push("i18n");
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|x| x.to_str()) != Some("json") {
            continue;
        }
        let Some(stem) = path.file_stem().and_then(|x| x.to_str()) else {
            continue;
        };
        if stem != "en" {
            out.push(stem.to_string());
        }
    }
    out.sort();
    out.dedup();
    out
}

fn detect_env_locale() -> Option<String> {
    for key in ["LC_ALL", "LC_MESSAGES", "LANG"] {
        if let Ok(val) = env::var(key) {
            let trimmed = val.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }
    }
    None
}

fn detect_system_locale() -> Option<String> {
    sys_locale::get_locale()
}

fn normalize_locale(raw: &str) -> Option<String> {
    let mut cleaned = raw.trim();
    if cleaned.is_empty() {
        return None;
    }
    if let Some((head, _)) = cleaned.split_once('.') {
        cleaned = head;
    }
    if let Some((head, _)) = cleaned.split_once('@') {
        cleaned = head;
    }
    let cleaned = cleaned.replace('_', "-");
    cleaned
        .parse::<LanguageIdentifier>()
        .ok()
        .map(|lid| lid.to_string())
}

fn base_language(tag: &str) -> Option<String> {
    tag.split('-').next().map(|s| s.to_ascii_lowercase())
}

fn resolve_supported(candidate: &str, supported: &[String]) -> Option<String> {
    let norm = normalize_locale(candidate)?;
    if supported.iter().any(|s| s == &norm) {
        return Some(norm);
    }
    let base = base_language(&norm)?;
    if supported.iter().any(|s| s == &base) {
        return Some(base);
    }
    None
}

fn select_locale(cli_locale: Option<&str>, supported: &[String]) -> String {
    if let Some(cli) = cli_locale
        && let Some(found) = resolve_supported(cli, supported)
    {
        return found;
    }

    if let Some(env_loc) = detect_env_locale()
        && let Some(found) = resolve_supported(&env_loc, supported)
    {
        return found;
    }

    if let Some(sys_loc) = detect_system_locale()
        && let Some(found) = resolve_supported(&sys_loc, supported)
    {
        return found;
    }

    "en".to_string()
}

fn parse_map(raw: &str) -> HashMap<String, String> {
    let mut map = HashMap::new();
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return map;
    };
    let Some(obj) = value.as_object() else {
        return map;
    };
    for (key, value) in obj {
        if let Some(text) = value.as_str() {
            map.insert(key.to_string(), text.to_string());
        }
    }
    map
}

fn format_template(template: &str, args: &[&str]) -> String {
    let mut out = String::with_capacity(template.len());
    let mut chars = template.chars().peekable();
    let mut idx = 0usize;

    while let Some(ch) = chars.next() {
        if ch == '{' && chars.peek() == Some(&'}') {
            let _ = chars.next();
            if let Some(val) = args.get(idx) {
                out.push_str(val);
            } else {
                out.push_str("{}");
            }
            idx += 1;
            continue;
        }
        out.push(ch);
    }
    out
}
