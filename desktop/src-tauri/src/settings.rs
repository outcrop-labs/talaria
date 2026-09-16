//! Shell settings — `settings.json` next to the instance registry. Today this
//! is the window titlebar mode. Written atomically (tmp + rename), same as
//! the registry, so a crash mid-write cannot truncate it.

use std::{fs, path::Path};

use serde::{Deserialize, Serialize};

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
    #[serde(default)]
    pub titlebar: TitlebarMode,
}

pub fn load(path: &Path) -> Result<DesktopSettings, String> {
    if !path.exists() {
        return Ok(DesktopSettings::default());
    }
    let raw = fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    serde_json::from_str(&raw).map_err(|e| format!("parsing {}: {e}", path.display()))
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

    #[test]
    fn missing_file_is_themed() {
        let dir =
            std::env::temp_dir().join(format!("talaria-desktop-settings-{}", uuid::Uuid::new_v4()));
        let path = dir.join("settings.json");
        let loaded = load(&path).unwrap();
        assert_eq!(loaded.titlebar, TitlebarMode::Themed);
    }

    #[test]
    fn roundtrip() {
        let dir =
            std::env::temp_dir().join(format!("talaria-desktop-settings-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();
        let path = dir.join("settings.json");
        let settings = DesktopSettings {
            titlebar: TitlebarMode::Os,
        };
        save(&path, &settings).unwrap();
        assert_eq!(load(&path).unwrap(), settings);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn unknown_file_still_defaults_titlebar() {
        let parsed: DesktopSettings = serde_json::from_str("{}").unwrap();
        assert_eq!(parsed.titlebar, TitlebarMode::Themed);
    }
}
