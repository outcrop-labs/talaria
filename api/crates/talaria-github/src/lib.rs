// The GitHub adapter of the git-host interface (talaria-git-host-types'
// `GitHost`) — github.com or a GitHub Enterprise Server, connected either way:
//   app  a GitHub App (preferred): short-lived installation tokens, per-repo
//        install scope, instant revocation from GitHub's side
//   pat  a personal access token (quickest start; fine-grained PAT advised)
//
// ONE CONNECTION PER HOST ROW. The registry (talaria-git-hosts) keeps each
// host an operator added as a `git_hosts` row; a GitHub row's `auth` column
// holds this connection's config with its secrets SEALED (secretbox), the
// shape `app_settings.github_config` used to hold before the registry. Nothing
// here reads a global: a [`Github`] is built from one row and speaks to that
// row's API origin (api.github.com, or `<server>/api/v3`).
//
// What the adapter owns: the config's parse and patch, the RS256 App JWT,
// per-repo-scoped and permission-narrowed installation tokens, the
// repo→installation routing table, the live status and installation listing,
// repo creation (on the org's own installation, with the default-branch
// ruleset), and the branch / pull-request reads and writes the workbench
// drives. Which agent may touch which repo is the registry's grant table, not
// this crate's: connecting GitHub grants nothing to anyone by itself.

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
use serde_json::{Value, json};

use talaria_agent_auth::now_ms;
use talaria_body::truncate_utf16;
use talaria_gateway::provider::http;
use talaria_secretbox::SecretBox;

pub use talaria_git_host_types::{
    CreatedRepo, GitHost, HostKind, HostStatus, Login, OpenPullRequest, OpenedPullRequest,
    PullRequestAsk, PullRequestRef, RepoAccess, RepoVisibility, checks_verdict, pr_number_from_url,
    review_decision,
};
use talaria_git_host_types::{PrStatusParts, pr_status_wire, value_at};

/// github.com's REST origin.
pub const GITHUB_API: &str = "https://api.github.com";
/// github.com's web origin.
pub const GITHUB_WEB: &str = "https://github.com";

tokio::task_local! {
    /// The GitHub REST origin for the calls one task makes, over whatever the
    /// connection's own is. Production never sets it; a test scopes it to a
    /// loopback stub with [`with_api_base`]. Task-local rather than a global
    /// so tests running in parallel each talk to their own stub.
    static API_BASE: String;
}

/// Run `f` with every GitHub call it makes sent to `base` instead of the
/// connection's API origin. A TEST SEAM, nothing else: the workbench's and
/// the plugin sweep's tests point it at a loopback stub so the PR and
/// repo-creation flows run end to end with no GitHub. Calls made from a task
/// `f` spawns are not covered (task-locals do not cross `tokio::spawn`).
#[doc(hidden)]
pub async fn with_api_base<F: std::future::Future>(base: impl Into<String>, f: F) -> F::Output {
    API_BASE.scope(base.into(), f).await
}

/// The API origin a host at `web` answers on: api.github.com for github.com,
/// `<server>/api/v3` for an Enterprise Server.
pub fn api_origin_for(web: &str) -> String {
    let web = web.trim_end_matches('/');
    if web.eq_ignore_ascii_case(GITHUB_WEB) || web.eq_ignore_ascii_case("https://www.github.com") {
        GITHUB_API.to_string()
    } else {
        format!("{web}/api/v3")
    }
}

// ── The connection's config (a git_hosts row's `auth`) ───────────────────────

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
}

fn cfg_str(obj: &serde_json::Map<String, Value>, key: &str) -> Option<String> {
    obj.get(key).and_then(|v| v.as_str()).map(|s| s.to_string())
}

impl GithubConfig {
    /// Read a stored `auth` value. A legacy single-installation config
    /// migrates transparently on read.
    pub fn from_auth(raw: &Value) -> GithubConfig {
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
        }
    }

    /// The stored shape, secrets still sealed.
    pub fn to_auth(&self) -> Value {
        json!({
            "mode": self.mode,
            "pat": { "tokenEnc": self.pat_token_enc },
            "app": {
                "appId": self.app_id,
                "installationIds": self.installation_ids,
                "privateKeyEnc": self.private_key_enc,
            },
        })
    }

    /// The patch merge and the seals. A secret that arrives is sealed (an
    /// EMPTY string clears); one that doesn't arrive keeps its sealed
    /// predecessor. `installationIds`/`appId` use `??` semantics — undefined
    /// keeps, any value (empty included) replaces.
    pub fn patched(
        &self,
        sb: &SecretBox,
        patch: &GithubConfigPatch<'_>,
    ) -> Result<GithubConfig, String> {
        let seal_or_clear = |v: &str| -> Result<Option<String>, String> {
            if v.is_empty() {
                Ok(None)
            } else {
                sb.seal(v)
                    .map(Some)
                    .map_err(|e| format!("github seal: {e}"))
            }
        };
        Ok(GithubConfig {
            mode: match patch.mode {
                PatchField::Unset => self.mode.clone(),
                PatchField::Set(m) => m.map(|s| s.to_string()),
            },
            pat_token_enc: match &patch.pat_token {
                PatchField::Unset => self.pat_token_enc.clone(),
                PatchField::Set(None) => None,
                PatchField::Set(Some(token)) => seal_or_clear(token)?,
            },
            app_id: patch
                .app_id
                .map(|s| s.to_string())
                .unwrap_or_else(|| self.app_id.clone()),
            installation_ids: patch
                .installation_ids
                .map(|ids| ids.iter().map(|s| s.to_string()).collect())
                .unwrap_or_else(|| self.installation_ids.clone()),
            private_key_enc: match &patch.private_key {
                PatchField::Unset => self.private_key_enc.clone(),
                PatchField::Set(None) => None,
                PatchField::Set(Some(key)) => seal_or_clear(key)?,
            },
        })
    }

    /// The full clear: mode off, PAT gone, App identity blanked.
    pub fn cleared() -> GithubConfig {
        GithubConfig::default()
    }

    /// The redacted config an admin reads: never a secret, only whether one
    /// is set.
    pub fn redacted(&self) -> Value {
        json!({
            "mode": self.mode,
            "app": {
                "appId": self.app_id,
                "installationIds": self.installation_ids,
                "keySet": self.private_key_enc.is_some(),
            },
            "patSet": self.pat_token_enc.is_some(),
        })
    }

    /// Every sealed value this config holds — the secret-health inventory's
    /// read, as `(path, label, cipher)`.
    pub fn sealed(&self) -> Vec<(&'static str, &'static str, &str)> {
        let mut out = Vec::new();
        if self.mode.as_deref() == Some("app")
            && let Some(k) = self.private_key_enc.as_deref()
        {
            out.push(("app.privateKeyEnc", "GitHub App private key", k));
        }
        if self.mode.as_deref() == Some("pat")
            && let Some(t) = self.pat_token_enc.as_deref()
        {
            out.push(("pat.tokenEnc", "GitHub access token", t));
        }
        out
    }
}

/// The three-state patch fields: Unset keeps the current value, Set(None)
/// clears, Set(Some) replaces. `app_id`/`installation_ids` are plain Option —
/// those keys carry no null-vs-undefined distinction on the wire.
pub enum PatchField<T> {
    Unset,
    Set(Option<T>),
}

/// A git host route's GitHub body, already validated.
pub struct GithubConfigPatch<'a> {
    pub mode: PatchField<&'a str>,
    pub pat_token: PatchField<&'a str>,
    pub app_id: Option<&'a str>,
    pub installation_ids: Option<&'a [&'a str]>,
    pub private_key: PatchField<&'a str>,
}

// ── The REST plumbing ─────────────────────────────────────────────────────────

async fn gh(
    api: &str,
    path: &str,
    token: &str,
    method: &str,
    body: Option<&str>,
) -> Result<reqwest::Response, String> {
    let mut req = http()
        .request(
            reqwest::Method::from_bytes(method.as_bytes())
                .map_err(|_| format!("github method: {method}"))?,
            format!("{api}{path}"),
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
    api: &str,
    path: &str,
    token: &str,
    method: &str,
    body: Option<&str>,
) -> Result<Value, String> {
    let res = gh(api, path, token, method, body).await?;
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

/// The permission set a [`RepoAccess`] narrows a token to.
pub fn permissions_for(access: RepoAccess) -> &'static str {
    match access {
        RepoAccess::Write => REPO_TOKEN_PERMISSIONS,
        RepoAccess::Read => REPO_READ_PERMISSIONS,
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

#[derive(Clone)]
struct RepoInstallCache {
    map: HashMap<String, String>,
    at: i64,
}

/// The repo→installation routing table, per host.
fn repo_install_cache() -> &'static Mutex<HashMap<String, RepoInstallCache>> {
    static CACHE: OnceLock<Mutex<HashMap<String, RepoInstallCache>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Forget every cached token and routing entry of host `host_id`: its
/// credentials changed, and nothing minted under the old ones may outlive
/// them.
pub fn forget_host(host_id: &str) {
    let prefix = format!("{host_id}#");
    install_tokens()
        .lock()
        .unwrap()
        .retain(|k, _| !k.starts_with(&prefix));
    repo_install_cache().lock().unwrap().remove(host_id);
}

/// `<https://api.example/...>; rel="next"` → the path under `api`.
fn next_link_path(api: &str, link: &str) -> Option<String> {
    let re = regex::Regex::new(r#"<([^>]+)>;\s*rel="next""#).ok()?;
    let next = re.captures(link)?.get(1)?.as_str();
    next.strip_prefix(api).map(str::to_string)
}

/// encodeURIComponent — the subset GitHub ref paths need (segment-internal
/// slashes stay literal in `a...b` compares; ref names encode whole).
pub fn enc(segment: &str) -> String {
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

// ── The connection ───────────────────────────────────────────────────────────

/// One GitHub connection: a `git_hosts` row of kind `github`.
#[derive(Clone)]
pub struct Github {
    /// The registry row's id — what the token caches are keyed under.
    pub host_id: String,
    pub cfg: GithubConfig,
    /// The web origin (`https://github.com`, `https://ghe.example.com`).
    pub web: String,
    /// The REST origin (`https://api.github.com`, `<server>/api/v3`).
    pub api: String,
    sb: SecretBox,
}

impl std::fmt::Debug for Github {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Github")
            .field("host_id", &self.host_id)
            .field("web", &self.web)
            .field("api", &self.api)
            .field("mode", &self.cfg.mode)
            .finish()
    }
}

impl Github {
    /// A connection for one host row. `api` overrides the origin derived from
    /// `web` ([`api_origin_for`]).
    pub fn new(host_id: &str, web: &str, api: Option<&str>, auth: &Value, sb: SecretBox) -> Github {
        let web = web.trim_end_matches('/').to_string();
        let api = api
            .map(|a| a.trim_end_matches('/').to_string())
            .filter(|a| !a.is_empty())
            .unwrap_or_else(|| api_origin_for(&web));
        Github {
            host_id: host_id.to_string(),
            cfg: GithubConfig::from_auth(auth),
            web,
            api,
            sb,
        }
    }

    /// The REST origin this call goes to: the test seam's, else the row's.
    fn api(&self) -> String {
        API_BASE
            .try_with(Clone::clone)
            .unwrap_or_else(|_| self.api.clone())
    }

    fn pat(&self) -> Result<Option<String>, String> {
        match &self.cfg.pat_token_enc {
            Some(enc) => self
                .sb
                .open(enc)
                .map(Some)
                .map_err(|e| format!("github pat unseal: {e}")),
            None => Ok(None),
        }
    }

    /// The App JWT for this connection — the credential the `/app` and
    /// `/orgs/{org}/installation` endpoints take.
    fn app_jwt(&self) -> Result<String, String> {
        let private_key_enc = self.cfg.private_key_enc.as_deref().unwrap_or_default();
        let key = self
            .sb
            .open(private_key_enc)
            .map_err(|e| format!("github app key unseal: {e}"))?;
        app_jwt(&self.cfg.app_id, &key)
    }

    fn app_ready(&self) -> bool {
        self.cfg.mode.as_deref() == Some("app")
            && !self.cfg.app_id.is_empty()
            && !self.cfg.installation_ids.is_empty()
            && self.cfg.private_key_enc.is_some()
    }

    /// Mint an installation access token. When `repo` is set the token is
    /// scoped to THAT repository alone — without a body GitHub issues one
    /// valid for EVERY repo in the installation, and this token ends up
    /// inside a sandbox running model-authored code, so an unscoped one made
    /// a single leaked credential equivalent to push access across the whole
    /// org. A repo-scoped token is also narrowed to `permissions`, with one
    /// retry on the 422 an under-granted installation answers (never for a
    /// read-only token); the installation-wide token (`repo: None` — listing,
    /// repo creation) is not narrowed, because those callers are the ones
    /// that need the App's wider grants and it never enters a sandbox. The
    /// cache keys on the host, the installation, the repo and the permission
    /// set: a read token is never served where a push token was asked for.
    async fn installation_token(
        &self,
        installation_id: &str,
        repo: Option<&str>,
        permissions: &str,
    ) -> Result<String, String> {
        let key = match repo {
            Some(r) => format!("{}#{installation_id}#{r}#{permissions}", self.host_id),
            None => format!("{}#{installation_id}", self.host_id),
        };
        if let Some((token, expires_at)) = install_tokens().lock().unwrap().get(&key)
            && *expires_at > now_ms() + 60_000
        {
            return Ok(token.clone());
        }

        let jwt = self.app_jwt()?;
        let base = self.api();

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
            // token is never widened this way: the installation's own grants
            // may include write, and a clone that cannot get a read-only
            // token gets no token.
            tracing::warn!(
                "[github] narrowed installation token for {repo} was refused (422) — retrying on the installation's own grants; check the App has {permissions}"
            );
            res = mint(Some(access_tokens_body_for(repo, None)))
                .await
                .map_err(|e| format!("github installation token: {e}"))?;
        }
        if !res.status().is_success() {
            // 422 here usually means the repo isn't in THIS installation —
            // worth saying so, because the caller picked the installation by
            // cache lookup.
            let suffix = repo.map(|r| format!(" for {r}")).unwrap_or_default();
            return Err(format!(
                "GitHub installation token failed ({}){suffix}",
                res.status().as_u16()
            ));
        }
        let j: Value = res
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

    /// A usable token for repo operations. PAT passes through (a PAT cannot
    /// be narrowed or scoped; that blast radius is the org's own choice); App
    /// mode mints a per-installation token — `repo` routes a multi-org setup
    /// to the installation that owns it AND scopes the token to that one
    /// repository, narrowed to `permissions`. None when the connection is not
    /// configured.
    pub async fn token(
        &self,
        repo: Option<&str>,
        permissions: &str,
    ) -> Result<Option<String>, String> {
        if self.cfg.mode.as_deref() == Some("pat") {
            return self.pat();
        }
        if !self.app_ready() {
            return Ok(None);
        }
        let mut installation_id = self.cfg.installation_ids[0].clone();
        if let Some(repo) = repo
            && self.cfg.installation_ids.len() > 1
        {
            let stale = repo_install_cache()
                .lock()
                .unwrap()
                .get(&self.host_id)
                .map(|c| now_ms() - c.at > 10 * 60_000)
                .unwrap_or(true);
            if stale {
                let _ = self.reachable().await;
            }
            if let Some(hit) = repo_install_cache().lock().unwrap().get(&self.host_id)
                && let Some(inst) = hit.map.get(repo)
            {
                installation_id = inst.clone();
            }
        }
        Ok(Some(
            self.installation_token(&installation_id, repo, permissions)
                .await?,
        ))
    }

    async fn token_or_err(&self, repo: &str, permissions: &str) -> Result<String, String> {
        self.token(Some(repo), permissions)
            .await?
            .ok_or_else(|| format!("GitHub is not connected on host \"{}\"", self.host_id))
    }

    /// Repos the connection can reach — the union across all selected
    /// installations (multi-org), paged, capped at 5 pages per source. The
    /// listing doubles as the repo→installation routing table for token
    /// minting; the cache lives for 10 minutes.
    async fn reachable(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        if self.cfg.mode.as_deref() == Some("pat") {
            if let Ok(Some(token)) = self.pat() {
                out.extend(
                    self.paged_repos(&token, "/user/repos?per_page=100&sort=pushed")
                        .await,
                );
            }
            return out;
        }
        if self.cfg.mode.as_deref() != Some("app") || self.cfg.installation_ids.is_empty() {
            return out;
        }
        let mut map: HashMap<String, String> = HashMap::new();
        for inst in &self.cfg.installation_ids {
            // A failing installation is skipped, not fatal: the union covers
            // the rest, and one broken org must not blind the routing table.
            let Ok(token) = self
                .installation_token(inst, None, REPO_TOKEN_PERMISSIONS)
                .await
            else {
                continue;
            };
            for repo in self
                .paged_repos(&token, "/installation/repositories?per_page=100")
                .await
            {
                if !map.contains_key(&repo) {
                    map.insert(repo.clone(), inst.clone());
                    out.push(repo);
                }
            }
        }
        repo_install_cache()
            .lock()
            .unwrap()
            .insert(self.host_id.clone(), RepoInstallCache { map, at: now_ms() });
        out
    }

    async fn paged_repos(&self, token: &str, first_path: &str) -> Vec<String> {
        let api = self.api();
        let mut out = Vec::new();
        let mut path = first_path.to_string();
        let mut page = 0;
        while page < 5 && !path.is_empty() {
            let Ok(res) = gh(&api, &path, token, "GET", None).await else {
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
            let Ok(j) = res.json::<Value>().await else {
                break;
            };
            // /user/repos answers an array; /installation/repositories wraps one.
            let items = match &j {
                Value::Array(items) => items.clone(),
                _ => j
                    .get("repositories")
                    .and_then(|v| v.as_array())
                    .cloned()
                    .unwrap_or_default(),
            };
            out.extend(
                items
                    .iter()
                    .filter_map(|r| r.get("full_name").and_then(|v| v.as_str()))
                    .map(str::to_string),
            );
            path = next_link_path(&api, &link).unwrap_or_default();
            page += 1;
        }
        out
    }

    /// App mode's easy-setup helper: once appId+key verify, list where the
    /// app is installed so the admin picks the installation instead of
    /// hunting an id. Any failure is an empty list, never a throw.
    pub async fn list_installations(&self) -> Vec<(i64, String)> {
        if self.cfg.app_id.is_empty() || self.cfg.private_key_enc.is_none() {
            return Vec::new();
        }
        let Ok(jwt) = self.app_jwt() else {
            return Vec::new();
        };
        let Ok(res) = gh(&self.api(), "/app/installations", &jwt, "GET", None).await else {
            return Vec::new();
        };
        if !res.status().is_success() {
            return Vec::new();
        }
        let Ok(j) = res.json::<Value>().await else {
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

    /// Where the App stands on `org`. Err is GitHub or the connection
    /// failing, never an answer about the org.
    pub async fn org_installation(&self, org: &str) -> Result<OrgInstallation, String> {
        let jwt = self.app_jwt()?;
        org_installation_with(&self.api(), &jwt, org, &self.cfg.installation_ids).await
    }
}

/// Why this connection cannot create repos at all, or None when it can try.
/// The org allowlist is the registry's (`git_hosts.repo_creation_orgs`); this
/// is the part only the GitHub connection knows.
pub fn repo_creation_blocker(cfg: &GithubConfig) -> Option<String> {
    if cfg.mode.as_deref() != Some("app") {
        return Some(
            "repo creation on GitHub needs the GitHub App connection, and this host connects with a personal access token — an admin can switch it to App mode in Admin → Git hosts"
                .into(),
        );
    }
    if cfg.app_id.is_empty() || cfg.private_key_enc.is_none() || cfg.installation_ids.is_empty() {
        return Some(
            "the GitHub App connection is incomplete (App ID, private key and at least one installation are needed) — an admin finishes it in Admin → Git hosts"
                .into(),
        );
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
                "the GitHub App is installed on {org} (installation {id}), but this connection does not use that installation — an admin selects it in Admin → Git hosts"
            )),
            OrgInstallation::NotInstalled => Some(format!(
                "the GitHub App is not installed on {org} — install it on that org from the App's GitHub page, then select the installation in Admin → Git hosts"
            )),
        }
    }
}

/// `GET /orgs/{org}/installation` with the App JWT: the one installation
/// that owns that org. Picking "any installation" is how a token minted for
/// one org was sent to create a repo in another.
async fn org_installation_with(
    api: &str,
    jwt: &str,
    org: &str,
    selected: &[String],
) -> Result<OrgInstallation, String> {
    let res = gh(
        api,
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
    let j: Value = res
        .json()
        .await
        .map_err(|e| format!("github decode: {e}"))?;
    let id = match j.get("id") {
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::String(s)) => s.clone(),
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

// ── Repo creation (request_repo → a human approves → the App creates) ───────

/// The ruleset a repo Talaria creates gets on its default branch: no
/// force-push, no deletion — and nothing else. No required reviews, no
/// required checks, no bypass list: how the branch is reviewed and merged
/// belongs to the repository's people, never to Talaria.
pub const DEFAULT_BRANCH_RULESET: &str = "Talaria: protect the default branch";

fn default_branch_ruleset_body() -> String {
    json!({
        "name": DEFAULT_BRANCH_RULESET,
        "target": "branch",
        "enforcement": "active",
        "conditions": { "ref_name": { "include": ["~DEFAULT_BRANCH"], "exclude": [] } },
        "rules": [{ "type": "deletion" }, { "type": "non_fast_forward" }],
    })
    .to_string()
}

/// Protect `repo`'s default branch against force-push and deletion with a
/// repository ruleset. Err is the sentence an approver reads: GitHub refuses
/// rulesets on a private repo in an org on the free plan, and an App without
/// the repository Administration permission cannot write one.
async fn protect_default_branch_with(api: &str, token: &str, repo: &str) -> Result<(), String> {
    let res = gh(
        api,
        &format!("/repos/{repo}/rulesets"),
        token,
        "POST",
        Some(&default_branch_ruleset_body()),
    )
    .await?;
    if res.status().is_success() {
        return Ok(());
    }
    let status = res.status().as_u16();
    let text = res.text().await.unwrap_or_default();
    let message = serde_json::from_str::<Value>(&text)
        .ok()
        .and_then(|v| v.get("message").and_then(|m| m.as_str()).map(String::from))
        .unwrap_or_else(|| truncate_utf16(&text, 200).to_string());
    Err(
        if status == 403 && message.to_lowercase().contains("upgrade") {
            format!(
                "GitHub cannot protect a branch of a private repo on this org's plan ({message}) — make the repo public, or protect it once the org's plan allows"
            )
        } else if status == 403 || status == 404 {
            format!(
                "GitHub refused the branch protection ({status}: {message}) — the App needs the repository Administration permission"
            )
        } else {
            format!("GitHub refused the branch protection ({status}: {message})")
        },
    )
}

/// Create a repo in an org (the App must hold the org's Administration
/// permission), with the token of the installation GitHub says owns that
/// org. A missing or unselected installation is refused with the fix.
/// `protect` asks for the default branch to be protected in the same step,
/// with the same token; the repo exists either way, and
/// `CreatedRepo::protection` says what GitHub answered.
async fn create_repo_in(
    gh_conn: &Github,
    org: &str,
    name: &str,
    description: &str,
    visibility: RepoVisibility,
    protect: bool,
) -> Result<CreatedRepo, String> {
    if gh_conn.cfg.mode.as_deref() != Some("app") {
        return Err("repo creation requires the GitHub App connection".into());
    }
    let api = gh_conn.api();
    let jwt = gh_conn.app_jwt()?;
    let installation =
        match org_installation_with(&api, &jwt, org, &gh_conn.cfg.installation_ids).await? {
            OrgInstallation::Selected(id) => id,
            other => return Err(other.refusal(org).unwrap_or_default()),
        };
    let token = gh_conn
        .installation_token(&installation, None, REPO_TOKEN_PERMISSIONS)
        .await?;
    let body = json!({
        "name": name,
        "description": truncate_utf16(description, 300),
        "private": visibility == RepoVisibility::Private,
        "auto_init": true,
    })
    .to_string();
    let res = gh(
        &api,
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
    let j: Value = res
        .json()
        .await
        .map_err(|e| format!("github decode: {e}"))?;
    let full_name = j
        .get("full_name")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    // The repo exists now, whatever the protection answers: a refusal is
    // carried back to the approver, never turned into a failed approval.
    let protection = if protect && !full_name.is_empty() {
        Some(protect_default_branch_with(&api, &token, &full_name).await)
    } else {
        None
    };
    // The pool changed.
    repo_install_cache()
        .lock()
        .unwrap()
        .remove(&gh_conn.host_id);
    Ok(CreatedRepo {
        full_name,
        url: j
            .get("html_url")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        protection,
    })
}

// ── Pull requests: open one, or keep using the one the branch already has ───

fn pr_ref_of(pr: &Value) -> Option<PullRequestRef> {
    Some(PullRequestRef {
        number: pr.get("number").and_then(|v| v.as_i64())?,
        url: pr
            .get("html_url")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
    })
}

/// The open PR whose head is `branch` in `repo`, if there is one. GitHub
/// allows one open PR per head and base, so a branch with an open PR is a
/// branch whose next push already updates it.
async fn find_open_pull_request_with(
    api: &str,
    token: &str,
    repo: &str,
    branch: &str,
) -> Result<Option<PullRequestRef>, String> {
    let owner = repo.split('/').next().unwrap_or(repo);
    let reply = gh_json(
        api,
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

/// Open the PR for `ask.head` — or, when the branch already has an open one
/// (a revise round pushing to the PR a reviewer bounced), keep that PR and
/// comment on it. Opening a second PR for the same head is a 422 from
/// GitHub, and that refusal left the job `started` with nothing to show.
async fn open_or_update_pull_request_with(
    api: &str,
    token: &str,
    repo: &str,
    ask: &PullRequestAsk<'_>,
) -> Result<OpenedPullRequest, String> {
    if let Some(pr) = find_open_pull_request_with(api, token, repo, ask.head).await? {
        return Ok(reuse_pull_request(api, token, repo, pr, ask.update_note).await);
    }
    let body = json!({
        "head": ask.head,
        "base": ask.base,
        "title": ask.title,
        "body": ask.body,
        "draft": ask.draft,
    })
    .to_string();
    let res = gh(
        api,
        &format!("/repos/{repo}/pulls"),
        token,
        "POST",
        Some(&body),
    )
    .await?;
    let status = res.status().as_u16();
    if !res.status().is_success() {
        let text = res.text().await.unwrap_or_default();
        // Opened between the lookup and this POST: GitHub's 422 says a PR
        // already exists for the head, and that PR is this job's.
        if status == 422
            && text.contains("already exists")
            && let Some(pr) = find_open_pull_request_with(api, token, repo, ask.head).await?
        {
            return Ok(reuse_pull_request(api, token, repo, pr, ask.update_note).await);
        }
        return Err(format!(
            "GitHub POST /repos/{repo}/pulls → {status}: {}",
            truncate_utf16(&text, 200)
        ));
    }
    let pr: Value = res
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
    api: &str,
    token: &str,
    repo: &str,
    pr: PullRequestRef,
    note: &str,
) -> OpenedPullRequest {
    let commented = if note.trim().is_empty() {
        false
    } else {
        let body = json!({ "body": note }).to_string();
        match gh_json(
            api,
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

fn open_pull_of(pr: &Value) -> Option<OpenPullRequest> {
    let s =
        |path: &[&str]| -> String { value_at(pr, path).as_str().unwrap_or_default().to_string() };
    Some(OpenPullRequest {
        number: pr.get("number").and_then(|v| v.as_i64())?,
        url: s(&["html_url"]),
        title: s(&["title"]),
        head: s(&["head", "ref"]),
        base: s(&["base", "ref"]),
        author: s(&["user", "login"]),
        draft: pr.get("draft").and_then(|v| v.as_bool()).unwrap_or(false),
    })
}

async fn open_pull_requests_with(
    api: &str,
    token: &str,
    repo: &str,
    limit: u32,
) -> Result<Vec<OpenPullRequest>, String> {
    let reply = gh_json(
        api,
        &format!(
            "/repos/{repo}/pulls?state=open&sort=created&direction=desc&per_page={}",
            limit.clamp(1, 100)
        ),
        token,
        "GET",
        None,
    )
    .await?;
    Ok(reply
        .as_array()
        .map(|prs| prs.iter().filter_map(open_pull_of).collect())
        .unwrap_or_default())
}

async fn pull_request_status_with(
    api: &str,
    token: &str,
    repo: &str,
    number: i64,
) -> Result<Value, String> {
    let pr = gh_json(
        api,
        &format!("/repos/{repo}/pulls/{number}"),
        token,
        "GET",
        None,
    )
    .await?;
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
            api,
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
            api,
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
    let reviews = match gh_json(
        api,
        &format!("/repos/{repo}/pulls/{number}/reviews?per_page=100"),
        token,
        "GET",
        None,
    )
    .await
    {
        Ok(j) => Some(j.as_array().cloned().unwrap_or_default()),
        Err(e) => {
            errors.push(unreadable("reviews", "Pull requests", e));
            None
        }
    };
    let requested: Vec<Value> = pr
        .get("requested_reviewers")
        .and_then(Value::as_array)
        .map(|a| a.iter().map(|u| value_at(u, &["login"])).collect())
        .unwrap_or_default();
    Ok(pr_status_wire(PrStatusParts {
        repo,
        number,
        url: value_at(&pr, &["html_url"]),
        title: value_at(&pr, &["title"]),
        state,
        draft: pr.get("draft").and_then(Value::as_bool).unwrap_or(false),
        base: value_at(&pr, &["base", "ref"]),
        head: value_at(&pr, &["head", "ref"]),
        head_sha,
        mergeable: value_at(&pr, &["mergeable"]),
        merged_at: value_at(&pr, &["merged_at"]),
        runs,
        statuses,
        reviews,
        requested_reviewers: requested,
        errors,
    }))
}

// ── The interface ────────────────────────────────────────────────────────────

impl GitHost for Github {
    fn kind(&self) -> HostKind {
        HostKind::Github
    }

    fn clone_url(&self, repo: &str) -> String {
        format!("{}/{repo}.git", self.web)
    }

    fn repo_url(&self, repo: &str) -> String {
        format!("{}/{repo}", self.web)
    }

    /// PAT login or App name — who the connection acts as, when verifiable.
    async fn status(&self) -> HostStatus {
        let configured =
            |configured: bool, account: Option<String>, error: Option<String>| HostStatus {
                configured,
                account,
                error: error.map(|e| truncate_utf16(&e, 200).to_string()),
            };
        let api = self.api();
        if self.cfg.mode.as_deref() == Some("pat")
            && let Some(enc) = self.cfg.pat_token_enc.clone()
        {
            let token = match self.sb.open(&enc) {
                Ok(t) => t,
                Err(e) => return configured(false, None, Some(format!("github pat unseal: {e}"))),
            };
            return match gh(&api, "/user", &token, "GET", None).await {
                Ok(res) if !res.status().is_success() => configured(
                    true,
                    None,
                    Some(format!("token rejected ({})", res.status().as_u16())),
                ),
                Ok(res) => match res.json::<Value>().await {
                    Ok(j) => configured(
                        true,
                        j.get("login").and_then(|v| v.as_str()).map(String::from),
                        None,
                    ),
                    Err(e) => configured(false, None, Some(format!("github decode: {e}"))),
                },
                Err(e) => configured(false, None, Some(e)),
            };
        }
        if self.cfg.mode.as_deref() == Some("app")
            && !self.cfg.app_id.is_empty()
            && self.cfg.private_key_enc.is_some()
        {
            let jwt = match self.app_jwt() {
                Ok(j) => j,
                Err(e) => return configured(false, None, Some(e)),
            };
            return match gh(&api, "/app", &jwt, "GET", None).await {
                Ok(res) if !res.status().is_success() => configured(
                    true,
                    None,
                    Some(format!(
                        "app credentials rejected ({})",
                        res.status().as_u16()
                    )),
                ),
                Ok(res) => match res.json::<Value>().await {
                    Ok(j) => {
                        let name = j.get("name").and_then(|v| v.as_str()).map(String::from);
                        if self.cfg.installation_ids.is_empty() {
                            return configured(
                                false,
                                name,
                                Some("pick at least one installation".into()),
                            );
                        }
                        configured(true, name, None)
                    }
                    Err(e) => configured(false, None, Some(format!("github decode: {e}"))),
                },
                Err(e) => configured(false, None, Some(e)),
            };
        }
        // Unconfigured (or configured-but-incomplete without a live check):
        // the calm null error.
        HostStatus::default()
    }

    async fn reachable_repos(&self) -> Result<Vec<String>, String> {
        Ok(self.reachable().await)
    }

    async fn repo_login(&self, repo: &str, access: RepoAccess) -> Result<Option<Login>, String> {
        Ok(self
            .token(Some(repo), permissions_for(access))
            .await?
            .map(|password| Login {
                username: "x-access-token".into(),
                password,
            }))
    }

    async fn default_branch(&self, repo: &str) -> Result<String, String> {
        let token = self.token_or_err(repo, REPO_TOKEN_PERMISSIONS).await?;
        let j = gh_json(&self.api(), &format!("/repos/{repo}"), &token, "GET", None).await?;
        j.get("default_branch")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .ok_or_else(|| "github default branch: none in reply".into())
    }

    async fn list_branches(&self, repo: &str) -> Result<Vec<String>, String> {
        let token = self.token_or_err(repo, REPO_TOKEN_PERMISSIONS).await?;
        let reply = gh_json(
            &self.api(),
            &format!("/repos/{repo}/branches?per_page=100"),
            &token,
            "GET",
            None,
        )
        .await?;
        Ok(reply
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|b| b.get("name").and_then(|n| n.as_str()).map(String::from))
                    .collect()
            })
            .unwrap_or_default())
    }

    /// Cut a branch from the base's head. Idempotent-ish: an existing branch
    /// of the same name is left alone (the job resumes on it).
    async fn create_branch(&self, repo: &str, branch: &str, base: &str) -> Result<bool, String> {
        let token = self.token_or_err(repo, REPO_TOKEN_PERMISSIONS).await?;
        let api = self.api();
        let existing = gh(
            &api,
            &format!("/repos/{repo}/git/ref/heads/{}", enc(branch)),
            &token,
            "GET",
            None,
        )
        .await?;
        if existing.status().is_success() {
            return Ok(false);
        }
        let head = gh_json(
            &api,
            &format!("/repos/{repo}/git/ref/heads/{}", enc(base)),
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
            &api,
            &format!("/repos/{repo}/git/refs"),
            &token,
            "POST",
            Some(&json!({ "ref": format!("refs/heads/{branch}"), "sha": sha }).to_string()),
        )
        .await?;
        Ok(true)
    }

    async fn branch_ahead(&self, repo: &str, base: &str, branch: &str) -> Result<i64, String> {
        let token = self.token_or_err(repo, REPO_TOKEN_PERMISSIONS).await?;
        let cmp = gh_json(
            &self.api(),
            &format!("/repos/{repo}/compare/{}...{}", enc(base), enc(branch)),
            &token,
            "GET",
            None,
        )
        .await?;
        Ok(cmp.get("ahead_by").and_then(|v| v.as_i64()).unwrap_or(0))
    }

    fn repo_creation_blocker(&self) -> Option<String> {
        repo_creation_blocker(&self.cfg)
    }

    /// Ask GitHub whether the App is on that org NOW: an org it is not
    /// installed on can never be approved.
    async fn repo_creation_refusal(&self, owner: &str) -> Result<Option<String>, String> {
        if let Some(why) = repo_creation_blocker(&self.cfg) {
            return Ok(Some(why));
        }
        Ok(self.org_installation(owner).await?.refusal(owner))
    }

    async fn create_repo(
        &self,
        owner: &str,
        name: &str,
        description: &str,
        visibility: RepoVisibility,
        protect: bool,
    ) -> Result<CreatedRepo, String> {
        create_repo_in(self, owner, name, description, visibility, protect).await
    }

    async fn find_open_pull_request(
        &self,
        repo: &str,
        branch: &str,
    ) -> Result<Option<PullRequestRef>, String> {
        let token = self.token_or_err(repo, REPO_TOKEN_PERMISSIONS).await?;
        find_open_pull_request_with(&self.api(), &token, repo, branch).await
    }

    async fn open_or_update_pull_request(
        &self,
        repo: &str,
        ask: &PullRequestAsk<'_>,
    ) -> Result<OpenedPullRequest, String> {
        let token = self.token_or_err(repo, REPO_TOKEN_PERMISSIONS).await?;
        open_or_update_pull_request_with(&self.api(), &token, repo, ask).await
    }

    /// A GET with the same read-only token `pr_status` uses.
    async fn open_pull_requests(
        &self,
        repo: &str,
        limit: u32,
    ) -> Result<Vec<OpenPullRequest>, String> {
        let token = self.token_or_err(repo, PR_STATUS_PERMISSIONS).await?;
        open_pull_requests_with(&self.api(), &token, repo, limit).await
    }

    async fn pull_request_status(&self, repo: &str, number: i64) -> Result<Value, String> {
        let token = self.token_or_err(repo, PR_STATUS_PERMISSIONS).await?;
        pull_request_status_with(&self.api(), &token, repo, number).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_repo_scoped_token_asks_for_three_permissions_and_the_retry_for_none() {
        let narrowed = access_tokens_body("acme/widgets", true);
        assert_eq!(
            narrowed,
            r#"{"repositories":["widgets"],"permissions":{"contents":"write","pull_requests":"write","metadata":"read"}}"#
        );
        let v: Value = serde_json::from_str(&narrowed).expect("valid JSON");
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
        assert!(serde_json::from_str::<Value>(&odd).is_ok());
    }

    /// The token app install and update clone with: one repository, read
    /// access to its contents, nothing that writes.
    #[test]
    fn a_read_token_asks_for_contents_read_and_nothing_that_writes() {
        let body = access_tokens_body_for(
            "acme/talaria-app-crm",
            Some(permissions_for(RepoAccess::Read)),
        );
        assert_eq!(
            body,
            r#"{"repositories":["talaria-app-crm"],"permissions":{"contents":"read","metadata":"read"}}"#
        );
        let v: Value = serde_json::from_str(&body).expect("valid JSON");
        let perms = v["permissions"].as_object().unwrap();
        assert!(perms.values().all(|p| p == "read"), "{perms:?}");
        assert_eq!(
            access_tokens_body_for("acme/x", Some(permissions_for(RepoAccess::Write))),
            access_tokens_body("acme/x", true)
        );
    }

    #[test]
    fn an_enterprise_server_answers_under_api_v3() {
        assert_eq!(api_origin_for("https://github.com"), GITHUB_API);
        assert_eq!(api_origin_for("https://github.com/"), GITHUB_API);
        assert_eq!(
            api_origin_for("https://ghe.example.com"),
            "https://ghe.example.com/api/v3"
        );
        let sb = test_box();
        let ghe = Github::new(
            "ghe",
            "https://ghe.example.com/",
            None,
            &json!({}),
            sb.clone(),
        );
        assert_eq!(ghe.api, "https://ghe.example.com/api/v3");
        assert_eq!(
            ghe.clone_url("acme/app"),
            "https://ghe.example.com/acme/app.git"
        );
        let pinned = Github::new(
            "ghe",
            "https://ghe.example.com",
            Some("https://api.ghe.example.com/"),
            &json!({}),
            sb,
        );
        assert_eq!(pinned.api, "https://api.ghe.example.com");
    }

    // ── A loopback GitHub: the workspace's stub-listener pattern ─────────────
    //
    // Each test binds its own listener on 127.0.0.1:0 and points a connection
    // (or an inner `_with` call) at it, so nothing here ever reaches
    // api.github.com and tests in parallel never share a stub. The route
    // closure answers each request by method and path; `seen` records what
    // was asked, in order.

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
        let opened = open_or_update_pull_request_with(
            &base,
            "tok",
            "acme/widgets",
            &ask("round two: fixed the review"),
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
        let body: Value = serde_json::from_str(&comment.body).unwrap();
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
        let opened = open_or_update_pull_request_with(&base, "tok", "acme/widgets", &ask("unused"))
            .await
            .expect("a PR opens");
        assert_eq!(opened.number, 8);
        assert!(!opened.reused && !opened.commented);
        let seen = seen.lock().unwrap().clone();
        let post = seen.iter().find(|r| r.method == "POST").expect("one POST");
        let body: Value = serde_json::from_str(&post.body).unwrap();
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
        let opened = open_or_update_pull_request_with(&base, "tok", "acme/widgets", &ask("note"))
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
        let err = open_or_update_pull_request_with(&base, "tok", "acme/widgets", &ask("note"))
            .await
            .expect_err("not a reuse");
        assert!(err.contains("422") && err.contains("No commits"), "{err}");
    }

    // ── Repo creation routes to the org's own installation (B13) ─────────────

    fn app_conn(base: &str, installations: &[&str], sb: &SecretBox) -> Github {
        let pem = include_str!("../../../tests/it/fixtures/github-jwt-traditional.pem");
        let cfg = GithubConfig {
            mode: Some("app".into()),
            pat_token_enc: None,
            app_id: "12345".into(),
            installation_ids: installations.iter().map(|s| s.to_string()).collect(),
            private_key_enc: Some(sb.seal(pem).expect("the key seals")),
        };
        // Each test its own host id, so the token caches never cross tests.
        let host_id = format!("gh-test-{}", installations.join("-"));
        Github::new(&host_id, GITHUB_WEB, Some(base), &cfg.to_auth(), sb.clone())
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
        let (base, seen) = stub(|req, _| match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/orgs/plugins/installation") => (200, r#"{"id":90002}"#.into()),
            ("POST", "/app/installations/90002/access_tokens") => {
                (201, r#"{"token":"inst-90002"}"#.into())
            }
            ("POST", "/orgs/plugins/repos") => (
                201,
                r#"{"full_name":"plugins/talaria-app-notes","html_url":"https://github.com/plugins/talaria-app-notes"}"#.into(),
            ),
            ("POST", "/repos/plugins/talaria-app-notes/rulesets") => (201, r#"{"id":1}"#.into()),
            _ => (500, "{}".into()),
        })
        .await;
        let conn = app_conn(&base, &["90001", "90002"], &sb);
        let created = conn
            .create_repo(
                "plugins",
                "talaria-app-notes",
                "notes",
                RepoVisibility::Public,
                true,
            )
            .await
            .expect("the repo is created");
        assert_eq!(created.full_name, "plugins/talaria-app-notes");
        assert_eq!(created.protection, Some(Ok(())));
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
        let body: Value = serde_json::from_str(&post.body).unwrap();
        assert_eq!(body["private"], false, "public was asked for");
        assert_eq!(body["auto_init"], true);
    }

    #[tokio::test]
    async fn create_repo_refuses_an_org_the_app_is_not_installed_on() {
        let sb = test_box();
        let (base, seen) = stub(|req, _| match req.path.as_str() {
            "/orgs/elsewhere/installation" => (404, r#"{"message":"Not Found"}"#.into()),
            _ => (500, "{}".into()),
        })
        .await;
        let conn = app_conn(&base, &["90011"], &sb);
        let err = conn
            .create_repo("elsewhere", "x", "", RepoVisibility::Private, false)
            .await
            .expect_err("refused");
        assert!(err.contains("not installed on elsewhere"), "{err}");
        assert_eq!(
            seen.lock().unwrap().len(),
            1,
            "nothing minted, nothing posted"
        );
        // The agent hears the same sentence before any approval.
        let refusal = conn
            .repo_creation_refusal("elsewhere")
            .await
            .expect("GitHub answered")
            .expect("refused");
        assert!(refusal.contains("not installed on elsewhere"), "{refusal}");
    }

    #[tokio::test]
    async fn create_repo_refuses_an_installation_the_admin_did_not_select() {
        let sb = test_box();
        let (base, _) = stub(|req, _| match req.path.as_str() {
            "/orgs/side/installation" => (200, r#"{"id":90099}"#.into()),
            _ => (500, "{}".into()),
        })
        .await;
        let conn = app_conn(&base, &["90021"], &sb);
        let err = conn
            .create_repo("side", "x", "", RepoVisibility::Private, false)
            .await
            .expect_err("refused");
        assert!(
            err.contains("installation 90099") && err.contains("does not use"),
            "{err}"
        );
    }

    #[tokio::test]
    async fn the_default_branch_ruleset_blocks_force_push_and_deletion_and_nothing_else() {
        let sb = test_box();
        let (base, seen) = stub(|req, _| match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/orgs/acme/installation") => (200, r#"{"id":90041}"#.into()),
            ("POST", "/app/installations/90041/access_tokens") => {
                (201, r#"{"token":"inst-90041"}"#.into())
            }
            ("POST", "/orgs/acme/repos") => (
                201,
                r#"{"full_name":"acme/crm","html_url":"https://github.com/acme/crm"}"#.into(),
            ),
            // GitHub's answer for a private repo in a free org.
            ("POST", "/repos/acme/crm/rulesets") => (
                403,
                r#"{"message":"Upgrade to GitHub Pro or make this repository public to enable this feature."}"#.into(),
            ),
            _ => (500, "{}".into()),
        })
        .await;
        let conn = app_conn(&base, &["90041"], &sb);
        let created = conn
            .create_repo("acme", "crm", "", RepoVisibility::Private, true)
            .await
            .expect("the repo is created even when protection is refused");
        assert_eq!(created.full_name, "acme/crm");
        let why = created
            .protection
            .expect("protection was asked for")
            .expect_err("the plan refused it");
        assert!(why.contains("make the repo public"), "{why}");
        let seen = seen.lock().unwrap().clone();
        let ruleset = seen
            .iter()
            .find(|r| r.path == "/repos/acme/crm/rulesets")
            .expect("the ruleset was asked for");
        assert_eq!(
            ruleset.auth, "Bearer inst-90041",
            "the creating installation's token protects the branch"
        );
        let body: Value = serde_json::from_str(&ruleset.body).unwrap();
        assert_eq!(body["enforcement"], "active");
        assert_eq!(
            body["conditions"]["ref_name"]["include"],
            json!(["~DEFAULT_BRANCH"])
        );
        let rules: Vec<&str> = body["rules"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|r| r["type"].as_str())
            .collect();
        assert_eq!(
            rules,
            ["deletion", "non_fast_forward"],
            "no required reviews, no required checks"
        );
        assert!(body.get("bypass_actors").is_none());
        // Not asked: no ruleset call at all.
        let (base, seen) = stub(|req, _| match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/orgs/acme/installation") => (200, r#"{"id":90042}"#.into()),
            ("POST", "/app/installations/90042/access_tokens") => (201, r#"{"token":"t"}"#.into()),
            ("POST", "/orgs/acme/repos") => (
                201,
                r#"{"full_name":"acme/crm","html_url":"https://github.com/acme/crm"}"#.into(),
            ),
            _ => (500, "{}".into()),
        })
        .await;
        let conn = app_conn(&base, &["90042"], &sb);
        let created = conn
            .create_repo("acme", "crm", "", RepoVisibility::Private, false)
            .await
            .unwrap();
        assert!(created.protection.is_none());
        assert!(
            !seen
                .lock()
                .unwrap()
                .iter()
                .any(|r| r.path.contains("rulesets"))
        );
    }

    #[tokio::test]
    async fn a_private_repo_is_the_default_and_a_403_names_the_permission() {
        let sb = test_box();
        let (base, seen) = stub(|req, _| match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/orgs/acme/installation") => (200, r#"{"id":"90031"}"#.into()),
            ("POST", "/app/installations/90031/access_tokens") => (201, r#"{"token":"t"}"#.into()),
            ("POST", "/orgs/acme/repos") => {
                (403, r#"{"message":"Resource not accessible"}"#.into())
            }
            _ => (500, "{}".into()),
        })
        .await;
        let conn = app_conn(&base, &["90031"], &sb);
        let err = conn
            .create_repo("acme", "x", "", RepoVisibility::default(), false)
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
        let body: Value = serde_json::from_str(&post.body).unwrap();
        assert_eq!(body["private"], true);
    }

    #[test]
    fn repo_creation_is_refused_up_front_outside_app_mode() {
        let sb = test_box();
        let mut cfg = app_conn("http://x", &["1"], &sb).cfg;
        assert_eq!(repo_creation_blocker(&cfg), None);
        cfg.mode = Some("pat".into());
        let pat = repo_creation_blocker(&cfg).expect("PAT mode cannot create");
        assert!(pat.contains("GitHub App"), "{pat}");
        cfg.mode = Some("app".into());
        cfg.installation_ids.clear();
        assert!(repo_creation_blocker(&cfg).unwrap().contains("incomplete"));
        assert_eq!(
            RepoVisibility::parse("public"),
            Some(RepoVisibility::Public)
        );
        assert_eq!(RepoVisibility::parse("internal"), None);
        assert_eq!(RepoVisibility::default().as_str(), "private");
    }

    // ── PR status: read-only, every call a GET ────────────────────────────────

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
        let status = pull_request_status_with(&base, "tok", "acme/widgets", 7)
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
        let status = pull_request_status_with(&base, "tok", "acme/widgets", 5)
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
        let v: Value = serde_json::from_str(&body).unwrap();
        let perms = v["permissions"].as_object().unwrap();
        assert!(perms.values().all(|p| p == "read"), "{perms:?}");
        assert_eq!(perms.get("checks").and_then(|p| p.as_str()), Some("read"));
        assert_eq!(v["repositories"], json!(["widgets"]));
    }

    #[test]
    fn next_link_path_extracts_the_path_under_the_connections_origin() {
        assert_eq!(
            next_link_path(
                GITHUB_API,
                r#"<https://api.github.com/user/repos?per_page=100&sort=pushed&page=2>; rel="next", <https://api.github.com/user/repos?page=1>; rel="first""#
            )
            .as_deref(),
            Some("/user/repos?per_page=100&sort=pushed&page=2")
        );
        // An Enterprise Server pages under its own origin.
        assert_eq!(
            next_link_path(
                "https://ghe.example.com/api/v3",
                r#"<https://ghe.example.com/api/v3/installation/repositories?page=2>; rel="next""#
            )
            .as_deref(),
            Some("/installation/repositories?page=2")
        );
        // A next link to anywhere else is not followed.
        assert_eq!(
            next_link_path(GITHUB_API, r#"<https://example.com/x>; rel="next""#),
            None
        );
        assert_eq!(next_link_path(GITHUB_API, ""), None);
    }

    #[test]
    fn legacy_installation_migrates_on_read() {
        let cfg = GithubConfig::from_auth(&json!({
            "mode": "app",
            "app": { "appId": "123", "installationId": "99", "privateKeyEnc": "x" }
        }));
        assert_eq!(cfg.installation_ids, vec!["99".to_string()]);
        assert_eq!(cfg.app_id, "123");
    }

    #[test]
    fn a_patch_seals_what_arrives_and_keeps_what_does_not() {
        let sb = test_box();
        let cur = GithubConfig::from_auth(&json!({
            "mode": "app",
            "app": { "appId": "1", "installationIds": ["9"], "privateKeyEnc": "sealed-key" }
        }));
        let next = cur
            .patched(
                &sb,
                &GithubConfigPatch {
                    mode: PatchField::Unset,
                    pat_token: PatchField::Set(Some("ghp_secret")),
                    app_id: None,
                    installation_ids: None,
                    private_key: PatchField::Unset,
                },
            )
            .unwrap();
        assert_eq!(next.private_key_enc.as_deref(), Some("sealed-key"));
        assert_eq!(next.installation_ids, vec!["9".to_string()]);
        let pat = next.pat_token_enc.clone().expect("sealed");
        assert_ne!(pat, "ghp_secret", "never stored in the clear");
        assert_eq!(sb.open(&pat).unwrap(), "ghp_secret");
        let auth = next.to_auth();
        assert!(!auth.to_string().contains("ghp_secret"));
        let shown = next.redacted();
        assert_eq!(shown["patSet"], true);
        assert_eq!(shown["app"]["keySet"], true);
        assert!(!shown.to_string().contains("sealed-key"));
    }
}
