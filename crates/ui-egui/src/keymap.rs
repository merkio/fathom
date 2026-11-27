//! Keyboard shortcuts: every action has a default binding that `[keys]` in `fathom.toml` can
//! replace, add to or remove.
//!
//! The file maps an action id to one combination or a list of them:
//!
//! ```toml
//! [keys]
//! run_query = "Ctrl+Enter"
//! save_query = ["Ctrl+S", "F2"]
//! export_data = "none"
//! ```
//!
//! `Ctrl` (also `Cmd`, `Command`, `Mod`) is the platform's command key: Control on Windows and
//! Linux, Command on macOS. `Alt` (`Option`) and `Shift` are what they say.

use std::collections::BTreeMap;

use eframe::egui::{InputState, Key, KeyboardShortcut, ModifierNames, Modifiers};

/// Everything the keyboard can trigger. The id is the name used in `fathom.toml`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Action {
    RunQuery,
    ReviewChanges,
    SaveQuery,
    CommandPalette,
    ExportData,
    FocusMode,
    ToggleExplorer,
    ToggleResults,
    ToggleSidePanel,
    ResetLayout,
    AskAi,
    Autocomplete,
    StageDeletion,
}

impl Action {
    pub const ALL: [Action; 13] = [
        Action::RunQuery,
        Action::ReviewChanges,
        Action::SaveQuery,
        Action::CommandPalette,
        Action::ExportData,
        Action::FocusMode,
        Action::ToggleExplorer,
        Action::ToggleResults,
        Action::ToggleSidePanel,
        Action::ResetLayout,
        Action::AskAi,
        Action::Autocomplete,
        Action::StageDeletion,
    ];

    pub fn id(self) -> &'static str {
        match self {
            Action::RunQuery => "run_query",
            Action::ReviewChanges => "review_changes",
            Action::SaveQuery => "save_query",
            Action::CommandPalette => "command_palette",
            Action::ExportData => "export_data",
            Action::FocusMode => "focus_mode",
            Action::ToggleExplorer => "toggle_explorer",
            Action::ToggleResults => "toggle_results",
            Action::ToggleSidePanel => "toggle_side_panel",
            Action::ResetLayout => "reset_layout",
            Action::AskAi => "ask_ai",
            Action::Autocomplete => "autocomplete",
            Action::StageDeletion => "stage_deletion",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Action::RunQuery => "Run query",
            Action::ReviewChanges => "Review and apply row changes",
            Action::SaveQuery => "Save query",
            Action::CommandPalette => "Command palette",
            Action::ExportData => "Export results",
            Action::FocusMode => "Focus mode",
            Action::ToggleExplorer => "Show or hide the explorer",
            Action::ToggleResults => "Show or hide the results panel",
            Action::ToggleSidePanel => "Show or hide the history and AI panel",
            Action::ResetLayout => "Reset the layout",
            Action::AskAi => "Ask the AI (focus the prompt)",
            Action::Autocomplete => "Show autocomplete",
            Action::StageDeletion => "Mark the ticked rows for deletion",
        }
    }

    /// Bare keys are fine here: they only act where no text is being typed.
    fn allows_bare_key(self) -> bool {
        self == Action::StageDeletion
    }

    fn from_id(id: &str) -> Option<Action> {
        Action::ALL.into_iter().find(|a| a.id() == id)
    }

    fn defaults(self) -> Vec<KeyboardShortcut> {
        let cmd = Modifiers::COMMAND;
        let shift = Modifiers::SHIFT;
        let s = KeyboardShortcut::new;
        match self {
            Action::RunQuery => vec![s(cmd, Key::Enter)],
            Action::ReviewChanges => vec![s(cmd | shift, Key::Enter)],
            Action::SaveQuery => vec![s(cmd, Key::S)],
            Action::CommandPalette => vec![s(cmd, Key::K)],
            Action::ExportData => vec![s(cmd | shift, Key::E)],
            Action::FocusMode => vec![s(cmd | shift, Key::F)],
            Action::ToggleExplorer => vec![s(cmd, Key::Num1)],
            Action::ToggleResults => vec![s(cmd, Key::Num2)],
            Action::ToggleSidePanel => vec![s(cmd, Key::H)],
            Action::ResetLayout => vec![s(cmd | shift, Key::L)],
            Action::AskAi => vec![s(cmd, Key::I)],
            Action::Autocomplete => vec![s(cmd, Key::Space)],
            Action::StageDeletion => vec![
                s(Modifiers::NONE, Key::Delete),
                s(Modifiers::NONE, Key::Backspace),
            ],
        }
    }
}

/// The bindings in force, plus anything wrong with the user's `[keys]` table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Keymap {
    bindings: BTreeMap<Action, Vec<KeyboardShortcut>>,
    problems: Vec<String>,
}

impl Default for Keymap {
    fn default() -> Self {
        Self {
            bindings: Action::ALL.into_iter().map(|a| (a, a.defaults())).collect(),
            problems: Vec::new(),
        }
    }
}

fn modifier_count(m: Modifiers) -> u8 {
    u8::from(m.alt)
        + u8::from(m.shift)
        + u8::from(m.command)
        + u8::from(m.ctrl)
        + u8::from(m.mac_cmd)
}

/// Parses `"Ctrl+Shift+E"`, `"F2"`, `"Alt+Enter"`.
pub fn parse_shortcut(text: &str) -> Result<KeyboardShortcut, String> {
    let mut modifiers = Modifiers::NONE;
    let mut key: Option<Key> = None;
    for part in text.split('+').map(str::trim) {
        // A trailing "+" (as in "Ctrl++") splits into empty parts: that is the Plus key.
        let part = if part.is_empty() { "+" } else { part };
        match part.to_lowercase().as_str() {
            "ctrl" | "control" | "cmd" | "command" | "mod" => modifiers.command = true,
            "alt" | "option" | "opt" => modifiers.alt = true,
            "shift" => modifiers.shift = true,
            _ => {
                if key.is_some() {
                    return Err(format!("`{text}` has more than one key"));
                }
                key = Some(parse_key(part).ok_or_else(|| format!("unknown key `{part}`"))?);
            }
        }
    }
    let key = key.ok_or_else(|| format!("`{text}` has no key"))?;
    Ok(KeyboardShortcut::new(modifiers, key))
}

fn parse_key(name: &str) -> Option<Key> {
    if let Some(k) = Key::from_name(name) {
        return Some(k);
    }
    // "enter" → "Enter", "f5" → "F5", "pageup" → "PageUp".
    let mut chars = name.chars();
    let capitalised: String = chars
        .next()?
        .to_uppercase()
        .chain(chars.flat_map(char::to_lowercase))
        .collect();
    if let Some(k) = Key::from_name(&capitalised) {
        return Some(k);
    }
    Key::ALL
        .iter()
        .copied()
        .find(|k| k.name().eq_ignore_ascii_case(name))
}

impl Keymap {
    /// The defaults with the user's `[keys]` applied. Mistakes are collected in
    /// [`Keymap::problems`] and the affected action keeps its default.
    pub fn with_overrides(overrides: &BTreeMap<String, Vec<String>>) -> Self {
        let mut map = Self::default();
        for (id, combos) in overrides {
            let Some(action) = Action::from_id(id) else {
                map.problems.push(format!(
                    "keys.{id}: no such action. Known: {}.",
                    Action::ALL.map(Action::id).join(", ")
                ));
                continue;
            };
            let mut parsed = Vec::new();
            let mut ok = true;
            for text in combos {
                if text.eq_ignore_ascii_case("none") {
                    continue;
                }
                match parse_shortcut(text) {
                    Ok(s) if !action.allows_bare_key() && !has_modifier_or_function_key(&s) => {
                        map.problems.push(format!(
                            "keys.{id}: `{text}` needs Ctrl, Alt or Cmd (a bare key would stop you typing)."
                        ));
                        ok = false;
                    }
                    Ok(s) => parsed.push(s),
                    Err(e) => {
                        map.problems.push(format!("keys.{id}: {e}."));
                        ok = false;
                    }
                }
            }
            if ok {
                map.bindings.insert(action, parsed);
            }
        }
        map.report_conflicts();
        map
    }

    fn report_conflicts(&mut self) {
        let mut seen: BTreeMap<String, Action> = BTreeMap::new();
        for action in Action::ALL {
            for s in self.bindings.get(&action).into_iter().flatten() {
                let name = format_shortcut(s);
                if let Some(other) = seen.insert(name.clone(), action)
                    && other != action
                {
                    self.problems.push(format!(
                        "{name} is bound to both `{}` and `{}`; only one of them will fire.",
                        other.id(),
                        action.id()
                    ));
                }
            }
        }
    }

    pub fn problems(&self) -> &[String] {
        &self.problems
    }

    pub fn bindings(&self, action: Action) -> &[KeyboardShortcut] {
        self.bindings.get(&action).map_or(&[], Vec::as_slice)
    }

    /// The default bindings, for the shortcuts list.
    pub fn defaults(action: Action) -> Vec<KeyboardShortcut> {
        action.defaults()
    }

    /// The first binding as text ("Ctrl+Shift+E"), for hints and menus.
    pub fn hint(&self, action: Action) -> Option<String> {
        self.bindings(action).first().map(format_shortcut)
    }

    /// Consumes every shortcut pressed this frame and returns the actions they trigger. More
    /// specific combinations are tried first, because egui lets `Ctrl+Enter` match a press of
    /// `Ctrl+Shift+Enter` too. Actions that act on a bare key are not consumed here (see
    /// [`Keymap::pressed`]).
    pub fn poll(&self, input: &mut InputState) -> Vec<Action> {
        let mut all: Vec<(Action, KeyboardShortcut)> = self
            .bindings
            .iter()
            .filter(|(a, _)| !a.allows_bare_key())
            .flat_map(|(a, list)| list.iter().map(|s| (*a, *s)))
            .collect();
        all.sort_by_key(|(_, s)| std::cmp::Reverse(modifier_count(s.modifiers)));
        let mut fired = Vec::new();
        for (action, shortcut) in all {
            if input.consume_shortcut(&shortcut) && !fired.contains(&action) {
                fired.push(action);
            }
        }
        fired
    }

    /// Whether a bare-key action was pressed this frame (without consuming the key).
    pub fn pressed(&self, input: &InputState, action: Action) -> bool {
        self.bindings(action)
            .iter()
            .any(|s| input.key_pressed(s.logical_key) && input.modifiers.matches_exact(s.modifiers))
    }
}

fn has_modifier_or_function_key(s: &KeyboardShortcut) -> bool {
    s.modifiers != Modifiers::NONE
        || matches!(
            s.logical_key,
            Key::F1
                | Key::F2
                | Key::F3
                | Key::F4
                | Key::F5
                | Key::F6
                | Key::F7
                | Key::F8
                | Key::F9
                | Key::F10
                | Key::F11
                | Key::F12
        )
}

pub fn format_shortcut(s: &KeyboardShortcut) -> String {
    s.format(&ModifierNames::NAMES, cfg!(target_os = "macos"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn overrides(pairs: &[(&str, &[&str])]) -> BTreeMap<String, Vec<String>> {
        pairs
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.iter().map(|s| (*s).to_owned()).collect()))
            .collect()
    }

    #[test]
    fn parses_combinations_case_insensitively() {
        let s = parse_shortcut("ctrl+shift+e").expect("valid");
        assert_eq!(
            s,
            KeyboardShortcut::new(Modifiers::COMMAND | Modifiers::SHIFT, Key::E)
        );
        assert_eq!(parse_shortcut("F2").expect("valid").logical_key, Key::F2);
        assert_eq!(
            parse_shortcut("Alt+enter").expect("valid").logical_key,
            Key::Enter
        );
        assert_eq!(
            parse_shortcut("Ctrl+1").expect("valid").logical_key,
            Key::Num1
        );
        assert!(parse_shortcut("Ctrl+Shift").is_err());
        assert!(parse_shortcut("Ctrl+Nope").is_err());
        assert!(parse_shortcut("Ctrl+A+B").is_err());
    }

    #[test]
    fn every_action_has_a_unique_id_and_a_default() {
        let mut ids: Vec<_> = Action::ALL.iter().map(|a| a.id()).collect();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), Action::ALL.len());
        let map = Keymap::default();
        for a in Action::ALL {
            assert!(!map.bindings(a).is_empty(), "{} has no default", a.id());
        }
        assert!(map.problems().is_empty(), "{:?}", map.problems());
    }

    #[test]
    fn overrides_replace_add_and_remove_bindings() {
        let map = Keymap::with_overrides(&overrides(&[
            ("run_query", &["Ctrl+R"]),
            ("save_query", &["Ctrl+S", "F2"]),
            ("export_data", &["none"]),
        ]));
        assert!(map.problems().is_empty(), "{:?}", map.problems());
        assert_eq!(map.hint(Action::RunQuery).as_deref(), Some("Ctrl+R"));
        assert_eq!(map.bindings(Action::SaveQuery).len(), 2);
        assert!(map.bindings(Action::ExportData).is_empty());
        assert_eq!(map.hint(Action::ExportData), None);
        // Untouched actions keep their defaults.
        assert_eq!(
            map.bindings(Action::CommandPalette),
            Keymap::defaults(Action::CommandPalette)
        );
    }

    #[test]
    fn mistakes_are_reported_and_the_default_stays() {
        let map = Keymap::with_overrides(&overrides(&[
            ("run_querry", &["Ctrl+R"]),
            ("save_query", &["Ctrl+Banana"]),
            ("focus_mode", &["X"]),
            ("export_data", &["Ctrl+S"]),
        ]));
        let text = map.problems().join("\n");
        assert!(text.contains("no such action"), "{text}");
        assert!(text.contains("unknown key"), "{text}");
        assert!(text.contains("needs Ctrl"), "{text}");
        assert!(text.contains("bound to both"), "{text}");
        assert_eq!(
            map.bindings(Action::SaveQuery),
            Keymap::defaults(Action::SaveQuery)
        );
        assert_eq!(
            map.bindings(Action::FocusMode),
            Keymap::defaults(Action::FocusMode)
        );
    }

    #[test]
    fn only_deletion_may_use_a_bare_key() {
        let map = Keymap::with_overrides(&overrides(&[("stage_deletion", &["Delete"])]));
        assert!(map.problems().is_empty(), "{:?}", map.problems());
        let map = Keymap::with_overrides(&overrides(&[("run_query", &["F5"])]));
        assert!(map.problems().is_empty(), "function keys need no modifier");
    }

    #[test]
    fn the_more_specific_shortcut_wins_when_both_match() {
        use eframe::egui::{Context, Event, RawInput};
        let map = Keymap::default();
        let mods = Modifiers::COMMAND | Modifiers::SHIFT;
        let mut input = RawInput::default();
        input.events.push(Event::Key {
            key: Key::Enter,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: mods,
        });
        let mut fired = Vec::new();
        let mut out = Context::default().run_ui(input, |ui| {
            fired = ui.input_mut(|i| map.poll(i));
        });
        out.textures_delta.clear();
        assert_eq!(fired, vec![Action::ReviewChanges]);
    }
}
