// The `git_hosts` rows: what an operator added, validated on the way in and
// redacted on the way out.
//
// A row is an id (a short slug the workbench tables reference), a kind, a
// display name, the web origin (`https://github.com`, `https://git.acme.dev`,
// `http://localhost:3000` on a development machine), an optional REST origin
// override, the orgs agents may ask for new repositories in, and `auth`:
//   github   the App / PAT config, secrets sealed (talaria-github's
//            `GithubConfig`)
//   forgejo  `{ "secret": "<workspace secret name>" }` — the token lives in
//            the workspace secret store, never here
//
// A row that anything still names (a grant, a job, a repo request, an
// install request, a plugin) cannot be removed: the foreign keys refuse, and
// the route says what still uses it.

use serde::Serialize;
use serde_json::{Value, json};
use sqlx::PgPool;

use talaria_agent_auth::epoch_ms_to_iso;
use talaria_git_host_types::{HostKind, HostStatus};
use talaria_github::{GithubConfig, GithubConfigPatch};
use talaria_secretbox::SecretBox;

/// One registered host.
#[derive(Debug, Clone)]
pub struct HostRow {
    pub id: String,
    pub kind: HostKind,
    pub name: String,
    /// The web origin, without a trailing slash.
    pub base_url: String,
    /// The REST origin, when it is not the kind's own derivation.
    pub api_url: Option<String>,
    pub auth: Value,
    /// Orgs (owners) agents may ask for new repositories under. Empty = repo
    /// creation is off on this host.
    pub repo_creation_orgs: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
}

impl HostRow {
    /// `host[:port]` of the web origin — how git names this host.
    pub fn authority(&self) -> String {
        crate::authority_of(&self.base_url).unwrap_or_default()
    }

    /// The bare hostname, without a port — what a workspace secret's
    /// Spendable-at list names.
    pub fn hostname(&self) -> String {
        url::Url::parse(&self.base_url)
            .ok()
            .and_then(|u| u.host_str().map(str::to_lowercase))
            .unwrap_or_default()
    }

    /// The workspace secret a non-GitHub row authenticates with.
    pub fn secret(&self) -> Option<String> {
        self.auth
            .get("secret")
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(String::from)
    }

    /// The GitHub connection config of a GitHub row.
    pub fn github_config(&self) -> Option<GithubConfig> {
        (self.kind == HostKind::Github).then(|| GithubConfig::from_auth(&self.auth))
    }

    /// Why this host cannot take a request for a repository under `owner`,
    /// from the allowlist alone: repo creation off here, or `owner` not on
    /// the list.
    pub fn repo_creation_allowlist_refusal(&self, owner: &str) -> Option<String> {
        if self.repo_creation_orgs.is_empty() {
            return Some(format!(
                "repo creation is off on {} — no org is approved for it (an admin lists orgs on the host in Admin → Git hosts)",
                self.name
            ));
        }
        if !self
            .repo_creation_orgs
            .iter()
            .any(|o| o.eq_ignore_ascii_case(owner))
        {
            return Some(format!(
                "org \"{owner}\" is not approved for repo creation on {} (approved: {})",
                self.name,
                self.repo_creation_orgs.join(", ")
            ));
        }
        None
    }

    /// The redacted wire an admin reads: the row, and for a GitHub row
    /// whether each secret is set — never a value.
    pub fn wire(&self) -> Value {
        let mut out = json!({
            "id": self.id,
            "kind": self.kind.as_str(),
            "kindLabel": self.kind.label(),
            "name": self.name,
            "baseUrl": self.base_url,
            "apiUrl": self.api_url,
            "repoCreationOrgs": self.repo_creation_orgs,
            "createdAt": self.created_at,
            "updatedAt": self.updated_at,
        });
        match self.kind {
            HostKind::Github => {
                out["github"] = GithubConfig::from_auth(&self.auth).redacted();
            }
            HostKind::Forgejo => {
                out["secret"] = json!(self.secret());
            }
        }
        out
    }

    /// [`HostRow::wire`] with a live status.
    pub fn wire_with(&self, status: &HostStatus) -> Value {
        let mut out = self.wire();
        out["status"] = json!(status);
        out
    }
}

type Row = (
    String,
    String,
    String,
    String,
    Option<String>,
    Value,
    Vec<String>,
    i64,
    i64,
);

const SELECT: &str = "select id, kind, name, base_url, api_url, auth, repo_creation_orgs, \
       (trunc(extract(epoch from created_at) * 1000))::bigint, \
       (trunc(extract(epoch from updated_at) * 1000))::bigint \
     from git_hosts";

fn row_of(r: Row) -> Option<HostRow> {
    Some(HostRow {
        id: r.0,
        kind: HostKind::parse(&r.1)?,
        name: r.2,
        base_url: r.3,
        api_url: r.4,
        auth: r.5,
        repo_creation_orgs: r.6,
        created_at: epoch_ms_to_iso(r.7),
        updated_at: epoch_ms_to_iso(r.8),
    })
}

/// Every registered host, oldest first.
pub async fn host_rows(pg: &PgPool) -> Result<Vec<HostRow>, String> {
    let rows: Vec<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} order by created_at, id"
    )))
    .fetch_all(pg)
    .await
    .map_err(|e| format!("git hosts read: {e}"))?;
    Ok(rows.into_iter().filter_map(row_of).collect())
}

/// [`host_rows`], for a caller that degrades on a failed read.
pub async fn list_hosts(pg: &PgPool) -> Vec<HostRow> {
    host_rows(pg).await.unwrap_or_else(|e| {
        tracing::warn!("[git-hosts] {e}");
        Vec::new()
    })
}

pub async fn get_host(pg: &PgPool, id: &str) -> Result<Option<HostRow>, String> {
    let row: Option<Row> = sqlx::query_as(sqlx::AssertSqlSafe(format!("{SELECT} where id = $1")))
        .bind(id)
        .fetch_optional(pg)
        .await
        .map_err(|e| format!("git host read: {e}"))?;
    Ok(row.and_then(row_of))
}

// ── Validation ───────────────────────────────────────────────────────────────

/// A host id: a short lowercase slug, because the workbench tables and the
/// agents' tools name the host by it.
pub fn id_ok(id: &str) -> bool {
    let b = id.as_bytes();
    !b.is_empty()
        && b.len() <= 40
        && b[0].is_ascii_alphanumeric()
        && b.iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'-')
}

fn is_loopback(host: &str) -> bool {
    matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "::1")
}

/// A web origin: `https://host[:port]` with no path, query, fragment or
/// login — git and the credential helper name a host by its origin, and a
/// login belongs in a workspace secret, never in a URL that is stored and
/// shown. Plain `http://` only to this machine's loopback (a development
/// Forgejo); a token over cleartext to anywhere else is an interception.
pub fn web_origin(raw: &str) -> Result<String, String> {
    let t = raw.trim().trim_end_matches('/');
    let u = url::Url::parse(t).map_err(|_| "the base URL must be https://host".to_string())?;
    let host = u.host_str().unwrap_or_default().to_lowercase();
    if host.is_empty() {
        return Err("the base URL must name a host".into());
    }
    match u.scheme() {
        "https" => {}
        "http" if is_loopback(&host) => {}
        "http" => {
            return Err(
                "plain http:// is accepted only to this machine's loopback — use https://".into(),
            );
        }
        _ => return Err("the base URL must be https://host".into()),
    }
    if !u.username().is_empty() || u.password().is_some() {
        return Err(
            "the base URL carries a login: remove it, and name a workspace secret instead".into(),
        );
    }
    if u.path() != "/" && !u.path().is_empty() {
        return Err(
            "the base URL is the host's origin alone (https://git.example.com), without a path"
                .into(),
        );
    }
    if u.query().is_some() || u.fragment().is_some() {
        return Err("the base URL must not carry a query or a fragment".into());
    }
    Ok(u.as_str().trim_end_matches('/').to_string())
}

/// A REST origin override: https (or loopback http), and it may carry a path
/// (`https://ghe.example.com/api/v3`).
pub fn api_origin(raw: &str) -> Result<String, String> {
    let t = raw.trim().trim_end_matches('/');
    let u = url::Url::parse(t).map_err(|_| "the API URL must be https://…".to_string())?;
    let host = u.host_str().unwrap_or_default().to_lowercase();
    let ok = u.scheme() == "https" || (u.scheme() == "http" && is_loopback(&host));
    if !ok || host.is_empty() {
        return Err(
            "the API URL must be https:// (plain http:// only to this machine's loopback)".into(),
        );
    }
    if !u.username().is_empty() || u.password().is_some() || u.query().is_some() {
        return Err("the API URL must not carry a login or a query".into());
    }
    Ok(u.as_str().trim_end_matches('/').to_string())
}

/// An org (owner) name as git hosts spell one.
pub fn owner_ok(owner: &str) -> bool {
    !owner.is_empty()
        && owner.len() <= 100
        && owner
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
}

/// A secret a Forgejo row may authenticate with: a workspace credential
/// (not a person's own), whose Spendable-at list names the host, with no use
/// limit (the connection reads it on every pass of the job sweep).
pub async fn check_secret(pg: &PgPool, name: &str, hostname: &str) -> Result<String, String> {
    let wanted = name.trim().to_lowercase();
    let doc = talaria_workspace_secrets::get_secret_doc(pg, &wanted)
        .await?
        .ok_or_else(|| {
            format!("there is no workspace secret named \"{wanted}\" — add it in Admin → Secrets")
        })?;
    if doc.owner_user_id.is_some() {
        return Err(format!(
            "\"{}\" is a person's own secret — a git host authenticates with a workspace credential",
            doc.name
        ));
    }
    if !talaria_workspace_secrets::host_allowed(hostname, &doc.allowed_hosts) {
        return Err(format!(
            "the workspace secret \"{}\" may not be spent at {hostname}: add {hostname} to its Spendable at list (Admin → Secrets)",
            doc.name
        ));
    }
    if doc.uses_remaining.is_some() {
        return Err(format!(
            "the workspace secret \"{}\" has a use limit, and a git host reads its token on every pass of the job sweep — give it none",
            doc.name
        ));
    }
    Ok(doc.name)
}

// ── Writes ───────────────────────────────────────────────────────────────────

/// What an admin sends to add a host.
pub struct NewHost<'a> {
    pub id: &'a str,
    pub kind: HostKind,
    pub name: &'a str,
    pub base_url: &'a str,
    pub api_url: Option<&'a str>,
    pub repo_creation_orgs: &'a [String],
    /// A Forgejo row's workspace secret.
    pub secret: Option<&'a str>,
    /// A GitHub row's connection.
    pub github: Option<GithubConfigPatch<'a>>,
    pub actor: &'a str,
}

/// What an admin sends to change a host. `None` keeps a field.
#[derive(Default)]
pub struct HostPatch<'a> {
    pub name: Option<&'a str>,
    pub base_url: Option<&'a str>,
    /// Some(None) clears the override.
    pub api_url: Option<Option<&'a str>>,
    pub repo_creation_orgs: Option<&'a [String]>,
    /// Some(None) clears the secret.
    pub secret: Option<Option<&'a str>>,
    pub github: Option<GithubConfigPatch<'a>>,
}

fn clean_orgs(orgs: &[String]) -> Result<Vec<String>, String> {
    let mut out: Vec<String> = Vec::new();
    for o in orgs {
        let o = o.trim();
        if o.is_empty() {
            continue;
        }
        if !owner_ok(o) {
            return Err(format!("\"{o}\" is not an org name"));
        }
        if !out.iter().any(|x| x.eq_ignore_ascii_case(o)) {
            out.push(o.to_string());
        }
    }
    Ok(out)
}

async fn authority_taken(
    pg: &PgPool,
    base_url: &str,
    except: Option<&str>,
) -> Result<Option<String>, String> {
    let want = crate::authority_of(base_url).unwrap_or_default();
    Ok(host_rows(pg)
        .await?
        .into_iter()
        .find(|r| Some(r.id.as_str()) != except && r.authority() == want)
        .map(|r| r.id))
}

/// Add a host. Err is the sentence the admin reads.
pub async fn create_host(
    pg: &PgPool,
    sb: &SecretBox,
    new: &NewHost<'_>,
) -> Result<HostRow, String> {
    if !id_ok(new.id) {
        return Err(
            "the host id is a short slug: lowercase letters, digits and dashes, at most 40".into(),
        );
    }
    let name = new.name.trim();
    if name.is_empty() || name.chars().count() > 80 {
        return Err("the host needs a name (at most 80 characters)".into());
    }
    let base_url = web_origin(new.base_url)?;
    let api_url = match new.api_url.map(str::trim).filter(|a| !a.is_empty()) {
        Some(a) => Some(api_origin(a)?),
        None => None,
    };
    if let Some(other) = authority_taken(pg, &base_url, None).await? {
        return Err(format!(
            "the git host \"{other}\" is already at {base_url} — one host per origin, or git could not tell which login to use"
        ));
    }
    let orgs = clean_orgs(new.repo_creation_orgs)?;
    let hostname = url::Url::parse(&base_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_lowercase))
        .unwrap_or_default();
    let auth = match new.kind {
        HostKind::Github => {
            let cfg = match &new.github {
                Some(patch) => GithubConfig::default().patched(sb, patch)?,
                None => GithubConfig::default(),
            };
            cfg.to_auth()
        }
        HostKind::Forgejo => match new.secret.map(str::trim).filter(|s| !s.is_empty()) {
            Some(s) => json!({ "secret": check_secret(pg, s, &hostname).await? }),
            None => json!({}),
        },
    };
    let inserted = sqlx::query(
        "insert into git_hosts (id, kind, name, base_url, api_url, auth, repo_creation_orgs, created_by) \
         values ($1, $2, $3, $4, $5, $6, $7, $8)",
    )
    .bind(new.id)
    .bind(new.kind.as_str())
    .bind(name)
    .bind(&base_url)
    .bind(&api_url)
    .bind(&auth)
    .bind(&orgs)
    .bind(new.actor)
    .execute(pg)
    .await;
    if let Err(e) = inserted {
        let msg = e.to_string();
        return Err(if msg.contains("duplicate key") {
            format!("there is already a git host \"{}\"", new.id)
        } else {
            format!("git host write: {msg}")
        });
    }
    crate::forget(new.id);
    get_host(pg, new.id)
        .await?
        .ok_or_else(|| "git host write: the row did not land".into())
}

/// Change a host. Err is the sentence the admin reads; Ok(None) is no such
/// host.
pub async fn update_host(
    pg: &PgPool,
    sb: &SecretBox,
    id: &str,
    patch: &HostPatch<'_>,
) -> Result<Option<HostRow>, String> {
    let Some(cur) = get_host(pg, id).await? else {
        return Ok(None);
    };
    let name = match patch.name.map(str::trim) {
        Some(n) if n.is_empty() || n.chars().count() > 80 => {
            return Err("the host needs a name (at most 80 characters)".into());
        }
        Some(n) => n.to_string(),
        None => cur.name.clone(),
    };
    let base_url = match patch.base_url {
        Some(b) => {
            let b = web_origin(b)?;
            if let Some(other) = authority_taken(pg, &b, Some(id)).await? {
                return Err(format!(
                    "the git host \"{other}\" is already at {b} — one host per origin"
                ));
            }
            b
        }
        None => cur.base_url.clone(),
    };
    let api_url = match patch.api_url {
        None => cur.api_url.clone(),
        Some(None) => None,
        Some(Some(a)) if a.trim().is_empty() => None,
        Some(Some(a)) => Some(api_origin(a)?),
    };
    let orgs = match patch.repo_creation_orgs {
        Some(o) => clean_orgs(o)?,
        None => cur.repo_creation_orgs.clone(),
    };
    let hostname = url::Url::parse(&base_url)
        .ok()
        .and_then(|u| u.host_str().map(str::to_lowercase))
        .unwrap_or_default();
    let auth = match cur.kind {
        HostKind::Github => match &patch.github {
            Some(p) => GithubConfig::from_auth(&cur.auth).patched(sb, p)?.to_auth(),
            None => cur.auth.clone(),
        },
        HostKind::Forgejo => match patch.secret {
            None => {
                // A moved origin must still be one the secret may be spent at.
                if patch.base_url.is_some()
                    && let Some(s) = cur.secret()
                {
                    check_secret(pg, &s, &hostname).await?;
                }
                cur.auth.clone()
            }
            Some(None) => json!({}),
            Some(Some(s)) if s.trim().is_empty() => json!({}),
            Some(Some(s)) => json!({ "secret": check_secret(pg, s, &hostname).await? }),
        },
    };
    sqlx::query(
        "update git_hosts set name = $2, base_url = $3, api_url = $4, auth = $5, \
         repo_creation_orgs = $6, updated_at = now() where id = $1",
    )
    .bind(id)
    .bind(&name)
    .bind(&base_url)
    .bind(&api_url)
    .bind(&auth)
    .bind(&orgs)
    .execute(pg)
    .await
    .map_err(|e| format!("git host write: {e}"))?;
    crate::forget(id);
    get_host(pg, id).await
}

/// What still names a host, by table — why it cannot be removed yet.
#[derive(Debug, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostUse {
    pub grants: i64,
    pub jobs: i64,
    pub repo_requests: i64,
    pub install_requests: i64,
    pub plugins: i64,
    pub flows: i64,
}

impl HostUse {
    pub fn any(&self) -> bool {
        self.grants + self.jobs + self.repo_requests + self.install_requests + self.plugins > 0
    }
}

pub async fn host_use(pg: &PgPool, id: &str) -> Result<HostUse, String> {
    let row: (i64, i64, i64, i64, i64, i64) = sqlx::query_as(
        "select (select count(*) from workbench_repos where host_id = $1), \
                (select count(*) from workbench_jobs where host_id = $1), \
                (select count(*) from workbench_repo_requests where host_id = $1), \
                (select count(*) from app_install_requests where host_id = $1), \
                (select count(*) from plugins where git_host_id = $1), \
                (select count(*) from workbench_repo_flow where host_id = $1)",
    )
    .bind(id)
    .fetch_one(pg)
    .await
    .map_err(|e| format!("git host use read: {e}"))?;
    Ok(HostUse {
        grants: row.0,
        jobs: row.1,
        repo_requests: row.2,
        install_requests: row.3,
        plugins: row.4,
        flows: row.5,
    })
}

/// Remove a host nothing names. Its per-repo PR bases go with it. Ok(false)
/// is no such host.
pub async fn delete_host(pg: &PgPool, id: &str) -> Result<bool, String> {
    let used = host_use(pg, id).await?;
    if used.any() {
        let mut what: Vec<String> = Vec::new();
        for (n, one, many) in [
            (used.grants, "repo grant", "repo grants"),
            (used.jobs, "workbench job", "workbench jobs"),
            (used.repo_requests, "repo request", "repo requests"),
            (used.install_requests, "install request", "install requests"),
            (used.plugins, "plugin", "plugins"),
        ] {
            if n > 0 {
                what.push(format!("{n} {}", if n == 1 { one } else { many }));
            }
        }
        return Err(format!(
            "the git host \"{id}\" is still in use ({}) — move or remove those first",
            what.join(", ")
        ));
    }
    let mut tx = pg
        .begin()
        .await
        .map_err(|e| format!("git host delete: {e}"))?;
    sqlx::query("delete from workbench_repo_flow where host_id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("git host delete: {e}"))?;
    let gone = sqlx::query("delete from git_hosts where id = $1")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("git host delete: {e}"))?
        .rows_affected()
        > 0;
    tx.commit()
        .await
        .map_err(|e| format!("git host delete: {e}"))?;
    crate::forget(id);
    Ok(gone)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_host_id_is_a_short_slug() {
        assert!(id_ok("github"));
        assert!(id_ok("forgejo-local"));
        assert!(id_ok("ghe2"));
        assert!(!id_ok(""));
        assert!(!id_ok("-x"));
        assert!(!id_ok("GitHub"));
        assert!(!id_ok("a b"));
        assert!(!id_ok(&"a".repeat(41)));
    }

    #[test]
    fn a_base_url_is_an_https_origin_and_nothing_more() {
        assert_eq!(
            web_origin(" https://Git.Example.com/ ").unwrap(),
            "https://git.example.com"
        );
        assert_eq!(
            web_origin("https://git.example.com:8443").unwrap(),
            "https://git.example.com:8443"
        );
        assert_eq!(
            web_origin("http://localhost:3000").unwrap(),
            "http://localhost:3000"
        );
        assert!(
            web_origin("http://git.example.com")
                .unwrap_err()
                .contains("loopback")
        );
        assert!(
            web_origin("https://git.example.com/sub")
                .unwrap_err()
                .contains("path")
        );
        assert!(
            web_origin("https://bot:tok@git.example.com")
                .unwrap_err()
                .contains("login")
        );
        assert!(web_origin("ftp://x").is_err());
        assert_eq!(
            api_origin("https://ghe.example.com/api/v3/").unwrap(),
            "https://ghe.example.com/api/v3"
        );
        assert!(api_origin("http://ghe.example.com/api/v3").is_err());
    }

    #[test]
    fn the_allowlist_refusal_names_the_host_and_the_approved_orgs() {
        let row = |orgs: &[&str]| HostRow {
            id: "forge".into(),
            kind: HostKind::Forgejo,
            name: "Forge".into(),
            base_url: "https://git.example.com".into(),
            api_url: None,
            auth: json!({ "secret": "forge-bot" }),
            repo_creation_orgs: orgs.iter().map(|s| s.to_string()).collect(),
            created_at: String::new(),
            updated_at: String::new(),
        };
        assert!(
            row(&[])
                .repo_creation_allowlist_refusal("acme")
                .unwrap()
                .contains("off on Forge")
        );
        let other = row(&["acme"])
            .repo_creation_allowlist_refusal("other")
            .unwrap();
        assert!(other.contains("approved: acme"), "{other}");
        assert_eq!(row(&["acme"]).repo_creation_allowlist_refusal("ACME"), None);
        let w = row(&["acme"]).wire();
        assert_eq!(w["secret"], "forge-bot");
        assert_eq!(w["kindLabel"], "Forgejo / Gitea");
        assert_eq!(row(&[]).authority(), "git.example.com");
    }

    #[test]
    fn a_github_rows_wire_says_which_secrets_are_set_and_never_their_values() {
        let row = HostRow {
            id: "github".into(),
            kind: HostKind::Github,
            name: "GitHub".into(),
            base_url: "https://github.com".into(),
            api_url: None,
            auth: json!({ "mode": "pat", "pat": { "tokenEnc": "v1:sealed-bytes" } }),
            repo_creation_orgs: vec![],
            created_at: String::new(),
            updated_at: String::new(),
        };
        let w = row.wire();
        assert_eq!(w["github"]["patSet"], true);
        assert!(!w.to_string().contains("sealed-bytes"));
    }

    #[test]
    fn org_lists_are_cleaned_and_deduplicated() {
        let orgs = vec![
            " acme ".to_string(),
            "ACME".into(),
            "".into(),
            "beta".into(),
        ];
        assert_eq!(clean_orgs(&orgs).unwrap(), vec!["acme", "beta"]);
        assert!(clean_orgs(&["a/b".to_string()]).is_err());
    }
}
