use super::*;
use std::collections::HashSet;

#[test]
fn test_locale_key_english() {
    assert_eq!(
        get_text(Language::English, LocaleKey::TabGeneral),
        "General"
    );
    assert_eq!(
        get_text(Language::English, LocaleKey::InterfaceLanguage),
        "Interface Language"
    );
    assert_eq!(
        get_text(Language::English, LocaleKey::StartAtLogin),
        "Start at Login"
    );
}

#[test]
fn test_locale_key_chinese() {
    assert_eq!(get_text(Language::Chinese, LocaleKey::TabGeneral), "通用");
    assert_eq!(get_text(Language::Chinese, LocaleKey::TabCookies), "Cookie");
    assert_eq!(
        get_text(Language::Chinese, LocaleKey::InterfaceLanguage),
        "界面语言"
    );
    assert_eq!(
        get_text(Language::Chinese, LocaleKey::StartAtLogin),
        "开机启动"
    );
}

#[test]
fn test_locale_key_traditional_chinese() {
    assert_eq!(
        get_text(Language::ChineseTraditional, LocaleKey::TabGeneral),
        "一般"
    );
    assert_eq!(
        get_text(Language::ChineseTraditional, LocaleKey::InterfaceLanguage),
        "介面語言"
    );
    assert_eq!(
        get_text(Language::ChineseTraditional, LocaleKey::StartAtLogin),
        "登入時啟動"
    );
}

#[test]
fn test_locale_key_japanese() {
    assert_eq!(get_text(Language::Japanese, LocaleKey::TabGeneral), "一般");
    assert_eq!(
        get_text(Language::Japanese, LocaleKey::InterfaceLanguage),
        "表示言語"
    );
    assert_eq!(
        get_text(Language::Japanese, LocaleKey::StartAtLogin),
        "ログイン時に起動"
    );
}

#[test]
fn test_japanese_menu_card_locale_values_are_translated() {
    let cases = [
        (LocaleKey::ActionCopyError, "エラーをコピー"),
        (LocaleKey::DetailWindowPrimary, "プライマリ"),
        (LocaleKey::DetailWindowSecondary, "セカンダリ"),
        (LocaleKey::DetailWindowModelSpecific, "モデル別"),
        (LocaleKey::DetailWindowTertiary, "第3枠"),
        (LocaleKey::DetailWindowExhausted, "使い切りました"),
        (LocaleKey::DetailPaceTitle, "ペース"),
        (LocaleKey::DetailPaceOnTrack, "順調"),
        (LocaleKey::DetailPaceAhead, "先行"),
        (LocaleKey::DetailPaceBehind, "遅れ"),
        (LocaleKey::DetailPaceRunsOutIn, "残り"),
        (LocaleKey::DetailPaceWillLastToReset, "リセットまで持ちます"),
        (LocaleKey::DetailCostTitle, "コスト"),
        (LocaleKey::DetailCostUsed, "使用済み"),
        (LocaleKey::DetailCostLimit, "上限"),
        (LocaleKey::DetailCostRemaining, "残り"),
        (LocaleKey::DetailCostBalance, "残高"),
        (LocaleKey::DetailCostResets, "リセット"),
        (LocaleKey::DetailChartCost, "コスト（30日間）"),
        (LocaleKey::DetailChartCredits, "使用クレジット（30日間）"),
        (
            LocaleKey::DetailChartUsageBreakdown,
            "サービス別使用量（30日間）",
        ),
        (
            LocaleKey::DetailChartEmpty,
            "まだチャートデータはありません。",
        ),
    ];

    for (key, expected) in cases {
        assert_eq!(get_text(Language::Japanese, key), expected, "{key:?}");
    }
}

#[test]
fn test_japanese_tray_panel_locale_values_are_translated() {
    // CUA-verified English strings that must not appear in the Japanese tray panel.
    let cases = [
        (LocaleKey::ProviderWeekly, "週間"),
        (LocaleKey::TrayWeeklyPercent, "週間 {}%"),
        (
            LocaleKey::TrayWeeklyExhausted,
            "週間の割り当てを使い切りました",
        ),
        (LocaleKey::ProviderWeeklyLabel, "週間"),
        (LocaleKey::MetricResetsIn, "リセットまで"),
        (LocaleKey::ResetsInShort, "リセットまで"),
        (LocaleKey::ResetsInDaysHours, "リセットまで {}日 {}時間"),
        (LocaleKey::ResetsInHoursMinutes, "リセットまで {}時間 {}分"),
        (LocaleKey::TrayResetsInLabel, "リセットまで {}"),
        (LocaleKey::UpdatedJustNow, "たった今"),
        (LocaleKey::UpdatedMinutesAgo, "{}分前"),
        (LocaleKey::UpdatedHoursAgo, "{}時間前"),
        (LocaleKey::UpdatedDaysAgo, "{}日前"),
        (
            LocaleKey::PanelEstimatedFromLocalLogs,
            "ローカルログから推定したもので、請求書と異なる場合があります",
        ),
        (LocaleKey::TrayStatusError, " (エラー)"),
        (LocaleKey::TrayStatusStale, " (古いデータ)"),
        (LocaleKey::TrayStatusIncident, " (インシデント)"),
        (LocaleKey::TrayStatusPartial, " (一部停止)"),
        (LocaleKey::TrayLoading, "CodexBar - 読み込み中..."),
        (LocaleKey::TrayStatusRowLoading, "読み込み中..."),
        (LocaleKey::TrayStatusRowError, "エラー"),
        (LocaleKey::TrayCreditsRemaining, "残りクレジット {}%"),
        (LocaleKey::TrayCreditsRow, "クレジット {}%"),
        (LocaleKey::ProviderStatusStale, "古い"),
        (LocaleKey::ProviderStatusError, "エラー"),
        (LocaleKey::ProviderStatusLoading, "読み込み中"),
        (LocaleKey::TrayCardErrorBadge, "エラー"),
        (LocaleKey::SummaryWithErrors, "エラーあり"),
        (
            LocaleKey::StateLoadingProviders,
            "プロバイダーを読み込み中...",
        ),
        (LocaleKey::StateError, "エラー"),
        (LocaleKey::PanelAllProviders, "すべてのプロバイダー"),
        (LocaleKey::PanelAllProvidersShort, "すべて"),
        (LocaleKey::PanelZoom, "ズーム"),
        (LocaleKey::PanelMenu, "メニュー"),
        (LocaleKey::PanelToday, "今日"),
        (LocaleKey::PanelThirtyDayCost, "30日間のコスト"),
        (LocaleKey::PanelThirtyDayTokens, "30日間のトークン"),
        (LocaleKey::PanelLatestTokens, "最新トークン"),
        (LocaleKey::PanelTopModelPrefix, "トップモデル"),
        (LocaleKey::PanelUsedSuffix, "使用済み"),
        (LocaleKey::PanelLeftSuffix, "残り"),
        (LocaleKey::PanelOnPaceBudget, "ペース内の予算"),
        (LocaleKey::PanelNow, "現在"),
        (LocaleKey::PanelOneHour, "1時間"),
        (LocaleKey::PanelFiveHours, "5時間"),
        (LocaleKey::PanelTodayBudget, "今日"),
        (LocaleKey::PanelReserveSuffix, "予備"),
        (
            LocaleKey::PanelReserveLastsUntilReset,
            "リセットまで持ちます",
        ),
        (
            LocaleKey::PanelShowAllProviders,
            "すべてのプロバイダーを表示",
        ),
        (LocaleKey::PanelShowFewerProviders, "表示を減らす"),
        (LocaleKey::PanelExpected, "予測"),
        (LocaleKey::PanelActual, "実績"),
    ];

    for (key, expected) in cases {
        assert_eq!(get_text(Language::Japanese, key), expected, "{key:?}");
    }
}

#[test]
fn test_chinese_tray_panel_locale_values_are_translated() {
    let cases = [
        (LocaleKey::PanelAllProviders, "所有提供者"),
        (LocaleKey::PanelAllProvidersShort, "全部"),
        (LocaleKey::PanelZoom, "缩放"),
        (LocaleKey::PanelMenu, "菜单"),
        (LocaleKey::PanelToday, "今日"),
        (LocaleKey::PanelThirtyDayCost, "30天成本"),
        (LocaleKey::PanelThirtyDayTokens, "30天令牌"),
        (LocaleKey::PanelLatestTokens, "最新令牌"),
        (LocaleKey::PanelTopModelPrefix, "热门模型"),
        (LocaleKey::PanelUsedSuffix, "已使用"),
        (LocaleKey::PanelLeftSuffix, "剩余"),
        (LocaleKey::PanelOnPaceBudget, "按节奏预算"),
        (LocaleKey::PanelNow, "现在"),
        (LocaleKey::PanelOneHour, "1小时"),
        (LocaleKey::PanelFiveHours, "5小时"),
        (LocaleKey::PanelTodayBudget, "今日"),
        (LocaleKey::PanelReserveSuffix, "储备"),
        (LocaleKey::PanelReserveLastsUntilReset, "持续到重置"),
        (
            LocaleKey::PanelEstimatedFromLocalLogs,
            "根据本地日志估算；可能与账单不同",
        ),
        (LocaleKey::PanelShowAllProviders, "显示所有提供者"),
        (LocaleKey::PanelShowFewerProviders, "显示较少提供者"),
        (LocaleKey::PanelExpected, "预期"),
        (LocaleKey::PanelActual, "实际"),
    ];

    for (key, expected) in cases {
        assert_eq!(get_text(Language::Chinese, key), expected, "{key:?}");
    }
}

#[test]
fn test_korean_tray_panel_locale_values_are_translated() {
    let cases = [
        (LocaleKey::PanelAllProviders, "모든 제공자"),
        (LocaleKey::PanelAllProvidersShort, "전체"),
        (LocaleKey::PanelZoom, "확대/축소"),
        (LocaleKey::PanelMenu, "메뉴"),
        (LocaleKey::PanelToday, "오늘"),
        (LocaleKey::PanelThirtyDayCost, "30일 비용"),
        (LocaleKey::PanelThirtyDayTokens, "30일 토큰"),
        (LocaleKey::PanelLatestTokens, "최신 토큰"),
        (LocaleKey::PanelTopModelPrefix, "상위 모델"),
        (LocaleKey::PanelUsedSuffix, "사용됨"),
        (LocaleKey::PanelLeftSuffix, "남음"),
        (LocaleKey::PanelOnPaceBudget, "예산 내"),
        (LocaleKey::PanelNow, "지금"),
        (LocaleKey::PanelOneHour, "1시간"),
        (LocaleKey::PanelFiveHours, "5시간"),
        (LocaleKey::PanelTodayBudget, "오늘"),
        (LocaleKey::PanelReserveSuffix, "예비"),
        (LocaleKey::PanelReserveLastsUntilReset, "리셋까지 지속"),
        (
            LocaleKey::PanelEstimatedFromLocalLogs,
            "로컬 로그에서 추정; 청구서와 다를 수 있음",
        ),
        (LocaleKey::PanelShowAllProviders, "모든 제공자 표시"),
        (LocaleKey::PanelShowFewerProviders, "적은 제공자 표시"),
        (LocaleKey::PanelExpected, "예상"),
        (LocaleKey::PanelActual, "실제"),
    ];

    for (key, expected) in cases {
        assert_eq!(get_text(Language::Korean, key), expected, "{key:?}");
    }
}

#[test]
fn test_spanish_tray_panel_locale_values_are_translated() {
    let cases = [
        (LocaleKey::PanelAllProviders, "Todos los proveedores"),
        (LocaleKey::PanelAllProvidersShort, "Todos"),
        (LocaleKey::PanelZoom, "Zoom"),
        (LocaleKey::PanelMenu, "Menú"),
        (LocaleKey::PanelToday, "Hoy"),
        (LocaleKey::PanelThirtyDayCost, "Costo 30d"),
        (LocaleKey::PanelThirtyDayTokens, "Tokens 30d"),
        (LocaleKey::PanelLatestTokens, "Últimos tokens"),
        (LocaleKey::PanelTopModelPrefix, "Modelo principal"),
        (LocaleKey::PanelUsedSuffix, "usado"),
        (LocaleKey::PanelLeftSuffix, "restante"),
        (LocaleKey::PanelOnPaceBudget, "Presupuesto al ritmo"),
        (LocaleKey::PanelNow, "ahora"),
        (LocaleKey::PanelOneHour, "1h"),
        (LocaleKey::PanelFiveHours, "5h"),
        (LocaleKey::PanelTodayBudget, "hoy"),
        (LocaleKey::PanelReserveSuffix, "en reserva"),
        (
            LocaleKey::PanelReserveLastsUntilReset,
            "Dura hasta el reinicio",
        ),
        (
            LocaleKey::PanelEstimatedFromLocalLogs,
            "Estimado desde logs locales; puede diferir de tu factura",
        ),
        (
            LocaleKey::PanelShowAllProviders,
            "Mostrar todos los proveedores",
        ),
        (
            LocaleKey::PanelShowFewerProviders,
            "Mostrar menos proveedores",
        ),
        (LocaleKey::PanelExpected, "Esperado"),
        (LocaleKey::PanelActual, "Real"),
    ];

    for (key, expected) in cases {
        assert_eq!(get_text(Language::Spanish, key), expected, "{key:?}");
    }
}

#[test]
fn test_locale_key_spanish() {
    assert_eq!(
        get_text(Language::Spanish, LocaleKey::TabGeneral),
        "General"
    );
    assert_eq!(
        get_text(Language::Spanish, LocaleKey::InterfaceLanguage),
        "Idioma de la interfaz"
    );
    assert_eq!(
        get_text(Language::Spanish, LocaleKey::StartAtLogin),
        "Iniciar al arrancar"
    );
}

#[test]
fn test_locale_key_russian() {
    assert_eq!(get_text(Language::Russian, LocaleKey::TabGeneral), "Общие");
    assert_eq!(
        get_text(Language::Russian, LocaleKey::InterfaceLanguage),
        "Язык интерфейса"
    );
    assert_eq!(
        get_text(Language::Russian, LocaleKey::StartAtLogin),
        "Запускать при входе в систему"
    );
}

#[test]
fn test_locale_key_turkish() {
    assert_eq!(get_text(Language::Turkish, LocaleKey::TabGeneral), "Genel");
    assert_eq!(
        get_text(Language::Turkish, LocaleKey::InterfaceLanguage),
        "Arayüz Dili"
    );
    assert_eq!(
        get_text(Language::Turkish, LocaleKey::StartAtLogin),
        "Girişte başlat"
    );
}

#[test]
fn test_locale_key_korean() {
    assert_eq!(get_text(Language::Korean, LocaleKey::TabGeneral), "일반");
    assert_eq!(
        get_text(Language::Korean, LocaleKey::InterfaceLanguage),
        "인터페이스 언어"
    );
    assert_eq!(
        get_text(Language::Korean, LocaleKey::StartAtLogin),
        "로그인 시 자동 실행"
    );
}

#[test]
fn test_locale_respects_language_setting() {
    // Test that English language returns English strings
    let lang = Language::English;
    assert_eq!(get_text(lang, LocaleKey::TabAbout), "About");

    // Test that Chinese language returns Chinese strings
    let lang = Language::Chinese;
    assert_eq!(get_text(lang, LocaleKey::TabAbout), "关于");

    // Test that Japanese language returns Japanese strings
    let lang = Language::Japanese;
    assert_eq!(get_text(lang, LocaleKey::TabAbout), "情報");

    // Test that Korean language returns Korean strings
    let lang = Language::Korean;
    assert_eq!(get_text(lang, LocaleKey::TabAbout), "정보");

    // Test that Spanish language returns Spanish strings
    let lang = Language::Spanish;
    assert_eq!(get_text(lang, LocaleKey::TabAbout), "Acerca de");

    // Test that Russian language returns Russian strings
    let lang = Language::Russian;
    assert_eq!(get_text(lang, LocaleKey::TabAbout), "О программе");

    // Test that Turkish language returns Turkish strings
    let lang = Language::Turkish;
    assert_eq!(get_text(lang, LocaleKey::TabAbout), "Hakkında");
}

#[test]
fn test_english_is_complete_and_other_languages_can_fallback() {
    let resources = [
        ("en-US", include_str!("en-US.ftl")),
        ("zh-CN", include_str!("zh-CN.ftl")),
        ("zh-TW", include_str!("zh-TW.ftl")),
        ("ja-JP", include_str!("ja-JP.ftl")),
        ("ko-KR", include_str!("ko-KR.ftl")),
        ("es-MX", include_str!("es-MX.ftl")),
        ("ru-RU", include_str!("ru-RU.ftl")),
        ("tr-TR", include_str!("tr-TR.ftl")),
    ];

    let resource_keys: Vec<(&str, HashSet<&str>)> = resources
        .into_iter()
        .map(|(locale, resource)| (locale, resource_key_names(resource)))
        .collect();
    let locale_key_names: HashSet<&str> = LocaleKey::ALL.iter().map(|(_, name)| *name).collect();

    for (locale, keys) in &resource_keys {
        for name in keys {
            assert!(
                locale_key_names.contains(name),
                "unknown Fluent key {name} in {locale}"
            );
        }
    }

    for (key, name) in LocaleKey::ALL {
        assert_eq!(key.name(), *name);
        assert!(
            resource_keys[0].1.contains(name),
            "missing Fluent key {name} in en-US"
        );
        for lang in Language::all() {
            let text = LOCALES
                .try_lookup(language_id(*lang), name)
                .unwrap_or_default();
            assert!(
                !text.trim().is_empty(),
                "missing or empty Fluent text for {:?} in {:?}",
                key,
                lang
            );
        }
    }
}

#[test]
fn test_fluent_preserves_literal_placeholders_and_status_spacing() {
    assert_eq!(
        get_text(Language::English, LocaleKey::TrayStatusError),
        " (Error)"
    );
    assert_eq!(
        get_text(Language::English, LocaleKey::TrayCreditsRemaining),
        "Credits remaining {}%"
    );
    assert_eq!(
        get_text(Language::English, LocaleKey::UsedPercent),
        "{:.0}% used"
    );
    assert_eq!(
        get_text(Language::English, LocaleKey::RemainingAmount),
        "{:.2} remaining"
    );
}

fn resource_key_names(resource: &str) -> HashSet<&str> {
    resource
        .lines()
        .filter_map(|line| line.split_once('=').map(|(name, _)| name.trim()))
        .filter(|name| !name.is_empty())
        .collect()
}
