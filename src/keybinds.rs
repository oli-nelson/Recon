use anyhow::{Result, bail};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use serde::Deserialize;
use std::collections::BTreeMap;

/// Action names are shared by config parsing, dispatch, and on-screen help.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Quit,
    Clear,
    GitSnapshot,
    Search,
    NextMatch,
    PreviousMatch,
    WidenHistory,
    NarrowHistory,
    Down,
    Up,
    Collapse,
    Expand,
    Focus,
    ToggleHistory,
    Open,
    Edit,
    PageDown,
    PageUp,
    HalfPageDown,
    HalfPageUp,
    Follow,
    AllHistory,
    Help,
}
impl Action {
    pub const ALL: [Self; 23] = [
        Self::Quit,
        Self::Clear,
        Self::GitSnapshot,
        Self::Search,
        Self::NextMatch,
        Self::PreviousMatch,
        Self::WidenHistory,
        Self::NarrowHistory,
        Self::Down,
        Self::Up,
        Self::Collapse,
        Self::Expand,
        Self::Focus,
        Self::ToggleHistory,
        Self::Open,
        Self::Edit,
        Self::PageDown,
        Self::PageUp,
        Self::HalfPageDown,
        Self::HalfPageUp,
        Self::Follow,
        Self::AllHistory,
        Self::Help,
    ];
    fn defaults(self) -> &'static [&'static str] {
        match self {
            Self::Quit => &["q"],
            Self::Clear => &["c"],
            Self::GitSnapshot => &["g"],
            Self::Search => &["/"],
            Self::NextMatch => &["n"],
            Self::PreviousMatch => &["N"],
            Self::WidenHistory => &["["],
            Self::NarrowHistory => &["]"],
            Self::Down => &["j", "Down"],
            Self::Up => &["k", "Up"],
            Self::Collapse => &["h", "Left"],
            Self::Expand => &["l", "Right"],
            Self::Focus => &["Tab"],
            Self::ToggleHistory => &["d"],
            Self::Open => &["Enter"],
            Self::Edit => &["e"],
            Self::PageDown => &["PageDown"],
            Self::PageUp => &["PageUp"],
            Self::HalfPageDown => &["Ctrl-d"],
            Self::HalfPageUp => &["Ctrl-u"],
            Self::Follow => &["f"],
            Self::AllHistory => &["Esc"],
            Self::Help => &["?"],
        }
    }
    /// Existing UI actions use canonical events; user events are resolved first.
    pub fn event(self) -> KeyEvent {
        parse(self.defaults()[0]).expect("valid built-in key")
    }
}

#[derive(Debug, Clone)]
pub struct Keybinds(BTreeMap<Action, Vec<String>>);
impl Default for Keybinds {
    fn default() -> Self {
        Self(
            Action::ALL
                .into_iter()
                .map(|a| (a, a.defaults().iter().map(|s| s.to_string()).collect()))
                .collect(),
        )
    }
}
impl Keybinds {
    pub fn with_overrides(overrides: BTreeMap<Action, Vec<String>>) -> Result<Self> {
        let mut bindings = Self::default();
        bindings.0.extend(overrides);
        let mut seen = Vec::new();
        for (&action, keys) in &bindings.0 {
            for key in keys {
                let event = parse(key)?;
                if let Some((_, previous)) = seen.iter().find(|(e, _)| *e == event) {
                    bail!("Key {key:?} is assigned to both {previous:?} and {action:?}");
                }
                seen.push((event, action));
            }
        }
        Ok(bindings)
    }
    pub fn resolve(&self, event: KeyEvent) -> Option<KeyEvent> {
        let event = normalize(event);
        self.0.iter().find_map(|(action, keys)| {
            keys.iter()
                .any(|k| parse(k).ok() == Some(event))
                .then(|| action.event())
        })
    }
    pub fn label(&self, action: Action) -> String {
        let keys = &self.0[&action];
        if keys.is_empty() {
            "unbound".into()
        } else {
            keys.join("/")
        }
    }
}
fn normalize(mut event: KeyEvent) -> KeyEvent {
    if event.code == KeyCode::Tab && event.modifiers.contains(KeyModifiers::SHIFT) {
        event.code = KeyCode::BackTab;
    }
    // Terminals disagree about reporting Shift for printable uppercase symbols.
    if let KeyCode::Char(c) = event.code {
        if c.is_uppercase() || c.is_ascii_punctuation() {
            event.modifiers.remove(KeyModifiers::SHIFT);
        }
    }
    KeyEvent::new(event.code, event.modifiers)
}
fn parse(input: &str) -> Result<KeyEvent> {
    let mut rest = input;
    let mut modifiers = KeyModifiers::NONE;
    loop {
        let Some((prefix, suffix)) = rest.split_once('-') else {
            break;
        };
        let modifier = match prefix.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => KeyModifiers::CONTROL,
            "alt" => KeyModifiers::ALT,
            "shift" => KeyModifiers::SHIFT,
            _ => break,
        };
        modifiers.insert(modifier);
        rest = suffix;
    }
    let code = match rest.to_ascii_lowercase().as_str() {
        "enter" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "tab" => KeyCode::Tab,
        "backtab" => {
            modifiers.insert(KeyModifiers::SHIFT);
            KeyCode::BackTab
        }
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "backspace" => KeyCode::Backspace,
        "delete" => KeyCode::Delete,
        "insert" => KeyCode::Insert,
        "space" => KeyCode::Char(' '),
        _ if rest.len() > 1 && rest.to_ascii_lowercase().starts_with('f') => {
            let number = rest[1..]
                .parse::<u8>()
                .ok()
                .filter(|n| (1..=24).contains(n));
            KeyCode::F(number.ok_or_else(|| anyhow::anyhow!("Unknown key binding {input:?}"))?)
        }
        _ => {
            let mut chars = rest.chars();
            let Some(mut c) = chars.next() else {
                bail!("Empty key binding")
            };
            if chars.next().is_some() {
                bail!("Unknown key binding {input:?}")
            }
            if modifiers.contains(KeyModifiers::SHIFT) && c.is_ascii_lowercase() {
                c = c.to_ascii_uppercase();
            }
            KeyCode::Char(c)
        }
    };
    Ok(normalize(KeyEvent::new(code, modifiers)))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overrides_replace_defaults_and_match_modifiers_exactly() {
        let keys =
            Keybinds::with_overrides(BTreeMap::from([(Action::Quit, vec!["Ctrl-x".into()])]))
                .unwrap();
        assert_eq!(
            keys.resolve(parse("Ctrl-x").unwrap()),
            Some(Action::Quit.event())
        );
        assert_eq!(parse("Shift-Tab").unwrap(), parse("BackTab").unwrap());
        assert_eq!(parse("F12").unwrap().code, KeyCode::F(12));
        assert_eq!(keys.resolve(parse("q").unwrap()), None);
        assert_eq!(keys.resolve(parse("Alt-c").unwrap()), None);
        assert_eq!(
            keys.resolve(parse("Shift-n").unwrap()),
            Some(Action::PreviousMatch.event())
        );
    }
    #[test]
    fn reject_conflicts_and_invalid_keys_and_allow_unbinding() {
        for keys in [vec!["c".into()], vec!["unknown".into()], vec!["".into()]] {
            assert!(Keybinds::with_overrides(BTreeMap::from([(Action::Quit, keys)])).is_err());
        }
        assert!(
            Keybinds::with_overrides(BTreeMap::from([(Action::Quit, vec![])]))
                .unwrap()
                .resolve(parse("q").unwrap())
                .is_none()
        );
        assert!(Keybinds::with_overrides(BTreeMap::new()).is_ok());
    }
}
