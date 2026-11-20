//! User preferences, stored as JSON in `%APPDATA%\Shadersweep\settings.json`.

use std::fs;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::theme::ThemeMode;

pub const STALE_DAY_CHOICES: [u32; 4] = [14, 30, 60, 90];

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeMode,
    /// A cache not written to for this many days counts as "not updated recently".
    pub stale_days: u32,
    /// Start with caches under 1 MB hidden.
    pub hide_small: bool,
    /// Look for a new release on startup (at most once a day). Installing always asks.
    pub auto_check_updates: bool,
    /// Unix time of the last completed update check.
    pub last_update_check: Option<u64>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeMode::System,
            stale_days: 30,
            hide_small: true,
            auto_check_updates: true,
            last_update_check: None,
        }
    }
}

impl Settings {
    /// Unknown or out-of-range values (from a hand-edited file) fall back to defaults.
    fn sanitized(mut self) -> Self {
        if !STALE_DAY_CHOICES.contains(&self.stale_days) {
            self.stale_days = Settings::default().stale_days;
        }
        self
    }

    fn from_json(json: &str) -> Result<Self, serde_json::Error> {
        serde_json::from_str::<Settings>(json).map(Settings::sanitized)
    }
}

pub fn settings_path() -> Option<PathBuf> {
    std::env::var_os("APPDATA").map(|dir| PathBuf::from(dir).join("Shadersweep").join("settings.json"))
}

/// Loads saved settings. A missing file is normal on first run; a damaged one is reported
/// so the UI can say the defaults are in use.
pub fn load() -> (Settings, Option<String>) {
    let Some(path) = settings_path() else { return (Settings::default(), None) };
    match fs::read_to_string(&path) {
        Ok(json) => match Settings::from_json(&json) {
            Ok(settings) => (settings, None),
            Err(err) => (Settings::default(), Some(format!("Settings file was unreadable ({err}); using defaults."))),
        },
        Err(err) if err.kind() == io::ErrorKind::NotFound => (Settings::default(), None),
        Err(err) => (Settings::default(), Some(format!("Could not read settings ({err}); using defaults."))),
    }
}

pub fn save(settings: &Settings) -> io::Result<()> {
    let path = settings_path().ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "APPDATA is not set"))?;
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let json = serde_json::to_string_pretty(settings).map_err(io::Error::other)?;
    fs::write(path, json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_json() {
        let settings = Settings {
            theme: ThemeMode::Light,
            stale_days: 60,
            hide_small: false,
            auto_check_updates: false,
            last_update_check: Some(1_700_000_000),
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert_eq!(Settings::from_json(&json).unwrap(), settings);
    }

    #[test]
    fn fills_missing_and_invalid_fields_with_defaults() {
        let settings = Settings::from_json(r#"{"theme":"dark","stale_days":7}"#).unwrap();
        assert_eq!(settings.theme, ThemeMode::Dark);
        assert_eq!(settings.stale_days, 30);
        assert!(settings.hide_small);
        assert!(Settings::from_json(r#"{"theme":"neon"}"#).is_err());
    }
}
