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
// per-repo-scoped installation tokens that are never wider than asked (met
// with what the installation holds, checked when GitHub returns them), the
// GitHub permission catalog, the repo→installation routing table, the live
// status and installation listing, repo creation (on the org's own
// installation, with the default-branch ruleset), and the branch /
// pull-request reads and writes the workbench drives. Which agent may touch
// which repo is the registry's grant table, not this crate's: connecting
// GitHub grants nothing to anyone by itself.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

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
    CreatedRepo, GitHost, HostKind, HostStatus, Level, Login, OpenPullRequest, OpenedPullRequest,
    PermissionSet, PullRequestAsk, PullRequestRef, RepoAccess, RepoVisibility, checks_verdict,
    pr_number_from_url, review_decision,
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

// ── The permission catalog ───────────────────────────────────────────────────
//
// GitHub's own names for what an installation token may carry. Two tables and
// no prefix rule for what may be granted: a key is grantable only when it is
// listed with its levels in GRANTABLE. The never-list is the one place a
// prefix counts, because it only ever refuses.

/// The keys no repo-scoped token Talaria mints may carry, whatever was asked
/// for and whatever the installation holds: the ones that rewrite who may do
/// what to a repository or an org, and a user account's own. A mint never
/// asks for one, and a token GitHub returns carrying one is revoked.
/// `administration` is here, so it is never grantable, on a token or through
/// a tool. So is every `organization_*` key ([`never_on_repo_token`]).
pub const NEVER_ON_REPO_TOKEN: &[&str] = &[
    "administration",
    "members",
    "custom_properties_for_organizations",
    "enterprise_custom_properties_for_organizations",
    "repository_creation",
    "installation_repositories",
    // The account keys: a user's own data, never a repository's.
    "email_addresses",
    "followers",
    "git_ssh_keys",
    "gpg_keys",
    "interaction_limits",
    "profile",
    "starring",
];

/// Whether `key` may never ride on a repo-scoped token: it is listed in
/// [`NEVER_ON_REPO_TOKEN`], or it is any `organization_*` key.
pub fn never_on_repo_token(key: &str) -> bool {
    key.starts_with("organization_") || NEVER_ON_REPO_TOKEN.contains(&key)
}

/// Who spends a grantable permission: the sandbox's git token, or a Talaria
/// tool that mints its own token inside the api and never hands it out.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SpentBy {
    Token,
    Tool(&'static str),
}

/// One permission an admin may grant an agent above the platform's base.
#[derive(Debug, Clone, Copy)]
pub struct Grantable {
    pub key: &'static str,
    /// The levels it may be granted at. `workflows`, for one, is write-only.
    pub levels: &'static [Level],
    pub spent_by: SpentBy,
    /// The sentence an admin reads before granting it.
    pub risk: &'static str,
}

/// THE ONLY HARD-CODED PERMISSION TABLE: what may be granted above the base,
/// at which levels, and what spends it. It is empty until a Talaria surface
/// spends a key, because a key nothing spends is a key no grant may name. The
/// per-agent grant adds `workflows`, and each tool adds the keys it spends.
pub const GRANTABLE: &[Grantable] = &[];

/// The catalog entry that lets `key` be granted at `level`. Err is the
/// sentence a refusal carries: a key no repo token may carry, a key nothing
/// spends, or a level the key is not granted at.
pub fn grantable(key: &str, level: Level) -> Result<&'static Grantable, String> {
    if never_on_repo_token(key) {
        return Err(format!(
            "{key} is never granted to an agent: no repository token Talaria mints may carry it"
        ));
    }
    let entry = GRANTABLE
        .iter()
        .find(|g| g.key == key)
        .ok_or_else(|| format!("no Talaria surface spends the GitHub permission {key} yet"))?;
    if !entry.levels.contains(&level) {
        let levels: Vec<&str> = entry.levels.iter().map(|l| l.as_str()).collect();
        return Err(format!(
            "{key} is granted at {} only, not {level}",
            levels.join(" or ")
        ));
    }
    Ok(entry)
}

// ── The asks ─────────────────────────────────────────────────────────────────

/// The permissions a REPO-SCOPED token asks for: what a job's git push, a
/// branch create, a default-branch read and a pull-request open need, and
/// nothing an installation might also hold (administration, workflows,
/// secrets, packages). The sandbox's credential helper hands this token to
/// model-authored code; a permission it does not carry is a permission that
/// code cannot spend.
pub const REPO_TOKEN_PERMISSIONS: &[(&str, Level)] = &[
    ("contents", Level::Write),
    ("pull_requests", Level::Write),
    ("metadata", Level::Read),
];

/// The permissions a READ-ONLY repo token asks for: a clone or a fetch of the
/// repository and nothing else. App install and update use it
/// (talaria-apps): this instance reads a plugin's code from a repository the
/// GitHub connection can see, and must not be able to push to it.
pub const REPO_READ_PERMISSIONS: &[(&str, Level)] =
    &[("contents", Level::Read), ("metadata", Level::Read)];

/// The permission set a [`RepoAccess`] narrows a token to.
pub fn permissions_for(access: RepoAccess) -> PermissionSet {
    PermissionSet::of(match access {
        RepoAccess::Write => REPO_TOKEN_PERMISSIONS,
        RepoAccess::Read => REPO_READ_PERMISSIONS,
    })
}

/// What `pr_status` reads with, and nothing it could write with: the pull
/// request, its check runs and commit statuses, and the reviews. Checks and
/// statuses are not in [`REPO_TOKEN_PERMISSIONS`] (a push never needs them),
/// so a PR's CI is unreadable on a private repo with that token. This one
/// never leaves the api process. An installation that lacks one of these is
/// asked for the rest and never for more, and the answer says which read is
/// missing.
pub const PR_STATUS_PERMISSIONS: &[(&str, Level)] = &[
    ("pull_requests", Level::Read),
    ("checks", Level::Read),
    ("statuses", Level::Read),
    ("metadata", Level::Read),
];

/// The body of an access_tokens request: the one repository, and the
/// permissions the token is narrowed to. There is no other shape. A body
/// without `permissions` asks for a token carrying everything the
/// installation holds, and GitHub would mint it.
pub fn access_tokens_body(repo: &str, permissions: &PermissionSet) -> String {
    let name = repo.rsplit('/').next().unwrap_or(repo);
    json!({ "repositories": [name], "permissions": permissions.to_json() }).to_string()
}

// ── The process caches ───────────────────────────────────────────────────────

/// What a cached installation token is filed under. A struct, not a joined
/// string: a repo path may hold any character a separator could be, and
/// forgetting a host must match that host's entries exactly, never by prefix.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct TokenKey {
    host: String,
    inst: String,
    /// None for an installation-wide token (the listing, repo creation).
    repo: Option<String>,
    /// The ask's canonical JSON, so two spellings of one set are one key.
    /// Empty for an installation-wide token, which asks for nothing in
    /// particular.
    perms: String,
}

struct CachedToken {
    token: String,
    expires_at: i64,
    /// What GitHub said the token carries.
    carried: PermissionSet,
    /// What the ask named that the installation did not hold at the mint.
    shortfall: PermissionSet,
}

type TokenCache = Mutex<HashMap<TokenKey, CachedToken>>;
fn install_tokens() -> &'static TokenCache {
    static CACHE: OnceLock<TokenCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// A cached token is served only while it has more than this left. Git may
/// hold it for a whole push, and a token that expires mid-push fails the push.
const SERVE_MARGIN_MS: i64 = 5 * 60_000;

/// How long what an installation holds is trusted before it is read again,
/// when it was read from GitHub.
const HOLDINGS_TTL_MS: i64 = 5 * 60_000;

/// The same, when it was read from the registry's record ([`HoldingsStore`]).
/// The record is kept current by a refresh that may run in another process,
/// so a minute is how long that refresh takes to reach this process's mints.
const RECORDED_HOLDINGS_TTL_MS: i64 = 60_000;

/// How long a repo→installation route is trusted, whether the listing or a
/// lookup learned it.
const ROUTING_TTL_MS: i64 = 10 * 60_000;

/// What each installation holds, per (host, installation), with when it was
/// read and how long that read is trusted.
type HoldingsCache = Mutex<HashMap<(String, String), (PermissionSet, i64, i64)>>;
fn holdings_cache() -> &'static HoldingsCache {
    static CACHE: OnceLock<HoldingsCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Keep `held` as what installation `installation_id` of host `host_id`
/// holds, trusted for `ttl_ms` from now.
fn remember_holdings(host_id: &str, installation_id: &str, held: &PermissionSet, ttl_ms: i64) {
    holdings_cache().lock().unwrap().insert(
        (host_id.to_string(), installation_id.to_string()),
        (held.clone(), now_ms(), ttl_ms),
    );
}

// ── Where the holdings are read first ────────────────────────────────────────

/// What a [`HoldingsStore`] read answers with.
pub type HeldFuture<'a> = Pin<Box<dyn Future<Output = Option<PermissionSet>> + Send + 'a>>;

/// The record a connection reads what an installation holds from before it
/// asks GitHub: the registry's `git_host_installations` table, which the
/// holdings refresh keeps (talaria-git-hosts). This crate has no database, so
/// the registry hands the record in when it connects a host
/// ([`Github::with_holdings`]). A connection without one asks GitHub, as the
/// adapter always did, and so does one whose record has no usable answer.
pub trait HoldingsStore: Send + Sync {
    /// What installation `installation_id` of host `host_id` held at its last
    /// good check. None when the record cannot say: there is no row, the
    /// last check failed, or it is too old to trust. None sends the read to
    /// GitHub; it never means "nothing".
    fn held<'a>(&'a self, host_id: &'a str, installation_id: &'a str) -> HeldFuture<'a>;

    /// GitHub refused a mint on that installation (a 422), so the record may
    /// be out of date: bring it up to date. The mint does not wait for it.
    fn refused(&self, host_id: &str, installation_id: &str);
}

/// One host's routes: each repo, by its lowercased `owner/name` (GitHub names
/// ignore case), with the installation it is minted on and when that was
/// learned.
type RoutingTable = HashMap<String, (String, i64)>;

/// The repo→installation routing table, per host. The repository listing
/// fills it whole; a mint for a repo the listing did not show asks GitHub
/// and adds that one answer.
fn repo_install_cache() -> &'static Mutex<HashMap<String, RoutingTable>> {
    static CACHE: OnceLock<Mutex<HashMap<String, RoutingTable>>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Route `repo` on host `host_id` to installation `inst`, as of now.
fn learn_route(host_id: &str, repo: &str, inst: &str) {
    repo_install_cache()
        .lock()
        .unwrap()
        .entry(host_id.to_string())
        .or_default()
        .insert(repo.to_ascii_lowercase(), (inst.to_string(), now_ms()));
}

/// Forget every cached token, holdings read and routing entry of host
/// `host_id`: its credentials changed, and nothing minted or read under the
/// old ones may outlive them.
pub fn forget_host(host_id: &str) {
    install_tokens()
        .lock()
        .unwrap()
        .retain(|k, _| k.host != host_id);
    holdings_cache()
        .lock()
        .unwrap()
        .retain(|(host, _), _| host != host_id);
    repo_install_cache().lock().unwrap().remove(host_id);
}

// ── Minting, never wider than asked ──────────────────────────────────────────

/// Why no token came out of a mint. Every arm says it in a sentence an admin
/// or an agent can act on (its `Display`), and a caller that only passes the
/// error on gets that sentence (`From<MintError> for String`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MintError {
    /// GitHub refused the mint: its status, its own message, and what the
    /// ask named that the installation was known not to hold.
    Refused {
        repo: String,
        status: u16,
        message: String,
        shortfall: PermissionSet,
    },
    /// GitHub minted a token for something other than what was asked: another
    /// repository, a key no repo token may carry, or a level above the ask.
    /// It was revoked on GitHub (`revoked`, when GitHub confirmed it) and was
    /// never used.
    Overbroad {
        repo: String,
        why: String,
        revoked: bool,
    },
    /// The installation holds none of what was asked, so nothing was asked.
    NothingHeld {
        repo: String,
        shortfall: PermissionSet,
    },
    /// The ask names nothing a repository token may carry: it is empty, or
    /// every key in it is one no repo token may carry. An empty permissions
    /// block is never posted, because GitHub documents only the omitted one
    /// (a token carrying everything the installation holds), and nothing
    /// says `{}` is not read the same way.
    NothingAsked { repo: String },
    /// The host has several installations, and GitHub says none of the ones
    /// this connection uses covers the repo. `unselected` is the installation
    /// that does, when GitHub named one the admin has not selected.
    Unrouted {
        repo: String,
        unselected: Option<String>,
    },
    /// The connection, the transport, or a reply that could not be read.
    Other(String),
}

impl std::fmt::Display for MintError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MintError::Refused {
                repo,
                status,
                message,
                shortfall,
            } => {
                write!(f, "GitHub refused a token for {repo} ({status}: {message})")?;
                if !shortfall.is_empty() {
                    write!(
                        f,
                        " — the App's installation does not hold {shortfall}; the App's owner adds it to the App, then an org owner accepts it on GitHub"
                    )?;
                }
                Ok(())
            }
            MintError::Overbroad { repo, why, revoked } => write!(
                f,
                "GitHub minted a token for {repo} wider than Talaria asked for ({why}); {}",
                if *revoked {
                    "it was revoked and never used"
                } else {
                    "it was never used, but GitHub did not confirm the revoke, so it lives until it expires within the hour"
                }
            ),
            MintError::NothingHeld { repo, shortfall } => write!(
                f,
                "the GitHub App's installation holds none of what a token for {repo} needs ({shortfall}), so none was asked for — the App's owner adds it to the App, then an org owner accepts it on GitHub"
            ),
            MintError::NothingAsked { repo } => write!(
                f,
                "a token for {repo} was asked for nothing a repository token may carry, so none was asked of GitHub"
            ),
            MintError::Unrouted {
                repo,
                unselected: Some(id),
            } => write!(
                f,
                "the GitHub App's installation {id} covers {repo}, but this host's connection does not use that installation — an admin selects it in Admin → Git hosts"
            ),
            MintError::Unrouted {
                repo,
                unselected: None,
            } => write!(
                f,
                "no installation of this host's GitHub App covers {repo} — install the App on that repository (or add the repository to its installation), then select the installation in Admin → Git hosts"
            ),
            MintError::Other(why) => f.write_str(why),
        }
    }
}

impl From<MintError> for String {
    fn from(e: MintError) -> String {
        e.to_string()
    }
}

/// A repo-scoped installation token, and what is known about it.
struct Minted {
    token: String,
    /// What GitHub said the token carries.
    carried: PermissionSet,
    /// What the ask named that the installation does not hold.
    shortfall: PermissionSet,
    /// Minted by this call, rather than served from the cache.
    minted: bool,
}

/// An INSTALLATION-WIDE token: every repository the installation covers, with
/// every permission it holds. Deliberately not a `String`: only the listing
/// and repo creation read it (through `bearer`), and nothing that hands a
/// credential out (`token`, `repo_login`) can return one.
struct InstallationWide(String);

impl InstallationWide {
    fn bearer(&self) -> &str {
        &self.0
    }
}

/// The permissions a mint posts for `ask`, and what the ask lacks. The first
/// is the ask met with what the installation holds, minus every key a repo
/// token may never carry; the second is each asked key the installation does
/// not hold at the asked level (never one no repo token may carry: that one
/// is not asked for at all, so it is no gap an admin should fill). Holdings
/// unknown: the ask goes unchanged (and GitHub is then the one to refuse it),
/// and nothing is known to be short.
fn narrow(ask: &PermissionSet, held: Option<&PermissionSet>) -> (PermissionSet, PermissionSet) {
    let allowed = ask.without(never_on_repo_token);
    match held {
        Some(held) => (allowed.meet(held), allowed.missing_from(held)),
        None => (allowed, PermissionSet::new()),
    }
}

/// Check the token GitHub minted against what was posted (`asked`). Refused,
/// with the reason: a token for anything but exactly `repo` (the selection is
/// `selected`, with that one repository), and a token that carries a key no
/// repo token may carry or an asked key above the asked level. Accepted, it
/// answers what the token carries, and the keys it carries that were not
/// asked for. `metadata` is implied on every token. What else GitHub implies
/// is not known yet, so the caller logs any other unasked key rather than
/// refusing it.
fn check_reply(
    repo: &str,
    asked: &PermissionSet,
    reply: &Value,
) -> Result<(PermissionSet, PermissionSet), String> {
    let selection = reply.get("repository_selection").and_then(Value::as_str);
    if selection != Some("selected") {
        return Err(format!(
            "its repository selection is {}, not the one repository asked for",
            selection.unwrap_or("unstated")
        ));
    }
    let names: Vec<&str> = reply
        .get("repositories")
        .and_then(Value::as_array)
        .map(|repos| {
            repos
                .iter()
                .map(|r| r.get("full_name").and_then(Value::as_str).unwrap_or("?"))
                .collect()
        })
        .unwrap_or_default();
    if !matches!(names.as_slice(), [only] if only.eq_ignore_ascii_case(repo)) {
        return Err(format!(
            "it names the repositories [{}], not {repo} alone",
            names.join(", ")
        ));
    }
    let carried = match reply.get("permissions") {
        Some(p) => PermissionSet::from_json(p)
            .map_err(|e| format!("its permissions cannot be read ({e})"))?,
        None => return Err("it states no permissions".into()),
    };
    for (key, level) in carried.iter() {
        if never_on_repo_token(key) {
            return Err(format!(
                "it carries {key}:{level}, which no repository token may carry"
            ));
        }
        if let Some(asked_level) = asked.get(key)
            && level > asked_level
        {
            return Err(format!(
                "it carries {key}:{level}, above the {asked_level} asked for"
            ));
        }
    }
    let unasked = carried.without(|k| k == "metadata" || asked.contains(k));
    Ok((carried, unasked))
}

/// How long a revoke may take before it counts as unconfirmed. The shared
/// client bounds only the connect, and the caller waits on it: a mint that
/// threw a token back, or a holdings refresh that holds its host while it
/// revokes.
const REVOKE_TIMEOUT: Duration = Duration::from_secs(20);

/// Revoke an installation token on GitHub (`DELETE /installation/token`,
/// authenticated by the token itself). True when GitHub confirmed it within
/// [`REVOKE_TIMEOUT`].
async fn revoke_installation_token(api: &str, token: &str) -> bool {
    tokio::time::timeout(
        REVOKE_TIMEOUT,
        gh(api, "/installation/token", token, "DELETE", None),
    )
    .await
    .is_ok_and(|sent| sent.is_ok_and(|res| res.status().is_success()))
}

/// GitHub's own words for a refusal: the reply's `message`, else the body
/// itself, cut short.
fn github_message(text: &str) -> String {
    let parsed = serde_json::from_str::<Value>(text).ok();
    match parsed
        .as_ref()
        .and_then(|v| v.get("message"))
        .and_then(Value::as_str)
    {
        Some(message) => truncate_utf16(message, 300).to_string(),
        None => truncate_utf16(text.trim(), 200).to_string(),
    }
}

/// The expiry GitHub sent with a token, in epoch ms. GitHub sends RFC 3339;
/// the cache only needs a comparable instant. A parse failure means "not
/// cacheable", never "no token".
fn expires_at_of(reply: &Value) -> i64 {
    reply
        .get("expires_at")
        .and_then(Value::as_str)
        .and_then(talaria_tz::parse_rfc3339_ms)
        .unwrap_or(0)
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

// ── What the App asks for, and what an installation accepted ────────────────

/// One installation of the App, as GitHub describes it
/// (`GET /app/installations/{id}`, or an item of `GET /app/installations`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallationSnapshot {
    pub id: String,
    /// The login of the account (an org, or a user) it is installed on.
    pub account: String,
    /// What the installation holds: the permissions its account accepted,
    /// with `metadata: read` always in.
    pub permissions: PermissionSet,
    /// `all` or `selected`.
    pub repository_selection: Option<String>,
    /// When the account suspended it, if it did (RFC 3339).
    pub suspended_at: Option<String>,
    /// Its settings page on GitHub, where an owner of the account reviews
    /// and accepts a permission change.
    pub html_url: Option<String>,
}

/// What an installation object says it holds: its `permissions`, with
/// `metadata: read` always in. A set this crate cannot read is an error,
/// never an empty set.
fn held_in(installation: &Value) -> Result<PermissionSet, String> {
    let mut permissions =
        PermissionSet::from_json(installation.get("permissions").unwrap_or(&Value::Null))?;
    permissions.raise("metadata", Level::Read);
    Ok(permissions)
}

/// An installation object's string field, when it has one.
fn text_in(installation: &Value, key: &str) -> Option<String> {
    installation
        .get(key)
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// The login of the account an installation object is on, or "".
fn account_in(installation: &Value) -> String {
    installation
        .get("account")
        .and_then(|a| a.get("login"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

impl InstallationSnapshot {
    /// Read GitHub's installation object. A permission set this crate cannot
    /// read is an error, never an empty set: what it holds is then unknown.
    pub fn from_reply(v: &Value) -> Result<InstallationSnapshot, String> {
        let id = installation_id_of(v).ok_or("no id in the installation")?;
        Ok(InstallationSnapshot {
            id,
            account: account_in(v),
            permissions: held_in(v)?,
            repository_selection: text_in(v, "repository_selection"),
            suspended_at: text_in(v, "suspended_at"),
            html_url: text_in(v, "html_url"),
        })
    }
}

/// One item of the installation picker (an item of `GET /app/installations`):
/// the id an admin selects, the account, and what the installation holds.
/// Unlike [`InstallationSnapshot`], what it holds may be unknown: an item
/// whose permissions this crate cannot read is still listed, because the
/// admin may need to select it, and the refresh then records why it could
/// not be read.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallationListing {
    pub id: String,
    pub account: String,
    /// None when GitHub's permissions block could not be read.
    pub permissions: Option<PermissionSet>,
    /// `all` or `selected`.
    pub repository_selection: Option<String>,
    /// RFC 3339.
    pub suspended_at: Option<String>,
}

impl InstallationListing {
    /// An item of GitHub's list, or None when it has no id or no account
    /// login (the picker has nothing to show for one).
    pub fn from_item(v: &Value) -> Option<InstallationListing> {
        let id = installation_id_of(v)?;
        let account = account_in(v);
        if account.is_empty() {
            return None;
        }
        let permissions = held_in(v)
            .inspect_err(|e| {
                tracing::warn!(
                    "[github] what installation {id} ({account}) holds could not be read from the listing: {e}"
                );
            })
            .ok();
        Some(InstallationListing {
            id,
            account,
            permissions,
            repository_selection: text_in(v, "repository_selection"),
            suspended_at: text_in(v, "suspended_at"),
        })
    }

    /// The picker's item on the wire: the id stays a number, and
    /// `permissions` is null when what the installation holds is unknown.
    pub fn listing_wire(&self) -> Value {
        json!({
            "id": self.id.parse::<i64>().map(Value::from).unwrap_or_else(|_| Value::from(self.id.clone())),
            "account": self.account,
            "permissions": self.permissions.as_ref().map(PermissionSet::to_json),
            "repositorySelection": self.repository_selection,
            "suspended": self.suspended_at.is_some(),
        })
    }
}

/// What [`Github::revoke_tokens_beyond`] did.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Revocation {
    /// Tokens GitHub confirmed revoked.
    pub revoked: usize,
    /// Tokens dropped from the cache whose revoke GitHub did not confirm:
    /// they live until they expire, within the hour.
    pub failed: usize,
    /// The permissions those tokens carried that the installation no longer
    /// holds.
    pub keys: PermissionSet,
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
    /// The registry's record of what each installation holds, read before
    /// GitHub is asked. None: GitHub is asked.
    holdings_store: Option<Arc<dyn HoldingsStore>>,
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
            holdings_store: None,
        }
    }

    /// This connection, reading what its installations hold from `store`
    /// before it asks GitHub ([`HoldingsStore`]).
    pub fn with_holdings(mut self, store: Arc<dyn HoldingsStore>) -> Github {
        self.holdings_store = Some(store);
        self
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

    /// What installation `installation_id` holds: the permissions its
    /// account accepted, with `metadata: read` always in, because GitHub
    /// grants it to every App. The registry's record answers first, when the
    /// connection has one and it can say ([`HoldingsStore`], kept a minute);
    /// otherwise GitHub does (`GET /app/installations/{id}`, kept five
    /// minutes). [`forget_host`] clears both, and a failure is never kept.
    /// Err means UNKNOWN, never "nothing".
    pub async fn installation_permissions(
        &self,
        installation_id: &str,
    ) -> Result<PermissionSet, String> {
        self.holdings(installation_id, false).await
    }

    /// [`Self::installation_permissions`], asked of GitHub past the cache and
    /// the record when `fresh`.
    async fn holdings(&self, installation_id: &str, fresh: bool) -> Result<PermissionSet, String> {
        if !fresh {
            let key = (self.host_id.clone(), installation_id.to_string());
            let kept = holdings_cache().lock().unwrap().get(&key).cloned();
            if let Some((held, at, ttl)) = kept
                && now_ms() - at < ttl
            {
                return Ok(held);
            }
            if let Some(store) = &self.holdings_store
                && let Some(mut held) = store.held(&self.host_id, installation_id).await
            {
                held.raise("metadata", Level::Read);
                remember_holdings(
                    &self.host_id,
                    installation_id,
                    &held,
                    RECORDED_HOLDINGS_TTL_MS,
                );
                return Ok(held);
            }
        }
        Ok(self.installation(installation_id).await?.permissions)
    }

    /// One installation of this App as GitHub answers it now
    /// (`GET /app/installations/{id}`, with the App JWT): the account it is
    /// on, what it holds (`metadata: read` always in), which repositories it
    /// covers, whether it is suspended, and its settings page. What it holds
    /// is kept for the mint, so a read here is a read the next token uses.
    pub async fn installation(
        &self,
        installation_id: &str,
    ) -> Result<InstallationSnapshot, String> {
        let jwt = self.app_jwt()?;
        let reply = gh_json(
            &self.api(),
            &format!("/app/installations/{}", enc(installation_id)),
            &jwt,
            "GET",
            None,
        )
        .await?;
        let snapshot = InstallationSnapshot::from_reply(&reply)
            .map_err(|e| format!("installation {installation_id}: {e}"))?;
        remember_holdings(
            &self.host_id,
            installation_id,
            &snapshot.permissions,
            HOLDINGS_TTL_MS,
        );
        Ok(snapshot)
    }

    /// What the App's registration asks every installation for
    /// (`GET /app`, with the App JWT). An installation holds at most this;
    /// what it has not accepted yet is waiting for an owner of its account.
    pub async fn app_permissions(&self) -> Result<PermissionSet, String> {
        let jwt = self.app_jwt()?;
        let app = gh_json(&self.api(), "/app", &jwt, "GET", None).await?;
        PermissionSet::from_json(app.get("permissions").unwrap_or(&Value::Null))
            .map_err(|e| format!("the App's permissions: {e}"))
    }

    /// Revoke every token this process keeps for installation
    /// `installation_id` that carries a permission the installation no longer
    /// holds (`held`, just read): each is dropped from the cache and deleted
    /// on GitHub (`DELETE /installation/token`). A key an org owner took away
    /// must not outlive the decision in a token Talaria minted before it, and
    /// the next mint asks for what is held now. `metadata` is never counted:
    /// every token carries it.
    pub async fn revoke_tokens_beyond(
        &self,
        installation_id: &str,
        held: &PermissionSet,
    ) -> Revocation {
        let beyond =
            |carried: &PermissionSet| carried.without(|k| k == "metadata").missing_from(held);
        let doomed: Vec<(String, PermissionSet)> = {
            let mut cache = install_tokens().lock().unwrap();
            let keys: Vec<TokenKey> = cache
                .iter()
                .filter(|(k, t)| {
                    k.host == self.host_id
                        && k.inst == installation_id
                        && !beyond(&t.carried).is_empty()
                })
                .map(|(k, _)| k.clone())
                .collect();
            keys.iter()
                .filter_map(|k| cache.remove(k))
                .map(|t| {
                    let keys = beyond(&t.carried);
                    (t.token, keys)
                })
                .collect()
        };
        let api = self.api();
        let mut out = Revocation::default();
        for (token, keys) in doomed {
            out.keys = out.keys.union(&keys);
            if revoke_installation_token(&api, &token).await {
                out.revoked += 1;
            } else {
                out.failed += 1;
            }
        }
        out
    }

    /// The holdings, or None (logged) when they cannot be read.
    async fn known_holdings(&self, installation_id: &str, fresh: bool) -> Option<PermissionSet> {
        match self.holdings(installation_id, fresh).await {
            Ok(held) => Some(held),
            Err(e) => {
                tracing::warn!(
                    "[github] what installation {installation_id} of host {} holds is unknown ({e}) — a token is asked for exactly as asked",
                    self.host_id
                );
                None
            }
        }
    }

    /// Mint, or serve from the cache, an installation token for `repo` alone,
    /// narrowed to `ask` and NEVER WIDER THAN IT. This token ends up inside a
    /// sandbox running model-authored code, so:
    ///   1. What the installation holds is read first
    ///      ([`Self::installation_permissions`]).
    ///   2. The ask is met with it, minus every key a repo token may never
    ///      carry ([`narrow`]). A token is asked only for what the
    ///      installation can give, and what it cannot is the shortfall.
    ///      Holdings unknown: the ask goes as it is. An empty permissions
    ///      block is never posted: an ask that comes to nothing is refused
    ///      here, before GitHub sees it.
    ///   3. The body always names the one repository and the permissions
    ///      ([`access_tokens_body`]). A refusal is never retried without them:
    ///      that token would carry everything the installation holds. On a
    ///      422 the holdings are read again, once, past the cache, and the
    ///      mint is retried only if that changes what would be asked.
    ///      Otherwise the mint fails with GitHub's own reason and the
    ///      shortfall.
    ///   4. The token GitHub returns is checked ([`check_reply`]). One that is
    ///      wider than asked, or for anything but `repo`, is revoked on GitHub
    ///      and never used.
    ///   5. The cache keys on the host, the installation, the repo and the
    ///      ask's canonical JSON, and serves a token while it has more than
    ///      five minutes left.
    async fn installation_token(
        &self,
        installation_id: &str,
        repo: &str,
        ask: &PermissionSet,
    ) -> Result<Minted, MintError> {
        let key = TokenKey {
            host: self.host_id.clone(),
            inst: installation_id.to_string(),
            repo: Some(repo.to_string()),
            perms: ask.canonical(),
        };
        if let Some(hit) = install_tokens().lock().unwrap().get(&key)
            && hit.expires_at > now_ms() + SERVE_MARGIN_MS
        {
            return Ok(Minted {
                token: hit.token.clone(),
                carried: hit.carried.clone(),
                shortfall: hit.shortfall.clone(),
                minted: false,
            });
        }

        let never: Vec<&str> = ask
            .iter()
            .map(|(k, _)| k)
            .filter(|k| never_on_repo_token(k))
            .collect();
        if !never.is_empty() {
            tracing::warn!(
                "[github] a token for {repo} was asked to carry {} — no repository token may, so it is not asked for",
                never.join(", ")
            );
        }
        // An ask that names nothing a repo token may carry is never posted:
        // its permissions block would be `{}`, and GitHub documents only what
        // a missing block means (everything the installation holds).
        let allowed = ask.without(never_on_repo_token);
        if allowed.is_empty() {
            return Err(MintError::NothingAsked {
                repo: repo.to_string(),
            });
        }
        let jwt = self.app_jwt().map_err(MintError::Other)?;
        let base = self.api();
        let held = self.known_holdings(installation_id, false).await;
        let (mut effective, mut shortfall) = narrow(ask, held.as_ref());
        // Nothing worth a token: what the installation would give is nothing
        // at all, or at most the metadata every token carries anyway.
        let beyond_metadata = |s: &PermissionSet| !s.without(|k| k == "metadata").is_empty();
        let mut refreshed = false;
        let reply: Value = loop {
            if effective.is_empty() || (beyond_metadata(&allowed) && !beyond_metadata(&effective)) {
                return Err(MintError::NothingHeld {
                    repo: repo.to_string(),
                    shortfall,
                });
            }
            let res = http()
                .post(format!(
                    "{base}/app/installations/{installation_id}/access_tokens"
                ))
                .header("accept", "application/vnd.github+json")
                .header("x-github-api-version", "2022-11-28")
                .header("user-agent", "talaria-workbench")
                .header("authorization", format!("Bearer {jwt}"))
                .header("content-type", "application/json")
                .body(access_tokens_body(repo, &effective))
                .send()
                .await
                .map_err(|e| MintError::Other(format!("github installation token: {e}")))?;
            let status = res.status().as_u16();
            if res.status().is_success() {
                break res.json().await.map_err(|e| {
                    MintError::Other(format!("github installation token decode: {e}"))
                })?;
            }
            let text = res.text().await.unwrap_or_default();
            if status == 422 && !refreshed {
                // The ask named something the installation does not hold, or
                // the repo is not in this installation. The holdings may have
                // changed since they were read: read them once more, and ask
                // again only if that changes what would be asked.
                refreshed = true;
                let held = self.known_holdings(installation_id, true).await;
                // The registry's record said the same, so it may be out of
                // date too: it is refreshed, and this mint does not wait.
                if let Some(store) = &self.holdings_store {
                    store.refused(&self.host_id, installation_id);
                }
                let (next, next_shortfall) = narrow(ask, held.as_ref());
                shortfall = next_shortfall;
                if next != effective {
                    tracing::warn!(
                        "[github] a token for {repo} on installation {installation_id} was refused (422: {}) — its permissions changed since they were read; asking again for {next}",
                        github_message(&text)
                    );
                    effective = next;
                    continue;
                }
            }
            return Err(MintError::Refused {
                repo: repo.to_string(),
                status,
                message: github_message(&text),
                shortfall,
            });
        };

        let token = reply
            .get("token")
            .and_then(Value::as_str)
            .ok_or_else(|| MintError::Other("github installation token: no token in reply".into()))?
            .to_string();
        let (carried, unasked) = match check_reply(repo, &effective, &reply) {
            Ok(checked) => checked,
            Err(why) => {
                let revoked = revoke_installation_token(&base, &token).await;
                tracing::error!(
                    "[github] ALERT: installation {installation_id} of host {} minted a token for {repo} wider than asked ({why}; asked for {effective}) — {}",
                    self.host_id,
                    if revoked {
                        "revoked"
                    } else {
                        "the revoke was not confirmed"
                    }
                );
                return Err(MintError::Overbroad {
                    repo: repo.to_string(),
                    why,
                    revoked,
                });
            }
        };
        if !unasked.is_empty() {
            tracing::warn!(
                "[github] the token minted for {repo} on installation {installation_id} of host {} carries {unasked}, which was not asked for (asked for {effective})",
                self.host_id
            );
        }
        install_tokens().lock().unwrap().insert(
            key,
            CachedToken {
                token: token.clone(),
                expires_at: expires_at_of(&reply),
                carried: carried.clone(),
                shortfall: shortfall.clone(),
            },
        );
        Ok(Minted {
            token,
            carried,
            shortfall,
            minted: true,
        })
    }

    /// An installation-wide token ([`InstallationWide`]): no body, so every
    /// repository of the installation and every permission it holds. Two
    /// callers only, both inside the api and both needing exactly that: the
    /// repository listing (the grant pool and the routing table) and repo
    /// creation on an org. `purpose` says which in the line every mint logs.
    async fn installation_token_unscoped(
        &self,
        installation_id: &str,
        purpose: &str,
    ) -> Result<InstallationWide, MintError> {
        let key = TokenKey {
            host: self.host_id.clone(),
            inst: installation_id.to_string(),
            repo: None,
            perms: String::new(),
        };
        if let Some(hit) = install_tokens().lock().unwrap().get(&key)
            && hit.expires_at > now_ms() + SERVE_MARGIN_MS
        {
            return Ok(InstallationWide(hit.token.clone()));
        }
        let jwt = self.app_jwt().map_err(MintError::Other)?;
        tracing::info!(
            "[github] minting an installation-wide token on installation {installation_id} of host {} for {purpose}",
            self.host_id
        );
        let res = http()
            .post(format!(
                "{}/app/installations/{installation_id}/access_tokens",
                self.api()
            ))
            .header("accept", "application/vnd.github+json")
            .header("x-github-api-version", "2022-11-28")
            .header("user-agent", "talaria-workbench")
            .header("authorization", format!("Bearer {jwt}"))
            .send()
            .await
            .map_err(|e| MintError::Other(format!("github installation token: {e}")))?;
        let status = res.status().as_u16();
        if !res.status().is_success() {
            let text = res.text().await.unwrap_or_default();
            return Err(MintError::Other(format!(
                "GitHub refused a token on installation {installation_id} for {purpose} ({status}: {})",
                github_message(&text)
            )));
        }
        let reply: Value = res
            .json()
            .await
            .map_err(|e| MintError::Other(format!("github installation token decode: {e}")))?;
        let token = reply
            .get("token")
            .and_then(Value::as_str)
            .ok_or_else(|| MintError::Other("github installation token: no token in reply".into()))?
            .to_string();
        install_tokens().lock().unwrap().insert(
            key,
            CachedToken {
                token: token.clone(),
                expires_at: expires_at_of(&reply),
                carried: reply
                    .get("permissions")
                    .and_then(|p| PermissionSet::from_json(p).ok())
                    .unwrap_or_default(),
                shortfall: PermissionSet::new(),
            },
        );
        Ok(InstallationWide(token))
    }

    /// The installation a token for `repo` is minted on. One installation:
    /// that one. Several: the one the routing table holds for the repo, and
    /// on a miss the one GitHub says covers it
    /// (`GET /repos/{owner}/{repo}/installation`, with the App JWT), which
    /// the table then keeps. The listing is not rebuilt for a miss: it stops
    /// at five pages per installation and skips one that fails, so a repo it
    /// does not show may still be covered. A repo GitHub says no selected
    /// installation covers is refused, never sent to the first installation:
    /// that is another org's, and a token minted there was never this
    /// repo's. When GitHub cannot be asked, the error says that, not that the
    /// App is missing.
    async fn installation_for(&self, repo: &str) -> Result<String, MintError> {
        if let [only] = self.cfg.installation_ids.as_slice() {
            return Ok(only.clone());
        }
        let routed = repo_install_cache()
            .lock()
            .unwrap()
            .get(&self.host_id)
            .and_then(|table| table.get(&repo.to_ascii_lowercase()).cloned());
        if let Some((inst, at)) = routed
            && now_ms() - at <= ROUTING_TTL_MS
        {
            return Ok(inst);
        }
        let jwt = self.app_jwt().map_err(MintError::Other)?;
        match repo_installation_with(&self.api(), &jwt, repo).await {
            Ok(Some(id)) if self.cfg.installation_ids.contains(&id) => {
                learn_route(&self.host_id, repo, &id);
                Ok(id)
            }
            Ok(unselected) => Err(MintError::Unrouted {
                repo: repo.to_string(),
                unselected,
            }),
            Err(e) => Err(MintError::Other(format!(
                "which of this host's GitHub App installations covers {repo} could not be asked ({e}), so no token was minted"
            ))),
        }
    }

    /// A usable token for `repo`. PAT passes through (a PAT cannot be
    /// narrowed or scoped; that blast radius is the org's own choice). App
    /// mode mints a token for that one repository, on the installation that
    /// holds it, narrowed to `ask` and never wider (`installation_token`).
    /// None when the connection is not configured.
    pub async fn token(&self, repo: &str, ask: &PermissionSet) -> Result<Option<String>, String> {
        if self.cfg.mode.as_deref() == Some("pat") {
            return self.pat();
        }
        if !self.app_ready() {
            return Ok(None);
        }
        let installation_id = self.installation_for(repo).await?;
        let minted = self.installation_token(&installation_id, repo, ask).await?;
        if minted.minted && !minted.shortfall.is_empty() {
            tracing::warn!(
                "[github] the token for {repo} carries {} — installation {installation_id} of host {} does not hold {}, so it was not asked for",
                minted.carried,
                self.host_id,
                minted.shortfall
            );
        }
        Ok(Some(minted.token))
    }

    async fn token_or_err(&self, repo: &str, ask: &[(&str, Level)]) -> Result<String, String> {
        self.token(repo, &PermissionSet::of(ask))
            .await?
            .ok_or_else(|| format!("GitHub is not connected on host \"{}\"", self.host_id))
    }

    /// Repos the connection can reach — the union across all selected
    /// installations (multi-org), paged, capped at 5 pages per source. The
    /// listing refills the repo→installation routing table for token
    /// minting, whose routes live for 10 minutes; a repo it does not show is
    /// routed by asking GitHub ([`Self::installation_for`]). Each
    /// installation is listed with its installation-wide token, which never
    /// leaves this call.
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
        let at = now_ms();
        let mut table: RoutingTable = HashMap::new();
        for inst in &self.cfg.installation_ids {
            // A failing installation is skipped, not fatal: the union covers
            // the rest, and one broken org must not blind the routing table.
            let token = match self
                .installation_token_unscoped(inst, "the repository listing")
                .await
            {
                Ok(token) => token,
                Err(e) => {
                    tracing::warn!(
                        "[github] installation {inst} of host {} is left out of the listing: {e}",
                        self.host_id
                    );
                    continue;
                }
            };
            for repo in self
                .paged_repos(token.bearer(), "/installation/repositories?per_page=100")
                .await
            {
                if let std::collections::hash_map::Entry::Vacant(slot) =
                    table.entry(repo.to_ascii_lowercase())
                {
                    slot.insert((inst.clone(), at));
                    out.push(repo);
                }
            }
        }
        repo_install_cache()
            .lock()
            .unwrap()
            .insert(self.host_id.clone(), table);
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
    /// hunting an id, with what each installation holds. An installation
    /// whose permissions cannot be read is still listed, as unknown. Any
    /// failure is an empty list, never a throw.
    pub async fn list_installations(&self) -> Vec<InstallationListing> {
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
                    .filter_map(InstallationListing::from_item)
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
    let id = installation_id_of(&j)
        .ok_or_else(|| format!("GitHub GET /orgs/{org}/installation: no id in reply"))?;
    Ok(if selected.iter().any(|s| s == &id) {
        OrgInstallation::Selected(id)
    } else {
        OrgInstallation::NotSelected(id)
    })
}

/// An installation's `id`, which GitHub sends as a number (a string is
/// taken too).
fn installation_id_of(installation: &Value) -> Option<String> {
    match installation.get("id") {
        Some(Value::Number(n)) => Some(n.to_string()),
        Some(Value::String(s)) => Some(s.clone()),
        _ => None,
    }
}

/// `GET /repos/{owner}/{repo}/installation` with the App JWT: the
/// installation of this App that covers `repo`, or None when GitHub says
/// none does (404). Err is GitHub or the connection failing, never an
/// answer about the repo. It answers from GitHub's own records, so a repo
/// past the listing's page cap is found as well as any other.
async fn repo_installation_with(
    api: &str,
    jwt: &str,
    repo: &str,
) -> Result<Option<String>, String> {
    let (owner, name) = repo
        .split_once('/')
        .ok_or_else(|| format!("{repo} is not an owner/name"))?;
    let path = format!("/repos/{}/{}/installation", enc(owner), enc(name));
    let res = gh(api, &path, jwt, "GET", None).await?;
    let status = res.status().as_u16();
    if status == 404 {
        return Ok(None);
    }
    if !res.status().is_success() {
        let text = res.text().await.unwrap_or_default();
        return Err(format!(
            "GitHub GET {path} → {status}: {}",
            github_message(&text)
        ));
    }
    let j: Value = res
        .json()
        .await
        .map_err(|e| format!("github decode: {e}"))?;
    installation_id_of(&j)
        .map(Some)
        .ok_or_else(|| format!("GitHub GET {path}: no id in reply"))
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
    // Installation-wide: creating the repository and writing its ruleset need
    // Administration, which no repo token may carry, on a repository that does
    // not exist yet.
    let wide = gh_conn
        .installation_token_unscoped(&installation, &format!("creating {org}/{name}"))
        .await?;
    let token = wide.bearer();
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
        token,
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
        Some(protect_default_branch_with(&api, token, &full_name).await)
    } else {
        None
    };
    // The pool changed: a token for the new repo goes to the installation
    // that created it.
    if !full_name.is_empty() {
        learn_route(&gh_conn.host_id, &full_name, &installation);
    }
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
    /// An App also says what its registration asks for (`permissions` on
    /// `GET /app`, the same read). What each installation accepted is the
    /// registry's record, not this live read (talaria-git-hosts).
    async fn status(&self) -> HostStatus {
        let configured =
            |configured: bool, account: Option<String>, error: Option<String>| HostStatus {
                configured,
                account,
                error: error.map(|e| truncate_utf16(&e, 200).to_string()),
                ..HostStatus::default()
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
                        // Unreadable is unknown (None), never an empty ask.
                        let app_permissions = match PermissionSet::from_json(
                            j.get("permissions").unwrap_or(&Value::Null),
                        ) {
                            Ok(p) => Some(p),
                            Err(e) => {
                                tracing::warn!(
                                    "[github] the App of host {} answered permissions that cannot be read: {e}",
                                    self.host_id
                                );
                                None
                            }
                        };
                        let mut st = if self.cfg.installation_ids.is_empty() {
                            configured(false, name, Some("pick at least one installation".into()))
                        } else {
                            configured(true, name, None)
                        };
                        st.app_permissions = app_permissions;
                        st
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
            .token(repo, &permissions_for(access))
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
    fn a_repo_scoped_token_asks_for_three_permissions_and_never_for_none() {
        let narrowed =
            access_tokens_body("acme/widgets", &PermissionSet::of(REPO_TOKEN_PERMISSIONS));
        assert_eq!(
            narrowed,
            r#"{"repositories":["widgets"],"permissions":{"contents":"write","metadata":"read","pull_requests":"write"}}"#
        );
        let v: Value = serde_json::from_str(&narrowed).expect("valid JSON");
        assert_eq!(v["permissions"].as_object().map(|o| o.len()), Some(3));
        assert!(v["permissions"].get("administration").is_none());
        assert!(v["permissions"].get("workflows").is_none());
        // There is no body without a permissions block: even an empty set is
        // a block, never the installation's own grants. (The mint never posts
        // an empty one: an_ask_for_nothing_is_never_posted.)
        let empty: Value =
            serde_json::from_str(&access_tokens_body("acme/widgets", &PermissionSet::new()))
                .unwrap();
        assert_eq!(empty["permissions"], json!({}));
        // A name with a quote in it cannot break the body.
        let odd = access_tokens_body("acme/wid\"gets", &PermissionSet::of(REPO_READ_PERMISSIONS));
        assert!(serde_json::from_str::<Value>(&odd).is_ok());
    }

    /// The token app install and update clone with: one repository, read
    /// access to its contents, nothing that writes.
    #[test]
    fn a_read_token_asks_for_contents_read_and_nothing_that_writes() {
        let body = access_tokens_body("acme/talaria-app-crm", &permissions_for(RepoAccess::Read));
        assert_eq!(
            body,
            r#"{"repositories":["talaria-app-crm"],"permissions":{"contents":"read","metadata":"read"}}"#
        );
        let v: Value = serde_json::from_str(&body).expect("valid JSON");
        let perms = v["permissions"].as_object().unwrap();
        assert!(perms.values().all(|p| p == "read"), "{perms:?}");
        assert_eq!(
            permissions_for(RepoAccess::Write),
            PermissionSet::of(REPO_TOKEN_PERMISSIONS)
        );
    }

    #[test]
    fn administration_is_never_grantable_and_nothing_is_that_nothing_spends() {
        for key in [
            "administration",
            "members",
            "organization_secrets",
            "organization_administration",
            "profile",
        ] {
            assert!(never_on_repo_token(key), "{key}");
            let why = grantable(key, Level::Write).expect_err(key);
            assert!(why.contains("never granted"), "{why}");
        }
        for key in [
            "contents",
            "metadata",
            "workflows",
            "secrets",
            "pull_requests",
        ] {
            assert!(!never_on_repo_token(key), "{key}");
        }
        // Nothing spends workflows yet, so nothing grants it.
        let why = grantable("workflows", Level::Write).expect_err("not yet");
        assert!(why.contains("no Talaria surface spends"), "{why}");
        for entry in GRANTABLE {
            assert!(!never_on_repo_token(entry.key), "{}", entry.key);
            assert!(!entry.levels.is_empty(), "{}", entry.key);
        }
        // None of the platform's own asks names a key no repo token may carry.
        for ask in [
            REPO_TOKEN_PERMISSIONS,
            REPO_READ_PERMISSIONS,
            PR_STATUS_PERMISSIONS,
        ] {
            assert!(ask.iter().all(|(k, _)| !never_on_repo_token(k)), "{ask:?}");
        }
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

    // ── Minting: never wider than asked ──────────────────────────────────────

    /// GitHub's 201 for a mint: the token, an expiry far out, the one
    /// repository, and the permissions the token carries.
    fn minted(token: &str, full_name: &str, permissions: Value) -> String {
        json!({
            "token": token,
            "expires_at": "2099-01-01T00:00:00Z",
            "permissions": permissions,
            "repository_selection": "selected",
            "repositories": [{ "full_name": full_name }],
        })
        .to_string()
    }

    /// GitHub's answer to `GET /app/installations/{id}`.
    fn installation(id: &str, permissions: Value) -> String {
        json!({ "id": id, "account": { "login": "acme" }, "permissions": permissions }).to_string()
    }

    fn calls<'a>(seen: &'a [Seen], method: &str, path: &str) -> Vec<&'a Seen> {
        seen.iter()
            .filter(|r| r.method == method && r.path == path)
            .collect()
    }

    fn posted_permissions(req: &Seen) -> Value {
        let body: Value = serde_json::from_str(&req.body).expect("a mint body is JSON");
        body["permissions"].clone()
    }

    #[tokio::test]
    async fn a_refused_mint_reads_the_holdings_again_once_and_never_posts_without_permissions() {
        // An org owner took pull_requests away after the holdings were read.
        // The first ask names it and is refused; the second read shows it
        // gone, and the second ask is for the rest.
        let sb = test_box();
        let counted = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (base, seen) = stub(move |req, _| match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/app/installations/91001") => {
                let held = if counted.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                    json!({ "contents": "write", "pull_requests": "write", "metadata": "read" })
                } else {
                    json!({ "contents": "write", "metadata": "read" })
                };
                (200, installation("91001", held))
            }
            ("POST", "/app/installations/91001/access_tokens") if req.body.contains("pull_requests") => (
                422,
                r#"{"message":"The permissions requested are not granted to this installation."}"#.into(),
            ),
            ("POST", "/app/installations/91001/access_tokens") => (
                201,
                minted("tok-91001", "acme/widgets", json!({ "contents": "write", "metadata": "read" })),
            ),
            _ => (500, "{}".into()),
        })
        .await;
        let conn = app_conn(&base, &["91001"], &sb);
        let token = conn
            .token("acme/widgets", &PermissionSet::of(REPO_TOKEN_PERMISSIONS))
            .await
            .expect("minted on the second ask");
        assert_eq!(token.as_deref(), Some("tok-91001"));
        let seen = seen.lock().unwrap().clone();
        assert_eq!(
            calls(&seen, "GET", "/app/installations/91001").len(),
            2,
            "read once, then once more past the cache: {seen:?}"
        );
        let mints = calls(&seen, "POST", "/app/installations/91001/access_tokens");
        assert_eq!(mints.len(), 2, "{seen:?}");
        for mint in &mints {
            let body: Value = serde_json::from_str(&mint.body).expect("every mint has a body");
            assert_eq!(body["repositories"], json!(["widgets"]));
            assert!(
                body["permissions"]
                    .as_object()
                    .is_some_and(|p| !p.is_empty()),
                "a body without permissions was posted: {}",
                mint.body
            );
        }
        assert_eq!(
            posted_permissions(mints[1]),
            json!({ "contents": "write", "metadata": "read" })
        );
    }

    #[tokio::test]
    async fn a_refusal_the_holdings_do_not_explain_fails_with_githubs_own_words() {
        let sb = test_box();
        let (base, seen) = stub(|req, _| match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/app/installations/91011") => (
                200,
                installation(
                    "91011",
                    json!({ "contents": "write", "pull_requests": "write", "metadata": "read" }),
                ),
            ),
            ("POST", "/app/installations/91011/access_tokens") => (
                422,
                r#"{"message":"There is at least one repository that does not exist or is not accessible to the parent installation."}"#.into(),
            ),
            _ => (500, "{}".into()),
        })
        .await;
        let conn = app_conn(&base, &["91011"], &sb);
        let err = conn
            .token("acme/gone", &PermissionSet::of(REPO_TOKEN_PERMISSIONS))
            .await
            .expect_err("refused");
        assert!(
            err.contains("422") && err.contains("not accessible to the parent installation"),
            "{err}"
        );
        let seen = seen.lock().unwrap().clone();
        assert_eq!(calls(&seen, "GET", "/app/installations/91011").len(), 2);
        assert_eq!(
            calls(&seen, "POST", "/app/installations/91011/access_tokens").len(),
            1,
            "the holdings did not change, so nothing was asked again: {seen:?}"
        );
    }

    #[tokio::test]
    async fn unknown_holdings_send_the_ask_unchanged_and_a_422_fails_closed() {
        let sb = test_box();
        let (base, seen) = stub(|req, _| match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/app/installations/91021") => (503, r#"{"message":"unavailable"}"#.into()),
            ("POST", "/app/installations/91021/access_tokens") => (
                422,
                r#"{"message":"The permissions requested are not granted to this installation."}"#
                    .into(),
            ),
            _ => (500, "{}".into()),
        })
        .await;
        let conn = app_conn(&base, &["91021"], &sb);
        let err = conn
            .token("acme/widgets", &PermissionSet::of(PR_STATUS_PERMISSIONS))
            .await
            .expect_err("fails closed");
        assert!(err.contains("not granted to this installation"), "{err}");
        let seen = seen.lock().unwrap().clone();
        let mints = calls(&seen, "POST", "/app/installations/91021/access_tokens");
        assert_eq!(
            mints.len(),
            1,
            "never retried on the installation's own grants: {seen:?}"
        );
        assert_eq!(
            posted_permissions(mints[0]),
            PermissionSet::of(PR_STATUS_PERMISSIONS).to_json()
        );
    }

    #[tokio::test]
    async fn an_installation_that_lacks_a_permission_is_asked_for_the_rest_and_the_shortfall_is_said()
     {
        let sb = test_box();
        let (base, seen) = stub(|req, _| match (req.method.as_str(), req.path.as_str()) {
            // It holds contents and statuses, not pull_requests or checks.
            ("GET", "/app/installations/91031") => (
                200,
                installation(
                    "91031",
                    json!({ "contents": "write", "statuses": "read", "metadata": "read" }),
                ),
            ),
            ("POST", "/app/installations/91031/access_tokens") => {
                let repo = if req.body.contains("\"widgets\"") {
                    "acme/widgets"
                } else {
                    "acme/gadgets"
                };
                (201, minted("tok-91031", repo, posted_permissions(req)))
            }
            _ => (500, "{}".into()),
        })
        .await;
        let conn = app_conn(&base, &["91031"], &sb);
        let job = conn
            .installation_token(
                "91031",
                "acme/widgets",
                &PermissionSet::of(REPO_TOKEN_PERMISSIONS),
            )
            .await
            .expect("minted");
        assert!(job.minted);
        assert_eq!(
            job.shortfall,
            PermissionSet::of(&[("pull_requests", Level::Write)])
        );
        assert_eq!(
            job.carried,
            PermissionSet::of(&[("contents", Level::Write), ("metadata", Level::Read)])
        );
        // The status token no longer widens: it is asked for the reads the
        // installation holds, and the rest is the shortfall.
        let status = conn
            .installation_token(
                "91031",
                "acme/gadgets",
                &PermissionSet::of(PR_STATUS_PERMISSIONS),
            )
            .await
            .expect("minted");
        assert_eq!(
            status.shortfall,
            PermissionSet::of(&[("checks", Level::Read), ("pull_requests", Level::Read)])
        );
        // Nothing worth a token is never asked for.
        let err = conn
            .installation_token(
                "91031",
                "acme/other",
                &PermissionSet::of(&[("checks", Level::Read), ("metadata", Level::Read)]),
            )
            .await
            .err()
            .expect("nothing to ask for");
        assert!(matches!(err, MintError::NothingHeld { .. }), "{err}");
        let seen = seen.lock().unwrap().clone();
        let mints = calls(&seen, "POST", "/app/installations/91031/access_tokens");
        assert_eq!(mints.len(), 2, "{seen:?}");
        assert_eq!(
            posted_permissions(mints[0]),
            json!({ "contents": "write", "metadata": "read" })
        );
        assert_eq!(
            posted_permissions(mints[1]),
            json!({ "metadata": "read", "statuses": "read" })
        );
        assert_eq!(
            calls(&seen, "GET", "/app/installations/91031").len(),
            1,
            "the holdings are cached"
        );
    }

    #[tokio::test]
    async fn an_ask_for_nothing_is_never_posted() {
        // The holdings of 91091 cannot be read, so nothing narrows an ask on
        // its way to GitHub. An empty ask, or one of keys no repo token may
        // carry, would go out as `{}`, and what GitHub mints for that is not
        // documented: it is refused before GitHub sees it.
        let sb = test_box();
        let (base, seen) = stub(|req, _| match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/app/installations/91091") => (503, r#"{"message":"unavailable"}"#.into()),
            ("GET", "/app/installations/91092") => (
                200,
                installation("91092", json!({ "contents": "write", "metadata": "read" })),
            ),
            ("POST", _) => (
                201,
                minted(
                    "tok-everything",
                    "acme/widgets",
                    json!({ "contents": "write", "workflows": "write", "secrets": "write", "metadata": "read" }),
                ),
            ),
            _ => (500, "{}".into()),
        })
        .await;
        let conn = app_conn(&base, &["91091"], &sb);
        for ask in [
            PermissionSet::new(),
            PermissionSet::of(&[("administration", Level::Write)]),
        ] {
            let err = conn
                .installation_token("91091", "acme/widgets", &ask)
                .await
                .err()
                .expect("nothing to ask for");
            assert!(matches!(err, MintError::NothingAsked { .. }), "{err}");
            let said = conn
                .token("acme/widgets", &ask)
                .await
                .expect_err("nothing to ask for");
            assert!(
                said.contains("asked for nothing a repository token may carry"),
                "{said}"
            );
        }
        // Holdings known: an ask the installation holds none of (and without
        // the metadata it always holds) narrows to nothing, and is refused
        // with what it lacks rather than posted as `{}`.
        let lacking = app_conn(&base, &["91092"], &sb);
        let err = lacking
            .installation_token(
                "91092",
                "acme/widgets",
                &PermissionSet::of(&[("checks", Level::Read)]),
            )
            .await
            .err()
            .expect("nothing held");
        assert_eq!(
            err,
            MintError::NothingHeld {
                repo: "acme/widgets".into(),
                shortfall: PermissionSet::of(&[("checks", Level::Read)]),
            }
        );
        let seen = seen.lock().unwrap().clone();
        assert!(
            !seen.iter().any(|r| r.method == "POST"),
            "a mint was posted: {seen:?}"
        );
    }

    #[tokio::test]
    async fn a_token_wider_than_asked_is_revoked_and_never_used() {
        let sb = test_box();
        let (base, seen) = stub(|req, _| match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/app/installations/91041") => (
                200,
                installation(
                    "91041",
                    json!({ "contents": "write", "pull_requests": "write", "administration": "write", "metadata": "read" }),
                ),
            ),
            // A key no repo token may carry.
            ("POST", "/app/installations/91041/access_tokens") if req.body.contains("\"admin-leak\"") => (
                201,
                minted(
                    "tok-admin",
                    "acme/admin-leak",
                    json!({ "contents": "write", "pull_requests": "write", "metadata": "read", "administration": "write" }),
                ),
            ),
            // A level above the ask.
            ("POST", "/app/installations/91041/access_tokens") => (
                201,
                minted("tok-write", "acme/read-leak", json!({ "contents": "write", "metadata": "read" })),
            ),
            ("DELETE", "/installation/token") => (204, String::new()),
            _ => (500, "{}".into()),
        })
        .await;
        let conn = app_conn(&base, &["91041"], &sb);
        let err = conn
            .token(
                "acme/admin-leak",
                &PermissionSet::of(REPO_TOKEN_PERMISSIONS),
            )
            .await
            .expect_err("revoked");
        assert!(
            err.contains("administration:write") && err.contains("it was revoked"),
            "{err}"
        );
        let err = conn
            .token("acme/read-leak", &PermissionSet::of(REPO_READ_PERMISSIONS))
            .await
            .expect_err("revoked");
        assert!(
            err.contains("contents:write") && err.contains("above the read"),
            "{err}"
        );
        let log = seen.lock().unwrap().clone();
        let revoked: Vec<&str> = calls(&log, "DELETE", "/installation/token")
            .iter()
            .map(|r| r.auth.as_str())
            .collect();
        assert_eq!(revoked, ["Bearer tok-admin", "Bearer tok-write"]);
        // Neither was cached: the next ask mints again, and is revoked again.
        let _ = conn
            .token("acme/read-leak", &PermissionSet::of(REPO_READ_PERMISSIONS))
            .await;
        let log = seen.lock().unwrap().clone();
        assert_eq!(
            calls(&log, "POST", "/app/installations/91041/access_tokens").len(),
            3
        );
        assert_eq!(calls(&log, "DELETE", "/installation/token").len(), 3);
    }

    #[tokio::test]
    async fn an_implied_metadata_key_is_accepted_and_an_unasked_one_is_said_not_refused() {
        let sb = test_box();
        let (base, seen) = stub(|req, _| match (req.method.as_str(), req.path.as_str()) {
            // The installation's answer leaves metadata out: every App holds
            // it anyway, so it is never a shortfall.
            ("GET", "/app/installations/91051") => (
                200,
                installation("91051", json!({ "contents": "write", "checks": "read" })),
            ),
            ("POST", "/app/installations/91051/access_tokens")
                if req.body.contains("\"plain\"") =>
            {
                (
                    201,
                    minted(
                        "tok-plain",
                        "acme/plain",
                        json!({ "contents": "read", "metadata": "read" }),
                    ),
                )
            }
            ("POST", "/app/installations/91051/access_tokens") => (
                201,
                minted(
                    "tok-extra",
                    "acme/extra",
                    json!({ "contents": "read", "metadata": "read", "checks": "read" }),
                ),
            ),
            _ => (500, "{}".into()),
        })
        .await;
        let conn = app_conn(&base, &["91051"], &sb);
        let ask = PermissionSet::of(&[("contents", Level::Read), ("metadata", Level::Read)]);
        let plain = conn
            .installation_token("91051", "acme/plain", &ask)
            .await
            .expect("metadata is implied");
        assert_eq!(plain.token, "tok-plain");
        assert!(plain.shortfall.is_empty(), "{}", plain.shortfall);
        let only_contents = PermissionSet::of(&[("contents", Level::Read)]);
        let extra = conn
            .installation_token("91051", "acme/extra", &only_contents)
            .await
            .expect("an unasked key is said, not refused");
        assert_eq!(extra.token, "tok-extra");
        assert_eq!(extra.carried.get("metadata"), Some(Level::Read));
        assert_eq!(extra.carried.get("checks"), Some(Level::Read));
        let seen = seen.lock().unwrap().clone();
        assert!(
            calls(&seen, "DELETE", "/installation/token").is_empty(),
            "nothing was revoked: {seen:?}"
        );
        let mints = calls(&seen, "POST", "/app/installations/91051/access_tokens");
        assert_eq!(
            posted_permissions(mints[0]),
            json!({ "contents": "read", "metadata": "read" })
        );
        assert_eq!(posted_permissions(mints[1]), json!({ "contents": "read" }));
    }

    #[tokio::test]
    async fn a_token_for_another_repository_is_revoked() {
        let sb = test_box();
        let (base, seen) = stub(|req, _| match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/app/installations/91061") => (
                200,
                installation(
                    "91061",
                    json!({ "contents": "write", "pull_requests": "write", "metadata": "read" }),
                ),
            ),
            ("POST", "/app/installations/91061/access_tokens") if req.body.contains("\"widgets\"") => (
                201,
                minted(
                    "tok-other",
                    "acme/other",
                    json!({ "contents": "write", "pull_requests": "write", "metadata": "read" }),
                ),
            ),
            ("POST", "/app/installations/91061/access_tokens") => (
                201,
                json!({
                    "token": "tok-all",
                    "expires_at": "2099-01-01T00:00:00Z",
                    "permissions": { "contents": "write", "metadata": "read" },
                    "repository_selection": "all",
                })
                .to_string(),
            ),
            ("DELETE", "/installation/token") => (204, String::new()),
            _ => (500, "{}".into()),
        })
        .await;
        let conn = app_conn(&base, &["91061"], &sb);
        let ask = PermissionSet::of(REPO_TOKEN_PERMISSIONS);
        let err = conn
            .token("acme/widgets", &ask)
            .await
            .expect_err("another repository");
        assert!(err.contains("[acme/other]"), "{err}");
        let err = conn
            .token("acme/gadgets", &ask)
            .await
            .expect_err("every repository");
        assert!(err.contains("selection is all"), "{err}");
        let seen = seen.lock().unwrap().clone();
        let revoked: Vec<&str> = calls(&seen, "DELETE", "/installation/token")
            .iter()
            .map(|r| r.auth.as_str())
            .collect();
        assert_eq!(revoked, ["Bearer tok-other", "Bearer tok-all"]);
    }

    #[tokio::test]
    async fn one_set_in_two_key_orders_is_one_mint() {
        let sb = test_box();
        let (base, seen) = stub(|req, _| match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/app/installations/91081") => (
                200,
                installation("91081", json!({ "contents": "write", "metadata": "read" })),
            ),
            ("POST", "/app/installations/91081/access_tokens") => (
                201,
                minted(
                    "tok-once",
                    "acme/widgets",
                    json!({ "contents": "write", "metadata": "read" }),
                ),
            ),
            _ => (500, "{}".into()),
        })
        .await;
        let conn = app_conn(&base, &["91081"], &sb);
        let spelled =
            |s: &str| PermissionSet::from_json(&serde_json::from_str::<Value>(s).unwrap()).unwrap();
        let a = conn
            .token(
                "acme/widgets",
                &spelled(r#"{"contents":"write","metadata":"read"}"#),
            )
            .await
            .unwrap();
        let b = conn
            .token(
                "acme/widgets",
                &spelled(r#"{"metadata":"read","contents":"write"}"#),
            )
            .await
            .unwrap();
        assert_eq!(
            (a.as_deref(), b.as_deref()),
            (Some("tok-once"), Some("tok-once"))
        );
        let count = |method: &str, path: &str| calls(&seen.lock().unwrap(), method, path).len();
        assert_eq!(count("POST", "/app/installations/91081/access_tokens"), 1);
        // Forgetting the host forgets its tokens and what its installation
        // holds: both are read again.
        forget_host(&conn.host_id);
        conn.token(
            "acme/widgets",
            &spelled(r#"{"metadata":"read","contents":"write"}"#),
        )
        .await
        .unwrap();
        assert_eq!(count("POST", "/app/installations/91081/access_tokens"), 2);
        assert_eq!(count("GET", "/app/installations/91081"), 2);
    }

    #[tokio::test]
    async fn a_host_with_several_installations_routes_a_repo_to_the_one_github_names() {
        let sb = test_box();
        let (base, seen) = stub(|req, _| {
            match (req.method.as_str(), req.path.as_str(), req.auth.as_str()) {
                // The listing: each installation's installation-wide token,
                // asked for with no body.
                ("POST", "/app/installations/91071/access_tokens", _) if req.body.is_empty() => (
                    201,
                    json!({ "token": "wide-91071", "expires_at": "2099-01-01T00:00:00Z" })
                        .to_string(),
                ),
                ("POST", "/app/installations/91072/access_tokens", _) if req.body.is_empty() => (
                    201,
                    json!({ "token": "wide-91072", "expires_at": "2099-01-01T00:00:00Z" })
                        .to_string(),
                ),
                ("GET", "/installation/repositories?per_page=100", "Bearer wide-91071") => {
                    (200, r#"{"repositories":[{"full_name":"one/a"}]}"#.into())
                }
                // two/far is past the listing (its page cap, say): only
                // GitHub's own answer routes it.
                ("GET", "/installation/repositories?per_page=100", "Bearer wide-91072") => {
                    (200, r#"{"repositories":[{"full_name":"two/b"}]}"#.into())
                }
                ("GET", "/repos/two/far/installation", _) => (200, r#"{"id":91072}"#.into()),
                ("GET", "/app/installations/91071" | "/app/installations/91072", _) => (
                    200,
                    installation(
                        "9107x",
                        json!({ "contents": "write", "pull_requests": "write", "metadata": "read" }),
                    ),
                ),
                ("POST", "/app/installations/91071/access_tokens", _) => (
                    201,
                    minted("scoped-91071", "one/a", posted_permissions(req)),
                ),
                ("POST", "/app/installations/91072/access_tokens", _) => (
                    201,
                    minted("scoped-91072", "two/far", posted_permissions(req)),
                ),
                _ => (500, "{}".into()),
            }
        })
        .await;
        let conn = app_conn(&base, &["91071", "91072"], &sb);
        assert_eq!(
            conn.reachable_repos().await.expect("listed"),
            ["one/a", "two/b"]
        );
        // A listed repo is routed by the listing, with no lookup.
        let one = conn
            .token("one/a", &PermissionSet::of(REPO_TOKEN_PERMISSIONS))
            .await
            .expect("routed");
        assert_eq!(one.as_deref(), Some("scoped-91071"));
        // A repo the listing does not show is routed by GitHub's answer, on
        // that installation and never the first, and the answer is kept.
        for ask in [REPO_TOKEN_PERMISSIONS, REPO_READ_PERMISSIONS] {
            let far = conn
                .token("two/far", &PermissionSet::of(ask))
                .await
                .expect("routed");
            assert_eq!(far.as_deref(), Some("scoped-91072"));
        }
        let seen = seen.lock().unwrap().clone();
        assert!(
            calls(&seen, "GET", "/repos/one/a/installation").is_empty(),
            "{seen:?}"
        );
        let lookups = calls(&seen, "GET", "/repos/two/far/installation");
        assert_eq!(lookups.len(), 1, "the route is kept: {seen:?}");
        assert!(
            lookups[0].auth.starts_with("Bearer ey"),
            "the lookup carries the App JWT"
        );
        let scoped = |inst: &str| {
            seen.iter()
                .filter(|r| {
                    r.method == "POST"
                        && r.path == format!("/app/installations/{inst}/access_tokens")
                        && !r.body.is_empty()
                })
                .count()
        };
        assert_eq!((scoped("91071"), scoped("91072")), (1, 2), "{seen:?}");
    }

    #[tokio::test]
    async fn a_host_with_several_installations_refuses_a_repo_no_selected_one_covers() {
        let sb = test_box();
        let (base, seen) = stub(|req, _| match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/repos/ghost/c/installation") => (404, r#"{"message":"Not Found"}"#.into()),
            ("GET", "/repos/side/d/installation") => (200, r#"{"id":99999}"#.into()),
            ("GET", "/repos/blip/e/installation") => (502, r#"{"message":"Server Error"}"#.into()),
            _ => (500, "{}".into()),
        })
        .await;
        let conn = app_conn(&base, &["91073", "91074"], &sb);
        let ask = PermissionSet::of(REPO_TOKEN_PERMISSIONS);
        let err = conn
            .token("ghost/c", &ask)
            .await
            .expect_err("no installation covers it");
        assert!(
            err.contains("no installation of this host's GitHub App covers ghost/c"),
            "{err}"
        );
        let err = conn
            .token("side/d", &ask)
            .await
            .expect_err("covered by an installation the admin did not select");
        assert!(
            err.contains("installation 99999") && err.contains("does not use"),
            "{err}"
        );
        // GitHub could not be asked: that is what the error says, and it
        // never tells the admin to install the App.
        let err = conn
            .token("blip/e", &ask)
            .await
            .expect_err("GitHub could not be asked");
        assert!(
            err.contains("could not be asked") && err.contains("502"),
            "{err}"
        );
        assert!(!err.contains("install the App"), "{err}");
        let seen = seen.lock().unwrap().clone();
        assert!(
            !seen.iter().any(|r| r.method == "POST"),
            "no token was asked for, on the first installation or any: {seen:?}"
        );
        assert!(
            !seen.iter().any(|r| r.path.starts_with("/installation/")),
            "a miss asks GitHub about the one repo, never relists: {seen:?}"
        );
    }

    // ── What the App asks for, and what each installation accepted ───────────

    /// GitHub's `GET /app` for an App whose registration asks for four
    /// permissions, one of which (workflows) an installation has not taken.
    const APP: &str = r#"{"name":"Talaria Workbench","permissions":{"contents":"write","pull_requests":"write","workflows":"write","metadata":"read"}}"#;

    fn org_installation(id: &str) -> Value {
        json!({
            "id": id.parse::<i64>().unwrap(),
            "account": { "login": "acme", "type": "Organization" },
            "permissions": { "contents": "write", "pull_requests": "write" },
            "repository_selection": "selected",
            "suspended_at": null,
            "html_url": format!("https://github.com/organizations/acme/settings/installations/{id}"),
        })
    }

    #[tokio::test]
    async fn the_app_and_its_installations_are_read_with_their_permissions() {
        let sb = test_box();
        let (base, seen) = stub(|req, _| match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/app") => (200, APP.into()),
            ("GET", "/app/installations") => (
                200,
                json!([
                    org_installation("92001"),
                    {
                        "id": 92002,
                        "account": { "login": "bo", "type": "User" },
                        "permissions": { "contents": "read" },
                        "repository_selection": "all",
                        "suspended_at": "2026-11-01T00:00:00Z",
                        "html_url": "https://github.com/settings/installations/92002",
                    },
                    // A level this crate does not know: the installation is
                    // still one an admin may need to pick.
                    {
                        "id": 92003,
                        "account": { "login": "cy", "type": "Organization" },
                        "permissions": { "contents": "write", "copilot": "superuser" },
                        "repository_selection": "all",
                    },
                    // No permissions block at all.
                    { "id": 92004, "account": { "login": "dee", "type": "Organization" } },
                    // Nothing for the picker to show: no account.
                    { "id": 92005 },
                ])
                .to_string(),
            ),
            ("GET", "/app/installations/92001") => (200, org_installation("92001").to_string()),
            ("POST", "/app/installations/92001/access_tokens") => (
                201,
                minted("tok-92001", "acme/widgets", posted_permissions(req)),
            ),
            _ => (500, "{}".into()),
        })
        .await;
        let conn = app_conn(&base, &["92001"], &sb);

        // The status keeps what the registration asks for.
        let st = conn.status().await;
        assert!(st.configured && st.error.is_none(), "{st:?}");
        assert_eq!(st.account.as_deref(), Some("Talaria Workbench"));
        let app = st.app_permissions.expect("the App's ask is kept");
        assert_eq!(
            app,
            PermissionSet::of(&[
                ("contents", Level::Write),
                ("metadata", Level::Read),
                ("pull_requests", Level::Write),
                ("workflows", Level::Write),
            ])
        );
        assert!(
            st.installations.is_empty(),
            "the live status never reads the installations"
        );

        // The picker keeps what each installation holds, and lists one
        // whose permissions it cannot read as unknown rather than leave it
        // out.
        let listed = conn.list_installations().await;
        assert_eq!(listed.len(), 4, "{listed:?}");
        assert_eq!(listed[0].account, "acme");
        assert_eq!(
            listed[0]
                .permissions
                .as_ref()
                .and_then(|p| p.get("metadata")),
            Some(Level::Read)
        );
        assert_eq!(
            listed[1].suspended_at.as_deref(),
            Some("2026-11-01T00:00:00Z")
        );
        let wire = listed[0].listing_wire();
        assert_eq!(wire["id"], 92001, "the picker's id stays a number");
        assert_eq!(wire["permissions"]["pull_requests"], "write");
        assert_eq!(listed[1].listing_wire()["suspended"], true);
        for (unread, account) in [(&listed[2], "cy"), (&listed[3], "dee")] {
            assert_eq!(unread.account, account);
            assert_eq!(unread.permissions, None, "{unread:?}");
            let wire = unread.listing_wire();
            assert_eq!(wire["permissions"], Value::Null, "{wire}");
            assert_eq!(wire["suspended"], false);
        }

        // One installation, with what its org accepted. Pending is the App's
        // ask minus that.
        let one = conn.installation("92001").await.expect("read");
        assert_eq!(one.account, "acme");
        assert_eq!(one.repository_selection.as_deref(), Some("selected"));
        assert_eq!(
            one.html_url.as_deref(),
            Some("https://github.com/organizations/acme/settings/installations/92001")
        );
        assert_eq!(
            one.permissions,
            PermissionSet::of(&[
                ("contents", Level::Write),
                ("metadata", Level::Read),
                ("pull_requests", Level::Write),
            ])
        );
        assert_eq!(
            app.missing_from(&one.permissions),
            PermissionSet::of(&[("workflows", Level::Write)])
        );
        assert_eq!(
            conn.app_permissions().await.expect("read"),
            app,
            "the refresh's read is the status's"
        );

        // That read is what the next mint is sized to: nothing is asked again.
        conn.token("acme/widgets", &PermissionSet::of(REPO_TOKEN_PERMISSIONS))
            .await
            .expect("minted");
        let seen = seen.lock().unwrap().clone();
        assert_eq!(
            calls(&seen, "GET", "/app/installations/92001").len(),
            1,
            "{seen:?}"
        );
    }

    /// A registry record that answers what it is set to, and counts.
    #[derive(Default)]
    struct Recorded {
        held: Mutex<Option<PermissionSet>>,
        reads: std::sync::atomic::AtomicUsize,
        refused: Mutex<Vec<String>>,
    }

    impl HoldingsStore for Recorded {
        fn held<'a>(&'a self, _host_id: &'a str, _installation_id: &'a str) -> HeldFuture<'a> {
            Box::pin(async move {
                self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                self.held.lock().unwrap().clone()
            })
        }

        fn refused(&self, host_id: &str, installation_id: &str) {
            self.refused
                .lock()
                .unwrap()
                .push(format!("{host_id}/{installation_id}"));
        }
    }

    fn recorded(held: Option<PermissionSet>) -> Arc<Recorded> {
        let store = Arc::new(Recorded::default());
        *store.held.lock().unwrap() = held;
        store
    }

    #[tokio::test]
    async fn the_registrys_record_answers_before_github_and_is_kept_a_minute() {
        let sb = test_box();
        let (base, seen) = stub(|req, _| match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/app/installations/92011" | "/app/installations/92012") => (
                200,
                installation(
                    "9201x",
                    json!({ "contents": "write", "pull_requests": "write" }),
                ),
            ),
            ("POST", p) if p.ends_with("/access_tokens") => {
                let repo = if req.body.contains("\"widgets\"") {
                    "acme/widgets"
                } else {
                    "acme/gadgets"
                };
                (201, minted("tok", repo, posted_permissions(req)))
            }
            _ => (500, "{}".into()),
        })
        .await;
        // The record says the installation took pull_requests away.
        let store = recorded(Some(PermissionSet::of(&[("contents", Level::Write)])));
        let conn = app_conn(&base, &["92011"], &sb).with_holdings(store.clone());
        let job = conn
            .installation_token(
                "92011",
                "acme/widgets",
                &PermissionSet::of(REPO_TOKEN_PERMISSIONS),
            )
            .await
            .expect("minted");
        assert_eq!(
            job.shortfall,
            PermissionSet::of(&[("pull_requests", Level::Write)])
        );
        conn.token("acme/gadgets", &PermissionSet::of(REPO_READ_PERMISSIONS))
            .await
            .expect("minted");
        assert_eq!(
            store.reads.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "the record's answer is kept"
        );
        // A record with no usable answer sends the read to GitHub.
        let silent = recorded(None);
        let other = app_conn(&base, &["92012"], &sb).with_holdings(silent.clone());
        other
            .token("acme/widgets", &PermissionSet::of(REPO_TOKEN_PERMISSIONS))
            .await
            .expect("minted");
        assert_eq!(silent.reads.load(std::sync::atomic::Ordering::SeqCst), 1);
        let seen = seen.lock().unwrap().clone();
        assert!(
            calls(&seen, "GET", "/app/installations/92011").is_empty(),
            "the record answered: {seen:?}"
        );
        assert_eq!(calls(&seen, "GET", "/app/installations/92012").len(), 1);
        let mints = calls(&seen, "POST", "/app/installations/92011/access_tokens");
        assert_eq!(
            posted_permissions(mints[0]),
            json!({ "contents": "write", "metadata": "read" }),
            "sized to the record, not to GitHub's older answer"
        );
    }

    #[tokio::test]
    async fn a_refused_mint_asks_github_and_tells_the_record_to_refresh() {
        let sb = test_box();
        let (base, seen) = stub(|req, _| match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/app/installations/92021") => (
                200,
                installation(
                    "92021",
                    json!({ "contents": "write", "pull_requests": "write" }),
                ),
            ),
            ("POST", "/app/installations/92021/access_tokens") => (
                422,
                r#"{"message":"There is at least one repository that does not exist or is not accessible to the parent installation."}"#.into(),
            ),
            _ => (500, "{}".into()),
        })
        .await;
        let store = recorded(Some(PermissionSet::of(REPO_TOKEN_PERMISSIONS)));
        let conn = app_conn(&base, &["92021"], &sb).with_holdings(store.clone());
        let err = conn
            .token("acme/gone", &PermissionSet::of(REPO_TOKEN_PERMISSIONS))
            .await
            .expect_err("refused");
        assert!(err.contains("not accessible"), "{err}");
        assert_eq!(
            *store.refused.lock().unwrap(),
            [format!("{}/92021", conn.host_id)]
        );
        let seen = seen.lock().unwrap().clone();
        assert_eq!(
            calls(&seen, "GET", "/app/installations/92021").len(),
            1,
            "the second look goes to GitHub, past the record: {seen:?}"
        );
    }

    #[tokio::test]
    async fn a_permission_the_installation_lost_revokes_the_tokens_that_carry_it() {
        let sb = test_box();
        let reads = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let (base, seen) = stub(
            move |req, _| match (req.method.as_str(), req.path.as_str()) {
                // An org owner takes pull_requests away between the first read
                // and the refresh's.
                ("GET", "/app/installations/92031") => {
                    let held = if reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst) == 0 {
                        json!({ "contents": "write", "pull_requests": "write" })
                    } else {
                        json!({ "contents": "write" })
                    };
                    (200, installation("92031", held))
                }
                ("POST", "/app/installations/92031/access_tokens") => {
                    let (token, repo) = if req.body.contains("\"widgets\"") {
                        ("tok-widgets", "acme/widgets")
                    } else {
                        ("tok-gadgets", "acme/gadgets")
                    };
                    (201, minted(token, repo, posted_permissions(req)))
                }
                ("DELETE", "/installation/token") => (204, String::new()),
                _ => (500, "{}".into()),
            },
        )
        .await;
        let conn = app_conn(&base, &["92031"], &sb);
        let job = PermissionSet::of(REPO_TOKEN_PERMISSIONS);
        let read = PermissionSet::of(REPO_READ_PERMISSIONS);
        conn.token("acme/widgets", &job).await.expect("minted");
        conn.token("acme/gadgets", &read).await.expect("minted");

        // The refresh reads the installation again and revokes what it lost.
        let now = conn.installation("92031").await.expect("read");
        let done = conn.revoke_tokens_beyond("92031", &now.permissions).await;
        assert_eq!(
            done,
            Revocation {
                revoked: 1,
                failed: 0,
                keys: PermissionSet::of(&[("pull_requests", Level::Write)]),
            }
        );
        let log = seen.lock().unwrap().clone();
        let revoked: Vec<&str> = calls(&log, "DELETE", "/installation/token")
            .iter()
            .map(|r| r.auth.as_str())
            .collect();
        assert_eq!(
            revoked,
            ["Bearer tok-widgets"],
            "a token without the lost key is kept"
        );
        // The kept token is still served; the revoked one is minted again,
        // for what the installation holds now.
        conn.token("acme/gadgets", &read).await.expect("cached");
        conn.token("acme/widgets", &job)
            .await
            .expect("minted again");
        let log = seen.lock().unwrap().clone();
        let mints = calls(&log, "POST", "/app/installations/92031/access_tokens");
        assert_eq!(mints.len(), 3, "{log:?}");
        assert_eq!(
            posted_permissions(mints[2]),
            json!({ "contents": "write", "metadata": "read" })
        );
        // Nothing left to revoke.
        assert_eq!(
            conn.revoke_tokens_beyond("92031", &now.permissions).await,
            Revocation::default()
        );
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
        let body = access_tokens_body("acme/widgets", &PermissionSet::of(PR_STATUS_PERMISSIONS));
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
