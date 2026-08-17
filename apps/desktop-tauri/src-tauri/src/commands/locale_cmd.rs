use super::*;

// ── Locale / i18n commands ───────────────────────────────────────────

/// Language catalog entry exposed to the frontend.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LanguageOption {
    /// Stable bridge/settings value (e.g. "english")
    pub value: &'static str,
    /// Native display name (e.g. "English", "中文", "Español")
    pub display: &'static str,
}

/// Return the canonical language catalog.
/// The frontend uses this to build a language picker without
/// hardcoding language lists or i18n keys.
#[tauri::command]
pub fn get_available_languages() -> Vec<LanguageOption> {
    Language::all()
        .iter()
        .map(|l| LanguageOption {
            value: l.label(),
            display: l.display_name(),
        })
        .collect()
}

/// Snapshot of every localized UI string in a given language.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LocaleStrings {
    /// Serialized language code (`"english"` or `"chinese"`).
    pub language: &'static str,
    /// Map of serialized `LocaleKey` variant name → localized text.
    pub entries: HashMap<&'static str, String>,
}

fn locale_strings_for(lang: Language) -> LocaleStrings {
    let mut entries = HashMap::with_capacity(locale::LocaleKey::ALL.len());
    for (key, name) in locale::LocaleKey::ALL {
        entries.insert(*name, locale::get_text(lang, *key));
    }
    LocaleStrings {
        language: language_label(lang),
        entries,
    }
}

/// Return every UI string for the requested language.
///
/// When `language` is `None`, the user's current persisted language is used.
/// The `language` argument accepts either the short code (`"en"`, `"zh"`),
/// the persisted label (`"english"`, `"chinese"`), or the full name
/// (`"English"`, `"Chinese"`, `"中文"`).
#[tauri::command]
pub fn get_locale_strings(language: Option<String>) -> Result<LocaleStrings, String> {
    let lang = match language.as_deref() {
        None => locale::current_language(),
        Some(raw) => {
            parse_locale_language(raw).ok_or_else(|| format!("unknown language code: {raw}"))?
        }
    };
    Ok(locale_strings_for(lang))
}

fn parse_locale_language(raw: &str) -> Option<Language> {
    Language::resolve(raw)
}

/// Persist the UI language and emit a `locale-changed` event so the
/// frontend can refetch its locale table without a restart.
#[tauri::command]
pub fn set_ui_language(app: tauri::AppHandle, language: String) -> Result<(), String> {
    let lang =
        parse_locale_language(&language).ok_or_else(|| format!("unknown language: {language}"))?;
    let mut settings = Settings::load();
    if settings.ui_language == lang {
        return Ok(());
    }
    settings.ui_language = lang;
    settings.save().map_err(|e| e.to_string())?;
    let _ = app.emit(events::LOCALE_CHANGED, language_label(lang));
    crate::tray_bridge::refresh_tray_presentation(&app);
    Ok(())
}

#[cfg(test)]
mod locale_tests {
    use super::*;

    #[test]
    fn locale_strings_roundtrip_english() {
        let bundle = locale_strings_for(Language::English);
        assert_eq!(bundle.language, "english");
        assert_eq!(
            bundle.entries.get("TabGeneral").map(String::as_str),
            Some("General"),
            "TabGeneral should resolve to English"
        );
        assert_eq!(
            bundle
                .entries
                .get("ProviderSidebarSearch")
                .map(String::as_str),
            Some("Search"),
            "ProviderSidebarSearch should resolve instead of leaking the key"
        );
        assert_eq!(bundle.entries.len(), locale::LocaleKey::ALL.len());
    }

    #[test]
    fn locale_strings_roundtrip_chinese() {
        let bundle = locale_strings_for(Language::Chinese);
        assert_eq!(bundle.language, "chinese");
        assert_eq!(
            bundle.entries.get("TabGeneral").map(String::as_str),
            Some("通用")
        );
        assert_eq!(bundle.entries.len(), locale::LocaleKey::ALL.len());
    }

    #[test]
    fn locale_strings_roundtrip_traditional_chinese() {
        let bundle = locale_strings_for(Language::ChineseTraditional);
        assert_eq!(bundle.language, "chinesetraditional");
        assert_eq!(
            bundle.entries.get("TabGeneral").map(String::as_str),
            Some("一般")
        );
        assert_eq!(bundle.entries.len(), locale::LocaleKey::ALL.len());
    }

    #[test]
    fn locale_strings_roundtrip_japanese() {
        let bundle = locale_strings_for(Language::Japanese);
        assert_eq!(bundle.language, "japanese");
        assert_eq!(
            bundle.entries.get("TabGeneral").map(String::as_str),
            Some("一般")
        );
        assert_eq!(bundle.entries.len(), locale::LocaleKey::ALL.len());
    }

    #[test]
    fn locale_strings_roundtrip_korean() {
        let bundle = locale_strings_for(Language::Korean);
        assert_eq!(bundle.language, "korean");
        assert_eq!(
            bundle.entries.get("TabGeneral").map(String::as_str),
            Some("일반")
        );
        assert_eq!(bundle.entries.len(), locale::LocaleKey::ALL.len());
    }

    #[test]
    fn locale_strings_roundtrip_turkish() {
        let bundle = locale_strings_for(Language::Turkish);
        assert_eq!(bundle.language, "turkish");
        assert_eq!(
            bundle.entries.get("TabGeneral").map(String::as_str),
            Some("Genel")
        );
        assert_eq!(bundle.entries.len(), locale::LocaleKey::ALL.len());
    }

    #[test]
    fn locale_strings_contains_every_variant() {
        let bundle = locale_strings_for(Language::English);
        for (_, name) in locale::LocaleKey::ALL {
            assert!(
                bundle.entries.contains_key(name),
                "missing key in locale bundle: {name}"
            );
        }
    }

    #[test]
    fn available_languages_uses_canonical_language_catalog() {
        let options = get_available_languages();
        let values: Vec<_> = options.iter().map(|option| option.value).collect();
        let displays: Vec<_> = options.iter().map(|option| option.display).collect();

        assert_eq!(
            values,
            vec![
                "english",
                "chinese",
                "chinesetraditional",
                "japanese",
                "korean",
                "spanish",
                "russian",
                "turkish"
            ]
        );
        assert_eq!(
            displays,
            vec![
                "English",
                "中文",
                "繁體中文",
                "日本語",
                "한국어",
                "Español",
                "Русский",
                "Türkçe"
            ]
        );
    }

    #[test]
    fn parse_locale_language_accepts_aliases() {
        assert!(matches!(
            parse_locale_language("en"),
            Some(Language::English)
        ));
        assert!(matches!(
            parse_locale_language("English"),
            Some(Language::English)
        ));
        assert!(matches!(
            parse_locale_language("zh"),
            Some(Language::Chinese)
        ));
        assert!(matches!(
            parse_locale_language("Chinese"),
            Some(Language::Chinese)
        ));
        assert!(matches!(
            parse_locale_language("中文"),
            Some(Language::Chinese)
        ));
        assert!(matches!(
            parse_locale_language("zh-tw"),
            Some(Language::ChineseTraditional)
        ));
        assert!(matches!(
            parse_locale_language("zh-hant"),
            Some(Language::ChineseTraditional)
        ));
        assert!(matches!(
            parse_locale_language("繁體中文"),
            Some(Language::ChineseTraditional)
        ));
        assert!(matches!(
            parse_locale_language("ja"),
            Some(Language::Japanese)
        ));
        assert!(matches!(
            parse_locale_language("Japanese"),
            Some(Language::Japanese)
        ));
        assert!(matches!(
            parse_locale_language("日本語"),
            Some(Language::Japanese)
        ));
        assert!(matches!(
            parse_locale_language("ko"),
            Some(Language::Korean)
        ));
        assert!(matches!(
            parse_locale_language("ko-kr"),
            Some(Language::Korean)
        ));
        assert!(matches!(
            parse_locale_language("korean"),
            Some(Language::Korean)
        ));
        assert!(matches!(
            parse_locale_language("한국어"),
            Some(Language::Korean)
        ));
        assert!(matches!(
            parse_locale_language("es"),
            Some(Language::Spanish)
        ));
        assert!(matches!(
            parse_locale_language("es-mx"),
            Some(Language::Spanish)
        ));
        assert!(matches!(
            parse_locale_language("spanish"),
            Some(Language::Spanish)
        ));
        assert!(matches!(
            parse_locale_language("español"),
            Some(Language::Spanish)
        ));
        assert!(matches!(
            parse_locale_language("tr-TR"),
            Some(Language::Turkish)
        ));
        assert!(matches!(
            parse_locale_language("Türkçe"),
            Some(Language::Turkish)
        ));
        assert!(parse_locale_language("klingon").is_none());
    }
}
