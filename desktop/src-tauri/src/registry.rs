//! The instance registry — `instances.json` in the app data dir. Small,
//! written atomically (tmp + rename) so a crash mid-write can never truncate
//! it, and read once at boot into the in-memory Vec every command mutates.
//!
//! Reading it never stops the app. A file that does not parse — a hand edit,
//! a disk that lied — is moved aside next to itself (`instances.json.corrupt-
//! <ms>`), whatever entries still parse are kept, and the launcher says what
//! happened. The old behavior was a panic in setup and no window at all.
//!
//! Each instance's last page (`lastUrls`) is kept here too, so a relaunch, or
//! a restore after a suspend, opens where the person was. It lives beside the
//! entries, not in them: `Instance` is what the in-app switcher of every OTHER
//! instance reads (`list_instances`), and one instance's paths are not another
//! origin's business.
//!
//! Data dirs are deleted only for an instance the person removed. The file
//! records each removal (`removed`) until a launch finds its dir gone, because
//! a closing WebKit process can re-create the dir after the remove deleted
//! it. A dir that no entry owns and no removal recorded is never touched: it
//! may be the session of an entry a damaged file lost.

use std::{
    collections::BTreeMap,
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
    /// Local ids of removed instances whose data dir may still come back (see
    /// `sweep_removed`). Older builds ignore the field and drop it on save.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    removed: Vec<String>,
    /// Local id → the last page that instance showed (suspend::restorable_url
    /// already applied). Older builds ignore the field and drop it on save.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    last_urls: BTreeMap<String, String>,
}

/// Everything `save` writes, borrowed from the shell state.
pub struct Record<'a> {
    pub instances: &'a [Instance],
    /// The instance on screen, or None for the launcher.
    pub last_view: Option<&'a str>,
    /// The removals still waiting on a sweep.
    pub removed: &'a [String],
    pub last_urls: &'a BTreeMap<String, String>,
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
    /// Removals whose data dir the next sweep deletes. Always empty for a file
    /// that did not parse: what it recorded is not trusted with a delete.
    pub removed: Vec<String>,
    /// Each registered instance's last page. Entries for ids the file no
    /// longer registers are dropped.
    pub last_urls: BTreeMap<String, String>,
}

pub fn load(path: &Path) -> Loaded {
    let raw = match fs::read_to_string(path) {
        Ok(raw) => raw,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
            return Loaded {
                instances: Vec::new(),
                resume: Resume::Launcher,
                notice: None,
                removed: Vec::new(),
                last_urls: BTreeMap::new(),
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
                removed: Vec::new(),
                last_urls: BTreeMap::new(),
            };
        }
    };
    match serde_json::from_str::<RegistryFile>(&raw) {
        Ok(file) => {
            let mut last_urls = file.last_urls;
            last_urls.retain(|id, _| file.instances.iter().any(|i| &i.id == id));
            Loaded {
                instances: file.instances,
                resume: match file.last_view {
                    None => Resume::Legacy,
                    Some(None) => Resume::Launcher,
                    Some(Some(id)) => Resume::Instance(id),
                },
                notice: None,
                removed: file.removed,
                last_urls,
            }
        }
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
            // The rewrite records no removals, so no launch after it deletes a
            // data dir on the damaged file's word.
            if aside.is_ok() {
                let _ = save(
                    path,
                    &Record {
                        instances: &instances,
                        last_view: None,
                        removed: &[],
                        last_urls: &BTreeMap::new(),
                    },
                );
            }
            Loaded {
                instances,
                resume: Resume::Launcher,
                notice: Some(format!(
                    "The instance list could not be read ({strict}); {moved}, and {kept}"
                )),
                removed: Vec::new(),
                last_urls: BTreeMap::new(),
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

pub fn save(path: &Path, record: &Record<'_>) -> Result<(), String> {
    let tmp = path.with_extension("json.tmp");
    let json = serde_json::to_string_pretty(&RegistryFile {
        instances: record.instances.to_vec(),
        last_view: Some(record.last_view.map(str::to_string)),
        removed: record.removed.to_vec(),
        last_urls: record
            .last_urls
            .iter()
            .filter(|(id, _)| record.instances.iter().any(|i| &i.id == *id))
            .map(|(id, url)| (id.clone(), url.clone()))
            .collect(),
    })
    .map_err(|e| format!("serializing registry: {e}"))?;
    fs::write(&tmp, json).map_err(|e| format!("writing {}: {e}", tmp.display()))?;
    fs::rename(&tmp, path).map_err(|e| format!("replacing {}: {e}", path.display()))?;
    Ok(())
}

/// What a boot sweep did: the dirs it deleted, and the removals to keep.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Swept {
    pub deleted: Vec<PathBuf>,
    pub pending: Vec<String>,
}

/// Delete the data dirs of removed instances (`removed`). A removed
/// instance's WebKit network process can re-create its storage dir while it
/// shuts down, after the remove deleted it; this is the sweep that catches
/// those. A removal stays pending until a launch finds its dir already gone,
/// so a dir re-created during the sweep is caught by the one after.
///
/// Only those dirs: one no registered instance owns but no remove recorded is
/// left alone. It may hold the session of an entry a damaged registry lost,
/// which the corrupt copy beside the registry can still restore. An id that
/// is not a uuid, or that a registered instance owns, is dropped untouched.
pub fn sweep_removed(root: &Path, removed: &[String], instances: &[Instance]) -> Swept {
    let mut swept = Swept::default();
    for id in removed {
        if uuid::Uuid::parse_str(id).is_err() || instances.iter().any(|i| &i.id == id) {
            continue;
        }
        let dir = root.join(id);
        if !dir.exists() {
            continue;
        }
        if fs::remove_dir_all(&dir).is_ok() {
            swept.deleted.push(dir);
        }
        swept.pending.push(id.clone());
    }
    swept
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
        assert!(loaded.removed.is_empty());
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_then_load_round_trips_fields_order_and_view() {
        let dir = temp_dir("roundtrip");
        let path = dir.join("instances.json");
        let mut a = sample("a", "11111111-1111-1111-1111-111111111111");
        a.last_opened_at = Some(2_000);
        let b = sample("b", "22222222-2222-2222-2222-222222222222");
        let gone = vec!["33333333-3333-3333-3333-333333333333".to_string()];
        let urls = BTreeMap::from([
            (
                "b".to_string(),
                "https://outcrop.example/boards?x=1#t".to_string(),
            ),
            ("gone".to_string(), "https://gone.example/".to_string()),
        ]);
        let both = [a.clone(), b.clone()];
        save(
            &path,
            &Record {
                instances: &both,
                last_view: Some("b"),
                removed: &gone,
                last_urls: &urls,
            },
        )
        .unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.instances, vec![a.clone(), b.clone()]);
        assert_eq!(loaded.resume, Resume::Instance("b".into()));
        assert_eq!(loaded.removed, gone);
        assert_eq!(
            loaded.last_urls,
            BTreeMap::from([(
                "b".to_string(),
                "https://outcrop.example/boards?x=1#t".to_string()
            )]),
            "a page is kept only for a registered instance"
        );
        save(
            &path,
            &Record {
                instances: &both,
                last_view: None,
                removed: &[],
                last_urls: &BTreeMap::new(),
            },
        )
        .unwrap();
        let loaded = load(&path);
        assert_eq!(loaded.resume, Resume::Launcher);
        assert!(loaded.removed.is_empty());
        let raw = fs::read_to_string(&path).unwrap();
        assert!(
            !raw.contains("removed") && !raw.contains("lastUrls"),
            "empty fields are not written, so older builds see the file they know"
        );
        fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_leaves_no_tmp_file_behind() {
        let dir = temp_dir("atomic");
        let path = dir.join("instances.json");
        save(
            &path,
            &Record {
                instances: &[sample("a", "11111111-1111-1111-1111-111111111111")],
                last_view: None,
                removed: &[],
                last_urls: &BTreeMap::new(),
            },
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
        assert!(loaded.removed.is_empty());
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
        let again = load(&path);
        assert!(again.notice.is_none());
        assert!(again.removed.is_empty());
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
    fn a_file_from_an_older_build_has_no_pages_and_an_older_build_reads_ours() {
        let dir = temp_dir("pages");
        let path = dir.join("instances.json");
        let a = sample("a", "11111111-1111-1111-1111-111111111111");
        fs::write(
            &path,
            serde_json::json!({ "instances": [a], "lastView": "a" }).to_string(),
        )
        .unwrap();
        assert!(load(&path).last_urls.is_empty());
        // What an older build deserializes: the same struct without the field.
        #[derive(Deserialize)]
        #[serde(rename_all = "camelCase")]
        #[allow(dead_code)]
        struct OlderFile {
            instances: Vec<Instance>,
            last_view: Option<String>,
        }
        let urls = BTreeMap::from([("a".to_string(), "https://outcrop.example/x".to_string())]);
        save(
            &path,
            &Record {
                instances: &[sample("a", "1")],
                last_view: Some("a"),
                removed: &[],
                last_urls: &urls,
            },
        )
        .unwrap();
        let older: OlderFile = serde_json::from_str(&fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(older.instances.len(), 1);
        assert_eq!(older.last_view.as_deref(), Some("a"));
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
    fn the_sweep_deletes_only_recorded_removals() {
        let base = temp_dir("sweep");
        let root = base.join("instances");
        let kept = uuid::Uuid::new_v4().to_string();
        let back = uuid::Uuid::new_v4().to_string();
        let gone = uuid::Uuid::new_v4().to_string();
        let unrecorded = uuid::Uuid::new_v4().to_string();
        fs::create_dir_all(root.join(&kept).join("webview")).unwrap();
        fs::create_dir_all(root.join(&back).join("webview/storage")).unwrap();
        fs::create_dir_all(root.join(&unrecorded).join("webview")).unwrap();
        fs::create_dir_all(root.join("not-ours")).unwrap();
        fs::create_dir_all(base.join("escape-test")).unwrap();
        let removed = vec![
            back.clone(),
            gone.clone(),
            kept.clone(),
            "../escape-test".to_string(),
        ];
        let swept = sweep_removed(&root, &removed, &[sample(&kept, "1")]);
        // The re-created dir is deleted, and its removal stays pending until a
        // launch finds it gone; one already gone is dropped.
        assert_eq!(
            swept,
            Swept {
                deleted: vec![root.join(&back)],
                pending: vec![back.clone()],
            }
        );
        assert!(!root.join(&back).exists());
        assert!(root.join(&kept).exists(), "a registered instance's dir");
        assert!(root.join(&unrecorded).exists(), "no remove recorded it");
        assert!(root.join("not-ours").exists());
        assert!(base.join("escape-test").exists(), "not a uuid");
        let next = sweep_removed(&root, &swept.pending, &[sample(&kept, "1")]);
        assert_eq!(next, Swept::default());
        fs::remove_dir_all(&base).unwrap();
    }

    /// A damaged registry loses entries whose sessions are still on disk. No
    /// launch after the recovery may delete them — not the first, and not the
    /// next one, which reads the file the recovery rewrote.
    #[test]
    fn a_recovered_registry_never_costs_a_session_on_any_later_launch() {
        let dir = temp_dir("recovered");
        let path = dir.join("instances.json");
        let root = dir.join("instances");
        let lost = "11111111-2222-3333-4444-555555555555";
        fs::create_dir_all(root.join(lost).join("webview")).unwrap();
        fs::write(&path, r#"{"instances": ["#).unwrap();
        for launch in 1..=3 {
            let loaded = load(&path);
            let swept = sweep_removed(&root, &loaded.removed, &loaded.instances);
            assert!(swept.deleted.is_empty(), "launch {launch}");
            assert!(root.join(lost).exists(), "launch {launch}");
        }
        fs::remove_dir_all(&dir).unwrap();
    }
}
