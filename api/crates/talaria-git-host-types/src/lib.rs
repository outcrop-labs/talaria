// The git-host interface: what Talaria asks of a place that keeps git
// repositories, and the shapes the answers come in.
//
// OPERATORS CHOOSE THEIR PROVIDERS. Talaria does not assume GitHub: a git host
// is a row an operator adds (talaria-git-hosts), of a kind that has an
// adapter. Each adapter implements [`GitHost`] — GitHub (App, PAT or
// Enterprise Server) in talaria-github, Forgejo and Gitea in talaria-forgejo —
// and every caller (the workbench verbs, the credential route, app install and
// update, the plugin upkeep sweep) talks to the trait, never to a vendor.
//
// WHAT THE INTERFACE COVERS is exactly what those callers need, and nothing a
// caller does not:
//   · repo-scoped credentials for clone, fetch and push (`repo_login`), and
//     whether the host can narrow one per agent (`narrowing`)
//   · the default branch, the branch list, cutting a branch, how far a branch
//     is ahead of its base
//   · creating a repository, with its visibility and default-branch protection
//   · opening a pull request, or keeping the one a branch already has
//   · reading a pull request: its state, its checks and its reviews
//
// TALARIA STOPS AT THE PR. There is no merge, no push to a default branch, no
// re-run and no approval in this trait, and an adapter must not grow one.
//
// The types live here, below every adapter (docs/RUST-MIGRATION.md, "Types
// outrank engines"), so the adapters share them without depending on each
// other and the registry above them can dispatch over all of them.

use std::collections::{BTreeMap, HashMap};
use std::future::Future;

use serde::Serialize;
use serde_json::{Value, json};

/// Which adapter a host row runs on. Forgejo and Gitea share one API.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum HostKind {
    Github,
    Forgejo,
}

impl HostKind {
    pub const ALL: [HostKind; 2] = [HostKind::Github, HostKind::Forgejo];

    pub fn parse(s: &str) -> Option<HostKind> {
        match s {
            "github" => Some(HostKind::Github),
            "forgejo" => Some(HostKind::Forgejo),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            HostKind::Github => "github",
            HostKind::Forgejo => "forgejo",
        }
    }

    /// What an admin reads.
    pub fn label(self) -> &'static str {
        match self {
            HostKind::Github => "GitHub",
            HostKind::Forgejo => "Forgejo / Gitea",
        }
    }
}

/// A login git presents over https: a token is the password, with whatever
/// username the host expects for one.
#[derive(Clone, PartialEq, Eq)]
pub struct Login {
    pub username: String,
    pub password: String,
}

impl std::fmt::Debug for Login {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Login")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

/// What a repo-scoped credential is for. `Read` is a clone's (app install and
/// update): the repository's contents, read, and nothing that writes. `Job`
/// is the credential one agent's sandbox is handed for a repository it was
/// granted: `permissions` is that agent's COMPLETE set on the host, as an
/// admin set it (or the default when none did), and the agent spends it
/// directly — git push, and the host's API. An adapter that can narrow a
/// token narrows it to exactly this, met with what it can give; one that
/// cannot says so through [`GitHost::narrowing`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RepoAccess {
    Read,
    Job {
        agent_id: String,
        permissions: PermissionSet,
    },
}

impl RepoAccess {
    /// The agent a `Job` credential is for. None for a `Read`, which is the
    /// platform's own and no agent's.
    pub fn agent(&self) -> Option<&str> {
        match self {
            RepoAccess::Read => None,
            RepoAccess::Job { agent_id, .. } => Some(agent_id),
        }
    }
}

/// Whether a host can narrow a credential per repository and per agent. A
/// GitHub App mints a token for one repository with exactly the permissions
/// asked; a PAT, or a Forgejo token, is the host's own and carries whatever
/// its owner gave it, so on such a host no per-agent permission is offered
/// and none is claimed: the host's own token decides.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Narrowing {
    PerRepo,
    /// The sentence that says why this host's token cannot be narrowed.
    HostWide(&'static str),
}

impl Narrowing {
    /// `perRepo` or `hostWide`, as a page reads it.
    pub fn wire(self) -> &'static str {
        match self {
            Narrowing::PerRepo => "perRepo",
            Narrowing::HostWide(_) => "hostWide",
        }
    }
}

/// A repo-scoped login a host handed out, and what is known about it: what
/// the credential door records for each one (the `git.credential` audit row),
/// and what a caller that must take it back again needs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuedLogin {
    pub login: Login,
    /// The GitHub App installation the token was minted on. None for every
    /// other credential.
    pub installation: Option<String>,
    /// What the token carries, when the host says (a GitHub App token). None
    /// when it does not: a PAT or a Forgejo token carries whatever its owner
    /// gave it.
    pub carried: Option<PermissionSet>,
    /// What the ask named that the host could not give (on GitHub, what the
    /// App's installation does not hold, and organization permissions GitHub
    /// would not put on a repository token).
    pub shortfall: PermissionSet,
    /// The host's own words for a part of the ask it refused, when it
    /// refused one rather than lacked it (GitHub's message when it would not
    /// mint a repository token carrying organization permissions). None
    /// otherwise.
    pub shortfall_why: Option<String>,
    /// Minted by this call, rather than served from a cache or stored.
    pub minted: bool,
}

impl IssuedLogin {
    /// A login the host keeps no record of: nothing is known about what it
    /// carries, and it was not minted.
    pub fn plain(login: Login) -> IssuedLogin {
        IssuedLogin {
            login,
            installation: None,
            carried: None,
            shortfall: PermissionSet::new(),
            shortfall_why: None,
            minted: false,
        }
    }
}

// ── Permissions ──────────────────────────────────────────────────────────────
//
// A host that can narrow a credential narrows it to named permissions, each at
// a level. The names are the host's own (GitHub's are `contents`, `metadata`,
// `workflows`, …), and which of them exist, at which levels, is the
// adapter's catalog, never this crate's. What lives here is the
// arithmetic every adapter and caller shares: the order of the levels, and the
// set operations a mint is built from.

/// How much of one permission a credential carries: `read` < `write` <
/// `admin`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Read,
    Write,
    Admin,
}

impl Level {
    pub fn parse(s: &str) -> Option<Level> {
        match s {
            "read" => Some(Level::Read),
            "write" => Some(Level::Write),
            "admin" => Some(Level::Admin),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Level::Read => "read",
            Level::Write => "write",
            Level::Admin => "admin",
        }
    }
}

impl std::fmt::Display for Level {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A set of permissions, each at one level. It is ordered by name, so two
/// spellings of one set (the same keys in another order) are one value with
/// one canonical JSON. That is what lets a token cache key on it. It
/// serializes as that JSON object (`{"contents":"write",…}`), the shape hosts
/// answer in and an admin's page reads.
#[derive(Debug, Clone, Default, PartialEq, Eq, Hash, Serialize)]
#[serde(transparent)]
pub struct PermissionSet(BTreeMap<String, Level>);

impl PermissionSet {
    pub fn new() -> PermissionSet {
        PermissionSet::default()
    }

    /// The set the pairs name. A key named twice keeps its higher level.
    pub fn of(pairs: &[(&str, Level)]) -> PermissionSet {
        let mut set = PermissionSet::new();
        for (key, level) in pairs {
            set.raise(key, *level);
        }
        set
    }

    /// Read a `{"key": "level", …}` object, the shape hosts answer in. Any
    /// level this crate does not know is an error rather than a guess: a set
    /// that cannot be read cannot be compared, and a caller deciding what a
    /// credential may carry must treat it as unknown.
    pub fn from_json(v: &Value) -> Result<PermissionSet, String> {
        let obj = v
            .as_object()
            .ok_or_else(|| format!("permissions are not an object: {v}"))?;
        let mut set = PermissionSet::new();
        for (key, level) in obj {
            let level = level.as_str().and_then(Level::parse).ok_or_else(|| {
                format!("permission {key}: level {level} is not read, write or admin")
            })?;
            set.raise(key, level);
        }
        Ok(set)
    }

    /// The canonical JSON object, keys in order.
    pub fn to_json(&self) -> Value {
        let mut obj = serde_json::Map::new();
        for (key, level) in &self.0 {
            obj.insert(key.clone(), Value::String(level.as_str().into()));
        }
        Value::Object(obj)
    }

    /// The canonical JSON text: one string per set, whatever order it was
    /// spelled in.
    pub fn canonical(&self) -> String {
        self.to_json().to_string()
    }

    pub fn get(&self, key: &str) -> Option<Level> {
        self.0.get(key).copied()
    }

    pub fn contains(&self, key: &str) -> bool {
        self.0.contains_key(key)
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn iter(&self) -> impl Iterator<Item = (&str, Level)> {
        self.0.iter().map(|(k, l)| (k.as_str(), *l))
    }

    /// Put `key` in the set at `level`, or keep the level it already has if
    /// that is higher.
    pub fn raise(&mut self, key: &str, level: Level) {
        let at = self.0.entry(key.to_string()).or_insert(level);
        if level > *at {
            *at = level;
        }
    }

    /// The set without the keys `leave_out` names.
    pub fn without(&self, leave_out: impl Fn(&str) -> bool) -> PermissionSet {
        PermissionSet(
            self.0
                .iter()
                .filter(|(k, _)| !leave_out(k.as_str()))
                .map(|(k, l)| (k.clone(), *l))
                .collect(),
        )
    }

    /// Every key either set has, at the higher of its two levels.
    pub fn union(&self, other: &PermissionSet) -> PermissionSet {
        let mut out = self.clone();
        for (key, level) in other.iter() {
            out.raise(key, level);
        }
        out
    }

    /// What both sets allow: the keys they share, each at the lower of its
    /// two levels. An ask met with what a credential's issuer holds is the
    /// most that can be asked for without being refused, and never more than
    /// the ask.
    pub fn meet(&self, other: &PermissionSet) -> PermissionSet {
        PermissionSet(
            self.0
                .iter()
                .filter_map(|(k, l)| other.get(k).map(|o| (k.clone(), (*l).min(o))))
                .collect(),
        )
    }

    /// What this set asks that `held` does not cover: each key `held` lacks,
    /// or holds below the asked level, at the asked level.
    pub fn missing_from(&self, held: &PermissionSet) -> PermissionSet {
        PermissionSet(
            self.0
                .iter()
                .filter(|(k, l)| held.get(k.as_str()).is_none_or(|h| h < **l))
                .map(|(k, l)| (k.clone(), *l))
                .collect(),
        )
    }

    /// Whether this set holds everything `ask` names, each at least as high.
    pub fn covers(&self, ask: &PermissionSet) -> bool {
        ask.missing_from(self).is_empty()
    }
}

impl std::fmt::Display for PermissionSet {
    /// `contents:write, metadata:read`, or `nothing`.
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.0.is_empty() {
            return f.write_str("nothing");
        }
        let mut first = true;
        for (key, level) in &self.0 {
            if !first {
                f.write_str(", ")?;
            }
            first = false;
            write!(f, "{key}:{level}")?;
        }
        Ok(())
    }
}

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

/// A repository a host created.
#[derive(Debug)]
pub struct CreatedRepo {
    /// `owner/name`, as the host spells it.
    pub full_name: String,
    /// Its page.
    pub url: String,
    /// None when protection was not asked for; Some(Err) carries why the host
    /// would not protect the default branch.
    pub protection: Option<Result<(), String>>,
}

/// A pull request by its number and page.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PullRequestRef {
    pub url: String,
    pub number: i64,
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

/// One open pull request, as the list read shows it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenPullRequest {
    pub number: i64,
    pub url: String,
    pub title: String,
    pub head: String,
    pub base: String,
    pub author: String,
    pub draft: bool,
}

/// A host's live answer to "can you reach it": whether it is configured
/// enough to try, who it acts as, and why not. A host whose credential is
/// a registered app that each account accepts on its own terms (a GitHub
/// App) also says what the app asks for and what each installation it uses
/// has accepted; every other host leaves both empty.
#[derive(Debug, Clone, Default, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct HostStatus {
    pub configured: bool,
    pub account: Option<String>,
    pub error: Option<String>,
    /// What the app's registration asks for, read live with the status.
    /// None when the host has no app, or the read failed.
    pub app_permissions: Option<PermissionSet>,
    /// Each installation the connection uses, as the registry last recorded
    /// it (never asked live with the status: that is the holdings refresh's
    /// job, and an admin's Refresh).
    pub installations: Vec<InstallationStatus>,
}

/// One installation of a host's app, as the registry last checked it: what
/// the app asked for then, what the installation had accepted, and what is
/// still waiting for the account's owner to accept.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstallationStatus {
    pub id: String,
    /// The account (an org, or a user) the app is installed on.
    pub account: String,
    /// What the app's registration asked for at the last check.
    pub app_permissions: Option<PermissionSet>,
    /// What the installation had accepted at the last check. None: never
    /// read, which is unknown, never "nothing".
    pub held: Option<PermissionSet>,
    /// What the app asks for that the installation has not accepted: each
    /// key it lacks, or holds below the asked level, at the asked level. An
    /// owner of the account accepts it on the host.
    pub pending: PermissionSet,
    /// `all` or `selected`: which of the account's repositories it covers.
    pub repository_selection: Option<String>,
    /// When the account suspended the installation, if it has.
    pub suspended_at: Option<String>,
    /// When the registry last read it successfully (ISO).
    pub checked_at: Option<String>,
    /// Why the last check failed, when it did. What is above is then the
    /// last good read.
    pub error: Option<String>,
    /// The host's own page for the installation, where an owner reviews and
    /// accepts what is pending.
    pub url: Option<String>,
}

/// The number at the end of a pull request's page URL — GitHub's `…/pull/42`
/// and Forgejo's `…/pulls/42` alike. How a job row written before
/// `pr_number` existed still names its PR.
pub fn pr_number_from_url(url: &str) -> Option<i64> {
    let trimmed = url.trim_end_matches('/');
    let tail = trimmed
        .rsplit_once("/pull/")
        .or_else(|| trimmed.rsplit_once("/pulls/"))
        .map(|(_, t)| t)?;
    tail.parse().ok().filter(|n| *n > 0)
}

// ── The PR status wire ───────────────────────────────────────────────────────
//
// `pull_request_status` answers ONE shape whatever the host, because the
// agent's `pr_status`, `plugin_status` and the upkeep sweep read it without
// knowing where the repo lives:
//
//   { repo, number, url, title, state: open|closed|merged, draft, base, head,
//     headSha, mergeable, mergedAt,
//     checks: { verdict: passing|failing|pending|none|unknown,
//               runs: [{name, status, conclusion, url}],
//               statuses: [{context, state, url}] },
//     reviewDecision: approved|changes_requested|commented|none|unknown,
//     reviews: [{by, state, submittedAt}], requestedReviewers: [login],
//     errors?: [sentence] }
//
// An adapter normalizes its host's records into the two inputs below — check
// runs as `{name, status, conclusion}` (status `completed` once finished) and
// commit statuses as `{context, state: success|failure|error|pending}`;
// reviews as `{user: {login}, state: APPROVED|CHANGES_REQUESTED|COMMENTED|
// DISMISSED|PENDING, submitted_at}` — and the verdicts come from here, so two
// hosts can never disagree about what "failing" means.

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
pub fn checks_verdict(runs: &[Value], statuses: &[Value]) -> &'static str {
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

/// The review decision from a reviews list: each reviewer's latest
/// approving, change-requesting or dismissed review counts (a later plain
/// comment does not undo an approval). Any reviewer still requesting changes
/// → `changes_requested`; else any approval → `approved`; else any review at
/// all → `commented`; else `none`. Answers the decision and each reviewer's
/// latest review, oldest reviewer first.
pub fn review_decision(reviews: &[Value]) -> (&'static str, Vec<Value>) {
    let mut order: Vec<String> = Vec::new();
    let mut decisive: HashMap<String, String> = HashMap::new();
    let mut latest: HashMap<String, Value> = HashMap::new();
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
            json!({
                "by": by,
                "state": state.to_lowercase(),
                "submittedAt": r.get("submitted_at").cloned().unwrap_or(Value::Null),
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

/// The string at `path` in `v`, or null — the wire's way of saying a field a
/// host did not send.
pub fn value_at(v: &Value, path: &[&str]) -> Value {
    let mut cur = v;
    for key in path {
        match cur.get(key) {
            Some(next) => cur = next,
            None => return Value::Null,
        }
    }
    cur.clone()
}

/// The fields of the PR status wire an adapter has read, before the verdicts.
pub struct PrStatusParts<'a> {
    pub repo: &'a str,
    pub number: i64,
    pub url: Value,
    pub title: Value,
    /// open | closed | merged
    pub state: String,
    pub draft: bool,
    pub base: Value,
    pub head: Value,
    pub head_sha: String,
    pub mergeable: Value,
    pub merged_at: Value,
    /// Normalized check runs (`{name, status, conclusion, html_url}`).
    pub runs: Vec<Value>,
    /// Normalized commit statuses (`{context, state, target_url}`).
    pub statuses: Vec<Value>,
    /// Normalized reviews; None when they could not be read.
    pub reviews: Option<Vec<Value>>,
    pub requested_reviewers: Vec<Value>,
    /// What could not be read, each a sentence.
    pub errors: Vec<String>,
}

/// Assemble the PR status wire (see the section header) from what an adapter
/// read. Checks that could not be read and came back empty are `unknown`,
/// never `none` (which says there are no checks).
pub fn pr_status_wire(p: PrStatusParts<'_>) -> Value {
    let checks_unread = p
        .errors
        .iter()
        .any(|e| e.starts_with("check runs") || e.starts_with("commit statuses"));
    let verdict = if checks_unread && p.runs.is_empty() && p.statuses.is_empty() {
        "unknown"
    } else {
        checks_verdict(&p.runs, &p.statuses)
    };
    let (decision, reviews) = match &p.reviews {
        Some(r) => {
            let (d, r) = review_decision(r);
            (d, r)
        }
        None => ("unknown", Vec::new()),
    };
    let run_wire: Vec<Value> = p
        .runs
        .iter()
        .map(|r| {
            json!({
                "name": value_at(r, &["name"]),
                "status": value_at(r, &["status"]),
                "conclusion": value_at(r, &["conclusion"]),
                "url": value_at(r, &["html_url"]),
            })
        })
        .collect();
    let status_wire: Vec<Value> = p
        .statuses
        .iter()
        .map(|s| {
            json!({
                "context": value_at(s, &["context"]),
                "state": value_at(s, &["state"]),
                "url": value_at(s, &["target_url"]),
            })
        })
        .collect();
    let mut out = json!({
        "repo": p.repo,
        "number": p.number,
        "url": p.url,
        "title": p.title,
        "state": p.state,
        "draft": p.draft,
        "base": p.base,
        "head": p.head,
        "headSha": p.head_sha,
        "mergeable": p.mergeable,
        "mergedAt": p.merged_at,
        "checks": { "verdict": verdict, "runs": run_wire, "statuses": status_wire },
        "reviewDecision": decision,
        "reviews": reviews,
        "requestedReviewers": p.requested_reviewers,
    });
    if !p.errors.is_empty() {
        out["errors"] = json!(p.errors);
    }
    out
}

// ── The interface ────────────────────────────────────────────────────────────

/// A git host Talaria works repositories on. `repo` is always the host's own
/// `owner/name`. Every `Err` is a sentence an admin or an agent can act on.
///
/// The futures are `Send` so a caller can hold a host across an await in a
/// spawned task. Dispatch over the registered kinds is the registry's
/// (`talaria_git_hosts::Host`), which is why there is no `dyn` here.
pub trait GitHost: Send + Sync {
    fn kind(&self) -> HostKind;

    /// The https URL git clones `repo` from. It never carries a credential:
    /// git asks Talaria's credential helper for one when it needs it.
    fn clone_url(&self, repo: &str) -> String;

    /// The repository's page.
    fn repo_url(&self, repo: &str) -> String;

    /// Whether the connection is configured and the host accepts it — the
    /// "test connection" an admin presses, and doctor's line.
    fn status(&self) -> impl Future<Output = HostStatus> + Send;

    /// Every repository the connection can reach: the pool an admin grants
    /// agents from.
    fn reachable_repos(&self) -> impl Future<Output = Result<Vec<String>, String>> + Send;

    /// Whether this connection can narrow a credential per repository and
    /// per agent ([`Narrowing`]). From its configuration alone (no network).
    fn narrowing(&self) -> Narrowing;

    /// A login for `repo` alone where the host can scope one (GitHub App),
    /// narrowed to `access` where it can narrow one, with what is known
    /// about it. None when the connection has nothing to give (a public repo
    /// then clones anonymously).
    fn repo_login(
        &self,
        repo: &str,
        access: &RepoAccess,
    ) -> impl Future<Output = Result<Option<IssuedLogin>, String>> + Send;

    fn default_branch(&self, repo: &str) -> impl Future<Output = Result<String, String>> + Send;

    fn list_branches(&self, repo: &str)
    -> impl Future<Output = Result<Vec<String>, String>> + Send;

    /// Cut `branch` from `base`'s head. An existing branch of that name is
    /// left alone and answers `false`.
    fn create_branch(
        &self,
        repo: &str,
        branch: &str,
        base: &str,
    ) -> impl Future<Output = Result<bool, String>> + Send;

    /// How many commits `branch` has that `base` does not.
    fn branch_ahead(
        &self,
        repo: &str,
        base: &str,
        branch: &str,
    ) -> impl Future<Output = Result<i64, String>> + Send;

    /// Why this connection cannot create repositories at all, from its
    /// configuration alone (no network). None when it can try.
    fn repo_creation_blocker(&self) -> Option<String>;

    /// Asked of the host now: why a repository cannot be created under
    /// `owner`, or None when it can. Err means the host did not answer, which
    /// is never a "no".
    fn repo_creation_refusal(
        &self,
        owner: &str,
    ) -> impl Future<Output = Result<Option<String>, String>> + Send;

    /// Create `owner/name`, initialized with a default branch, and — when
    /// `protect` — protect that branch against force-push and deletion and
    /// nothing else (no required reviews, no required checks: how a branch is
    /// reviewed belongs to the repository's people).
    fn create_repo(
        &self,
        owner: &str,
        name: &str,
        description: &str,
        visibility: RepoVisibility,
        protect: bool,
    ) -> impl Future<Output = Result<CreatedRepo, String>> + Send;

    /// The open PR whose head is `branch`, if there is one.
    fn find_open_pull_request(
        &self,
        repo: &str,
        branch: &str,
    ) -> impl Future<Output = Result<Option<PullRequestRef>, String>> + Send;

    /// Open the PR for `ask.head`, or keep the one it already has and leave
    /// `ask.update_note` on it. Never a second PR for one branch.
    fn open_or_update_pull_request(
        &self,
        repo: &str,
        ask: &PullRequestAsk<'_>,
    ) -> impl Future<Output = Result<OpenedPullRequest, String>> + Send;

    /// The repository's open PRs, newest first, at most `limit`.
    fn open_pull_requests(
        &self,
        repo: &str,
        limit: u32,
    ) -> impl Future<Output = Result<Vec<OpenPullRequest>, String>> + Send;

    /// One PR's state, checks and reviews, in the PR status wire. Every call
    /// underneath is a read.
    fn pull_request_status(
        &self,
        repo: &str,
        number: i64,
    ) -> impl Future<Output = Result<Value, String>> + Send;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_pr_number_reads_off_either_hosts_page_url() {
        assert_eq!(
            pr_number_from_url("https://github.com/acme/app/pull/42"),
            Some(42)
        );
        assert_eq!(
            pr_number_from_url("https://github.com/acme/app/pull/42/"),
            Some(42)
        );
        assert_eq!(
            pr_number_from_url("https://git.example.com/acme/app/pulls/7"),
            Some(7)
        );
        assert_eq!(pr_number_from_url("https://github.com/acme/app"), None);
        assert_eq!(pr_number_from_url("https://x/acme/app/pull/0"), None);
        assert_eq!(pr_number_from_url("https://x/acme/app/pull/abc"), None);
    }

    #[test]
    fn the_checks_verdict_fails_on_any_failure_and_waits_on_any_run() {
        let run = |status: &str, conclusion: Option<&str>| json!({ "name": "ci", "status": status, "conclusion": conclusion });
        let status = |state: &str| json!({ "context": "ci", "state": state });
        assert_eq!(checks_verdict(&[], &[]), "none");
        assert_eq!(
            checks_verdict(&[run("completed", Some("success"))], &[]),
            "passing"
        );
        assert_eq!(
            checks_verdict(
                &[run("completed", Some("success")), run("in_progress", None)],
                &[]
            ),
            "pending"
        );
        assert_eq!(
            checks_verdict(
                &[run("in_progress", None), run("completed", Some("failure"))],
                &[]
            ),
            "failing"
        );
        assert_eq!(checks_verdict(&[], &[status("pending")]), "pending");
        assert_eq!(checks_verdict(&[], &[status("error")]), "failing");
        assert_eq!(checks_verdict(&[], &[status("success")]), "passing");
    }

    #[test]
    fn the_review_decision_follows_each_reviewers_latest_verdict() {
        let review = |by: &str, state: &str| json!({ "user": { "login": by }, "state": state });
        assert_eq!(review_decision(&[]).0, "none");
        assert_eq!(
            review_decision(&[review("ann", "COMMENTED")]).0,
            "commented"
        );
        assert_eq!(
            review_decision(&[
                review("ann", "CHANGES_REQUESTED"),
                review("ann", "APPROVED")
            ])
            .0,
            "approved"
        );
        assert_eq!(
            review_decision(&[
                review("ann", "APPROVED"),
                review("bob", "CHANGES_REQUESTED")
            ])
            .0,
            "changes_requested"
        );
        // A later comment does not undo an approval.
        assert_eq!(
            review_decision(&[review("ann", "APPROVED"), review("ann", "COMMENTED")]).0,
            "approved"
        );
        // Pending reviews are drafts nobody submitted.
        assert_eq!(review_decision(&[review("ann", "PENDING")]).0, "none");
    }

    #[test]
    fn unread_checks_are_unknown_never_none() {
        let wire = pr_status_wire(PrStatusParts {
            repo: "acme/app",
            number: 3,
            url: json!("u"),
            title: json!("t"),
            state: "open".into(),
            draft: false,
            base: json!("main"),
            head: json!("feat"),
            head_sha: "abc".into(),
            mergeable: Value::Null,
            merged_at: Value::Null,
            runs: vec![],
            statuses: vec![],
            reviews: None,
            requested_reviewers: vec![],
            errors: vec!["commit statuses unreadable: 403".into()],
        });
        assert_eq!(wire["checks"]["verdict"], "unknown");
        assert_eq!(wire["reviewDecision"], "unknown");
        assert_eq!(wire["errors"][0], "commit statuses unreadable: 403");
    }

    #[test]
    fn a_permission_set_is_one_value_whatever_order_it_was_spelled_in() {
        let a = PermissionSet::from_json(
            &serde_json::from_str::<Value>(r#"{"metadata":"read","contents":"write"}"#).unwrap(),
        )
        .unwrap();
        let b = PermissionSet::of(&[("contents", Level::Write), ("metadata", Level::Read)]);
        assert_eq!(a, b);
        assert_eq!(a.canonical(), b.canonical());
        assert_eq!(a.canonical(), r#"{"contents":"write","metadata":"read"}"#);
        assert_eq!(a.to_string(), "contents:write, metadata:read");
        assert_eq!(PermissionSet::new().to_string(), "nothing");
        // It serializes as that same canonical object, never as a list.
        assert_eq!(serde_json::to_value(&a).unwrap(), a.to_json());
        // A key named twice keeps its higher level.
        assert_eq!(
            PermissionSet::of(&[("contents", Level::Write), ("contents", Level::Read)])
                .get("contents"),
            Some(Level::Write)
        );
    }

    #[test]
    fn a_level_no_one_knows_is_an_error_never_a_guess() {
        let read = |s: &str| PermissionSet::from_json(&serde_json::from_str::<Value>(s).unwrap());
        assert!(read(r#"{"contents":"maintain"}"#).is_err());
        assert!(read(r#"{"contents":1}"#).is_err());
        assert!(read(r#"["contents"]"#).is_err());
        assert_eq!(read("{}").unwrap(), PermissionSet::new());
        assert!(Level::Read < Level::Write && Level::Write < Level::Admin);
        assert_eq!(Level::parse("admin"), Some(Level::Admin));
        assert_eq!(Level::parse("none"), None);
    }

    #[test]
    fn meet_never_rises_above_the_ask_and_missing_from_names_the_shortfall() {
        let ask = PermissionSet::of(&[
            ("contents", Level::Write),
            ("pull_requests", Level::Write),
            ("metadata", Level::Read),
        ]);
        let held = PermissionSet::of(&[
            ("contents", Level::Read),
            ("metadata", Level::Read),
            ("administration", Level::Write),
        ]);
        // The shared keys at the lower level: contents comes down to read,
        // and administration, held but never asked, is not in it.
        assert_eq!(
            ask.meet(&held),
            PermissionSet::of(&[("contents", Level::Read), ("metadata", Level::Read)])
        );
        // What the ask wants that held does not give, at the asked level.
        assert_eq!(
            ask.missing_from(&held),
            PermissionSet::of(&[("contents", Level::Write), ("pull_requests", Level::Write)])
        );
        assert!(!held.covers(&ask));
        assert!(held.union(&ask).covers(&ask));
        assert_eq!(
            held.union(&ask).get("contents"),
            Some(Level::Write),
            "a union keeps the higher level"
        );
        assert_eq!(
            ask.without(|k| k == "pull_requests"),
            PermissionSet::of(&[("contents", Level::Write), ("metadata", Level::Read)])
        );
    }

    #[test]
    fn a_login_never_prints_its_password() {
        let l = Login {
            username: "bot".into(),
            password: "s3cret".into(),
        };
        assert!(!format!("{l:?}").contains("s3cret"));
        let issued = IssuedLogin::plain(l);
        assert!(!format!("{issued:?}").contains("s3cret"));
        assert_eq!((issued.carried, issued.minted), (None, false));
    }

    #[test]
    fn a_job_credential_names_its_agent_and_a_read_names_none() {
        let job = RepoAccess::Job {
            agent_id: "a-1".into(),
            permissions: PermissionSet::of(&[("workflows", Level::Write)]),
        };
        assert_eq!(job.agent(), Some("a-1"));
        assert_eq!(RepoAccess::Read.agent(), None);
        assert_eq!(Narrowing::PerRepo.wire(), "perRepo");
        assert_eq!(Narrowing::HostWide("why").wire(), "hostWide");
    }

    /// The status wire an admin's page reads: a host with no app says so
    /// with a null and an empty list, never by leaving the keys out.
    #[test]
    fn a_status_carries_the_apps_ask_and_each_installation_in_camel_case() {
        let bare = serde_json::to_value(HostStatus::default()).unwrap();
        assert_eq!(bare["appPermissions"], Value::Null);
        assert_eq!(bare["installations"], json!([]));
        let st = HostStatus {
            configured: true,
            account: Some("Talaria".into()),
            error: None,
            app_permissions: Some(PermissionSet::of(&[("contents", Level::Write)])),
            installations: vec![InstallationStatus {
                id: "91".into(),
                account: "acme".into(),
                held: Some(PermissionSet::new()),
                pending: PermissionSet::of(&[("contents", Level::Write)]),
                checked_at: Some("2026-11-16T00:00:00.000Z".into()),
                ..InstallationStatus::default()
            }],
        };
        let wire = serde_json::to_value(&st).unwrap();
        assert_eq!(wire["appPermissions"], json!({ "contents": "write" }));
        let inst = &wire["installations"][0];
        assert_eq!(inst["id"], "91");
        assert_eq!(inst["held"], json!({}));
        assert_eq!(inst["pending"], json!({ "contents": "write" }));
        assert_eq!(inst["checkedAt"], "2026-11-16T00:00:00.000Z");
        for key in [
            "appPermissions",
            "repositorySelection",
            "suspendedAt",
            "error",
            "url",
        ] {
            assert_eq!(inst[key], Value::Null, "{key}");
        }
    }
}
