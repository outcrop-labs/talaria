// The sync's own rows: `board_syncs` (one live connection per board) and
// `board_sync_items` (per-task state and tombstones). Tickets are never
// written here — the task engine is their only writer (world.rs).

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::PgPool;

use crate::mapping::{People, Problem, StatusMap};
use crate::merge::Applied;

/// What a connection carries over, beyond tickets.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SyncOptions {
    /// Make tickets for tasks already closed in ClickUp when they have none.
    /// A ticket that exists follows its task when it closes, whatever this
    /// says.
    pub include_closed: bool,
    pub comments: bool,
    pub attachments: bool,
    /// Tell apps about the sync's writes: `ticket.created` for an imported
    /// ticket and `ticket.moved` for a move the sync makes. Off by default: a
    /// first import of a long list would run such an app once per ticket (and
    /// once more per closed task, made in the intake and then moved), and so
    /// would a mapping edit that re-places tickets.
    pub notify_apps: bool,
}

impl Default for SyncOptions {
    fn default() -> Self {
        SyncOptions {
            include_closed: false,
            comments: true,
            attachments: true,
            notify_apps: false,
        }
    }
}

/// The cadences on offer: manual only, 15 minutes, hourly (the default),
/// daily.
pub const INTERVALS: &[Option<i32>] = &[None, Some(15), Some(60), Some(1440)];
pub const DEFAULT_INTERVAL: Option<i32> = Some(60);

pub fn interval_allowed(m: Option<i32>) -> bool {
    INTERVALS.contains(&m)
}

/// One `board_syncs` row.
#[derive(Debug, Clone)]
pub struct SyncRow {
    pub id: String,
    pub board_id: String,
    pub provider: String,
    pub workspace_id: Option<String>,
    pub list_id: String,
    pub list_path: Option<String>,
    pub list_statuses: Value,
    pub credential_sealed: Option<String>,
    pub credential_hint: Option<String>,
    pub credential_account: Option<Value>,
    pub status_map: StatusMap,
    pub map_version: i32,
    pub remap_pending: bool,
    pub options: SyncOptions,
    pub interval_minutes: Option<i32>,
    pub state: String,
    pub watermark_ms: Option<i64>,
    pub last_sweep_ms: Option<i64>,
    pub last_synced_ms: Option<i64>,
    pub next_sync_ms: Option<i64>,
    pub last_run_id: Option<String>,
    pub run_problems: Vec<Problem>,
    pub connected_by: Option<String>,
    /// Who paused the sync and when (epoch ms). Set when the state flips to
    /// paused and cleared on resume. A credential that dies while the sync is
    /// paused leaves them set, so a replaced token returns it to paused.
    pub paused_by: Option<String>,
    pub paused_ms: Option<i64>,
    pub created_ms: i64,
    pub updated_ms: i64,
}

#[derive(sqlx::FromRow)]
struct RawSync {
    id: String,
    board_id: String,
    provider: String,
    workspace_id: Option<String>,
    list_id: String,
    list_path: Option<String>,
    list_statuses: Value,
    credential_sealed: Option<String>,
    credential_hint: Option<String>,
    credential_account: Option<Value>,
    status_map: Value,
    map_version: i32,
    remap_pending: bool,
    options: Value,
    interval_minutes: Option<i32>,
    state: String,
    watermark_ms: Option<i64>,
    last_sweep_ms: Option<i64>,
    last_synced_ms: Option<i64>,
    next_sync_ms: Option<i64>,
    last_run_id: Option<String>,
    run_problems: Value,
    connected_by: Option<String>,
    paused_by: Option<String>,
    paused_ms: Option<i64>,
    created_ms: i64,
    updated_ms: i64,
}

impl From<RawSync> for SyncRow {
    fn from(r: RawSync) -> Self {
        SyncRow {
            id: r.id,
            board_id: r.board_id,
            provider: r.provider,
            workspace_id: r.workspace_id,
            list_id: r.list_id,
            list_path: r.list_path,
            list_statuses: r.list_statuses,
            credential_sealed: r.credential_sealed,
            credential_hint: r.credential_hint,
            credential_account: r.credential_account,
            status_map: serde_json::from_value(r.status_map).unwrap_or_default(),
            map_version: r.map_version,
            remap_pending: r.remap_pending,
            options: serde_json::from_value(r.options).unwrap_or_default(),
            interval_minutes: r.interval_minutes,
            state: r.state,
            watermark_ms: r.watermark_ms,
            last_sweep_ms: r.last_sweep_ms,
            last_synced_ms: r.last_synced_ms,
            next_sync_ms: r.next_sync_ms,
            last_run_id: r.last_run_id,
            run_problems: serde_json::from_value(r.run_problems).unwrap_or_default(),
            connected_by: r.connected_by,
            paused_by: r.paused_by,
            paused_ms: r.paused_ms,
            created_ms: r.created_ms,
            updated_ms: r.updated_ms,
        }
    }
}

const SYNC_SELECT: &str = "select id::text as id, board_id::text as board_id, provider, workspace_id, \
  list_id, list_path, list_statuses, credential_sealed, credential_hint, credential_account, \
  status_map, map_version, remap_pending, options, interval_minutes, state, watermark_ms, \
  (trunc(extract(epoch from last_sweep_at) * 1000))::bigint as last_sweep_ms, \
  (trunc(extract(epoch from last_synced_at) * 1000))::bigint as last_synced_ms, \
  (trunc(extract(epoch from next_sync_at) * 1000))::bigint as next_sync_ms, \
  last_run_id::text as last_run_id, run_problems, connected_by::text as connected_by, \
  paused_by::text as paused_by, \
  (trunc(extract(epoch from paused_at) * 1000))::bigint as paused_ms, \
  (trunc(extract(epoch from created_at) * 1000))::bigint as created_ms, \
  (trunc(extract(epoch from updated_at) * 1000))::bigint as updated_ms \
  from board_syncs";

/// The board's live connection (any state but disconnected).
pub async fn live_sync(pg: &PgPool, board_id: &str) -> Result<Option<SyncRow>, sqlx::Error> {
    let row: Option<RawSync> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SYNC_SELECT} where board_id = $1::uuid and state <> 'disconnected'"
    )))
    .bind(board_id)
    .fetch_optional(pg)
    .await?;
    Ok(row.map(SyncRow::from))
}

pub async fn sync_by_id(pg: &PgPool, id: &str) -> Result<Option<SyncRow>, sqlx::Error> {
    let row: Option<RawSync> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{SYNC_SELECT} where id = $1::uuid"
    )))
    .bind(id)
    .fetch_optional(pg)
    .await?;
    Ok(row.map(SyncRow::from))
}

/// The board's last DISCONNECTED connection to this list — the preview
/// pre-fills its mapping table from it.
pub async fn previous_map(
    pg: &PgPool,
    board_id: &str,
    list_id: &str,
) -> Result<StatusMap, sqlx::Error> {
    let row: Option<(Value,)> = sqlx::query_as(
        "select status_map from board_syncs where board_id = $1::uuid and list_id = $2 \
         and state = 'disconnected' order by disconnected_at desc nulls last limit 1",
    )
    .bind(board_id)
    .bind(list_id)
    .fetch_optional(pg)
    .await?;
    Ok(row
        .and_then(|(v,)| serde_json::from_value(v).ok())
        .unwrap_or_default())
}

/// A pending or settled attachment of one task.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct AttachmentState {
    pub title: String,
    pub size: Option<i64>,
    pub mime: Option<String>,
    pub url: Option<String>,
    /// pending | done | too-large | failed
    pub state: String,
    pub upload_id: Option<String>,
    pub tries: i32,
}

/// One imported comment: its text hash (an edit in ClickUp changes it), the
/// replies already imported, and the message it became.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CommentState {
    pub hash: String,
    pub replies: i64,
    pub message_id: String,
}

/// One `board_sync_items` row.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Item {
    pub external_id: String,
    pub sync_id: Option<String>,
    pub board_id: String,
    pub task_id: Option<String>,
    pub custom_id: Option<String>,
    pub external_url: Option<String>,
    pub remote_state: String,
    pub remote_updated_ms: Option<i64>,
    pub applied: Applied,
    pub attachments: BTreeMap<String, AttachmentState>,
    pub comments: BTreeMap<String, CommentState>,
    pub problems: Vec<Problem>,
}

impl Item {
    /// A row whose ticket was deleted in Talaria: never made again.
    pub fn tombstone(&self) -> bool {
        self.task_id.is_none()
    }

    pub fn has_pending_attachments(&self) -> bool {
        self.attachments.values().any(|a| a.state == "pending")
    }
}

#[derive(sqlx::FromRow)]
struct RawItem {
    external_id: String,
    sync_id: Option<String>,
    board_id: String,
    task_id: Option<String>,
    custom_id: Option<String>,
    external_url: Option<String>,
    remote_state: String,
    remote_updated_ms: Option<i64>,
    applied: Value,
    attachments: Value,
    comments: Value,
    problems: Value,
}

impl From<RawItem> for Item {
    fn from(r: RawItem) -> Self {
        Item {
            external_id: r.external_id,
            sync_id: r.sync_id,
            board_id: r.board_id,
            task_id: r.task_id,
            custom_id: r.custom_id,
            external_url: r.external_url,
            remote_state: r.remote_state,
            remote_updated_ms: r.remote_updated_ms,
            applied: serde_json::from_value(r.applied).unwrap_or_default(),
            attachments: serde_json::from_value(r.attachments).unwrap_or_default(),
            comments: serde_json::from_value(r.comments).unwrap_or_default(),
            problems: serde_json::from_value(r.problems).unwrap_or_default(),
        }
    }
}

const ITEM_SELECT: &str = "select external_id, sync_id::text as sync_id, board_id::text as board_id, \
  task_id::text as task_id, custom_id, external_url, remote_state, remote_updated_ms, applied, \
  attachments, comments, problems from board_sync_items";

pub async fn item(
    pg: &PgPool,
    provider: &str,
    external_id: &str,
) -> Result<Option<Item>, sqlx::Error> {
    let row: Option<RawItem> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{ITEM_SELECT} where provider = $1 and external_id = $2"
    )))
    .bind(provider)
    .bind(external_id)
    .fetch_optional(pg)
    .await?;
    Ok(row.map(Item::from))
}

pub async fn item_for_task(pg: &PgPool, task_id: &str) -> Result<Option<Item>, sqlx::Error> {
    let row: Option<RawItem> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{ITEM_SELECT} where task_id = $1::uuid"
    )))
    .bind(task_id)
    .fetch_optional(pg)
    .await?;
    Ok(row.map(Item::from))
}

pub async fn save_item(pg: &PgPool, provider: &str, it: &Item) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into board_sync_items (provider, external_id, sync_id, board_id, task_id, custom_id, \
           external_url, remote_state, remote_updated_ms, applied, attachments, comments, problems, synced_at) \
         values ($1, $2, $3::uuid, $4::uuid, $5::uuid, $6, $7, $8, $9, $10, $11, $12, $13, now()) \
         on conflict (provider, external_id) do update set \
           sync_id = excluded.sync_id, board_id = excluded.board_id, task_id = excluded.task_id, \
           custom_id = excluded.custom_id, external_url = excluded.external_url, \
           remote_state = excluded.remote_state, remote_updated_ms = excluded.remote_updated_ms, \
           applied = excluded.applied, attachments = excluded.attachments, \
           comments = excluded.comments, problems = excluded.problems, synced_at = now()",
    )
    .bind(provider)
    .bind(&it.external_id)
    .bind(&it.sync_id)
    .bind(&it.board_id)
    .bind(&it.task_id)
    .bind(&it.custom_id)
    .bind(&it.external_url)
    .bind(&it.remote_state)
    .bind(it.remote_updated_ms)
    .bind(serde_json::to_value(&it.applied).unwrap_or_else(|_| json!({})))
    .bind(serde_json::to_value(&it.attachments).unwrap_or_else(|_| json!({})))
    .bind(serde_json::to_value(&it.comments).unwrap_or_else(|_| json!({})))
    .bind(serde_json::to_value(&it.problems).unwrap_or_else(|_| json!([])))
    .execute(pg)
    .await?;
    Ok(())
}

/// A page of this CONNECTION's linked items after `after` (keyset on the
/// external id), filtered by one of the walks. Scoped by `sync_id`, not by the
/// board: a board that once synced another list keeps those tickets, and their
/// items keep the old connection's id, so this list's sweep, remap and status
/// never count them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Walk {
    /// Every linked item (the remap pass).
    Linked,
    /// Linked, active, and not seen by this run's sweep (the missing pass).
    Unseen,
    /// With an attachment still pending.
    PendingAttachments,
    /// With a parent still pending.
    PendingParents,
}

pub async fn walk_items(
    pg: &PgPool,
    sync_id: &str,
    walk: Walk,
    run_id: &str,
    after: Option<&str>,
    limit: i64,
) -> Result<Vec<Item>, sqlx::Error> {
    let filter = match walk {
        Walk::Linked => "",
        Walk::Unseen => {
            " and remote_state = 'active' and (seen_run is null or seen_run::text <> $3)"
        }
        Walk::PendingAttachments => {
            " and exists (select 1 from jsonb_each(attachments) a where a.value->>'state' = 'pending')"
        }
        Walk::PendingParents => " and coalesce(applied->>'parentPending', '') <> ''",
    };
    let rows: Vec<RawItem> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "{ITEM_SELECT} where sync_id = $1::uuid and task_id is not null \
           and ($2::text is null or external_id > $2){filter} \
           and ($3::text is not null) \
         order by external_id asc limit $4"
    )))
    .bind(sync_id)
    .bind(after)
    .bind(run_id)
    .bind(limit)
    .fetch_all(pg)
    .await?;
    Ok(rows.into_iter().map(Item::from).collect())
}

/// How many linked, active items this connection has, and how many this
/// run's sweep did not see — the sweep guard's two numbers.
pub async fn unseen_counts(
    pg: &PgPool,
    sync_id: &str,
    run_id: &str,
) -> Result<(i64, i64), sqlx::Error> {
    sqlx::query_as(
        "select count(*)::bigint, \
           count(*) filter (where seen_run is null or seen_run::text <> $2)::bigint \
         from board_sync_items where sync_id = $1::uuid and task_id is not null \
           and remote_state = 'active'",
    )
    .bind(sync_id)
    .bind(run_id)
    .fetch_one(pg)
    .await
}

/// What the sweep knows about ids it saw on a page: their item rows, marked
/// seen by this run in the same statement.
pub async fn mark_seen(
    pg: &PgPool,
    provider: &str,
    board_id: &str,
    run_id: &str,
    ids: &[String],
) -> Result<Vec<(String, Option<i64>, bool, String)>, sqlx::Error> {
    // (external_id, remote_updated_ms, tombstone, board_id) for every known id;
    // only this board's rows are marked.
    let rows: Vec<(String, Option<i64>, bool, String)> = sqlx::query_as(
        "with known as ( \
           select external_id, remote_updated_ms, task_id is null as tomb, board_id::text as board_id \
           from board_sync_items where provider = $1 and external_id = any($2) \
         ), marked as ( \
           update board_sync_items set seen_run = $4::uuid \
           where provider = $1 and external_id = any($2) and board_id = $3::uuid \
         ) select external_id, remote_updated_ms, tomb, board_id from known",
    )
    .bind(provider)
    .bind(ids)
    .bind(board_id)
    .bind(run_id)
    .fetch_all(pg)
    .await?;
    Ok(rows)
}

/// The finish pass's one write. The watermark moves only when the walk that
/// earned it completed; a mapping edit's flag clears only when the version it
/// was raised at is the one this run applied (an edit made since stays
/// pending for the next run).
///
/// ONLY A RUN THAT READ THE LIST SAYS HOW THE CONNECTION IS. A scheduled or
/// manual run (`walked`) stamps last_synced_at and its problems replace the
/// run-level ones. A remap (no ClickUp call) or a one-ticket re-sync does
/// neither: a credential or list problem the last full run left stands, and
/// the run's own problems, if any, are added to it.
#[derive(Debug, Clone, Default)]
pub struct Finish {
    pub watermark_ms: Option<i64>,
    pub walked: bool,
    pub swept: bool,
    pub remap_version: Option<i32>,
    pub problems: Vec<Problem>,
}

pub async fn finish(pg: &PgPool, sync_id: &str, f: &Finish) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update board_syncs set \
           watermark_ms = coalesce($2, watermark_ms), \
           last_synced_at = case when $6 then now() else last_synced_at end, \
           last_sweep_at = case when $3 then now() else last_sweep_at end, \
           remap_pending = case when $4::int is not null and map_version = $4::int then false \
                                else remap_pending end, \
           run_problems = case when $6 then $5::jsonb \
             else run_problems || coalesce((select jsonb_agg(p.value) from jsonb_array_elements($5::jsonb) p \
                                            where not run_problems @> jsonb_build_array(p.value)), '[]'::jsonb) end, \
           updated_at = now() \
         where id = $1::uuid and state <> 'disconnected'",
    )
    .bind(sync_id)
    .bind(f.watermark_ms)
    .bind(f.swept)
    .bind(f.remap_version)
    .bind(serde_json::to_value(&f.problems).unwrap_or_else(|_| json!([])))
    .bind(f.walked)
    .execute(pg)
    .await?;
    Ok(())
}

/// A run that stopped on a run-level problem (the list is gone) records it,
/// so the status says why without anyone reading the run row. True only when
/// the connection did not already hold a problem of that kind: the
/// transition, which alone files a notice. The connection stays active — a
/// list made visible again resumes on its own — so until then every run stops
/// here, and none of them tells anybody again. A run that finishes replaces
/// the run-level problems, which re-arms the notice.
pub async fn record_stop(
    pg: &PgPool,
    sync_id: &str,
    problem: &Problem,
) -> Result<bool, sqlx::Error> {
    let r = sqlx::query(
        "update board_syncs set run_problems = $2, updated_at = now() \
         where id = $1::uuid and state <> 'disconnected' \
           and not run_problems @> jsonb_build_array(jsonb_build_object('kind', $3::text))",
    )
    .bind(sync_id)
    .bind(json!([problem]))
    .bind(&problem.kind)
    .execute(pg)
    .await?;
    Ok(r.rows_affected() > 0)
}

/// Whether this connection still holds its items — any state but
/// disconnected. An item whose connection let go is free for another board.
pub async fn connection_live(pg: &PgPool, sync_id: &str) -> Result<bool, sqlx::Error> {
    let (live,): (bool,) = sqlx::query_as(
        "select exists (select 1 from board_syncs where id = $1::uuid and state <> 'disconnected')",
    )
    .bind(sync_id)
    .fetch_one(pg)
    .await?;
    Ok(live)
}

/// One owner edit of a live sync. A `None` leaves the field as the row holds
/// it at the moment of the write.
#[derive(Debug, Default)]
pub struct SyncEdit {
    pub status_map: Option<Value>,
    /// The map differs from the one in force: bump map_version, flag a remap.
    pub remap: bool,
    pub options: Option<Value>,
    /// `Some(None)` turns the schedule to manual.
    pub interval_minutes: Option<Option<i32>>,
    /// `Some(true)` pauses an active sync, `Some(false)` resumes a paused one.
    pub paused: Option<bool>,
}

// The state an edit leaves, worked out from the row's own state at the
// write — never from an earlier read. A pause takes only an active sync and
// a resume only a paused one; every other state stays.
const EDIT_STATE: &str = "(case when $7::boolean is true and state = 'active' then 'paused' \
                                when $7::boolean is false and state = 'paused' then 'active' \
                                else state end)";
const EDIT_INTERVAL: &str = "(case when $6 then $5::int else interval_minutes end)";

/// Writes an owner's edit in one statement that reads the row it changes, so
/// an edit saved as someone presses Pause cannot undo the pause: a request
/// that names no `paused` leaves the state, who paused and when, and the
/// next sync as the row holds them. The next sync follows the state and the
/// interval the write leaves. Answers the state and the interval written, or
/// None when the sync was disconnected meanwhile.
pub async fn write_edit(
    pg: &PgPool,
    sync_id: &str,
    user_id: &str,
    edit: &SyncEdit,
) -> Result<Option<(String, Option<i32>)>, sqlx::Error> {
    sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "update board_syncs set \
           status_map = coalesce($2, status_map), \
           map_version = map_version + case when $3 then 1 else 0 end, \
           remap_pending = remap_pending or $3, \
           options = coalesce($4, options), \
           interval_minutes = {EDIT_INTERVAL}, \
           next_sync_at = case when {EDIT_INTERVAL} is null or {EDIT_STATE} <> 'active' then null \
                               when $6 or {EDIT_STATE} <> state or next_sync_at is null \
                                 then now() + ({EDIT_INTERVAL} * interval '1 minute') \
                               else next_sync_at end, \
           paused_by = case when {EDIT_STATE} = 'paused' and state <> 'paused' then $8::uuid \
                            when {EDIT_STATE} = 'active' then null else paused_by end, \
           paused_at = case when {EDIT_STATE} = 'paused' and state <> 'paused' then now() \
                            when {EDIT_STATE} = 'active' then null else paused_at end, \
           state = {EDIT_STATE}, updated_at = now() \
         where id = $1::uuid and state <> 'disconnected' \
         returning state, interval_minutes"
    )))
    .bind(sync_id)
    .bind(edit.status_map.clone())
    .bind(edit.remap)
    .bind(edit.options.clone())
    .bind(edit.interval_minutes.flatten())
    .bind(edit.interval_minutes.is_some())
    .bind(edit.paused)
    .bind(user_id)
    .fetch_optional(pg)
    .await
}

/// The token was refused by the GET /user probe: the sync waits for a person.
pub async fn set_needs_credential(
    pg: &PgPool,
    sync_id: &str,
    problem: &Problem,
) -> Result<bool, sqlx::Error> {
    let r = sqlx::query(
        "update board_syncs set state = 'needs_credential', next_sync_at = null, \
           run_problems = $2, updated_at = now() \
         where id = $1::uuid and state in ('active', 'paused')",
    )
    .bind(sync_id)
    .bind(json!([problem]))
    .execute(pg)
    .await?;
    Ok(r.rows_affected() > 0)
}

pub async fn record_list(
    pg: &PgPool,
    sync_id: &str,
    path: &str,
    statuses: &Value,
    workspace_id: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update board_syncs set list_path = $2, list_statuses = $3, \
           workspace_id = coalesce($4, workspace_id), updated_at = now() where id = $1::uuid",
    )
    .bind(sync_id)
    .bind(path)
    .bind(statuses)
    .bind(workspace_id)
    .execute(pg)
    .await?;
    Ok(())
}

pub async fn set_last_run(pg: &PgPool, sync_id: &str, run_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "update board_syncs set last_run_id = $2::uuid, updated_at = now() where id = $1::uuid",
    )
    .bind(sync_id)
    .bind(run_id)
    .execute(pg)
    .await?;
    Ok(())
}

/// The latest problems across this connection's items, most recently synced
/// first, capped — the status projection's list — and the total.
pub async fn item_problems(
    pg: &PgPool,
    sync_id: &str,
    cap: i64,
) -> Result<(Vec<Problem>, i64), sqlx::Error> {
    let rows: Vec<(Value, Option<String>, Option<String>, Option<String>)> = sqlx::query_as(
        "select p.value, i.task_id::text, \
           case when t.ticket_no is not null then coalesce(b.ticket_prefix, 'TASK') || '-' || t.ticket_no end, \
           i.external_url \
         from board_sync_items i \
         cross join lateral jsonb_array_elements(i.problems) p \
         left join tasks t on t.id = i.task_id \
         left join boards b on b.id = t.board_id \
         where i.sync_id = $1::uuid and jsonb_array_length(i.problems) > 0 \
         order by i.synced_at desc nulls last limit $2",
    )
    .bind(sync_id)
    .bind(cap)
    .fetch_all(pg)
    .await?;
    let (total,): (i64,) = sqlx::query_as(
        "select coalesce(sum(jsonb_array_length(problems)), 0)::bigint from board_sync_items \
         where sync_id = $1::uuid",
    )
    .bind(sync_id)
    .fetch_one(pg)
    .await?;
    let problems = rows
        .into_iter()
        .filter_map(|(v, task_id, ticket_ref, url)| {
            let mut p: Problem = serde_json::from_value(v).ok()?;
            p.ticket_id = p.ticket_id.or(task_id);
            p.ticket_ref = p.ticket_ref.or(ticket_ref);
            p.clickup_url = p.clickup_url.or(url);
            Some(p)
        })
        .collect();
    Ok((problems, total))
}

/// Tickets this connection syncs whose ClickUp task is no longer active in
/// its list, for the cards' chips: task id → archived | deleted | moved |
/// hidden.
pub async fn remote_states(
    pg: &PgPool,
    sync_id: &str,
) -> Result<BTreeMap<String, String>, sqlx::Error> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "select task_id::text, remote_state from board_sync_items \
         where sync_id = $1::uuid and task_id is not null and remote_state <> 'active' \
         order by external_id limit 2000",
    )
    .bind(sync_id)
    .fetch_all(pg)
    .await?;
    Ok(rows.into_iter().collect())
}

/// The people an assignee email can match: the board's members (team members
/// included) and every Talaria user, by lowercased email.
pub async fn people(pg: &PgPool, board_id: &str) -> Result<People, sqlx::Error> {
    let members: Vec<(String, String)> = sqlx::query_as(
        "select lower(u.email), u.id::text from users u where u.email is not null and ( \
           exists (select 1 from board_members m where m.board_id = $1::uuid and m.user_id = u.id) \
           or exists (select 1 from boards b join team_members tm on tm.team_id = b.team_id \
                      where b.id = $1::uuid and tm.user_id = u.id))",
    )
    .bind(board_id)
    .fetch_all(pg)
    .await?;
    let users: Vec<(String, String, Option<String>)> =
        sqlx::query_as("select lower(email), id::text, name from users where email is not null")
            .fetch_all(pg)
            .await?;
    Ok(People {
        members: members.into_iter().collect(),
        users: users
            .into_iter()
            .map(|(e, id, name)| (e, (id, name.unwrap_or_default())))
            .collect(),
    })
}
