// An agent asks for a GitHub permission, and an admin decides:
// `workbench_permission_requests`, one row per (agent, host, permission)
// while it is open.
//
// WHY A REQUEST AND NOT A REPORT. A GitHub 403 naming a permission used to
// reach a person as a Helpdesk ticket (`report_problem`), with no decision
// state: nobody could tell an agent "yes", and the agent could not tell when
// to try again. An agent now asks with `request_git_permission`; the request
// is an approval (`git_permission` in talaria-approvals' census), an admin
// approves or denies it in the agent's editor in Agent policy → GitHub access,
// and the agent reads the outcome in `list_repos` and on the ticket.
//
// WHAT IS REFUSED UP FRONT, and nothing else: a key or level GitHub's App
// catalog does not know (talaria-github's `CATALOG`), a repo the agent is not
// granted, and a host whose own token decides (a PAT, Forgejo:
// `Narrowing::HostWide`). There is no cooldown, no cap on open requests, and
// no key the agent may not ask for: what an agent may carry is an admin's
// call, per agent, and this is how the agent asks.
//
// ONE OPEN REQUEST per (agent, host, permission), kept by the partial unique
// index `workbench_permission_requests_one_open`: a second raise appends its
// `why` and its evidence to the open row (and raises its level when it asks
// for more), so an admin decides once. Nothing is refused for asking again:
// `why` keeps its newest WHY_MAX characters and `evidence` its newest
// EVIDENCE_MAX entries, so a long run of raises costs nothing but its oldest
// words. The row keeps the ticket it was first asked from, and that ticket
// gets the decision's line. `repo` is where the need arose; an approval
// merges the key into the agent's set on the HOST (crate::permissions), so it
// applies to every repo the agent holds there.
//
// AN APPROVAL IS OF WHAT THE ADMIN SAW. A raise can lift an open request's
// level while an admin reads it, so the decide route posts the level the
// admin saw, and a request whose level moved since is a conflict
// ([`DecideError::Changed`]), never a wider grant than the one confirmed.
//
// THE STATES.
//   pending          an admin has not decided
//   awaiting_github  approved, or already in the agent's set, and the
//                    installation that mints the repo's token does not hold
//                    it yet: an org owner accepts it on GitHub. The holdings
//                    refresh flips it to granted when it does (settle_awaiting)
//   granted          the agent's set carries it and the installation holds it
//   denied           an admin said no
//   withdrawn        the agent lost its last grant on the host, an admin
//                    took the permission out of its set before GitHub held
//                    it, or an admin closed one waiting on GitHub (its note
//                    says the set still carries it)
//
// WHO WRITES WHAT. This module writes the rows and their audit rows. The
// callers say it to people: the verb (talaria-workbench-mcp) writes the
// ticket line and announces the approval; the decide route
// (talaria-routes-workbench) writes the decision's ticket line and notice;
// and a request the refresh settles is told through [`SETTLED`], which the
// routes wire at boot. Unwired, the row and its audit row still land.

use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, OnceLock};

use serde_json::{Value, json};
use sqlx::PgPool;

use talaria_agent_auth::{epoch_ms_to_iso, now_ms};
use talaria_audit::{AuditEntry, log_audit, log_audit_in};
use talaria_git_host_types::{Level, Narrowing, PermissionSet};

use crate::holdings::{InstallationRow, installation_rows, tracked_installations};
use crate::permissions::{self, installation_for_repo};
use crate::registry::{HostRow, get_host};
use crate::{narrowing_of, resolve_grant};

/// The two open states: what the census lists and the unique index keeps to
/// one per (agent, host, permission).
pub const OPEN: [&str; 2] = ["pending", "awaiting_github"];

/// How long `why` may grow across raises, in characters: past it, the
/// oldest words go.
const WHY_MAX: usize = 4000;

/// How many raises' evidence a request keeps: past it, the oldest go.
const EVIDENCE_MAX: usize = 20;

/// The decision note of a request an admin closed while it waited on GitHub:
/// what the agent reads in list_repos, since its set still carries it.
pub const CLOSED_WAITING: &str = "closed while waiting on GitHub: the agent's permissions on this host still include it, and a git credential carries it once the installation accepts it";

/// One raise's own `why`, in characters.
const WHY_ONE_MAX: usize = 1000;

/// One request as the census, the decide route and the agent read it.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow)]
pub struct PermissionRequest {
    pub id: String,
    pub agent_id: String,
    pub agent_model: String,
    /// The agent's display name, when it still has one.
    pub agent_name: Option<String>,
    pub host_id: String,
    pub host_name: Option<String>,
    /// Where the need arose.
    pub repo: String,
    pub permission: String,
    pub level: String,
    /// The agent's words, every raise's, appended. Content: shown only to
    /// those who may decide it.
    pub why: String,
    /// One entry per raise: `{source, verified, repo, taskId, at}`.
    pub evidence: Value,
    pub task_id: Option<String>,
    pub ticket_ref: Option<String>,
    pub ticket_title: Option<String>,
    pub board_id: Option<String>,
    /// The ticket's column key.
    pub ticket_status: Option<String>,
    pub status: String,
    pub decided_by: Option<String>,
    pub decided_by_name: Option<String>,
    pub decision_note: Option<String>,
    pub created_ms: i64,
    pub updated_ms: i64,
    pub decided_ms: Option<i64>,
}

impl PermissionRequest {
    /// The approval's key in the census (`git_permission:<id>`).
    pub fn approval_key(&self) -> String {
        approval_key(&self.id)
    }

    /// `workflows write`, the way every line about it says it.
    pub fn asked(&self) -> String {
        format!("{} {}", self.permission, self.level)
    }

    /// The agent's name for a line, its model when it has none.
    pub fn agent_label(&self) -> &str {
        self.agent_name.as_deref().unwrap_or(&self.agent_model)
    }

    /// The host's name for a line, its id when it is gone.
    pub fn host_label(&self) -> &str {
        self.host_name.as_deref().unwrap_or(&self.host_id)
    }

    /// Whether it is still open (pending, or waiting on GitHub).
    pub fn is_open(&self) -> bool {
        OPEN.contains(&self.status.as_str())
    }
}

/// The census key of request `id`.
pub fn approval_key(id: &str) -> String {
    format!("git_permission:{id}")
}

const SELECT: &str = "select r.id::text as id, r.agent_id::text as agent_id, r.agent_model, \
        d.display_name as agent_name, r.host_id, h.name as host_name, r.repo, r.permission, \
        r.level, r.why, coalesce(r.evidence, '[]'::jsonb) as evidence, \
        r.task_id::text as task_id, \
        case when t.ticket_no is not null then coalesce(b.ticket_prefix, 'TASK') || '-' || t.ticket_no end as ticket_ref, \
        t.title as ticket_title, t.board_id::text as board_id, t.status as ticket_status, \
        r.status, r.decided_by::text as decided_by, \
        coalesce(nullif(u.name, ''), u.email) as decided_by_name, r.decision_note, \
        (trunc(extract(epoch from r.created_at) * 1000))::bigint as created_ms, \
        (trunc(extract(epoch from r.updated_at) * 1000))::bigint as updated_ms, \
        (trunc(extract(epoch from r.decided_at) * 1000))::bigint as decided_ms \
     from workbench_permission_requests r \
     left join agent_defs d on d.id = r.agent_id \
     left join git_hosts h on h.id = r.host_id \
     left join tasks t on t.id = r.task_id \
     left join boards b on b.id = t.board_id \
     left join users u on u.id = r.decided_by";

fn read_failed(e: sqlx::Error) -> String {
    format!("permission requests read: {e}")
}

fn write_failed(e: sqlx::Error) -> String {
    format!("permission request write: {e}")
}

/// One request, whatever its state.
pub async fn get_request(pg: &PgPool, id: &str) -> Result<Option<PermissionRequest>, String> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} where r.id = $1::uuid"
    )))
    .bind(id)
    .fetch_optional(pg)
    .await
    .map_err(read_failed)
}

/// Every open request, oldest first: the admin's queue.
pub async fn open_requests(pg: &PgPool) -> Result<Vec<PermissionRequest>, String> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} where r.status in ('pending', 'awaiting_github') order by r.created_at"
    )))
    .fetch_all(pg)
    .await
    .map_err(read_failed)
}

/// `agent_id`'s requests as `list_repos` shows the agent: every open one,
/// however old (an owner can take weeks to accept one on GitHub), and every
/// one decided in the last `days` days. Open first, then newest.
pub async fn agent_requests(
    pg: &PgPool,
    agent_id: &str,
    days: i32,
) -> Result<Vec<PermissionRequest>, String> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} where r.agent_id = $1::uuid \
           and (r.status in ('pending', 'awaiting_github') \
                or coalesce(r.decided_at, r.updated_at) > now() - make_interval(days => $2)) \
         order by r.status in ('pending', 'awaiting_github') desc, r.updated_at desc limit 30"
    )))
    .bind(agent_id)
    .bind(days)
    .fetch_all(pg)
    .await
    .map_err(read_failed)
}

// ── Asking ───────────────────────────────────────────────────────────────────

/// What an agent asked for, as the verb read its arguments.
pub struct Ask<'a> {
    pub agent_id: &'a str,
    pub agent_model: &'a str,
    /// `owner/name`, one of the agent's grants.
    pub repo: &'a str,
    /// The host, when the repo is granted on more than one.
    pub host: Option<&'a str>,
    /// Each `(name, level)` as the agent spelled it.
    pub permissions: &'a [(String, String)],
    pub why: &'a str,
    /// The ticket it was working, already authorised by the verb's ticket
    /// door.
    pub task_id: Option<&'a str>,
}

/// What became of one permission an agent asked for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Raised {
    /// A new request, `pending` or `awaiting_github`.
    Filed { id: String, status: String },
    /// One was open already: this raise's words and evidence were added to
    /// it.
    AlreadyOpen { id: String, status: String },
    /// Its set on the host carries it and the installation holds it: the
    /// next credential for the repo carries it, and there is nothing to ask.
    Carried,
}

/// The ticket a request is attached to: the one it was first asked from,
/// where its decision's line lands.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RequestTicket {
    pub id: String,
    /// `WEB-12`, when the ticket has a number.
    pub ticket_ref: Option<String>,
}

impl RequestTicket {
    /// The ticket's name in a sentence.
    pub fn label(&self) -> &str {
        self.ticket_ref.as_deref().unwrap_or("its ticket")
    }
}

/// One permission of a filing, and what became of it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RaisedPermission {
    pub permission: String,
    pub level: Level,
    pub raised: Raised,
    /// The ticket the request is attached to (None: carried, or asked from
    /// no ticket). A raise from another ticket leaves it where it was.
    pub ticket: Option<RequestTicket>,
}

/// What a filing did, per permission, on the host the repo's grant names.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Filing {
    pub host_id: String,
    pub host_name: String,
    /// The repo as granted (its case).
    pub repo: String,
    pub permissions: Vec<RaisedPermission>,
}

/// The permissions an agent asked for, each a key and level GitHub's App
/// catalog knows; a key named twice keeps its higher level. Err is the
/// sentence the agent reads.
pub fn read_asks(raw: &[(String, String)]) -> Result<PermissionSet, String> {
    if raw.is_empty() {
        return Err(
            "permissions is required: each {name, level} you need, like [{\"name\": \"workflows\", \"level\": \"write\"}]"
                .into(),
        );
    }
    let mut set = PermissionSet::new();
    for (name, level) in raw {
        let name = name.trim();
        let Some(l) = Level::parse(level.trim()) else {
            return Err(format!(
                "permission {name}: level \"{}\" is not read, write or admin",
                level.trim()
            ));
        };
        talaria_github::known(name, l)
            .map_err(|e| format!("{e} — use GitHub's App permission names, like contents, pull_requests, workflows or secrets"))?;
        set.raise(name, l);
    }
    Ok(set)
}

/// What the record says the installation that mints `repo`'s token is, on
/// `row`: None when no installation is selected for it, or nothing was
/// ever read.
async fn minting_installation(pg: &PgPool, row: &HostRow, repo: &str) -> Option<InstallationRow> {
    let selected = tracked_installations(row);
    let rows = installation_rows(pg, &row.id).await.unwrap_or_else(|e| {
        tracing::warn!("[git-hosts] {e}");
        Vec::new()
    });
    installation_for_repo(&selected, &rows, repo).cloned()
}

/// Whether `held` carries `key` at `level` or higher.
fn holds(held: &PermissionSet, key: &str, level: Level) -> bool {
    held.get(key).is_some_and(|h| h >= level)
}

/// What GitHub's side says about a request, from the record and never from
/// GitHub: the installation that mints its repo's token, its settings page
/// (where an owner of the account accepts a permission), and whether it holds
/// the permission (None: no installation selected for the repo, or never
/// read).
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct GithubSide {
    pub installation: Option<String>,
    pub settings_url: Option<String>,
    pub held: Option<bool>,
}

/// [`GithubSide`] for `req`.
pub async fn github_side(pg: &PgPool, req: &PermissionRequest) -> GithubSide {
    let Ok(Some(row)) = get_host(pg, &req.host_id).await else {
        return GithubSide::default();
    };
    let Some(on) = minting_installation(pg, &row, &req.repo).await else {
        return GithubSide::default();
    };
    let held = match (on.held.as_ref(), Level::parse(&req.level)) {
        (Some(held), Some(level)) => Some(holds(held, &req.permission, level)),
        _ => None,
    };
    GithubSide {
        installation: Some(on.installation_id),
        settings_url: on.html_url,
        held,
    }
}

/// THE ONE FILING: an agent asks for permissions on the host one of its
/// repos lives on. Refused (Err, the sentence the agent reads) only for a key
/// or level GitHub does not know, a repo the agent is not granted, or a host
/// whose own token decides. Otherwise each permission is filed — `pending`,
/// or `awaiting_github` when the agent's set already carries it and only
/// GitHub lacks it — or appended to the request already open for it, or
/// answered as carried. The ticket line and the announcement are the
/// caller's.
pub async fn file_git_permission_request(pg: &PgPool, ask: &Ask<'_>) -> Result<Filing, String> {
    let asks = read_asks(ask.permissions)?;
    let grant = resolve_grant(pg, ask.agent_id, ask.repo, ask.host)
        .await
        .map_err(|e| format!("{e}. A permission is asked for on a repo you work"))?;
    let row = get_host(pg, &grant.host_id)
        .await?
        .ok_or_else(|| format!("the git host \"{}\" is gone", grant.host_id))?;
    if let Narrowing::HostWide(why) = narrowing_of(&row) {
        return Err(format!(
            "{} gives no per-agent permissions: its own token decides what it may do ({why}), and its operator changes it — there is nothing to ask for here",
            row.name
        ));
    }
    let set = permissions::host_permissions(pg, ask.agent_id, &row.id).await?;
    let held = minting_installation(pg, &row, &grant.repo)
        .await
        .and_then(|r| r.held);
    let why: String = ask.why.trim().chars().take(WHY_ONE_MAX).collect();
    let evidence = json!([{
        "source": "agent",
        "verified": false,
        "repo": grant.repo,
        "taskId": ask.task_id,
        "at": epoch_ms_to_iso(now_ms()),
    }]);
    let mut out = Vec::new();
    for (key, level) in asks.iter() {
        let carried = holds(&set, key, level);
        let (raised, ticket) = match (&held, carried) {
            (Some(held), true) if holds(held, key, level) => (Raised::Carried, None),
            (_, true) => {
                upsert(
                    pg,
                    ask,
                    &row.id,
                    &grant.repo,
                    key,
                    level,
                    &why,
                    &evidence,
                    "awaiting_github",
                )
                .await?
            }
            (_, false) => {
                upsert(
                    pg,
                    ask,
                    &row.id,
                    &grant.repo,
                    key,
                    level,
                    &why,
                    &evidence,
                    "pending",
                )
                .await?
            }
        };
        out.push(RaisedPermission {
            permission: key.to_string(),
            level,
            raised,
            ticket,
        });
    }
    Ok(Filing {
        host_id: row.id.clone(),
        host_name: row.name.clone(),
        repo: grant.repo,
        permissions: out,
    })
}

/// File one permission, or add this raise to the one open for it. One
/// statement: two raises at once are one request. An open request's status
/// follows the raise when it asks for more, and goes back to `pending` when
/// it waited on GitHub and the agent's set no longer carries it (an admin
/// took it out since): `awaiting_github` would tell the agent its set
/// includes it. Answers the ticket the request is attached to as well.
#[allow(clippy::too_many_arguments)]
async fn upsert(
    pg: &PgPool,
    ask: &Ask<'_>,
    host_id: &str,
    repo: &str,
    key: &str,
    level: Level,
    why: &str,
    evidence: &Value,
    status: &str,
) -> Result<(Raised, Option<RequestTicket>), String> {
    let (id, status, inserted, task_id, ticket_ref): (
        String,
        String,
        bool,
        Option<String>,
        Option<String>,
    ) = sqlx::query_as(
        "insert into workbench_permission_requests as r \
           (agent_id, agent_model, host_id, repo, permission, level, why, evidence, task_id, status) \
         values ($1::uuid, $2, $3, $4, $5, $6, $7, $8, $9::uuid, $10) \
         on conflict (agent_id, host_id, permission) where status in ('pending', 'awaiting_github') \
         do update set \
           why = right(case when excluded.why = '' then r.why \
                            when r.why = '' then excluded.why \
                            else r.why || E'\\n\\n' || excluded.why end, $11), \
           evidence = jsonb_path_query_array( \
             coalesce(r.evidence, '[]'::jsonb) || coalesce(excluded.evidence, '[]'::jsonb), \
             $12::jsonpath), \
           task_id = coalesce(r.task_id, excluded.task_id), \
           status = case when array_position(array['read','write','admin'], excluded.level) \
                            > array_position(array['read','write','admin'], r.level) \
                         then excluded.status \
                         when r.status = 'awaiting_github' and excluded.status = 'pending' \
                         then 'pending' \
                         else r.status end, \
           level = case when array_position(array['read','write','admin'], excluded.level) \
                           > array_position(array['read','write','admin'], r.level) \
                        then excluded.level else r.level end, \
           updated_at = now() \
         returning id::text, status, (xmax = 0), task_id::text, \
           (select coalesce(b.ticket_prefix, 'TASK') || '-' || t.ticket_no from tasks t \
              left join boards b on b.id = t.board_id \
             where t.id = r.task_id and t.ticket_no is not null)",
    )
    .bind(ask.agent_id)
    .bind(ask.agent_model)
    .bind(host_id)
    .bind(repo)
    .bind(key)
    .bind(level.as_str())
    .bind(why)
    .bind(evidence)
    .bind(ask.task_id)
    .bind(status)
    .bind(WHY_MAX as i32)
    .bind(format!("$[last-{} to last]", EVIDENCE_MAX - 1))
    .fetch_one(pg)
    .await
    .map_err(write_failed)?;
    let ticket = task_id.map(|id| RequestTicket { id, ticket_ref });
    let raised = if inserted {
        Raised::Filed { id, status }
    } else {
        Raised::AlreadyOpen { id, status }
    };
    Ok((raised, ticket))
}

// ── Deciding ─────────────────────────────────────────────────────────────────

/// Why a decision did not land.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecideError {
    /// No such request, or it is not open (to approve: not pending).
    NotOpen,
    /// The agent holds no repo on the host any more: there is nothing for
    /// the permission to apply to.
    GrantGone,
    /// The agent asked again since the admin read it, and the request now
    /// stands at this level, not the one the admin saw.
    Changed(String),
    /// The database failed.
    Failed(String),
}

impl From<String> for DecideError {
    fn from(e: String) -> Self {
        DecideError::Failed(e)
    }
}

/// A decision that landed: the request as it stands now, and for an
/// approval the agent's set on the host after it and every repo it applies
/// to.
#[derive(Debug, Clone, PartialEq)]
pub struct Decided {
    pub request: PermissionRequest,
    pub permissions: Option<PermissionSet>,
    pub repos: Vec<String>,
}

/// Who decided.
pub struct Decider<'a> {
    /// The audit row's actor (talaria-session's `actor_of`).
    pub actor: &'a str,
    pub user_id: &'a str,
}

/// APPROVE, in one transaction: the agent's grants on the host are locked
/// (a removal waits, or went first and this answers GrantGone), the request
/// must still be pending AT `seen`, the level the admin was shown (a raise
/// since is [`DecideError::Changed`]), the key is merged into the agent's
/// set on the host at that level (from the default when it had none), the
/// request becomes `granted` when the installation that mints the repo's
/// token holds it and `awaiting_github` when it does not, and the audit row
/// is written with it. The ticket line, the notice and the census are the
/// caller's, after.
pub async fn approve(
    pg: &PgPool,
    id: &str,
    by: &Decider<'_>,
    seen: Level,
    note: Option<&str>,
) -> Result<Decided, DecideError> {
    let Some(req) = get_request(pg, id).await? else {
        return Err(DecideError::NotOpen);
    };
    if req.status != "pending" {
        return Err(DecideError::NotOpen);
    }
    let held = match get_host(pg, &req.host_id).await? {
        Some(row) => minting_installation(pg, &row, &req.repo)
            .await
            .and_then(|r| r.held),
        None => None,
    };
    let mut tx = pg.begin().await.map_err(write_failed)?;
    // The grants first, then the request: a grant removal deletes the
    // grants and then withdraws the request, in that order too, so the two
    // never wait on each other.
    let repos: Vec<String> = sqlx::query_scalar(
        "select repo from workbench_repos where agent_id = $1::uuid and host_id = $2 \
         order by repo for share",
    )
    .bind(&req.agent_id)
    .bind(&req.host_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(read_failed)?;
    if repos.is_empty() {
        return Err(DecideError::GrantGone);
    }
    // Under the row's lock, a raise waits for this transaction or went
    // first, and the level merged is the one locked, never one read before.
    let still: Option<(String, String)> = sqlx::query_as(
        "select status, level from workbench_permission_requests where id = $1::uuid for update",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(read_failed)?;
    let Some((_, now)) = still.filter(|(s, _)| s == "pending") else {
        return Err(DecideError::NotOpen);
    };
    if now != seen.as_str() {
        return Err(DecideError::Changed(now));
    }
    let level = seen;
    let status = if held
        .as_ref()
        .is_some_and(|h| holds(h, &req.permission, level))
    {
        "granted"
    } else {
        "awaiting_github"
    };
    let after = permissions::merge_agent_host_permission(
        &mut tx,
        &req.agent_id,
        &req.host_id,
        &req.permission,
        level,
        Some(by.user_id),
    )
    .await?;
    sqlx::query(
        "update workbench_permission_requests set status = $2, decided_by = $3::uuid, \
           decision_note = $4, decided_at = now(), updated_at = now() \
         where id = $1::uuid",
    )
    .bind(id)
    .bind(status)
    .bind(by.user_id)
    .bind(note)
    .execute(&mut *tx)
    .await
    .map_err(write_failed)?;
    log_audit_in(
        &mut tx,
        AuditEntry {
            actor: by.actor,
            action: "workbench.github_permission_grant",
            target_type: "agent",
            target_id: Some(&req.agent_id),
            target_label: Some(req.agent_label()),
            before: None,
            after: Some(json!({
                "request": id,
                "host": req.host_id,
                "repo": req.repo,
                "permission": req.permission,
                "level": level.as_str(),
                "status": status,
                "permissions": after.to_json(),
                "appliesTo": repos,
                "note": note,
            })),
        },
    )
    .await
    .map_err(write_failed)?;
    tx.commit().await.map_err(write_failed)?;
    let request = get_request(pg, id).await?.ok_or(DecideError::NotOpen)?;
    Ok(Decided {
        request,
        permissions: Some(after),
        repos,
    })
}

/// DENY a pending request: `denied`, with the admin's note. On a request
/// waiting on GitHub it CLOSES it instead: `withdrawn`, and its note says
/// the agent's set still carries the permission (what is in the set stays
/// there; an admin takes it out in the agent's editor), since `denied` would
/// tell the agent it does not. `seen`, when given, is the level the admin
/// was shown, and a raise since is [`DecideError::Changed`]. The audit row
/// is written with it.
pub async fn deny(
    pg: &PgPool,
    id: &str,
    by: &Decider<'_>,
    seen: Option<Level>,
    note: Option<&str>,
) -> Result<Decided, DecideError> {
    let mut tx = pg.begin().await.map_err(write_failed)?;
    let was: Option<(String, String)> = sqlx::query_as(
        "select status, level from workbench_permission_requests where id = $1::uuid for update",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(read_failed)?;
    let Some((was, now)) = was.filter(|(s, _)| OPEN.contains(&s.as_str())) else {
        return Err(DecideError::NotOpen);
    };
    if seen.is_some_and(|l| l.as_str() != now) {
        return Err(DecideError::Changed(now));
    }
    let (status, stored) = if was == "awaiting_github" {
        let stored = match note {
            Some(n) => format!("{CLOSED_WAITING}: {n}"),
            None => CLOSED_WAITING.to_string(),
        };
        ("withdrawn", Some(stored))
    } else {
        ("denied", note.map(str::to_string))
    };
    sqlx::query(
        "update workbench_permission_requests set status = $2, decided_by = $3::uuid, \
           decision_note = $4, decided_at = now(), updated_at = now() \
         where id = $1::uuid",
    )
    .bind(id)
    .bind(status)
    .bind(by.user_id)
    .bind(stored)
    .execute(&mut *tx)
    .await
    .map_err(write_failed)?;
    let req = sqlx::query_as::<_, PermissionRequest>(sqlx::AssertSqlSafe(format!(
        "{SELECT} where r.id = $1::uuid"
    )))
    .bind(id)
    .fetch_one(&mut *tx)
    .await
    .map_err(read_failed)?;
    log_audit_in(
        &mut tx,
        AuditEntry {
            actor: by.actor,
            action: "workbench.github_permission_deny",
            target_type: "agent",
            target_id: Some(&req.agent_id),
            target_label: Some(req.agent_label()),
            before: Some(json!({ "status": was })),
            after: Some(json!({
                "request": id,
                "host": req.host_id,
                "repo": req.repo,
                "permission": req.permission,
                "level": req.level,
                "status": status,
                "note": note,
            })),
        },
    )
    .await
    .map_err(write_failed)?;
    tx.commit().await.map_err(write_failed)?;
    Ok(Decided {
        request: req,
        permissions: None,
        repos: Vec::new(),
    })
}

/// In a grant write's transaction, after its rows changed: withdraw every
/// open request of `agent_id` on the hosts in `hosts` where it holds no repo
/// any more. A request on a host where the agent keeps another grant stays
/// open: the permission is the host's, not the removed repo's. Answers the
/// ids withdrawn, for the caller to settle their approvals.
pub async fn withdraw_without_grants(
    tx: &mut sqlx::PgConnection,
    agent_id: &str,
    hosts: &[String],
) -> Result<Vec<String>, String> {
    if hosts.is_empty() {
        return Ok(Vec::new());
    }
    sqlx::query_scalar(
        "update workbench_permission_requests r set status = 'withdrawn', updated_at = now(), \
           decision_note = 'the agent no longer holds a repo on this host' \
         where r.agent_id = $1::uuid and r.status in ('pending', 'awaiting_github') \
           and r.host_id = any($2::text[]) \
           and not exists (select 1 from workbench_repos w \
                           where w.agent_id = r.agent_id and w.host_id = r.host_id) \
         returning r.id::text",
    )
    .bind(agent_id)
    .bind(hosts)
    .fetch_all(tx)
    .await
    .map_err(write_failed)
}

// ── Settling what waits on GitHub ────────────────────────────────────────────

/// Where a request the refresh settled is said to people: its ticket line,
/// its notice, its approval read (talaria-routes-workbench wires it at
/// boot). Unwired, the rows and audit rows still land, and the approvals
/// sweep reads the settled approvals' notifications.
pub type SettledSink =
    Arc<dyn Fn(Vec<PermissionRequest>) -> Pin<Box<dyn Future<Output = ()> + Send>> + Send + Sync>;

/// The sink [`settle_awaiting`] tells.
pub static SETTLED: OnceLock<SettledSink> = OnceLock::new();

/// What [`settle_awaiting`] makes of one request waiting on GitHub.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Settle {
    /// The installation holds it, and the agent's set still carries it.
    Granted,
    /// The agent's set no longer carries it: an admin took it out since.
    Withdrawn,
    /// Still waiting.
    Wait,
}

/// The rule, pure: `set` is what the agent carries on the host now, `held`
/// what the installation that mints the repo's token holds (None: never
/// read).
pub fn settle(
    set: &PermissionSet,
    held: Option<&PermissionSet>,
    key: &str,
    level: Level,
) -> Settle {
    if !holds(set, key, level) {
        return Settle::Withdrawn;
    }
    match held {
        Some(held) if holds(held, key, level) => Settle::Granted,
        _ => Settle::Wait,
    }
}

/// After a holdings refresh of `host_id`: every request there waiting on
/// GitHub that the installation now holds becomes `granted` (one
/// `workbench.github_permission_request_granted` row each), one whose
/// permission an admin took back out of the agent's set is withdrawn, and
/// the settled ones are told through [`SETTLED`]. Answers how many settled.
pub async fn settle_awaiting(pg: &PgPool, host_id: &str) -> usize {
    let waiting: Vec<PermissionRequest> = match sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SELECT} where r.host_id = $1 and r.status = 'awaiting_github' order by r.created_at"
    )))
    .bind(host_id)
    .fetch_all(pg)
    .await
    {
        Ok(rows) => rows,
        Err(e) => {
            tracing::warn!(
                "[git-hosts] the requests waiting on GitHub on {host_id} could not be read: {e}"
            );
            return 0;
        }
    };
    if waiting.is_empty() {
        return 0;
    }
    let Ok(Some(row)) = get_host(pg, host_id).await else {
        return 0;
    };
    let mut settled = Vec::new();
    for req in waiting {
        let Some(level) = Level::parse(&req.level) else {
            continue;
        };
        let set = match permissions::host_permissions(pg, &req.agent_id, host_id).await {
            Ok(set) => set,
            Err(e) => {
                tracing::warn!("[git-hosts] request {}: {e}", req.id);
                continue;
            }
        };
        let held = minting_installation(pg, &row, &req.repo)
            .await
            .and_then(|r| r.held);
        let (status, note, action) = match settle(&set, held.as_ref(), &req.permission, level) {
            Settle::Wait => continue,
            Settle::Granted => (
                "granted",
                None,
                "workbench.github_permission_request_granted",
            ),
            Settle::Withdrawn => (
                "withdrawn",
                Some("taken out of the agent's set before GitHub held it"),
                "workbench.github_permission_request_withdrawn",
            ),
        };
        let landed = sqlx::query(
            "update workbench_permission_requests set status = $2, updated_at = now(), \
               decision_note = coalesce($3, decision_note) \
             where id = $1::uuid and status = 'awaiting_github'",
        )
        .bind(&req.id)
        .bind(status)
        .bind(note)
        .execute(pg)
        .await;
        match landed {
            Ok(r) if r.rows_affected() == 1 => {}
            Ok(_) => continue,
            Err(e) => {
                tracing::warn!("[git-hosts] request {} could not be settled: {e}", req.id);
                continue;
            }
        }
        log_audit(
            pg,
            AuditEntry {
                actor: "talaria",
                action,
                target_type: "agent",
                target_id: Some(&req.agent_id),
                target_label: Some(req.agent_label()),
                before: Some(json!({ "status": "awaiting_github" })),
                after: Some(json!({
                    "request": req.id,
                    "host": host_id,
                    "repo": req.repo,
                    "permission": req.permission,
                    "level": req.level,
                    "status": status,
                })),
            },
        )
        .await;
        tracing::info!(
            "[git-hosts] {}'s request for {} on {host_id} is {status}",
            req.agent_label(),
            req.asked()
        );
        if let Ok(Some(now)) = get_request(pg, &req.id).await {
            settled.push(now);
        }
    }
    let n = settled.len();
    if n > 0
        && let Some(sink) = SETTLED.get()
    {
        sink(settled).await;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn asks(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs
            .iter()
            .map(|(k, l)| (k.to_string(), l.to_string()))
            .collect()
    }

    #[test]
    fn an_ask_is_a_key_and_level_github_knows_and_nothing_else_is_refused() {
        let set = read_asks(&asks(&[
            ("workflows", "write"),
            ("administration", "write"),
            ("organization_secrets", "read"),
            ("secrets", "read"),
            ("secrets", "write"),
        ]))
        .unwrap();
        assert_eq!(
            set,
            PermissionSet::of(&[
                ("administration", Level::Write),
                ("organization_secrets", Level::Read),
                ("secrets", Level::Write),
                ("workflows", Level::Write),
            ]),
            "a key named twice keeps its higher level, and no key GitHub knows is refused"
        );
        let unknown = read_asks(&asks(&[("sudo", "write")])).unwrap_err();
        assert!(unknown.contains("sudo"), "{unknown}");
        let level = read_asks(&asks(&[("workflows", "sometimes")])).unwrap_err();
        assert!(level.contains("not read, write or admin"), "{level}");
        let taken = read_asks(&asks(&[("workflows", "read")])).unwrap_err();
        assert!(taken.contains("workflows"), "{taken}");
        assert!(
            read_asks(&[])
                .unwrap_err()
                .contains("permissions is required")
        );
    }

    #[test]
    fn a_request_waiting_on_github_settles_when_both_sides_hold_it() {
        let wf = PermissionSet::of(&[("workflows", Level::Write)]);
        let default = permissions::default_set();
        let given = default.union(&wf);
        assert_eq!(
            settle(&given, Some(&given), "workflows", Level::Write),
            Settle::Granted
        );
        assert_eq!(
            settle(&given, Some(&default), "workflows", Level::Write),
            Settle::Wait
        );
        assert_eq!(
            settle(&given, None, "workflows", Level::Write),
            Settle::Wait,
            "never read is not held"
        );
        let read = default.union(&PermissionSet::of(&[("workflows", Level::Read)]));
        assert_eq!(
            settle(&given, Some(&read), "workflows", Level::Write),
            Settle::Wait,
            "held below the level asked is not held"
        );
        assert_eq!(
            settle(&default, Some(&given), "workflows", Level::Write),
            Settle::Withdrawn,
            "taken back out of the set"
        );
    }

    #[test]
    fn the_census_key_and_the_lines_name_the_request() {
        assert_eq!(approval_key("r1"), "git_permission:r1");
    }
}
