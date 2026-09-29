use foundation::serialization::JSONSerialization;
use std::collections::HashMap;
use std::sync::OnceLock;

const EN_US: &str = include_str!("../lang/en_us.json");
const DE_DE: &str = include_str!("../lang/de_de.json");

static MESSAGES: OnceLock<HashMap<String, String>> = OnceLock::new();

pub fn current_locale() -> &'static str {
    let lang = std::env::var("LC_ALL")
        .or_else(|_| std::env::var("LC_MESSAGES"))
        .or_else(|_| std::env::var("LANG"))
        .unwrap_or_default();
    if lang.to_lowercase().starts_with("de") {
        "de_de"
    } else {
        "en_us"
    }
}

fn messages() -> &'static HashMap<String, String> {
    MESSAGES.get_or_init(|| {
        let raw = match current_locale() {
            "de_de" => DE_DE,
            _ => EN_US,
        };
        JSONSerialization::parse_flat_string_map(raw)
            .expect("built-in language file is invalid")
    })
}

pub fn t(key: &str) -> String {
    messages()
        .get(key)
        .cloned()
        .unwrap_or_else(|| key.to_string())
}

pub fn t_fmt(key: &str, arg: &str) -> String {
    t(key).replacen("{}", arg, 1)
}
