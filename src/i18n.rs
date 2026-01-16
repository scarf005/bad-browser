use std::env;

pub use rust_i18n::t;

pub fn init_locale(preferred: Option<&str>) {
    let candidate = preferred
        .map(|s| s.to_string())
        .or_else(|| env::var("LANG").ok());

    if let Some(locale) = candidate.and_then(normalize_locale) {
        rust_i18n::set_locale(&locale);
    } else {
        rust_i18n::set_locale("en");
    }
}

fn normalize_locale(locale: String) -> Option<String> {
    let lower = locale.to_lowercase();

    if lower.starts_with("ko") {
        Some("ko".to_string())
    } else if lower.starts_with("ja") || lower.starts_with("jp") {
        Some("ja".to_string())
    } else if lower.starts_with("en") {
        Some("en".to_string())
    } else {
        None
    }
}
