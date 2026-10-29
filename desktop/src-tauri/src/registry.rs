//! The instance registry — `instances.json` in the app data dir. Small,
//! written atomically (tmp + rename) so a crash mid-write can never truncate
//! it, and read once at boot into the in-memory Vec every command mutates.
//!
//! Reading it never stops the app. A file that does not parse — a hand edit,
//! a disk that lied — is moved aside next to itself (`instances.json.corrupt-
//! <ms>`), whatever entries still parse are kept, and the launcher says what
//! happened. The old behavior was a panic in setup and no window at all.

use std::{
    fs,
    path::{Path, PathBuf},
    time::UNIX_EPOCH,
};

use serde::{Deserialize, Deserializer, Serialize};

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

/// Where a relaunch lands: the instance the window last showed, or the
/// launcher if that is where the person left it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Resume {
    /// A registry written before `lastView` existed. Reopen the instance the
    /// person opened most recently — what those builds did — but only one they
    /// actually opened (`lastOpenedAt` set).
    Legacy,
    Launcher,
    Instance(String),
}

impl Resume {
    /// The instance to reopen at boot, if any, given what is registered.
    pub fn target(&self, instances: &[Instance]) -> Option<String> {
        match self {
            Self::Launcher => None,
            Self::Instance(id) => instances.iter().find(|i| &i.id == id).map(|i| i.id.clone()),
            Self::Legacy => instances
                .iter()
                .filter(|i| i.last_opened_at.is_some())
                .max_by_key(|i| i.last_opened_at)
                .map(|i| i.id.clone()),
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RegistryFile {
    instances: Vec<Instance>,
    /// `null` = the launcher, `"<id>"` = that instance, absent = written by an
    /// older build (see `Resume::Legacy`). Older builds ignore the field.
    #[serde(default, deserialize_with = "present")]
    last_view: Option<Option<String>>,
}

/// Distinguish an absent field (None, from `default`) from `null`.
fn present<'de, D: Deserializer<'de>>(d: D) -> Result<Option<Option<String>>, D::Error> {
    Option::<String>::deserialize(d).map(Some)
}

pub struct Loaded {
    pub instances: Vec<Instance>,
    pub resume: Resume,
    /// What went wrong reading the file, in words for the launcher.
    pub notice: Option<String>,
    /// The file existed and parsed as written, so the data dirs beside it can
    /// be trusted to be the only live ones (see `sweep_orphans`).
    pub trusted: bool,
}

pub fn load(path: &Path) -> Loaded {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Loaded {
                instances: Vec::new(),
                resume: Resume::Launcher,
                notice: None,
                trusted: false,
            };
        }
        Err(e) => {
            return Loaded {
                instances: Vec::new(),
                resume: Resume::Launcher,
                notice: Some(format!(
                    "The instance list at {} could not be read ({e}), so none are shown",
                    path.display()
                )),
                trusted: false,
            };
        }
    };
    match serde_json::from_str::<RegistryFile>(&raw) {
        Ok(file) => Loaded {
            instances: file.instances,
            resume: match file.last_view {
                None => Resume::Legacy,
                Some(None) => Resume::Launcher,
                Some(Some(id)) => Resume::Instance(id),
            },
            notice: None,
            trusted: true,
        },
        Err(strict) => {
            let (instances, total) = salvage(&raw);
            let aside = set_aside(path);
            let moved = match &aside {
                Ok(to) => format!("it was moved to {}", to.display()),
                Err(e) => format!("it could not be moved aside ({e})"),
            };
            let kept = match (instances.len(), total) {
                (_, None) => "nothing in it could be recovered".to_string(),
                (n, Some(t)) => format!("{n} of {t} instances were recovered"),
            };
            // Write what survived so the next launch reads a clean file. If the
            // move failed, leave the original alone rather than overwrite it.
            if aside.is_ok() {
                let _ = save(path, &instances, None);
            }
            Loaded {
                instances,
                resume: Resume::Launcher,
                notice: Some(format!(
                    "The instance list could not be read ({strict}); {moved}, and {kept}"
                )),
                trusted: false,
            }
        }
    }
}

/// Entries that still parse out of a file that, as a whole, does not. `None`
/// for the total means the file is not even a JSON object with a list in it.
fn salvage(raw: &str) -> (Vec<Instance>, Option<usize>) {
    let Ok(value) = serde_json::from_str::<serde_json::Value>(raw) else {
        return (Vec::new(), None);
    };
    let Some(list) = value.get("instances").and_then(|v| v.as_array()) else {
        return (Vec::new(), None);
    };
    let kept = list
        .iter()
        .filter_map(|v| serde_json::from_value::<Instance>(v.clone()).ok())
        .collect();
    (kept, Some(list.len()))
}

/// Move an unreadable file next to itself, stamped, so nothing is lost and the
/// next write starts clean.
pub fn set_aside(path: &Path) -> std::io::Result<PathBuf> {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    let to = path.with_file_name(format!("{name}.corrupt-{}", now_ms()));
    fs::rename(path, &to)?;
    Ok(to)
}

/// `last_view`: the instance on screen, or None for the launcher.
pub fn save(path: &Path, instances: &[Instance], last_view: Option<&str>) -> Result<(), String> {
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(&RegistryFile {
        instances: instances.to_vec(),
        last_view: Some(last_view.map(str::to_string)),
    })
    .map_err(|e| format!("serializing registry: {e}"))?;
    fs::write(&tmp, json).map_err(|e| format!("writing {}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("replacing {}: {e}", path.display()))?;
    Ok(())
}

/// Delete data dirs under `root` that no registered instance owns. A removed
/// instance's WebKit network process can recreate its storage dir while it
/// shuts down, after the remove already deleted it; this is the sweep that
/// catches those. Only run it against a registry that loaded as written.
pub fn sweep_orphans(root: &Path, instances: &[Instance]) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut removed = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        // Only dirs this shell made: uuid-named. Anything else is left alone.
        if uuid::Uuid::parse_str(&name).is_err() || instances.iter().any(|i| i.id == name) {
            continue;
        }
        let path = entry.path();
        if path.is_dir() && fs::remove_dir_all(&path).is_ok() {
            removed.push(path);
        }
    }
    removed
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

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "talaria-desktop-test-{name}-{}",
            uuid::Uuid::new_v4()
        ));
        fs::create_dir_all(&dir).unwrap();
        dir
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
        let dir = temp_dir("missing");
        let loaded = load(&dir.join("instances.json"));
        assert!(loaded.instances.is_empty());
        assert!(loaded.notice.is_none());
        assert!(!loaded.trusted);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_then_load_round_trips_fields_order_and_view() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("instances.json");
        let mut a = sample("a", "11111111-1111-1111-1111-111111111111");
        a.last_opened_at = Some(2_000);
        let b = sample("b", "22222222-2222-2222-2222-222222222222");
        save(&path, &[a.clone(), b.clone()], Some("b")).unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.instances, vec![a.clone(), b.clone()]);
        assert_eq!(loaded.resume, Resume::Instance("b".into()));
        assert!(loaded.trusted);
        save(&path, &[a, b], None).unwrap();
        assert_eq!(load(&path).resume, Resume::Launcher);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_leaves_no_tmp_file_behind() {
        let dir = temp_dir("atomic");
        let path = dir.join("instances.json");
        save(
            &path,
            &[sample("a", "11111111-1111-1111-1111-111111111111")],
            None,
        )
        .unwrap();
        assert!(path.exists());
        assert!(!path.with_extension("json.tmp").exists());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_truncated_file_is_set_aside_and_the_app_starts_empty() {
        let dir = temp_dir("truncated");
        let path = dir.join("instances.json");
        fs::write(&path, r#"{"instances": ["#).unwrap();
        let loaded = load(&path);
        assert!(loaded.instances.is_empty());
        assert!(!loaded.trusted);
        let notice = loaded.notice.expect("a notice");
        assert!(notice.contains("could not be read"), "{notice}");
        assert!(
            notice.contains("nothing in it could be recovered"),
            "{notice}"
        );
        let aside: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .flatten()
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .filter(|n| n.starts_with("instances.json.corrupt-"))
            .collect();
        assert_eq!(aside.len(), 1, "the original is kept beside it");
        assert_eq!(
            fs::read_to_string(dir.join(&aside[0])).unwrap(),
            r#"{"instances": ["#
        );
        // And the registry is valid again for the next launch.
        assert!(load(&path).trusted);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn good_entries_survive_a_bad_one() {
        let dir = temp_dir("salvage");
        let path = dir.join("instances.json");
        let good = sample("a", "11111111-1111-1111-1111-111111111111");
        let raw = serde_json::json!({
            "instances": [good, {"id": 7, "label": "broken"}],
            "lastView": "a"
        });
        fs::write(&path, raw.to_string()).unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.instances, vec![good]);
        assert_eq!(
            loaded.resume,
            Resume::Launcher,
            "a damaged file never auto-opens"
        );
        assert!(
            loaded
                .notice
                .unwrap()
                .contains("1 of 2 instances were recovered")
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn a_legacy_file_reopens_only_an_instance_that_was_opened() {
        let dir = temp_dir("legacy");
        let path = dir.join("instances.json");
        let never = sample("never", "11111111-1111-1111-1111-111111111111");
        let raw = serde_json::json!({ "instances": [never] });
        fs::write(&path, raw.to_string()).unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.resume, Resume::Legacy);
        assert_eq!(loaded.resume.target(&loaded.instances), None);

        let mut opened = sample("opened", "22222222-2222-2222-2222-222222222222");
        opened.last_opened_at = Some(5);
        let instances = vec![sample("x", "3"), opened];
        assert_eq!(Resume::Legacy.target(&instances), Some("opened".into()));
        assert_eq!(Resume::Instance("gone".into()).target(&instances), None);
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn the_sweep_removes_only_unowned_uuid_dirs() {
        let root = temp_dir("sweep");
        let kept = uuid::Uuid::new_v4().to_string();
        let orphan = uuid::Uuid::new_v4().to_string();
        fs::create_dir_all(root.join(&kept).join("webview")).unwrap();
        fs::create_dir_all(root.join(&orphan).join("webview/storage")).unwrap();
        fs::create_dir_all(root.join("not-ours")).unwrap();
        let removed = sweep_orphans(&root, &[sample(&kept, "1")]);
        assert_eq!(removed, vec![root.join(&orphan)]);
        assert!(root.join(&kept).exists());
        assert!(root.join("not-ours").exists());
        fs::remove_dir_all(&root).unwrap();
    }
}
