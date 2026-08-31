// The Workbench's GitHub connection — the TOKEN half. Port of the parts of
// ui/src/server/github.ts that `/api/secrets/git-credential` rides: the
// org-level connection config (app or PAT, secrets sealed), the RS256 app JWT
// GitHub Apps authenticate with, short-lived installation tokens (per-repo
// scoped, process-cached), the repo→installation routing table, and the
// per-agent repo grant check that scopes the sandbox's credential.
//
// THE ADMIN HALF (status probes, installation listing, config writes, the
// repo flow and merge APIs) stays TS with the workbench family; it crosses
// when those routes do. Both halves read the same `github_config` setting,
// and neither writes it here.

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

use crate::gateway::provider::http;
use crate::gateway::settings::get_setting;
use crate::secretbox::SecretBox;

const KEY: &str = "github_config";
const GH: &str = "https://api.github.com";

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// github.ts GithubConfig — the fields the token half reads. The defaults
/// merge is TS's `{...DEFAULTS, ...raw, app: {...DEFAULTS.app, ...(raw.app ??
/// {})}}`: per-key, so extraction here is the same merge.
#[derive(Debug, Clone, Default)]
pub struct GithubConfig {
    /// 'app' | 'pat' | null (unconfigured).
    pub mode: Option<String>,
    pub pat_token_enc: Option<String>,
    pub app_id: String,
    pub installation_ids: Vec<String>,
    pub private_key_enc: Option<String>,
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
    }
}

async fn gh(path: &str, token: &str) -> Result<reqwest::Response, String> {
    http()
        .get(format!("{GH}{path}"))
        .header("accept", "application/vnd.github+json")
        .header("x-github-api-version", "2022-11-28")
        .header("user-agent", "talaria-workbench")
        .header("authorization", format!("Bearer {token}"))
        .send()
        .await
        .map_err(|e| format!("github request: {e}"))
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
        let Ok(res) = gh(&path, token).await else {
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
