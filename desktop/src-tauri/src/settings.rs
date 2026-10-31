//! Shell settings — `settings.json` next to the instance registry: the window
//! titlebar mode, and how long an instance's page may stay off screen before
//! the shell suspends it (suspend.rs). Written atomically (tmp + rename), same
//! as the registry, so a crash mid-write cannot truncate it.
//!
//! Reading it never stops the app: a value this build does not know (a newer
//! build wrote it, then a downgrade) reads as the default, and a file that is
//! not JSON at all is moved aside and replaced by defaults.

use std::{fs, path::Path, time::Duration};

use serde::{Deserialize, Deserializer, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub enum TitlebarMode {
    /// Custom chrome drawn in the webview, with a drag region. Default on
    /// every OS — macOS and Windows native titlebars were not wired when the
    /// window shipped `decorations: false`.
    #[default]
    Themed,
    /// The operating system's titlebar (`Window::set_decorations(true)`).
    Os,
    /// No chrome. Close/move via the OS (Alt+F4, etc.).
    None,
}

/// The suspend timeout when settings.json does not say: ten minutes.
pub const DEFAULT_SUSPEND_MINUTES: u32 = 10;

/// The longest timeout a file may ask for: a day. Anything above it, like
/// anything that is not a whole number of minutes, reads as the default.
pub const MAX_SUSPEND_MINUTES: u32 = 24 * 60;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSettings {
    #[serde(default, deserialize_with = "lenient")]
    pub titlebar: TitlebarMode,
    /// Minutes an instance's page may stay off screen before the shell frees
    /// its webview (suspend.rs). `0` is Off. An older build ignores the field.
    #[serde(default = "default_suspend", deserialize_with = "lenient_minutes")]
    pub suspend_after_minutes: u32,
}

impl Default for DesktopSettings {
    fn default() -> Self {
        Self {
            titlebar: TitlebarMode::default(),
            suspend_after_minutes: DEFAULT_SUSPEND_MINUTES,
        }
    }
}

impl DesktopSettings {
    /// None when suspending is off.
    pub fn suspend_after(&self) -> Option<Duration> {
        (self.suspend_after_minutes > 0)
            .then(|| Duration::from_secs(u64::from(self.suspend_after_minutes) * 60))
    }
}

fn default_suspend() -> u32 {
    DEFAULT_SUSPEND_MINUTES
}

/// A whole number of minutes from 0 (Off) to a day; anything else is the
/// default, not a startup failure.
fn lenient_minutes<'de, D: Deserializer<'de>>(d: D) -> Result<u32, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value::<u32>(value)
        .ok()
        .filter(|m| *m <= MAX_SUSPEND_MINUTES)
        .unwrap_or(DEFAULT_SUSPEND_MINUTES))
}

/// Any value this build cannot read is the default, not a startup failure.
fn lenient<'de, D: Deserializer<'de>, T: Default + serde::de::DeserializeOwned>(
    d: D,
) -> Result<T, D::Error> {
    let value = serde_json::Value::deserialize(d)?;
    Ok(serde_json::from_value(value).unwrap_or_default())
}

/// The settings, plus a sentence for the launcher when the file had to be
/// replaced.
pub fn load(path: &Path) -> (DesktopSettings, Option<String>) {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return (DesktopSettings::default(), None);
        }
        Err(e) => {
            return (
                DesktopSettings::default(),
                Some(format!(
                    "The shell settings at {} could not be read ({e}); defaults are in use",
                    path.display()
                )),
            );
        }
    };
    match serde_json::from_str(&raw) {
        Ok(settings) => (settings, None),
        Err(e) => {
            let moved = match crate::registry::set_aside(path) {
                Ok(to) => format!("it was moved to {}", to.display()),
                Err(err) => format!("it could not be moved aside ({err})"),
            };
            (
                DesktopSettings::default(),
                Some(format!(
                    "The shell settings could not be read ({e}); {moved}, and defaults are in use"
                )),
            )
        }
    }
}

pub fn save(path: &Path, settings: &DesktopSettings) -> Result<(), String> {
    let tmp = path.with_extension("json.tmp");
    let json =
        serde_json::to_string_pretty(settings).map_err(|e| format!("serializing settings: {e}"))?;
    fs::write(&tmp, json).map_err(|e| format!("writing {}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("replacing {}: {e}", path.display()))?;
    Ok(())
}

pub fn apply(window: &tauri::Window, mode: TitlebarMode) -> Result<(), String> {
    window
        .set_decorations(matches!(mode, TitlebarMode::Os))
        .map_err(|e| format!("setting window decorations: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir() -> std::path::PathBuf {
        let dir =
            std::env::temp_dir().join(format!("talaria-desktop-settings-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn missing_file_is_themed() {
        let dir = temp_dir();
        let (loaded, notice) = load(&dir.join("settings.json"));
        assert_eq!(loaded.titlebar, TitlebarMode::Themed);
        assert!(notice.is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn roundtrip() {
        let dir = temp_dir();
        let path = dir.join("settings.json");
        let settings = DesktopSettings {
            titlebar: TitlebarMode::Os,
            suspend_after_minutes: 0,
        };
        save(&path, &settings).unwrap();
        assert_eq!(load(&path), (settings, None));
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_file_still_defaults_titlebar() {
        let parsed: DesktopSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(parsed.titlebar, TitlebarMode::Themed);
    }

    #[test]
    fn a_value_from_a_newer_build_reads_as_the_default() {
        let parsed: DesktopSettings = serde_json::from_str(r#"{"titlebar":"fancy"}"#).unwrap();
        assert_eq!(parsed.titlebar, TitlebarMode::Themed);
        let parsed: DesktopSettings = serde_json::from_str(r#"{"titlebar":7}"#).unwrap();
        assert_eq!(parsed.titlebar, TitlebarMode::Themed);
        let parsed: DesktopSettings =
            serde_json::from_str(r#"{"titlebar":"none","later":true}"#).unwrap();
        assert_eq!(parsed.titlebar, TitlebarMode::None);
    }

    #[test]
    fn the_suspend_timeout_defaults_to_ten_minutes_and_zero_is_off() {
        let parsed: DesktopSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(parsed.suspend_after_minutes, DEFAULT_SUSPEND_MINUTES);
        assert_eq!(parsed.suspend_after(), Some(Duration::from_secs(600)));
        let off: DesktopSettings = serde_json::from_str(r#"{"suspendAfterMinutes":0}"#).unwrap();
        assert_eq!(off.suspend_after(), None);
        let one: DesktopSettings = serde_json::from_str(r#"{"suspendAfterMinutes":1}"#).unwrap();
        assert_eq!(one.suspend_after(), Some(Duration::from_secs(60)));
        let day: DesktopSettings = serde_json::from_str(r#"{"suspendAfterMinutes":1440}"#).unwrap();
        assert_eq!(day.suspend_after_minutes, 1440);
    }

    #[test]
    fn a_suspend_timeout_this_build_cannot_read_is_the_default() {
        for raw in [
            r#"{"suspendAfterMinutes":-5}"#,
            r#"{"suspendAfterMinutes":2.5}"#,
            r#"{"suspendAfterMinutes":"10"}"#,
            r#"{"suspendAfterMinutes":null}"#,
            r#"{"suspendAfterMinutes":1441}"#,
        ] {
            let parsed: DesktopSettings = serde_json::from_str(raw).unwrap();
            assert_eq!(
                parsed.suspend_after_minutes, DEFAULT_SUSPEND_MINUTES,
                "{raw}"
            );
        }
        // And a timeout never costs the titlebar mode beside it.
        let parsed: DesktopSettings =
            serde_json::from_str(r#"{"titlebar":"os","suspendAfterMinutes":"x"}"#).unwrap();
        assert_eq!(parsed.titlebar, TitlebarMode::Os);
    }

    #[test]
    fn a_file_that_is_not_json_is_set_aside_with_a_notice() {
        let dir = temp_dir();
        let path = dir.join("settings.json");
        fs::write(&path, "{titlebar:").unwrap();
        let (loaded, notice) = load(&path);
        assert_eq!(loaded, DesktopSettings::default());
        assert!(notice.unwrap().contains("moved to"));
        assert!(!path.exists());
        let _ = fs::remove_dir_all(&dir);
    }
}
