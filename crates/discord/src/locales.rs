//! The languages the official client lets an account pick (`getLanguages`,
//! the table in 296437, those it enables), in the order its picker lists
//! them: by name, case folded. The picked code is the account's
//! `localization.locale`.
//!
//! Neucord only has English text, so the setting is for the account's other
//! clients and for what Discord sends by mail and push.

/// (code, the language's name in itself).
pub const LOCALES: [(&str, &str); 31] = [
    ("da", "Dansk"),
    ("de", "Deutsch"),
    ("en-GB", "English, GB"),
    ("en-US", "English, USA"),
    ("es-ES", "Español"),
    ("es-419", "Español, LATAM"),
    ("fr", "Français"),
    ("hr", "Hrvatski"),
    ("it", "Italiano"),
    ("lt", "Lietuviškai"),
    ("hu", "Magyar"),
    ("nl", "Nederlands"),
    ("no", "Norwegian"),
    ("pl", "Polski"),
    ("pt-BR", "Português do Brasil"),
    ("ro", "Română"),
    ("fi", "Suomi"),
    ("sv-SE", "Svenska"),
    ("vi", "Tiếng Việt"),
    ("tr", "Türkçe"),
    ("cs", "Čeština"),
    ("el", "Ελληνικά"),
    ("bg", "български"),
    ("ru", "Русский"),
    ("uk", "Українська"),
    ("hi", "हिंदी"),
    ("th", "ไทย"),
    ("zh-CN", "中文"),
    ("ja", "日本語"),
    ("zh-TW", "繁體中文"),
    ("ko", "한국어"),
];

/// Where `code` is in [`LOCALES`].
pub fn position(code: &str) -> Option<usize> {
    LOCALES.iter().position(|(c, _)| *c == code)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listed_by_name_without_repeats() {
        let names: Vec<String> = LOCALES.iter().map(|(_, name)| name.to_lowercase()).collect();
        assert!(names.is_sorted(), "{names:?}");
        let mut codes: Vec<_> = LOCALES.iter().map(|(code, _)| *code).collect();
        codes.sort();
        codes.dedup();
        assert_eq!(codes.len(), LOCALES.len());
        assert_eq!(position("pt-BR"), Some(14));
        assert_eq!(position("xx"), None);
    }
}
