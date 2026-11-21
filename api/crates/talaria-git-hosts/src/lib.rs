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
//              whose origin it names, a repo on the agent's own grant list
//              (checked before any credential is opened), the adapter's
//              repo-scoped login, narrowed on GitHub to what a push spends;
//              the door's whole order in one place (`credential::resolve`),
//              which an admin's "Check token" runs too, and the rows that
//              record what each token carried or why none was given
//   holdings   what a GitHub host's App asks for and what each installation
//              it uses has accepted (`git_host_installations`), refreshed on
//              a schedule, read by the mint, the alerts and the host card
//   permissions which GitHub permissions each agent's token carries on each
//              host (`agent_github_permissions`): its whole set, set by an
//              admin only, or the default; minted into the agent's token
//   revoke     taking an agent's tokens back when what it may do narrows, in
//              this api process and, by a Redis broadcast, in every other
//   requests   an agent asking for a GitHub permission it lacks
//              (`workbench_permission_requests`), and an admin's decision:
//              approved into the agent's set, or denied
//   token_view what the agent is told its credential carries on each granted
//              repo (list_repos, doctor), from the record and never spent
//
// The interface every adapter implements, and every type it answers in, is
// talaria-git-host-types'. `Host` is the dispatch over the registered kinds.

pub mod credential;
pub mod grants;
pub mod holdings;
pub mod permissions;
#[cfg(test)]
mod permissions_live;
pub mod registry;
pub mod requests;
#[cfg(test)]
mod requests_live;
pub mod revoke;
pub mod token_view;

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use serde_json::Value;
use sqlx::PgPool;

use talaria_agent_auth::now_ms;
use talaria_forgejo::Forgejo;
use talaria_github::Github;
use talaria_secretbox::SecretBox;

pub use credential::{GrantError, HostRefusal};
pub use grants::{
    GrantChange, PROTECTED_BRANCHES, RepoFlow, RepoRef, RepoRule, add_grant, effective_base,
    granted_repos, list_repo_flows, push_allowed, repo_flow, repo_rule, repo_rules, resolve_grant,
    set_granted_repos, set_repo_flow, set_repo_rule,
};
pub use holdings::{host_status, refresh_host};
pub use registry::{HostRow, get_host, host_rows, list_hosts};
pub use talaria_git_host_types::{
    CreatedRepo, GitHost, HostKind, HostStatus, InstallationStatus, IssuedLogin, Level, Login,
    Narrowing, OpenPullRequest, OpenedPullRequest, PermissionSet, PullRequestAsk, PullRequestRef,
    RepoAccess, RepoVisibility, pr_number_from_url,
};
pub use talaria_github::hashed_token;

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

    fn narrowing(&self) -> Narrowing {
        match self {
            Host::Github(h) => h.narrowing(),
            Host::Forgejo(h) => h.narrowing(),
        }
    }

    async fn repo_login(
        &self,
        repo: &str,
        access: &RepoAccess,
    ) -> Result<Option<IssuedLogin>, String> {
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

/// A live connection for `row`. A GitHub connection reads what its
/// installations hold from the registry's record before it asks GitHub
/// ([`holdings`]).
pub async fn connect(pg: &PgPool, sb: &SecretBox, row: &HostRow) -> Host {
    match row.kind {
        HostKind::Github => Host::Github(
            Github::new(
                &row.id,
                &row.base_url,
                row.api_url.as_deref(),
                &row.auth,
                sb.clone(),
            )
            .with_holdings(holdings::store(pg, sb)),
        ),
        HostKind::Forgejo => Host::Forgejo(Forgejo::new(
            &row.id,
            &row.base_url,
            row.api_url.as_deref(),
            secret_login(pg, sb, row).await,
        )),
    }
}

/// Whether `row`'s connection can narrow a credential per agent
/// ([`GitHost::narrowing`]), from its configuration alone: no secret is
/// opened and nothing is asked of the host.
pub fn narrowing_of(row: &HostRow) -> Narrowing {
    match row.kind {
        HostKind::Github => Github::new(
            &row.id,
            &row.base_url,
            row.api_url.as_deref(),
            &row.auth,
            SecretBox::default(),
        )
        .narrowing(),
        HostKind::Forgejo => {
            Forgejo::new(&row.id, &row.base_url, row.api_url.as_deref(), Ok(None)).narrowing()
        }
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

/// Where the sandbox's credential ask lands, decided BEFORE any credential is
/// opened, spent or minted: the workspace store's answer is a spend (audited,
/// and it burns a one-shot), so a repo the agent may not touch must be
/// refused before that store is ever asked.
///
/// A WORKSPACE SECRET SPENDABLE AT A REGISTERED HOST answers only through
/// that host's grant check. The store matches a secret by its Spendable-at
/// list, which covers subdomains (`github.com` covers `api.github.com`) and
/// names a hostname without its port, while a host is registered at exactly
/// one authority. So the PAT pinned for github.com would otherwise answer an
/// ask for `api.github.com`, for a registered origin's bare hostname without
/// its port, or for another host on the same list — for any repo — and the
/// value it hands out works at github.com all the same. `withheld` names
/// every registered host such an answer must not cover ([`HostRow::names`]),
/// and the store passes over a secret whose list covers one of them.
#[derive(Debug)]
pub enum CredentialScope {
    /// No registered host is at the authority git named. The workspace store
    /// answers by host, as it always has, minus a secret that is also
    /// spendable at a registered host: `withheld` names every one.
    Unregistered { withheld: Vec<String> },
    /// A registered host, and the repo git named is not on this agent's grant
    /// list there, or git named no repo at all. Nothing answers.
    Ungranted {
        host_id: String,
        repo: Option<String>,
    },
    /// A registered host, and a repo this agent holds a grant on. The
    /// workspace store answers first (a credential somebody pinned for the
    /// host is the more specific answer), then the host's own login. A
    /// secret also spendable at ANOTHER registered host passed only this
    /// host's grant check, so `withheld` names every other one. The row is
    /// boxed only to keep the variants of one size.
    Granted {
        row: Box<HostRow>,
        rule: RepoRule,
        withheld: Vec<String>,
    },
}

impl CredentialScope {
    /// The registered host names a workspace secret must not cover to answer
    /// this ask (see the variants). An `Ungranted` ask asks no store at all.
    pub fn withheld(&self) -> &[String] {
        match self {
            CredentialScope::Unregistered { withheld }
            | CredentialScope::Granted { withheld, .. } => withheld,
            CredentialScope::Ungranted { .. } => &[],
        }
    }
}

/// THE GRANT CHECK of `/api/secrets/git-credential`, by the authority and the
/// path git sends. On a registered host the path is what scopes an answer to
/// one repository: git sends it when `credential.useHttpPath` is set (the
/// rendered gitconfig sets it, and the helper reads `origin` when git omits
/// it), and a path that names no repo is a request nothing can be scoped to.
/// The grant is the one-row read ([`repo_rule`]): exactly the repo, on
/// exactly that host.
pub async fn credential_scope(
    pg: &PgPool,
    agent_id: &str,
    authority: &str,
    path: Option<&str>,
) -> Result<CredentialScope, String> {
    let rows = host_rows(pg).await?;
    let want = authority.trim().to_lowercase();
    let Some(row) = rows.iter().find(|r| r.authority() == want) else {
        return Ok(CredentialScope::Unregistered {
            withheld: withheld_names(&rows, None),
        });
    };
    let Some(repo) = repo_of_path(path.unwrap_or_default()) else {
        return Ok(CredentialScope::Ungranted {
            host_id: row.id.clone(),
            repo: None,
        });
    };
    match repo_rule(pg, agent_id, &row.id, &repo).await {
        Some(rule) => Ok(CredentialScope::Granted {
            withheld: withheld_names(&rows, Some(row.id.as_str())),
            row: Box::new(row.clone()),
            rule,
        }),
        None => Ok(CredentialScope::Ungranted {
            host_id: row.id.clone(),
            repo: Some(repo),
        }),
    }
}

/// The names of every registered host but `except` — what a workspace secret
/// must not cover to answer an ask that is not on that host's grant list.
pub(crate) fn withheld_names(rows: &[HostRow], except: Option<&str>) -> Vec<String> {
    rows.iter()
        .filter(|r| Some(r.id.as_str()) != except)
        .flat_map(HostRow::names)
        .collect()
}

/// A login handed to an agent's git by the registered host, what it was for,
/// and what is known about it — what the credential door's audit row records.
pub struct AgentCredential {
    pub login: Login,
    pub host_id: String,
    pub repo: String,
    /// The GitHub App installation the token was minted on. None for a PAT
    /// and for every other kind of host.
    pub installation: Option<String>,
    /// What the token was asked for: the agent's set on the host (or the
    /// default) as the mint read it.
    pub asked: PermissionSet,
    /// What the token carries, when the host says (a GitHub App token). None
    /// when it does not: a PAT or a Forgejo token carries whatever its owner
    /// gave it.
    pub carried: Option<PermissionSet>,
    /// What the ask named that the token does not carry: what the GitHub
    /// App's installation does not hold, what GitHub would not put on a
    /// repository token, and what GitHub left off the token it minted.
    pub shortfall: PermissionSet,
    /// GitHub's words for what it would not put on the token, when it said
    /// (organization permissions on a repository token, or keys it left off).
    pub shortfall_why: Option<String>,
    /// The part of `shortfall` the installation did not hold at the mint.
    pub unheld: PermissionSet,
    /// The part of `shortfall` GitHub was asked for and left off the token.
    pub omitted: PermissionSet,
    /// What the token carries that `asked` did not name: keys GitHub put on
    /// a GitHub App token of its own accord, which Talaria kept and never
    /// asks for (`metadata: read` aside, which every token carries). Empty
    /// for a PAT and every other kind of host, which say nothing about what
    /// they carry.
    pub added: PermissionSet,
    /// Minted by this call, rather than served from a cache or stored.
    pub minted: bool,
}

impl std::fmt::Debug for AgentCredential {
    /// Everything but the token, which is never printed.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AgentCredential")
            .field("username", &self.login.username)
            .field("password", &"<redacted>")
            .field("host_id", &self.host_id)
            .field("repo", &self.repo)
            .field("installation", &self.installation)
            .field("asked", &self.asked)
            .field("carried", &self.carried)
            .field("shortfall", &self.shortfall)
            .field("shortfall_why", &self.shortfall_why)
            .field("unheld", &self.unheld)
            .field("omitted", &self.omitted)
            .field("added", &self.added)
            .field("minted", &self.minted)
            .finish()
    }
}

/// THE AGENT'S CREDENTIAL from the registered host, for a repo
/// [`credential_scope`] found granted to `agent_id`: the host's own login for
/// that one repository, which the agent spends directly — git push, and the
/// host's API. On GitHub (App mode) an installation token scoped to the
/// repo, filed under the agent, and narrowed to the agent's set on the host
/// ([`permissions`]: what an admin set, or the default — contents and pull
/// requests to write, metadata to read), met with what the installation
/// holds. Elsewhere the host's own token, which cannot be narrowed.
///
/// THE RE-CHECK. What the agent may do is read before the mint, and an admin
/// can narrow it while the mint is out: the narrowing's revoke then finds no
/// token to take back, and the one being minted would carry the old grant
/// for an hour. So after a mint the grant, the Developer switch and the
/// agent's permissions on the host are read again. Changed: the fresh token
/// is revoked, and the agent gets nothing (the grant or the switch went) or
/// one more mint against what it may do now. A token served from the cache
/// is not re-checked: the narrowing's revoke took it out of every cache.
///
/// A FAILURE SAYS WHICH KIND ([`GrantError`]): the host would not give a
/// login (GitHub refused the mint, the token came back wider than asked, the
/// installation holds none of the set, no installation covers the repo), in
/// the host's own sentence, which the door hands the agent; or the database
/// failed, or the host's own credential would not unseal, which stays an
/// internal error.
pub async fn grant_login(
    pg: &PgPool,
    sb: &SecretBox,
    row: &HostRow,
    repo: &str,
    agent_id: &str,
) -> Result<Option<AgentCredential>, GrantError> {
    let host = connect(pg, sb, row).await;
    let set = permissions::host_permissions(pg, agent_id, &row.id)
        .await
        .map_err(GrantError::Internal)?;
    let Some(issued) = issue_login(&host, row, repo, agent_id, &set).await? else {
        return Ok(None);
    };
    if !issued.minted {
        return Ok(Some(issued));
    }
    let now = permissions::recheck(pg, agent_id, &row.id, repo)
        .await
        .map_err(GrantError::Internal)?;
    if now.as_ref() == Some(&set) {
        return Ok(Some(issued));
    }
    let revoked = match &host {
        Host::Github(github) => github.revoke_issued(&issued.login.password).await,
        Host::Forgejo(_) => false,
    };
    tracing::warn!(
        "[git-hosts] what agent {agent_id} may do on {repo} ({}) changed while its token was minted — the fresh token was {}; {}",
        row.id,
        if revoked {
            "revoked"
        } else {
            "dropped, though GitHub did not confirm its revoke"
        },
        match &now {
            Some(_) => "minting once more against the grant as it stands",
            None => "the grant or the Developer switch is gone, so it gets nothing",
        }
    );
    let Some(now) = now else {
        return Ok(None);
    };
    issue_login(&host, row, repo, agent_id, &now).await
}

/// One login from `host` for `repo`, filed under `agent_id` and asked for
/// `set`. On GitHub the mint's own failure is kept typed (talaria-github's
/// `MintError`): its kind and shortfall ride on the refusal beside its
/// sentence. Every other kind of host says only its sentence.
async fn issue_login(
    host: &Host,
    row: &HostRow,
    repo: &str,
    agent_id: &str,
    set: &PermissionSet,
) -> Result<Option<AgentCredential>, GrantError> {
    let credential = |login: Login, issued: talaria_github::IssuedToken| AgentCredential {
        login,
        host_id: row.id.clone(),
        repo: repo.to_string(),
        installation: issued.installation,
        asked: set.clone(),
        carried: issued.carried,
        shortfall: issued.shortfall,
        shortfall_why: issued.shortfall_why,
        unheld: issued.unheld,
        omitted: issued.omitted,
        added: issued.added,
        minted: issued.minted,
    };
    match host {
        Host::Github(github) => match github.agent_token(repo, set, agent_id).await {
            Ok(Some(issued)) => {
                let login = Login {
                    username: "x-access-token".into(),
                    password: issued.token.clone(),
                };
                Ok(Some(credential(login, issued)))
            }
            Ok(None) => Ok(None),
            // The App key or the PAT would not unseal: Talaria's own state,
            // never the agent's to read.
            Err(talaria_github::MintError::Local(e)) => Err(GrantError::Internal(e)),
            Err(e) => Err(GrantError::Host(HostRefusal::of_mint(&e, set))),
        },
        Host::Forgejo(_) => {
            let job = RepoAccess::Job {
                agent_id: agent_id.to_string(),
                permissions: set.clone(),
            };
            match host.repo_login(repo, &job).await {
                Ok(Some(issued)) => Ok(Some(agent_credential(row, repo, set, issued))),
                Ok(None) => Ok(None),
                // A Forgejo login is the host's workspace secret, read here:
                // its failure names that secret and its state, or the
                // database's error. The log keeps those words; the agent and
                // the trail get a sentence that names nothing of the
                // workspace.
                Err(e) => {
                    tracing::warn!(
                        "[git-hosts] the login of git host {} could not be read for {repo}: {e}",
                        row.id
                    );
                    Err(GrantError::Host(HostRefusal {
                        message: forgejo_login_unreadable(row),
                        kind: "credential",
                        shortfall: PermissionSet::new(),
                        asked: set.clone(),
                    }))
                }
            }
        }
    }
}

/// What an agent's git is told when the credential a Forgejo host's row
/// names cannot be read: which host, and who fixes it, never the secret.
fn forgejo_login_unreadable(row: &HostRow) -> String {
    format!(
        "the git host {} has no usable login right now — an admin checks the credential it names in Admin → Git hosts",
        row.name
    )
}

/// The credential door's record of a login `row` issued for `repo` through
/// the adapter's interface (a host other than GitHub).
fn agent_credential(
    row: &HostRow,
    repo: &str,
    asked: &PermissionSet,
    issued: IssuedLogin,
) -> AgentCredential {
    AgentCredential {
        login: issued.login,
        host_id: row.id.clone(),
        repo: repo.to_string(),
        installation: issued.installation,
        asked: asked.clone(),
        carried: issued.carried,
        unheld: issued.shortfall.clone(),
        shortfall: issued.shortfall,
        shortfall_why: issued.shortfall_why,
        omitted: PermissionSet::new(),
        added: PermissionSet::new(),
        minted: issued.minted,
    }
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

    #[test]
    fn a_registered_host_withholds_every_name_a_secret_could_cover_it_by() {
        let row = |id: &str, base_url: &str| HostRow {
            id: id.into(),
            kind: HostKind::Forgejo,
            name: id.into(),
            base_url: base_url.into(),
            api_url: None,
            auth: serde_json::json!({}),
            repo_creation_orgs: vec![],
            created_at: String::new(),
            updated_at: String::new(),
        };
        let rows = vec![
            row("github", "https://github.com"),
            row("forge", "https://Git.Example.com:3000"),
        ];
        // A default port is one name; an origin with a port is reached by its
        // bare hostname too, which is what a Spendable-at list names.
        assert_eq!(rows[0].names(), vec!["github.com"]);
        assert_eq!(
            rows[1].names(),
            vec!["git.example.com", "git.example.com:3000"]
        );
        // An ask on no registered host withholds them all.
        assert_eq!(
            withheld_names(&rows, None),
            vec!["github.com", "git.example.com", "git.example.com:3000"]
        );
        // An ask granted on one withholds every other.
        assert_eq!(
            withheld_names(&rows, Some("github")),
            vec!["git.example.com", "git.example.com:3000"]
        );
        let scope = CredentialScope::Ungranted {
            host_id: "github".into(),
            repo: None,
        };
        assert!(
            scope.withheld().is_empty(),
            "an ungranted ask asks no store"
        );
    }

    /// A login that fails on Talaria's own side is never the host's refusal:
    /// a Forgejo host whose credential cannot be read tells the agent a
    /// sentence that names no secret, and a GitHub App key that will not
    /// unseal is an internal error, never handed to the agent's git.
    #[tokio::test]
    async fn a_login_that_fails_on_talarias_side_names_nothing_of_the_workspace() {
        let set = PermissionSet::of(&[("contents", Level::Write)]);
        let forge = HostRow {
            id: "forge".into(),
            kind: HostKind::Forgejo,
            name: "Forge".into(),
            base_url: "https://git.example.com".into(),
            api_url: None,
            auth: serde_json::json!({}),
            repo_creation_orgs: vec![],
            created_at: String::new(),
            updated_at: String::new(),
        };
        let unreadable = Host::Forgejo(Forgejo::new(
            &forge.id,
            &forge.base_url,
            None,
            Err("the workspace secret \"forge-bot\" could not be opened".into()),
        ));
        let Err(GrantError::Host(refusal)) =
            issue_login(&unreadable, &forge, "acme/app", "a1", &set).await
        else {
            panic!("a refusal with a sentence");
        };
        assert_eq!(refusal.kind, "credential");
        assert!(refusal.message.contains("Forge"), "{}", refusal.message);
        assert!(
            !refusal.message.contains("forge-bot"),
            "{}",
            refusal.message
        );
        assert_eq!(refusal.asked, set);

        let github = HostRow {
            id: "gh-unsealable".into(),
            kind: HostKind::Github,
            name: "GitHub".into(),
            base_url: "https://github.com".into(),
            // Nothing listens here: the key fails before any request.
            api_url: Some("http://127.0.0.1:9".into()),
            auth: serde_json::json!({
                "mode": "app",
                "app": { "appId": "7", "installationIds": ["91"], "privateKeyEnc": "not-sealed" },
            }),
            repo_creation_orgs: vec![],
            created_at: String::new(),
            updated_at: String::new(),
        };
        let sb = SecretBox::from_parts(
            [7u8; 32],
            HashMap::from([(1u32, talaria_secretbox::new_dek().expect("entropy"))]),
            Some(1),
        );
        let host = Host::Github(Github::new(
            &github.id,
            &github.base_url,
            github.api_url.as_deref(),
            &github.auth,
            sb,
        ));
        match issue_login(&host, &github, "acme/app", "a1", &set).await {
            Err(GrantError::Internal(e)) => assert!(e.contains("unseal"), "{e}"),
            other => panic!("an internal error, not {other:?}"),
        }
    }
}
