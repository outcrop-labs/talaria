// The Workbench's GitHub connection — org-level, either flavor:
//   app  a GitHub App (preferred): short-lived installation tokens, per-repo
//        install scope, instant revocation from GitHub's side
//   pat  a personal access token (quickest start; fine-grained PAT advised)
// Config lives in app_settings with secrets SEALED (secretbox), same contract
// as email. Repo access per agent is a separate explicit grant table —
// connecting GitHub grants nothing to anyone by itself.
//
// The whole connection in one module: the token half the secrets route
// rides (config read, RS256 app JWT, per-repo-scoped installation tokens,
// the repo→installation routing table, the sandbox credential) plus the
// admin half the workbench family owns — live-verified status, installation
// listing, the sealed config write, the per-repo git flow, repo creation,
// and the merge/branch/PR operations the workbench MCP drives.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use rsa::pkcs1::DecodeRsaPrivateKey;
use rsa::pkcs1v15::SigningKey;
use rsa::pkcs8::DecodePrivateKey;
use rsa::sha2::Sha256;
use rsa::signature::{SignatureEncoding, Signer};
use sqlx::PgPool;

use talaria_agent_auth::now_ms;
use talaria_body::truncate_utf16;
use talaria_gateway::provider::http;
use talaria_gateway::settings::{get_setting, set_setting};
use talaria_secretbox::SecretBox;

const KEY: &str = "github_config";
const GH: &str = "https://api.github.com";

tokio::task_local! {
    /// The GitHub REST origin for the calls one task makes. Production never
    /// sets it, so every request goes to api.github.com; a test scopes it to
    /// a loopback stub with [`with_api_base`]. Task-local rather than a
    /// global so tests running in parallel each talk to their own stub.
    static API_BASE: String;
}

fn api_base() -> String {
    API_BASE
        .try_with(Clone::clone)
        .unwrap_or_else(|_| GH.to_string())
}

/// Run `f` with every GitHub call it makes sent to `base` instead of
/// api.github.com. A TEST SEAM, nothing else: this crate's tests and the
/// workbench's point it at a loopback stub so the PR and repo-creation flows
/// run end to end with no GitHub. Calls made from a task `f` spawns are not
/// covered (task-locals do not cross `tokio::spawn`).
#[doc(hidden)]
pub async fn with_api_base<F: std::future::Future>(base: impl Into<String>, f: F) -> F::Output {
    API_BASE.scope(base.into(), f).await
}

/// The whole config shape. Defaults merge per key — top-level over
/// top-level, `app` keys over `app` keys — which is what the per-field
/// extraction below performs.
#[derive(Debug, Clone, Default)]
pub struct GithubConfig {
    /// 'app' | 'pat' | null (unconfigured).
    pub mode: Option<String>,
    pub pat_token_enc: Option<String>,
    pub app_id: String,
    pub installation_ids: Vec<String>,
    pub private_key_enc: Option<String>,
    /// Orgs where agents may REQUEST new repos (human approval creates
    /// them). Empty = the feature is off. Requires the App's org
    /// Administration permission to actually create.
    pub repo_creation_orgs: Vec<String>,
}

fn cfg_str(obj: &serde_json::Map<String, serde_json::Value>, key: &str) -> Option<String> {
    obj.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}

pub async fn get_github_config(pg: &PgPool) -> GithubConfig {
    let raw = get_setting(pg, KEY, serde_json::Value::Object(Default::default())).await;
    let obj = raw.as_object().cloned().unwrap_or_default();
    let app = obj.get("app").and_then(|a| a.as_object()).cloned();
    let app = app.unwrap_or_default();

    // Legacy single-installation config migrates transparently on read.
    let mut installation_ids: Vec<String> = app
        .get("installationIds")
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    if installation_ids.is_empty()
        && let Some(legacy) = cfg_str(&app, "installationId")
    {
        installation_ids = vec![legacy];
    }

    GithubConfig {
        mode: cfg_str(&obj, "mode"),
        pat_token_enc: obj
            .get("pat")
            .and_then(|p| p.as_object())
            .and_then(|p| cfg_str(p, "tokenEnc")),
        app_id: cfg_str(&app, "appId").unwrap_or_default(),
        installation_ids,
        private_key_enc: cfg_str(&app, "privateKeyEnc"),
        repo_creation_orgs: obj
            .get("repoCreationOrgs")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default(),
    }
}

/// The patch merge, the seals, the setting write, and the
/// cache invalidation. A secret that arrives is sealed (an EMPTY string
/// clears); one that doesn't arrive keeps
/// its sealed predecessor. `installationIds`/`appId`/`repoCreationOrgs` use
/// `??` semantics — undefined keeps, any value (empty included) replaces.
pub async fn set_github_config(
    pg: &PgPool,
    sb: &SecretBox,
    patch: &GithubConfigPatch<'_>,
) -> Result<(), String> {
    let cur = get_github_config(pg).await;
    let seal_or_clear = |v: &str| -> Result<Option<String>, String> {
        if v.is_empty() {
            Ok(None)
        } else {
            sb.seal(v)
                .map(Some)
                .map_err(|e| format!("github seal: {e}"))
        }
    };
    let mode = match patch.mode {
        PatchField::Unset => cur.mode.clone(),
        PatchField::Set(m) => m.map(|s| s.to_string()),
    };
    let pat_token_enc = match &patch.pat_token {
        PatchField::Unset => cur.pat_token_enc.clone(),
        PatchField::Set(None) => None,
        PatchField::Set(Some(token)) => seal_or_clear(token)?,
    };
    let app_id = patch
        .app_id
        .as_ref()
        .map(|s| s.to_string())
        .unwrap_or(cur.app_id);
    let installation_ids = patch
        .installation_ids
        .as_ref()
        .map(|ids| ids.iter().map(|s| s.to_string()).collect())
        .unwrap_or(cur.installation_ids);
    let private_key_enc = match &patch.private_key {
        PatchField::Unset => cur.private_key_enc.clone(),
        PatchField::Set(None) => None,
        PatchField::Set(Some(key)) => seal_or_clear(key)?,
    };
    let repo_creation_orgs = patch
        .repo_creation_orgs
        .as_ref()
        .map(|orgs| orgs.iter().map(|s| s.to_string()).collect())
        .unwrap_or(cur.repo_creation_orgs);

    let next = serde_json::json!({
        "mode": mode,
        "pat": { "tokenEnc": pat_token_enc },
        "app": {
            "appId": app_id,
            "installationIds": installation_ids,
            "privateKeyEnc": private_key_enc,
        },
        "repoCreationOrgs": repo_creation_orgs,
    });
    set_setting(pg, KEY, &next)
        .await
        .map_err(|e| format!("github config write: {e}"))?;
    // Credentials changed — cached tokens/routing must never outlive them.
    install_tokens().lock().unwrap().clear();
    *repo_install_cache().lock().unwrap() = None;
    Ok(())
}

/// The three-state patch fields: Unset keeps the current value, Set(None)
/// clears, Set(Some) replaces. `app_id`/`installation_ids`/`repo_creation_
/// orgs` are plain Option — those keys carry no null-vs-undefined
/// distinction on the wire.
pub enum PatchField<T> {
    Unset,
    Set(Option<T>),
}

/// The workbench.github route's body, already zod-validated.
pub struct GithubConfigPatch<'a> {
    pub mode: PatchField<&'a str>,
    pub pat_token: PatchField<&'a str>,
    pub app_id: Option<&'a str>,
    pub installation_ids: Option<&'a [&'a str]>,
    pub private_key: PatchField<&'a str>,
    pub repo_creation_orgs: Option<&'a [&'a str]>,
}

async fn gh(
    path: &str,
    token: &str,
    method: &str,
    body: Option<&str>,
) -> Result<reqwest::Response, String> {
    let mut req = http()
        .request(
            reqwest::Method::from_bytes(method.as_bytes())
                .map_err(|_| format!("github method: {method}"))?,
            format!("{}{path}", api_base()),
        )
        .header("accept", "application/vnd.github+json")
        .header("x-github-api-version", "2022-11-28")
        .header("user-agent", "talaria-workbench")
        .header("authorization", format!("Bearer {token}"));
    if let Some(body) = body {
        req = req
            .header("content-type", "application/json")
            .body(body.to_string());
    }
    req.send().await.map_err(|e| format!("github request: {e}"))
}

async fn gh_json(
    path: &str,
    token: &str,
    method: &str,
    body: Option<&str>,
) -> Result<serde_json::Value, String> {
    let res = gh(path, token, method, body).await?;
    let status = res.status().as_u16();
    if !res.status().is_success() {
        let text = res.text().await.unwrap_or_default();
        return Err(format!(
            "GitHub {} {} → {}: {}",
            method,
            path,
            status,
            truncate_utf16(&text, 200)
        ));
    }
    res.json().await.map_err(|e| format!("github decode: {e}"))
}

/// Re-insert the line breaks a single-line input strips: the admin form's
/// password field runs the HTML value sanitizer, which deletes LF/CR from the
/// pasted value, so a real .pem arrives (and gets STORED, envelope-encrypted)
/// as `-----BEGIN RSA PRIVATE KEY-----base64…` on one line. That shape is
/// unambiguous, so repair it here at parse time — storage-time validation
/// could not heal the installs that already hold a stripped key. A PEM that
/// still has any line break, or does not match the BEGIN…END frame, passes
/// through untouched and fails parsing with its own honest error.
fn reline_pem(s: &str) -> String {
    if s.contains('\n') {
        return s.to_string();
    }
    let Some(rest) = s.strip_prefix("-----BEGIN ") else {
        return s.to_string();
    };
    let Some((label, rest)) = rest.split_once("-----") else {
        return s.to_string();
    };
    let Some((body, _)) = rest.split_once("-----END") else {
        return s.to_string();
    };
    let label = label.trim();
    let body: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    let mut out = format!("-----BEGIN {label}-----\n");
    for (i, c) in body.chars().enumerate() {
        if i > 0 && i % 64 == 0 {
            out.push('\n');
        }
        out.push(c);
    }
    out.push_str("\n-----END ");
    out.push_str(label);
    out.push_str("-----\n");
    out
}

/// RS256 app JWT — GitHub Apps authenticate to /app endpoints with this.
/// Header/payload are fixed literals in `JSON.stringify` key order; `iat` is
/// backdated 60s and `exp` is +9m.
/// `now_secs` is injectable so the vectors pin the bytes.
pub fn app_jwt_at(app_id: &str, private_key_pem: &str, now_secs: i64) -> Result<String, String> {
    let unsigned = format!(
        "{}.{}",
        URL_SAFE_NO_PAD.encode(br#"{"alg":"RS256","typ":"JWT"}"#),
        URL_SAFE_NO_PAD.encode(
            format!(
                r#"{{"iat":{},"exp":{},"iss":"{}"}}"#,
                now_secs - 60,
                now_secs + 9 * 60,
                app_id
            )
            .as_bytes()
        ),
    );
    // GitHub's App keys download as traditional `-----BEGIN RSA PRIVATE
    // KEY-----` (SEC1) PEMs; the TS signer handed the PEM to node's crypto,
    // which reads both labels. PKCS#8 first (the fixture's shape), then the
    // label GitHub actually ships — the same key either way.
    let pem = reline_pem(private_key_pem);
    let key = rsa::RsaPrivateKey::from_pkcs8_pem(&pem)
        .or_else(|_| rsa::RsaPrivateKey::from_pkcs1_pem(&pem))
        .map_err(|e| format!("github app key parse: {e}"))?;
    let signing = SigningKey::<Sha256>::new(key);
    let sig = signing.sign(unsigned.as_bytes());
    Ok(format!(
        "{}.{}",
        unsigned,
        URL_SAFE_NO_PAD.encode(sig.to_bytes())
    ))
}

pub fn app_jwt(app_id: &str, private_key_pem: &str) -> Result<String, String> {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    app_jwt_at(app_id, private_key_pem, now)
}

type TokenCache = Mutex<HashMap<String, (String, i64)>>;
fn install_tokens() -> &'static TokenCache {
    static CACHE: OnceLock<TokenCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The permissions a REPO-SCOPED token asks for: what a job's git push, a
/// branch create, a default-branch read and a pull-request open need, and
/// nothing an installation might also hold (administration, workflows,
/// secrets, packages). The sandbox's credential helper hands this token to
/// model-authored code; a permission it does not carry is a permission that
/// code cannot spend.
pub const REPO_TOKEN_PERMISSIONS: &str =
    r#"{"contents":"write","pull_requests":"write","metadata":"read"}"#;

/// The permissions a READ-ONLY repo token asks for: a clone or a fetch of the
/// repository and nothing else. App install and update use it
/// (talaria-apps): this instance reads a plugin's code from a repository the
/// GitHub connection can see, and must not be able to push to it.
pub const REPO_READ_PERMISSIONS: &str = r#"{"contents":"read","metadata":"read"}"#;

/// What a repo-scoped token may do. `Write` is the workbench's job token
/// ([`REPO_TOKEN_PERMISSIONS`]); `Read` a clone's ([`REPO_READ_PERMISSIONS`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoAccess {
    Write,
    Read,
}

impl RepoAccess {
    pub fn permissions(self) -> &'static str {
        match self {
            RepoAccess::Write => REPO_TOKEN_PERMISSIONS,
            RepoAccess::Read => REPO_READ_PERMISSIONS,
        }
    }
}

/// What `pr_status` reads with, and nothing it could write with: the pull
/// request, its check runs and commit statuses, and the reviews. Checks and
/// statuses are not in [`REPO_TOKEN_PERMISSIONS`] (a push never needs them),
/// so a PR's CI is unreadable on a private repo with that token. This one
/// never leaves the api process.
pub const PR_STATUS_PERMISSIONS: &str =
    r#"{"pull_requests":"read","checks":"read","statuses":"read","metadata":"read"}"#;

/// The body of an access_tokens request: the one repository, and — on the
/// first try — the narrowed permissions. GitHub answers 422 when the request
/// names a permission the installation was never granted, so the caller
/// retries once without the block and the token falls back to the
/// installation's own grants (logged, so an operator can see the App is
/// missing a permission the workbench expects).
pub fn access_tokens_body(repo: &str, narrowed: bool) -> String {
    access_tokens_body_for(repo, narrowed.then_some(REPO_TOKEN_PERMISSIONS))
}

fn access_tokens_body_for(repo: &str, permissions: Option<&str>) -> String {
    let name = repo.rsplit('/').next().unwrap_or(repo);
    let repos = serde_json::to_string(&[name]).expect("a repo name serializes");
    match permissions {
        Some(permissions) => {
            format!(r#"{{"repositories":{repos},"permissions":{permissions}}}"#)
        }
        None => format!(r#"{{"repositories":{repos}}}"#),
    }
}

/// Mint an installation access token. When `repo` is set the token is scoped
/// to THAT repository alone — without a body GitHub issues one valid for
/// EVERY repo in the installation, and this token ends up inside a sandbox
/// running model-authored code, so an unscoped one made a single leaked
/// credential equivalent to push access across the whole org. A repo-scoped
/// token is also narrowed to [`REPO_TOKEN_PERMISSIONS`], with one retry on
/// the 422 an under-granted installation answers; the installation-wide
/// token (`repo: None` — listing, repo creation) is not narrowed, because
/// those callers are the ones that need the App's wider grants and it never
/// enters a sandbox.
pub async fn installation_token(
    sb: &SecretBox,
    cfg: &GithubConfig,
    installation_id: &str,
    repo: Option<&str>,
) -> Result<String, String> {
    installation_token_with(sb, cfg, installation_id, repo, REPO_TOKEN_PERMISSIONS).await
}

/// [`installation_token`] for either access: `Write` is the workbench's job
/// token, `Read` a clone's ([`REPO_READ_PERMISSIONS`], never widened).
pub async fn installation_token_for(
    sb: &SecretBox,
    cfg: &GithubConfig,
    installation_id: &str,
    repo: Option<&str>,
    access: RepoAccess,
) -> Result<String, String> {
    installation_token_with(sb, cfg, installation_id, repo, access.permissions()).await
}

/// [`installation_token`] narrowed to `permissions` instead of the push
/// set. The cache keys on the permission set too: a read token must never
/// be served where a push token was asked for, or the other way round.
async fn installation_token_with(
    sb: &SecretBox,
    cfg: &GithubConfig,
    installation_id: &str,
    repo: Option<&str>,
    permissions: &str,
) -> Result<String, String> {
    let key = match repo {
        Some(r) => format!("{installation_id}#{r}#{permissions}"),
        None => installation_id.to_string(),
    };
    if let Some((token, expires_at)) = install_tokens().lock().unwrap().get(&key)
        && *expires_at > now_ms() + 60_000
    {
        return Ok(token.clone());
    }

    let jwt = app_jwt_for(sb, cfg)?;
    let base = api_base();

    let mint = |body: Option<String>| {
        let mut req = http()
            .post(format!(
                "{base}/app/installations/{installation_id}/access_tokens"
            ))
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", "2022-11-28")
            .header("user-agent", "talaria-workbench")
            .header("authorization", format!("Bearer {jwt}"));
        if let Some(body) = body {
            req = req.header("content-type", "application/json").body(body);
        }
        req.send()
    };
    let mut res = mint(repo.map(|r| access_tokens_body_for(r, Some(permissions))))
        .await
        .map_err(|e| format!("github installation token: {e}"))?;
    if res.status().as_u16() == 422
        && permissions != REPO_READ_PERMISSIONS
        && let Some(repo) = repo
    {
        // The narrowed ask named a permission this installation lacks (or
        // the repo is not in this installation — the retry answers that
        // too). Once more on the installation's own grants. A READ-ONLY
        // token is never widened this way: the installation's own grants may
        // include write, and a clone that cannot get a read-only token gets
        // no token.
        tracing::warn!(
            "[github] narrowed installation token for {repo} was refused (422) — retrying on the installation's own grants; check the App has {permissions}"
        );
        res = mint(Some(access_tokens_body_for(repo, None)))
            .await
            .map_err(|e| format!("github installation token: {e}"))?;
    }
    if !res.status().is_success() {
        // 422 here usually means the repo isn't in THIS installation — worth
        // saying so, because the caller picked the installation by cache
        // lookup.
        let suffix = repo.map(|r| format!(" for {r}")).unwrap_or_default();
        return Err(format!(
            "GitHub installation token failed ({}){suffix}",
            res.status().as_u16()
        ));
    }
    let j: serde_json::Value = res
        .json()
        .await
        .map_err(|e| format!("github installation token decode: {e}"))?;
    let token = j
        .get("token")
        .and_then(|v| v.as_str())
        .ok_or_else(|| "github installation token: no token in reply".to_string())?
        .to_string();
    // GitHub sends RFC 3339; the cache only needs a comparable instant. A
    // parse failure means "not cacheable", never "no token".
    let expires_at = j
        .get("expires_at")
        .and_then(|v| v.as_str())
        .and_then(talaria_tz::parse_rfc3339_ms)
        .unwrap_or(0);
    install_tokens()
        .lock()
        .unwrap()
        .insert(key, (token.clone(), expires_at));
    Ok(token)
}

#[derive(Clone)]
struct RepoInstallCache {
    map: HashMap<String, String>,
    at: i64,
}

fn repo_install_cache() -> &'static Mutex<Option<RepoInstallCache>> {
    static CACHE: OnceLock<Mutex<Option<RepoInstallCache>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(None))
}

/// Repos the connection can reach — the union across all selected
/// installations (multi-org), paged, capped at 5 pages per source. The
/// listing doubles as the repo→installation routing table for token minting;
/// the cache lives for 10 minutes.
pub async fn list_reachable_repos(pg: &PgPool, sb: &SecretBox) -> Vec<String> {
    let cfg = get_github_config(pg).await;
    let mut map: HashMap<String, String> = HashMap::new();
    let mut out: Vec<String> = Vec::new();
    if cfg.mode.as_deref() == Some("pat") {
        if let Ok(Some(token)) = pat_token(sb, &cfg) {
            for repo in paged_repos(&token, "/user/repos?per_page=100&sort=pushed").await {
                out.push(repo);
            }
        }
        return out;
    }
    if cfg.mode.as_deref() != Some("app") || cfg.installation_ids.is_empty() {
        return out;
    }
    for inst in &cfg.installation_ids {
        // A failing installation is skipped, not fatal: the union covers the
        // rest, and one broken org must not blind the routing table.
        let Ok(token) = installation_token(sb, &cfg, inst, None).await else {
            continue;
        };
        for repo in paged_repos(&token, "/installation/repositories?per_page=100").await {
            if !map.contains_key(&repo) {
                map.insert(repo.clone(), inst.clone());
                out.push(repo);
            }
        }
    }
    *repo_install_cache().lock().unwrap() = Some(RepoInstallCache { map, at: now_ms() });
    out
}

async fn paged_repos(token: &str, first_path: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut path = first_path.to_string();
    let mut page = 0;
    while page < 5 && !path.is_empty() {
        let Ok(res) = gh(&path, token, "GET", None).await else {
            break;
        };
        if !res.status().is_success() {
            break;
        }
        let link = res
            .headers()
            .get("link")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let Ok(j) = res.json::<serde_json::Value>().await else {
            break;
        };
        // /user/repos answers an array; /installation/repositories wraps one.
        let repos: Vec<String> = match &j {
            serde_json::Value::Array(items) => items
                .iter()
                .filter_map(|r| r.get("full_name").and_then(|v| v.as_str()))
                .map(|s| s.to_string())
                .collect(),
            _ => j
                .get("repositories")
                .and_then(|v| v.as_array())
                .map(|items| {
                    items
                        .iter()
                        .filter_map(|r| r.get("full_name").and_then(|v| v.as_str()))
                        .map(|s| s.to_string())
                        .collect()
                })
                .unwrap_or_default(),
        };
        out.extend(repos);
        path = next_link_path(&link).unwrap_or_default();
        page += 1;
    }
    out
}

/// `<https://api.github.com/...>; rel="next"` → the path. The regex anchors
/// on the api.github.com origin; the path is what `gh()` wants.
fn next_link_path(link: &str) -> Option<String> {
    let re = regex::Regex::new(r#"<https://api\.github\.com([^>]+)>;\s*rel="next""#).ok()?;
    re.captures(link)
        .and_then(|c| c.get(1))
        .map(|m| m.as_str().to_string())
}

/// The PAT passthrough both the broad token and `github_token(None)` take.
fn pat_token(sb: &SecretBox, cfg: &GithubConfig) -> Result<Option<String>, String> {
    match &cfg.pat_token_enc {
        Some(enc) => sb
            .open(enc)
            .map(Some)
            .map_err(|e| format!("github pat unseal: {e}")),
        None => Ok(None),
    }
}

/// The App JWT for this connection — the credential the `/app` and
/// `/orgs/{org}/installation` endpoints take.
fn app_jwt_for(sb: &SecretBox, cfg: &GithubConfig) -> Result<String, String> {
    let private_key_enc = cfg.private_key_enc.as_deref().unwrap_or_default();
    let key = sb
        .open(private_key_enc)
        .map_err(|e| format!("github app key unseal: {e}"))?;
    app_jwt(&cfg.app_id, &key)
}

/// A usable token for repo operations. PAT passes through; App mode mints a
/// per-installation token — `repo` routes a multi-org setup to the
/// installation that owns it AND scopes the token to that one repository.
/// Callers that need installation-wide reach omit `repo` and get the broad
/// token. PAT mode cannot be scoped at all; that blast radius is the org's
/// own choice.
pub async fn github_token(
    pg: &PgPool,
    sb: &SecretBox,
    repo: Option<&str>,
) -> Result<Option<String>, String> {
    repo_token(pg, sb, repo, REPO_TOKEN_PERMISSIONS).await
}

/// [`github_token`] for either access. In App mode a `repo` token is narrowed
/// to `access` ([`REPO_READ_PERMISSIONS`] for `Read`); PAT mode passes the PAT
/// through whatever the access, because a PAT cannot be narrowed.
pub async fn github_token_for(
    pg: &PgPool,
    sb: &SecretBox,
    repo: Option<&str>,
    access: RepoAccess,
) -> Result<Option<String>, String> {
    repo_token(pg, sb, repo, access.permissions()).await
}

/// [`github_token`] with the repo-scoped token narrowed to `permissions`.
async fn repo_token(
    pg: &PgPool,
    sb: &SecretBox,
    repo: Option<&str>,
    permissions: &str,
) -> Result<Option<String>, String> {
    let cfg = get_github_config(pg).await;
    if cfg.mode.as_deref() == Some("pat") {
        return pat_token(sb, &cfg);
    }
    if cfg.mode.as_deref() != Some("app")
        || cfg.app_id.is_empty()
        || cfg.installation_ids.is_empty()
        || cfg.private_key_enc.is_none()
    {
        return Ok(None);
    }
    let first = cfg.installation_ids[0].clone();
    let mut installation_id = first;
    if let Some(repo) = repo
        && cfg.installation_ids.len() > 1
    {
        let stale = repo_install_cache()
            .lock()
            .unwrap()
            .as_ref()
            .map(|c| now_ms() - c.at > 10 * 60_000)
            .unwrap_or(true);
        if stale {
            list_reachable_repos(pg, sb).await;
        }
        if let Some(hit) = repo_install_cache().lock().unwrap().as_ref()
            && let Some(inst) = hit.map.get(repo)
        {
            installation_id = inst.clone();
        }
    }
    Ok(Some(
        installation_token_with(sb, &cfg, &installation_id, repo, permissions).await?,
    ))
}

/// The repos an agent was actually granted — the workbench grant table.
/// Connecting GitHub grants nothing to anyone by itself.
pub async fn granted_repos(pg: &PgPool, agent_id: &str) -> Vec<String> {
    let Ok(rows) = sqlx::query_scalar::<_, String>(
        "select repo from workbench_repos where agent_id = $1::uuid order by repo",
    )
    .bind(agent_id)
    .fetch_all(pg)
    .await
    else {
        return Vec::new();
    };
    rows
}

/// Atomic grant replace — a crash can never leave the agent grantless by
/// accident. `keep` is capped at 100.
pub async fn set_granted_repos(pg: &PgPool, agent_id: &str, keep: &[String]) -> Result<(), String> {
    let keep = &keep[..keep.len().min(100)];
    let mut tx = pg
        .begin()
        .await
        .map_err(|e| format!("workbench grant tx: {e}"))?;
    // SET semantics, not replace-by-delete: rows that survive keep their
    // hygiene rules, rows that leave go away, rows that arrive start at the
    // defaults. A delete-everything write would wipe an admin's branch rules
    // on every chip toggle.
    sqlx::query("delete from workbench_repos where agent_id = $1::uuid and not (repo = any($2))")
        .bind(agent_id)
        .bind(keep)
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("workbench grant clear: {e}"))?;
    if !keep.is_empty() {
        sqlx::query(
            "insert into workbench_repos (agent_id, repo) \
             select $1::uuid, unnest($2::text[]) on conflict do nothing",
        )
        .bind(agent_id)
        .bind(keep)
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("workbench grant write: {e}"))?;
    }
    tx.commit()
        .await
        .map_err(|e| format!("workbench grant commit: {e}"))
}

// ── Repo hygiene rules ───────────────────────────────────────────────────────
//
// The per-grant branch law: work lives on a branch, a human merges. The
// DEFAULT (columns null/'branches_only') already enforces that — an agent
// with a grant and no configuration may push any branch except the base.
// Configuration narrows or widens: name the base explicitly (a repo whose
// default branch is not the integration target), require a prefix for the
// agent's branches, or explicitly trust an agent with the base ('free').
// GitHub's branch protection enforces the same posture where the plan
// allows it; this is the platform's OWN gate, which works for every repo
// including the private ones a free plan cannot protect.

#[derive(Debug, Clone, Default, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoRule {
    pub repo: String,
    /// The integration branch — never pushable in branches_only mode. Null
    /// means "the repo's default branch", resolved live at check time.
    pub base_branch: Option<String>,
    /// 'branches_only' (the default posture) or 'free'.
    pub push_mode: String,
    /// When set, the agent's branches must start with this prefix.
    pub branch_prefix: Option<String>,
}

pub async fn repo_rules(pg: &PgPool, agent_id: &str) -> Vec<RepoRule> {
    let Ok(rows) = sqlx::query_as::<_, (String, Option<String>, String, Option<String>)>(
        "select repo, base_branch, push_mode, branch_prefix from workbench_repos \
         where agent_id = $1::uuid order by repo",
    )
    .bind(agent_id)
    .fetch_all(pg)
    .await
    else {
        return Vec::new();
    };
    rows.into_iter()
        .map(|(repo, base_branch, push_mode, branch_prefix)| RepoRule {
            repo,
            base_branch,
            push_mode,
            branch_prefix,
        })
        .collect()
}

/// The one-row read the push check needs. None = no grant, which the caller
/// answers exactly like the credential route's no-credential case.
pub async fn repo_rule(pg: &PgPool, agent_id: &str, repo: &str) -> Option<RepoRule> {
    sqlx::query_as::<_, (String, Option<String>, String, Option<String>)>(
        "select repo, base_branch, push_mode, branch_prefix from workbench_repos \
         where agent_id = $1::uuid and repo = $2",
    )
    .bind(agent_id)
    .bind(repo)
    .fetch_optional(pg)
    .await
    .ok()
    .flatten()
    .map(|r| RepoRule {
        repo: r.0,
        base_branch: r.1,
        push_mode: r.2,
        branch_prefix: r.3,
    })
}

/// Write one grant's rules. The repo must already be granted (the route
/// validates); values are trusted to the route's own validation.
pub async fn set_repo_rule(pg: &PgPool, agent_id: &str, rule: &RepoRule) -> Result<(), String> {
    sqlx::query(
        "update workbench_repos set base_branch = $3, push_mode = $4, branch_prefix = $5 \
         where agent_id = $1::uuid and repo = $2",
    )
    .bind(agent_id)
    .bind(&rule.repo)
    .bind(&rule.base_branch)
    .bind(if rule.push_mode == "free" {
        "free"
    } else {
        "branches_only"
    })
    .bind(&rule.branch_prefix)
    .execute(pg)
    .await
    .map_err(|e| format!("repo rule write: {e}"))?;
    Ok(())
}

/// Branches no agent pushes under `branches_only`, whatever the rule names
/// as its base. A repo whose flow base is `rc` (this repository's own model)
/// still has a `main` that promotion owns; a rule that only refused the
/// configured base let a push to `main` through. `free` is the operator's
/// explicit opt-out and keeps its meaning.
pub const PROTECTED_BRANCHES: [&str; 3] = ["main", "master", "rc"];

/// The decision the pre-push hook and the brief both read. Pure: given the
/// rule, the repo's resolved default branch (for a null base_branch), and a
/// ref being pushed, allow or refuse with a sentence for the agent.
///
/// Refused under `branches_only`: the configured base, the repo's default
/// branch (even when a base is configured — the default is where the
/// repo's own promotion lands), and [`PROTECTED_BRANCHES`].
///
/// THIS IS A CLIENT-SIDE GATE. It answers the rendered pre-push hook inside
/// the sandbox, and git skips a pre-push hook on `--no-verify`. The token
/// the sandbox holds is narrowed to contents/pull_requests/metadata, but
/// contents:write is what pushing a branch takes, so the base branch is
/// only actually safe when the repository's own branch protection says so.
/// This rule is the agent's instruction and the operator's log line, not
/// the lock on the door.
pub fn push_allowed(rule: &RepoRule, default_branch: &str, ref_name: &str) -> Result<(), String> {
    if rule.push_mode == "free" {
        return Ok(());
    }
    let branch = ref_name.trim_start_matches("refs/heads/");
    let base = rule.base_branch.as_deref().unwrap_or(default_branch);
    if branch == base {
        return Err(format!(
            "{base} is the base branch for {} — your work belongs on your own branch and a human merges it",
            rule.repo
        ));
    }
    if !default_branch.is_empty() && branch == default_branch {
        return Err(format!(
            "{default_branch} is the default branch of {} — your work belongs on your own branch and a human merges it",
            rule.repo
        ));
    }
    if PROTECTED_BRANCHES.contains(&branch) {
        return Err(format!(
            "{branch} is a protected branch of {} — your work belongs on your own branch and a human merges it",
            rule.repo
        ));
    }
    if let Some(prefix) = rule.branch_prefix.as_deref().filter(|p| !p.is_empty())
        && !branch.starts_with(prefix)
    {
        return Err(format!(
            "branches for {} must start with \"{prefix}\" — got \"{branch}\"",
            rule.repo
        ));
    }
    Ok(())
}

/// A repo's default branch, via the installation — the base a null
/// base_branch resolves to at check time.
pub async fn repo_default_branch(pg: &PgPool, sb: &SecretBox, repo: &str) -> Option<String> {
    let token = github_token(pg, sb, Some(repo)).await.ok()??;
    let reply = gh_json(&format!("/repos/{repo}"), &token, "GET", None)
        .await
        .ok()?;
    reply
        .get("default_branch")
        .and_then(|v| v.as_str())
        .map(String::from)
}

/// A repo's branch names for the admin picker — the live list, capped.
pub async fn list_branches(pg: &PgPool, sb: &SecretBox, repo: &str) -> Vec<String> {
    let Some(token) = github_token(pg, sb, Some(repo)).await.ok().flatten() else {
        return Vec::new();
    };
    let Ok(reply) = gh_json(
        &format!("/repos/{repo}/branches?per_page=100"),
        &token,
        "GET",
        None,
    )
    .await
    else {
        return Vec::new();
    };
    reply
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|b| b.get("name").and_then(|n| n.as_str()).map(String::from))
                .collect()
        })
        .unwrap_or_default()
}

// ── The admin/status plane (workbench.github, the doctor verb) ────────────────

/// Redacted, live-verified, never leaking secrets.
/// Field order is the wire's: base first (mode, app, patSet,
/// repoCreationOrgs), then the appended configured/account/error.
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubStatus {
    pub mode: Option<String>,
    pub app: GithubStatusApp,
    pub pat_set: bool,
    pub repo_creation_orgs: Vec<String>,
    pub configured: bool,
    pub account: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct GithubStatusApp {
    pub app_id: String,
    pub installation_ids: Vec<String>,
    pub key_set: bool,
}

/// PAT login or App name — who the connection acts as, when verifiable.
pub async fn github_status(pg: &PgPool, sb: &SecretBox) -> GithubStatus {
    let cfg = get_github_config(pg).await;
    let base = GithubStatus {
        mode: cfg.mode.clone(),
        app: GithubStatusApp {
            app_id: cfg.app_id.clone(),
            installation_ids: cfg.installation_ids.clone(),
            key_set: cfg.private_key_enc.is_some(),
        },
        pat_set: cfg.pat_token_enc.is_some(),
        repo_creation_orgs: cfg.repo_creation_orgs.clone(),
        configured: false,
        account: None,
        error: None,
    };
    let configured =
        |configured: bool, account: Option<String>, error: Option<String>| GithubStatus {
            configured,
            account,
            error,
            ..base.clone()
        };
    if cfg.mode.as_deref() == Some("pat")
        && let Some(enc) = cfg.pat_token_enc.clone()
    {
        let token = match sb.open(&enc) {
            Ok(t) => t,
            Err(e) => {
                let msg = format!("github pat unseal: {e}");
                return configured(false, None, Some(truncate_utf16(&msg, 200).to_string()));
            }
        };
        return match gh("/user", &token, "GET", None).await {
            Ok(res) if !res.status().is_success() => configured(
                true,
                None,
                Some(format!("token rejected ({})", res.status().as_u16())),
            ),
            Ok(res) => match res.json::<serde_json::Value>().await {
                Ok(j) => {
                    let login = j
                        .get("login")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    configured(true, login, None)
                }
                Err(e) => configured(
                    false,
                    None,
                    Some(truncate_utf16(&format!("github decode: {e}"), 200).to_string()),
                ),
            },
            Err(e) => configured(false, None, Some(truncate_utf16(&e, 200).to_string())),
        };
    }
    if cfg.mode.as_deref() == Some("app")
        && !cfg.app_id.is_empty()
        && let Some(enc) = cfg.private_key_enc.clone()
    {
        let key = match sb.open(&enc) {
            Ok(k) => k,
            Err(e) => {
                let msg = format!("github app key unseal: {e}");
                return configured(false, None, Some(truncate_utf16(&msg, 200).to_string()));
            }
        };
        let jwt = match app_jwt(&cfg.app_id, &key) {
            Ok(j) => j,
            Err(e) => {
                return configured(false, None, Some(truncate_utf16(&e, 200).to_string()));
            }
        };
        return match gh("/app", &jwt, "GET", None).await {
            Ok(res) if !res.status().is_success() => configured(
                true,
                None,
                Some(format!(
                    "app credentials rejected ({})",
                    res.status().as_u16()
                )),
            ),
            Ok(res) => match res.json::<serde_json::Value>().await {
                Ok(j) => {
                    let name = j
                        .get("name")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    if cfg.installation_ids.is_empty() {
                        return configured(
                            false,
                            name,
                            Some("pick at least one installation".into()),
                        );
                    }
                    configured(true, name, None)
                }
                Err(e) => configured(
                    false,
                    None,
                    Some(truncate_utf16(&format!("github decode: {e}"), 200).to_string()),
                ),
            },
            Err(e) => configured(false, None, Some(truncate_utf16(&e, 200).to_string())),
        };
    }
    // Unconfigured (or configured-but-incomplete without a live check): the
    // calm null error.
    base
}

/// App mode's easy-setup helper: once appId+key verify, list where the app
/// is installed so the admin picks the installation instead of hunting an
/// id. Any failure is an empty list, never a throw.
pub async fn list_installations(pg: &PgPool, sb: &SecretBox) -> Vec<(i64, String)> {
    let cfg = get_github_config(pg).await;
    if cfg.app_id.is_empty() {
        return Vec::new();
    }
    let Some(enc) = cfg.private_key_enc else {
        return Vec::new();
    };
    let Ok(key) = sb.open(&enc) else {
        return Vec::new();
    };
    let Ok(jwt) = app_jwt(&cfg.app_id, &key) else {
        return Vec::new();
    };
    let Ok(res) = gh("/app/installations", &jwt, "GET", None).await else {
        return Vec::new();
    };
    if !res.status().is_success() {
        return Vec::new();
    }
    let Ok(j) = res.json::<serde_json::Value>().await else {
        return Vec::new();
    };
    j.as_array()
        .map(|items| {
            items
                .iter()
                .filter_map(|i| {
                    let id = i.get("id").and_then(|v| v.as_i64())?;
                    let account = i
                        .get("account")
                        .and_then(|a| a.get("login"))
                        .and_then(|v| v.as_str())?
                        .to_string();
                    Some((id, account))
                })
                .collect()
        })
        .unwrap_or_default()
}

// ── Per-repo git flow (the branch PRs target) ────────────────────────────────

/// workbench_repo_flow's row — `{repo, baseBranch}` on the wire, a null
/// branch meaning "not set".
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoFlow {
    pub repo: String,
    pub base_branch: Option<String>,
}

type FlowRow = (String, Option<String>);

pub async fn repo_flow(pg: &PgPool, repo: &str) -> Result<RepoFlow, String> {
    let row: Option<FlowRow> =
        sqlx::query_as("select repo, base_branch from workbench_repo_flow where repo = $1")
            .bind(repo)
            .fetch_optional(pg)
            .await
            .map_err(|e| format!("repo flow read: {e}"))?;
    Ok(row.unwrap_or((repo.to_string(), None)).into())
}

impl From<FlowRow> for RepoFlow {
    fn from(r: FlowRow) -> Self {
        RepoFlow {
            repo: r.0,
            base_branch: r.1,
        }
    }
}

pub async fn list_repo_flows(pg: &PgPool) -> Result<Vec<RepoFlow>, String> {
    let rows: Vec<FlowRow> =
        sqlx::query_as("select repo, base_branch from workbench_repo_flow order by repo")
            .fetch_all(pg)
            .await
            .map_err(|e| format!("repo flows read: {e}"))?;
    Ok(rows.into_iter().map(Into::into).collect())
}

/// Upsert one repo's flow. `PatchField`-shaped semantics via Option<Option>:
/// None keeps the current value, Some(None) clears it.
pub async fn set_repo_flow(
    pg: &PgPool,
    repo: &str,
    base_branch: Option<Option<String>>,
) -> Result<(), String> {
    let cur = repo_flow(pg, repo).await?;
    let base = base_branch.unwrap_or(cur.base_branch);
    sqlx::query(
        "insert into workbench_repo_flow (repo, base_branch) \
         values ($1, $2) \
         on conflict (repo) do update set \
           base_branch = excluded.base_branch, updated_at = now()",
    )
    .bind(repo)
    .bind(base)
    .execute(pg)
    .await
    .map_err(|e| format!("repo flow write: {e}"))?;
    Ok(())
}

/// The branch jobs cut from and PRs target — the flow override, else default.
pub async fn effective_base(pg: &PgPool, sb: &SecretBox, repo: &str) -> Result<String, String> {
    let flow = repo_flow(pg, repo).await?;
    match flow.base_branch {
        Some(base) => Ok(base),
        None => default_branch(pg, sb, repo).await,
    }
}

async fn default_branch(pg: &PgPool, sb: &SecretBox, repo: &str) -> Result<String, String> {
    let token = github_token(pg, sb, Some(repo))
        .await?
        .ok_or("GitHub is not connected")?;
    let j = gh_json(&format!("/repos/{repo}"), &token, "GET", None).await?;
    j.get("default_branch")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .ok_or_else(|| "github default branch: none in reply".into())
}

/// encodeURIComponent — the subset GitHub ref paths need (segment-internal
/// slashes stay literal in `a...b` compares; ref names encode whole).
fn enc(segment: &str) -> String {
    let mut out = String::new();
    for b in segment.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

// ── Repo creation (request_repo → a human approves → the App creates) ───────

/// Who may see a repository Talaria creates. Private unless the request or
/// the approver says otherwise.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum RepoVisibility {
    #[default]
    Private,
    Public,
}

impl RepoVisibility {
    pub fn parse(s: &str) -> Option<RepoVisibility> {
        match s {
            "private" => Some(RepoVisibility::Private),
            "public" => Some(RepoVisibility::Public),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            RepoVisibility::Private => "private",
            RepoVisibility::Public => "public",
        }
    }
}

/// Why this connection cannot create repos at all, or None when it can try.
/// The whole decision `request_repo`, `doctor` and the approval share, so an
/// agent is refused for the same reason the approval would have failed with.
pub fn repo_creation_blocker(cfg: &GithubConfig) -> Option<String> {
    if cfg.repo_creation_orgs.is_empty() {
        return Some(
            "repo creation is off — no org is approved for it (an admin lists orgs on the GitHub panel in Admin → Org)"
                .into(),
        );
    }
    if cfg.mode.as_deref() != Some("app") {
        return Some(
            "repo creation needs the GitHub App connection, and this instance connects GitHub with a personal access token — an admin can switch the GitHub panel in Admin → Org to App mode"
                .into(),
        );
    }
    if cfg.app_id.is_empty() || cfg.private_key_enc.is_none() || cfg.installation_ids.is_empty() {
        return Some(
            "the GitHub App connection is incomplete (App ID, private key and at least one installation are needed) — an admin finishes it in Admin → Org"
                .into(),
        );
    }
    None
}

/// [`repo_creation_blocker`], then the org allowlist.
pub fn repo_creation_refusal(cfg: &GithubConfig, org: &str) -> Option<String> {
    if let Some(why) = repo_creation_blocker(cfg) {
        return Some(why);
    }
    if !cfg.repo_creation_orgs.iter().any(|o| o == org) {
        return Some(format!(
            "org \"{org}\" is not approved for repo creation (approved: {})",
            cfg.repo_creation_orgs.join(", ")
        ));
    }
    None
}

/// Where the App stands on one org, asked of GitHub rather than inferred
/// from a cache of repos (an org with no repos yet never appeared in one).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OrgInstallation {
    /// Installed there, and that installation is one this connection uses.
    Selected(String),
    /// Installed there, but the admin has not selected that installation.
    NotSelected(String),
    /// GitHub knows no installation of this App on that org.
    NotInstalled,
}

impl OrgInstallation {
    /// The sentence a refusal carries, or None when repos can be created.
    pub fn refusal(&self, org: &str) -> Option<String> {
        match self {
            OrgInstallation::Selected(_) => None,
            OrgInstallation::NotSelected(id) => Some(format!(
                "the GitHub App is installed on {org} (installation {id}), but this connection does not use that installation — an admin selects it on the GitHub panel in Admin → Org"
            )),
            OrgInstallation::NotInstalled => Some(format!(
                "the GitHub App is not installed on {org} — install it on that org from the App's GitHub page, then select the installation in Admin → Org"
            )),
        }
    }
}

/// `GET /orgs/{org}/installation` with the App JWT: the one installation
/// that owns that org. Picking "any installation" is how a token minted for
/// one org was sent to create a repo in another.
async fn org_installation_with(
    jwt: &str,
    org: &str,
    selected: &[String],
) -> Result<OrgInstallation, String> {
    let res = gh(
        &format!("/orgs/{}/installation", enc(org)),
        jwt,
        "GET",
        None,
    )
    .await?;
    let status = res.status().as_u16();
    if status == 404 {
        return Ok(OrgInstallation::NotInstalled);
    }
    if !res.status().is_success() {
        let text = res.text().await.unwrap_or_default();
        return Err(format!(
            "GitHub GET /orgs/{org}/installation → {status}: {}",
            truncate_utf16(&text, 200)
        ));
    }
    let j: serde_json::Value = res
        .json()
        .await
        .map_err(|e| format!("github decode: {e}"))?;
    let id = match j.get("id") {
        Some(serde_json::Value::Number(n)) => n.to_string(),
        Some(serde_json::Value::String(s)) => s.clone(),
        _ => {
            return Err(format!(
                "GitHub GET /orgs/{org}/installation: no id in reply"
            ));
        }
    };
    Ok(if selected.iter().any(|s| s == &id) {
        OrgInstallation::Selected(id)
    } else {
        OrgInstallation::NotSelected(id)
    })
}

/// Where the App stands on `org`. Err is GitHub or the connection failing,
/// never an answer about the org.
pub async fn org_installation(
    pg: &PgPool,
    sb: &SecretBox,
    org: &str,
) -> Result<OrgInstallation, String> {
    let cfg = get_github_config(pg).await;
    let jwt = app_jwt_for(sb, &cfg)?;
    org_installation_with(&jwt, org, &cfg.installation_ids).await
}

/// Create a repo in an org (the App must hold the org's Administration
/// permission), with the token of the installation GitHub says owns that
/// org. A missing or unselected installation is refused with the fix.
pub async fn create_repo(
    pg: &PgPool,
    sb: &SecretBox,
    org: &str,
    name: &str,
    description: &str,
    visibility: RepoVisibility,
) -> Result<CreatedRepo, String> {
    let cfg = get_github_config(pg).await;
    let created = create_repo_in(sb, &cfg, org, name, description, visibility).await?;
    // Pool changed.
    *repo_install_cache().lock().unwrap() = None;
    Ok(created)
}

async fn create_repo_in(
    sb: &SecretBox,
    cfg: &GithubConfig,
    org: &str,
    name: &str,
    description: &str,
    visibility: RepoVisibility,
) -> Result<CreatedRepo, String> {
    if cfg.mode.as_deref() != Some("app") {
        return Err("repo creation requires the GitHub App connection".into());
    }
    let jwt = app_jwt_for(sb, cfg)?;
    let installation = match org_installation_with(&jwt, org, &cfg.installation_ids).await? {
        OrgInstallation::Selected(id) => id,
        other => return Err(other.refusal(org).unwrap_or_default()),
    };
    let token = installation_token(sb, cfg, &installation, None).await?;
    let body = serde_json::json!({
        "name": name,
        "description": truncate_utf16(description, 300),
        "private": visibility == RepoVisibility::Private,
        "auto_init": true,
    })
    .to_string();
    let res = gh(
        &format!("/orgs/{}/repos", enc(org)),
        &token,
        "POST",
        Some(&body),
    )
    .await?;
    if res.status().as_u16() == 403 {
        return Err(format!(
            "the App's installation on {org} lacks the org's Administration permission. Grant it in the App settings, then re-approve."
        ));
    }
    if !res.status().is_success() {
        let status = res.status().as_u16();
        let text = res.text().await.unwrap_or_default();
        return Err(format!(
            "GitHub repo creation failed ({status}): {}",
            truncate_utf16(&text, 200)
        ));
    }
    let j: serde_json::Value = res
        .json()
        .await
        .map_err(|e| format!("github decode: {e}"))?;
    Ok(CreatedRepo {
        full_name: j
            .get("full_name")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        url: j
            .get("html_url")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
    })
}

#[derive(Debug)]
pub struct CreatedRepo {
    pub full_name: String,
    pub url: String,
}

// ── Repo operations (the platform-owned git flow) ────────────────────────────

/// Cut a branch from the base's head. Idempotent-ish: an existing branch of
/// the same name is left alone (the job resumes on it).
pub async fn create_branch(
    pg: &PgPool,
    sb: &SecretBox,
    repo: &str,
    branch: &str,
    base_override: Option<&str>,
) -> Result<CreatedBranch, String> {
    let token = github_token(pg, sb, Some(repo))
        .await?
        .ok_or("GitHub is not connected")?;
    let base = match base_override {
        Some(b) => b.to_string(),
        None => effective_base(pg, sb, repo).await?,
    };
    let existing = gh(
        &format!("/repos/{repo}/git/ref/heads/{}", enc(branch)),
        &token,
        "GET",
        None,
    )
    .await?;
    if existing.status().is_success() {
        return Ok(CreatedBranch {
            base,
            created: false,
        });
    }
    let head = gh_json(
        &format!("/repos/{repo}/git/ref/heads/{}", enc(&base)),
        &token,
        "GET",
        None,
    )
    .await?;
    let sha = head
        .get("object")
        .and_then(|o| o.get("sha"))
        .and_then(|v| v.as_str())
        .ok_or("github ref read: no sha in reply")?;
    gh_json(
        &format!("/repos/{repo}/git/refs"),
        &token,
        "POST",
        Some(&format!(r#"{{"ref":"refs/heads/{branch}","sha":"{sha}"}}"#)),
    )
    .await?;
    Ok(CreatedBranch {
        base,
        created: true,
    })
}

pub struct CreatedBranch {
    pub base: String,
    pub created: bool,
}

pub async fn branch_ahead(
    pg: &PgPool,
    sb: &SecretBox,
    repo: &str,
    base: &str,
    branch: &str,
) -> Result<i64, String> {
    let token = github_token(pg, sb, Some(repo))
        .await?
        .ok_or("GitHub is not connected")?;
    let cmp = gh_json(
        &format!("/repos/{repo}/compare/{}...{}", enc(base), enc(branch)),
        &token,
        "GET",
        None,
    )
    .await?;
    Ok(cmp.get("ahead_by").and_then(|v| v.as_i64()).unwrap_or(0))
}

// ── Pull requests: open one, or keep using the one the branch already has ───

/// A pull request by its number and page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestRef {
    pub url: String,
    pub number: i64,
}

fn pr_ref_of(pr: &serde_json::Value) -> Option<PullRequestRef> {
    Some(PullRequestRef {
        number: pr.get("number").and_then(|v| v.as_i64())?,
        url: pr
            .get("html_url")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
    })
}

/// The number at the end of a PR page URL (`…/pull/42` → 42) — how a job
/// row written before `pr_number` existed still names its PR.
pub fn pr_number_from_url(url: &str) -> Option<i64> {
    let (_, tail) = url.trim_end_matches('/').rsplit_once("/pull/")?;
    tail.parse().ok().filter(|n| *n > 0)
}

/// The open PR whose head is `branch` in `repo`, if there is one. GitHub
/// allows one open PR per head and base, so a branch with an open PR is a
/// branch whose next push already updates it.
async fn find_open_pull_request_with(
    token: &str,
    repo: &str,
    branch: &str,
) -> Result<Option<PullRequestRef>, String> {
    let owner = repo.split('/').next().unwrap_or(repo);
    let reply = gh_json(
        &format!(
            "/repos/{repo}/pulls?state=open&per_page=1&head={}",
            enc(&format!("{owner}:{branch}"))
        ),
        token,
        "GET",
        None,
    )
    .await?;
    Ok(reply
        .as_array()
        .and_then(|prs| prs.first())
        .and_then(pr_ref_of))
}

pub async fn find_open_pull_request(
    pg: &PgPool,
    sb: &SecretBox,
    repo: &str,
    branch: &str,
) -> Result<Option<PullRequestRef>, String> {
    let token = github_token(pg, sb, Some(repo))
        .await?
        .ok_or("GitHub is not connected")?;
    find_open_pull_request_with(&token, repo, branch).await
}

/// What `finish_job` asks for: the PR to open, and the note to leave on the
/// PR instead when the branch already has one.
pub struct PullRequestAsk<'a> {
    pub head: &'a str,
    pub base: &'a str,
    pub title: &'a str,
    pub body: &'a str,
    pub draft: bool,
    /// Posted as a comment on an existing PR, so a revise round's summary
    /// lands where its reviewer reads.
    pub update_note: &'a str,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenedPullRequest {
    pub url: String,
    pub number: i64,
    /// The branch already had this PR open, so none was created.
    pub reused: bool,
    /// The update note landed as a PR comment (only ever true when reused).
    pub commented: bool,
}

/// Open the PR for `ask.head` — or, when the branch already has an open one
/// (a revise round pushing to the PR a reviewer bounced), keep that PR and
/// comment on it. Opening a second PR for the same head is a 422 from
/// GitHub, and that refusal left the job `started` with nothing to show.
pub async fn open_or_update_pull_request(
    pg: &PgPool,
    sb: &SecretBox,
    repo: &str,
    ask: &PullRequestAsk<'_>,
) -> Result<OpenedPullRequest, String> {
    let token = github_token(pg, sb, Some(repo))
        .await?
        .ok_or("GitHub is not connected")?;
    open_or_update_pull_request_with(&token, repo, ask).await
}

async fn open_or_update_pull_request_with(
    token: &str,
    repo: &str,
    ask: &PullRequestAsk<'_>,
) -> Result<OpenedPullRequest, String> {
    if let Some(pr) = find_open_pull_request_with(token, repo, ask.head).await? {
        return Ok(reuse_pull_request(token, repo, pr, ask.update_note).await);
    }
    let body = serde_json::json!({
        "head": ask.head,
        "base": ask.base,
        "title": ask.title,
        "body": ask.body,
        "draft": ask.draft,
    })
    .to_string();
    let res = gh(&format!("/repos/{repo}/pulls"), token, "POST", Some(&body)).await?;
    let status = res.status().as_u16();
    if !res.status().is_success() {
        let text = res.text().await.unwrap_or_default();
        // Opened between the lookup and this POST: GitHub's 422 says a PR
        // already exists for the head, and that PR is this job's.
        if status == 422
            && text.contains("already exists")
            && let Some(pr) = find_open_pull_request_with(token, repo, ask.head).await?
        {
            return Ok(reuse_pull_request(token, repo, pr, ask.update_note).await);
        }
        return Err(format!(
            "GitHub POST /repos/{repo}/pulls → {status}: {}",
            truncate_utf16(&text, 200)
        ));
    }
    let pr: serde_json::Value = res
        .json()
        .await
        .map_err(|e| format!("github decode: {e}"))?;
    let opened = pr_ref_of(&pr).ok_or("github pull request: no number in reply")?;
    Ok(OpenedPullRequest {
        url: opened.url,
        number: opened.number,
        reused: false,
        commented: false,
    })
}

/// Keep `pr`, and leave the note on it. The comment is the courtesy, not
/// the operation: a failure to post it is logged and said in the answer,
/// and the PR is still this job's.
async fn reuse_pull_request(
    token: &str,
    repo: &str,
    pr: PullRequestRef,
    note: &str,
) -> OpenedPullRequest {
    let commented = if note.trim().is_empty() {
        false
    } else {
        let body = serde_json::json!({ "body": note }).to_string();
        match gh_json(
            &format!("/repos/{repo}/issues/{}/comments", pr.number),
            token,
            "POST",
            Some(&body),
        )
        .await
        {
            Ok(_) => true,
            Err(e) => {
                tracing::warn!("[github] could not comment on {repo}#{}: {e}", pr.number);
                false
            }
        }
    };
    OpenedPullRequest {
        url: pr.url,
        number: pr.number,
        reused: true,
        commented,
    }
}

// ── PR status (read-only: Talaria never merges, comments or re-runs) ────────

/// Check-run conclusions that mean the check did not pass.
const FAILING_CONCLUSIONS: [&str; 5] = [
    "failure",
    "timed_out",
    "cancelled",
    "action_required",
    "startup_failure",
];

/// One verdict over a head commit's check runs and commit statuses:
/// `failing` if any failed, else `pending` if any has not finished, else
/// `passing` if there was anything to pass, else `none`.
pub fn checks_verdict(runs: &[serde_json::Value], statuses: &[serde_json::Value]) -> &'static str {
    let run_failed = runs.iter().any(|r| {
        r.get("conclusion")
            .and_then(|v| v.as_str())
            .is_some_and(|c| FAILING_CONCLUSIONS.contains(&c))
    });
    let status_failed = statuses.iter().any(|s| {
        matches!(
            s.get("state").and_then(|v| v.as_str()),
            Some("failure" | "error")
        )
    });
    if run_failed || status_failed {
        return "failing";
    }
    let run_pending = runs
        .iter()
        .any(|r| r.get("status").and_then(|v| v.as_str()) != Some("completed"));
    let status_pending = statuses
        .iter()
        .any(|s| s.get("state").and_then(|v| v.as_str()) == Some("pending"));
    if run_pending || status_pending {
        return "pending";
    }
    if runs.is_empty() && statuses.is_empty() {
        "none"
    } else {
        "passing"
    }
}

/// The review decision from the REST reviews list: each reviewer's latest
/// approving, change-requesting or dismissed review counts (a later plain
/// comment does not undo an approval, as on GitHub). Any reviewer still
/// requesting changes → `changes_requested`; else any approval →
/// `approved`; else any review at all → `commented`; else `none`. Answers
/// the decision and each reviewer's latest review, oldest reviewer first.
pub fn review_decision(reviews: &[serde_json::Value]) -> (&'static str, Vec<serde_json::Value>) {
    let mut order: Vec<String> = Vec::new();
    let mut decisive: HashMap<String, String> = HashMap::new();
    let mut latest: HashMap<String, serde_json::Value> = HashMap::new();
    for r in reviews {
        let by = r
            .get("user")
            .and_then(|u| u.get("login"))
            .and_then(|v| v.as_str())
            .unwrap_or("unknown")
            .to_string();
        let state = r.get("state").and_then(|v| v.as_str()).unwrap_or_default();
        if state == "PENDING" {
            continue;
        }
        if !order.contains(&by) {
            order.push(by.clone());
        }
        if matches!(state, "APPROVED" | "CHANGES_REQUESTED" | "DISMISSED") {
            decisive.insert(by.clone(), state.to_string());
        }
        latest.insert(
            by.clone(),
            serde_json::json!({
                "by": by,
                "state": state.to_lowercase(),
                "submittedAt": r.get("submitted_at").cloned().unwrap_or(serde_json::Value::Null),
            }),
        );
    }
    let decision = if decisive.values().any(|s| s == "CHANGES_REQUESTED") {
        "changes_requested"
    } else if decisive.values().any(|s| s == "APPROVED") {
        "approved"
    } else if order.is_empty() {
        "none"
    } else {
        "commented"
    };
    let reviews = order.iter().filter_map(|by| latest.remove(by)).collect();
    (decision, reviews)
}

/// A PR's state, its head commit's checks and its review decision — every
/// call a GET. `pr_status`'s whole answer, minus the job fields.
pub async fn pull_request_status(
    pg: &PgPool,
    sb: &SecretBox,
    repo: &str,
    number: i64,
) -> Result<serde_json::Value, String> {
    let token = repo_token(pg, sb, Some(repo), PR_STATUS_PERMISSIONS)
        .await?
        .ok_or("GitHub is not connected")?;
    pull_request_status_with(&token, repo, number).await
}

async fn pull_request_status_with(
    token: &str,
    repo: &str,
    number: i64,
) -> Result<serde_json::Value, String> {
    use serde_json::{Value, json};
    let pr = gh_json(&format!("/repos/{repo}/pulls/{number}"), token, "GET", None).await?;
    let str_at = |v: &Value, path: &[&str]| -> Value {
        let mut cur = v;
        for key in path {
            match cur.get(key) {
                Some(next) => cur = next,
                None => return Value::Null,
            }
        }
        cur.clone()
    };
    let merged = pr.get("merged").and_then(Value::as_bool) == Some(true)
        || pr.get("merged_at").is_some_and(Value::is_string);
    let state = if merged {
        "merged".to_string()
    } else {
        pr.get("state")
            .and_then(Value::as_str)
            .unwrap_or("unknown")
            .to_string()
    };
    let head_sha = pr
        .get("head")
        .and_then(|h| h.get("sha"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();

    // Checks and reviews are read separately and a failure to read one is
    // SAID, not thrown: the PR's own state is still worth having, and a
    // 403 here is almost always the App missing a read permission.
    let mut errors: Vec<String> = Vec::new();
    let unreadable = |what: &str, permission: &str, e: String| -> String {
        if e.contains("→ 403") || e.contains("→ 404") {
            format!(
                "{what} unreadable — the GitHub App needs the {permission} (read) permission ({e})"
            )
        } else {
            format!("{what} unreadable: {e}")
        }
    };
    let mut runs: Vec<Value> = Vec::new();
    let mut statuses: Vec<Value> = Vec::new();
    if !head_sha.is_empty() {
        match gh_json(
            &format!("/repos/{repo}/commits/{head_sha}/check-runs?per_page=100"),
            token,
            "GET",
            None,
        )
        .await
        {
            Ok(j) => {
                runs = j
                    .get("check_runs")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
            }
            Err(e) => errors.push(unreadable("check runs", "Checks", e)),
        }
        match gh_json(
            &format!("/repos/{repo}/commits/{head_sha}/status"),
            token,
            "GET",
            None,
        )
        .await
        {
            Ok(j) => {
                statuses = j
                    .get("statuses")
                    .and_then(Value::as_array)
                    .cloned()
                    .unwrap_or_default();
            }
            Err(e) => errors.push(unreadable("commit statuses", "Commit statuses", e)),
        }
    }
    let (decision, reviews) = match gh_json(
        &format!("/repos/{repo}/pulls/{number}/reviews?per_page=100"),
        token,
        "GET",
        None,
    )
    .await
    {
        Ok(j) => {
            let (d, r) = review_decision(j.as_array().map(Vec::as_slice).unwrap_or_default());
            (d.to_string(), r)
        }
        Err(e) => {
            errors.push(unreadable("reviews", "Pull requests", e));
            ("unknown".to_string(), Vec::new())
        }
    };
    // Nothing read and a read failed: the honest verdict is that nobody
    // knows, never `none` (which says there are no checks).
    let checks_unread = errors
        .iter()
        .any(|e| e.starts_with("check runs") || e.starts_with("commit statuses"));
    let verdict = if checks_unread && runs.is_empty() && statuses.is_empty() {
        "unknown"
    } else {
        checks_verdict(&runs, &statuses)
    };
    let run_wire: Vec<Value> = runs
        .iter()
        .map(|r| {
            json!({
                "name": str_at(r, &["name"]),
                "status": str_at(r, &["status"]),
                "conclusion": str_at(r, &["conclusion"]),
                "url": str_at(r, &["html_url"]),
            })
        })
        .collect();
    let status_wire: Vec<Value> = statuses
        .iter()
        .map(|s| {
            json!({
                "context": str_at(s, &["context"]),
                "state": str_at(s, &["state"]),
                "url": str_at(s, &["target_url"]),
            })
        })
        .collect();
    let requested: Vec<Value> = pr
        .get("requested_reviewers")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|u| str_at(u, &["login"])).collect())
        .unwrap_or_default();
    let mut out = json!({
        "repo": repo,
        "number": number,
        "url": str_at(&pr, &["html_url"]),
        "title": str_at(&pr, &["title"]),
        "state": state,
        "draft": pr.get("draft").and_then(Value::as_bool).unwrap_or(false),
        "base": str_at(&pr, &["base", "ref"]),
        "head": str_at(&pr, &["head", "ref"]),
        "headSha": head_sha,
        "mergeable": str_at(&pr, &["mergeable"]),
        "mergedAt": str_at(&pr, &["merged_at"]),
        "checks": { "verdict": verdict, "runs": run_wire, "statuses": status_wire },
        "reviewDecision": decision,
        "reviews": reviews,
        "requestedReviewers": requested,
    });
    if !errors.is_empty() {
        out["errors"] = json!(errors);
    }
    Ok(out)
}

/// THE CLONE URL AN AGENT IS GIVEN — and it never carries the token. The
/// sandbox's git is configured with Talaria's credential helper, so git asks
/// US when it needs one; the agent clones a plain URL, the push works, and
/// no credential was ever in the transcript. Still nullable: a deployment
/// with no GitHub configured has no repo to offer.
pub async fn clone_url(pg: &PgPool, sb: &SecretBox, repo: &str) -> Result<Option<String>, String> {
    Ok(if github_token(pg, sb, Some(repo)).await?.is_some() {
        Some(format!("https://github.com/{repo}.git"))
    } else {
        None
    })
}

/// GIT'S CREDENTIAL, for a repo this agent was actually granted. The second
/// source behind `/api/secrets/git-credential`: Talaria's own GitHub
/// installation token, which is what a workbench job needs in order to push
/// and which no workspace credential replaces.
///
/// SCOPED BY THE PATH GIT ASKS ABOUT. Git sends `path` when
/// `credential.useHttpPath` is set — the rendered gitconfig sets it — so the
/// request names the repository, and an agent gets a token only for a repo on
/// its own grant list. Without that, this would hand every agent a token for
/// every repo the installation can reach.
pub async fn agent_git_credential(
    pg: &PgPool,
    sb: &SecretBox,
    agent_id: &str,
    host: &str,
    path: Option<&str>,
) -> Result<Option<GitCredential>, String> {
    if host.to_lowercase() != "github.com" {
        return Ok(None);
    }
    let repo = path.unwrap_or_default();
    let repo = repo
        .trim_start_matches('/')
        .strip_suffix(".git")
        .unwrap_or(repo.trim_start_matches('/'));
    // `owner/name` and nothing else: a path we cannot read as a repo is a
    // request we cannot scope, and an unscoped answer is what this check
    // exists to stop.
    let re = regex::Regex::new(r"^[^/]+/[^/]+$").unwrap();
    if !re.is_match(repo) {
        return Ok(None);
    }
    if !granted_repos(pg, agent_id).await.iter().any(|r| r == repo) {
        return Ok(None);
    }
    match github_token(pg, sb, Some(repo)).await? {
        Some(token) => Ok(Some(GitCredential {
            username: "x-access-token".into(),
            password: token,
            repo: repo.to_string(),
        })),
        None => Ok(None),
    }
}

pub struct GitCredential {
    pub username: String,
    pub password: String,
    pub repo: String,
}

#[cfg(test)]
mod tests {

    #[test]
    fn the_branch_law_decides_every_shape() {
        use super::{RepoRule, push_allowed};
        let rule = |base: Option<&str>, mode: &str, prefix: Option<&str>| RepoRule {
            repo: "acme/widgets".into(),
            base_branch: base.map(String::from),
            push_mode: mode.into(),
            branch_prefix: prefix.map(String::from),
        };
        // The DEFAULT posture: any branch but the base.
        let d = rule(None, "branches_only", None);
        assert!(push_allowed(&d, "main", "refs/heads/agent/thing").is_ok());
        assert!(push_allowed(&d, "main", "refs/heads/main").is_err());
        assert!(push_allowed(&d, "main", "refs/tags/v1").is_ok()); // tags are not branch pushes
        // A named base is refused — and so is the repo's default, which the
        // named base used to leave open: a flow based on `release` still
        // has a `main` that promotion owns.
        let named = rule(Some("release"), "branches_only", None);
        assert!(push_allowed(&named, "main", "refs/heads/main").is_err());
        assert!(push_allowed(&named, "main", "refs/heads/release").is_err());
        assert!(push_allowed(&named, "main", "refs/heads/agent/thing").is_ok());
        // The well-known promotion branches are refused whatever the rule
        // names: a repo whose flow base is `rc` (this repo's own model)
        // keeps `main` and `master` closed too.
        let rc = rule(Some("rc"), "branches_only", None);
        for b in ["main", "master", "rc"] {
            assert!(
                push_allowed(&rc, "develop", &format!("refs/heads/{b}")).is_err(),
                "{b}"
            );
        }
        assert!(push_allowed(&rc, "develop", "refs/heads/develop").is_err());
        assert!(push_allowed(&rc, "develop", "refs/heads/main-ish").is_ok());
        assert!(push_allowed(&rc, "develop", "refs/heads/rc/agent-1").is_ok());
        // A prefix narrows the agent's own branches.
        let pfx = rule(None, "branches_only", Some("agent/"));
        assert!(push_allowed(&pfx, "main", "refs/heads/agent/ticket-1").is_ok());
        assert!(push_allowed(&pfx, "main", "refs/heads/feature-x").is_err());
        // 'free' is the explicit trust: everything goes, base included.
        let free = rule(None, "free", Some("agent/"));
        assert!(push_allowed(&free, "main", "refs/heads/main").is_ok());
        assert!(push_allowed(&free, "main", "refs/heads/master").is_ok());
    }

    #[test]
    fn a_repo_scoped_token_asks_for_three_permissions_and_the_retry_for_none() {
        use super::access_tokens_body;
        let narrowed = access_tokens_body("acme/widgets", true);
        assert_eq!(
            narrowed,
            r#"{"repositories":["widgets"],"permissions":{"contents":"write","pull_requests":"write","metadata":"read"}}"#
        );
        let v: serde_json::Value = serde_json::from_str(&narrowed).expect("valid JSON");
        assert_eq!(v["permissions"].as_object().map(|o| o.len()), Some(3));
        assert!(v["permissions"].get("administration").is_none());
        assert!(v["permissions"].get("workflows").is_none());
        // The fallback drops the block and keeps the one-repo scope: a
        // broad token never comes out of the retry either.
        assert_eq!(
            access_tokens_body("acme/widgets", false),
            r#"{"repositories":["widgets"]}"#
        );
        // A name with a quote in it cannot break the body.
        let odd = access_tokens_body("acme/wid\"gets", false);
        assert!(serde_json::from_str::<serde_json::Value>(&odd).is_ok());
    }

    /// The token app install and update clone with: one repository, read
    /// access to its contents, nothing that writes.
    #[test]
    fn a_read_token_asks_for_contents_read_and_nothing_that_writes() {
        use super::{RepoAccess, access_tokens_body_for};
        let body =
            access_tokens_body_for("acme/talaria-app-crm", Some(RepoAccess::Read.permissions()));
        assert_eq!(
            body,
            r#"{"repositories":["talaria-app-crm"],"permissions":{"contents":"read","metadata":"read"}}"#
        );
        let v: serde_json::Value = serde_json::from_str(&body).expect("valid JSON");
        let perms = v["permissions"].as_object().unwrap();
        assert!(perms.values().all(|p| p == "read"), "{perms:?}");
        assert_eq!(
            access_tokens_body_for("acme/x", Some(RepoAccess::Write.permissions())),
            access_tokens_body("acme/x", true)
        );
    }

    use super::*;

    // ── A loopback GitHub: the workspace's stub-listener pattern ─────────────
    //
    // Each test binds its own listener on 127.0.0.1:0 and scopes its calls to
    // it with `with_api_base`, so nothing here ever reaches api.github.com and
    // tests in parallel never share a stub. The route closure answers each
    // request by method and path; `seen` records what was asked, in order.

    #[derive(Debug, Clone)]
    struct Seen {
        method: String,
        path: String,
        auth: String,
        body: String,
    }

    type Route = dyn Fn(&Seen, usize) -> (u16, String) + Send + Sync;

    async fn stub(
        route: impl Fn(&Seen, usize) -> (u16, String) + Send + Sync + 'static,
    ) -> (String, std::sync::Arc<Mutex<Vec<Seen>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback listener binds");
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let seen = std::sync::Arc::new(Mutex::new(Vec::<Seen>::new()));
        let log = seen.clone();
        let route: std::sync::Arc<Route> = std::sync::Arc::new(route);
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let (log, route) = (log.clone(), route.clone());
                tokio::spawn(async move {
                    let mut buf = Vec::new();
                    let mut chunk = [0u8; 4096];
                    let (head, body) = loop {
                        let n = sock.read(&mut chunk).await.unwrap_or(0);
                        if n == 0 {
                            return;
                        }
                        buf.extend_from_slice(&chunk[..n]);
                        let text = String::from_utf8_lossy(&buf).to_string();
                        if let Some(end) = text.find("\r\n\r\n") {
                            let len = text[..end]
                                .lines()
                                .find_map(|l| {
                                    let (k, v) = l.split_once(':')?;
                                    k.eq_ignore_ascii_case("content-length")
                                        .then(|| v.trim().parse::<usize>().ok())?
                                })
                                .unwrap_or(0);
                            if buf.len() >= end + 4 + len {
                                break (
                                    text[..end].to_string(),
                                    text[end + 4..end + 4 + len].to_string(),
                                );
                            }
                        }
                    };
                    let mut words = head.lines().next().unwrap_or_default().split(' ');
                    let method = words.next().unwrap_or_default().to_string();
                    let path = words.next().unwrap_or_default().to_string();
                    let auth = head
                        .lines()
                        .find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            k.eq_ignore_ascii_case("authorization")
                                .then(|| v.trim().to_string())
                        })
                        .unwrap_or_default();
                    let req = Seen {
                        method,
                        path,
                        auth,
                        body,
                    };
                    let n = {
                        let mut log = log.lock().unwrap();
                        log.push(req.clone());
                        log.len() - 1
                    };
                    let (status, answer) = route(&req, n);
                    let reply = format!(
                        "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{answer}",
                        answer.len()
                    );
                    let _ = sock.write_all(reply.as_bytes()).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        (base, seen)
    }

    fn ask<'a>(note: &'a str) -> PullRequestAsk<'a> {
        PullRequestAsk {
            head: "talaria/tala-7-fix-login",
            base: "main",
            title: "[TALA-7] Fix login",
            body: "the body",
            draft: false,
            update_note: note,
        }
    }

    const PR_LOOKUP: &str =
        "/repos/acme/widgets/pulls?state=open&per_page=1&head=acme%3Atalaria%2Ftala-7-fix-login";

    #[tokio::test]
    async fn a_branch_with_an_open_pr_reuses_it_and_never_opens_a_second() {
        let (base, seen) = stub(|req, _| match (req.method.as_str(), req.path.as_str()) {
            ("GET", PR_LOOKUP) => (
                200,
                r#"[{"number":7,"html_url":"https://github.com/acme/widgets/pull/7"}]"#.into(),
            ),
            ("POST", "/repos/acme/widgets/issues/7/comments") => (201, "{}".into()),
            _ => (500, r#"{"message":"unexpected"}"#.into()),
        })
        .await;
        let opened = with_api_base(
            base,
            open_or_update_pull_request_with(
                "tok",
                "acme/widgets",
                &ask("round two: fixed the review"),
            ),
        )
        .await
        .expect("the existing PR is reused");
        assert_eq!(
            opened,
            OpenedPullRequest {
                url: "https://github.com/acme/widgets/pull/7".into(),
                number: 7,
                reused: true,
                commented: true,
            }
        );
        let seen = seen.lock().unwrap().clone();
        assert!(
            !seen
                .iter()
                .any(|r| r.method == "POST" && r.path == "/repos/acme/widgets/pulls"),
            "no second PR was asked for: {seen:?}"
        );
        let comment = seen
            .iter()
            .find(|r| r.path.ends_with("/issues/7/comments"))
            .expect("the summary was left on the PR");
        let body: serde_json::Value = serde_json::from_str(&comment.body).unwrap();
        assert_eq!(body["body"], "round two: fixed the review");
        assert_eq!(comment.auth, "Bearer tok");
    }

    #[tokio::test]
    async fn a_branch_without_a_pr_gets_one_opened() {
        let (base, seen) = stub(|req, _| match (req.method.as_str(), req.path.as_str()) {
            ("GET", PR_LOOKUP) => (200, "[]".into()),
            ("POST", "/repos/acme/widgets/pulls") => (
                201,
                r#"{"number":8,"html_url":"https://github.com/acme/widgets/pull/8"}"#.into(),
            ),
            _ => (500, "{}".into()),
        })
        .await;
        let opened = with_api_base(
            base,
            open_or_update_pull_request_with("tok", "acme/widgets", &ask("unused")),
        )
        .await
        .expect("a PR opens");
        assert_eq!(opened.number, 8);
        assert!(!opened.reused && !opened.commented);
        let seen = seen.lock().unwrap().clone();
        let post = seen.iter().find(|r| r.method == "POST").expect("one POST");
        let body: serde_json::Value = serde_json::from_str(&post.body).unwrap();
        assert_eq!(body["head"], "talaria/tala-7-fix-login");
        assert_eq!(body["base"], "main");
        assert_eq!(body["title"], "[TALA-7] Fix login");
        assert_eq!(body["draft"], false);
        assert_eq!(seen.len(), 2, "lookup then create, nothing else: {seen:?}");
    }

    #[tokio::test]
    async fn a_pr_opened_between_lookup_and_create_is_found_not_thrown() {
        // GitHub's own refusal for a second PR on a head: 422, "already exists".
        let (base, _) = stub(|req, n| match (req.method.as_str(), req.path.as_str()) {
            ("GET", PR_LOOKUP) if n == 0 => (200, "[]".into()),
            ("GET", PR_LOOKUP) => (
                200,
                r#"[{"number":9,"html_url":"https://github.com/acme/widgets/pull/9"}]"#.into(),
            ),
            ("POST", "/repos/acme/widgets/pulls") => (
                422,
                r#"{"message":"Validation Failed","errors":[{"resource":"PullRequest","code":"custom","message":"A pull request already exists for acme:talaria/tala-7-fix-login."}]}"#.into(),
            ),
            _ => (201, "{}".into()),
        })
        .await;
        let opened = with_api_base(
            base,
            open_or_update_pull_request_with("tok", "acme/widgets", &ask("note")),
        )
        .await
        .expect("the race resolves to the PR that exists");
        assert_eq!((opened.number, opened.reused), (9, true));
    }

    #[tokio::test]
    async fn any_other_create_failure_is_an_error_with_githubs_words() {
        let (base, _) = stub(|req, _| match req.method.as_str() {
            "GET" => (200, "[]".into()),
            _ => (
                422,
                r#"{"message":"Validation Failed","errors":[{"message":"No commits between main and talaria/tala-7-fix-login"}]}"#.into(),
            ),
        })
        .await;
        let err = with_api_base(
            base,
            open_or_update_pull_request_with("tok", "acme/widgets", &ask("note")),
        )
        .await
        .expect_err("not a reuse");
        assert!(err.contains("422") && err.contains("No commits"), "{err}");
    }

    #[test]
    fn a_pr_number_reads_off_its_page_url() {
        assert_eq!(
            pr_number_from_url("https://github.com/acme/widgets/pull/42"),
            Some(42)
        );
        assert_eq!(
            pr_number_from_url("https://github.com/acme/widgets/pull/42/"),
            Some(42)
        );
        assert_eq!(pr_number_from_url("https://github.com/acme/widgets"), None);
        assert_eq!(
            pr_number_from_url("https://github.com/acme/widgets/pull/x"),
            None
        );
    }

    // ── Repo creation routes to the org's own installation (B13) ─────────────

    fn app_cfg(installations: &[&str], orgs: &[&str], sb: &SecretBox) -> GithubConfig {
        let pem = include_str!("../../../tests/it/fixtures/github-jwt-traditional.pem");
        GithubConfig {
            mode: Some("app".into()),
            pat_token_enc: None,
            app_id: "12345".into(),
            installation_ids: installations.iter().map(|s| s.to_string()).collect(),
            private_key_enc: Some(sb.seal(pem).expect("the key seals")),
            repo_creation_orgs: orgs.iter().map(|s| s.to_string()).collect(),
        }
    }

    fn test_box() -> SecretBox {
        SecretBox::from_parts(
            [7u8; 32],
            HashMap::from([(1u32, talaria_secretbox::new_dek().expect("entropy"))]),
            Some(1),
        )
    }

    #[tokio::test]
    async fn create_repo_uses_the_installation_github_names_for_the_org() {
        // Two installations; the FIRST is another org's. The org is new and
        // empty, so no cache could have routed it — GitHub is asked instead.
        let sb = test_box();
        let cfg = app_cfg(&["90001", "90002"], &["plugins"], &sb);
        let (base, seen) = stub(|req, _| match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/orgs/plugins/installation") => (200, r#"{"id":90002}"#.into()),
            ("POST", "/app/installations/90002/access_tokens") => {
                (201, r#"{"token":"inst-90002"}"#.into())
            }
            ("POST", "/orgs/plugins/repos") => (
                201,
                r#"{"full_name":"plugins/talaria-app-notes","html_url":"https://github.com/plugins/talaria-app-notes"}"#.into(),
            ),
            _ => (500, "{}".into()),
        })
        .await;
        let created = with_api_base(
            base,
            create_repo_in(
                &sb,
                &cfg,
                "plugins",
                "talaria-app-notes",
                "notes",
                RepoVisibility::Public,
            ),
        )
        .await
        .expect("the repo is created");
        assert_eq!(created.full_name, "plugins/talaria-app-notes");
        let seen = seen.lock().unwrap().clone();
        assert!(
            seen[0].auth.starts_with("Bearer ey"),
            "the lookup carries the App JWT"
        );
        let post = seen
            .iter()
            .find(|r| r.path == "/orgs/plugins/repos")
            .expect("the create");
        assert_eq!(
            post.auth, "Bearer inst-90002",
            "minted on the org's own installation"
        );
        assert!(
            !seen
                .iter()
                .any(|r| r.path.contains("/installations/90001/")),
            "the other org's installation is never touched: {seen:?}"
        );
        let body: serde_json::Value = serde_json::from_str(&post.body).unwrap();
        assert_eq!(body["private"], false, "public was asked for");
        assert_eq!(body["auto_init"], true);
    }

    #[tokio::test]
    async fn create_repo_refuses_an_org_the_app_is_not_installed_on() {
        let sb = test_box();
        let cfg = app_cfg(&["90011"], &["elsewhere"], &sb);
        let (base, seen) = stub(|req, _| match req.path.as_str() {
            "/orgs/elsewhere/installation" => (404, r#"{"message":"Not Found"}"#.into()),
            _ => (500, "{}".into()),
        })
        .await;
        let err = with_api_base(
            base,
            create_repo_in(&sb, &cfg, "elsewhere", "x", "", RepoVisibility::Private),
        )
        .await
        .expect_err("refused");
        assert!(err.contains("not installed on elsewhere"), "{err}");
        assert_eq!(
            seen.lock().unwrap().len(),
            1,
            "nothing minted, nothing posted"
        );
    }

    #[tokio::test]
    async fn create_repo_refuses_an_installation_the_admin_did_not_select() {
        let sb = test_box();
        let cfg = app_cfg(&["90021"], &["side"], &sb);
        let (base, _) = stub(|req, _| match req.path.as_str() {
            "/orgs/side/installation" => (200, r#"{"id":90099}"#.into()),
            _ => (500, "{}".into()),
        })
        .await;
        let err = with_api_base(
            base,
            create_repo_in(&sb, &cfg, "side", "x", "", RepoVisibility::Private),
        )
        .await
        .expect_err("refused");
        assert!(
            err.contains("installation 90099") && err.contains("does not use"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn a_private_repo_is_the_default_and_a_403_names_the_permission() {
        let sb = test_box();
        let cfg = app_cfg(&["90031"], &["acme"], &sb);
        let (base, seen) = stub(|req, _| match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/orgs/acme/installation") => (200, r#"{"id":"90031"}"#.into()),
            ("POST", "/app/installations/90031/access_tokens") => (201, r#"{"token":"t"}"#.into()),
            ("POST", "/orgs/acme/repos") => {
                (403, r#"{"message":"Resource not accessible"}"#.into())
            }
            _ => (500, "{}".into()),
        })
        .await;
        let err = with_api_base(
            base,
            create_repo_in(&sb, &cfg, "acme", "x", "", RepoVisibility::default()),
        )
        .await
        .expect_err("403");
        assert!(err.contains("Administration permission"), "{err}");
        let post = seen
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.path == "/orgs/acme/repos")
            .cloned()
            .unwrap();
        let body: serde_json::Value = serde_json::from_str(&post.body).unwrap();
        assert_eq!(body["private"], true);
    }

    #[test]
    fn repo_creation_is_refused_up_front_outside_app_mode() {
        let sb = test_box();
        let mut cfg = app_cfg(&["1"], &["acme"], &sb);
        assert_eq!(repo_creation_refusal(&cfg, "acme"), None);
        let other = repo_creation_refusal(&cfg, "other").expect("not on the list");
        assert!(other.contains("approved: acme"), "{other}");
        cfg.mode = Some("pat".into());
        let pat = repo_creation_blocker(&cfg).expect("PAT mode cannot create");
        assert!(pat.contains("GitHub App"), "{pat}");
        cfg.mode = Some("app".into());
        cfg.repo_creation_orgs.clear();
        assert!(repo_creation_blocker(&cfg).unwrap().contains("off"));
        let mut partial = app_cfg(&[], &["acme"], &sb);
        partial.installation_ids.clear();
        assert!(
            repo_creation_blocker(&partial)
                .unwrap()
                .contains("incomplete")
        );
        assert_eq!(
            RepoVisibility::parse("public"),
            Some(RepoVisibility::Public)
        );
        assert_eq!(RepoVisibility::parse("internal"), None);
        assert_eq!(RepoVisibility::default().as_str(), "private");
    }

    // ── PR status: read-only, every call a GET ────────────────────────────────

    #[test]
    fn the_checks_verdict_fails_on_any_failure_and_waits_on_any_run() {
        use serde_json::json;
        let ok = json!({"status":"completed","conclusion":"success"});
        let skipped = json!({"status":"completed","conclusion":"skipped"});
        let bad = json!({"status":"completed","conclusion":"failure"});
        let running = json!({"status":"in_progress","conclusion":null});
        assert_eq!(checks_verdict(&[], &[]), "none");
        assert_eq!(checks_verdict(&[ok.clone(), skipped], &[]), "passing");
        assert_eq!(
            checks_verdict(&[ok.clone(), running.clone()], &[]),
            "pending"
        );
        assert_eq!(checks_verdict(&[running, bad], &[]), "failing");
        assert_eq!(
            checks_verdict(std::slice::from_ref(&ok), &[json!({"state":"error"})]),
            "failing"
        );
        assert_eq!(
            checks_verdict(&[ok], &[json!({"state":"pending"})]),
            "pending"
        );
        assert_eq!(
            checks_verdict(
                &[json!({"status":"completed","conclusion":"timed_out"})],
                &[]
            ),
            "failing"
        );
    }

    #[test]
    fn the_review_decision_follows_each_reviewers_latest_verdict() {
        use serde_json::json;
        let r = |who: &str, state: &str| json!({"user":{"login":who},"state":state,"submitted_at":"2026-10-22T00:00:00Z"});
        assert_eq!(review_decision(&[]).0, "none");
        assert_eq!(review_decision(&[r("ann", "COMMENTED")]).0, "commented");
        assert_eq!(review_decision(&[r("ann", "APPROVED")]).0, "approved");
        // A later comment does not undo an approval.
        assert_eq!(
            review_decision(&[r("ann", "APPROVED"), r("ann", "COMMENTED")]).0,
            "approved"
        );
        // Changes requested, then approved by the same person: approved.
        assert_eq!(
            review_decision(&[r("ann", "CHANGES_REQUESTED"), r("ann", "APPROVED")]).0,
            "approved"
        );
        // One reviewer still asking for changes outweighs another's approval.
        let (decision, latest) = review_decision(&[
            r("ann", "APPROVED"),
            r("bob", "CHANGES_REQUESTED"),
            r("bob", "COMMENTED"),
        ]);
        assert_eq!(decision, "changes_requested");
        assert_eq!(latest.len(), 2);
        assert_eq!(latest[1]["by"], "bob");
        assert_eq!(latest[1]["state"], "commented");
        // A dismissed change request no longer blocks.
        assert_eq!(
            review_decision(&[r("bob", "CHANGES_REQUESTED"), r("bob", "DISMISSED")]).0,
            "commented"
        );
        // A draft (PENDING) review is nobody's verdict yet.
        assert_eq!(review_decision(&[r("cy", "PENDING")]).0, "none");
    }

    #[tokio::test]
    async fn pr_status_reads_state_checks_and_reviews_and_writes_nothing() {
        let (base, seen) = stub(|req, _| match req.path.as_str() {
            "/repos/acme/widgets/pulls/7" => (
                200,
                r#"{"number":7,"html_url":"https://github.com/acme/widgets/pull/7","title":"[TALA-7] Fix login","state":"open","merged":false,"merged_at":null,"draft":false,"mergeable":true,"base":{"ref":"main"},"head":{"ref":"talaria/tala-7-fix-login","sha":"abc123"},"requested_reviewers":[{"login":"dee"}]}"#.into(),
            ),
            "/repos/acme/widgets/commits/abc123/check-runs?per_page=100" => (
                200,
                r#"{"total_count":2,"check_runs":[{"name":"test","status":"completed","conclusion":"failure","html_url":"https://ci/1"},{"name":"lint","status":"completed","conclusion":"success","html_url":"https://ci/2"}]}"#.into(),
            ),
            "/repos/acme/widgets/commits/abc123/status" => {
                (200, r#"{"state":"pending","total_count":0,"statuses":[]}"#.into())
            }
            "/repos/acme/widgets/pulls/7/reviews?per_page=100" => (
                200,
                r#"[{"user":{"login":"ann"},"state":"CHANGES_REQUESTED","submitted_at":"2026-10-22T01:00:00Z"}]"#.into(),
            ),
            _ => (500, "{}".into()),
        })
        .await;
        let status = with_api_base(base, pull_request_status_with("tok", "acme/widgets", 7))
            .await
            .expect("status reads");
        assert_eq!(status["state"], "open");
        assert_eq!(status["headSha"], "abc123");
        assert_eq!(status["checks"]["verdict"], "failing");
        assert_eq!(status["checks"]["runs"][0]["name"], "test");
        assert_eq!(status["checks"]["runs"][0]["conclusion"], "failure");
        assert_eq!(status["reviewDecision"], "changes_requested");
        assert_eq!(status["requestedReviewers"][0], "dee");
        assert!(status.get("errors").is_none());
        let seen = seen.lock().unwrap().clone();
        assert_eq!(seen.len(), 4);
        assert!(
            seen.iter().all(|r| r.method == "GET"),
            "pr_status never writes: {seen:?}"
        );
    }

    #[tokio::test]
    async fn pr_status_says_merged_and_names_a_missing_checks_permission() {
        let (base, _) = stub(|req, _| match req.path.as_str() {
            "/repos/acme/widgets/pulls/5" => (
                200,
                r#"{"number":5,"state":"closed","merged":true,"merged_at":"2026-10-22T02:00:00Z","head":{"ref":"b","sha":"def"},"base":{"ref":"main"}}"#.into(),
            ),
            p if p.contains("/check-runs") => (403, r#"{"message":"Resource not accessible by integration"}"#.into()),
            p if p.ends_with("/status") => (403, r#"{"message":"Resource not accessible by integration"}"#.into()),
            p if p.contains("/reviews") => (200, "[]".into()),
            _ => (500, "{}".into()),
        })
        .await;
        let status = with_api_base(base, pull_request_status_with("tok", "acme/widgets", 5))
            .await
            .expect("a PR state is still an answer");
        assert_eq!(status["state"], "merged");
        assert_eq!(status["checks"]["verdict"], "unknown");
        assert_eq!(status["reviewDecision"], "none");
        let errors = status["errors"].as_array().expect("the gap is said");
        assert!(
            errors[0]
                .as_str()
                .unwrap()
                .contains("Checks (read) permission"),
            "{errors:?}"
        );
    }

    #[test]
    fn a_status_token_asks_for_reads_only() {
        let body = access_tokens_body_for("acme/widgets", Some(PR_STATUS_PERMISSIONS));
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        let perms = v["permissions"].as_object().unwrap();
        assert!(perms.values().all(|p| p == "read"), "{perms:?}");
        assert_eq!(perms.get("checks").and_then(|p| p.as_str()), Some("read"));
        assert_eq!(v["repositories"], serde_json::json!(["widgets"]));
    }

    #[test]
    fn next_link_path_extracts_the_path() {
        assert_eq!(
            next_link_path(
                r#"<https://api.github.com/user/repos?per_page=100&sort=pushed&page=2>; rel="next", <https://api.github.com/user/repos?page=1>; rel="first""#
            )
            .as_deref(),
            Some("/user/repos?per_page=100&sort=pushed&page=2")
        );
        assert_eq!(
            next_link_path(r#"<https://example.com/x>; rel="next""#),
            None
        );
        assert_eq!(next_link_path(""), None);
    }

    #[test]
    fn legacy_installation_migrates_on_read() {
        let raw: serde_json::Value = serde_json::json!({
            "mode": "app",
            "app": { "appId": "123", "installationId": "99", "privateKeyEnc": "x" }
        });
        let obj = raw.as_object().cloned().unwrap_or_default();
        let app = obj
            .get("app")
            .and_then(|a| a.as_object())
            .cloned()
            .unwrap_or_default();
        let mut installation_ids: Vec<String> = app
            .get("installationIds")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();
        assert!(installation_ids.is_empty());
        if installation_ids.is_empty()
            && let Some(legacy) = cfg_str(&app, "installationId")
        {
            installation_ids = vec![legacy];
        }
        assert_eq!(installation_ids, vec!["99".to_string()]);
    }
}
