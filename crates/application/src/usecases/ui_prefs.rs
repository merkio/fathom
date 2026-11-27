//! Appearance and layout preferences: theme and which panels are shown.
//!
//! Stored as plain strings in the key/value settings (no secrets, no schema change). Unknown or
//! missing values fall back to the defaults, so older or hand-edited files never break start-up.

use std::sync::Arc;

use crate::ports::{SettingsRepository, StorageError};

const THEME_KEY: &str = "ui.theme";
const EXPLORER_KEY: &str = "ui.panel.explorer";
const RESULTS_KEY: &str = "ui.panel.results";
const SIDE_PANEL_KEY: &str = "ui.panel.side";

/// Which colour scheme to use. `System` follows the operating system.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum ThemePreference {
    Light,
    Dark,
    #[default]
    System,
}

impl ThemePreference {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Light => "light",
            Self::Dark => "dark",
            Self::System => "system",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "light" => Some(Self::Light),
            "dark" => Some(Self::Dark),
            "system" => Some(Self::System),
            _ => None,
        }
    }
}

/// The saved layout and appearance. `Default` shows the explorer and the results panel; the
/// History/AI side panel is opt-in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UiPrefs {
    pub theme: ThemePreference,
    pub explorer: bool,
    pub results: bool,
    pub side_panel: bool,
}

impl Default for UiPrefs {
    fn default() -> Self {
        Self {
            theme: ThemePreference::System,
            explorer: true,
            results: true,
            side_panel: false,
        }
    }
}

pub struct UiPrefsStore {
    settings: Arc<dyn SettingsRepository>,
}

impl UiPrefsStore {
    pub fn new(settings: Arc<dyn SettingsRepository>) -> Self {
        Self { settings }
    }

    pub async fn load(&self) -> Result<UiPrefs, StorageError> {
        let d = UiPrefs::default();
        let flag = |v: Option<String>, default: bool| match v.as_deref() {
            Some("true") => true,
            Some("false") => false,
            _ => default,
        };
        Ok(UiPrefs {
            theme: self
                .settings
                .get(THEME_KEY)
                .await?
                .as_deref()
                .and_then(ThemePreference::parse)
                .unwrap_or(d.theme),
            explorer: flag(self.settings.get(EXPLORER_KEY).await?, d.explorer),
            results: flag(self.settings.get(RESULTS_KEY).await?, d.results),
            side_panel: flag(self.settings.get(SIDE_PANEL_KEY).await?, d.side_panel),
        })
    }

    pub async fn save(&self, prefs: &UiPrefs) -> Result<(), StorageError> {
        let b = |v: bool| if v { "true" } else { "false" };
        self.settings.set(THEME_KEY, prefs.theme.as_str()).await?;
        self.settings.set(EXPLORER_KEY, b(prefs.explorer)).await?;
        self.settings.set(RESULTS_KEY, b(prefs.results)).await?;
        self.settings
            .set(SIDE_PANEL_KEY, b(prefs.side_panel))
            .await?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Mutex;

    use async_trait::async_trait;
    use futures::executor::block_on;

    use super::*;

    #[derive(Default)]
    struct MemorySettings(Mutex<HashMap<String, String>>);

    #[async_trait]
    impl SettingsRepository for MemorySettings {
        async fn get(&self, key: &str) -> Result<Option<String>, StorageError> {
            Ok(self.0.lock().unwrap().get(key).cloned())
        }
        async fn set(&self, key: &str, value: &str) -> Result<(), StorageError> {
            self.0.lock().unwrap().insert(key.into(), value.into());
            Ok(())
        }
    }

    #[test]
    fn defaults_when_nothing_is_saved() {
        let store = UiPrefsStore::new(Arc::new(MemorySettings::default()));
        assert_eq!(block_on(store.load()).unwrap(), UiPrefs::default());
    }

    #[test]
    fn saved_preferences_round_trip() {
        let store = UiPrefsStore::new(Arc::new(MemorySettings::default()));
        let prefs = UiPrefs {
            theme: ThemePreference::Dark,
            explorer: false,
            results: false,
            side_panel: true,
        };
        block_on(store.save(&prefs)).unwrap();
        assert_eq!(block_on(store.load()).unwrap(), prefs);
    }

    #[test]
    fn garbage_values_fall_back_to_defaults() {
        let settings = Arc::new(MemorySettings::default());
        block_on(settings.set(THEME_KEY, "neon")).unwrap();
        block_on(settings.set(EXPLORER_KEY, "maybe")).unwrap();
        let store = UiPrefsStore::new(settings);
        assert_eq!(block_on(store.load()).unwrap(), UiPrefs::default());
    }

    #[test]
    fn theme_names_round_trip() {
        for t in [
            ThemePreference::Light,
            ThemePreference::Dark,
            ThemePreference::System,
        ] {
            assert_eq!(ThemePreference::parse(t.as_str()), Some(t));
        }
    }
}
