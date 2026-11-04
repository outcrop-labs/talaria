// The git-host registry — the git providers an operator chose, and the one
// door every caller goes through to reach them.
//
// OPERATORS CHOOSE THEIR PROVIDERS. A host is a `git_hosts` row the operator
// adds in Admin → Git hosts: a kind (GitHub — github.com or an Enterprise
// Server — or Forgejo / Gitea), a name, its web origin, and how it
// authenticates. A GitHub row holds its App or PAT config with the secrets
// sealed, as the GitHub connection always did; every other kind names a
// WORKSPACE SECRET (envelope-encrypted, docs/ENCRYPTION.md) that holds its
// token. Nothing is read from a file or the environment.
//
// The installs that predate the registry keep working with no operator
// action: the migration moved the one GitHub connection into a `github` row,
// and every workbench row that names a repo carries the host it is on
// (`host_id`, backfilled to that row).
//
//   registry   the rows: list, add, edit, remove (refused while anything
//              names the host), the redacted wire an admin reads
//   connect    a row → a live `Host` (the adapter for its kind), with a
//              Forgejo row's token opened from its workspace secret and kept
//              for five minutes so a sweep is not an audit row per call
//   grants     which agent may work which repo on which host, each grant's
//              branch law, and each repo's PR base (`workbench_repos`,
//              `workbench_repo_flow`)
//   credential the sandbox's git asks for a login by host and path: the host
//              whose origin it names, a repo on the agent's own grant list,
//              the adapter's repo-scoped login
//
// The interface every adapter implements, and every type it answers in, is
// talaria-git-host-types'. `Host` is the dispatch over the registered kinds.

pub mod grants;
pub mod registry;

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde_json::Value;
use sqlx::PgPool;

use talaria_agent_auth::now_ms;
use talaria_forgejo::Forgejo;
use talaria_github::Github;
use talaria_secretbox::SecretBox;

pub use grants::{
    PROTECTED_BRANCHES, RepoFlow, RepoRef, RepoRule, effective_base, granted_repos,
    list_repo_flows, push_allowed, repo_flow, repo_rule, repo_rules, resolve_grant,
    set_granted_repos, set_repo_flow, set_repo_rule,
};
pub use registry::{HostRow, get_host, host_rows, list_hosts};
pub use talaria_git_host_types::{
    CreatedRepo, GitHost, HostKind, HostStatus, Login, OpenPullRequest, OpenedPullRequest,
    PullRequestAsk, PullRequestRef, RepoAccess, RepoVisibility, pr_number_from_url,
};

/// A live connection to one registered host, of whichever kind it is.
#[derive(Clone, Debug)]
pub enum Host {
    Github(Github),
    Forgejo(Forgejo),
}

impl Host {
    /// The registry row's id.
    pub fn id(&self) -> &str {
        match self {
            Host::Github(g) => &g.host_id,
            Host::Forgejo(f) => &f.host_id,
        }
    }

    /// The GitHub connection, for the GitHub-only admin plane (installation
    /// listing). None for every other kind.
    pub fn github(&self) -> Option<&Github> {
        match self {
            Host::Github(g) => Some(g),
            Host::Forgejo(_) => None,
        }
    }
}

impl GitHost for Host {
    fn kind(&self) -> HostKind {
        match self {
            Host::Github(h) => h.kind(),
            Host::Forgejo(h) => h.kind(),
        }
    }

    fn clone_url(&self, repo: &str) -> String {
        match self {
            Host::Github(h) => h.clone_url(repo),
            Host::Forgejo(h) => h.clone_url(repo),
        }
    }

    fn repo_url(&self, repo: &str) -> String {
        match self {
            Host::Github(h) => h.repo_url(repo),
            Host::Forgejo(h) => h.repo_url(repo),
        }
    }

    async fn status(&self) -> HostStatus {
        match self {
            Host::Github(h) => h.status().await,
            Host::Forgejo(h) => h.status().await,
        }
    }

    async fn reachable_repos(&self) -> Result<Vec<String>, String> {
        match self {
            Host::Github(h) => h.reachable_repos().await,
            Host::Forgejo(h) => h.reachable_repos().await,
        }
    }

    async fn repo_login(&self, repo: &str, access: RepoAccess) -> Result<Option<Login>, String> {
        match self {
            Host::Github(h) => h.repo_login(repo, access).await,
            Host::Forgejo(h) => h.repo_login(repo, access).await,
        }
    }

    async fn default_branch(&self, repo: &str) -> Result<String, String> {
        match self {
            Host::Github(h) => h.default_branch(repo).await,
            Host::Forgejo(h) => h.default_branch(repo).await,
        }
    }

    async fn list_branches(&self, repo: &str) -> Result<Vec<String>, String> {
        match self {
            Host::Github(h) => h.list_branches(repo).await,
            Host::Forgejo(h) => h.list_branches(repo).await,
        }
    }

    async fn create_branch(&self, repo: &str, branch: &str, base: &str) -> Result<bool, String> {
        match self {
            Host::Github(h) => h.create_branch(repo, branch, base).await,
            Host::Forgejo(h) => h.create_branch(repo, branch, base).await,
        }
    }

    async fn branch_ahead(&self, repo: &str, base: &str, branch: &str) -> Result<i64, String> {
        match self {
            Host::Github(h) => h.branch_ahead(repo, base, branch).await,
            Host::Forgejo(h) => h.branch_ahead(repo, base, branch).await,
        }
    }

    fn repo_creation_blocker(&self) -> Option<String> {
        match self {
            Host::Github(h) => h.repo_creation_blocker(),
            Host::Forgejo(h) => h.repo_creation_blocker(),
        }
    }

    async fn repo_creation_refusal(&self, owner: &str) -> Result<Option<String>, String> {
        match self {
            Host::Github(h) => h.repo_creation_refusal(owner).await,
            Host::Forgejo(h) => h.repo_creation_refusal(owner).await,
        }
    }

    async fn create_repo(
        &self,
        owner: &str,
        name: &str,
        description: &str,
        visibility: RepoVisibility,
        protect: bool,
    ) -> Result<CreatedRepo, String> {
        match self {
            Host::Github(h) => {
                h.create_repo(owner, name, description, visibility, protect)
                    .await
            }
            Host::Forgejo(h) => {
                h.create_repo(owner, name, description, visibility, protect)
                    .await
            }
        }
    }

    async fn find_open_pull_request(
        &self,
        repo: &str,
        branch: &str,
    ) -> Result<Option<PullRequestRef>, String> {
        match self {
            Host::Github(h) => h.find_open_pull_request(repo, branch).await,
            Host::Forgejo(h) => h.find_open_pull_request(repo, branch).await,
        }
    }

    async fn open_or_update_pull_request(
        &self,
        repo: &str,
        ask: &PullRequestAsk<'_>,
    ) -> Result<OpenedPullRequest, String> {
        match self {
            Host::Github(h) => h.open_or_update_pull_request(repo, ask).await,
            Host::Forgejo(h) => h.open_or_update_pull_request(repo, ask).await,
        }
    }

    async fn open_pull_requests(
        &self,
        repo: &str,
        limit: u32,
    ) -> Result<Vec<OpenPullRequest>, String> {
        match self {
            Host::Github(h) => h.open_pull_requests(repo, limit).await,
            Host::Forgejo(h) => h.open_pull_requests(repo, limit).await,
        }
    }

    async fn pull_request_status(&self, repo: &str, number: i64) -> Result<Value, String> {
        match self {
            Host::Github(h) => h.pull_request_status(repo, number).await,
            Host::Forgejo(h) => h.pull_request_status(repo, number).await,
        }
    }
}

// ── Connecting ───────────────────────────────────────────────────────────────

/// How long an opened host token is kept in process memory before the
/// workspace secret is read (and audited) again.
const SECRET_TTL_MS: i64 = 5 * 60_000;

type LoginCache = Mutex<HashMap<String, (Result<Option<Login>, String>, i64)>>;

fn logins() -> &'static LoginCache {
    static CACHE: OnceLock<LoginCache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Forget everything cached for host `id`: its row changed, and nothing
/// opened or minted under the old credentials may outlive them.
pub fn forget(id: &str) {
    logins().lock().unwrap().remove(id);
    talaria_github::forget_host(id);
}

/// The login a non-GitHub row's workspace secret holds, opened through the
/// workspace-secret rules (the secret's Spendable-at list must name the
/// host, it must not have expired; the read is audited as `git-host`).
async fn secret_login(pg: &PgPool, sb: &SecretBox, row: &HostRow) -> Result<Option<Login>, String> {
    let Some(name) = row.secret() else {
        return Ok(None);
    };
    if let Some((cached, at)) = logins().lock().unwrap().get(&row.id)
        && now_ms() - at < SECRET_TTL_MS
    {
        return cached.clone();
    }
    let opened = talaria_workspace_secrets::credential_from_secret_via(
        pg,
        sb,
        &name,
        &row.hostname(),
        &format!("git-host:{}", row.id),
        "git-host",
    )
    .await
    .map(|c| {
        Some(Login {
            username: c.username,
            password: c.password,
        })
    });
    logins()
        .lock()
        .unwrap()
        .insert(row.id.clone(), (opened.clone(), now_ms()));
    opened
}

/// A live connection for `row`.
pub async fn connect(pg: &PgPool, sb: &SecretBox, row: &HostRow) -> Host {
    match row.kind {
        HostKind::Github => Host::Github(Github::new(
            &row.id,
            &row.base_url,
            row.api_url.as_deref(),
            &row.auth,
            sb.clone(),
        )),
        HostKind::Forgejo => Host::Forgejo(Forgejo::new(
            &row.id,
            &row.base_url,
            row.api_url.as_deref(),
            secret_login(pg, sb, row).await,
        )),
    }
}

/// The live connection for host `id`. Err names a host that is not
/// registered (it was removed, or never added).
pub async fn host(pg: &PgPool, sb: &SecretBox, id: &str) -> Result<Host, String> {
    match get_host(pg, id).await? {
        Some(row) => Ok(connect(pg, sb, &row).await),
        None => Err(format!(
            "there is no git host \"{id}\" — an admin adds it in Admin → Git hosts"
        )),
    }
}

/// Every registered host with the repositories its connection reaches now
/// — the pool an admin grants agents from and sets PR bases on. A host that
/// cannot list carries its error instead of an empty list, so a page can say
/// which host is down rather than show it as empty.
pub async fn reachable_pool(
    pg: &PgPool,
    sb: &SecretBox,
) -> Vec<(HostRow, Result<Vec<String>, String>)> {
    let mut out = Vec::new();
    for row in list_hosts(pg).await {
        let host = connect(pg, sb, &row).await;
        let repos = host.reachable_repos().await;
        out.push((row, repos));
    }
    out
}

/// `host[:port]` of a URL, lowercased, the port dropped when it is the
/// scheme's default — the form git names a host in its credential protocol.
pub fn authority_of(url: &str) -> Option<String> {
    let u = url::Url::parse(url.trim()).ok()?;
    let host = u.host_str()?.to_lowercase();
    Some(match u.port() {
        Some(p) => format!("{host}:{p}"),
        None => host,
    })
}

/// The registered host whose web origin is at `authority` (`host[:port]`).
pub async fn host_row_at(pg: &PgPool, authority: &str) -> Result<Option<HostRow>, String> {
    let want = authority.trim().to_lowercase();
    Ok(host_rows(pg)
        .await?
        .into_iter()
        .find(|r| r.authority() == want))
}

/// The registered host a repository URL lives on, by its origin.
pub async fn host_row_for_url(pg: &PgPool, url: &str) -> Result<Option<HostRow>, String> {
    match authority_of(url) {
        Some(a) => host_row_at(pg, &a).await,
        None => Ok(None),
    }
}

// ── The sandbox's git credential ─────────────────────────────────────────────

/// A login handed to an agent's git, and what it was for.
pub struct AgentCredential {
    pub login: Login,
    pub host_id: String,
    pub repo: String,
}

/// GIT'S CREDENTIAL, for a repo this agent was actually granted — the
/// second source behind `/api/secrets/git-credential`, after the workspace
/// store: the registered host's own login for that one repository, which is
/// what a workbench job needs in order to push.
///
/// SCOPED BY THE PATH GIT ASKS ABOUT. Git sends `path` when
/// `credential.useHttpPath` is set — the rendered gitconfig sets it — so the
/// request names the repository, and an agent gets a login only for a repo
/// on its own grant list, on the host git named. Without that, this would
/// hand every agent a credential for every repo the host can reach.
pub async fn agent_git_credential(
    pg: &PgPool,
    sb: &SecretBox,
    agent_id: &str,
    authority: &str,
    path: Option<&str>,
) -> Result<Option<AgentCredential>, String> {
    let Some(row) = host_row_at(pg, authority).await? else {
        return Ok(None);
    };
    let Some(repo) = repo_of_path(path.unwrap_or_default()) else {
        return Ok(None);
    };
    let granted = granted_repos(pg, agent_id)
        .await
        .into_iter()
        .any(|g| g.host_id == row.id && g.repo == repo);
    if !granted {
        return Ok(None);
    }
    let host = connect(pg, sb, &row).await;
    Ok(host
        .repo_login(&repo, RepoAccess::Write)
        .await?
        .map(|login| AgentCredential {
            login,
            host_id: row.id.clone(),
            repo,
        }))
}

/// `owner/name` from the path git sends (`owner/name.git`, with or without a
/// leading slash) — and nothing else: a path we cannot read as a repo is a
/// request we cannot scope, and an unscoped answer is what the grant check
/// exists to stop.
pub fn repo_of_path(path: &str) -> Option<String> {
    let p = path.trim_start_matches('/');
    let p = p.strip_suffix(".git").unwrap_or(p).trim_end_matches('/');
    let re = regex::Regex::new(r"^[^/\s]+/[^/\s]+$").expect("repo path pattern");
    re.is_match(p).then(|| p.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_authority_is_what_git_names_a_host() {
        assert_eq!(
            authority_of("https://github.com").as_deref(),
            Some("github.com")
        );
        assert_eq!(
            authority_of("https://GitHub.com/acme/app").as_deref(),
            Some("github.com")
        );
        assert_eq!(
            authority_of("https://git.example.com:443").as_deref(),
            Some("git.example.com"),
            "the scheme's default port is not named"
        );
        assert_eq!(
            authority_of("http://localhost:3000").as_deref(),
            Some("localhost:3000")
        );
        assert_eq!(authority_of("not a url"), None);
    }

    #[test]
    fn only_an_owner_name_path_is_a_repo() {
        assert_eq!(repo_of_path("acme/app.git").as_deref(), Some("acme/app"));
        assert_eq!(repo_of_path("/acme/app").as_deref(), Some("acme/app"));
        assert_eq!(repo_of_path("acme/app/").as_deref(), Some("acme/app"));
        assert_eq!(repo_of_path(""), None);
        assert_eq!(repo_of_path("acme"), None);
        assert_eq!(repo_of_path("acme/app/extra"), None);
    }
}
