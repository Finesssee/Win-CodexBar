//! Global keyboard shortcuts for CodexBar
//!
//! Provides system-wide hotkeys to open the menu

#![allow(dead_code, reason = "keyboard shortcut types reserved for future global hotkey integration")]

use global_hotkey::{
    GlobalHotKeyEvent, GlobalHotKeyManager,
    hotkey::{Code, HotKey, Modifiers},
};
use std::sync::Arc;
use std::sync::atomic::AtomicBool;

const KEY_ALIASES: &[(&str, Code)] = &[
    ("a", Code::KeyA),
    ("b", Code::KeyB),
    ("c", Code::KeyC),
    ("d", Code::KeyD),
    ("e", Code::KeyE),
    ("f", Code::KeyF),
    ("g", Code::KeyG),
    ("h", Code::KeyH),
    ("i", Code::KeyI),
    ("j", Code::KeyJ),
    ("k", Code::KeyK),
    ("l", Code::KeyL),
    ("m", Code::KeyM),
    ("n", Code::KeyN),
    ("o", Code::KeyO),
    ("p", Code::KeyP),
    ("q", Code::KeyQ),
    ("r", Code::KeyR),
    ("s", Code::KeyS),
    ("t", Code::KeyT),
    ("u", Code::KeyU),
    ("v", Code::KeyV),
    ("w", Code::KeyW),
    ("x", Code::KeyX),
    ("y", Code::KeyY),
    ("z", Code::KeyZ),
    ("0", Code::Digit0),
    ("1", Code::Digit1),
    ("2", Code::Digit2),
    ("3", Code::Digit3),
    ("4", Code::Digit4),
    ("5", Code::Digit5),
    ("6", Code::Digit6),
    ("7", Code::Digit7),
    ("8", Code::Digit8),
    ("9", Code::Digit9),
    ("f1", Code::F1),
    ("f2", Code::F2),
    ("f3", Code::F3),
    ("f4", Code::F4),
    ("f5", Code::F5),
    ("f6", Code::F6),
    ("f7", Code::F7),
    ("f8", Code::F8),
    ("f9", Code::F9),
    ("f10", Code::F10),
    ("f11", Code::F11),
    ("f12", Code::F12),
    ("space", Code::Space),
    ("enter", Code::Enter),
    ("return", Code::Enter),
    ("escape", Code::Escape),
    ("esc", Code::Escape),
    ("tab", Code::Tab),
];

const KEY_LABELS: &[(Code, &str)] = &[
    (Code::KeyA, "A"),
    (Code::KeyB, "B"),
    (Code::KeyC, "C"),
    (Code::KeyD, "D"),
    (Code::KeyE, "E"),
    (Code::KeyF, "F"),
    (Code::KeyG, "G"),
    (Code::KeyH, "H"),
    (Code::KeyI, "I"),
    (Code::KeyJ, "J"),
    (Code::KeyK, "K"),
    (Code::KeyL, "L"),
    (Code::KeyM, "M"),
    (Code::KeyN, "N"),
    (Code::KeyO, "O"),
    (Code::KeyP, "P"),
    (Code::KeyQ, "Q"),
    (Code::KeyR, "R"),
    (Code::KeyS, "S"),
    (Code::KeyT, "T"),
    (Code::KeyU, "U"),
    (Code::KeyV, "V"),
    (Code::KeyW, "W"),
    (Code::KeyX, "X"),
    (Code::KeyY, "Y"),
    (Code::KeyZ, "Z"),
    (Code::Digit0, "0"),
    (Code::Digit1, "1"),
    (Code::Digit2, "2"),
    (Code::Digit3, "3"),
    (Code::Digit4, "4"),
    (Code::Digit5, "5"),
    (Code::Digit6, "6"),
    (Code::Digit7, "7"),
    (Code::Digit8, "8"),
    (Code::Digit9, "9"),
    (Code::F1, "F1"),
    (Code::F2, "F2"),
    (Code::F3, "F3"),
    (Code::F4, "F4"),
    (Code::F5, "F5"),
    (Code::F6, "F6"),
    (Code::F7, "F7"),
    (Code::F8, "F8"),
    (Code::F9, "F9"),
    (Code::F10, "F10"),
    (Code::F11, "F11"),
    (Code::F12, "F12"),
    (Code::Space, "Space"),
    (Code::Enter, "Enter"),
    (Code::Escape, "Esc"),
    (Code::Tab, "Tab"),
];

/// Keyboard shortcut manager
pub struct ShortcutManager {
    manager: GlobalHotKeyManager,
    open_menu_id: u32,
    triggered: Arc<AtomicBool>,
}

impl ShortcutManager {
    /// Create a new shortcut manager with default shortcuts
    pub fn new() -> anyhow::Result<Self> {
        let manager = GlobalHotKeyManager::new()?;

        // Default shortcut: Ctrl+Shift+U (U for Usage)
        let open_menu = HotKey::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyU);
        let open_menu_id = open_menu.id();

        manager.register(open_menu)?;

        Ok(Self {
            manager,
            open_menu_id,
            triggered: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Register a custom shortcut for opening the menu
    pub fn set_open_menu_shortcut(
        &mut self,
        modifiers: Modifiers,
        key: Code,
    ) -> anyhow::Result<()> {
        // Unregister old shortcut
        let old = HotKey::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyU);
        let _ = self.manager.unregister(old);

        // Register new shortcut
        let new_hotkey = HotKey::new(Some(modifiers), key);
        self.open_menu_id = new_hotkey.id();
        self.manager.register(new_hotkey)?;

        Ok(())
    }

    /// Check if the open menu shortcut was triggered
    /// Call this in your event loop
    pub fn check_events(&self) -> bool {
        if let Ok(event) = GlobalHotKeyEvent::receiver().try_recv()
            && event.id == self.open_menu_id
        {
            return true;
        }
        false
    }

    /// Get the triggered flag (for async usage)
    pub fn triggered_flag(&self) -> Arc<AtomicBool> {
        Arc::clone(&self.triggered)
    }
}

impl Drop for ShortcutManager {
    fn drop(&mut self) {
        // Unregister all shortcuts on drop
        let hotkey = HotKey::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::KeyU);
        let _ = self.manager.unregister(hotkey);
    }
}

/// Parse a shortcut string like "Ctrl+Shift+U" into modifiers and key
pub fn parse_shortcut(s: &str) -> Option<(Modifiers, Code)> {
    let mut modifiers = Modifiers::empty();
    let mut key_code = None;

    for part in s.split('+').map(str::trim).filter(|part| !part.is_empty()) {
        if let Some(parsed_modifier) = parse_modifier(part) {
            modifiers |= parsed_modifier;
        } else if let Some(parsed_key) = parse_key(part) {
            key_code = Some(parsed_key);
        }
    }

    key_code.map(|k| (modifiers, k))
}

fn parse_modifier(token: &str) -> Option<Modifiers> {
    match token.to_ascii_lowercase().as_str() {
        "ctrl" | "control" => Some(Modifiers::CONTROL),
        "shift" => Some(Modifiers::SHIFT),
        "alt" => Some(Modifiers::ALT),
        "super" | "win" | "meta" => Some(Modifiers::SUPER),
        _ => None,
    }
}

fn parse_key(token: &str) -> Option<Code> {
    let normalized = token.to_ascii_lowercase();
    KEY_ALIASES
        .iter()
        .find_map(|(alias, code)| (*alias == normalized).then_some(*code))
}

/// Format a shortcut for display
pub fn format_shortcut(modifiers: Modifiers, key: Code) -> String {
    let mut parts = Vec::new();

    if modifiers.contains(Modifiers::CONTROL) {
        parts.push("Ctrl");
    }
    if modifiers.contains(Modifiers::SHIFT) {
        parts.push("Shift");
    }
    if modifiers.contains(Modifiers::ALT) {
        parts.push("Alt");
    }
    if modifiers.contains(Modifiers::SUPER) {
        parts.push("Win");
    }

    parts.push(format_key(key));
    parts.join("+")
}

fn format_key(key: Code) -> &'static str {
    KEY_LABELS
        .iter()
        .find_map(|(code, label)| (*code == key).then_some(*label))
        .unwrap_or("?")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_shortcut() {
        let (mods, key) = parse_shortcut("Ctrl+Shift+U").unwrap();
        assert!(mods.contains(Modifiers::CONTROL));
        assert!(mods.contains(Modifiers::SHIFT));
        assert_eq!(key, Code::KeyU);

        let (mods, key) = parse_shortcut("Alt+F1").unwrap();
        assert!(mods.contains(Modifiers::ALT));
        assert_eq!(key, Code::F1);
    }

    #[test]
    fn test_format_shortcut() {
        let s = format_shortcut(Modifiers::CONTROL | Modifiers::SHIFT, Code::KeyU);
        assert_eq!(s, "Ctrl+Shift+U");
    }
}
