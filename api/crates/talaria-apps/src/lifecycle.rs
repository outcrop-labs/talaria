// An app's lifecycle around its code: install from git, update to a newer
// commit, uninstall. The api owns these (the credential, the swap, the
// record); the app host builds and loads the code (`AppHost`).
//
//   install    fetch into apps/.install-<random> with a read-only credential,
//              validate talaria.json, pick the slug and hold it, settle any
//              data an earlier install kept under it (adopted only when it
//              was kept from this same repository, or when the admin says
//              so), rename into apps/<slug> (one event for the watcher, never
//              a half-written tree), record {host, repo, ref, sha} in
//              app_sources and the repository in the data directory, then ask
//              the host to build it — the file watcher is not what builds it
//   update     fetch the new commit into apps/.<slug>.next, have the host
//              build and probe it there while the live app keeps serving,
//              swap the two trees in one step (renameat2 RENAME_EXCHANGE);
//              when the host cannot switch to the new build, swap back. Only
//              once the new version is live does the old one replace
//              apps/.<slug>.prev. Data is never touched.
//   uninstall  the app's data is kept unless wipeData: then it is emptied
//              while its database still runs, and the directory (owned by the
//              database container's user) is removed through a throwaway
//              container when this process cannot remove it itself

use std::collections::{BTreeMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

use serde_json::Value;
use sqlx::PgPool;

use talaria_agent_auth::{epoch_ms_to_iso, now_ms};
use talaria_app_manifest::{Manifest, SDK_VERSION, parse_manifest};
use talaria_gateway::settings::{get_setting, set_setting};
use talaria_secretbox::SecretBox;
use talaria_users::{app_builds_dir, app_data_dir, apps_dir, slug_ok};

use crate::git::{
    ATOMIC_SWAP, GitAuth, exchange, fetch_into, remote_head, remove_tree, rename_noreplace,
};
use crate::host::{AppHost, host_error, host_ok};
use crate::source::{GitSource, is_commit, local_sources_allowed, parse_source, ref_ok};

const INSTALLED_KEY: &str = "apps_installed";

// ── The source record ────────────────────────────────────────────────────────

/// Where an installed app came from and what it runs: one `app_sources` row.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SourceRecord {
    pub url: String,
    pub host: String,
    pub repo: String,
    /// The ref install or the last update was asked for; None is the
    /// repository's default branch.
    #[serde(rename = "ref")]
    pub git_ref: Option<String>,
    /// The commit running now. None only for an app installed before commits
    /// were recorded.
    pub sha: Option<String>,
    /// The commit an update replaced — what `apps/.<slug>.prev` holds. None
    /// when it holds nothing the record can vouch for.
    pub previous_sha: Option<String>,
    /// The workspace secret a non-GitHub host is read with, by name.
    pub secret: Option<String>,
    pub installed_at: Option<String>,
    pub updated_at: Option<String>,
}

type SourceRow = (
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<i64>,
);

const SOURCES_SQL: &str = "select slug, url, host, repo, git_ref, sha, previous_sha, secret, \
     (extract(epoch from installed_at) * 1000)::bigint, \
     (extract(epoch from updated_at) * 1000)::bigint from app_sources";

fn record_of(row: (String, SourceRow)) -> (String, SourceRecord) {
    let (slug, (url, host, repo, git_ref, sha, previous_sha, secret, installed, updated)) = row;
    (
        slug,
        SourceRecord {
            url,
            host,
            repo,
            git_ref,
            sha,
            previous_sha,
            secret,
            installed_at: installed.map(epoch_ms_to_iso),
            updated_at: updated.map(epoch_ms_to_iso),
        },
    )
}

type FullRow = (
    String,
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<i64>,
);

fn split_row(r: FullRow) -> (String, SourceRow) {
    (r.0, (r.1, r.2, r.3, r.4, r.5, r.6, r.7, r.8, r.9))
}

/// Every installed app's source, by slug: the `app_sources` rows, and for an
/// app installed before them, what the old `apps_installed` setting kept (a
/// URL and a time, no commit).
pub async fn source_records(pg: &PgPool) -> BTreeMap<String, SourceRecord> {
    let mut out = BTreeMap::new();
    let legacy = get_setting(pg, INSTALLED_KEY, serde_json::json!({})).await;
    if let Some(obj) = legacy.as_object() {
        for (slug, v) in obj {
            let Some(url) = v.get("source").and_then(Value::as_str) else {
                continue;
            };
            let (host, repo) = parse_source(url, true)
                .map(|s| (s.host, s.repo))
                .unwrap_or_default();
            out.insert(
                slug.clone(),
                SourceRecord {
                    url: url.to_string(),
                    host,
                    repo,
                    git_ref: None,
                    sha: None,
                    previous_sha: None,
                    secret: None,
                    installed_at: v
                        .get("installedAt")
                        .and_then(Value::as_str)
                        .map(String::from),
                    updated_at: None,
                },
            );
        }
    }
    match sqlx::query_as::<_, FullRow>(SOURCES_SQL)
        .fetch_all(pg)
        .await
    {
        Ok(rows) => {
            for r in rows {
                let (slug, rec) = record_of(split_row(r));
                out.insert(slug, rec);
            }
        }
        Err(e) => tracing::warn!("[apps] could not read app_sources: {e}"),
    }
    out
}

async fn source_record(pg: &PgPool, slug: &str) -> Option<SourceRecord> {
    source_records(pg).await.remove(slug)
}

async fn write_source(
    pg: &PgPool,
    slug: &str,
    src: &GitSource,
    git_ref: Option<&str>,
    sha: &str,
    secret: Option<&str>,
    actor: &str,
) -> Result<(), String> {
    sqlx::query(
        "insert into app_sources (slug, url, host, repo, git_ref, sha, secret, installed_by, updated_by) \
         values ($1, $2, $3, $4, $5, $6, $7, $8, $8) \
         on conflict (slug) do update set url = excluded.url, host = excluded.host, \
            repo = excluded.repo, git_ref = excluded.git_ref, sha = excluded.sha, \
            previous_sha = null, secret = excluded.secret, installed_at = now(), \
            installed_by = excluded.installed_by, updated_at = now(), updated_by = excluded.updated_by",
    )
    .bind(slug)
    .bind(&src.url)
    .bind(&src.host)
    .bind(&src.repo)
    .bind(git_ref)
    .bind(sha)
    .bind(secret)
    .bind(actor)
    .execute(pg)
    .await
    .map_err(|e| format!("could not record the app's source: {e}"))?;
    Ok(())
}

/// Record that `slug` now runs `sha`, and that `apps/.<slug>.prev` holds
/// `previous` (None: it holds nothing this update can vouch for).
async fn record_update(
    pg: &PgPool,
    slug: &str,
    rec: &SourceRecord,
    git_ref: Option<&str>,
    sha: &str,
    previous: Option<&str>,
    actor: &str,
) -> Result<(), String> {
    // A legacy install (no row yet) gets its row on its first update.
    sqlx::query(
        "insert into app_sources (slug, url, host, repo, git_ref, sha, previous_sha, secret, installed_by, updated_by) \
         values ($1, $2, $3, $4, $5, $6, $7, $8, null, $9) \
         on conflict (slug) do update set git_ref = excluded.git_ref, sha = excluded.sha, \
            previous_sha = excluded.previous_sha, updated_at = now(), updated_by = excluded.updated_by",
    )
    .bind(slug)
    .bind(&rec.url)
    .bind(&rec.host)
    .bind(&rec.repo)
    .bind(git_ref)
    .bind(sha)
    .bind(previous)
    .bind(&rec.secret)
    .bind(actor)
    .execute(pg)
    .await
    .map_err(|e| format!("could not record the update: {e}"))?;
    Ok(())
}

async fn forget_source(pg: &PgPool, slug: &str) -> Result<(), String> {
    sqlx::query("delete from app_sources where slug = $1")
        .bind(slug)
        .execute(pg)
        .await
        .map_err(|e| format!("could not forget the app's source: {e}"))?;
    let mut installed = get_setting(pg, INSTALLED_KEY, serde_json::json!({})).await;
    if let Some(obj) = installed.as_object_mut()
        && obj.remove(slug).is_some()
    {
        set_setting(pg, INSTALLED_KEY, &installed)
            .await
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

// ── Credentials ──────────────────────────────────────────────────────────────

/// Where a fetch's credential comes from. The default is the GitHub
/// connection; tests substitute a fixed one.
pub trait Credentials: Sync {
    /// A read-only token for `owner/name` on github.com, or None when the
    /// connection has none to give (not configured, or it cannot see the
    /// repository — a public one then clones anonymously).
    fn github_read_token(
        &self,
        repo: &str,
    ) -> impl std::future::Future<Output = Result<Option<String>, String>> + Send;
    /// The login the workspace secret `name` holds for `host`.
    fn secret_login(
        &self,
        name: &str,
        host: &str,
    ) -> impl std::future::Future<Output = Result<(String, String), String>> + Send;
}

/// The platform's credentials: the GitHub connection
/// (`talaria_github::github_token_for`, narrowed to `REPO_READ_PERMISSIONS`)
/// and workspace secrets (`credential_from_secret`).
pub struct PlatformCredentials<'a> {
    pub pg: &'a PgPool,
    pub sb: &'a SecretBox,
    pub actor: &'a str,
}

impl Credentials for PlatformCredentials<'_> {
    async fn github_read_token(&self, repo: &str) -> Result<Option<String>, String> {
        talaria_github::github_token_for(
            self.pg,
            self.sb,
            Some(repo),
            talaria_github::RepoAccess::Read,
        )
        .await
    }

    async fn secret_login(&self, name: &str, host: &str) -> Result<(String, String), String> {
        let c = talaria_workspace_secrets::credential_from_secret(
            self.pg, self.sb, name, host, self.actor,
        )
        .await?;
        Ok((c.username, c.password))
    }
}

/// How a fetch of `src` authenticates, and a note for the error when the
/// GitHub connection had no token to give. A named workspace secret wins;
/// a github.com repository the connection can read gets a repo-scoped,
/// read-only token; anything else is fetched anonymously.
pub async fn auth_for(
    creds: &impl Credentials,
    src: &GitSource,
    secret: Option<&str>,
) -> Result<(GitAuth, Option<String>), String> {
    if let Some(name) = secret {
        if src.host.is_empty() {
            return Err("a local repository takes no credential".into());
        }
        let (username, password) = creds.secret_login(name, &src.host).await?;
        return Ok((
            GitAuth::Login {
                host: src.host.clone(),
                username,
                password,
            },
            None,
        ));
    }
    if let Some(repo) = src.github_repo() {
        return Ok(match creds.github_read_token(repo).await {
            Ok(Some(token)) => (
                GitAuth::Login {
                    host: src.host.clone(),
                    username: "x-access-token".into(),
                    password: token,
                },
                None,
            ),
            Ok(None) => (GitAuth::Anonymous, None),
            Err(e) => (
                GitAuth::Anonymous,
                Some(format!(
                    "the GitHub connection has no read token for {repo} ({e}), so it was fetched anonymously"
                )),
            ),
        });
    }
    Ok((GitAuth::Anonymous, None))
}

fn with_note(err: String, note: &Option<String>) -> String {
    match note {
        Some(n) => format!("{err} — {n}"),
        None => err,
    }
}

// ── One lifecycle step per app at a time ─────────────────────────────────────

fn busy_slugs() -> &'static Mutex<HashSet<String>> {
    static BUSY: OnceLock<Mutex<HashSet<String>>> = OnceLock::new();
    BUSY.get_or_init(|| Mutex::new(HashSet::new()))
}

/// Held while an install, update or uninstall of one app runs; dropped, it
/// frees the slug.
pub struct SlugLock(String);

impl SlugLock {
    pub fn take(slug: &str) -> Result<Self, String> {
        let mut busy = busy_slugs().lock().unwrap();
        if !busy.insert(slug.to_string()) {
            return Err(format!(
                "{slug} is already being installed, updated or removed; try again when that finishes"
            ));
        }
        Ok(Self(slug.to_string()))
    }
}

impl Drop for SlugLock {
    fn drop(&mut self) {
        busy_slugs().lock().unwrap().remove(&self.0);
    }
}

// ── Install ──────────────────────────────────────────────────────────────────

fn random_hex() -> String {
    let mut b = [0u8; 8];
    let _ = getrandom::fill(&mut b);
    b.iter().map(|x| format!("{x:02x}")).collect()
}

/// `apps/.<slug>.next` and `apps/.<slug>.prev`: the update's staging tree
/// and the version it replaced. Dot-named, so discovery and the host's
/// watcher never take them for an app.
pub fn next_dir(slug: &str) -> PathBuf {
    apps_dir().join(format!(".{slug}.next"))
}

pub fn prev_dir(slug: &str) -> PathBuf {
    apps_dir().join(format!(".{slug}.prev"))
}

/// The manifest at the root of a fetched tree, validated.
fn manifest_at(dir: &Path) -> Result<Manifest, String> {
    let bytes = std::fs::read(dir.join("talaria.json")).map_err(|_| {
        "that repository is not a Talaria app (no talaria.json at its root)".to_string()
    })?;
    parse_manifest(&bytes, SDK_VERSION).map_err(|e| format!("talaria.json is refused: {e}"))
}

/// The slug a fetched app installs as: the one its manifest declares, else
/// the admin's override, else the repository's name (`install_slug`). A
/// declared slug cannot be overridden: the app's own code may name it.
pub fn choose_slug(
    src: &GitSource,
    slug_override: Option<&str>,
    declared: Option<&str>,
) -> Result<String, String> {
    match (declared, slug_override) {
        (Some(d), Some(o)) if d != o.trim().to_lowercase() => Err(format!(
            "this app declares its slug as \"{d}\" in talaria.json; install it without another slug"
        )),
        (Some(d), _) => {
            talaria_users::slug_installable(d)?;
            Ok(d.to_string())
        }
        (None, o) => crate::install_slug(&src.url, o),
    }
}

// ── Kept data and the repository it belongs to ───────────────────────────────

/// The file in an app's data directory that names the repository the data
/// belongs to. Install writes it; an uninstall that keeps the data keeps it
/// too, so a later install of the slug can tell its own repository's data
/// from another's. It holds no secret.
pub const DATA_SOURCE_FILE: &str = "source.json";

/// The repository an app's data belongs to, as `DATA_SOURCE_FILE` holds it.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct DataSource {
    pub url: String,
    pub host: String,
    pub repo: String,
}

impl DataSource {
    fn of(src: &GitSource) -> Self {
        Self {
            url: src.url.clone(),
            host: src.host.clone(),
            repo: src.repo.clone(),
        }
    }

    /// The same repository: the same host and path, the path compared
    /// without case (GitHub, and most hosts, read it that way).
    pub fn same_repo(&self, src: &GitSource) -> bool {
        self.host == src.host && self.repo.eq_ignore_ascii_case(&src.repo)
    }
}

/// Whether an earlier install left data in `dir`: the database files, or the
/// db.env that holds their password.
pub fn has_kept_data(dir: &Path) -> bool {
    dir.join("pg").exists() || dir.join("db.env").exists()
}

/// The repository `dir`'s data belongs to; None when nothing recorded one.
pub fn read_data_source(dir: &Path) -> Option<DataSource> {
    let raw = std::fs::read(dir.join(DATA_SOURCE_FILE)).ok()?;
    serde_json::from_slice(&raw).ok()
}

/// An app's data directory, made 0700 as the app host makes it.
fn create_data_dir(dir: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        std::fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(dir)
    }
    #[cfg(not(unix))]
    {
        std::fs::create_dir_all(dir)
    }
}

/// Record that `dir`'s data belongs to `source`, creating the directory when
/// the app has stored nothing yet.
pub fn write_data_source(dir: &Path, source: &DataSource) -> Result<(), String> {
    create_data_dir(dir).map_err(|e| format!("could not create {}: {e}", dir.display()))?;
    let body = serde_json::to_vec_pretty(source).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(DATA_SOURCE_FILE), body)
        .map_err(|e| format!("could not record whose data {} holds: {e}", dir.display()))
}

/// What an install does with data an earlier install of its slug kept.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeptDataChoice {
    /// The new app takes the data: its store and its database password.
    Adopt,
    /// The data is deleted before the new app is placed.
    Wipe,
}

/// What an install settled on for the slug's data.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeptDataPlan {
    /// Nothing was kept.
    Fresh,
    Adopt,
    Wipe,
}

/// Data kept under a slug that an install may not take without being told:
/// it was kept from another repository, or from an install that recorded
/// none.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct KeptDataConflict {
    pub slug: String,
    /// The repository the data was kept from; None when it was not recorded.
    pub kept_from: Option<String>,
    /// The repository being installed.
    pub installing: String,
}

/// Why an install did not happen. `kept_data` is set when what stands in the
/// way is data an earlier install kept, and the request can say what to do
/// with it (`KeptDataChoice`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallError {
    pub error: String,
    pub kept_data: Option<KeptDataConflict>,
}

impl From<String> for InstallError {
    fn from(error: String) -> Self {
        Self {
            error,
            kept_data: None,
        }
    }
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.error)
    }
}

/// What an install of `src` as `slug` does with the data in `dir`. Data kept
/// from the same repository is adopted, as a reinstall expects. Data kept
/// from another repository, or from an install that did not record one, is
/// another app's store and database password: the install refuses, naming
/// both, unless the request chose. An explicit choice is always honoured.
pub fn kept_data_plan(
    slug: &str,
    dir: &Path,
    src: &GitSource,
    choice: Option<KeptDataChoice>,
) -> Result<KeptDataPlan, InstallError> {
    if !has_kept_data(dir) {
        return Ok(KeptDataPlan::Fresh);
    }
    let recorded = read_data_source(dir);
    match choice {
        Some(KeptDataChoice::Wipe) => return Ok(KeptDataPlan::Wipe),
        Some(KeptDataChoice::Adopt) => return Ok(KeptDataPlan::Adopt),
        None if recorded.as_ref().is_some_and(|r| r.same_repo(src)) => {
            return Ok(KeptDataPlan::Adopt);
        }
        None => {}
    }
    let error = match &recorded {
        Some(r) => format!(
            "the slug \"{slug}\" holds data kept from {}, and this install is {}: say whether to adopt that data or wipe it (keptData: \"adopt\" or \"wipe\")",
            r.url, src.url
        ),
        None => format!(
            "the slug \"{slug}\" holds data kept from an earlier install that did not record its repository, and this install is {}: say whether to adopt that data or wipe it (keptData: \"adopt\" or \"wipe\")",
            src.url
        ),
    };
    Err(InstallError {
        error,
        kept_data: Some(KeptDataConflict {
            slug: slug.to_string(),
            kept_from: recorded.map(|r| r.url),
            installing: src.url.clone(),
        }),
    })
}

/// What an install asks for.
pub struct InstallRequest<'a> {
    pub url: &'a str,
    pub slug: Option<&'a str>,
    pub git_ref: Option<&'a str>,
    /// A workspace secret holding an https token, for a host other than the
    /// GitHub connection's.
    pub secret: Option<&'a str>,
    /// What to do with data an earlier install of the slug kept. None adopts
    /// it only when it was kept from this same repository (`kept_data_plan`).
    pub kept_data: Option<KeptDataChoice>,
    pub actor: &'a str,
}

/// What an install did.
#[derive(Debug, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Installed {
    pub slug: String,
    pub sha: String,
    /// A previous install's data was kept (uninstall without wipeData), and
    /// this install reattaches it.
    pub kept_data: bool,
    /// A previous install's data was kept, and this install deleted it first,
    /// as asked.
    pub wiped_data: bool,
    /// The host's build of the new app: `{ ok, build, error? }`, or None when
    /// the host could not be asked (`build_error` says why).
    pub build: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build_error: Option<String>,
}

/// Install an app from git. See the module header for the steps. The slug is
/// held (`SlugLock`) from the moment it is chosen until the build answers, so
/// an install never races an update or uninstall of the same slug.
pub async fn install_app(
    pg: &PgPool,
    creds: &impl Credentials,
    host: Option<&AppHost>,
    req: InstallRequest<'_>,
) -> Result<Installed, InstallError> {
    let src = parse_source(req.url, local_sources_allowed())?;
    if let Some(r) = req.git_ref {
        ref_ok(r)?;
    }
    if let Some(o) = req.slug {
        talaria_users::slug_installable(&o.trim().to_lowercase())?;
    }
    let (auth, note) = auth_for(creds, &src, req.secret).await?;
    let apps = apps_dir();
    tokio::fs::create_dir_all(&apps)
        .await
        .map_err(|e| format!("could not create {}: {e}", apps.display()))?;
    let tmp = apps.join(format!(".install-{}", random_hex()));
    let sha = fetch_into(&tmp, &src, req.git_ref, &auth)
        .await
        .map_err(|e| with_note(e, &note))?;
    let placed = async {
        let manifest = manifest_at(&tmp)?;
        let slug = choose_slug(&src, req.slug, manifest.slug.as_deref())?;
        let lock = SlugLock::take(&slug)?;
        let target = apps.join(&slug);
        if target.exists() {
            return Err(format!("apps/{slug} already exists").into());
        }
        let data = app_data_dir().join(&slug);
        let plan = kept_data_plan(&slug, &data, &src, req.kept_data)?;
        if plan == KeptDataPlan::Wipe {
            crate::stop_app_db(&slug).await;
            remove_data_dir(&data).await?;
            crate::forget_app_db_password(pg, &slug).await;
        }
        rename_noreplace(&tmp, &target).map_err(|e| match e.kind() {
            std::io::ErrorKind::AlreadyExists | std::io::ErrorKind::DirectoryNotEmpty => {
                format!("apps/{slug} already exists")
            }
            _ => format!("could not place apps/{slug}: {e}"),
        })?;
        Ok::<_, InstallError>((slug, lock, data, plan))
    }
    .await;
    let (slug, _lock, data, plan) = match placed {
        Ok(p) => p,
        Err(e) => {
            let _ = remove_tree(&tmp).await;
            return Err(e);
        }
    };
    // The data is this repository's from now on. A failed write only costs
    // a later reinstall a question (no record: it asks before it adopts).
    if let Err(e) = write_data_source(&data, &DataSource::of(&src)) {
        tracing::warn!("[apps] {slug}: {e}");
    }
    write_source(pg, &slug, &src, req.git_ref, &sha, req.secret, req.actor).await?;
    // The build is asked for, not left to the host's file watcher: the
    // install answers with its outcome.
    let (build, build_error) = match host {
        Some(h) => match h.rebuild(&slug).await {
            Ok(v) => (Some(v), None),
            Err(e) => (None, Some(e)),
        },
        None => (None, Some("no app host to build it".into())),
    };
    Ok(Installed {
        slug,
        sha,
        kept_data: plan == KeptDataPlan::Adopt,
        wiped_data: plan == KeptDataPlan::Wipe,
        build,
        build_error,
    })
}

// ── Update ───────────────────────────────────────────────────────────────────

/// What an update did. `ok` false with `rolled_back` true means the app is
/// still running `from` — the new version failed before or after the swap
/// and the live tree is the previous one. `ok` false with `rolled_back`
/// false means the new version failed after the swap and the swap back
/// failed too: `apps/<slug>` holds `to`, which did not load.
#[derive(Debug, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Updated {
    pub ok: bool,
    pub slug: String,
    pub from: Option<String>,
    pub to: Option<String>,
    #[serde(rename = "ref")]
    pub git_ref: Option<String>,
    pub up_to_date: bool,
    pub rolled_back: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub build: Option<Value>,
}

impl Updated {
    /// The update did not land, and the live tree is the one it started
    /// from.
    fn failed(mut self, error: String) -> Self {
        self.ok = false;
        self.rolled_back = true;
        self.error = Some(error);
        self
    }

    /// The update did not land, and the live tree is the new version: the
    /// swap back failed.
    fn stuck(mut self, error: String) -> Self {
        self.ok = false;
        self.rolled_back = false;
        self.error = Some(error);
        self
    }
}

/// After the swap, `next` holds the version that was live (`from`). Make it
/// `apps/.<slug>.prev`, replacing what that held (`held`), and answer the
/// commit `.prev` holds afterwards — what the record's previous_sha must say:
/// `from` when the move landed; `held` when there was nothing at `next` to
/// move, so `.prev` was left alone; None when the move failed part way.
async fn keep_previous<'a>(
    slug: &str,
    next: &Path,
    prev: &Path,
    from: Option<&'a str>,
    held: Option<&'a str>,
) -> Option<&'a str> {
    if !next.is_dir() {
        return held;
    }
    let kept = async {
        remove_tree(prev).await?;
        tokio::fs::rename(next, prev).await
    }
    .await;
    match kept {
        Ok(()) => from,
        Err(e) => {
            tracing::warn!(
                "[apps] {slug}: could not keep the previous version as {}: {e}",
                prev.display()
            );
            let _ = remove_tree(next).await;
            None
        }
    }
}

/// Update an installed app to what `git_ref` (its recorded ref when None)
/// points at now. Err is a refusal before anything was fetched; every later
/// failure is an `Updated` that says whether it was rolled back.
pub async fn update_app(
    pg: &PgPool,
    creds: &impl Credentials,
    host: &AppHost,
    slug: &str,
    git_ref: Option<&str>,
    actor: &str,
) -> Result<Updated, String> {
    if !slug_ok(slug) {
        return Err("bad slug".into());
    }
    // Refused before the fetch: without the one-step swap, the fetch, the
    // build and the probe would all run for a swap that cannot happen.
    if !ATOMIC_SWAP {
        return Err(
            "updating an app needs an atomic directory swap (Linux renameat2), which this platform does not have: uninstall the app, keeping its data, and install the new version"
                .into(),
        );
    }
    let live = apps_dir().join(slug);
    if !live.is_dir() {
        return Err(format!("no app \"{slug}\" installed"));
    }
    let _lock = SlugLock::take(slug)?;
    let Some(rec) = source_record(pg, slug).await else {
        return Err(format!(
            "{slug} was not installed from git, so there is nothing to update it from: reinstall it from its repository's URL"
        ));
    };
    if let Some(r) = git_ref {
        ref_ok(r)?;
    }
    let want_ref = git_ref.map(String::from).or_else(|| rec.git_ref.clone());
    let src = parse_source(&rec.url, local_sources_allowed())?;
    let (auth, note) = auth_for(creds, &src, rec.secret.as_deref()).await?;
    let mut out = Updated {
        slug: slug.to_string(),
        from: rec.sha.clone(),
        git_ref: want_ref.clone(),
        ..Default::default()
    };

    let next = next_dir(slug);
    remove_tree(&next)
        .await
        .map_err(|e| format!("could not clear {}: {e}", next.display()))?;
    let sha = match fetch_into(&next, &src, want_ref.as_deref(), &auth).await {
        Ok(sha) => sha,
        Err(e) => return Ok(out.failed(with_note(e, &note))),
    };
    out.to = Some(sha.clone());
    if rec.sha.as_deref() == Some(sha.as_str()) && want_ref == rec.git_ref {
        let _ = remove_tree(&next).await;
        out.ok = true;
        out.up_to_date = true;
        return Ok(out);
    }
    let checked = manifest_at(&next).and_then(|m| match m.slug {
        Some(d) if d != slug => Err(format!(
            "the new version declares the slug \"{d}\", but this app is installed as \"{slug}\""
        )),
        _ => Ok(()),
    });
    if let Err(e) = checked {
        let _ = remove_tree(&next).await;
        return Ok(out.failed(e));
    }

    // Build and probe the new tree beside the live one.
    match host.lifecycle(slug, "stage").await {
        Ok(v) if host_ok(&v) => {}
        Ok(v) => {
            let _ = remove_tree(&next).await;
            return Ok(out.failed(format!("the new version did not build: {}", host_error(&v))));
        }
        Err(e) => {
            let _ = host.lifecycle(slug, "release").await;
            let _ = remove_tree(&next).await;
            return Ok(out.failed(e));
        }
    }

    // The swap: one step, and apps/<slug> never disappears.
    if let Err(e) = exchange(&next, &live) {
        let _ = host.lifecycle(slug, "release").await;
        let _ = remove_tree(&next).await;
        return Ok(out.failed(format!("could not swap the new version in: {e}")));
    }
    // apps/.<slug>.next now holds the version that was live, and stays there
    // until the new one is: apps/.<slug>.prev keeps what the record's
    // previous_sha names until this update has landed.
    let prev = prev_dir(slug);
    match host.lifecycle(slug, "activate").await {
        Ok(v) if host_ok(&v) => {
            let previous = keep_previous(
                slug,
                &next,
                &prev,
                rec.sha.as_deref(),
                rec.previous_sha.as_deref(),
            )
            .await;
            record_update(pg, slug, &rec, want_ref.as_deref(), &sha, previous, actor).await?;
            out.ok = true;
            out.build = v.get("build").cloned();
            Ok(out)
        }
        answer => {
            let why = match answer {
                Ok(v) => host_error(&v),
                Err(e) => e,
            };
            // Swap back: the previous version is live again, and the host
            // rebuilds it (its build is still on disk, so this is a reuse).
            if let Err(e) = exchange(&next, &live) {
                // apps/<slug> holds the new version: the record says so, and
                // the answer does not claim a rollback.
                let previous = keep_previous(
                    slug,
                    &next,
                    &prev,
                    rec.sha.as_deref(),
                    rec.previous_sha.as_deref(),
                )
                .await;
                if let Err(re) =
                    record_update(pg, slug, &rec, want_ref.as_deref(), &sha, previous, actor).await
                {
                    tracing::warn!("[apps] {slug}: {re}");
                }
                return Ok(out.stuck(format!(
                    "the new version failed ({why}), and swapping the previous one back failed too: {e}"
                )));
            }
            let _ = remove_tree(&next).await;
            let rebuilt = host.rebuild(slug).await;
            out.build = rebuilt.ok().and_then(|v| v.get("build").cloned());
            Ok(out.failed(format!("the new version failed to load: {why}")))
        }
    }
}

// ── Is there a newer commit? ─────────────────────────────────────────────────

/// One app's update check.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateCheck {
    pub current: Option<String>,
    pub latest: Option<String>,
    /// The ref points somewhere other than the running commit.
    pub available: bool,
    /// The app is pinned to a commit id: there is nothing newer to follow.
    pub pinned: bool,
    /// Why the source was not asked at all: it is read with a workspace
    /// secret, and asking would spend a use of it. A passive check never
    /// does; Update fetches with it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_checked: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub checked_at: String,
}

type CheckCache = Mutex<BTreeMap<String, (i64, UpdateCheck)>>;

fn check_cache() -> &'static CheckCache {
    static CACHE: OnceLock<CheckCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

/// A check is reused for this long — the page that asks is opened often, and
/// every check mints a GitHub read token. (A workspace secret is never spent
/// by a check at all: `NOT_CHECKED_SECRET`.)
const CHECK_TTL_MS: i64 = 60_000;

/// Why a check leaves a secret-read source alone. A workspace secret may have
/// a use limit, and Manage → Apps checks on its own whenever an admin opens
/// it: spending a use there could exhaust the secret before anyone pressed
/// Update, and leave the app with no way to update.
pub const NOT_CHECKED_SECRET: &str = "not checked: this app's source is read with a workspace secret, and checking would spend a use of it; Update fetches with it";

async fn check_one(creds: &impl Credentials, slug: &str, rec: &SourceRecord) -> UpdateCheck {
    let cache_key = format!(
        "{slug}\0{}\0{}\0{}",
        rec.url,
        rec.git_ref.as_deref().unwrap_or(""),
        rec.sha.as_deref().unwrap_or("")
    );
    if let Some((at, hit)) = check_cache().lock().unwrap().get(&cache_key)
        && now_ms() - at < CHECK_TTL_MS
    {
        return hit.clone();
    }
    let mut check = UpdateCheck {
        current: rec.sha.clone(),
        latest: None,
        available: false,
        pinned: rec.git_ref.as_deref().is_some_and(is_commit),
        not_checked: None,
        error: None,
        checked_at: epoch_ms_to_iso(now_ms()),
    };
    if check.pinned {
        check.latest = rec.git_ref.clone();
    } else if rec.secret.is_some() {
        check.not_checked = Some(NOT_CHECKED_SECRET.into());
    } else {
        let looked = async {
            let src = parse_source(&rec.url, local_sources_allowed())?;
            let (auth, note) = auth_for(creds, &src, None).await?;
            remote_head(&src, rec.git_ref.as_deref(), &auth)
                .await
                .map_err(|e| with_note(e, &note))
        }
        .await;
        match looked {
            Ok(Some(latest)) => {
                check.available = rec.sha.as_deref() != Some(latest.as_str());
                check.latest = Some(latest);
            }
            Ok(None) => {
                check.error = Some(format!(
                    "the repository has no {}",
                    rec.git_ref.as_deref().unwrap_or("default branch")
                ));
            }
            Err(e) => check.error = Some(e),
        }
    }
    check_cache()
        .lock()
        .unwrap()
        .insert(cache_key, (now_ms(), check.clone()));
    check
}

/// For every app installed from git: the commit it runs, the one its ref
/// points at now (`git ls-remote`), and whether they differ. The checks run
/// together, each under git's deadline.
pub async fn update_checks(pg: &PgPool, creds: &impl Credentials) -> BTreeMap<String, UpdateCheck> {
    let records = source_records(pg).await;
    let installed: HashSet<String> = talaria_users::discovered_apps()
        .into_iter()
        .map(|a| a.slug)
        .chain(talaria_users::broken_apps().into_iter().map(|b| b.slug))
        .collect();
    let jobs = records
        .iter()
        .filter(|(slug, _)| installed.contains(*slug))
        .map(|(slug, rec)| async move { (slug.clone(), check_one(creds, slug, rec).await) });
    futures_util::future::join_all(jobs)
        .await
        .into_iter()
        .collect()
}

// ── Uninstall ────────────────────────────────────────────────────────────────

/// What uninstall left of the app's data.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DataLeft {
    /// Asked to wipe, and the data directory is gone.
    Wiped,
    /// Kept for a reinstall: the database files, db.env and the sealed
    /// password, with the record of the repository they belong to.
    Kept,
    /// The app never stored anything.
    None,
}

fn db_image() -> String {
    std::env::var("TALARIA_APP_DB_IMAGE")
        .or_else(|_| std::env::var("TALARIA_PG_IMAGE"))
        .ok()
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| "docker.io/library/postgres:16-alpine".into())
}

async fn docker(args: &[&str], deadline: Duration) -> Result<String, String> {
    let out = tokio::time::timeout(
        deadline,
        tokio::process::Command::new("docker")
            .args(args)
            .stdin(std::process::Stdio::null())
            .kill_on_drop(true)
            .output(),
    )
    .await
    .map_err(|_| format!("docker {}: timed out", args.first().copied().unwrap_or("")))?
    .map_err(|e| format!("docker {}: {e}", args.first().copied().unwrap_or("")))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Empty the app's store while its database still runs.
async fn truncate_store(slug: &str) -> Result<(), String> {
    let container = crate::app_db_container(slug);
    let running = docker(
        &["inspect", "-f", "{{.State.Running}}", &container],
        Duration::from_secs(15),
    )
    .await
    .map(|s| s == "true")
    .unwrap_or(false);
    if !running {
        return Ok(());
    }
    docker(
        &[
            "exec",
            &container,
            "psql",
            "-U",
            "talaria",
            "-d",
            "talaria",
            "-v",
            "ON_ERROR_STOP=1",
            "-q",
            "-c",
            "truncate docs",
        ],
        Duration::from_secs(60),
    )
    .await
    .map(|_| ())
}

/// Remove an app's data directory. The database files inside belong to the
/// database container's user, which this process usually is not, so what it
/// cannot remove itself is removed by a throwaway container of the same
/// image, with no network, mounting only this directory.
pub async fn remove_data_dir(dir: &Path) -> Result<(), String> {
    if remove_tree(dir).await.is_ok() {
        return Ok(());
    }
    let mount = format!("{}:/d", dir.display());
    docker(
        &[
            "run",
            "--rm",
            "--network",
            "none",
            "--entrypoint",
            "sh",
            "-v",
            &mount,
            &db_image(),
            "-c",
            "find /d -mindepth 1 -delete",
        ],
        Duration::from_secs(120),
    )
    .await
    .map_err(|e| format!("could not delete the app's data: {e}"))?;
    remove_tree(dir)
        .await
        .map_err(|e| format!("could not delete {}: {e}", dir.display()))
}

/// Remove an app: its enablement, its MCP server row (and every grant on it),
/// its database container, its code (with any update staging trees), its
/// build output and its source record. Its data is kept unless `wipe`: then
/// the store is emptied while the database still runs, the database is
/// stopped, and the whole data directory is removed with the sealed password.
/// Kept data keeps the record of the repository it belongs to
/// (`DATA_SOURCE_FILE`), written now from the source record for an app
/// installed before install wrote one.
pub async fn uninstall_app(
    pg: &PgPool,
    sb: &SecretBox,
    host: Option<&AppHost>,
    slug: &str,
    wipe: bool,
) -> Result<DataLeft, String> {
    if !slug_ok(slug) {
        return Err("bad slug".into());
    }
    let _lock = SlugLock::take(slug)?;
    crate::set_app_enabled(pg, sb, slug, false).await?;
    crate::remove_app_mcp_server(pg, sb, slug)
        .await
        .map_err(|e| format!("could not remove the app's MCP server: {e}"))?;
    let data = app_data_dir().join(slug);
    let _ = tokio::fs::remove_file(crate::mcp_catalog_path(slug)).await;
    if wipe && let Err(e) = truncate_store(slug).await {
        tracing::warn!("[apps] {slug}: emptying the store before stopping it failed: {e}");
    }
    crate::stop_app_db(slug).await;
    let _ = tokio::fs::remove_file(data.join("docker-compose.yml")).await;
    for dir in [
        apps_dir().join(slug),
        next_dir(slug),
        prev_dir(slug),
        app_builds_dir().join(slug),
    ] {
        if let Err(e) = remove_tree(&dir).await {
            tracing::warn!("[apps] {slug}: could not remove {}: {e}", dir.display());
        }
    }
    if let Some(h) = host
        && let Err(e) = h.lifecycle(slug, "removed").await
    {
        tracing::warn!("[apps] {slug}: the app host was not told it is gone: {e}");
    }
    if !wipe
        && has_kept_data(&data)
        && read_data_source(&data).is_none()
        && let Some(rec) = source_record(pg, slug).await
    {
        let kept = DataSource {
            url: rec.url,
            host: rec.host,
            repo: rec.repo,
        };
        if let Err(e) = write_data_source(&data, &kept) {
            tracing::warn!("[apps] {slug}: {e}");
        }
    }
    forget_source(pg, slug).await?;
    let left = if wipe {
        remove_data_dir(&data).await?;
        crate::forget_app_db_password(pg, slug).await;
        DataLeft::Wiped
    } else if has_kept_data(&data) {
        DataLeft::Kept
    } else {
        let _ = remove_tree(&data).await;
        DataLeft::None
    };
    Ok(left)
}

/// The install, update and uninstall sequences end to end, against a live
/// database, a mock app host and local git repositories (`lifecycle/live.rs`).
#[cfg(test)]
mod live;

#[cfg(test)]
mod tests {
    use super::*;

    /// Credentials that record every ask and hand out nothing.
    #[derive(Default)]
    struct Asked(Mutex<Vec<String>>);

    impl Credentials for Asked {
        async fn github_read_token(&self, repo: &str) -> Result<Option<String>, String> {
            self.0.lock().unwrap().push(format!("github:{repo}"));
            Ok(None)
        }

        async fn secret_login(&self, name: &str, host: &str) -> Result<(String, String), String> {
            self.0.lock().unwrap().push(format!("secret:{name}@{host}"));
            Err("a passive check spent the secret".into())
        }
    }

    fn record(url: &str, git_ref: Option<&str>, secret: Option<&str>) -> SourceRecord {
        let src = parse_source(url, false).unwrap();
        SourceRecord {
            url: src.url,
            host: src.host,
            repo: src.repo,
            git_ref: git_ref.map(String::from),
            sha: Some("a".repeat(40)),
            previous_sha: None,
            secret: secret.map(String::from),
            installed_at: None,
            updated_at: None,
        }
    }

    /// Manage → Apps checks on its own whenever an admin opens it. A source
    /// read with a workspace secret is left alone — a check would spend a use
    /// of the secret, and could exhaust it before anyone pressed Update.
    #[tokio::test]
    async fn a_passive_check_never_spends_a_workspace_secret() {
        let creds = Asked::default();
        let rec = record("https://GitLab.example.com/acme/crm", None, Some("deploy"));
        let check = check_one(&creds, "unit-secret-check", &rec).await;
        assert!(
            creds.0.lock().unwrap().is_empty(),
            "the check asked for a credential"
        );
        assert_eq!(check.not_checked.as_deref(), Some(NOT_CHECKED_SECRET));
        assert!(!check.available);
        assert_eq!(check.latest, None);
        assert_eq!(check.error, None);
        let wire = serde_json::to_value(&check).unwrap();
        assert_eq!(wire["notChecked"], NOT_CHECKED_SECRET);
        // A pinned app needs no remote either way: still pinned, not "not checked".
        let pinned = record(
            "https://gitlab.example.com/acme/crm",
            Some(&"b".repeat(40)),
            Some("deploy"),
        );
        let check = check_one(&creds, "unit-secret-pinned", &pinned).await;
        assert!(check.pinned && check.not_checked.is_none());
        assert!(creds.0.lock().unwrap().is_empty());
    }

    fn scratch(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("talaria-kept-{tag}-{}", random_hex()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Data kept from this same repository is adopted; another repository's,
    /// or an unrecorded one's, is refused with both named until the request
    /// chooses; a choice is always honoured.
    #[test]
    fn kept_data_is_adopted_only_from_its_own_repository() {
        let a = parse_source("https://github.com/acme/crm", false).unwrap();
        let b = parse_source("https://github.com/other/crm", false).unwrap();
        let dir = scratch("plan");
        // Nothing kept: a fresh install, whatever the request says.
        assert_eq!(
            kept_data_plan("crm", &dir, &a, None),
            Ok(KeptDataPlan::Fresh)
        );
        // A record alone (an app that never stored anything) is not kept data.
        write_data_source(&dir, &DataSource::of(&a)).unwrap();
        assert_eq!(
            kept_data_plan("crm", &dir, &b, None),
            Ok(KeptDataPlan::Fresh)
        );

        std::fs::create_dir_all(dir.join("pg")).unwrap();
        assert_eq!(
            kept_data_plan("crm", &dir, &a, None),
            Ok(KeptDataPlan::Adopt)
        );
        // The same repository typed with other capitals is still the same.
        let a_caps = parse_source("https://GitHub.com/Acme/CRM.git", false).unwrap();
        assert_eq!(
            kept_data_plan("crm", &dir, &a_caps, None),
            Ok(KeptDataPlan::Adopt)
        );

        let err = kept_data_plan("crm", &dir, &b, None).unwrap_err();
        assert!(
            err.error.contains("https://github.com/acme/crm"),
            "{}",
            err.error
        );
        assert!(
            err.error.contains("https://github.com/other/crm"),
            "{}",
            err.error
        );
        assert_eq!(
            err.kept_data,
            Some(KeptDataConflict {
                slug: "crm".into(),
                kept_from: Some("https://github.com/acme/crm".into()),
                installing: "https://github.com/other/crm".into(),
            })
        );
        assert_eq!(
            kept_data_plan("crm", &dir, &b, Some(KeptDataChoice::Adopt)),
            Ok(KeptDataPlan::Adopt)
        );
        assert_eq!(
            kept_data_plan("crm", &dir, &b, Some(KeptDataChoice::Wipe)),
            Ok(KeptDataPlan::Wipe)
        );
        assert_eq!(
            kept_data_plan("crm", &dir, &a, Some(KeptDataChoice::Wipe)),
            Ok(KeptDataPlan::Wipe)
        );

        // Kept by an install that recorded no repository (db.env alone is
        // kept data too): even the "same" repository must choose.
        std::fs::remove_file(dir.join(DATA_SOURCE_FILE)).unwrap();
        std::fs::remove_dir_all(dir.join("pg")).unwrap();
        std::fs::write(dir.join("db.env"), "POSTGRES_PASSWORD=x\n").unwrap();
        let err = kept_data_plan("crm", &dir, &a, None).unwrap_err();
        assert!(
            err.error.contains("did not record its repository"),
            "{}",
            err.error
        );
        assert_eq!(err.kept_data.unwrap().kept_from, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_failed_update_says_whether_it_was_rolled_back() {
        let failed = Updated::default().failed("x".into());
        assert!(!failed.ok && failed.rolled_back);
        let stuck = Updated::default().stuck("y".into());
        assert!(!stuck.ok && !stuck.rolled_back);
        assert_eq!(serde_json::to_value(&stuck).unwrap()["rolledBack"], false);
    }

    #[test]
    fn a_declared_slug_wins_and_cannot_be_overridden() {
        let src = parse_source("https://github.com/acme/talaria-app-crm", false).unwrap();
        assert_eq!(choose_slug(&src, None, None).unwrap(), "crm");
        assert_eq!(choose_slug(&src, Some("sales"), None).unwrap(), "sales");
        assert_eq!(
            choose_slug(&src, None, Some("contacts")).unwrap(),
            "contacts"
        );
        assert_eq!(
            choose_slug(&src, Some("contacts"), Some("contacts")).unwrap(),
            "contacts"
        );
        let err = choose_slug(&src, Some("sales"), Some("contacts")).unwrap_err();
        assert!(err.contains("declares its slug as \"contacts\""), "{err}");
    }

    #[test]
    fn staging_trees_are_never_apps() {
        assert!(!slug_ok(
            next_dir("crm").file_name().unwrap().to_str().unwrap()
        ));
        assert!(!slug_ok(
            prev_dir("crm").file_name().unwrap().to_str().unwrap()
        ));
    }
}
