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

/// No git host: every fetch here is a file:// repository.
pub(crate) struct NoCreds;

impl Credentials for NoCreds {
    async fn host_read_login(
        &self,
        _src: &talaria_apps::source::GitSource,
    ) -> Result<Option<(String, String)>, String> {
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
            "insert into users (sub, name, role) values ($1, 'Plugin Owner', 'admin') returning id::text",
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

    /// A request for `repo` on the `github` host, filed at `url`.
    pub fn ask<'a>(&'a self, repo: &'a str, url: &'a str) -> InstallAsk<'a> {
        InstallAsk {
            agent_id: &self.agent_id,
            agent_model: &self.agent_model,
            host_id: "github",
            repo,
            url,
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

/// Point a filed request at a local repository: the tool files the repo's
/// page on its host, and a test has no host to fetch from.
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
    let gh_url = format!("https://github.com/{gh_repo}");

    // The ticket's person: its first human assignee.
    assert_eq!(
        crate::install::ticket_owner(&pg, &w.task_id)
            .await
            .as_deref(),
        Some(w.user_id.as_str())
    );
    // The agent's request, then the refusals a second one meets.
    let id = file_request(&pg, &w.ask(&gh_repo, &gh_url))
        .await
        .expect("filed");
    let again = file_request(&pg, &w.ask(&gh_repo, &gh_url))
        .await
        .unwrap_err();
    assert!(again.contains("already have a pending"), "{again}");
    let bad = file_request(&pg, &w.ask("not-a-repo", "https://github.com/not-a-repo"))
        .await
        .unwrap_err();
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
    assert_eq!(
        p.git_host_id.as_deref(),
        Some("github"),
        "the plugin is on the host its request named"
    );
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
    let installed = file_request(&pg, &w.ask(&gh_repo, &gh_url))
        .await
        .unwrap_err();
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
    let id = file_request(
        &pg,
        &w.ask("acme/not-an-app", "https://github.com/acme/not-an-app"),
    )
    .await
    .unwrap();
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

    let id = file_request(&pg, &w.ask("acme/other", "https://github.com/acme/other"))
        .await
        .unwrap();
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

// ── The upkeep pass ──────────────────────────────────────────────────────────

/// A loopback GitHub that answers GETs by path from `route`, and records
/// every request as `METHOD path`. The sweep only reads, so anything but a
/// GET is answered 500 and recorded for the test to refuse.
async fn github_stub(
    route: Arc<dyn Fn(&str) -> (u16, String) + Send + Sync>,
) -> (String, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
    let seen = Arc::new(Mutex::new(Vec::<String>::new()));
    let log = seen.clone();
    tokio::spawn(async move {
        while let Ok((mut sock, _)) = listener.accept().await {
            let (log, route) = (log.clone(), route.clone());
            tokio::spawn(async move {
                let mut buf = vec![0u8; 16384];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                let head = String::from_utf8_lossy(&buf[..n]).to_string();
                let mut words = head.lines().next().unwrap_or_default().split(' ');
                let method = words.next().unwrap_or_default().to_string();
                let path = words.next().unwrap_or_default().to_string();
                log.lock().unwrap().push(format!("{method} {path}"));
                let (status, body) = if method == "GET" {
                    route(&path)
                } else {
                    (500, r#"{"message":"the sweep only reads"}"#.to_string())
                };
                let reply = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(reply.as_bytes()).await;
            });
        }
    });
    (base, seen)
}

/// The ticket room's comments that start with `prefix`.
async fn comments(pg: &PgPool, task_id: &str, prefix: &str) -> Vec<String> {
    sqlx::query_scalar(
        "select cm.content from channel_messages cm join channels c on c.id = cm.channel_id \
         where c.task_id = $1::uuid and starts_with(cm.content, $2) order by cm.seq",
    )
    .bind(task_id)
    .bind(prefix)
    .fetch_all(pg)
    .await
    .unwrap()
}

#[tokio::test]
#[ignore = "needs a live database (DATABASE_URL)"]
async fn the_upkeep_pass_reads_prs_raises_the_update_and_files_tickets_once() {
    use crate::update::{UpdateDecision, decide_update};
    use crate::upkeep::{UpkeepDeps, upkeep_pass};

    let (_serial, pg) = setup().await;
    // A run that stopped part way left its plugin behind, with its repository
    // gone: the pass would count it as an error here.
    sqlx::query("delete from app_sources where slug like 'pu-upkeep-%'")
        .execute(&pg)
        .await
        .unwrap();
    let w = World::new(&pg, "upkeep").await;
    let slug = format!("pu-{}", w.tag);
    let (repo, sha1) = app_repo(&slug);
    let gh_repo = format!("acme/{slug}");
    let gh_url = format!("https://github.com/{gh_repo}");

    // Installed through the decision, then read as a GitHub plugin: PRs come
    // from the stub under `acme/<slug>`, commits from the file:// remote.
    let id = file_request(&pg, &w.ask(&gh_repo, &gh_url)).await.unwrap();
    point_at(&pg, &id, &repo.url).await;
    let (host, steps) = mock_host().await;
    decide(
        &pg,
        &NoCreds,
        &host,
        &id,
        Decision::Approve { kept_data: None },
        None,
        "admin@example.invalid",
    )
    .await
    .unwrap();
    sqlx::query("update app_sources set repo = $2 where slug = $1")
        .bind(&slug)
        .bind(&gh_repo)
        .execute(&pg)
        .await
        .unwrap();

    // The maintainer opened PR #7 from the ticket and handed it to review.
    let meta = talaria_statuses::status_meta(&pg, &w.board_id)
        .await
        .unwrap();
    let review = meta.review_key.clone().expect("a review column");
    sqlx::query("update tasks set status = $2 where id = $1::uuid")
        .bind(&w.task_id)
        .bind(&review)
        .execute(&pg)
        .await
        .unwrap();
    sqlx::query(
        "insert into workbench_jobs (agent_id, agent_model, task_id, host_id, repo, branch, status, pr_number, pr_url) \
         values ($1::uuid, $2, $3::uuid, 'github', $4, 'talaria/plg-1-tags', 'pr_open', 7, $5)",
    )
    .bind(&w.agent_id)
    .bind(&w.agent_model)
    .bind(&w.task_id)
    .bind(&gh_repo)
    .bind(format!("https://github.com/{gh_repo}/pull/7"))
    .execute(&pg)
    .await
    .unwrap();

    // A PAT connection the test's own box sealed, restored after.
    let sb = talaria_secretbox::SecretBox::from_parts(
        [5u8; 32],
        std::collections::HashMap::from([(1u32, talaria_secretbox::new_dek().unwrap())]),
        Some(1),
    );
    let prior: Value = sqlx::query_scalar("select auth from git_hosts where id = 'github'")
        .fetch_one(&pg)
        .await
        .unwrap();
    let set_config = |v: Value| {
        let pg = pg.clone();
        async move {
            sqlx::query("update git_hosts set auth = $1 where id = 'github'")
                .bind(v)
                .execute(&pg)
                .await
                .unwrap();
            talaria_git_hosts::forget("github");
        }
    };
    set_config(json!({ "mode": "pat", "pat": { "tokenEnc": sb.seal("pat-test").unwrap() } })).await;

    // GitHub's recorded answers: PR #7 open with a failing check and a
    // change request, then merged.
    let merged = Arc::new(Mutex::new(false));
    let prefix = format!("/repos/{gh_repo}");
    let route: Arc<dyn Fn(&str) -> (u16, String) + Send + Sync> = {
        let merged = merged.clone();
        let prefix = prefix.clone();
        let gh_repo = gh_repo.clone();
        Arc::new(move |path: &str| {
            let is_merged = *merged.lock().unwrap();
            let rest = path.strip_prefix(&prefix).unwrap_or(path);
            if rest.starts_with("/pulls?state=open") {
                return if is_merged {
                    (200, "[]".into())
                } else {
                    (
                        200,
                        json!([
                            { "number": 7, "html_url": format!("https://github.com/{gh_repo}/pull/7"), "title": "Add tags", "head": { "ref": "talaria/plg-1-tags" }, "base": { "ref": "main" }, "user": { "login": "talaria-app[bot]" }, "draft": false },
                            { "number": 8, "html_url": format!("https://github.com/{gh_repo}/pull/8"), "title": "Ann's tweak", "head": { "ref": "ann/tweak" }, "base": { "ref": "main" }, "user": { "login": "ann" }, "draft": false },
                        ]).to_string(),
                    )
                };
            }
            match rest {
                // A PR no workbench job opened: its failure is said on the
                // plugin's ticket, and nobody is put on it.
                "/pulls/8" => (
                    200,
                    json!({ "number": 8, "html_url": format!("https://github.com/{gh_repo}/pull/8"), "title": "Ann's tweak", "state": if is_merged { "closed" } else { "open" }, "merged": false, "head": { "ref": "ann/tweak", "sha": "f00d" }, "base": { "ref": "main" } }).to_string(),
                ),
                "/commits/f00d/check-runs?per_page=100" => (
                    200,
                    r#"{"check_runs":[{"name":"lint","status":"completed","conclusion":"failure"}]}"#.into(),
                ),
                "/commits/f00d/status" => (200, r#"{"statuses":[]}"#.into()),
                "/pulls/8/reviews?per_page=100" => (200, "[]".into()),
                "/pulls/7" => (
                    200,
                    json!({ "number": 7, "html_url": format!("https://github.com/{gh_repo}/pull/7"), "title": "Add tags", "state": if is_merged { "closed" } else { "open" }, "merged": is_merged, "merged_at": if is_merged { json!("2026-10-26T12:00:00Z") } else { Value::Null }, "head": { "ref": "talaria/plg-1-tags", "sha": "beef" }, "base": { "ref": "main" } }).to_string(),
                ),
                "/commits/beef/check-runs?per_page=100" => (
                    200,
                    r#"{"check_runs":[{"name":"test","status":"completed","conclusion":"failure"}]}"#.into(),
                ),
                "/commits/beef/status" => (200, r#"{"statuses":[]}"#.into()),
                "/pulls/7/reviews?per_page=100" => (
                    200,
                    r#"[{"user":{"login":"ann"},"state":"CHANGES_REQUESTED","submitted_at":"2026-10-26T10:00:00Z"}]"#.into(),
                ),
                _ => (404, r#"{"message":"Not Found"}"#.into()),
            }
        })
    };
    let (base, calls) = github_stub(route).await;
    let deps = UpkeepDeps {
        pg: pg.clone(),
        sb: sb.clone(),
        redis: None,
        host: host.clone(),
    };
    let pass = || talaria_github::with_api_base(base.clone(), upkeep_pass(&deps));

    // 1. Failing checks and a change request: one comment, and the ticket
    //    goes back from review to the board's working column with the
    //    maintainer on it.
    let r = pass().await.unwrap();
    assert_eq!((r.plugins >= 1, r.resumed, r.errors), (true, 2, 0), "{r:?}");
    let ann = comments(&pg, &w.task_id, "Plugin PR #8").await;
    assert_eq!(ann.len(), 1, "{ann:?}");
    assert!(
        ann[0].contains("its checks are failing (lint)")
            && ann[0].contains(
                "No workbench job opened it (its author is ann), so no agent was put on it."
            ),
        "{}",
        ann[0]
    );
    let said = comments(&pg, &w.task_id, "Plugin PR #7").await;
    assert_eq!(said.len(), 1, "{said:?}");
    assert!(
        said[0].contains("its checks are failing (test) and ann requested changes"),
        "{}",
        said[0]
    );
    assert!(said[0].contains(&format!(
        "Talaria put {} back on this ticket",
        w.agent_model
    )));
    let task = talaria_tasks::get_task(&pg, &w.task_id)
        .await
        .unwrap()
        .unwrap();
    assert!(
        meta.working_keys.contains(&task.status),
        "moved to {}",
        task.status
    );
    assert!(task.assignees.contains(&w.agent_model));

    // 2. The same state again is not news.
    let r = pass().await.unwrap();
    assert_eq!(r.resumed, 0, "{r:?}");
    assert_eq!(comments(&pg, &w.task_id, "Plugin PR #7").await.len(), 1);

    // 3. The PR merges and moves main: the merge is said on the ticket, and
    //    one update waits for an admin.
    *merged.lock().unwrap() = true;
    let sha2 = repo.commit(&[("version", "2".to_string())], "v2");
    let r = pass().await.unwrap();
    assert_eq!((r.merged, r.updates_raised), (1, 1), "{r:?}");
    let pending: Vec<(String, String, Value)> = sqlx::query_as(
        "select id::text, target_sha, prs from app_update_requests where app_slug = $1 and status = 'pending'",
    )
    .bind(&slug)
    .fetch_all(&pg)
    .await
    .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].1, sha2);
    assert_eq!(pending[0].2, json!([7]));
    let lines = w.lines(&pg).await;
    assert!(
        lines
            .iter()
            .any(|l| l.starts_with("plugin PR #7 merged into main")),
        "{lines:?}"
    );
    assert!(
        lines
            .iter()
            .any(|l| l.contains("waits for an admin's approval in Manage → Apps")),
        "{lines:?}"
    );
    // Announced on the spot, through the one announcer.
    let announced: Value =
        sqlx::query_scalar("select value from app_settings where key = 'approval_announce_state'")
            .fetch_one(&pg)
            .await
            .unwrap();
    assert!(
        announced["announced"]
            .get(format!("app_update:{}", pending[0].0))
            .is_some(),
        "{announced}"
    );
    let r = pass().await.unwrap();
    assert_eq!(r.updates_raised, 0, "one waiting update per plugin");

    // 4. The admin approves: update_app through the mock host, and the line.
    let creds = talaria_apps::PlatformCredentials {
        pg: &pg,
        sb: &sb,
        actor: "admin@example.invalid",
    };
    steps.lock().unwrap().clear();
    let decided = decide_update(
        &pg,
        &creds,
        &host,
        &pending[0].0,
        UpdateDecision::Approve,
        None,
        "admin@example.invalid",
    )
    .await
    .unwrap();
    assert_eq!(decided.status, "applied", "{decided:?}");
    assert_eq!(*steps.lock().unwrap(), ["stage", "activate"]);
    let p = crate::plugin(&pg, &slug).await.unwrap().unwrap();
    assert_eq!(p.installed_commit.as_deref(), Some(sha2.as_str()));
    assert!(w.lines(&pg).await.iter().any(|l| l
        == &format!(
            "approved update: updated plugin \"{slug}\" from {} to {}",
            &sha1[..7],
            &sha2[..7]
        )));
    // Running the newest commit now: the pass raises nothing.
    assert_eq!(pass().await.unwrap().updates_raised, 0);

    // 5. Health: it built on host h1, and fails after an upgrade to h2 — one
    //    ticket on the plugin's board for its maintainer, however often the
    //    pass sees it.
    let builds = talaria_users::app_builds_dir().join(&slug);
    std::fs::create_dir_all(&builds).unwrap();
    let pointer = |status: &str, host_build: &str| {
        std::fs::write(
            builds.join("current.json"),
            json!({ "key": "k", "status": status, "hostBuild": host_build, "error": "app.ts: cannot find name 'useApp'" }).to_string(),
        )
        .unwrap();
    };
    pointer("ready", "h1");
    assert_eq!(pass().await.unwrap().notices, 0);
    pointer("failed", "h2");
    assert_eq!(pass().await.unwrap().notices, 1);
    assert_eq!(pass().await.unwrap().notices, 0, "one open ticket per kind");
    let filed: Vec<(String, String, Value)> = sqlx::query_as(
        "select t.title, n.kind, t.assignees from plugin_notices n join tasks t on t.id = n.task_id \
         where n.app_slug = $1 order by n.created_at",
    )
    .bind(&slug)
    .fetch_all(&pg)
    .await
    .unwrap();
    assert_eq!(filed.len(), 1);
    assert_eq!(
        filed[0].0,
        format!("[plugin {slug}] broke after the host upgrade")
    );
    assert_eq!(filed[0].1, "host_upgrade");
    assert_eq!(filed[0].2, json!([w.agent_model]));

    // 6. Evals: a band that gets worse is a ticket.
    let prior_index: Option<Value> =
        sqlx::query_scalar("select value from app_settings where key = 'model_fitness_index'")
            .fetch_optional(&pg)
            .await
            .unwrap();
    let index = |band: &str| json!({ "model-a": { "at": "2026-10-26T00:00:00Z", "cells": { format!("app:{slug}/{slug}:triage"): { "band": band, "reason": "contract held 3/10" } } } });
    let set_index = |v: Value| {
        let pg = pg.clone();
        async move {
            sqlx::query(
                "insert into app_settings (key, value) values ('model_fitness_index', $1) \
                 on conflict (key) do update set value = excluded.value",
            )
            .bind(v)
            .execute(&pg)
            .await
            .unwrap();
        }
    };
    set_index(index("ready")).await;
    assert_eq!(
        pass().await.unwrap().notices,
        0,
        "the first pass only records"
    );
    set_index(index("unfit")).await;
    assert_eq!(pass().await.unwrap().notices, 1);
    let eval: String = sqlx::query_scalar(
        "select t.description from plugin_notices n join tasks t on t.id = n.task_id \
         where n.app_slug = $1 and n.kind = 'eval_regression'",
    )
    .bind(&slug)
    .fetch_one(&pg)
    .await
    .unwrap();
    assert!(
        eval.contains(&format!(
            "{slug}:triage on model-a: ready → unfit (contract held 3/10)"
        )),
        "{eval}"
    );

    // The pass only ever read GitHub.
    let calls = calls.lock().unwrap().clone();
    assert!(calls.iter().all(|c| c.starts_with("GET ")), "{calls:?}");

    sqlx::query("delete from app_sources where slug = $1")
        .bind(&slug)
        .execute(&pg)
        .await
        .unwrap();
    let _ = std::fs::remove_dir_all(talaria_users::apps_dir().join(&slug));
    match prior_index {
        Some(v) => set_index(v).await,
        None => {
            sqlx::query("delete from app_settings where key = 'model_fitness_index'")
                .execute(&pg)
                .await
                .unwrap();
        }
    }
    set_config(prior).await;
}
