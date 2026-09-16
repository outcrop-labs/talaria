//! The instance registry — `instances.json` in the app data dir. Small,
//! written atomically (tmp + rename) so a crash mid-write can never truncate
//! it, and read once at boot into the in-memory Vec every command mutates.

use std::{fs, path::Path, time::UNIX_EPOCH};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Instance {
    /// Local id (uuid v4): webview label suffix and data-dir name.
    pub id: String,
    /// The beacon's instance uuid — the dedupe key across URLs.
    pub instance_id: String,
    pub label: String,
    /// Normalized origin, no trailing slash ("https://host[:port]").
    pub url: String,
    /// Epoch ms.
    pub created_at: i64,
    pub last_opened_at: Option<i64>,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct RegistryFile {
    instances: Vec<Instance>,
}

pub fn load(path: &Path) -> Result<Vec<Instance>, String> {
    if !path.exists() {
        return Ok(Vec::new());
    }
    let raw = fs::read_to_string(path).map_err(|e| format!("reading {}: {e}", path.display()))?;
    let file: RegistryFile =
        serde_json::from_str(&raw).map_err(|e| format!("parsing {}: {e}", path.display()))?;
    Ok(file.instances)
}

pub fn save(path: &Path, instances: &[Instance]) -> Result<(), String> {
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(&RegistryFile {
        instances: instances.to_vec(),
    })
    .map_err(|e| format!("serializing registry: {e}"))?;
    fs::write(&tmp, json).map_err(|e| format!("writing {}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("replacing {}: {e}", path.display()))?;
    Ok(())
}

pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "talaria-desktop-test-{name}-{}",
            uuid::Uuid::new_v4()
        ));
        let _ = fs::remove_file(&path);
        path
    }

    fn sample(id: &str, instance_id: &str) -> Instance {
        Instance {
            id: id.to_string(),
            instance_id: instance_id.to_string(),
            label: "Outcrop".to_string(),
            url: "https://outcrop.example".to_string(),
            created_at: 1_000,
            last_opened_at: None,
        }
    }

    #[test]
    fn a_missing_file_is_an_empty_registry() {
        let path = temp_path("missing");
        assert!(load(&path).unwrap().is_empty());
    }

    #[test]
    fn save_then_load_round_trips_fields_and_order() {
        let path = temp_path("roundtrip");
        let mut a = sample("a", "11111111-1111-1111-1111-111111111111");
        a.last_opened_at = Some(2_000);
        let b = sample("b", "22222222-2222-2222-2222-222222222222");
        save(&path, &[a.clone(), b.clone()]).unwrap();
        assert_eq!(load(&path).unwrap(), vec![a, b]);
        fs::remove_file(&path).unwrap();
    }

    #[test]
    fn save_leaves_no_tmp_file_behind() {
        let path = temp_path("atomic");
        save(
            &path,
            &[sample("a", "11111111-1111-1111-1111-111111111111")],
        )
        .unwrap();
        assert!(path.exists());
        assert!(!path.with_extension("json.tmp").exists());
        fs::remove_file(&path).unwrap();
    }
}
