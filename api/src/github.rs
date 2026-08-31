// The Workbench's GitHub connection — org-level, either flavor:
//   app  a GitHub App (preferred): short-lived installation tokens, per-repo
//        install scope, instant revocation from GitHub's side
//   pat  a personal access token (quickest start; fine-grained PAT advised)
// Config lives in app_settings with secrets SEALED (secretbox), same contract
// as email. Repo access per agent is a separate explicit grant table —
// connecting GitHub grants nothing to anyone by itself.
//
// Port of ui/src/server/github.ts whole: the token half the secrets route
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
use rsa::pkcs1v15::SigningKey;
use rsa::pkcs8::DecodePrivateKey;
use rsa::sha2::Sha256;
use rsa::signature::{SignatureEncoding, Signer};
use sqlx::PgPool;

use crate::body::truncate_utf16;
use crate::gateway::provider::http;
use crate::gateway::settings::{get_setting, set_setting};
use crate::secretbox::SecretBox;

const KEY: &str = "github_config";
const GH: &str = "https://api.github.com";

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// github.ts GithubConfig — the whole shape now that the admin half crossed.
/// The defaults merge is TS's `{...DEFAULTS, ...raw, app: {...DEFAULTS.app,
/// ...(raw.app ?? {})}}`: per-key, so extraction here is the same merge.
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

/// setGithubConfig — the patch merge, the seals, the setting write, and the
/// cache invalidation. A secret that arrives is sealed (an EMPTY string
/// clears, exactly as TS's truthiness check); one that doesn't arrive keeps
/// its sealed predecessor. `installationIds`/`appId`/`repoCreationOrgs` use
/// TS's `??` — undefined keeps, any value (empty included) replaces.
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
/// orgs` are plain Option (TS's `??` has no null-vs-undefined distinction
/// for them — a null appId would be a type error in the zod schema).
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

/// `gh()` — one request builder for every GitHub call: the three fixed
/// headers plus the bearer, any method, and a JSON body when there is one.
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
            format!("{GH}{path}"),
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

/// ghJson — `gh()` plus the ok-check and the error the workbench surfaces:
/// `GitHub <METHOD> <path> → <status>: <first 200 chars>`.
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

/// RS256 app JWT — GitHub Apps authenticate to /app endpoints with this.
/// Header/payload are the TS literals byte-for-byte (`JSON.stringify` key
/// order); `iat` is backdated 60s and `exp` is +9m, exactly as github.ts.
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
    let key = rsa::RsaPrivateKey::from_pkcs8_pem(private_key_pem)
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

/// Mint an installation access token. When `repo` is set the token is scoped
/// to THAT repository alone — without a body GitHub issues one valid for
/// EVERY repo in the installation, and this token ends up inside a sandbox
/// running model-authored code, so an unscoped one made a single leaked
/// credential equivalent to push access across the whole org. Permissions are
/// deliberately NOT narrowed (a permission the installation lacks is a hard
/// 422); the token inherits the installation's grants.
pub async fn installation_token(
    sb: &SecretBox,
    cfg: &GithubConfig,
    installation_id: &str,
    repo: Option<&str>,
) -> Result<String, String> {
    let key = match repo {
        Some(r) => format!("{installation_id}#{r}"),
        None => installation_id.to_string(),
    };
    if let Some((token, expires_at)) = install_tokens().lock().unwrap().get(&key)
        && *expires_at > now_ms() + 60_000
    {
        return Ok(token.clone());
    }

    let private_key_enc = cfg.private_key_enc.as_deref().unwrap_or_default();
    let jwt = {
        let key = sb
            .open(private_key_enc)
            .map_err(|e| format!("github app key unseal: {e}"))?;
        app_jwt(&cfg.app_id, &key)?
    };

    let mut req = http()
        .post(format!(
            "{GH}/app/installations/{installation_id}/access_tokens"
        ))
        .header("accept", "application/vnd.github+json")
        .header("x-github-api-version", "2022-11-28")
        .header("user-agent", "talaria-workbench")
        .header("authorization", format!("Bearer {jwt}"));
    // `repositories` takes bare names, not owner/name.
    if let Some(repo) = repo {
        let name = repo.rsplit('/').next().unwrap_or(repo);
        req = req
            .header("content-type", "application/json")
            .body(format!(r#"{{"repositories":["{name}"]}}"#));
    }
    let res = req
        .send()
        .await
        .map_err(|e| format!("github installation token: {e}"))?;
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
        .and_then(crate::tz::parse_rfc3339_ms)
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
/// the cache lives for 10 minutes, exactly as github.ts's does.
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

/// Follow GitHub's `link` rel=next headers, at most 5 pages.
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

/// `<https://api.github.com/...>; rel="next"` → the path. The regex in TS
/// anchors on the api.github.com origin; the path is what `gh()` wants.
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
        installation_token(sb, &cfg, &installation_id, repo).await?,
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
/// accident. `keep` is TS's `repos.slice(0, 100)`.
pub async fn set_granted_repos(pg: &PgPool, agent_id: &str, keep: &[String]) -> Result<(), String> {
    let keep = &keep[..keep.len().min(100)];
    let mut tx = pg
        .begin()
        .await
        .map_err(|e| format!("workbench grant tx: {e}"))?;
    sqlx::query("delete from workbench_repos where agent_id = $1::uuid")
        .bind(agent_id)
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

// ── The admin/status plane (workbench.github, the doctor verb) ────────────────

/// github.ts GithubStatus — redacted, live-verified, never leaking secrets.
/// Field order is the wire's: base first (mode, app, patSet,
/// repoCreationOrgs), then the spread's appended configured/account/error.
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
    // calm null error, exactly TS's fall-through.
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

// ── Per-repo git flow (PR target + optional testing branch) ──────────────────

/// workbench_repo_flow's row — `{repo, baseBranch, testingBranch}` on the
/// wire, null branches meaning "not set".
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RepoFlow {
    pub repo: String,
    pub base_branch: Option<String>,
    pub testing_branch: Option<String>,
}

type FlowRow = (String, Option<String>, Option<String>);

pub async fn repo_flow(pg: &PgPool, repo: &str) -> Result<RepoFlow, String> {
    let row: Option<FlowRow> = sqlx::query_as(
        "select repo, base_branch, testing_branch from workbench_repo_flow where repo = $1",
    )
    .bind(repo)
    .fetch_optional(pg)
    .await
    .map_err(|e| format!("repo flow read: {e}"))?;
    Ok(row.unwrap_or((repo.to_string(), None, None)).into())
}

impl From<FlowRow> for RepoFlow {
    fn from(r: FlowRow) -> Self {
        RepoFlow {
            repo: r.0,
            base_branch: r.1,
            testing_branch: r.2,
        }
    }
}

pub async fn list_repo_flows(pg: &PgPool) -> Result<Vec<RepoFlow>, String> {
    let rows: Vec<FlowRow> = sqlx::query_as(
        "select repo, base_branch, testing_branch from workbench_repo_flow order by repo",
    )
    .fetch_all(pg)
    .await
    .map_err(|e| format!("repo flows read: {e}"))?;
    Ok(rows.into_iter().map(Into::into).collect())
}

/// Upsert one repo's flow. Each side is `PatchField`-shaped semantics via
/// Option<Option>: None keeps the current value, Some(None) clears it.
pub async fn set_repo_flow(
    pg: &PgPool,
    repo: &str,
    base_branch: Option<Option<String>>,
    testing_branch: Option<Option<String>>,
) -> Result<(), String> {
    let cur = repo_flow(pg, repo).await?;
    let base = base_branch.unwrap_or(cur.base_branch);
    let testing = testing_branch.unwrap_or(cur.testing_branch);
    sqlx::query(
        "insert into workbench_repo_flow (repo, base_branch, testing_branch) \
         values ($1, $2, $3) \
         on conflict (repo) do update set \
           base_branch = excluded.base_branch, testing_branch = excluded.testing_branch, \
           updated_at = now()",
    )
    .bind(repo)
    .bind(base)
    .bind(testing)
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

/// Merge head into base via GitHub's merge API (e.g. feature → testing).
/// Ensures the target exists (created from the effective base if missing).
/// The 201/204/409 ladder is the whole contract: 204 is SUCCESS ("already up
/// to date"), 409 is the one conflict message, anything else failed.
pub async fn merge_into(
    pg: &PgPool,
    sb: &SecretBox,
    repo: &str,
    target_branch: &str,
    head: &str,
) -> Result<MergeOutcome, String> {
    let token = github_token(pg, sb, Some(repo))
        .await?
        .ok_or("GitHub is not connected")?;
    let existing = gh(
        &format!("/repos/{repo}/git/ref/heads/{}", enc(target_branch)),
        &token,
        "GET",
        None,
    )
    .await?;
    if !existing.status().is_success() {
        let base = effective_base(pg, sb, repo).await?;
        let head_ref = gh_json(
            &format!("/repos/{repo}/git/ref/heads/{}", enc(&base)),
            &token,
            "GET",
            None,
        )
        .await?;
        let sha = head_ref
            .get("object")
            .and_then(|o| o.get("sha"))
            .and_then(|v| v.as_str())
            .ok_or("github ref read: no sha in reply")?;
        gh_json(
            &format!("/repos/{repo}/git/refs"),
            &token,
            "POST",
            Some(&format!(
                r#"{{"ref":"refs/heads/{target_branch}","sha":"{sha}"}}"#
            )),
        )
        .await?;
    }
    let res = gh(
        &format!("/repos/{repo}/merges"),
        &token,
        "POST",
        Some(&format!(
            r#"{{"base":"{target_branch}","head":"{head}","commit_message":"Merge {head} into {target_branch} (Talaria workbench, for testing)"}}"#
        )),
    )
    .await?;
    match res.status().as_u16() {
        201 => Ok(MergeOutcome {
            merged: true,
            reason: None,
        }),
        204 => Ok(MergeOutcome {
            merged: true,
            reason: Some("already up to date".into()),
        }),
        409 => Ok(MergeOutcome {
            merged: false,
            reason: Some("merge conflict. Resolve on the branch first".into()),
        }),
        status => Ok(MergeOutcome {
            merged: false,
            reason: Some(format!("GitHub merge failed ({status})")),
        }),
    }
}

pub struct MergeOutcome {
    pub merged: bool,
    pub reason: Option<String>,
}

/// Create a repo in an org (App must hold org Administration permission).
/// Token routes to the installation on that org when multi-install.
pub async fn create_repo(
    pg: &PgPool,
    sb: &SecretBox,
    org: &str,
    name: &str,
    description: &str,
) -> Result<CreatedRepo, String> {
    let cfg = get_github_config(pg).await;
    if cfg.mode.as_deref() != Some("app") {
        return Err("repo creation requires the GitHub App connection".into());
    }
    // Route by any repo we know in that org, else first installation.
    let cache = repo_install_cache().lock().unwrap().clone();
    let in_org = cache.as_ref().and_then(|c| {
        c.map
            .iter()
            .find(|(repo, _)| repo.starts_with(&format!("{org}/")))
            .map(|(_, inst)| inst.clone())
    });
    let token = match in_org {
        Some(inst) => installation_token(sb, &cfg, &inst, None).await?,
        None => github_token(pg, sb, None)
            .await?
            .ok_or("GitHub is not connected")?,
    };
    let res = gh(
        &format!("/orgs/{org}/repos"),
        &token,
        "POST",
        Some(&format!(
            r#"{{"name":"{}","description":"{}","private":true,"auto_init":true}}"#,
            name,
            json_escape(truncate_utf16(description, 300))
        )),
    )
    .await?;
    if res.status().as_u16() == 403 {
        return Err(
            "the App lacks the org's Administration permission. Grant it in the App settings, then re-approve."
                .into(),
        );
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
    // Pool changed.
    *repo_install_cache().lock().unwrap() = None;
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

pub struct CreatedRepo {
    pub full_name: String,
    pub url: String,
}

/// The minimal JSON string escape — bodies built by format! still need their
/// interpolated strings quote-safe.
fn json_escape(s: &str) -> String {
    serde_json::to_string(s).unwrap_or_else(|_| format!("\"{s}\""))
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

pub struct CreatedPullRequest {
    pub url: String,
    pub number: i64,
}

#[allow(clippy::too_many_arguments)] // the request's inputs are GitHub's own — all eight name one
pub async fn create_pull_request(
    pg: &PgPool,
    sb: &SecretBox,
    repo: &str,
    head: &str,
    base: &str,
    title: &str,
    body: &str,
    draft: bool,
) -> Result<CreatedPullRequest, String> {
    let token = github_token(pg, sb, Some(repo))
        .await?
        .ok_or("GitHub is not connected")?;
    let pr = gh_json(
        &format!("/repos/{repo}/pulls"),
        &token,
        "POST",
        Some(&format!(
            r#"{{"head":{},"base":{},"title":{},"body":{},"draft":{draft}}}"#,
            json_escape(head),
            json_escape(base),
            json_escape(title),
            json_escape(body)
        )),
    )
    .await?;
    Ok(CreatedPullRequest {
        url: pr
            .get("html_url")
            .and_then(|v| v.as_str())
            .unwrap_or_default()
            .to_string(),
        number: pr.get("number").and_then(|v| v.as_i64()).unwrap_or(0),
    })
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
    use super::*;

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
