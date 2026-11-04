// The Forgejo adapter of the git-host interface (talaria-git-host-types'
// `GitHost`). Forgejo and Gitea answer the same REST API under
// `<server>/api/v1`, so one adapter serves both — a Codeberg account, a
// company's self-hosted Forgejo, or a Gitea.
//
// HOW IT AUTHENTICATES. A Forgejo host row names a WORKSPACE SECRET
// (envelope-encrypted in Postgres, docs/ENCRYPTION.md) holding an access
// token — and optionally the username it belongs to. The registry
// (talaria-git-hosts) opens it and hands this adapter the login; nothing here
// reads a file, an env var or the database. REST calls send the token as
// `Authorization: token …`; git sends it as the password of an https login.
//
// WHAT IT CANNOT DO THAT GITHUB CAN. A Forgejo access token cannot be scoped
// to one repository or narrowed per call: `repo_login` hands out the host's
// token whatever the access asked. Give the token to a bot account that can
// see only the repositories agents work on, with the `write:repository`,
// `write:issue` (comments) and `read:user` / `read:organization` scopes.
//
// TALARIA STOPS AT THE PR, here as everywhere: the only writes are cutting a
// job branch, opening a pull request (or commenting on the one a branch has),
// creating a repository an admin approved, and protecting its default branch.

use std::time::Duration;

use serde_json::{Value, json};

use talaria_body::truncate_utf16;
use talaria_gateway::provider::http;
pub use talaria_git_host_types::{
    CreatedRepo, GitHost, HostKind, HostStatus, Login, OpenPullRequest, OpenedPullRequest,
    PullRequestAsk, PullRequestRef, RepoAccess, RepoVisibility,
};
use talaria_git_host_types::{PrStatusParts, pr_status_wire, value_at};

/// The most one REST call may take.
const CALL_DEADLINE: Duration = Duration::from_secs(30);

/// The most pages any listing reads.
const MAX_PAGES: u32 = 5;

/// One page's size (Forgejo's own ceiling is 50 by default).
const PAGE: u32 = 50;

/// The API origin a Forgejo at `web` answers on.
pub fn api_origin_for(web: &str) -> String {
    format!("{}/api/v1", web.trim_end_matches('/'))
}

/// Percent-encode a path, keeping `/` — Forgejo's branch and compare routes
/// take a ref with its slashes (`/branches/talaria/tala-7-fix`).
pub fn enc_path(path: &str) -> String {
    let mut out = String::new();
    for b in path.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// One Forgejo or Gitea connection: a `git_hosts` row of kind `forgejo`.
#[derive(Clone)]
pub struct Forgejo {
    pub host_id: String,
    /// The web origin (`https://codeberg.org`, `https://git.example.com`).
    pub web: String,
    /// The REST origin (`<web>/api/v1`).
    pub api: String,
    /// The token (and its username) the row's workspace secret holds. None
    /// when the row names no secret: public repositories are still readable.
    login: Option<Login>,
    /// Why the login is missing, when the row named a secret that could not
    /// be opened — said instead of a silent anonymous connection.
    login_error: Option<String>,
}

impl std::fmt::Debug for Forgejo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Forgejo")
            .field("host_id", &self.host_id)
            .field("web", &self.web)
            .field("api", &self.api)
            .field("login", &self.login)
            .finish()
    }
}

/// A REST failure, kept apart so a caller can branch on the status.
struct CallError {
    status: u16,
    message: String,
}

impl Forgejo {
    pub fn new(
        host_id: &str,
        web: &str,
        api: Option<&str>,
        login: Result<Option<Login>, String>,
    ) -> Forgejo {
        let web = web.trim_end_matches('/').to_string();
        let api = api
            .map(|a| a.trim_end_matches('/').to_string())
            .filter(|a| !a.is_empty())
            .unwrap_or_else(|| api_origin_for(&web));
        let (login, login_error) = match login {
            Ok(l) => (l, None),
            Err(e) => (None, Some(e)),
        };
        Forgejo {
            host_id: host_id.to_string(),
            web,
            api,
            login,
            login_error,
        }
    }

    fn token(&self) -> Result<&str, String> {
        match (&self.login, &self.login_error) {
            (Some(l), _) => Ok(&l.password),
            (None, Some(e)) => Err(e.clone()),
            (None, None) => Err(format!(
                "the git host \"{}\" has no credential — an admin names a workspace secret for it in Admin → Git hosts",
                self.host_id
            )),
        }
    }

    async fn call(
        &self,
        method: &str,
        path: &str,
        body: Option<&Value>,
    ) -> Result<Value, CallError> {
        let mut req = http()
            .request(
                reqwest::Method::from_bytes(method.as_bytes()).map_err(|_| CallError {
                    status: 0,
                    message: format!("forgejo method: {method}"),
                })?,
                format!("{}{path}", self.api),
            )
            .timeout(CALL_DEADLINE)
            .header("accept", "application/json")
            .header("user-agent", "talaria-workbench");
        if let Some(l) = &self.login {
            req = req.header("authorization", format!("token {}", l.password));
        }
        if let Some(body) = body {
            req = req
                .header("content-type", "application/json")
                .body(body.to_string());
        }
        let res = req.send().await.map_err(|e| CallError {
            status: 0,
            message: format!("Forgejo {method} {path}: {e}"),
        })?;
        let status = res.status().as_u16();
        let text = res.text().await.unwrap_or_default();
        if !(200..300).contains(&status) {
            let said = serde_json::from_str::<Value>(&text)
                .ok()
                .and_then(|v| v.get("message").and_then(Value::as_str).map(String::from))
                .unwrap_or_else(|| truncate_utf16(&text, 200).to_string());
            return Err(CallError {
                status,
                message: format!("Forgejo {method} {path} → {status}: {said}"),
            });
        }
        if text.trim().is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_str(&text).map_err(|e| CallError {
            status,
            message: format!("forgejo decode: {e}"),
        })
    }

    async fn get(&self, path: &str) -> Result<Value, String> {
        self.call("GET", path, None).await.map_err(|e| e.message)
    }

    /// Every item of a paged listing, at most [`MAX_PAGES`] pages.
    async fn paged(&self, path: &str) -> Result<Vec<Value>, String> {
        let sep = if path.contains('?') { '&' } else { '?' };
        let mut out = Vec::new();
        for page in 1..=MAX_PAGES {
            let items = self
                .get(&format!("{path}{sep}limit={PAGE}&page={page}"))
                .await?;
            let items = items.as_array().cloned().unwrap_or_default();
            let short = items.len() < PAGE as usize;
            out.extend(items);
            if short {
                break;
            }
        }
        Ok(out)
    }

    /// The account the token belongs to.
    async fn me(&self) -> Result<String, String> {
        self.token()?;
        let me = self.get("/user").await?;
        me.get("login")
            .and_then(Value::as_str)
            .map(String::from)
            .ok_or_else(|| "forgejo /user: no login in reply".into())
    }

    async fn comment(&self, repo: &str, number: i64, note: &str) -> bool {
        match self
            .call(
                "POST",
                &format!("/repos/{repo}/issues/{number}/comments"),
                Some(&json!({ "body": note })),
            )
            .await
        {
            Ok(_) => true,
            Err(e) => {
                tracing::warn!(
                    "[forgejo] could not comment on {repo}#{number}: {}",
                    e.message
                );
                false
            }
        }
    }
}

fn pr_ref_of(pr: &Value) -> Option<PullRequestRef> {
    Some(PullRequestRef {
        number: pr.get("number").and_then(Value::as_i64)?,
        url: value_at(pr, &["html_url"])
            .as_str()
            .unwrap_or_default()
            .to_string(),
    })
}

/// A Forgejo commit status, in the wire's vocabulary: `warning` is not a
/// failure, so it reads as passed.
fn status_state(forgejo: &str) -> &'static str {
    match forgejo {
        "success" | "warning" => "success",
        "failure" => "failure",
        "error" => "error",
        _ => "pending",
    }
}

/// A Forgejo review, in the wire's vocabulary. A review REQUEST is not a
/// review, and is left out.
fn review_of(r: &Value) -> Option<Value> {
    let state = r.get("state").and_then(Value::as_str).unwrap_or_default();
    let state = if r.get("dismissed").and_then(Value::as_bool) == Some(true) {
        "DISMISSED"
    } else {
        match state {
            "APPROVED" => "APPROVED",
            "REQUEST_CHANGES" => "CHANGES_REQUESTED",
            "COMMENT" => "COMMENTED",
            "PENDING" => "PENDING",
            _ => return None,
        }
    };
    Some(json!({
        "user": { "login": value_at(r, &["user", "login"]) },
        "state": state,
        "submitted_at": value_at(r, &["submitted_at"]),
    }))
}

fn open_pull_of(pr: &Value) -> Option<OpenPullRequest> {
    let s =
        |path: &[&str]| -> String { value_at(pr, path).as_str().unwrap_or_default().to_string() };
    Some(OpenPullRequest {
        number: pr.get("number").and_then(Value::as_i64)?,
        url: s(&["html_url"]),
        title: s(&["title"]),
        head: s(&["head", "ref"]),
        base: s(&["base", "ref"]),
        author: s(&["user", "login"]),
        draft: pr.get("draft").and_then(Value::as_bool).unwrap_or(false),
    })
}

impl GitHost for Forgejo {
    fn kind(&self) -> HostKind {
        HostKind::Forgejo
    }

    fn clone_url(&self, repo: &str) -> String {
        format!("{}/{repo}.git", self.web)
    }

    fn repo_url(&self, repo: &str) -> String {
        format!("{}/{repo}", self.web)
    }

    async fn status(&self) -> HostStatus {
        if let Err(e) = self.token() {
            return HostStatus {
                configured: false,
                account: None,
                error: Some(e),
            };
        }
        match self.me().await {
            Ok(login) => HostStatus {
                configured: true,
                account: Some(login),
                error: None,
            },
            Err(e) => HostStatus {
                configured: true,
                account: None,
                error: Some(truncate_utf16(&e, 200).to_string()),
            },
        }
    }

    /// The token's own repositories and those of every organization it
    /// belongs to. Never the instance-wide search: on a public host that is
    /// every public repository there is.
    async fn reachable_repos(&self) -> Result<Vec<String>, String> {
        self.token()?;
        let mut out: Vec<String> = Vec::new();
        let mut push = |items: Vec<Value>| {
            for r in items {
                if let Some(name) = r.get("full_name").and_then(Value::as_str)
                    && !out.iter().any(|x| x == name)
                {
                    out.push(name.to_string());
                }
            }
        };
        push(self.paged("/user/repos").await?);
        for org in self.paged("/user/orgs").await? {
            let Some(name) = org.get("username").and_then(Value::as_str) else {
                continue;
            };
            // One org the token cannot list is skipped, not fatal.
            if let Ok(items) = self.paged(&format!("/orgs/{}/repos", enc_path(name))).await {
                push(items);
            }
        }
        Ok(out)
    }

    /// The host's token, whatever `access` asked: a Forgejo token cannot be
    /// scoped to one repository or narrowed per call (see the module header).
    async fn repo_login(&self, _repo: &str, _access: RepoAccess) -> Result<Option<Login>, String> {
        if let Some(e) = &self.login_error {
            return Err(e.clone());
        }
        Ok(self.login.clone())
    }

    async fn default_branch(&self, repo: &str) -> Result<String, String> {
        let j = self.get(&format!("/repos/{repo}")).await?;
        j.get("default_branch")
            .and_then(Value::as_str)
            .filter(|b| !b.is_empty())
            .map(String::from)
            .ok_or_else(|| format!("forgejo: {repo} names no default branch (is it empty?)"))
    }

    async fn list_branches(&self, repo: &str) -> Result<Vec<String>, String> {
        Ok(self
            .paged(&format!("/repos/{repo}/branches"))
            .await?
            .iter()
            .filter_map(|b| b.get("name").and_then(Value::as_str).map(String::from))
            .collect())
    }

    async fn create_branch(&self, repo: &str, branch: &str, base: &str) -> Result<bool, String> {
        self.token()?;
        match self
            .call(
                "GET",
                &format!("/repos/{repo}/branches/{}", enc_path(branch)),
                None,
            )
            .await
        {
            Ok(_) => return Ok(false),
            Err(e) if e.status == 404 => {}
            Err(e) => return Err(e.message),
        }
        match self
            .call(
                "POST",
                &format!("/repos/{repo}/branches"),
                Some(&json!({ "new_branch_name": branch, "old_branch_name": base })),
            )
            .await
        {
            Ok(_) => Ok(true),
            // Cut between the lookup and the create: it is this job's branch.
            Err(e) if e.status == 409 => Ok(false),
            Err(e) => Err(e.message),
        }
    }

    async fn branch_ahead(&self, repo: &str, base: &str, branch: &str) -> Result<i64, String> {
        let cmp = self
            .get(&format!(
                "/repos/{repo}/compare/{}...{}",
                enc_path(base),
                enc_path(branch)
            ))
            .await?;
        Ok(cmp
            .get("total_commits")
            .and_then(Value::as_i64)
            .or_else(|| {
                cmp.get("commits")
                    .and_then(Value::as_array)
                    .map(|c| c.len() as i64)
            })
            .unwrap_or(0))
    }

    fn repo_creation_blocker(&self) -> Option<String> {
        self.token().err()
    }

    /// The token's own account, or an organization where it may create
    /// repositories.
    async fn repo_creation_refusal(&self, owner: &str) -> Result<Option<String>, String> {
        if let Err(e) = self.token() {
            return Ok(Some(e));
        }
        let me = self.me().await?;
        if me.eq_ignore_ascii_case(owner) {
            return Ok(None);
        }
        match self
            .call("GET", &format!("/orgs/{}", enc_path(owner)), None)
            .await
        {
            Ok(_) => {}
            Err(e) if e.status == 404 => {
                return Ok(Some(format!(
                    "there is no organization named {owner} on {} — create it there first, or ask for the repo under {me}",
                    self.web
                )));
            }
            Err(e) => return Err(e.message),
        }
        let perms = self
            .get(&format!(
                "/users/{}/orgs/{}/permissions",
                enc_path(&me),
                enc_path(owner)
            ))
            .await?;
        if perms.get("can_create_repository").and_then(Value::as_bool) == Some(true) {
            Ok(None)
        } else {
            Ok(Some(format!(
                "{me} may not create repositories in {owner} on {} — an owner of {owner} grants it, or the request names another organization",
                self.web
            )))
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
        let me = self.me().await?;
        let path = if me.eq_ignore_ascii_case(owner) {
            "/user/repos".to_string()
        } else {
            format!("/orgs/{}/repos", enc_path(owner))
        };
        let created = self
            .call(
                "POST",
                &path,
                Some(&json!({
                    "name": name,
                    "description": truncate_utf16(description, 300),
                    "private": visibility == RepoVisibility::Private,
                    "auto_init": true,
                })),
            )
            .await
            .map_err(|e| match e.status {
                403 => format!(
                    "{me} may not create repositories in {owner} on {} ({})",
                    self.web, e.message
                ),
                409 => format!("{owner}/{name} already exists on {}", self.web),
                _ => e.message,
            })?;
        let full_name = value_at(&created, &["full_name"])
            .as_str()
            .unwrap_or_default()
            .to_string();
        let default_branch = value_at(&created, &["default_branch"])
            .as_str()
            .unwrap_or("main")
            .to_string();
        // The repo exists now, whatever the protection answers: a refusal is
        // carried back to the approver, never turned into a failed approval.
        // The rule blocks force-push and deletion of the default branch, and
        // nothing else: pushes stay open to everyone who may write, and
        // there are no required approvals or status checks.
        let protection = if protect && !full_name.is_empty() {
            Some(
                self.call(
                    "POST",
                    &format!("/repos/{full_name}/branch_protections"),
                    Some(&json!({
                        "rule_name": default_branch,
                        "branch_name": default_branch,
                        "enable_push": true,
                        "enable_force_push": false,
                        "required_approvals": 0,
                        "enable_status_check": false,
                    })),
                )
                .await
                .map(|_| ())
                .map_err(|e| format!("{} refused the branch protection ({})", self.web, e.message)),
            )
        } else {
            None
        };
        Ok(CreatedRepo {
            url: value_at(&created, &["html_url"])
                .as_str()
                .map(String::from)
                .unwrap_or_else(|| self.repo_url(&full_name)),
            full_name,
            protection,
        })
    }

    async fn find_open_pull_request(
        &self,
        repo: &str,
        branch: &str,
    ) -> Result<Option<PullRequestRef>, String> {
        let open = self
            .paged(&format!("/repos/{repo}/pulls?state=open"))
            .await?;
        Ok(open
            .iter()
            .find(|pr| {
                value_at(pr, &["head", "ref"]).as_str() == Some(branch)
                    && value_at(pr, &["head", "repo", "full_name"])
                        .as_str()
                        .is_none_or(|r| r.eq_ignore_ascii_case(repo))
            })
            .and_then(pr_ref_of))
    }

    async fn open_or_update_pull_request(
        &self,
        repo: &str,
        ask: &PullRequestAsk<'_>,
    ) -> Result<OpenedPullRequest, String> {
        let reuse = |pr: PullRequestRef| async move {
            let commented = !ask.update_note.trim().is_empty()
                && self.comment(repo, pr.number, ask.update_note).await;
            OpenedPullRequest {
                url: pr.url,
                number: pr.number,
                reused: true,
                commented,
            }
        };
        if let Some(pr) = self.find_open_pull_request(repo, ask.head).await? {
            return Ok(reuse(pr).await);
        }
        // Forgejo marks a draft by its title, not a flag.
        let title = if ask.draft {
            format!("WIP: {}", ask.title)
        } else {
            ask.title.to_string()
        };
        match self
            .call(
                "POST",
                &format!("/repos/{repo}/pulls"),
                Some(&json!({
                    "head": ask.head,
                    "base": ask.base,
                    "title": title,
                    "body": ask.body,
                })),
            )
            .await
        {
            Ok(pr) => {
                let opened = pr_ref_of(&pr).ok_or("forgejo pull request: no number in reply")?;
                Ok(OpenedPullRequest {
                    url: opened.url,
                    number: opened.number,
                    reused: false,
                    commented: false,
                })
            }
            // Opened between the lookup and this POST: Forgejo's 409 says a
            // PR already exists for the head, and that PR is this job's.
            Err(e) if e.status == 409 => match self.find_open_pull_request(repo, ask.head).await? {
                Some(pr) => Ok(reuse(pr).await),
                None => Err(e.message),
            },
            Err(e) => Err(e.message),
        }
    }

    async fn open_pull_requests(
        &self,
        repo: &str,
        limit: u32,
    ) -> Result<Vec<OpenPullRequest>, String> {
        let limit = limit.clamp(1, PAGE);
        let reply = self
            .get(&format!("/repos/{repo}/pulls?state=open&limit={limit}"))
            .await?;
        let mut prs: Vec<OpenPullRequest> = reply
            .as_array()
            .map(|a| a.iter().filter_map(open_pull_of).collect())
            .unwrap_or_default();
        prs.sort_by_key(|p| std::cmp::Reverse(p.number));
        prs.truncate(limit as usize);
        Ok(prs)
    }

    async fn pull_request_status(&self, repo: &str, number: i64) -> Result<Value, String> {
        let pr = self.get(&format!("/repos/{repo}/pulls/{number}")).await?;
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
        let head_sha = value_at(&pr, &["head", "sha"])
            .as_str()
            .unwrap_or_default()
            .to_string();
        // Checks and reviews are read separately and a failure to read one
        // is SAID, not thrown: the PR's own state is still worth having.
        let mut errors: Vec<String> = Vec::new();
        let mut statuses: Vec<Value> = Vec::new();
        if !head_sha.is_empty() {
            match self
                .get(&format!("/repos/{repo}/commits/{head_sha}/status"))
                .await
            {
                Ok(j) => {
                    statuses = j
                        .get("statuses")
                        .and_then(Value::as_array)
                        .map(|a| {
                            a.iter()
                                .map(|s| {
                                    json!({
                                        "context": value_at(s, &["context"]),
                                        "state": status_state(
                                            s.get("status").and_then(Value::as_str).unwrap_or_default()
                                        ),
                                        "target_url": value_at(s, &["target_url"]),
                                    })
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                }
                Err(e) => errors.push(format!("commit statuses unreadable: {e}")),
            }
        }
        let reviews = match self
            .get(&format!("/repos/{repo}/pulls/{number}/reviews"))
            .await
        {
            Ok(j) => Some(
                j.as_array()
                    .map(|a| a.iter().filter_map(review_of).collect())
                    .unwrap_or_default(),
            ),
            Err(e) => {
                errors.push(format!("reviews unreadable: {e}"));
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
            // Forgejo Actions (and any CI) report as commit statuses; there
            // are no check runs.
            runs: Vec::new(),
            statuses,
            reviews,
            requested_reviewers: requested,
            errors,
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use super::*;

    // ── A loopback Forgejo (the workspace's stub-listener pattern) ───────────

    #[derive(Debug, Clone)]
    struct Seen {
        method: String,
        path: String,
        auth: String,
        body: String,
    }

    type Route = dyn Fn(&Seen) -> (u16, String) + Send + Sync;

    async fn stub(
        route: impl Fn(&Seen) -> (u16, String) + Send + Sync + 'static,
    ) -> (String, Arc<Mutex<Vec<Seen>>>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("a loopback listener binds");
        let base = format!("http://127.0.0.1:{}", listener.local_addr().unwrap().port());
        let seen = Arc::new(Mutex::new(Vec::<Seen>::new()));
        let log = seen.clone();
        let route: Arc<Route> = Arc::new(route);
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
                    log.lock().unwrap().push(req.clone());
                    let (status, answer) = route(&req);
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

    fn conn(base: &str) -> Forgejo {
        Forgejo::new(
            "forge",
            base,
            None,
            Ok(Some(Login {
                username: "bot".into(),
                password: "tok".into(),
            })),
        )
    }

    fn ask<'a>(note: &'a str) -> PullRequestAsk<'a> {
        PullRequestAsk {
            head: "talaria/tala-7-fix",
            base: "main",
            title: "[TALA-7] Fix",
            body: "the body",
            draft: false,
            update_note: note,
        }
    }

    #[test]
    fn urls_are_the_hosts_own() {
        let f = conn("https://git.example.com/");
        assert_eq!(f.api, "https://git.example.com/api/v1");
        assert_eq!(
            f.clone_url("acme/notes"),
            "https://git.example.com/acme/notes.git"
        );
        assert_eq!(
            f.repo_url("acme/notes"),
            "https://git.example.com/acme/notes"
        );
        assert_eq!(enc_path("talaria/tala 7"), "talaria/tala%207");
    }

    #[tokio::test]
    async fn a_host_without_a_secret_says_so_instead_of_failing_quietly() {
        let f = Forgejo::new("forge", "https://git.example.com", None, Ok(None));
        let st = f.status().await;
        assert!(!st.configured);
        assert!(st.error.unwrap().contains("names a workspace secret"));
        assert!(f.repo_creation_blocker().is_some());
        let broken = Forgejo::new(
            "forge",
            "https://git.example.com",
            None,
            Err("the workspace secret \"forge\" has expired".into()),
        );
        assert_eq!(
            broken
                .repo_login("a/b", RepoAccess::Read)
                .await
                .unwrap_err(),
            "the workspace secret \"forge\" has expired"
        );
    }

    #[tokio::test]
    async fn the_token_rides_as_a_forgejo_token_and_status_names_the_account() {
        let (base, seen) = stub(|req| match req.path.as_str() {
            "/api/v1/user" => (200, r#"{"login":"talaria-bot"}"#.into()),
            _ => (404, "{}".into()),
        })
        .await;
        let st = conn(&base).status().await;
        assert!(st.configured);
        assert_eq!(st.account.as_deref(), Some("talaria-bot"));
        assert_eq!(seen.lock().unwrap()[0].auth, "token tok");
    }

    #[tokio::test]
    async fn reachable_repos_are_the_tokens_own_and_its_orgs_never_a_search() {
        let (base, seen) = stub(|req| match req.path.as_str() {
            "/api/v1/user/repos?limit=50&page=1" => {
                (200, r#"[{"full_name":"bot/scratch"}]"#.into())
            }
            "/api/v1/user/orgs?limit=50&page=1" => (200, r#"[{"username":"acme"}]"#.into()),
            "/api/v1/orgs/acme/repos?limit=50&page=1" => (
                200,
                r#"[{"full_name":"acme/notes"},{"full_name":"bot/scratch"}]"#.into(),
            ),
            _ => (404, "{}".into()),
        })
        .await;
        let repos = conn(&base).reachable_repos().await.unwrap();
        assert_eq!(repos, vec!["bot/scratch", "acme/notes"]);
        assert!(
            !seen
                .lock()
                .unwrap()
                .iter()
                .any(|r| r.path.contains("/repos/search"))
        );
    }

    #[tokio::test]
    async fn a_branch_is_cut_once_and_an_existing_one_is_left_alone() {
        let (base, seen) = stub(|req| match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/api/v1/repos/acme/notes/branches/talaria/tala-7-fix") => {
                (404, r#"{"message":"not found"}"#.into())
            }
            ("POST", "/api/v1/repos/acme/notes/branches") => (201, r#"{"name":"x"}"#.into()),
            ("GET", "/api/v1/repos/acme/notes/branches/talaria/exists") => {
                (200, r#"{"name":"talaria/exists"}"#.into())
            }
            _ => (500, "{}".into()),
        })
        .await;
        let f = conn(&base);
        assert!(
            f.create_branch("acme/notes", "talaria/tala-7-fix", "main")
                .await
                .unwrap()
        );
        assert!(
            !f.create_branch("acme/notes", "talaria/exists", "main")
                .await
                .unwrap()
        );
        let seen = seen.lock().unwrap().clone();
        let post = seen.iter().find(|r| r.method == "POST").unwrap();
        let body: Value = serde_json::from_str(&post.body).unwrap();
        assert_eq!(body["new_branch_name"], "talaria/tala-7-fix");
        assert_eq!(body["old_branch_name"], "main");
        assert_eq!(seen.iter().filter(|r| r.method == "POST").count(), 1);
    }

    #[tokio::test]
    async fn a_branch_with_an_open_pr_keeps_it_and_gets_the_note() {
        let (base, seen) = stub(|req| match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/api/v1/repos/acme/notes/pulls?state=open&limit=50&page=1") => (
                200,
                r#"[{"number":4,"html_url":"https://git.example.com/acme/notes/pulls/4","head":{"ref":"other"}},{"number":3,"html_url":"https://git.example.com/acme/notes/pulls/3","head":{"ref":"talaria/tala-7-fix","repo":{"full_name":"acme/notes"}}}]"#.into(),
            ),
            ("POST", "/api/v1/repos/acme/notes/issues/3/comments") => (201, "{}".into()),
            _ => (500, "{}".into()),
        })
        .await;
        let opened = conn(&base)
            .open_or_update_pull_request("acme/notes", &ask("round two"))
            .await
            .unwrap();
        assert_eq!(
            opened,
            OpenedPullRequest {
                url: "https://git.example.com/acme/notes/pulls/3".into(),
                number: 3,
                reused: true,
                commented: true,
            }
        );
        assert!(
            !seen
                .lock()
                .unwrap()
                .iter()
                .any(|r| r.method == "POST" && r.path.ends_with("/pulls")),
            "never a second PR"
        );
    }

    #[tokio::test]
    async fn a_branch_without_a_pr_gets_one_and_a_race_resolves_to_the_one_that_exists() {
        let (base, seen) = stub(|req| match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/api/v1/repos/acme/notes/pulls?state=open&limit=50&page=1") => {
                (200, "[]".into())
            }
            ("POST", "/api/v1/repos/acme/notes/pulls") => (
                201,
                r#"{"number":5,"html_url":"https://git.example.com/acme/notes/pulls/5"}"#.into(),
            ),
            _ => (500, "{}".into()),
        })
        .await;
        let opened = conn(&base)
            .open_or_update_pull_request("acme/notes", &ask("unused"))
            .await
            .unwrap();
        assert_eq!((opened.number, opened.reused), (5, false));
        let post = seen
            .lock()
            .unwrap()
            .iter()
            .find(|r| r.method == "POST")
            .cloned()
            .unwrap();
        let body: Value = serde_json::from_str(&post.body).unwrap();
        assert_eq!(body["head"], "talaria/tala-7-fix");
        assert_eq!(body["base"], "main");

        let calls = Arc::new(Mutex::new(0u32));
        let c = calls.clone();
        let (base, _) = stub(move |req| match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/api/v1/repos/acme/notes/pulls?state=open&limit=50&page=1") => {
                let mut n = c.lock().unwrap();
                *n += 1;
                if *n == 1 {
                    (200, "[]".into())
                } else {
                    (
                        200,
                        r#"[{"number":6,"html_url":"https://git.example.com/acme/notes/pulls/6","head":{"ref":"talaria/tala-7-fix"}}]"#.into(),
                    )
                }
            }
            ("POST", "/api/v1/repos/acme/notes/pulls") => (
                409,
                r#"{"message":"pull request already exists for these targets"}"#.into(),
            ),
            _ => (201, "{}".into()),
        })
        .await;
        let opened = conn(&base)
            .open_or_update_pull_request("acme/notes", &ask("note"))
            .await
            .unwrap();
        assert_eq!((opened.number, opened.reused), (6, true));
    }

    #[tokio::test]
    async fn pr_status_speaks_the_shared_wire_and_only_reads() {
        let (base, seen) = stub(|req| match req.path.as_str() {
            "/api/v1/repos/acme/notes/pulls/3" => (
                200,
                r#"{"number":3,"html_url":"https://git.example.com/acme/notes/pulls/3","title":"[TALA-7] Fix","state":"open","merged":false,"merged_at":null,"mergeable":true,"base":{"ref":"main"},"head":{"ref":"talaria/tala-7-fix","sha":"abc"},"requested_reviewers":[{"login":"dee"}]}"#.into(),
            ),
            "/api/v1/repos/acme/notes/commits/abc/status" => (
                200,
                r#"{"state":"failure","statuses":[{"context":"ci/test","status":"failure","target_url":"https://ci/1"},{"context":"ci/lint","status":"warning","target_url":"https://ci/2"}]}"#.into(),
            ),
            "/api/v1/repos/acme/notes/pulls/3/reviews" => (
                200,
                r#"[{"user":{"login":"ann"},"state":"REQUEST_CHANGES","submitted_at":"2026-10-27T01:00:00Z"},{"user":{"login":"bob"},"state":"REQUEST_REVIEW"},{"user":{"login":"cy"},"state":"APPROVED","dismissed":true}]"#.into(),
            ),
            _ => (500, "{}".into()),
        })
        .await;
        let st = conn(&base)
            .pull_request_status("acme/notes", 3)
            .await
            .unwrap();
        assert_eq!(st["state"], "open");
        assert_eq!(st["headSha"], "abc");
        assert_eq!(st["checks"]["verdict"], "failing");
        assert_eq!(st["checks"]["statuses"][0]["state"], "failure");
        assert_eq!(st["checks"]["statuses"][1]["state"], "success");
        assert_eq!(st["reviewDecision"], "changes_requested");
        assert_eq!(
            st["reviews"].as_array().unwrap().len(),
            2,
            "a review request is not a review"
        );
        assert_eq!(st["reviews"][1]["state"], "dismissed");
        assert_eq!(st["requestedReviewers"][0], "dee");
        assert!(seen.lock().unwrap().iter().all(|r| r.method == "GET"));
    }

    #[tokio::test]
    async fn a_merged_pr_reads_merged() {
        let (base, _) = stub(|req| match req.path.as_str() {
            "/api/v1/repos/acme/notes/pulls/2" => (
                200,
                r#"{"number":2,"state":"closed","merged":true,"merged_at":"2026-10-27T02:00:00Z","head":{"ref":"b","sha":"def"},"base":{"ref":"main"}}"#.into(),
            ),
            "/api/v1/repos/acme/notes/commits/def/status" => (200, r#"{"statuses":[]}"#.into()),
            "/api/v1/repos/acme/notes/pulls/2/reviews" => (200, "[]".into()),
            _ => (500, "{}".into()),
        })
        .await;
        let st = conn(&base)
            .pull_request_status("acme/notes", 2)
            .await
            .unwrap();
        assert_eq!(st["state"], "merged");
        assert_eq!(st["checks"]["verdict"], "none");
        assert_eq!(st["reviewDecision"], "none");
    }

    #[tokio::test]
    async fn repo_creation_goes_to_the_org_and_protects_only_against_force_push_and_deletion() {
        let (base, seen) = stub(|req| match (req.method.as_str(), req.path.as_str()) {
            ("GET", "/api/v1/user") => (200, r#"{"login":"talaria-bot"}"#.into()),
            ("GET", "/api/v1/orgs/acme") => (200, r#"{"username":"acme"}"#.into()),
            ("GET", "/api/v1/users/talaria-bot/orgs/acme/permissions") => {
                (200, r#"{"can_create_repository":true}"#.into())
            }
            ("POST", "/api/v1/orgs/acme/repos") => (
                201,
                r#"{"full_name":"acme/talaria-app-notes","html_url":"https://git.example.com/acme/talaria-app-notes","default_branch":"main"}"#.into(),
            ),
            ("POST", "/api/v1/repos/acme/talaria-app-notes/branch_protections") => {
                (201, "{}".into())
            }
            _ => (500, "{}".into()),
        })
        .await;
        let f = conn(&base);
        assert_eq!(f.repo_creation_refusal("acme").await.unwrap(), None);
        let created = f
            .create_repo(
                "acme",
                "talaria-app-notes",
                "notes",
                RepoVisibility::Private,
                true,
            )
            .await
            .unwrap();
        assert_eq!(created.full_name, "acme/talaria-app-notes");
        assert_eq!(created.protection, Some(Ok(())));
        let seen = seen.lock().unwrap().clone();
        let create = seen
            .iter()
            .find(|r| r.path == "/api/v1/orgs/acme/repos")
            .unwrap();
        let body: Value = serde_json::from_str(&create.body).unwrap();
        assert_eq!(body["private"], true);
        assert_eq!(body["auto_init"], true);
        let rule = seen
            .iter()
            .find(|r| r.path.ends_with("/branch_protections"))
            .unwrap();
        let body: Value = serde_json::from_str(&rule.body).unwrap();
        assert_eq!(body["rule_name"], "main");
        assert_eq!(body["enable_push"], true, "pushes stay open");
        assert_eq!(body["enable_force_push"], false);
        assert_eq!(body["required_approvals"], 0, "no required reviews");
        assert_eq!(body["enable_status_check"], false, "no required checks");
    }

    #[tokio::test]
    async fn repo_creation_refuses_an_unknown_org_and_one_the_token_cannot_create_in() {
        let (base, _) = stub(|req| match req.path.as_str() {
            "/api/v1/user" => (200, r#"{"login":"talaria-bot"}"#.into()),
            "/api/v1/orgs/ghost" => (404, r#"{"message":"not found"}"#.into()),
            "/api/v1/orgs/locked" => (200, "{}".into()),
            "/api/v1/users/talaria-bot/orgs/locked/permissions" => {
                (200, r#"{"can_create_repository":false}"#.into())
            }
            _ => (500, "{}".into()),
        })
        .await;
        let f = conn(&base);
        assert!(
            f.repo_creation_refusal("ghost")
                .await
                .unwrap()
                .unwrap()
                .contains("no organization named ghost")
        );
        assert!(
            f.repo_creation_refusal("locked")
                .await
                .unwrap()
                .unwrap()
                .contains("may not create repositories in locked")
        );
        // The token's own account always may.
        assert_eq!(f.repo_creation_refusal("talaria-bot").await.unwrap(), None);
    }

    #[tokio::test]
    async fn branch_ahead_reads_the_compare() {
        let (base, _) = stub(|req| match req.path.as_str() {
            "/api/v1/repos/acme/notes/compare/main...talaria/tala-7-fix" => {
                (200, r#"{"total_commits":2,"commits":[{},{}]}"#.into())
            }
            _ => (500, "{}".into()),
        })
        .await;
        assert_eq!(
            conn(&base)
                .branch_ahead("acme/notes", "main", "talaria/tala-7-fix")
                .await
                .unwrap(),
            2
        );
    }
}
