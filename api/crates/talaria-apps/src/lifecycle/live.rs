// The lifecycle sequences end to end: install, update (fetch → stage →
// exchange → activate → keep the previous version, or swap back) and
// uninstall (keep or wipe the data), against a migrated database, a mock app
// host that answers each step as a test scripts it, and local git
// repositories (file://). The record in app_sources and the trees on disk are
// asserted after every step. `#[ignore]`d — they need a database — and CI's
// live-tests job runs them (`cargo test --workspace --lib -- --ignored`):
//
//   DATABASE_URL=postgres://… cargo test -p talaria-apps --lib -- --ignored lifecycle::live
//
// The apps, data and builds directories are this process's environment, set
// once to a scratch root; every test owns its own slugs and repositories.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use super::*;

#[path = "../../tests/support/git.rs"]
mod support_git;
use support_git::git;

/// The scratch root, set as the apps, data and builds directories (and
/// local sources allowed) the first time a test asks.
fn root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let root = std::env::temp_dir().join(format!("talaria-lifecycle-live-{}", random_hex()));
        std::fs::create_dir_all(&root).unwrap();
        // SAFETY: set once, before any test of this module reads them, and
        // every test of this module asks for the root first (`serial`). Only
        // these ignored tests set them: the default run never does.
        unsafe {
            std::env::set_var("TALARIA_APPS_DIR", root.join("apps"));
            std::env::set_var("TALARIA_APP_DATA_DIR", root.join("app-data"));
            std::env::set_var("TALARIA_APP_BUILDS_DIR", root.join("app-builds"));
            std::env::set_var("TALARIA_APPS_LOCAL_SOURCES", "1");
        }
        root
    })
}

/// One test of this module at a time. They share the apps directory, and an
/// install's staging tree (`apps/.install-<random>`) names no slug: a test
/// asserting that its own refused install left none behind would otherwise
/// see another test's install in flight. Taken first; it sets the root.
async fn serial() -> tokio::sync::MutexGuard<'static, ()> {
    static SERIAL: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    root();
    SERIAL
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await
}

async fn pg() -> PgPool {
    let url = std::env::var("DATABASE_URL")
        .expect("set DATABASE_URL (a migrated database) to run the ignored live tests");
    PgPool::connect(&url).await.expect("connect")
}

fn slug(tag: &str) -> String {
    format!("live-{tag}-{}", &random_hex()[..6])
}

// ── Local repositories ───────────────────────────────────────────────────────

/// A bare repository and a work tree that pushes to it, removed when
/// dropped.
struct Repo {
    dir: PathBuf,
    work: PathBuf,
    url: String,
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Repo {
    fn new(name: &str) -> Self {
        let dir = root()
            .join("repos")
            .join(format!("{name}-{}", random_hex()));
        let work = dir.join("work");
        std::fs::create_dir_all(&work).unwrap();
        git(
            &dir,
            &["init", "--quiet", "--bare", "-b", "main", "app.git"],
        );
        git(&work, &["init", "--quiet", "-b", "main"]);
        git(
            &work,
            &[
                "remote",
                "add",
                "origin",
                dir.join("app.git").to_str().unwrap(),
            ],
        );
        Self {
            url: format!("file://{}", dir.join("app.git").display()),
            dir,
            work,
        }
    }

    /// Commit an app whose `version` file says `v`, push it, answer its sha.
    fn commit(&self, v: &str) -> String {
        std::fs::write(
            self.work.join("talaria.json"),
            json!({ "name": "Live", "sdk": "^1.0" }).to_string(),
        )
        .unwrap();
        std::fs::write(self.work.join("version"), v).unwrap();
        git(&self.work, &["add", "."]);
        git(&self.work, &["commit", "--quiet", "-m", v]);
        git(&self.work, &["push", "--quiet", "origin", "main"]);
        git(&self.work, &["rev-parse", "HEAD"])
    }
}

/// The `version` file in a tree, or None when the tree is not there.
fn version_at(dir: &Path) -> Option<String> {
    std::fs::read_to_string(dir.join("version")).ok()
}

// ── The mock app host ────────────────────────────────────────────────────────

/// How the mock host answers, and every step it was asked, as `step`
/// (`rebuild` for a build).
#[derive(Default)]
struct Script {
    stage_fails: bool,
    /// The stage call fails as a call (a 500), not as an answer.
    stage_errors: bool,
    activate_fails: bool,
    /// Run when activate is asked, before it answers.
    on_activate: Option<Box<dyn Fn() + Send + Sync>>,
    asked: Vec<String>,
}

type Shared = Arc<Mutex<Script>>;

async fn mock_host(script: Shared) -> AppHost {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                return;
            };
            let script = script.clone();
            tokio::spawn(async move {
                let _ = answer(&mut sock, &script).await;
            });
        }
    });
    AppHost::at(format!("http://127.0.0.1:{port}"), "test-token".into())
}

async fn answer(sock: &mut tokio::net::TcpStream, script: &Shared) -> std::io::Result<()> {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 4096];
    let head_end = loop {
        let n = sock.read(&mut chunk).await?;
        if n == 0 {
            return Ok(());
        }
        buf.extend_from_slice(&chunk[..n]);
        if let Some(i) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
            break i;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).to_ascii_lowercase();
    let len = head
        .lines()
        .find_map(|l| l.strip_prefix("content-length:"))
        .and_then(|v| v.trim().parse::<usize>().ok())
        .unwrap_or(0);
    let mut body = buf[head_end + 4..].to_vec();
    while body.len() < len {
        let n = sock.read(&mut chunk).await?;
        if n == 0 {
            break;
        }
        body.extend_from_slice(&chunk[..n]);
    }
    let path = head.split(' ').nth(1).unwrap_or("").to_string();
    let step = if path.ends_with("/rebuild") {
        "rebuild".to_string()
    } else {
        serde_json::from_slice::<Value>(&body)
            .ok()
            .and_then(|v| v.get("step").and_then(Value::as_str).map(String::from))
            .unwrap_or_default()
    };
    let (status, reply) = {
        let mut s = script.lock().unwrap();
        s.asked.push(step.clone());
        match step.as_str() {
            "stage" if s.stage_errors => (500, json!({ "error": "the host fell over" })),
            "stage" if s.stage_fails => (200, json!({ "ok": false, "error": "SyntaxError" })),
            "activate" => {
                if let Some(f) = &s.on_activate {
                    f();
                }
                if s.activate_fails {
                    (200, json!({ "ok": false, "error": "it threw on load" }))
                } else {
                    (200, json!({ "ok": true, "build": { "key": "new" } }))
                }
            }
            _ => (200, json!({ "ok": true, "build": { "key": "k" } })),
        }
    };
    let text = reply.to_string();
    let msg = format!(
        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{text}",
        text.len()
    );
    sock.write_all(msg.as_bytes()).await
}

fn asked(script: &Shared) -> Vec<String> {
    std::mem::take(&mut script.lock().unwrap().asked)
}

/// Credentials that hand out nothing: every source here is a local one.
struct NoCreds;

impl Credentials for NoCreds {
    async fn host_read_login(
        &self,
        _src: &crate::source::GitSource,
    ) -> Result<Option<(String, String)>, String> {
        Ok(None)
    }

    async fn secret_login(&self, _name: &str, _host: &str) -> Result<(String, String), String> {
        Err("no secret here".into())
    }
}

fn request<'a>(url: &'a str, slug: &'a str, kept: Option<KeptDataChoice>) -> InstallRequest<'a> {
    InstallRequest {
        url,
        slug: Some(slug),
        git_ref: None,
        secret: None,
        kept_data: kept,
        actor: "live-test",
    }
}

/// (sha, previous_sha) as app_sources records them.
async fn recorded(pg: &PgPool, slug: &str) -> Option<(Option<String>, Option<String>)> {
    sqlx::query_as("select sha, previous_sha from app_sources where slug = $1")
        .bind(slug)
        .fetch_optional(pg)
        .await
        .unwrap()
}

async fn forget(pg: &PgPool, slug: &str) {
    sqlx::query("delete from app_sources where slug = $1")
        .bind(slug)
        .execute(pg)
        .await
        .unwrap();
}

/// No staging tree an install or update left behind.
fn no_leftovers() -> bool {
    std::fs::read_dir(apps_dir())
        .map(|d| {
            d.flatten()
                .all(|e| !e.file_name().to_string_lossy().starts_with(".install-"))
        })
        .unwrap_or(true)
}

// ── The sequences ────────────────────────────────────────────────────────────

/// An update lands, a failed one rolls back, and apps/.<slug>.prev holds what
/// the record's previous_sha names after each — a failed update never
/// destroys it. A swap back that fails is reported as not rolled back, and
/// the record follows the tree.
#[tokio::test]
#[ignore = "needs a migrated Postgres (DATABASE_URL)"]
async fn an_update_lands_or_rolls_back_and_the_record_follows_the_trees() {
    let _serial = serial().await;
    let pg = pg().await;
    let slug = slug("update");
    let repo = Repo::new("update");
    let script: Shared = Arc::default();
    let host = mock_host(script.clone()).await;
    let (live, next, prev) = (apps_dir().join(&slug), next_dir(&slug), prev_dir(&slug));

    let v1 = repo.commit("v1");
    let installed = install_app(&pg, &NoCreds, Some(&host), request(&repo.url, &slug, None))
        .await
        .expect("install");
    assert_eq!(installed.sha, v1);
    assert_eq!(asked(&script), ["rebuild"]);
    assert_eq!(recorded(&pg, &slug).await, Some((Some(v1.clone()), None)));

    // Nothing new: up to date, nothing staged.
    let same = update_app(&pg, &NoCreds, &host, &slug, None, "live-test")
        .await
        .unwrap();
    assert!(same.ok && same.up_to_date);
    assert!(asked(&script).is_empty());
    assert!(!next.exists());

    // v2 lands: live holds it, .prev holds v1, and the record says both.
    let v2 = repo.commit("v2");
    let up = update_app(&pg, &NoCreds, &host, &slug, None, "live-test")
        .await
        .unwrap();
    assert!(up.ok && !up.rolled_back, "{up:?}");
    assert_eq!(
        (up.from.as_deref(), up.to.as_deref()),
        (Some(v1.as_str()), Some(v2.as_str()))
    );
    assert_eq!(asked(&script), ["stage", "activate"]);
    assert_eq!(version_at(&live).as_deref(), Some("v2"));
    assert_eq!(version_at(&prev).as_deref(), Some("v1"));
    assert!(!next.exists());
    assert_eq!(
        recorded(&pg, &slug).await,
        Some((Some(v2.clone()), Some(v1.clone())))
    );

    // v3 fails to build: nothing swapped, nothing kept.
    let v3 = repo.commit("v3");
    script.lock().unwrap().stage_fails = true;
    let built = update_app(&pg, &NoCreds, &host, &slug, None, "live-test")
        .await
        .unwrap();
    assert!(!built.ok && built.rolled_back, "{built:?}");
    assert_eq!(asked(&script), ["stage"]);
    assert_eq!(version_at(&live).as_deref(), Some("v2"));
    assert_eq!(version_at(&prev).as_deref(), Some("v1"));
    assert!(!next.exists());

    // The stage call itself fails: the host is told to release the app (and
    // discard what it staged), and nothing is swapped.
    {
        let mut s = script.lock().unwrap();
        s.stage_fails = false;
        s.stage_errors = true;
    }
    let errored = update_app(&pg, &NoCreds, &host, &slug, None, "live-test")
        .await
        .unwrap();
    assert!(!errored.ok && errored.rolled_back, "{errored:?}");
    assert_eq!(asked(&script), ["stage", "release"]);
    assert_eq!(version_at(&live).as_deref(), Some("v2"));
    assert!(!next.exists());

    // v3 builds but fails to load once live: swapped back, and .prev still
    // holds v1 — the version the record's previous_sha names.
    {
        let mut s = script.lock().unwrap();
        s.stage_errors = false;
        s.activate_fails = true;
    }
    let loaded = update_app(&pg, &NoCreds, &host, &slug, None, "live-test")
        .await
        .unwrap();
    assert!(!loaded.ok && loaded.rolled_back, "{loaded:?}");
    assert!(
        loaded
            .error
            .as_deref()
            .unwrap()
            .contains("it threw on load")
    );
    assert_eq!(asked(&script), ["stage", "activate", "rebuild"]);
    assert_eq!(version_at(&live).as_deref(), Some("v2"));
    assert_eq!(version_at(&prev).as_deref(), Some("v1"));
    assert!(!next.exists());
    assert_eq!(
        recorded(&pg, &slug).await,
        Some((Some(v2.clone()), Some(v1.clone())))
    );

    // v3 fails to load, and the swap back fails too (the tree it would swap
    // back is gone): not rolled back, apps/<slug> holds v3, and the record
    // says so. .prev was left alone, so it still names v1.
    let gone = next.clone();
    script.lock().unwrap().on_activate = Some(Box::new(move || {
        let _ = std::fs::remove_dir_all(&gone);
    }));
    let stuck = update_app(&pg, &NoCreds, &host, &slug, None, "live-test")
        .await
        .unwrap();
    assert!(!stuck.ok && !stuck.rolled_back, "{stuck:?}");
    assert!(
        stuck
            .error
            .as_deref()
            .unwrap()
            .contains("swapping the previous one back failed too"),
        "{stuck:?}"
    );
    assert_eq!(asked(&script), ["stage", "activate"]);
    assert_eq!(version_at(&live).as_deref(), Some("v3"));
    assert_eq!(version_at(&prev).as_deref(), Some("v1"));
    assert_eq!(
        recorded(&pg, &slug).await,
        Some((Some(v3.clone()), Some(v1.clone())))
    );

    let left = uninstall_app(&pg, &SecretBox::default(), Some(&host), &slug, false)
        .await
        .unwrap();
    assert_eq!(left, DataLeft::None);
    assert_eq!(asked(&script), ["removed"]);
    for dir in [&live, &next, &prev] {
        assert!(!dir.exists(), "{} was left", dir.display());
    }
    assert_eq!(recorded(&pg, &slug).await, None);
    assert!(!app_data_dir().join(&slug).exists());
}

/// Kept data goes back only to the repository it was kept from. Another
/// repository's install under the same slug is refused with both named, and
/// takes the data, or deletes it, only when told. Uninstall keeps the record
/// with the data, and writes it for an app installed before install did.
#[tokio::test]
#[ignore = "needs a migrated Postgres (DATABASE_URL)"]
async fn kept_data_goes_back_only_to_its_own_repository() {
    let _serial = serial().await;
    let pg = pg().await;
    let sb = SecretBox::default();
    let slug = slug("kept");
    let (a, b) = (Repo::new("kept-a"), Repo::new("kept-b"));
    a.commit("a1");
    b.commit("b1");
    let script: Shared = Arc::default();
    let host = mock_host(script.clone()).await;
    let data = app_data_dir().join(&slug);
    let a_url = parse_source(&a.url, true).unwrap().url;
    let b_url = parse_source(&b.url, true).unwrap().url;
    let owner = || read_data_source(&data).map(|d| d.url);
    // What the app host does on the app's first store call.
    let store = || std::fs::create_dir_all(data.join("pg")).unwrap();

    let first = install_app(&pg, &NoCreds, Some(&host), request(&a.url, &slug, None))
        .await
        .unwrap();
    assert!(!first.kept_data && !first.wiped_data);
    assert_eq!(owner(), Some(a_url.clone()));
    store();
    assert_eq!(
        uninstall_app(&pg, &sb, Some(&host), &slug, false)
            .await
            .unwrap(),
        DataLeft::Kept
    );
    assert_eq!(
        owner(),
        Some(a_url.clone()),
        "the record stays with the data"
    );

    // The same repository: adopted, as a reinstall expects.
    let again = install_app(&pg, &NoCreds, Some(&host), request(&a.url, &slug, None))
        .await
        .unwrap();
    assert!(again.kept_data && !again.wiped_data);
    uninstall_app(&pg, &sb, Some(&host), &slug, false)
        .await
        .unwrap();

    // Another repository under the same slug: refused, both named, and
    // nothing placed or recorded.
    let refused = install_app(&pg, &NoCreds, Some(&host), request(&b.url, &slug, None))
        .await
        .unwrap_err();
    assert_eq!(
        refused.kept_data,
        Some(KeptDataConflict {
            slug: slug.clone(),
            kept_from: Some(a_url.clone()),
            installing: b_url.clone(),
        })
    );
    assert!(refused.error.contains(&a_url) && refused.error.contains(&b_url));
    assert!(!apps_dir().join(&slug).exists());
    assert!(no_leftovers());
    assert_eq!(recorded(&pg, &slug).await, None);
    assert!(data.join("pg").exists(), "a refusal touches no data");

    // Told to adopt: it does, and the data is the new repository's now.
    let adopted = install_app(
        &pg,
        &NoCreds,
        Some(&host),
        request(&b.url, &slug, Some(KeptDataChoice::Adopt)),
    )
    .await
    .unwrap();
    assert!(adopted.kept_data);
    assert_eq!(owner(), Some(b_url.clone()));

    // An app installed before install wrote the record: uninstall writes it
    // from the source record, so the same repository still gets its data.
    std::fs::remove_file(data.join(DATA_SOURCE_FILE)).unwrap();
    uninstall_app(&pg, &sb, Some(&host), &slug, false)
        .await
        .unwrap();
    assert_eq!(owner(), Some(b_url.clone()));

    // Data no install recorded: even a reinstall must choose. Told to wipe,
    // the old data is gone and the new app starts empty.
    std::fs::remove_file(data.join(DATA_SOURCE_FILE)).unwrap();
    let unrecorded = install_app(&pg, &NoCreds, Some(&host), request(&b.url, &slug, None))
        .await
        .unwrap_err();
    assert_eq!(unrecorded.kept_data.unwrap().kept_from, None);
    let wiped = install_app(
        &pg,
        &NoCreds,
        Some(&host),
        request(&b.url, &slug, Some(KeptDataChoice::Wipe)),
    )
    .await
    .unwrap();
    assert!(wiped.wiped_data && !wiped.kept_data);
    assert!(!data.join("pg").exists());
    assert_eq!(owner(), Some(b_url.clone()));

    store();
    assert_eq!(
        uninstall_app(&pg, &sb, Some(&host), &slug, true)
            .await
            .unwrap(),
        DataLeft::Wiped
    );
    assert!(!data.exists());
}

/// An install holds its slug like an update and an uninstall do: one racing
/// another lifecycle step of the same slug is refused, and leaves nothing.
#[tokio::test]
#[ignore = "needs a migrated Postgres (DATABASE_URL)"]
async fn an_install_holds_its_slug() {
    let _serial = serial().await;
    let pg = pg().await;
    let slug = slug("lock");
    let repo = Repo::new("lock");
    repo.commit("v1");
    let script: Shared = Arc::default();
    let host = mock_host(script.clone()).await;

    let held = SlugLock::take(&slug).unwrap();
    let err = install_app(&pg, &NoCreds, Some(&host), request(&repo.url, &slug, None))
        .await
        .unwrap_err();
    assert!(
        err.error
            .contains("already being installed, updated or removed"),
        "{err}"
    );
    assert!(!apps_dir().join(&slug).exists());
    assert!(no_leftovers());
    assert_eq!(recorded(&pg, &slug).await, None);
    assert!(asked(&script).is_empty());
    drop(held);

    install_app(&pg, &NoCreds, Some(&host), request(&repo.url, &slug, None))
        .await
        .expect("the slug is free again");
    // The install let go of the slug when it finished.
    drop(SlugLock::take(&slug).expect("released"));
    // And an uninstall racing an update (or an install) waits its turn.
    let held = SlugLock::take(&slug).unwrap();
    let err = uninstall_app(&pg, &SecretBox::default(), Some(&host), &slug, true)
        .await
        .unwrap_err();
    assert!(
        err.contains("already being installed, updated or removed"),
        "{err}"
    );
    drop(held);
    uninstall_app(&pg, &SecretBox::default(), Some(&host), &slug, true)
        .await
        .unwrap();
    forget(&pg, &slug).await;
}

/// The update check asks a public source and reports a newer commit.
#[tokio::test]
#[ignore = "sets this process's apps directories, as the live tests do"]
async fn a_passive_check_reads_a_public_source() {
    let _serial = serial().await;
    let repo = Repo::new("check");
    let v1 = repo.commit("v1");
    let src = parse_source(&repo.url, true).unwrap();
    let rec = SourceRecord {
        url: src.url,
        host: src.host,
        repo: src.repo,
        git_ref: None,
        sha: Some(v1.clone()),
        previous_sha: None,
        secret: None,
        installed_at: None,
        updated_at: None,
    };
    let check = check_one(&NoCreds, &slug("check"), &rec).await;
    assert_eq!(
        (check.available, check.latest.as_deref()),
        (false, Some(v1.as_str()))
    );
    let v2 = repo.commit("v2");
    let check = check_one(&NoCreds, &slug("check"), &rec).await;
    assert!(check.available && check.not_checked.is_none());
    assert_eq!(check.latest.as_deref(), Some(v2.as_str()));
}
