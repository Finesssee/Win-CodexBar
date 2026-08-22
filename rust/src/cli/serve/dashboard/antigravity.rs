use std::collections::{HashMap, HashSet};

use crate::core::NamedRateWindow;

/// Identify model-family lanes that are known untouched. Unknown-zero lanes stay
/// visible, and an all-zero global reset keeps every family visible.
pub(super) fn idle_window_ids(windows: &[NamedRateWindow]) -> HashSet<String> {
    if windows.is_empty() {
        return HashSet::new();
    }
    let mut families: HashMap<String, Vec<&NamedRateWindow>> = HashMap::new();
    for window in windows {
        families.entry(family_key(window)).or_default().push(window);
    }
    let idle_families: Vec<_> = families
        .iter()
        .filter(|(_, lanes)| {
            lanes
                .iter()
                .all(|lane| lane.usage_known && lane.window.used_percent <= 0.0)
        })
        .map(|(family, _)| family.clone())
        .collect();
    if idle_families.len() == families.len() {
        return HashSet::new();
    }
    idle_families
        .into_iter()
        .flat_map(|family| {
            families
                .get(&family)
                .into_iter()
                .flatten()
                .map(|lane| lane.id.clone())
                .collect::<Vec<_>>()
        })
        .collect()
}

fn family_key(window: &NamedRateWindow) -> String {
    let id = window.id.to_ascii_lowercase();
    if id.contains("gemini") {
        return "gemini".to_string();
    }
    if id.contains("3p") || id.contains("third-party") {
        return "claude-gpt".to_string();
    }
    let title = window.title.trim().to_ascii_lowercase();
    if title.contains("gemini") {
        return "gemini".to_string();
    }
    if title.contains("claude") || title.contains("gpt") {
        return "claude-gpt".to_string();
    }
    for suffix in [" 5-hour", " weekly"] {
        if let Some(stripped) = title.strip_suffix(suffix) {
            let stripped = stripped.trim();
            if !stripped.is_empty() {
                return stripped.to_string();
            }
        }
    }
    title
}
