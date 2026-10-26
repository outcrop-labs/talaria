// Plugins end to end against a migrated database, local git repositories
// (file://) and a mock app host: an agent's install request decided every
// way an admin can decide it, and what each decision leaves behind — the
// plugin record, the request row, and the line on the requesting ticket.
// `#[ignore]`d — they need a database — and CI's live-tests job runs them
// (`cargo test --workspace --lib -- --ignored`):
//
//   DATABASE_URL=postgres://… cargo test -p talaria-plugins --lib -- --ignored live
//
// The apps, data and builds directories are this process's environment, set
// once to a scratch root; every test owns its own slugs, repos and rows.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::{Value, json};
use sqlx::PgPool;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

use talaria_apps::AppHost;
use talaria_apps::lifecycle::Credentials;

use crate::install::{Decision, InstallAsk, decide, file_request};

pub(crate) fn random_hex() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let n = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    format!("{:x}", n ^ (std::process::id() as u128) << 64)
        .chars()
        .rev()
        .take(10)
        .collect()
}

/// The scratch root, set as the apps, data and builds directories (and local
/// sources allowed) the first time a test asks.
pub(crate) fn root() -> &'static Path {
    static ROOT: OnceLock<PathBuf> = OnceLock::new();
    ROOT.get_or_init(|| {
        let root = std::env::temp_dir().join(format!("talaria-plugins-live-{}", random_hex()));
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

/// Each test's start: one test at a time (they share the apps directory),
/// the scratch root in place, and a pool on the migrated database.
pub(crate) async fn setup() -> (tokio::sync::MutexGuard<'static, ()>, PgPool) {
    static SERIAL: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    root();
    let guard = SERIAL
        .get_or_init(|| tokio::sync::Mutex::new(()))
        .lock()
        .await;
    let url = std::env::var("DATABASE_URL")
        .expect("set DATABASE_URL (a migrated database) to run the ignored live tests");
    (guard, PgPool::connect(&url).await.expect("connect"))
}

pub(crate) fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .args(args)
        .current_dir(dir)
        .env("GIT_AUTHOR_NAME", "Live")
        .env("GIT_AUTHOR_EMAIL", "live@example.invalid")
        .env("GIT_COMMITTER_NAME", "Live")
        .env("GIT_COMMITTER_EMAIL", "live@example.invalid")
        .output()
        .expect("git runs");
    assert!(
        out.status.success(),
        "git {args:?}: {}",
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

/// A bare repository and a work tree that pushes to it.
pub(crate) struct Repo {
    pub dir: PathBuf,
    pub work: PathBuf,
    pub url: String,
}

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

impl Repo {
    pub fn new(name: &str) -> Self {
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

    /// Commit `files` (name → contents), push, answer the sha.
    pub fn commit(&self, files: &[(&str, String)], message: &str) -> String {
        for (name, body) in files {
            std::fs::write(self.work.join(name), body).unwrap();
        }
        git(&self.work, &["add", "."]);
        git(&self.work, &["commit", "--quiet", "-m", message]);
        git(&self.work, &["push", "--quiet", "origin", "main"]);
        git(&self.work, &["rev-parse", "HEAD"])
    }
}

/// A mock app host: every step it is asked, and a fixed `{ ok: true }`.
pub(crate) async fn mock_host() -> (AppHost, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let log = log.clone();
            tokio::spawn(async move {
                let mut buf = vec![0u8; 8192];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).to_string();
                let path = head
                    .lines()
                    .next()
                    .and_then(|l| l.split(' ').nth(1))
                    .unwrap_or_default()
                    .to_string();
                let step = if path.ends_with("/rebuild") {
                    "rebuild".to_string()
                } else if let Some(i) = head.find("\"step\":\"") {
                    head[i + 8..].split('"').next().unwrap_or("").to_string()
                } else {
                    path.clone()
                };
                log.lock().unwrap().push(step);
                let body = json!({ "ok": true, "build": { "status": "ready" } }).to_string();
                let reply = format!(
                    "HTTP/1.1 200 OK\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(reply.as_bytes()).await;
            });
        }
    });
    (AppHost::at(base, "internal-test".into()), seen)
}

/// No GitHub: every fetch here is a file:// repository.
pub(crate) struct NoCreds;

impl Credentials for NoCreds {
    async fn github_read_token(&self, _repo: &str) -> Result<Option<String>, String> {
        Ok(None)
    }
    async fn secret_login(&self, _name: &str, _host: &str) -> Result<(String, String), String> {
        Err("no secrets in this test".into())
    }
}

/// A user, a board, a ticket assigned to that user, and a Developer Agent.
pub(crate) struct World {
    pub tag: String,
    pub user_id: String,
    pub board_id: String,
    pub task_id: String,
    pub agent_id: String,
    pub agent_model: String,
}

impl World {
    pub async fn new(pg: &PgPool, tag: &str) -> Self {
        let tag = format!("{tag}-{}", random_hex());
        let user_id: String = sqlx::query_scalar(
            "insert into users (sub, name) values ($1, 'Plugin Owner') returning id::text",
        )
        .bind(format!("plugins-live-{tag}"))
        .fetch_one(pg)
        .await
        .unwrap();
        let board_id: String = sqlx::query_scalar(
            "insert into boards (name, owner_id) values ($1, $2::uuid) returning id::text",
        )
        .bind(format!("Plugins {tag}"))
        .bind(&user_id)
        .fetch_one(pg)
        .await
        .unwrap();
        let task_id: String = sqlx::query_scalar(
            "insert into tasks (board_id, title, assignees, ticket_no) \
             values ($1::uuid, 'Build the notes app', $2, 1) returning id::text",
        )
        .bind(&board_id)
        .bind(json!([format!("user:{user_id}")]))
        .fetch_one(pg)
        .await
        .unwrap();
        let agent_model = format!("plugins-live-{tag}");
        let agent_id: String = sqlx::query_scalar(
            "insert into agent_defs (slug, department, model, display_name, developer) \
             values ($1, 'plugins-live', $1, 'Plugin Dev', true) returning id::text",
        )
        .bind(&agent_model)
        .fetch_one(pg)
        .await
        .unwrap();
        Self {
            tag,
            user_id,
            board_id,
            task_id,
            agent_id,
            agent_model,
        }
    }

    pub fn ask<'a>(&'a self, repo: &'a str) -> InstallAsk<'a> {
        InstallAsk {
            agent_id: &self.agent_id,
            agent_model: &self.agent_model,
            repo,
            git_ref: None,
            why: "the notes app the owner asked for",
            task_id: &self.task_id,
            board_id: &self.board_id,
            owner_user_id: Some(&self.user_id),
        }
    }

    /// The workbench lines on the ticket, oldest first.
    pub async fn lines(&self, pg: &PgPool) -> Vec<String> {
        sqlx::query_scalar(
            "select description from task_activity where task_id = $1::uuid \
             and type = 'workbench' order by created_at, id",
        )
        .bind(&self.task_id)
        .fetch_all(pg)
        .await
        .unwrap()
    }
}

/// An app repository whose manifest declares `slug`.
pub(crate) fn app_repo(slug: &str) -> (Repo, String) {
    let repo = Repo::new(slug);
    let sha = repo.commit(
        &[(
            "talaria.json",
            json!({ "name": "Plugin Live", "slug": slug, "sdk": "^1.0" }).to_string(),
        )],
        "the app",
    );
    (repo, sha)
}

/// Point a filed request at a local repository: the tool files GitHub URLs,
/// and a test has no GitHub.
async fn point_at(pg: &PgPool, id: &str, url: &str) {
    sqlx::query("update app_install_requests set url = $2 where id = $1::uuid")
        .bind(id)
        .bind(url)
        .execute(pg)
        .await
        .unwrap();
}

#[tokio::test]
#[ignore = "needs a live database (DATABASE_URL)"]
async fn an_approved_request_installs_records_the_plugin_and_says_so_on_the_ticket() {
    let (_serial, pg) = setup().await;
    let w = World::new(&pg, "approve").await;
    let slug = format!("pl-{}", w.tag);
    let (repo, sha) = app_repo(&slug);
    let gh_repo = format!("acme/{slug}");

    // The ticket's person: its first human assignee.
    assert_eq!(
        crate::install::ticket_owner(&pg, &w.task_id)
            .await
            .as_deref(),
        Some(w.user_id.as_str())
    );
    // The agent's request, then the refusals a second one meets.
    let id = file_request(&pg, &w.ask(&gh_repo)).await.expect("filed");
    let again = file_request(&pg, &w.ask(&gh_repo)).await.unwrap_err();
    assert!(again.contains("already have a pending"), "{again}");
    let bad = file_request(&pg, &w.ask("not-a-repo")).await.unwrap_err();
    assert!(bad.contains("owner/name"), "{bad}");
    point_at(&pg, &id, &repo.url).await;

    let (host, steps) = mock_host().await;
    let decided = decide(
        &pg,
        &NoCreds,
        &host,
        &id,
        Decision::Approve { kept_data: None },
        None,
        "admin@example.invalid",
    )
    .await
    .expect("decided");
    assert_eq!(decided.status, "approved");
    assert_eq!(decided.slug.as_deref(), Some(slug.as_str()));
    assert_eq!(decided.sha.as_deref(), Some(sha.as_str()));
    assert_eq!(
        *steps.lock().unwrap(),
        ["rebuild"],
        "the install asked for its build"
    );
    assert!(decided.warning.is_none(), "{:?}", decided.warning);

    // The plugin: the requesting agent maintains it, the ticket's person
    // owns it, the ticket's board takes its tickets; the source is the
    // app's own record, read through the join.
    let p = crate::plugin(&pg, &slug).await.unwrap().expect("a plugin");
    assert_eq!(p.maintainer_agent_id.as_deref(), Some(w.agent_id.as_str()));
    assert_eq!(p.maintainer_model.as_deref(), Some(w.agent_model.as_str()));
    assert_eq!(p.owner_user_id.as_deref(), Some(w.user_id.as_str()));
    assert_eq!(p.board_id.as_deref(), Some(w.board_id.as_str()));
    assert_eq!(p.task_id.as_deref(), Some(w.task_id.as_str()));
    assert_eq!(p.installed_commit.as_deref(), Some(sha.as_str()));
    assert_eq!(p.url, repo.url);
    assert_eq!(p.update_policy, "approve");
    assert_eq!(p.git_host_id, None, "a file:// source names no git host");
    let mine = crate::plugins_maintained_by(&pg, &w.agent_id)
        .await
        .unwrap();
    assert_eq!(mine.len(), 1);

    // The request row and the ticket line.
    let (status, result, app_slug): (String, Option<String>, Option<String>) = sqlx::query_as(
        "select status, result, app_slug from app_install_requests where id = $1::uuid",
    )
    .bind(&id)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert_eq!(status, "approved");
    assert_eq!(app_slug.as_deref(), Some(slug.as_str()));
    let line = format!(
        "approved installing {gh_repo} — installed as \"{slug}\" at {}; it built. It is not enabled yet: an admin enables it and grants its surfaces in Manage → Apps",
        &sha[..7]
    );
    assert_eq!(result.as_deref(), Some(line.as_str()));
    assert_eq!(w.lines(&pg).await, [line]);

    // Installed now: a further request for the repo is refused.
    sqlx::query("update app_sources set host = 'github.com', repo = $2 where slug = $1")
        .bind(&slug)
        .bind(&gh_repo)
        .execute(&pg)
        .await
        .unwrap();
    let installed = file_request(&pg, &w.ask(&gh_repo)).await.unwrap_err();
    assert!(installed.contains("already installed"), "{installed}");

    // The admin's update policy.
    assert_eq!(
        crate::set_update_policy(&pg, &slug, "auto").await.unwrap(),
        Some("approve".into())
    );
    assert!(
        crate::set_update_policy(&pg, &slug, "sometimes")
            .await
            .is_err()
    );
    assert_eq!(
        crate::set_update_policy(&pg, "no-such-app", "auto")
            .await
            .unwrap(),
        None
    );

    // Deciding again finds nothing pending.
    assert!(matches!(
        decide(&pg, &NoCreds, &host, &id, Decision::Reject, None, "a").await,
        Err(crate::install::DecideError::NotFound)
    ));
    // Uninstalling the app's source takes the plugin with it.
    sqlx::query("delete from app_sources where slug = $1")
        .bind(&slug)
        .execute(&pg)
        .await
        .unwrap();
    assert!(crate::plugin(&pg, &slug).await.unwrap().is_none());
    let _ = std::fs::remove_dir_all(talaria_users::apps_dir().join(&slug));
}

#[tokio::test]
#[ignore = "needs a live database (DATABASE_URL)"]
async fn a_failed_install_and_a_rejection_each_leave_their_own_line() {
    let (_serial, pg) = setup().await;
    let w = World::new(&pg, "fail").await;
    // Not an app: no talaria.json.
    let repo = Repo::new("not-an-app");
    repo.commit(&[("README.md", "hello".to_string())], "readme");
    let id = file_request(&pg, &w.ask("acme/not-an-app")).await.unwrap();
    point_at(&pg, &id, &repo.url).await;
    let (host, steps) = mock_host().await;
    let decided = decide(
        &pg,
        &NoCreds,
        &host,
        &id,
        Decision::Approve { kept_data: None },
        Some("trying it"),
        "admin@example.invalid",
    )
    .await
    .unwrap();
    assert_eq!(decided.status, "failed");
    assert!(steps.lock().unwrap().is_empty(), "nothing was built");
    assert_eq!(
        decided.line,
        "approved installing acme/not-an-app, but the install failed: that repository is not a Talaria app (no talaria.json at its root) — trying it"
    );

    let id = file_request(&pg, &w.ask("acme/other")).await.unwrap();
    let decided = decide(
        &pg,
        &NoCreds,
        &host,
        &id,
        Decision::Reject,
        Some("not this quarter"),
        "admin@example.invalid",
    )
    .await
    .unwrap();
    assert_eq!(decided.status, "rejected");
    let lines = w.lines(&pg).await;
    assert_eq!(lines.len(), 2);
    assert_eq!(
        lines[1],
        format!(
            "rejected installing acme/other that {} asked for — not this quarter",
            w.agent_model
        )
    );
    let requests = crate::install::requests_by_agent(&pg, &w.agent_id)
        .await
        .unwrap();
    let statuses: Vec<&str> = requests
        .iter()
        .map(|r| r["status"].as_str().unwrap())
        .collect::<Vec<_>>();
    assert_eq!(statuses, ["rejected", "failed"]);
    assert_eq!(
        requests[0]["note"],
        Value::String("not this quarter".into())
    );
}
