// What the routes do (talaria-routes-boards' boards_id_sync*.rs and
// tasks_id_resync.rs answer with these). Each operation checks the board role
// it needs, does its writes, audits them, and answers a status and a fixed
// sentence on refusal — never ClickUp's own words.
//
// WHO: any member reads the status; an owner connects, edits the mapping and
// cadence, pauses and resumes, replaces the credential, browses, previews and
// disconnects; an owner or editor presses Sync now or re-syncs a ticket.
//
// A PAUSE FREEZES THE SYNC. While paused nothing reads ClickUp: the schedule
// claims only active connections (job.rs), Sync now and a one-ticket re-sync
// answer 409 with PAUSED, pressing Pause cancels the live run that reads
// ClickUp, and a run that raced all of that stops at its next step (run.rs).
// An owner pauses to fix a mapping or to take a last import before archiving,
// and an editor's Sync now must not undo that. The pause records who and when
// (paused_by, paused_at) for the board to show. A mapping edit still applies
// while paused, through a remap run that reads nothing from ClickUp, and
// Disconnect works from any state.

use serde::Serialize;
use serde_json::{Value, json};
use talaria_audit::{AuditEntry, log_audit};
use talaria_clickup::{ClickUpError, Client, TaskQuery, token_hint};
use talaria_state::AppState;

use crate::mapping::{
    Columns, People, StatusMap, Target, clamp_tags, map_assignees, offered_targets, place,
    priority_of, suggest, validate_map,
};
use crate::store::{self, SyncOptions, SyncRow, interval_allowed, live_sync, previous_map};
use crate::{Mode, Started, cancel_live, live_run, start_run};

/// A refusal: the HTTP status and the sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpError {
    pub status: u16,
    pub message: String,
}

impl OpError {
    fn new(status: u16, message: impl Into<String>) -> OpError {
        OpError {
            status,
            message: message.into(),
        }
    }
    fn internal(context: &str, e: impl std::fmt::Display) -> OpError {
        tracing::error!("[board-sync] {context}: {e}");
        OpError::new(500, "internal error")
    }
}

type OpResult<T> = Result<T, OpError>;

/// The refusal of a Sync now or a one-ticket re-sync on a paused sync.
pub const PAUSED: &str =
    "the ClickUp sync is paused — a board owner resumes it in Board settings → Sync";

/// The two gates. A non-member is refused like a missing board.
pub async fn require_role(
    state: &AppState,
    user_id: &str,
    board_id: &str,
    owner_only: bool,
) -> OpResult<String> {
    let role = talaria_boards::board_role(&state.pg, user_id, board_id)
        .await
        .map_err(|e| OpError::internal("role read", e))?;
    match role.as_deref() {
        Some("owner") => Ok("owner".into()),
        Some("editor") if !owner_only => Ok("editor".into()),
        Some(_) if owner_only => Err(OpError::new(
            403,
            "only a board owner can change the ClickUp sync",
        )),
        Some(_) => Err(OpError::new(
            403,
            "only a board owner or editor can start a sync",
        )),
        None => Err(OpError::new(403, "forbidden")),
    }
}

fn token_refused(e: &ClickUpError) -> OpError {
    match e {
        ClickUpError::Unauthorized { .. } | ClickUpError::Forbidden => OpError::new(
            400,
            "ClickUp refused that token — make a personal token in ClickUp → Settings → Apps",
        ),
        ClickUpError::RateLimited { .. } => OpError::new(
            429,
            "ClickUp's rate limit was reached — try again in a minute",
        ),
        other => OpError::new(502, format!("ClickUp could not be asked: {other}")),
    }
}

fn list_refused(e: &ClickUpError) -> OpError {
    match e {
        ClickUpError::NotFound | ClickUpError::Forbidden | ClickUpError::Unauthorized { .. } => {
            OpError::new(
                400,
                "that ClickUp list does not exist or this token cannot see it",
            )
        }
        other => token_refused(other),
    }
}

async fn actor_label(state: &AppState, user_id: &str) -> String {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("select email from users where id = $1::uuid")
            .bind(user_id)
            .fetch_optional(&state.pg)
            .await
            .ok()
            .flatten();
    row.and_then(|(e,)| e)
        .unwrap_or_else(|| user_id.to_string())
}

async fn audit(state: &AppState, user_id: &str, action: &str, board_id: &str, after: Value) {
    let actor = actor_label(state, user_id).await;
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &actor,
            action,
            target_type: "board",
            target_id: Some(board_id),
            target_label: None,
            before: None,
            after: Some(after),
        },
    )
    .await;
}

async fn columns(state: &AppState, board_id: &str) -> OpResult<Columns> {
    let list = talaria_statuses::list_statuses(&state.pg, board_id)
        .await
        .map_err(|e| OpError::internal("columns read", e))?;
    Ok(Columns::new(list))
}

/// The client for a call that may carry a fresh token or use the stored one.
async fn client_for(state: &AppState, board_id: &str, token: Option<&str>) -> OpResult<Client> {
    if let Some(t) = token.map(str::trim).filter(|t| !t.is_empty()) {
        return Ok(Client::new(t));
    }
    let sync = live_sync(&state.pg, board_id)
        .await
        .map_err(|e| OpError::internal("sync read", e))?
        .ok_or_else(|| OpError::new(400, "paste a ClickUp token"))?;
    let sb = state
        .secretbox()
        .await
        .map_err(|e| OpError::internal("secretbox", e))?;
    crate::open_client(&sb, &sync)
        .map_err(|_| OpError::new(400, "the stored credential cannot be read — paste a token"))
}

// ── connect ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct ConnectInput {
    pub token: String,
    pub list_id: String,
    pub status_map: StatusMap,
    pub options: SyncOptions,
    pub interval_minutes: Option<i32>,
}

pub async fn connect(
    state: &AppState,
    board_id: &str,
    user_id: &str,
    input: ConnectInput,
) -> OpResult<(SyncRow, Started)> {
    require_role(state, user_id, board_id, true).await?;
    if !interval_allowed(input.interval_minutes) {
        return Err(OpError::new(
            400,
            "intervalMinutes must be null (manual), 15, 60 or 1440",
        ));
    }
    if live_sync(&state.pg, board_id)
        .await
        .map_err(|e| OpError::internal("sync read", e))?
        .is_some()
    {
        return Err(OpError::new(
            409,
            "this board is already connected to ClickUp — disconnect it first",
        ));
    }
    let client = Client::new(&input.token);
    let account = client.user().await.map_err(|e| token_refused(&e))?;
    let list = client
        .list(&input.list_id)
        .await
        .map_err(|e| list_refused(&e))?;
    let cols = columns(state, board_id).await?;
    let map = validate_map(&input.status_map, &cols).map_err(|m| OpError::new(400, m))?;
    let sb = state
        .secretbox()
        .await
        .map_err(|e| OpError::internal("secretbox", e))?;
    let sealed = sb
        .seal(input.token.trim())
        .map_err(|e| OpError::internal("seal", e))?;
    let workspace = workspace_of(&client, &list).await;
    let inserted: Result<(String,), sqlx::Error> = sqlx::query_as(
        "insert into board_syncs (board_id, provider, workspace_id, list_id, list_path, list_statuses, \
           credential_sealed, credential_hint, credential_account, status_map, options, interval_minutes, \
           state, next_sync_at, connected_by) \
         values ($1::uuid, 'clickup', $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, 'active', \
           case when $11::int is null then null else now() + ($11::int * interval '1 minute') end, $12::uuid) \
         returning id::text",
    )
    .bind(board_id)
    .bind(&workspace)
    .bind(&list.id)
    .bind(list.path())
    .bind(serde_json::to_value(&list.statuses).unwrap_or_else(|_| json!([])))
    .bind(&sealed)
    .bind(token_hint(&input.token))
    .bind(json!({ "username": account.username, "email": account.email }))
    .bind(serde_json::to_value(&map).unwrap_or_else(|_| json!({})))
    .bind(serde_json::to_value(&input.options).unwrap_or_else(|_| json!({})))
    .bind(input.interval_minutes)
    .bind(user_id)
    .fetch_one(&state.pg)
    .await;
    let id = match inserted {
        Ok((id,)) => id,
        Err(e) if e.as_database_error().and_then(|d| d.code()).as_deref() == Some("23505") => {
            return Err(OpError::new(
                409,
                "this board is already connected to ClickUp — disconnect it first",
            ));
        }
        Err(e) => return Err(OpError::internal("connect insert", e)),
    };
    // RECONNECTING THE SAME LIST re-attaches this board's items from that
    // list (and so their tickets, by external_key) to the new connection.
    // Items from another list this board once synced keep their old
    // connection: they are not this list's, so its sweep, remap and status
    // never count them.
    if let Err(e) = sqlx::query(
        "update board_sync_items i set sync_id = $1::uuid from board_syncs s \
         where s.id = i.sync_id and s.board_id = $2::uuid and s.list_id = $3 \
           and s.state = 'disconnected' and i.board_id = $2::uuid and i.provider = 'clickup'",
    )
    .bind(&id)
    .bind(board_id)
    .bind(&list.id)
    .execute(&state.pg)
    .await
    {
        tracing::warn!("[board-sync] items not re-attached on {board_id}: {e}");
    }
    audit(
        state,
        user_id,
        "board.sync_connect",
        board_id,
        json!({ "provider": "clickup", "listId": list.id, "listPath": list.path(), "intervalMinutes": input.interval_minutes }),
    )
    .await;
    let sync = store::sync_by_id(&state.pg, &id)
        .await
        .map_err(|e| OpError::internal("sync read", e))?
        .ok_or_else(|| OpError::new(500, "internal error"))?;
    let started = start_run(state, &sync, Mode::Manual, None)
        .await
        .map_err(|e| OpError::internal("first run", e))?;
    Ok((sync, started))
}

/// The workspace a list sits in: the one whose spaces include the list's.
async fn workspace_of(client: &Client, list: &talaria_clickup::ListInfo) -> Option<String> {
    let space = list.space.as_ref()?.id.clone();
    let teams = client.workspaces().await.ok()?;
    if teams.len() == 1 {
        return teams.first().map(|t| t.id.clone());
    }
    for t in teams {
        if let Ok(spaces) = client.spaces(&t.id).await
            && spaces.iter().any(|s| s.id == space)
        {
            return Some(t.id);
        }
    }
    None
}

// ── edit ────────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct UpdateInput {
    pub status_map: Option<StatusMap>,
    pub options: Option<SyncOptions>,
    pub interval_minutes: Option<Option<i32>>,
    pub paused: Option<bool>,
}

pub async fn update(
    state: &AppState,
    board_id: &str,
    user_id: &str,
    input: UpdateInput,
) -> OpResult<SyncRow> {
    require_role(state, user_id, board_id, true).await?;
    let sync = live_sync(&state.pg, board_id)
        .await
        .map_err(|e| OpError::internal("sync read", e))?
        .ok_or_else(|| OpError::new(404, "this board is not connected to ClickUp"))?;
    if let Some(m) = input.interval_minutes
        && !interval_allowed(m)
    {
        return Err(OpError::new(
            400,
            "intervalMinutes must be null (manual), 15, 60 or 1440",
        ));
    }
    let map = match &input.status_map {
        Some(m) => {
            let cols = columns(state, board_id).await?;
            Some(validate_map(m, &cols).map_err(|e| OpError::new(400, e))?)
        }
        None => None,
    };
    if input.paused == Some(false) && sync.state == "needs_credential" {
        return Err(OpError::new(
            409,
            "the sync needs a new credential before it can resume",
        ));
    }
    // The write works the state, and whether the map changed, out from the
    // row it locks, not from the read above: a mapping or schedule edit that
    // read the sync a moment before someone pressed Pause leaves the pause in
    // place, and a map saved over another owner's edit still remaps.
    let edit = store::SyncEdit {
        status_map: map
            .as_ref()
            .map(|m| serde_json::to_value(m).unwrap_or_else(|_| json!({}))),
        options: input
            .options
            .as_ref()
            .map(|o| serde_json::to_value(o).unwrap_or_else(|_| json!({}))),
        interval_minutes: input.interval_minutes,
        paused: input.paused,
    };
    let (state_now, interval, remap) = store::write_edit(&state.pg, &sync.id, user_id, &edit)
        .await
        .map_err(|e| OpError::internal("sync update", e))?
        .ok_or_else(|| OpError::new(404, "this board is not connected to ClickUp"))?;
    // The freeze takes hold now, not at the live run's next step: a run that
    // reads ClickUp is cancelled (its in-flight step stops at its next await).
    // A remap run is the owner's own mapping edit and reads nothing; it
    // finishes.
    if input.paused == Some(true)
        && state_now == "paused"
        && let Err(e) = crate::cancel_live_reads(state, board_id, "the sync was paused").await
    {
        tracing::warn!("[board-sync] live run not cancelled on pause of {board_id}: {e}");
    }
    audit(
        state,
        user_id,
        "board.sync_update",
        board_id,
        json!({
            "statusMap": map,
            "options": input.options,
            "intervalMinutes": interval,
            "state": state_now,
        }),
    )
    .await;
    let next = store::sync_by_id(&state.pg, &sync.id)
        .await
        .map_err(|e| OpError::internal("sync read", e))?
        .ok_or_else(|| OpError::new(404, "this board is not connected to ClickUp"))?;
    if remap {
        // A live run applies it in its own remap pass (or the next run does —
        // the flag clears only at the version applied); otherwise a remap
        // run, which makes no ClickUp calls.
        if let Err(e) = start_run(state, &next, Mode::Remap, None).await {
            tracing::warn!("[board-sync] remap not started on {board_id}: {e}");
        }
    }
    Ok(next)
}

// ── credential ──────────────────────────────────────────────────────────────

/// A new token, the mapping, links and history kept. A sync waiting for a
/// credential turns active and runs, unless it was paused when the old token
/// died (paused_at is set): then it returns to paused, with no next sync,
/// because only Resume lifts a pause.
pub async fn replace_credential(
    state: &AppState,
    board_id: &str,
    user_id: &str,
    token: &str,
) -> OpResult<SyncRow> {
    require_role(state, user_id, board_id, true).await?;
    let sync = live_sync(&state.pg, board_id)
        .await
        .map_err(|e| OpError::internal("sync read", e))?
        .ok_or_else(|| OpError::new(404, "this board is not connected to ClickUp"))?;
    let client = Client::new(token);
    let account = client.user().await.map_err(|e| token_refused(&e))?;
    client
        .list(&sync.list_id)
        .await
        .map_err(|e| list_refused(&e))?;
    let sb = state
        .secretbox()
        .await
        .map_err(|e| OpError::internal("secretbox", e))?;
    let sealed = sb
        .seal(token.trim())
        .map_err(|e| OpError::internal("seal", e))?;
    sqlx::query(
        "update board_syncs set credential_sealed = $2, credential_hint = $3, credential_account = $4, \
           state = case when state <> 'needs_credential' then state \
                        when paused_at is not null then 'paused' else 'active' end, \
           next_sync_at = case when interval_minutes is null or state = 'paused' \
                                    or paused_at is not null then null \
                               else now() + (interval_minutes * interval '1 minute') end, \
           run_problems = '[]'::jsonb, updated_at = now() \
         where id = $1::uuid",
    )
    .bind(&sync.id)
    .bind(&sealed)
    .bind(token_hint(token))
    .bind(json!({ "username": account.username, "email": account.email }))
    .execute(&state.pg)
    .await
    .map_err(|e| OpError::internal("credential update", e))?;
    audit(
        state,
        user_id,
        "board.sync_credential",
        board_id,
        json!({ "account": account.username, "hint": token_hint(token) }),
    )
    .await;
    let next = store::sync_by_id(&state.pg, &sync.id)
        .await
        .map_err(|e| OpError::internal("sync read", e))?
        .ok_or_else(|| OpError::new(404, "this board is not connected to ClickUp"))?;
    if next.state == "active"
        && let Err(e) = start_run(state, &next, Mode::Manual, None).await
    {
        tracing::warn!("[board-sync] run after credential replace not started on {board_id}: {e}");
    }
    Ok(next)
}

// ── disconnect ──────────────────────────────────────────────────────────────

/// Stops future syncs. Every imported ticket, its link and its imported
/// comments stay, editable, owned by Talaria; reconnecting the same list
/// re-attaches them by external_key with no duplicates.
pub async fn disconnect(state: &AppState, board_id: &str, user_id: &str) -> OpResult<()> {
    require_role(state, user_id, board_id, true).await?;
    let row: Option<(String,)> = sqlx::query_as(
        "update board_syncs set state = 'disconnected', credential_sealed = null, next_sync_at = null, \
           remap_pending = false, disconnected_at = now(), disconnected_by = $2::uuid, updated_at = now() \
         where board_id = $1::uuid and state <> 'disconnected' returning id::text",
    )
    .bind(board_id)
    .bind(user_id)
    .fetch_optional(&state.pg)
    .await
    .map_err(|e| OpError::internal("disconnect", e))?;
    if row.is_none() {
        return Err(OpError::new(404, "this board is not connected to ClickUp"));
    }
    if let Err(e) = cancel_live(state, board_id, "the board was disconnected from ClickUp").await {
        tracing::warn!("[board-sync] live run not cancelled on {board_id}: {e}");
    }
    audit(
        state,
        user_id,
        "board.sync_disconnect",
        board_id,
        json!({ "provider": "clickup" }),
    )
    .await;
    Ok(())
}

// ── runs ────────────────────────────────────────────────────────────────────

/// Sync now: the live run if one is going, else a new manual run (which
/// always sweeps). Refused while the sync waits for a credential or is paused.
pub async fn sync_now(state: &AppState, board_id: &str, user_id: &str) -> OpResult<Started> {
    require_role(state, user_id, board_id, false).await?;
    let sync = live_sync(&state.pg, board_id)
        .await
        .map_err(|e| OpError::internal("sync read", e))?
        .ok_or_else(|| OpError::new(404, "this board is not connected to ClickUp"))?;
    if sync.state == "needs_credential" {
        return Err(OpError::new(
            409,
            "the sync needs a new credential — a board owner replaces it",
        ));
    }
    if sync.state == "paused" {
        return Err(OpError::new(409, PAUSED));
    }
    start_run(state, &sync, Mode::Manual, None)
        .await
        .map_err(|e| OpError::internal("sync now", e))
}

/// Re-sync one ticket from ClickUp, forced. Refused while the sync waits for
/// a credential, is paused, or is running.
pub async fn resync_ticket(state: &AppState, task_id: &str, user_id: &str) -> OpResult<String> {
    let board_id = talaria_tasks::task_board_id(&state.pg, task_id)
        .await
        .map_err(|e| OpError::internal("ticket read", e))?
        .ok_or_else(|| OpError::new(404, "no such ticket"))?;
    require_role(state, user_id, &board_id, false).await?;
    let item = store::item_for_task(&state.pg, task_id)
        .await
        .map_err(|e| OpError::internal("item read", e))?
        .filter(|i| i.board_id == board_id)
        .ok_or_else(|| OpError::new(400, "this ticket is not synced from ClickUp"))?;
    let sync = live_sync(&state.pg, &board_id)
        .await
        .map_err(|e| OpError::internal("sync read", e))?
        .ok_or_else(|| OpError::new(400, "this board is no longer connected to ClickUp"))?;
    if sync.state == "needs_credential" {
        return Err(OpError::new(
            409,
            "the sync needs a new credential — a board owner replaces it",
        ));
    }
    if sync.state == "paused" {
        return Err(OpError::new(409, PAUSED));
    }
    if live_run(&state.pg, &board_id)
        .await
        .map_err(|e| OpError::internal("run read", e))?
        .is_some()
    {
        return Err(OpError::new(
            409,
            "a sync is running on this board — try again when it finishes",
        ));
    }
    match start_run(state, &sync, Mode::Ticket, Some(item.external_id))
        .await
        .map_err(|e| OpError::internal("re-sync", e))?
    {
        Started::New(id) => Ok(id),
        Started::Live(_) => Err(OpError::new(
            409,
            "a sync is running on this board — try again when it finishes",
        )),
    }
}

// ── browse and preview ──────────────────────────────────────────────────────

#[derive(Debug, Clone, Default)]
pub struct BrowseInput {
    pub token: Option<String>,
    pub team_id: Option<String>,
    pub space_id: Option<String>,
    pub folder_id: Option<String>,
}

/// One level of ClickUp's hierarchy per call: workspaces, then a
/// workspace's spaces, then a space's folders and folderless lists, then a
/// folder's lists.
pub async fn browse(
    state: &AppState,
    board_id: &str,
    user_id: &str,
    input: BrowseInput,
) -> OpResult<Value> {
    require_role(state, user_id, board_id, true).await?;
    let client = client_for(state, board_id, input.token.as_deref()).await?;
    let account = client.user().await.map_err(|e| token_refused(&e))?;
    let level = if let Some(folder) = &input.folder_id {
        json!({ "lists": client.folder_lists(folder).await.map_err(|e| list_refused(&e))? })
    } else if let Some(space) = &input.space_id {
        let folders = client.folders(space).await.map_err(|e| list_refused(&e))?;
        let lists = client
            .folderless_lists(space)
            .await
            .map_err(|e| list_refused(&e))?;
        json!({ "folders": folders, "lists": lists })
    } else if let Some(team) = &input.team_id {
        json!({ "spaces": client.spaces(team).await.map_err(|e| list_refused(&e))? })
    } else {
        json!({ "workspaces": client.workspaces().await.map_err(|e| token_refused(&e))? })
    };
    let mut out = json!({ "account": { "username": account.username, "email": account.email } });
    if let (Some(o), Some(l)) = (out.as_object_mut(), level.as_object()) {
        for (k, v) in l {
            o.insert(k.clone(), v.clone());
        }
    }
    Ok(out)
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct PreviewStatus {
    name: String,
    color: Option<String>,
    #[serde(rename = "type")]
    kind: Option<String>,
    suggested: Option<String>,
}

/// Up to this many tasks feed the preview's statistics.
pub const PREVIEW_TASKS: usize = 500;
/// The sample shown as it would land, and the tasks whose files are measured.
pub const PREVIEW_SAMPLE: usize = 10;

/// What connecting would do, before anything is written: the statuses with
/// their suggested columns, the columns on offer (with which start agent
/// work), the first tasks as they would land, who would and would not be
/// assigned, the labels that would be created, oversized files in the sample,
/// and how long the first import should take at this token's rate limit.
pub async fn preview(
    state: &AppState,
    board_id: &str,
    user_id: &str,
    token: Option<&str>,
    list_id: &str,
) -> OpResult<Value> {
    require_role(state, user_id, board_id, true).await?;
    let client = client_for(state, board_id, token).await?;
    let list = client.list(list_id).await.map_err(|e| list_refused(&e))?;
    let cols = columns(state, board_id).await?;
    let previous = previous_map(&state.pg, board_id, list_id)
        .await
        .map_err(|e| OpError::internal("previous map", e))?;
    let current = live_sync(&state.pg, board_id)
        .await
        .map_err(|e| OpError::internal("sync read", e))?
        .filter(|s| s.list_id == list_id)
        .map(|s| s.status_map)
        .unwrap_or_default();
    let prefill: StatusMap = if current.is_empty() {
        previous
    } else {
        current
    };
    let statuses: Vec<PreviewStatus> = list
        .statuses
        .iter()
        .map(|s| PreviewStatus {
            name: s.status.clone(),
            color: s.color.clone(),
            kind: s.kind.clone(),
            suggested: suggest(&s.status, &cols, &prefill),
        })
        .collect();
    let targets: Vec<Target> = offered_targets(&cols);
    // The sample lands as the pre-filled table would place it.
    let suggested: StatusMap = statuses
        .iter()
        .filter_map(|s| {
            s.suggested
                .clone()
                .map(|k| (s.name.trim().to_lowercase(), k))
        })
        .collect();

    // The statistics: up to PREVIEW_TASKS tasks, closed included.
    let mut tasks = Vec::new();
    let mut page = 0u32;
    let mut more = true;
    while more && tasks.len() < PREVIEW_TASKS {
        let p = client
            .tasks_page(
                list_id,
                &TaskQuery {
                    page,
                    archived: false,
                    updated_after_ms: None,
                },
            )
            .await
            .map_err(|e| list_refused(&e))?;
        more = !p.last_page;
        tasks.extend(p.tasks);
        page += 1;
    }
    let partial = more;
    let people = people_of(state, board_id).await?;
    let mut outcomes: std::collections::BTreeMap<String, Value> = Default::default();
    let mut labels: Vec<String> = Vec::new();
    let existing_labels: Vec<String> =
        sqlx::query_as::<_, (String,)>("select name from board_labels where board_id = $1::uuid")
            .bind(board_id)
            .fetch_all(&state.pg)
            .await
            .map_err(|e| OpError::internal("labels read", e))?
            .into_iter()
            .map(|(n,)| n.to_lowercase())
            .collect();
    let mut closed = 0usize;
    for t in &tasks {
        if t.closed() {
            closed += 1;
        }
        for p in &t.assignees {
            let key = p.email.clone().unwrap_or_else(|| p.display());
            if outcomes.contains_key(&key) {
                continue;
            }
            let outcome = match crate::mapping::assignee(p, &people) {
                crate::mapping::Assignee::Member(_) => "member",
                crate::mapping::Assignee::NotOnBoard(_) => "not-on-board",
                crate::mapping::Assignee::NoAccount(_) => "no-account",
                crate::mapping::Assignee::NoEmail(_) => "no-email",
            };
            outcomes.insert(
                key,
                json!({ "clickupName": p.display(), "email": p.email, "outcome": outcome }),
            );
        }
        for tag in clamp_tags(&t.tags).0 {
            if !existing_labels.contains(&tag.to_lowercase()) && !labels.contains(&tag) {
                labels.push(tag);
            }
        }
    }
    let sample: Vec<Value> = tasks
        .iter()
        .take(PREVIEW_SAMPLE)
        .map(|t| {
            let placement = place(&t.status, &suggested, &cols);
            let (humans, _) = map_assignees(t, &people);
            json!({
                "id": t.id,
                "title": crate::mapping::clamp_title(&t.name),
                "clickupStatus": t.status,
                "column": placement.column(),
                "unmapped": !matches!(placement, crate::mapping::Placement::Mapped(_)),
                "priority": priority_of(t.priority),
                "due": crate::mapping::iso(t.due_ms),
                "assignees": humans,
                "tags": clamp_tags(&t.tags).0,
                "closed": t.closed(),
            })
        })
        .collect();
    // Files: measured on the sample (one GET /task each).
    let (mut oversize, mut sampled_bytes, mut sampled) = (0usize, 0i64, 0usize);
    for t in tasks.iter().take(PREVIEW_SAMPLE) {
        if let Ok(d) = client.task(&t.id).await {
            sampled += 1;
            for a in d.attachments.unwrap_or_default() {
                let size = a.size.unwrap_or(0);
                if size as usize > talaria_uploads::MAX_BYTES {
                    oversize += 1;
                } else {
                    sampled_bytes += size;
                }
            }
        }
    }
    let total = list
        .task_count
        .unwrap_or(tasks.len() as i64)
        .max(tasks.len() as i64);
    let pages = (total + 99) / 100;
    let requests = pages + total * 2; // the walk, then per task its detail and a comment page
    let per_minute = client.rate().limit.unwrap_or(100).max(1);
    let bytes = if sampled > 0 {
        sampled_bytes / sampled as i64 * total
    } else {
        0
    };
    Ok(json!({
        "list": { "id": list.id, "path": list.path(), "taskCount": list.task_count },
        "statuses": statuses,
        "columns": targets,
        "intake": cols.intake(),
        "sample": sample,
        "people": outcomes.into_values().collect::<Vec<_>>(),
        "newLabels": labels,
        "closedTasks": closed,
        "oversizeInSample": oversize,
        "estimate": {
            "requests": requests,
            "perMinute": per_minute,
            "minutes": (requests + per_minute - 1) / per_minute,
            "bytes": bytes,
        },
        "statsFrom": { "tasks": tasks.len(), "partial": partial, "attachmentSample": sampled },
    }))
}

async fn people_of(state: &AppState, board_id: &str) -> OpResult<People> {
    store::people(&state.pg, board_id)
        .await
        .map_err(|e| OpError::internal("people read", e))
}
