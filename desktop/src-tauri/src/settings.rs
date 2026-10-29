//! Shell settings — `settings.json` next to the instance registry. Today this
//! is the window titlebar mode. Written atomically (tmp + rename), same as
//! the registry, so a crash mid-write cannot truncate it.
//!
//! Reading it never stops the app: a value this build does not know (a newer
//! build wrote it, then a downgrade) reads as the default, and a file that is
//! not JSON at all is moved aside and replaced by defaults.

use std::{fs, path::Path};

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

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase")]
pub struct DesktopSettings {
    #[serde(default, deserialize_with = "lenient")]
    pub titlebar: TitlebarMode,
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
