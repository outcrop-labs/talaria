// Board statuses — the port of ui/src/server/statuses.ts, whole: the read
// plane (list/meta/diagnostics), the shared invariants (agentStartConflict,
// the required-category rules), and all four writes. The update/delete halves
// MOVE TICKETS through update_task (recategorising a populated sign-off column
// drains it; deleting one reassigns) — never a second writer of tasks.status,
// which is the exact shape those functions exist to avoid. TS reached
// tasks.ts by dynamic import to dodge the cycle; Rust has no cycle problem.

use crate::boards::get_board_agent_config;
use crate::realtime::{BoardEvent, RealtimeDeps, publish_board};
use sqlx::PgPool;
use std::collections::HashSet;

/// Legal on every board but never board COLUMNS, so they carry no category —
/// terminal in every UI that reads them, and agents may not park work there.
/// A board CAN mint a column keyed 'failed' (slug of "Failed") — every
/// resolver here has to agree with the engine about what those two words mean.
pub const OFF_BOARD_STATUSES: &[&str] = &["failed", "cancelled"];

/// The wire row (statuses.ts BoardStatus): id is None for virtual defaults and
/// the system blocked row; `system` rides only on that row, so it is omitted
/// rather than null (TS `system?: boolean` under JSON.stringify).
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardStatus {
    pub id: Option<String>,
    pub key: String,
    pub label: String,
    pub color: String,
    pub category: String, // open | active | review | done | blocked
    pub agent_start: bool,
    pub position: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub system: Option<bool>,
}

/// The system Blocked column — always present, always the same, never stored.
pub fn blocked_status() -> BoardStatus {
    BoardStatus {
        id: None,
        key: "blocked".into(),
        label: "Blocked".into(),
        color: "red".into(),
        category: "blocked".into(),
        agent_start: false,
        position: 0,
        system: Some(true),
    }
}

/// The shipped default set (statuses.ts DEFAULTS) — what a board that never
/// customized serves, and what materialize() copies into rows on first touch.
/// (key, label, color, category, agentStart.)
const DEFAULTS: &[(&str, &str, &str, &str, bool)] = &[
    ("inbox", "Inbox", "slate", "open", false),
    ("assigned", "Assigned", "bronze", "open", true),
    ("in_progress", "In progress", "amber", "active", true),
    (
        "quality_review",
        "Quality review",
        "purple",
        "review",
        false,
    ),
    ("done", "Done", "green", "done", false),
];

type StatusRow = (Option<String>, String, String, String, String, bool, i32);

/// The board's ordered status list, BLOCKED injected after the last
/// active-category status (or mid-list for pure defaults).
pub async fn list_statuses(pg: &PgPool, board_id: &str) -> Result<Vec<BoardStatus>, sqlx::Error> {
    let rows: Vec<StatusRow> = sqlx::query_as(
        "select id::text, key, label, color, category, agent_start, position \
         from board_statuses where board_id = $1::uuid order by position, created_at",
    )
    .bind(board_id)
    .fetch_all(pg)
    .await?;
    let base = if rows.is_empty() {
        DEFAULTS
            .iter()
            .enumerate()
            .map(
                |(i, &(key, label, color, category, agent_start))| BoardStatus {
                    id: None,
                    key: key.into(),
                    label: label.into(),
                    color: color.into(),
                    category: category.into(),
                    agent_start,
                    position: i as i32,
                    system: None,
                },
            )
            .collect()
    } else {
        rows.into_iter()
            .map(
                |(id, key, label, color, category, agent_start, position)| BoardStatus {
                    id,
                    key,
                    label,
                    color,
                    category,
                    agent_start,
                    position,
                    system: None,
                },
            )
            .collect()
    };
    Ok(inject_blocked(base))
}

/// BLOCKED's placement, split out of listStatuses so it is testable without a
/// board: after the last 'active' status, falling back to just-before-review,
/// then to the end. Positions are renumbered over the finished list, exactly
/// as TS's `.map((s, i) => …)` does.
fn inject_blocked(mut base: Vec<BoardStatus>) -> Vec<BoardStatus> {
    let mut at: i64 = -1;
    for (i, s) in base.iter().enumerate() {
        if s.category == "active" {
            at = i as i64;
        }
    }
    if at == -1 {
        at = match base.iter().position(|s| s.category == "review") {
            Some(r) => r as i64 - 1,
            None => base.len() as i64 - 1,
        };
    }
    let mut blocked = blocked_status();
    blocked.position = (at + 1) as i32;
    base.insert((at + 1) as usize, blocked);
    for (i, s) in base.iter_mut().enumerate() {
        s.position = i as i32;
    }
    base
}

/// Workflow metadata the task engine needs, resolved per board (statuses.ts
/// StatusMeta). NO KEY HERE IS A GUESS: every field either names a column this
/// board really has, or is None — resolvers do not invent destinations. The
/// one fail-SAFE exception is `done_keys` (see below). Every destination field
/// is picked from ONE filtered list (`placeable`): never terminal, never the
/// system Blocked column, never re-derived from the raw list.
#[derive(Debug, Clone)]
pub struct StatusMeta {
    pub keys: Vec<String>,
    /// The RAW `agent_start` flag set — the REFUSAL set, deliberately the
    /// widest one: entering any of these is assignment, which an agent may not
    /// do for itself. Never a destination or a serve answer.
    pub agent_start_keys: Vec<String>,
    /// Where an agent's terminal move is redirected: the first review column a
    /// PERSON can actually sign off from. None when the board has no such
    /// column — the caller must say so rather than pick.
    pub review_key: Option<String>,
    /// Every review-category column on the board (possibly empty).
    pub review_keys: Vec<String>,
    /// Every done-category column; falls back to `['done']` when the board has
    /// none — fail-SAFE, because done_keys is only ever read to decide what an
    /// agent may NOT do.
    pub done_keys: Vec<String>,
    /// The board's intake column. Never a terminal key (an owner may legally
    /// label an OPEN column "Cancelled"); None when no usable open column.
    pub default_key: Option<String>,
    /// The board's PICKUP QUEUE: first real agent-start column that is not
    /// terminal and not the reviewer's sign-off queue. None = callers refuse.
    pub assigned_key: Option<String>,
    /// Every column work may be PICKED UP from. `assigned_key` is entry zero.
    /// A review column that also carries agent_start (legacy) is NOT one: the
    /// review-exit rule freezes anything parked there.
    pub pickup_keys: Vec<String>,
    /// Every column a WORK SESSION may continue in: pickup queues plus the
    /// active columns. The one answer to "is this ticket still in play?".
    pub working_keys: Vec<String>,
    /// The board's first WORKING column — where an agent parks a ticket while
    /// it works, where a revision bounces to. None when the board has no
    /// active column; callers fall back to `assigned_key` or say so.
    pub active_key: Option<String>,
}

impl StatusMeta {
    /// Is this key TERMINAL on this board — a done-category column, or one of
    /// the off-board keys that are terminal everywhere. The one definition;
    /// callers must not spell it by hand (a function cannot be half-copied).
    pub fn terminal(&self, key: &str) -> bool {
        self.done_keys.iter().any(|k| k == key) || OFF_BOARD_STATUSES.contains(&key)
    }
}

/// The resolution, split out of statusMeta so the rules are testable against a
/// synthetic list. Field-by-field port of statuses.ts statusMeta.
fn meta_of(list: &[BoardStatus]) -> StatusMeta {
    let agent_start: Vec<String> = list
        .iter()
        .filter(|s| s.agent_start)
        .map(|s| s.key.clone())
        .collect();
    let reviews: Vec<String> = list
        .iter()
        .filter(|s| s.category == "review")
        .map(|s| s.key.clone())
        .collect();
    let done: Vec<String> = list
        .iter()
        .filter(|s| s.category == "done")
        .map(|s| s.key.clone())
        .collect();
    let done_keys = if done.is_empty() {
        vec!["done".to_string()]
    } else {
        done
    };
    let is_terminal = |k: &str| done_keys.iter().any(|d| d == k) || OFF_BOARD_STATUSES.contains(&k);
    // One filtered list, every destination picks from it: never terminal,
    // never the system Blocked column (list_statuses injects it, so it can be
    // list[0]).
    let placeable: Vec<&BoardStatus> = list
        .iter()
        .filter(|s| s.category != "blocked" && !is_terminal(&s.key))
        .collect();
    let pick = |pred: &dyn Fn(&BoardStatus) -> bool, placeable: &Vec<&BoardStatus>| {
        placeable.iter().find(|s| pred(s)).map(|s| s.key.clone())
    };
    // A review column is the human sign-off queue at BOTH ends: an agent may
    // not move a ticket out of one, so a review column can never be somewhere
    // work is picked up or continued — whatever its agent_start flag says.
    let pickup_keys: Vec<String> = placeable
        .iter()
        .filter(|s| s.agent_start && s.category != "review")
        .map(|s| s.key.clone())
        .collect();
    let working_keys: Vec<String> = placeable
        .iter()
        .filter(|s| (s.agent_start || s.category == "active") && s.category != "review")
        .map(|s| s.key.clone())
        .collect();
    StatusMeta {
        keys: list.iter().map(|s| s.key.clone()).collect(),
        agent_start_keys: agent_start,
        review_key: pick(&|s| s.category == "review" && !s.agent_start, &placeable),
        review_keys: reviews,
        done_keys,
        default_key: pick(&|s| s.category == "open", &placeable),
        assigned_key: pickup_keys.first().cloned(),
        pickup_keys,
        working_keys,
        active_key: pick(&|s| s.category == "active", &placeable),
    }
}

pub async fn status_meta(pg: &PgPool, board_id: &str) -> Result<StatusMeta, sqlx::Error> {
    Ok(meta_of(&list_statuses(pg, board_id).await?))
}

/// A board-shape problem, in one sentence, addressed to the person who can fix
/// it. `error` = something the board cannot do at all; `warning` = something
/// it will do, silently, that is probably not what was meant.
#[derive(Debug, serde::Serialize)]
pub struct StatusDiagnostic {
    pub level: String,
    pub text: String,
}

/// WHAT THE BOARD OWNER CANNOT OTHERWISE SEE (statuses.ts statusDiagnostics).
/// Derived, not re-derived: every judgment reads statusMeta's resolved fields,
/// so a board is called broken here exactly when the engine will refuse. The
/// agent-workflow diagnostics are conditional on the board permitting agents;
/// everything else is wrong for every board and stays unconditional.
pub async fn status_diagnostics(
    pg: &PgPool,
    board_id: &str,
) -> Result<Vec<StatusDiagnostic>, sqlx::Error> {
    let list = list_statuses(pg, board_id).await?;
    let meta = status_meta(pg, board_id).await?;
    let mut out: Vec<StatusDiagnostic> = Vec::new();
    let name = |key: &str| -> String {
        let label = list.iter().find(|s| s.key == key).map(|s| s.label.as_str());
        format!("“{}”", label.unwrap_or(key))
    };
    let cols =
        |keys: &[String]| -> String { keys.iter().map(|k| name(k)).collect::<Vec<_>>().join(", ") };

    // Data that predates the cross-validation. agentStartConflict is the same
    // rule the writes use, so this panel calls a row illegal exactly when a
    // write of it would be refused; the two categories break DIFFERENT things.
    let legacy: Vec<&BoardStatus> = list
        .iter()
        .filter(|s| {
            s.category != "blocked" && agent_start_conflict(&s.category, s.agent_start).is_some()
        })
        .collect();
    for s in &legacy {
        let consequence = if s.category == "review" {
            "so it is not a column an agent can hand work off to, and hand-offs that would land here are refused"
        } else {
            "so moving a ticket into it fires a fresh dispatch on the work a person just closed"
        };
        out.push(StatusDiagnostic {
            level: "error".into(),
            text: format!(
                "“{}” is a {} column and also flagged “agent start” — {}. That pairing is refused \
                 on new edits but survives from before the rule and there is no migration for it, \
                 so it stays until someone clears the box. Clear “agent start” on it.",
                s.label, s.category, consequence
            ),
        });
    }
    if board_permits_agents(pg, board_id).await? {
        if meta.review_keys.is_empty() {
            out.push(StatusDiagnostic {
                level: "error".into(),
                text: "This board has no review column, so an agent that finishes a ticket has \
                       nowhere to hand it — it cannot close the ticket itself, and the hand-off is \
                       refused. Add a column with the category \"review\", or turn agents off for \
                       this board."
                    .into(),
            });
        } else if meta.review_key.is_none() {
            // Only the review columns the line above has not already accounted
            // for — otherwise a board with one legacy row says the same thing
            // twice.
            let unexplained: Vec<String> = meta
                .review_keys
                .iter()
                .filter(|k| !legacy.iter().any(|s| s.key == **k))
                .cloned()
                .collect();
            if !unexplained.is_empty() {
                out.push(StatusDiagnostic {
                    level: "error".into(),
                    text: format!(
                        "{} cannot receive a hand-off: a review column that is itself terminal \
                         (its key means \"off the board\") is not somewhere work can be signed off \
                         from. No agent can hand work off on this board until one review column \
                         can hold it.",
                        cols(&unexplained)
                    ),
                });
            }
        }
        if meta.pickup_keys.is_empty() {
            out.push(StatusDiagnostic {
                level: "warning".into(),
                text: "No column is a usable agent-start queue, so agents will never pick work \
                       up on this board. Tick \"agent start\" on the intake or active column \
                       where assigning a ticket should count as approval to begin."
                    .into(),
            });
        }
    }
    if meta.default_key.is_none() {
        out.push(StatusDiagnostic {
            level: "error".into(),
            text: "This board has no intake column, so new tickets have nowhere to land and \
                   creating one fails. Add a column with the category \"intake\"."
                .into(),
        });
    }
    // A column whose SLUG collides with an off-board key is terminal everywhere
    // in Talaria, whatever category the owner chose — so it silently drops out
    // of every destination the engine can resolve. Renaming does not move a
    // key; only a new column does.
    let off_board: Vec<&BoardStatus> = list
        .iter()
        .filter(|s| {
            s.category != "blocked"
                && s.category != "done"
                && OFF_BOARD_STATUSES.contains(&s.key.as_str())
        })
        .collect();
    if !off_board.is_empty() {
        let one = off_board.len() == 1;
        let keys_listed = off_board
            .iter()
            .map(|s| format!("“{}”", s.key))
            .collect::<Vec<_>>()
            .join(", ");
        let key_names = off_board.iter().map(|s| s.key.clone()).collect::<Vec<_>>();
        out.push(StatusDiagnostic {
            level: "warning".into(),
            text: format!(
                "{} {} {}, which {} “off the board” everywhere in Talaria — tickets there are \
                 closed to agents and nothing routes work into {}, whatever category {}. A \
                 column’s key is fixed at creation, so replace {} with a differently-named column.",
                cols(&key_names),
                if one { "has the key" } else { "have the keys" },
                keys_listed,
                if one { "means" } else { "mean" },
                if one { "it" } else { "them" },
                if one { "it has" } else { "they have" },
                if one { "it" } else { "them" },
            ),
        });
    }
    Ok(out)
}

/// SQL fragment: `t.status` is in the given CATEGORY on the ticket's own board,
/// with the legacy fallback for never-customized boards (statuses.ts
/// statusCategorySql).
///
/// Interpolate only with a LITERAL category + fallback list — no user input —
/// which is the same contract the TS template carries; the callers here are
/// compile-time strings.
pub fn status_category_sql(category: &str, legacy_keys: &[&str]) -> String {
    let keys = legacy_keys
        .iter()
        .map(|k| format!("'{k}'"))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "( t.status in (select bs.key from board_statuses bs where bs.board_id = t.board_id and \
         bs.category = '{category}') or ( not exists (select 1 from board_statuses bs where \
         bs.board_id = t.board_id) and t.status in ({keys}) ) )"
    )
}

/// First customization COPIES the defaults into rows so edits are complete.
async fn materialize(pg: &PgPool, board_id: &str) -> Result<(), sqlx::Error> {
    let existing: Option<(i32,)> =
        sqlx::query_as("select 1 from board_statuses where board_id = $1::uuid limit 1")
            .bind(board_id)
            .fetch_optional(pg)
            .await?;
    if existing.is_some() {
        return Ok(());
    }
    for (i, &(key, label, color, category, agent_start)) in DEFAULTS.iter().enumerate() {
        sqlx::query(
            "insert into board_statuses (board_id, key, label, color, category, agent_start, \
             position) values ($1::uuid, $2, $3, $4, $5, $6, $7) on conflict do nothing",
        )
        .bind(board_id)
        .bind(key)
        .bind(label)
        .bind(color)
        .bind(category)
        .bind(agent_start)
        .bind(i as i32)
        .execute(pg)
        .await?;
    }
    Ok(())
}

/// The slug of a label (statuses.ts `slug`): lowercase, runs of anything but
/// [a-z0-9] become one '_', leading/trailing '_' stripped, 40 chars, with
/// 'status' as the floor.
fn slug(label: &str) -> String {
    let mut out = String::new();
    let mut underscore = false;
    for c in label.trim().to_lowercase().chars() {
        if c.is_ascii_lowercase() || c.is_ascii_digit() {
            out.push(c);
            underscore = false;
        } else if !out.is_empty() && !underscore {
            out.push('_');
            underscore = true;
        }
    }
    let s = out.trim_matches('_');
    let s: String = s.chars().take(40).collect();
    if s.is_empty() { "status".into() } else { s }
}

/// The key a new column gets for `label`: the slug, deflected off the two key
/// namespaces it must never collide with — the system Blocked column, and the
/// off-board keys that are terminal everywhere. The label the owner typed is
/// kept; only the internal key moves.
fn mint_key(label: &str) -> String {
    let mut key = slug(label);
    if key == "blocked" {
        key = "blocked_2".into();
    }
    if OFF_BOARD_STATUSES.contains(&key.as_str()) {
        key = format!("{key}_2");
    }
    key
}

// ── Structural invariants ────────────────────────────────────────────────────

/// The workflow positions a board cannot do without, and the word the refusal
/// uses for each.
const REQUIRED_CATEGORIES: &[(&str, &str)] = &[
    ("open", "intake"),
    ("review", "review"),
    ("done", "terminal"),
];

/// …and the one of them that is required BY THE AGENTS, not by the workflow.
/// Intake and terminal are needed by every board; review is the agent HAND-OFF
/// TARGET, so the rule follows the dependency: absolute for a board that
/// permits agents, absent for one that does not. Safe to make conditional
/// precisely because the engine REFUSES when the column is missing — there is
/// no path to a hand-off laundered into a done column.
const AGENT_REQUIRED_CATEGORIES: &[&str] = &["review"];

/// The refusal, for a board that DOES permit agents. It explains both halves —
/// why agents need the column, and that turning agents off is the other way to
/// get what the owner asked for.
pub const REVIEW_REQUIRED_FOR_AGENTS: &str = "this is the board’s last review column, and agents are allowed to work this board. An agent may never sign off its own work, so it hands a finished ticket to a review column for a person — with none, every hand-off is refused and an agent here can start work it can never finish. Keep one review column, or turn agents off for this board (board settings → agents), after which a review column is no longer required.";

/// Does this board admit agents at all — the board's own policy, allow-all or
/// an explicit allow-list (statuses.ts boardPermitsAgents; TS dynamic-imports
/// boards.ts to dodge a cycle, which Rust does not have).
async fn board_permits_agents(pg: &PgPool, board_id: &str) -> Result<bool, sqlx::Error> {
    let cfg = get_board_agent_config(pg, board_id).await?;
    Ok(cfg.allow_all || !cfg.models.is_empty())
}

/// Refuse an operation that would leave this board with no column of
/// `category` (the row being deleted, or recategorised away, is `except_id`).
/// The inner Err is TS's thrown sentence, answered as a 400 by the route.
pub async fn assert_not_last_of_category(
    pg: &PgPool,
    board_id: &str,
    except_id: &str,
    category: &str,
) -> Result<Result<(), String>, sqlx::Error> {
    let Some((_, noun)) = REQUIRED_CATEGORIES.iter().find(|(c, _)| *c == category) else {
        return Ok(Ok(()));
    };
    let others: Option<(i32,)> = sqlx::query_as(
        "select 1 from board_statuses where board_id = $1::uuid and category = $2 \
         and id <> $3::uuid limit 1",
    )
    .bind(board_id)
    .bind(category)
    .bind(except_id)
    .fetch_optional(pg)
    .await?;
    if others.is_some() {
        return Ok(Ok(()));
    }
    if AGENT_REQUIRED_CATEGORIES.contains(&category) && !board_permits_agents(pg, board_id).await? {
        return Ok(Ok(()));
    }
    Ok(Err(if AGENT_REQUIRED_CATEGORIES.contains(&category) {
        REVIEW_REQUIRED_FOR_AGENTS.into()
    } else {
        format!("the workflow needs at least one {noun} status — this is the board’s last one")
    }))
}

/// `review` and `agentStart` are opposite ends of the same handover: a review
/// column is the queue a PERSON signs work off from, an agent-start column is
/// the queue agents pick work UP from. Flagged both, an agent’s hand-off drops
/// straight back into its own pickup queue.
pub const REVIEW_AGENT_START_CONFLICT: &str = "a review column is where a person signs work off — it cannot also be an agent-start column agents pick work up from. Clear \"agent start\" on this column in the same change to make it a review column.";

/// The sibling rule: `agent_start` on a DONE column turns CLOSING a ticket
/// into a dispatch. Terminal and pickup are opposite ends of the same handover,
/// exactly like review and pickup.
pub const DONE_AGENT_START_CONFLICT: &str = "a done column is where work is closed — it cannot also be an agent-start column agents pick work up from, or closing a ticket would dispatch fresh work on it. Clear \"agent start\" on this column in the same change to make it a done column.";

/// The one rule, stated once, over the EFFECTIVE post-patch column: an
/// agent-start pickup queue is neither human gate. Returns the refusal, or
/// None when the pair is fine.
pub fn agent_start_conflict(category: &str, agent_start: bool) -> Option<&'static str> {
    if !agent_start {
        return None;
    }
    if category == "review" {
        return Some(REVIEW_AGENT_START_CONFLICT);
    }
    if category == "done" {
        return Some(DONE_AGENT_START_CONFLICT);
    }
    None
}

/// statuses.ts createStatus. The inner Err is TS's thrown conflict sentence.
/// Key-addressed boards get the defaults materialized first; the key is minted
/// off the label with the two namespace deflections, and a clash with a live
/// row gets a four-char suffix (TS: four base-36 chars from Math.random; the
/// uuid's hex tail is the same shape — four lowercase alphanumerics, a fresh
/// ~1.6M space per attempt — without pulling an RNG into the crate).
pub async fn create_status(
    pg: &PgPool,
    realtime: &RealtimeDeps,
    board_id: &str,
    label: &str,
    color: Option<&str>,
    category: Option<&str>,
    agent_start: Option<bool>,
) -> Result<Result<BoardStatus, String>, sqlx::Error> {
    materialize(pg, board_id).await?;
    let category = category.unwrap_or("active");
    let agent_start = agent_start.unwrap_or(false);
    if let Some(conflict) = agent_start_conflict(category, agent_start) {
        return Ok(Err(conflict.into()));
    }
    let mut key = mint_key(label);
    let clash: Option<(i32,)> =
        sqlx::query_as("select 1 from board_statuses where board_id = $1::uuid and key = $2")
            .bind(board_id)
            .bind(&key)
            .fetch_optional(pg)
            .await?;
    if clash.is_some() {
        key = format!("{key}_{}", &uuid::Uuid::new_v4().simple().to_string()[..4]);
    }
    let row: StatusRow = sqlx::query_as(
        "insert into board_statuses (board_id, key, label, color, category, agent_start, \
         position) values ($1::uuid, $2, $3, $4, $5, $6, \
         coalesce((select max(position) + 1 from board_statuses where board_id = $1::uuid), 0)) \
         returning id::text, key, label, color, category, agent_start, position",
    )
    .bind(board_id)
    .bind(&key)
    .bind(label.trim())
    .bind(color.unwrap_or("slate"))
    .bind(category)
    .bind(agent_start)
    .fetch_one(pg)
    .await?;
    publish_board(
        realtime,
        board_id,
        &BoardEvent {
            kind_tag: "board",
            task_id: None,
            deleted: None,
        },
    );
    let (id, key, label, color, category, agent_start, position) = row;
    Ok(Ok(BoardStatus {
        id,
        key,
        label,
        color,
        category,
        agent_start,
        position,
        system: None,
    }))
}

/// The permutation check under reorderStatuses, split out so it is testable:
/// an order that names a key the board does not have, or leaves one of its
/// columns out, was once accepted with a 200 and quietly produced an order
/// nobody asked for. The refusal names every part of the mismatch at once,
/// rather than failing on the first. The system Blocked column is not a row
/// and is never listed here — list_statuses places it — so naming it is an
/// unknown key like any other.
fn order_refusal(have: &[String], keys: &[String]) -> Option<String> {
    let have_set: HashSet<&String> = have.iter().collect();
    let mut seen: HashSet<String> = HashSet::new();
    let mut unknown: Vec<String> = Vec::new();
    let mut twice: Vec<String> = Vec::new();
    for k in keys {
        if !have_set.contains(k) {
            unknown.push(k.clone());
        } else if seen.contains(k) {
            twice.push(k.clone());
        }
        seen.insert(k.clone());
    }
    let missing: Vec<String> = have
        .iter()
        .filter(|k| !seen.contains(*k))
        .cloned()
        .collect();
    if unknown.is_empty() && twice.is_empty() && missing.is_empty() {
        return None;
    }
    let q = |ks: &[String]| -> String {
        let mut deduped: Vec<String> = Vec::new();
        for k in ks {
            if !deduped.iter().any(|d| d == k) {
                deduped.push(k.clone());
            }
        }
        deduped
            .iter()
            .map(|k| format!("“{k}”"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    let mut parts: Vec<String> = Vec::new();
    if !unknown.is_empty() {
        parts.push(format!(
            "{} {} on this board",
            q(&unknown),
            if unknown.len() == 1 {
                "is not a column"
            } else {
                "are not columns"
            }
        ));
    }
    if !twice.is_empty() {
        parts.push(format!(
            "{} {} more than once",
            q(&twice),
            if twice.len() == 1 {
                "is listed"
            } else {
                "are listed"
            }
        ));
    }
    if !missing.is_empty() {
        parts.push(format!(
            "{} {} missing from the order",
            q(&missing),
            if missing.len() == 1 { "is" } else { "are" }
        ));
    }
    Some(format!(
        "a new column order has to list every column on this board exactly once — {}. Nothing \
         was reordered.",
        parts.join("; ")
    ))
}

/// Replace the whole order (array of status KEYS in the new order). The
/// validation is the permutation check above; the writes ARE a transaction,
/// unlike the drain — this touches nothing but board_statuses.position, so
/// there is nothing outside the database to leave half-done.
pub async fn reorder_statuses(
    pg: &PgPool,
    realtime: &RealtimeDeps,
    board_id: &str,
    keys: &[String],
) -> Result<Result<(), String>, sqlx::Error> {
    materialize(pg, board_id).await?;
    let rows: Vec<(String,)> =
        sqlx::query_as("select key from board_statuses where board_id = $1::uuid")
            .bind(board_id)
            .fetch_all(pg)
            .await?;
    let have: Vec<String> = rows.into_iter().map(|(k,)| k).collect();
    if let Some(refusal) = order_refusal(&have, keys) {
        return Ok(Err(refusal));
    }
    let mut tx = pg.begin().await?;
    for (i, k) in keys.iter().enumerate() {
        sqlx::query(
            "update board_statuses set position = $1 where key = $2 and board_id = $3::uuid",
        )
        .bind(i as i32)
        .bind(k)
        .bind(board_id)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    publish_board(
        realtime,
        board_id,
        &BoardEvent {
            kind_tag: "board",
            task_id: None,
            deleted: None,
        },
    );
    Ok(Ok(()))
}

// ── The update/delete writes ─────────────────────────────────────────────────
// Both MOVE TICKETS, which is why they waited for the tasks slice: the moves
// go through `update_task` — never a second writer of tasks.status — and the
// actor who reshaped the column owns them on every ticket's activity log.

/// updateStatus's patch — every field present-or-absent (no clears).
pub struct StatusPatch {
    pub label: Option<String>,
    pub color: Option<String>,
    pub category: Option<String>,
    pub agent_start: Option<bool>,
    pub position: Option<i32>,
}

/// statuses.ts updateStatus. Key-addressed: boards that never customized
/// serve VIRTUAL defaults with no row ids, so the stable handle is the status
/// KEY — materialize turns the virtual set into rows on first touch, then the
/// key resolves.
///
/// `actor` is required for the same reason delete_status requires one:
/// recategorising a populated sign-off column MOVES TICKETS, and a ticket move
/// belongs to the person who caused it. Optional would mean a caller could
/// forget, and the moves would land on the record unattributed.
pub async fn update_status(
    deps: &crate::tasks::TaskDeps,
    board_id: &str,
    key: &str,
    patch: &StatusPatch,
    actor: &str,
) -> Result<Result<(), String>, sqlx::Error> {
    let pg = &deps.pg;
    if key == "blocked" {
        return Ok(Err("Blocked is a system status".into()));
    }
    materialize(pg, board_id).await?;
    let cur: Option<(String, String, String, bool)> = sqlx::query_as(
        "select id::text, label, category, agent_start from board_statuses \
         where key = $1 and board_id = $2::uuid",
    )
    .bind(key)
    .bind(board_id)
    .fetch_optional(pg)
    .await?;
    // A key that names nothing used to no-op and report success — a silent
    // failure, and one that hides a typo'd guard rather than surfacing it.
    let Some((row_id, cur_label, cur_category, cur_agent_start)) = cur else {
        return Ok(Err(format!("\"{key}\" is not a status on this board")));
    };
    // The effective post-patch column, so neither flag can arrive alone and
    // slip the pair past the check. FIRST, before anything below writes: the
    // drain moves tickets, and a write that is going to be refused must not
    // move any. `PUT {review column, category:'done', agentStart:true}` is
    // exactly that write — legal-looking until the pair is checked, and its
    // drain would have emptied the column on the way to a 400.
    let effective_category = patch.category.clone().unwrap_or(cur_category.clone());
    let effective_agent_start = patch.agent_start.unwrap_or(cur_agent_start);
    if let Some(conflict) = agent_start_conflict(&effective_category, effective_agent_start) {
        return Ok(Err(conflict.into()));
    }
    // Recategorising is the same loss as deleting: symmetric with
    // delete_status. …and it is the same ticket move as emptying the column
    // by hand, so it is made, not refused — through update_task, exactly as
    // delete_status does.
    if let Some(new_category) = &patch.category
        && *new_category != cur_category
    {
        if let Err(sentence) =
            assert_not_last_of_category(pg, board_id, &row_id, &cur_category).await?
        {
            return Ok(Err(sentence));
        }
        if let Err(sentence) = drain_signoff_column(
            deps,
            board_id,
            key,
            &cur_label,
            &row_id,
            &cur_category,
            new_category,
            actor,
        )
        .await?
        {
            return Ok(Err(sentence));
        }
    }
    // ONE statement, not five. `category` and `agent_start` are read TOGETHER
    // by agent_start_conflict, so five separate writes meant a failure between
    // them could leave the pair the checked patch was validated against split
    // across the column — a review column still flagged agent-start, refused
    // on every edit but sitting there. Addressed by id: the row was resolved
    // above.
    let mut frags: Vec<String> = Vec::new();
    if patch.label.is_some() {
        frags.push(format!("label = ${}", frags.len() + 1));
    }
    if patch.color.is_some() {
        frags.push(format!("color = ${}", frags.len() + 1));
    }
    if patch.category.is_some() {
        frags.push(format!("category = ${}", frags.len() + 1));
    }
    if patch.agent_start.is_some() {
        frags.push(format!("agent_start = ${}", frags.len() + 1));
    }
    if patch.position.is_some() {
        frags.push(format!("position = ${}", frags.len() + 1));
    }
    if !frags.is_empty() {
        // AssertSqlSafe: the placeholders are this function's own set list;
        // every value stays a bind.
        let sql = format!(
            "update board_statuses set {} where id = ${}",
            frags.join(", "),
            frags.len() + 1
        );
        let mut q = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()));
        if let Some(l) = &patch.label {
            q = q.bind(l.trim());
        }
        if let Some(c) = &patch.color {
            q = q.bind(c);
        }
        if let Some(c) = &patch.category {
            q = q.bind(c);
        }
        if let Some(a) = patch.agent_start {
            q = q.bind(a);
        }
        if let Some(p) = patch.position {
            q = q.bind(p);
        }
        q.bind(&row_id).execute(pg).await?;
    }
    publish_board(
        &deps.realtime,
        board_id,
        &BoardEvent {
            kind_tag: "board",
            task_id: None,
            deleted: None,
        },
    );
    Ok(Ok(()))
}

/// What a sign-off category HOLDS, for the refusal sentences (statuses.ts
/// SIGNOFF_CATEGORIES).
fn signoff_what(category: &str) -> Option<&'static str> {
    match category {
        "review" => Some("waiting for sign-off"),
        "done" => Some("a person signed off on"),
        _ => None,
    }
}

/// statuses.ts drainSignoffColumn — recategorising a populated sign-off
/// column must not release its tickets. With exactly ONE sibling of the same
/// kind the move is made for the owner (each ticket through update_task, so
/// activity/completed_at/watchers/dispatch all happen); with none or several
/// it is REFUSED and the sentence makes the owner choose, because which
/// column those tickets belong in is a person's call.
///
/// The sibling-count sentences and the stopped-mid-drain sentence are the
/// product's, verbatim; the stopped one matters most — a drain that failed
/// halfway says exactly where it stopped and that re-asking carries on.
#[allow(clippy::too_many_arguments)] // the drain's inputs are the column's — all eight name one
async fn drain_signoff_column(
    deps: &crate::tasks::TaskDeps,
    board_id: &str,
    key: &str,
    label: &str,
    row_id: &str,
    from: &str,
    to: &str,
    actor: &str,
) -> Result<Result<(), String>, sqlx::Error> {
    if from == to {
        return Ok(Ok(()));
    }
    let Some(what) = signoff_what(from) else {
        return Ok(Ok(()));
    };
    let pg = &deps.pg;
    let held: Vec<(String,)> = sqlx::query_as(
        "select id::text from tasks where board_id = $1::uuid and status = $2 \
         order by created_at asc",
    )
    .bind(board_id)
    .bind(key)
    .fetch_all(pg)
    .await?;
    let n = held.len();
    if n == 0 {
        return Ok(Ok(()));
    }
    let one = n == 1;
    let tickets = format!("{n} ticket{}", if one { "" } else { "s" });
    let siblings: Vec<(String, String)> = sqlx::query_as(
        "select key, label from board_statuses \
         where board_id = $1::uuid and category = $2 and id <> $3::uuid \
         order by position, created_at",
    )
    .bind(board_id)
    .bind(from)
    .bind(row_id)
    .fetch_all(pg)
    .await?;
    if siblings.is_empty() {
        return Ok(Err(format!(
            "\"{label}\" still holds {tickets} {what}, and it is this board's only {from} column \
             — there is nowhere of the same kind to move {} to, so changing the category here \
             would release {} with nothing on the record. Move {} first, then change the \
             category.",
            if one { "it" } else { "them" },
            if one { "it" } else { "them" },
            if one { "that ticket" } else { "those tickets" },
        )));
    }
    if siblings.len() > 1 {
        let cols = siblings
            .iter()
            .map(|(_, l)| format!("\"{l}\""))
            .collect::<Vec<_>>()
            .join(", ");
        return Ok(Err(format!(
            "\"{label}\" still holds {tickets} {what}, and this board has {} other {from} columns \
             ({cols}) — which one {} in is your call, not ours. Move {} first, then change the \
             category.",
            siblings.len(),
            if one { "it belongs" } else { "they belong" },
            if one { "that ticket" } else { "those tickets" },
        )));
    }
    let (dest_key, dest_label) = &siblings[0];
    // BEFORE the category changes, for the same reason delete_status moves
    // before it drops the row: update_task reads the board's columns as they
    // are.
    let mut moved = 0usize;
    for (i, (ticket,)) in held.iter().enumerate() {
        let patch = crate::tasks::TaskPatch {
            status: Some(dest_key.clone()),
            status_note: Some(format!(
                "the \"{label}\" column was recategorised as \"{to}\""
            )),
            ..Default::default()
        };
        match crate::tasks::update_task(deps, ticket, patch, &crate::tasks::TaskActor::human(actor))
            .await
        {
            Ok(_) => {}
            Err(e) => {
                let sofar = if moved > 0 {
                    format!(
                        "{moved} of {tickets} had already moved to \"{dest_label}\", where {} \
                         still {what}",
                        if moved == 1 { "it is" } else { "they are" }
                    )
                } else {
                    "no ticket moved".to_string()
                };
                return Ok(Err(format!(
                    "\"{label}\" was not recategorised: moving its tickets into \"{dest_label}\" \
                     stopped on one of them — {}. {sofar}, and \"{label}\" still has its {from} \
                     category, so nothing was released. Make the same change again once that is \
                     fixed: the tickets already moved are out of this column, so it carries on \
                     from where it stopped.",
                    e.message()
                )));
            }
        }
        moved = i + 1;
    }
    Ok(Ok(()))
}

/// statuses.ts deleteStatus. The last intake and the last terminal column are
/// protected on every board, and the last review catch on every board that
/// permits agents — status_meta resolves each of those to a real key for
/// whoever needs it.
///
/// THE REASSIGNMENT IS AN ORDINARY STATUS MOVE, and `actor` is why this takes
/// one. A bulk `update tasks set status = …` writes no activity row, never
/// stamps completed_at when the tickets land in a done column, tells no
/// watcher, and never reaches the push side when they land in a pickup queue
/// — four things update_task does, that a second writer of tasks.status has
/// to remember. There is no second writer: each ticket moves through
/// update_task, as the person who deleted the column, and inherits all four.
/// N updates instead of one statement is the price, and deleting a populated
/// column is a rare admin action; being wrong four ways was the alternative.
pub async fn delete_status(
    deps: &crate::tasks::TaskDeps,
    board_id: &str,
    key: &str,
    reassign_to: &str,
    actor: &str,
) -> Result<Result<(), String>, sqlx::Error> {
    let pg = &deps.pg;
    if key == "blocked" {
        return Ok(Err("Blocked is a system status".into()));
    }
    materialize(pg, board_id).await?;
    let victim: Option<(String, String, String, String)> = sqlx::query_as(
        "select id::text, key, label, category from board_statuses \
         where key = $1 and board_id = $2::uuid",
    )
    .bind(key)
    .bind(board_id)
    .fetch_optional(pg)
    .await?;
    // Same as update_status, and it was the asymmetric half: a key that names
    // nothing used to return 200 {ok:true} here, so DELETE of a typo'd or
    // already-deleted column reported that a column had been removed and its
    // tickets reassigned when neither had happened.
    let Some((victim_id, victim_key, victim_label, victim_category)) = victim else {
        return Ok(Err(format!("\"{key}\" is not a status on this board")));
    };
    if let Err(sentence) =
        assert_not_last_of_category(pg, board_id, &victim_id, &victim_category).await?
    {
        return Ok(Err(sentence));
    }
    let meta = status_meta(pg, board_id).await?;
    if !meta.keys.iter().any(|k| k == reassign_to) || reassign_to == victim_key {
        return Ok(Err("pick a surviving status for its tickets".into()));
    }
    let doomed: Vec<(String,)> = sqlx::query_as(
        "select id::text from tasks where board_id = $1::uuid and status = $2 \
         order by created_at asc",
    )
    .bind(board_id)
    .bind(&victim_key)
    .fetch_all(pg)
    .await?;
    // BEFORE the column is dropped: update_task refuses a status the board
    // does not have, and while this move is legal the one being vacated must
    // still resolve for the ticket it is reading. A refused move (TS lets the
    // throw carry out of the loop) names the ticket that could not move.
    for (ticket,) in &doomed {
        let patch = crate::tasks::TaskPatch {
            status: Some(reassign_to.to_string()),
            status_note: Some(format!("the \"{victim_label}\" column was deleted")),
            ..Default::default()
        };
        match crate::tasks::update_task(deps, ticket, patch, &crate::tasks::TaskActor::human(actor))
            .await
        {
            Ok(_) => {}
            Err(e) => return Ok(Err(e.message())),
        }
    }
    sqlx::query("delete from board_statuses where id = $1::uuid")
        .bind(&victim_id)
        .execute(pg)
        .await?;
    publish_board(
        &deps.realtime,
        board_id,
        &BoardEvent {
            kind_tag: "board",
            task_id: None,
            deleted: None,
        },
    );
    Ok(Ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn status(key: &str, label: &str, category: &str, agent_start: bool) -> BoardStatus {
        BoardStatus {
            id: None,
            key: key.into(),
            label: label.into(),
            color: "slate".into(),
            category: category.into(),
            agent_start,
            position: 0,
            system: None,
        }
    }

    #[test]
    fn fragment_matches_the_ts_composition() {
        assert_eq!(
            status_category_sql("review", &["quality_review"]),
            "( t.status in (select bs.key from board_statuses bs where bs.board_id = t.board_id \
             and bs.category = 'review') or ( not exists (select 1 from board_statuses bs where \
             bs.board_id = t.board_id) and t.status in ('quality_review') ) )"
        );
        // A second legacy key joins with a comma, as the TS `.map().join()` does.
        assert!(status_category_sql("done", &["done", "closed"]).contains("'done', 'closed'"));
    }

    #[test]
    fn slug_matches_the_ts_pipeline() {
        assert_eq!(slug("In progress"), "in_progress");
        assert_eq!(slug("  Hi! There  "), "hi_there");
        assert_eq!(slug("Hi!!!  ---  There"), "hi_there"); // runs collapse to one _
        assert_eq!(slug("!?!"), "status"); // stripped to nothing → floor
        assert_eq!(slug("_leading"), "leading"); // leading _ trimmed
        assert_eq!(slug("Résumé"), "r_sum"); // non-ascii is a separator, as TS's regex is
        // 40-char cap, applied after the trim — TS slice(0,40) can cut on the
        // separator and leave a trailing '_', and so does this.
        assert_eq!(slug("a".repeat(50).as_str()), "a".repeat(40));
        assert_eq!(
            slug("a?b?c?d?e?f?g?h?i?j?k?l?m?n?o?p?q?r?s?t?u"),
            "a_b_c_d_e_f_g_h_i_j_k_l_m_n_o_p_q_r_s_t_"
        );
    }

    #[test]
    fn mint_key_deflects_the_two_reserved_namespaces() {
        assert_eq!(mint_key("Blocked"), "blocked_2");
        assert_eq!(mint_key("Cancelled"), "cancelled_2");
        assert_eq!(mint_key("Failed"), "failed_2");
        assert_eq!(mint_key("In progress"), "in_progress");
    }

    #[test]
    fn blocked_lands_after_the_last_active_column() {
        let defaults = DEFAULTS
            .iter()
            .enumerate()
            .map(
                |(i, &(key, label, color, category, agent_start))| BoardStatus {
                    id: None,
                    key: key.into(),
                    label: label.into(),
                    color: color.into(),
                    category: category.into(),
                    agent_start,
                    position: i as i32,
                    system: None,
                },
            )
            .collect();
        let out = inject_blocked(defaults);
        let keys: Vec<&str> = out.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(
            keys,
            vec![
                "inbox",
                "assigned",
                "in_progress",
                "blocked",
                "quality_review",
                "done"
            ]
        );
        for (i, s) in out.iter().enumerate() {
            assert_eq!(s.position, i as i32);
        }
        assert_eq!(out[3].system, Some(true));
    }

    #[test]
    fn blocked_falls_back_to_before_review_then_end() {
        // No active column: just before the first review column.
        let list = vec![
            status("inbox", "Inbox", "open", false),
            status("quality_review", "QA", "review", false),
        ];
        let out = inject_blocked(list);
        let keys: Vec<&str> = out.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(keys, vec!["inbox", "blocked", "quality_review"]);

        // No active and no review: the end.
        let list = vec![
            status("inbox", "Inbox", "open", false),
            status("done", "Done", "done", false),
        ];
        let out = inject_blocked(list);
        let keys: Vec<&str> = out.iter().map(|s| s.key.as_str()).collect();
        assert_eq!(keys, vec!["inbox", "done", "blocked"]);
    }

    #[test]
    fn meta_resolves_the_default_board() {
        let list = inject_blocked(vec![
            status("inbox", "Inbox", "open", false),
            status("assigned", "Assigned", "open", true),
            status("in_progress", "In progress", "active", true),
            status("quality_review", "Quality review", "review", false),
            status("done", "Done", "done", false),
        ]);
        let meta = meta_of(&list);
        assert_eq!(meta.review_key.as_deref(), Some("quality_review"));
        assert_eq!(meta.default_key.as_deref(), Some("inbox"));
        assert_eq!(meta.assigned_key.as_deref(), Some("assigned"));
        assert_eq!(meta.active_key.as_deref(), Some("in_progress"));
        assert_eq!(meta.pickup_keys, vec!["assigned", "in_progress"]);
        // working = pickup + active (deduped by list membership).
        assert_eq!(meta.working_keys, vec!["assigned", "in_progress"]);
        assert_eq!(meta.done_keys, vec!["done"]);
        assert!(meta.terminal("done"));
        assert!(meta.terminal("failed")); // off-board, terminal everywhere
        assert!(!meta.terminal("blocked")); // the system column is not terminal…
        assert!(!meta.terminal("inbox"));
    }

    #[test]
    fn meta_refuses_rather_than_guesses() {
        // A board with NO done columns: doneKeys falls back to the phantom
        // 'done' — fail-SAFE, it only widens what agents may not touch.
        let list = vec![status("inbox", "Inbox", "open", false)];
        let meta = meta_of(&list);
        assert_eq!(meta.done_keys, vec!["done"]);
        assert!(meta.terminal("done"));

        // A board whose open column is keyed like an off-board key: it is
        // terminal everywhere, so it is NOT placeable and defaultKey is None.
        let list = vec![
            status("cancelled", "Cancelled", "open", false),
            status("inbox", "Inbox", "open", false),
        ];
        let meta = meta_of(&list);
        assert_eq!(meta.default_key.as_deref(), Some("inbox"));

        // A review column flagged agent-start (legacy): it cannot be the
        // hand-off destination and it is not a pickup queue — reviewKey stays
        // None and pickupKeys excludes it.
        let list = vec![
            status("inbox", "Inbox", "open", false),
            status("qa", "QA", "review", true),
            status("done", "Done", "done", false),
        ];
        let meta = meta_of(&list);
        assert_eq!(meta.review_key, None);
        assert_eq!(meta.review_keys, vec!["qa"]);
        assert!(meta.pickup_keys.is_empty());
        assert_eq!(meta.assigned_key, None);
        assert!(meta.working_keys.is_empty());
        assert_eq!(meta.active_key, None);
    }

    #[test]
    fn conflict_rules_are_the_two_gates() {
        assert!(agent_start_conflict("review", true).is_some());
        assert!(agent_start_conflict("done", true).is_some());
        assert_eq!(agent_start_conflict("review", false), None);
        assert_eq!(agent_start_conflict("open", true), None);
        assert_eq!(agent_start_conflict("active", true), None);
        assert_eq!(
            agent_start_conflict("done", true),
            Some(DONE_AGENT_START_CONFLICT)
        );
    }

    #[test]
    fn order_refusal_names_every_part_at_once() {
        let have: Vec<String> = vec!["a".into(), "b".into(), "c".into()];
        assert_eq!(
            order_refusal(&have, &["a".into(), "b".into(), "c".into()]),
            None
        );

        let msg =
            order_refusal(&have, &["a".into(), "ghost".into(), "c".into(), "c".into()]).unwrap();
        assert!(msg.contains("“ghost” is not a column on this board"));
        assert!(msg.contains("“c” is listed more than once"));
        assert!(msg.contains("“b” is missing from the order"));
        assert!(msg.starts_with(
            "a new column order has to list every column on this board exactly once — "
        ));
        assert!(msg.ends_with("Nothing was reordered."));

        // Unknown keys dedupe in the display, as TS's [...new Set()] does.
        let msg = order_refusal(&have, &["ghost".into(), "ghost".into()]).unwrap();
        assert_eq!(msg.matches("“ghost”").count(), 1);

        // Naming the system blocked column is an unknown key like any other.
        let msg = order_refusal(&have, &["blocked".into(), "b".into(), "c".into()]).unwrap();
        assert!(msg.contains("“blocked” is not a column on this board"));
    }
}
