// Moving a ticket between boards — the one writer of `tasks.board_id` after
// a ticket is born.
//
// A board owns everything a ticket's fields MEAN: its columns (status keys),
// its labels, its members and agents, its ticket prefix and counter, its
// workchains and its sub-task tree. So a move is not an UPDATE of one column.
// It is a translation of every board-scoped field into the destination's
// terms, and each translation either has an honest answer or refuses:
//
//   status     the caller's column, else the same NAME there, else the same
//              CATEGORY, else the destination's intake. Never a closed
//              column for an open ticket — a move is not a sign-off — and
//              never one at all for an agent (`pick_move_status`).
//   labels     kept by name (the destination's spelling); the rest DROPPED,
//              never minted on the destination, and named in the answer and
//              the activity line (`map_labels`).
//   assignees  a person who cannot see the destination, or an agent it does
//              not allow, is UNASSIGNED and named. Keeping them would leave
//              an assignee who is told about a ticket they cannot open, or an
//              agent dispatch will refuse — the assignment would read true
//              and be false.
//   ref        re-keyed on the destination's counter; the old (board, number)
//              stays in `task_ref_aliases`, so the old ref still resolves.
//   parent     a sub-task leaves its parent behind (sub-tasks live on their
//              parent's board); a ticket WITH sub-tasks, a workchain step,
//              and a ticket an agent is working right now are refused (409)
//              rather than half-moved.
//
// Everything keyed by the ticket's own id — comments and the room, activity,
// watchers, reviews, judge verdicts, usage, attachments, dependencies (which
// are already cross-board) — comes along untouched. Watchers who cannot see
// the destination stay stored but dormant, exactly as for a person removed
// from a board (`resolve_watchers`), and the answer names them.

use serde::Serialize;
use sqlx::PgPool;
use talaria_agent_auth::AgentSubject;
use talaria_boards::{board_allows_agent, board_info, board_role, can_edit};
use talaria_labels::list_labels;
use talaria_realtime::{BoardEvent, publish_board};
use talaria_retrieval::sources::{CommentSrc, TicketSrc, comment_doc, index_activity, ticket_doc};
use talaria_retrieval::{embed, qdrant};
use talaria_statuses::{BoardStatus, OFF_BOARD_STATUSES, list_statuses, meta_of};

use super::{
    AgentIntent, AgentWriteTarget, Task, TaskDeps, TaskNotification, agent_assignees,
    agent_ticket_refusal, get_task, log_activity, notify_task_users, resolve_review_approval,
    resolve_watchers, spawn_dispatch_id, ticket_audience,
};

// ── The request, the mover, the answer ───────────────────────────────────────

pub struct MoveRequest<'a> {
    pub board_id: &'a str,
    /// A column key on the DESTINATION board. None: mapped (see the header).
    pub status: Option<&'a str>,
    /// Answer what the move WOULD do and write nothing — the confirm dialog's
    /// read, and an agent's way to look before it leaps.
    pub preview: bool,
}

/// Who is moving the ticket. A person is gated by board ROLE on both ends; an
/// agent by board POLICY on both ends plus the same HITL rules its other
/// ticket writes obey.
pub enum Mover {
    Human {
        user_id: String,
        actor: String,
    },
    Agent {
        subject: AgentSubject,
        model: String,
    },
}

impl Mover {
    fn actor(&self) -> &str {
        match self {
            Mover::Human { actor, .. } => actor,
            Mover::Agent { model, .. } => model,
        }
    }
    fn is_agent(&self) -> bool {
        matches!(self, Mover::Agent { .. })
    }
}

/// How the destination column was chosen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum StatusHow {
    /// The caller named it.
    Requested,
    /// A column with the same key or label.
    SameName,
    /// The first column of the same category (open/active/review/done).
    SameCategory,
    /// No match: the destination's intake column.
    Intake,
    /// A closed ticket in an off-board terminal (failed/cancelled) — legal on
    /// every board, so it keeps its status.
    KeptClosed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusPick {
    pub key: String,
    pub how: StatusHow,
}

/// What a move did (or, previewed, would do). Every key is something a
/// person reads in the confirm dialog or an agent reads in its tool result.
///
/// NO `boardId` + `id` PAIR ON PURPOSE: the chip extractor turns any object
/// carrying both into a ticket link (talaria-chips `walk_links`), so the
/// summary names its boards `fromBoardId` / `toBoardId` and the one link a
/// tool result yields is the moved `task` itself, on its new board.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MoveSummary {
    pub preview: bool,
    pub from_board_id: String,
    pub from_board_name: String,
    pub to_board_id: String,
    pub to_board_name: String,
    pub from_ref: Option<String>,
    /// The new ref. On a preview, the number the destination's counter would
    /// hand out right now — a concurrent create can take it first.
    pub to_ref: Option<String>,
    pub from_status: String,
    pub status: String,
    pub status_label: String,
    pub status_how: StatusHow,
    pub labels_kept: Vec<String>,
    pub labels_dropped: Vec<String>,
    pub unassigned: Vec<String>,
    /// It was a sub-task; it leaves its parent on the old board.
    pub parent_detached: bool,
    /// A human sign-off mark (approved_at) that does not travel: the
    /// destination's reviewers sign it off. A closed ticket keeps its mark.
    pub approval_cleared: bool,
    /// Watchers who cannot see the destination: kept but silent until they
    /// can (the same rule as a person removed from a board).
    pub dormant_watchers: Vec<String>,
    /// Dependency links, kept as they are (they may cross boards).
    pub dependencies: i64,
}

pub struct MoveOutcome {
    /// The ticket as it now stands; None on a preview.
    pub task: Option<Task>,
    pub moved: MoveSummary,
}

#[derive(Debug)]
pub enum MoveError {
    /// 404 — the ticket is gone.
    NotFound,
    /// 400 — the caller's own fix (a status the board lacks, a closing move).
    Bad(String),
    /// 403 — the mover may not.
    Forbidden(String),
    /// 409 — something live holds the ticket where it is.
    Conflict(String),
    Db(sqlx::Error),
}

impl From<sqlx::Error> for MoveError {
    fn from(e: sqlx::Error) -> Self {
        MoveError::Db(e)
    }
}

/// `pick_move_status`'s two refusals: the caller's fix (400) or authority (403).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PickRefusal {
    Bad(String),
    Forbidden(String),
}

impl From<PickRefusal> for MoveError {
    fn from(r: PickRefusal) -> Self {
        match r {
            PickRefusal::Bad(m) => MoveError::Bad(m),
            PickRefusal::Forbidden(m) => MoveError::Forbidden(m),
        }
    }
}

/// The agent's refusal for a closed destination, one sentence wherever it
/// is said.
const AGENT_NO_CLOSE: &str = "agents cannot move a ticket into a closed column — a person signs work off. Leave status out and the move keeps the ticket open";

// ── The pure mappings ────────────────────────────────────────────────────────

fn same_name(s: &BoardStatus, key: &str, label: Option<&str>) -> bool {
    s.key == key
        || label
            .is_some_and(|l| !l.trim().is_empty() && s.label.trim().eq_ignore_ascii_case(l.trim()))
}

/// The destination column for a move. `src`/`dest` are the two boards'
/// status lists as `list_statuses` serves them (system Blocked included);
/// `dest_name` words the refusals.
///
/// THE RULES, in order:
/// · an AGENT may not move a ticket that is in review (a person signs it off),
///   blocked (a person unblocks it), stranded in a column its board no longer
///   has (a person places it), or closed — the same four stops its status
///   writes obey (`agent_safe_patch`, `agent_ticket_refusal`);
/// · a NAMED column must exist on the destination. A closed one is refused
///   for an agent always, and for a person unless the ticket is already
///   closed — a move carries closure, it never creates it;
/// · an agent may not ENTER an agent-start (pickup) column from a column that
///   was not one: entering a pickup queue is assignment, a person's call;
/// · otherwise: same name, then same category, then intake — never a closed
///   column for an open ticket. A closed ticket (a person's move) lands in
///   the destination's done column of the same name, else its first one;
///   an off-board terminal (failed/cancelled) keeps its key.
pub fn pick_move_status(
    src: &[BoardStatus],
    src_key: &str,
    dest: &[BoardStatus],
    dest_name: &str,
    requested: Option<&str>,
    agent: bool,
) -> Result<StatusPick, PickRefusal> {
    let sm = meta_of(src);
    let dm = meta_of(dest);
    let closed = sm.terminal(src_key);
    let src_status = src.iter().find(|s| s.key == src_key);
    let src_label = src_status.map(|s| s.label.as_str());
    let src_start = sm.agent_start_keys.iter().any(|k| k == src_key);
    let enters_pickup = |k: &str| !src_start && dm.agent_start_keys.iter().any(|s| s == k);
    if agent {
        if closed {
            return Err(PickRefusal::Forbidden(
                "agents cannot change a closed ticket".into(),
            ));
        }
        if sm.review_keys.iter().any(|k| k == src_key) {
            return Err(PickRefusal::Forbidden(
                "agents cannot take a ticket out of review — a person signs it off, or moves it"
                    .into(),
            ));
        }
        if src_key == "blocked" {
            return Err(PickRefusal::Forbidden(
                "agents cannot move a blocked ticket — a person unblocks it first".into(),
            ));
        }
        if !sm.keys.iter().any(|k| k == src_key) {
            return Err(PickRefusal::Forbidden(format!(
                "\"{src_key}\" is no longer a column on this board — a person has to place this ticket"
            )));
        }
    }
    if let Some(k) = requested {
        let legal = dm.keys.iter().any(|x| x == k) || OFF_BOARD_STATUSES.contains(&k);
        if !legal {
            return Err(PickRefusal::Bad(format!(
                "\"{k}\" is not a status on {dest_name}"
            )));
        }
        if dm.terminal(k) {
            if agent {
                return Err(PickRefusal::Forbidden(AGENT_NO_CLOSE.into()));
            }
            if !closed {
                return Err(PickRefusal::Bad(format!(
                    "a move never closes a ticket — move it to an open column on {dest_name}, and close it there once it is signed off"
                )));
            }
        } else if agent && enters_pickup(k) {
            return Err(PickRefusal::Forbidden(
                "agents cannot assign tickets — that column is an agent pickup queue, and a person puts work there"
                    .into(),
            ));
        }
        return Ok(StatusPick {
            key: k.to_string(),
            how: StatusHow::Requested,
        });
    }
    if closed {
        if OFF_BOARD_STATUSES.contains(&src_key) {
            return Ok(StatusPick {
                key: src_key.to_string(),
                how: StatusHow::KeptClosed,
            });
        }
        let dones: Vec<&BoardStatus> = dest.iter().filter(|s| s.category == "done").collect();
        if let Some(s) = dones.iter().find(|s| same_name(s, src_key, src_label)) {
            return Ok(StatusPick {
                key: s.key.clone(),
                how: StatusHow::SameName,
            });
        }
        return match dones.first() {
            Some(s) => Ok(StatusPick {
                key: s.key.clone(),
                how: StatusHow::SameCategory,
            }),
            None => Err(PickRefusal::Bad(format!(
                "{dest_name} has no done column to carry a closed ticket into — reopen it first, or add a done column there"
            ))),
        };
    }
    // Open ticket: never a closed column, and for an agent never a pickup
    // queue it was not already in.
    let candidates: Vec<&BoardStatus> = dest
        .iter()
        .filter(|s| !dm.terminal(&s.key) && !(agent && enters_pickup(&s.key)))
        .collect();
    if let Some(s) = candidates.iter().find(|s| same_name(s, src_key, src_label)) {
        return Ok(StatusPick {
            key: s.key.clone(),
            how: StatusHow::SameName,
        });
    }
    if let Some(cat) = src_status.map(|s| s.category.as_str())
        && let Some(s) = candidates.iter().find(|s| s.category == cat)
    {
        return Ok(StatusPick {
            key: s.key.clone(),
            how: StatusHow::SameCategory,
        });
    }
    let intake = dm
        .default_key
        .as_deref()
        .filter(|k| candidates.iter().any(|s| s.key == *k))
        .map(str::to_string)
        .or_else(|| {
            candidates
                .iter()
                .find(|s| s.category == "open")
                .map(|s| s.key.clone())
        });
    match intake {
        Some(key) => Ok(StatusPick {
            key,
            how: StatusHow::Intake,
        }),
        None => Err(PickRefusal::Bad(format!(
            "{dest_name} has no intake column, so there is nowhere to put the ticket. Add an open-category column in board settings → statuses"
        ))),
    }
}

/// The ticket's labels in the destination's terms: (kept, dropped). A label
/// is kept when the destination has one of that name — exactly, else
/// ignoring case and surrounding space, in which case the destination's
/// spelling is the one kept. Nothing is created on the destination: a
/// label is the board's vocabulary, and a move must not grow it. Order is
/// the ticket's; duplicates collapse.
pub fn map_labels(tags: &[String], dest: &[String]) -> (Vec<String>, Vec<String>) {
    let mut kept: Vec<String> = Vec::new();
    let mut dropped: Vec<String> = Vec::new();
    for tag in tags {
        let t = tag.trim();
        if t.is_empty() {
            continue;
        }
        let hit = dest
            .iter()
            .find(|d| d.as_str() == t)
            .or_else(|| dest.iter().find(|d| d.trim().eq_ignore_ascii_case(t)));
        match hit {
            Some(d) => {
                if !kept.contains(d) {
                    kept.push(d.clone());
                }
            }
            None => {
                if !dropped.iter().any(|x| x == t) {
                    dropped.push(t.to_string());
                }
            }
        }
    }
    (kept, dropped)
}

// ── Reads the move needs ─────────────────────────────────────────────────────

/// The ids of tickets that once carried `PREFIX-<no>` on a board they have
/// since left (`task_ref_aliases`), under the board's CURRENT prefix — the
/// same expression a live ref resolves through. Distinct: one ticket can
/// answer for an alias only once.
pub(crate) async fn aliased_task_ids(
    pg: &PgPool,
    prefix: &str,
    no: i32,
) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "select distinct a.task_id::text from task_ref_aliases a \
         join boards b on b.id = a.board_id \
         where upper(coalesce(b.ticket_prefix, 'TASK')) = upper($1) and a.ticket_no = $2 \
         limit 2",
    )
    .bind(prefix)
    .bind(no)
    .fetch_all(pg)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// (name, prefix, counter) — the destination's identity and the number the
/// next ticket would get.
async fn board_head(pg: &PgPool, board_id: &str) -> Result<(String, String, i32), sqlx::Error> {
    let row: Option<(String, Option<String>, i32)> =
        sqlx::query_as("select name, ticket_prefix, ticket_seq from boards where id = $1::uuid")
            .bind(board_id)
            .fetch_optional(pg)
            .await?;
    Ok(match row {
        Some((name, prefix, seq)) => (name, prefix.unwrap_or_else(|| "TASK".into()), seq),
        None => (board_id.to_string(), "TASK".into(), 0),
    })
}

/// The three live holds that keep a ticket where it is, each a 409 a person
/// can act on.
async fn live_hold(pg: &PgPool, task_id: &str) -> Result<Option<String>, sqlx::Error> {
    // The ticker's own question (GET /api/tasks/{id}/work-session): a
    // non-terminal work-session run on this ticket. `awaiting` counts — a
    // session parked on a person still holds the ticket's context.
    let working: Option<(i32,)> = sqlx::query_as(
        "select 1 from runs where kind = 'work-session' and subject_id = $1 \
           and state in ('queued', 'running', 'awaiting') limit 1",
    )
    .bind(task_id)
    .fetch_optional(pg)
    .await?;
    if working.is_some() {
        return Ok(Some(
            "an agent's work session is live on this ticket — stop the work first (the ticket's Stop the work), then move it"
                .into(),
        ));
    }
    let chain: Option<(String,)> = sqlx::query_as(
        "select w.name from task_workchain_steps s \
         join task_workchains w on w.id = s.workchain_id where s.task_id = $1::uuid limit 1",
    )
    .bind(task_id)
    .fetch_optional(pg)
    .await?;
    if let Some((name,)) = chain {
        return Ok(Some(format!(
            "this ticket is a step of the workchain \"{name}\", and a workchain's steps live on its board — take it out of the chain first, then move it"
        )));
    }
    let (kids,): (i64,) = sqlx::query_as("select count(*) from tasks where parent_id = $1::uuid")
        .bind(task_id)
        .fetch_one(pg)
        .await?;
    if kids > 0 {
        return Ok(Some(format!(
            "this ticket has {kids} sub-task{} on its board, and sub-tasks live on their parent's board — promote {} to top level, or move {} first",
            if kids == 1 { "" } else { "s" },
            if kids == 1 { "it" } else { "them" },
            if kids == 1 { "it" } else { "them" },
        )));
    }
    Ok(None)
}

// ── The move ─────────────────────────────────────────────────────────────────

/// Move ticket `id` to `req.board_id`, or (preview) say what that would do.
/// The route authenticates; this decides — permission, holds, the mapping —
/// and writes the row in one transaction, then owes the two boards their
/// events, the ticket its activity, the people their notices and the brain
/// its re-index.
pub async fn move_task(
    deps: &TaskDeps,
    id: &str,
    req: &MoveRequest<'_>,
    mover: &Mover,
) -> Result<MoveOutcome, MoveError> {
    let pg = &deps.pg;
    let Some(cur) = get_task(pg, id).await? else {
        return Err(MoveError::NotFound);
    };
    let src_info = board_info(pg, &cur.board_id).await?;
    let dest_info = board_info(pg, req.board_id).await?;

    // ── Permission: both ends ────────────────────────────────────────────
    match mover {
        Mover::Human { user_id, .. } => {
            if !can_edit(board_role(pg, user_id, &cur.board_id).await?.as_deref()) {
                return Err(MoveError::Forbidden("forbidden".into()));
            }
            // Forbidden, not hidden — the boards family's rule: a board you
            // cannot see answers the same whether it exists or not.
            let dest_role = board_role(pg, user_id, req.board_id).await?;
            if !dest_info.exists || dest_role.is_none() {
                return Err(MoveError::Forbidden(
                    "you are not a member of that board — you can only move a ticket to a board you can add tickets to"
                        .into(),
                ));
            }
            if !can_edit(dest_role.as_deref()) {
                return Err(MoveError::Forbidden(format!(
                    "you can view {} but not add tickets to it — an editor or owner of that board can move tickets in",
                    dest_info.label
                )));
            }
            if dest_info.archived_at.is_some() {
                return Err(MoveError::Bad(format!(
                    "{} is archived — restore it before moving tickets onto it",
                    dest_info.label
                )));
            }
            if cur.archived_at.is_some() {
                return Err(MoveError::Bad(
                    "this ticket is archived — restore it first, then move it".into(),
                ));
            }
        }
        Mover::Agent { subject, model } => {
            // The source end is the question every agent ticket write asks.
            if let Some(shut) = agent_ticket_refusal(
                pg,
                &AgentWriteTarget::from(&cur),
                subject,
                AgentIntent::Write,
            )
            .await?
            {
                return Err(MoveError::Forbidden(shut));
            }
            // The destination end is board policy — the same gate creating a
            // ticket there asks (`board_allows_agent`, the caller so the
            // elevated-assistant bypass reads `legacy`).
            if !board_allows_agent(pg, req.board_id, subject).await? {
                return Err(MoveError::Forbidden(
                    if dest_info.exists && dest_info.archived_at.is_some() {
                        format!(
                            "agents cannot move a ticket onto an archived board — a person restores {} first",
                            dest_info.label
                        )
                    } else {
                        format!(
                            "agent \"{model}\" is not allowed on that board — you can only move a ticket between boards you are on (list_boards); a board editor adds agents to a board"
                        )
                    },
                ));
            }
        }
    }
    if cur.board_id == req.board_id {
        return Err(MoveError::Bad(format!(
            "the ticket is already on {}",
            src_info.label
        )));
    }

    // ── What holds it where it is ────────────────────────────────────────
    if let Some(hold) = live_hold(pg, id).await? {
        return Err(MoveError::Conflict(hold));
    }

    // ── The mapping ──────────────────────────────────────────────────────
    let (src_statuses, dest_statuses) = tokio::try_join!(
        list_statuses(pg, &cur.board_id),
        list_statuses(pg, req.board_id),
    )?;
    let pick = pick_move_status(
        &src_statuses,
        &cur.status,
        &dest_statuses,
        &dest_info.label,
        req.status,
        mover.is_agent(),
    )?;
    let src_meta = meta_of(&src_statuses);
    let dest_meta = meta_of(&dest_statuses);
    let status_label = dest_statuses
        .iter()
        .find(|s| s.key == pick.key)
        .map(|s| s.label.clone())
        .unwrap_or_else(|| pick.key.replace('_', " "));
    let dest_labels: Vec<String> = list_labels(pg, req.board_id)
        .await?
        .into_iter()
        .map(|l| l.name)
        .collect();
    let (labels_kept, labels_dropped) = map_labels(&cur.tags, &dest_labels);
    let mut assignees: Vec<String> = Vec::new();
    let mut unassigned: Vec<String> = Vec::new();
    for a in &cur.assignees {
        let stays = match a.strip_prefix("user:") {
            Some(user_id) => {
                uuid::Uuid::parse_str(user_id).is_ok()
                    && board_role(pg, user_id, req.board_id).await?.is_some()
            }
            None => board_allows_agent(pg, req.board_id, &AgentSubject::Model(a.clone())).await?,
        };
        if stays {
            assignees.push(a.clone());
        } else {
            unassigned.push(a.clone());
        }
    }
    let watching_now: Vec<String> = resolve_watchers(pg, id, Some(cur.board_id.as_str()))
        .await?
        .into_iter()
        .map(|w| w.watcher)
        .collect();
    let watching_after: Vec<String> = resolve_watchers(pg, id, Some(req.board_id))
        .await?
        .into_iter()
        .map(|w| w.watcher)
        .collect();
    let dormant_watchers: Vec<String> = watching_now
        .into_iter()
        .filter(|w| !watching_after.contains(w))
        .collect();
    let (dependencies,): (i64,) = sqlx::query_as(
        "select count(*) from task_dependencies where task_id = $1::uuid or depends_on_id = $1::uuid",
    )
    .bind(id)
    .fetch_one(pg)
    .await?;
    // Closure travels, sign-off by a reviewer of the OLD board does not.
    let stays_closed = dest_meta.terminal(&pick.key);
    let approval_cleared = cur.approved_at.is_some() && !stays_closed;
    let (src_name, _, _) = board_head(pg, &cur.board_id).await?;
    let (dest_name, dest_prefix, dest_seq) = board_head(pg, req.board_id).await?;
    let mut moved = MoveSummary {
        preview: req.preview,
        from_board_id: cur.board_id.clone(),
        from_board_name: src_name.clone(),
        to_board_id: req.board_id.to_string(),
        to_board_name: dest_name.clone(),
        from_ref: cur.ticket_ref.clone(),
        to_ref: Some(format!("{dest_prefix}-{}", dest_seq + 1)),
        from_status: cur.status.clone(),
        status: pick.key.clone(),
        status_label,
        status_how: pick.how,
        labels_kept: labels_kept.clone(),
        labels_dropped,
        unassigned,
        parent_detached: cur.parent_id.is_some(),
        approval_cleared,
        dormant_watchers,
        dependencies,
    };
    if req.preview {
        return Ok(MoveOutcome { task: None, moved });
    }

    // ── The write: one transaction ───────────────────────────────────────
    // The counter bump, the alias and the row share a fate, like create's
    // ref and row: a board never burns a number into nothing, and an old ref
    // never points at a ticket that did not move.
    let mut tx = pg.begin().await?;
    let held: Option<(String, Option<i32>)> = sqlx::query_as(
        "select board_id::text, ticket_no from tasks where id = $1::uuid for update",
    )
    .bind(id)
    .fetch_optional(&mut *tx)
    .await?;
    let Some((held_board, old_no)) = held else {
        return Err(MoveError::NotFound);
    };
    if held_board != cur.board_id {
        return Err(MoveError::Conflict(
            "the ticket moved while this was being decided — reload it and try again".into(),
        ));
    }
    let (ticket_no,): (i32,) = sqlx::query_as(
        "update boards set ticket_seq = ticket_seq + 1, updated_at = now() \
         where id = $1::uuid returning ticket_seq",
    )
    .bind(req.board_id)
    .fetch_one(&mut *tx)
    .await?;
    if let Some(no) = old_no {
        sqlx::query(
            "insert into task_ref_aliases (board_id, ticket_no, task_id) \
             values ($1::uuid, $2, $3::uuid) on conflict (board_id, ticket_no) do nothing",
        )
        .bind(&cur.board_id)
        .bind(no)
        .bind(id)
        .execute(&mut *tx)
        .await?;
    }
    sqlx::query(
        "update tasks set board_id = $2::uuid, ticket_no = $3, status = $4, tags = $5, \
         assignees = $6, parent_id = null, \
         completed_at = case when $7 then completed_at end, \
         approved_at = case when $7 then approved_at end, \
         updated_at = now() where id = $1::uuid",
    )
    .bind(id)
    .bind(req.board_id)
    .bind(ticket_no)
    .bind(&pick.key)
    .bind(serde_json::json!(labels_kept))
    .bind(serde_json::json!(assignees))
    .bind(stays_closed)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    moved.to_ref = Some(format!("{dest_prefix}-{ticket_no}"));

    // ── What the move owes, after the commit ─────────────────────────────
    // Both boards' views: the card left one and arrived on the other.
    publish_board(
        &deps.realtime,
        &cur.board_id,
        &BoardEvent {
            kind_tag: "task",
            task_id: Some(id.to_string()),
            deleted: Some(true),
        },
    );
    publish_board(
        &deps.realtime,
        req.board_id,
        &BoardEvent {
            kind_tag: "task",
            task_id: Some(id.to_string()),
            deleted: None,
        },
    );
    spawn_move_notices(deps, &cur, &moved, &assignees, mover.actor());
    let actor = mover.actor();
    log_activity(pg, id, actor, "moved", &activity_line(&moved)).await?;
    if moved.status != moved.from_status {
        // The column changed too, and the record says so in the words every
        // status move uses — the approvals inbox reads "entered review" off
        // exactly this line.
        log_activity(
            pg,
            id,
            actor,
            "status",
            &format!("moved to {} (moved boards)", moved.status),
        )
        .await?;
    }
    // The sign-off queue of the old board no longer holds it.
    if src_meta.review_keys.iter().any(|k| k == &cur.status) {
        resolve_review_approval(&deps.notify, id);
    }
    // Landing in a pickup queue with agent assignees is work for them — the
    // dispatch gate decides whether it may start, as for any write.
    if dest_meta.agent_start_keys.contains(&moved.status) && !agent_assignees(&assignees).is_empty()
    {
        spawn_dispatch_id(deps, id.to_string(), None);
    }
    let task = get_task(pg, id).await?;
    if let Some(t) = &task {
        // The room's topic carries the ref; the brain's points carry the
        // board (`payload.boardId` is the retrieval ACL), so the ticket and
        // every comment re-index under the new board or the OLD board's
        // members keep finding them.
        refresh_room_and_brain(pg, t);
    }
    Ok(MoveOutcome { task, moved })
}

/// The ticket's own record of the move, one line: from → to with both refs,
/// and every translation that lost something.
fn activity_line(m: &MoveSummary) -> String {
    let refd = |name: &str, r: &Option<String>| match r {
        Some(r) => format!("{name} ({r})"),
        None => name.to_string(),
    };
    let mut line = format!(
        "moved from {} to {}",
        refd(&m.from_board_name, &m.from_ref),
        refd(&m.to_board_name, &m.to_ref)
    );
    if !m.labels_dropped.is_empty() {
        line.push_str(&format!(
            " · labels dropped (not on {}): {}",
            m.to_board_name,
            m.labels_dropped.join(", ")
        ));
    }
    if !m.unassigned.is_empty() {
        line.push_str(&format!(
            " · unassigned (not on {}): {}",
            m.to_board_name,
            m.unassigned.join(", ")
        ));
    }
    if m.parent_detached {
        line.push_str(" · left its parent ticket behind");
    }
    if m.approval_cleared {
        line.push_str(" · sign-off cleared");
    }
    line
}

/// The move's notice: the ticket's audience ON THE DESTINATION — watchers,
/// assigned humans and the creator who can see it there (`ticket_audience`,
/// the same set a status move tells), never the mover. People who cannot
/// see the new board are not told where it went. Detached, like every
/// post-commit notice (C30).
fn spawn_move_notices(
    deps: &TaskDeps,
    cur: &Task,
    m: &MoveSummary,
    assignees: &[String],
    actor: &str,
) {
    let deps = deps.clone();
    let id = cur.id.clone();
    let created_by = cur.created_by.clone();
    let title = cur.title.clone();
    let assignees = assignees.to_vec();
    let actor = actor.to_string();
    let m = m.clone();
    tokio::spawn(async move {
        let audience =
            match ticket_audience(&deps.pg, &id, &m.to_board_id, &assignees, &created_by).await {
                Ok(a) => a,
                Err(e) => {
                    tracing::error!("tasks: move audience read failed: {e}");
                    return;
                }
            };
        let was = match &m.from_ref {
            Some(r) => format!("was {r} on {}", m.from_board_name),
            None => format!("was on {}", m.from_board_name),
        };
        notify_task_users(
            &deps.notify,
            &audience,
            &actor,
            &TaskNotification {
                kind: "task-status".into(),
                title: format!(
                    "{}: moved to {}",
                    m.to_ref.clone().unwrap_or_else(|| title.clone()),
                    m.to_board_name
                ),
                body: Some(format!("{title} — {was}")),
                href: Some(format!("/boards/{}/{id}", m.to_board_id)),
                task_id: id.clone(),
                status_burst: None,
            },
        )
        .await;
    });
}

/// Detached: the room's topic line (it names the ref) and the brain's
/// ticket + comment points, under the new board.
fn refresh_room_and_brain(pg: &PgPool, t: &Task) {
    let pg = pg.clone();
    let t = t.clone();
    tokio::spawn(async move {
        if let Some(head) = talaria_ticket_chat::ticket_head(&pg, &t.id).await
            && let Err(e) = sqlx::query("update channels set topic = $2 where task_id = $1::uuid")
                .bind(&t.id)
                .bind(head.line())
                .execute(&pg)
                .await
        {
            tracing::error!("tasks: room topic after move failed: {e}");
        }
        let qd = qdrant::real_deps();
        let ed = embed::real_deps();
        let _ = index_activity(
            &pg,
            &qd,
            &ed,
            &ticket_doc(&TicketSrc {
                id: &t.id,
                board_id: &t.board_id,
                ticket_ref: t.ticket_ref.as_deref(),
                title: &t.title,
                description: t.description.as_deref(),
            }),
        )
        .await;
        let comments: Vec<(String, String, String)> = sqlx::query_as(
            "select cm.id::text, \
                    case when cm.author_type = 'user' \
                         then coalesce(u.name, u.email, cm.author) else cm.author end, \
                    cm.content \
             from channel_messages cm \
             join channels c on c.id = cm.channel_id \
             left join users u on u.email = cm.author \
             where c.task_id = $1::uuid and cm.status = 'complete' and cm.content <> ''",
        )
        .bind(&t.id)
        .fetch_all(&pg)
        .await
        .unwrap_or_default();
        for (cid, author, content) in comments {
            let _ = index_activity(
                &pg,
                &qd,
                &ed,
                &comment_doc(&CommentSrc {
                    id: &cid,
                    task_id: &t.id,
                    board_id: &t.board_id,
                    ticket_ref: t.ticket_ref.as_deref(),
                    author: &author,
                    content: &content,
                }),
            )
            .await;
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(key: &str, label: &str, category: &str, agent_start: bool) -> BoardStatus {
        BoardStatus {
            id: Some(format!("id-{key}")),
            key: key.into(),
            label: label.into(),
            color: "slate".into(),
            category: category.into(),
            agent_start,
            position: 0,
            system: None,
        }
    }

    /// The shipped defaults, plus the system Blocked column the list serves.
    fn defaults() -> Vec<BoardStatus> {
        vec![
            st("inbox", "Inbox", "open", false),
            st("assigned", "Assigned", "open", true),
            st("in_progress", "In progress", "active", true),
            talaria_statuses::blocked_status(),
            st("quality_review", "Quality review", "review", false),
            st("done", "Done", "done", false),
        ]
    }

    /// A board that renamed everything: different keys, one shared label.
    fn custom() -> Vec<BoardStatus> {
        vec![
            st("triage", "Triage", "open", false),
            st("queue", "Queue", "open", true),
            st("doing", "In Progress", "active", true),
            talaria_statuses::blocked_status(),
            st("qa", "QA", "review", false),
            st("shipped", "Shipped", "done", false),
        ]
    }

    fn pick(
        src_key: &str,
        requested: Option<&str>,
        agent: bool,
    ) -> Result<StatusPick, PickRefusal> {
        pick_move_status(&defaults(), src_key, &custom(), "\"Ops\"", requested, agent)
    }

    #[test]
    fn a_column_maps_by_name_then_category_then_intake() {
        // "In progress" ↔ "In Progress": the label, case aside.
        assert_eq!(
            pick("in_progress", None, false).unwrap(),
            StatusPick {
                key: "doing".into(),
                how: StatusHow::SameName
            }
        );
        // No "Inbox" there: the first open column.
        assert_eq!(
            pick("inbox", None, false).unwrap(),
            StatusPick {
                key: "triage".into(),
                how: StatusHow::SameCategory
            }
        );
        assert_eq!(
            pick("quality_review", None, false).unwrap(),
            StatusPick {
                key: "qa".into(),
                how: StatusHow::SameCategory
            }
        );
        // Blocked is on every board, by key.
        assert_eq!(pick("blocked", None, false).unwrap().key, "blocked");
        // A key the source board no longer has (stranded): intake.
        assert_eq!(
            pick("ghost", None, false).unwrap(),
            StatusPick {
                key: "triage".into(),
                how: StatusHow::Intake
            }
        );
    }

    #[test]
    fn a_move_never_closes_an_open_ticket() {
        // Named: refused, with the way through.
        let Err(PickRefusal::Bad(msg)) = pick("in_progress", Some("shipped"), false) else {
            panic!("a person naming a done column for an open ticket must be refused");
        };
        assert!(msg.contains("never closes"), "{msg}");
        assert!(matches!(
            pick("inbox", Some("cancelled"), false),
            Err(PickRefusal::Bad(_))
        ));
        // Mapped: a done-category match is never a candidate for an open
        // ticket, even when the source column is NAMED like the done one.
        let mut src = defaults();
        src.push(st("shipped_soon", "Shipped", "active", false));
        let p = pick_move_status(&src, "shipped_soon", &custom(), "Ops", None, false).unwrap();
        assert_eq!(
            p.key, "doing",
            "an active ticket lands active, never in Shipped"
        );
    }

    #[test]
    fn a_closed_ticket_stays_closed_for_a_person() {
        assert_eq!(
            pick("done", None, false).unwrap(),
            StatusPick {
                key: "shipped".into(),
                how: StatusHow::SameCategory
            }
        );
        assert_eq!(
            pick("cancelled", None, false).unwrap(),
            StatusPick {
                key: "cancelled".into(),
                how: StatusHow::KeptClosed
            }
        );
        // A person may also reopen it into a named open column.
        assert_eq!(pick("done", Some("triage"), false).unwrap().key, "triage");
        // …or name a done column, since it is already closed.
        assert_eq!(pick("done", Some("shipped"), false).unwrap().key, "shipped");
    }

    #[test]
    fn an_agent_never_lands_a_ticket_in_a_closed_column() {
        for closed in ["shipped", "failed", "cancelled"] {
            let r = pick("in_progress", Some(closed), true);
            assert_eq!(
                r,
                Err(PickRefusal::Forbidden(AGENT_NO_CLOSE.into())),
                "{closed}"
            );
        }
        // And never moves a closed ticket at all.
        assert!(matches!(
            pick("done", None, true),
            Err(PickRefusal::Forbidden(_))
        ));
        assert!(matches!(
            pick("cancelled", None, true),
            Err(PickRefusal::Forbidden(_))
        ));
    }

    #[test]
    fn an_agent_obeys_the_review_blocked_and_stranded_stops() {
        for src in ["quality_review", "blocked", "ghost"] {
            assert!(
                matches!(pick(src, None, true), Err(PickRefusal::Forbidden(_))),
                "{src}"
            );
        }
    }

    #[test]
    fn an_agent_cannot_enter_a_pickup_queue_it_was_not_in() {
        // Named: refused.
        let Err(PickRefusal::Forbidden(msg)) = pick("inbox", Some("queue"), true) else {
            panic!("entering a pickup queue is assignment");
        };
        assert!(msg.contains("cannot assign"), "{msg}");
        // Mapped: the pickup columns are skipped — "Assigned"/"Queue" open
        // and agent-start, so an open-category match falls to Triage.
        assert_eq!(pick("inbox", None, true).unwrap().key, "triage");
        // From a pickup queue it was already in, the move is not assignment.
        assert_eq!(pick("assigned", Some("queue"), true).unwrap().key, "queue");
        assert_eq!(pick("in_progress", None, true).unwrap().key, "doing");
        // A person may.
        assert_eq!(pick("inbox", Some("queue"), false).unwrap().key, "queue");
    }

    #[test]
    fn a_status_the_destination_lacks_is_the_callers_fix() {
        let Err(PickRefusal::Bad(msg)) = pick("inbox", Some("in_progress"), false) else {
            panic!("a default key on a renamed board is not a column there");
        };
        assert_eq!(msg, "\"in_progress\" is not a status on \"Ops\"");
    }

    #[test]
    fn a_destination_with_no_open_column_refuses_rather_than_guesses() {
        let dest = vec![
            st("doing", "Doing", "active", true),
            st("shipped", "Shipped", "done", false),
        ];
        // Same category still answers…
        assert_eq!(
            pick_move_status(&defaults(), "in_progress", &dest, "Ops", None, false)
                .unwrap()
                .key,
            "doing"
        );
        // …but an inbox ticket has nowhere to go but a closed column.
        assert!(matches!(
            pick_move_status(&defaults(), "inbox", &dest, "Ops", None, false),
            Err(PickRefusal::Bad(m)) if m.contains("no intake column")
        ));
    }

    #[test]
    fn labels_map_by_name_and_the_rest_are_dropped_not_minted() {
        let dest = vec!["Bug".to_string(), "billing".to_string()];
        let tags = vec![
            "bug".to_string(),
            "billing".to_string(),
            "urgent".to_string(),
            "BUG".to_string(),
            " ".to_string(),
            "urgent".to_string(),
        ];
        let (kept, dropped) = map_labels(&tags, &dest);
        assert_eq!(kept, vec!["Bug".to_string(), "billing".to_string()]);
        assert_eq!(dropped, vec!["urgent".to_string()]);
        assert_eq!(map_labels(&[], &dest), (vec![], vec![]));
    }

    #[test]
    fn the_summary_never_reads_as_a_second_ticket_link() {
        // walk_links makes a ticket chip from any object with boardId + id.
        let m = MoveSummary {
            preview: false,
            from_board_id: "b1".into(),
            from_board_name: "A".into(),
            to_board_id: "b2".into(),
            to_board_name: "B".into(),
            from_ref: Some("A-1".into()),
            to_ref: Some("B-7".into()),
            from_status: "inbox".into(),
            status: "triage".into(),
            status_label: "Triage".into(),
            status_how: StatusHow::SameCategory,
            labels_kept: vec![],
            labels_dropped: vec!["x".into()],
            unassigned: vec![],
            parent_detached: true,
            approval_cleared: false,
            dormant_watchers: vec![],
            dependencies: 0,
        };
        let v = serde_json::to_value(&m).unwrap();
        let o = v.as_object().unwrap();
        assert!(!o.contains_key("boardId") && !o.contains_key("id"));
        assert_eq!(v["statusHow"], "same-category");
        assert_eq!(
            activity_line(&m),
            "moved from A (A-1) to B (B-7) · labels dropped (not on B): x · left its parent ticket behind"
        );
    }
}
