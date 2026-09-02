// Boards — visibility, role resolution, CRUD, membership, and the agent
// policy every board-scoped gate obeys.

use crate::agent_auth::{AgentSubject, epoch_ms_to_iso, subject_model};
use crate::users::is_elevated_assistant;
use sqlx::PgPool;

/// SQL fragment: the board `b` is one this USER can see — a direct member, or
/// a member of the team that owns it — and not archived. `includeArchived`
/// drops the archival arm for `listBoards`, the one caller with a deliberate
/// view of retired boards (it states its own). The two placeholder
/// SPELLINGS are passed in because the fragment can sit at any position in a
/// larger query's bind order — callers bind the same user id at each site.
pub fn board_visibility_sql(user_1: &str, user_2: &str, include_archived: bool) -> String {
    let arms = format!(
        "(exists (select 1 from board_members bvm where bvm.board_id = b.id and bvm.user_id = {user_1}::uuid) \
         or exists (select 1 from team_members tvm where tvm.team_id = b.team_id and tvm.user_id = {user_2}::uuid))"
    );
    if include_archived {
        arms
    } else {
        format!("(b.archived_at is null and {arms})")
    }
}

/// Which of two roles wins when both queries answer —
/// descending by rank,
/// first wins.
fn role_rank(role: &str) -> i32 {
    match role {
        "owner" => 3,
        "editor" => 2,
        "viewer" => 1,
        // Anything else ranks below all of it rather than jittering the
        // order — the map above is the whole declared set.
        _ => 0,
    }
}

/// The caller's strongest role on a board. Two arms,
/// UNION ALL — a direct `board_members` row, and membership of the TEAM that
/// owns it (a team OWNER acts as board owner, any other team member as
/// editor) — then the RANK pick when both answer. Null = no relationship at
/// all. This is the one predicate every "may this person see this board"
/// question routes through.
pub async fn board_role(
    pg: &sqlx::PgPool,
    user_id: &str,
    board_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "select role from board_members where board_id = $1::uuid and user_id = $2::uuid \
         union all \
         select case when tm.role = 'owner' then 'owner' else 'editor' end as role \
         from boards b join team_members tm on tm.team_id = b.team_id and tm.user_id = $2::uuid \
         where b.id = $1::uuid",
    )
    .bind(board_id)
    .bind(user_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(role,)| role)
        .max_by_key(|role| role_rank(role)))
}

/// SQL fragment: the board `b` is one this AGENT may touch under its board
/// policy — `allow_all_agents`, or listed on the board — and not archived.
/// This is `board_allows_agent`'s question,
/// as a fragment, for the set-scoping queries that cannot call a per-row
/// predicate without an N+1. The elevated-assistant bypass stays OUT of it:
/// that exemption is policy, and the SQL sites it would widen are reach
/// checks an assistant must not pass on a board it was never added to.
pub fn agent_board_policy_sql(model_1: &str) -> String {
    format!(
        "(b.archived_at is null and (b.allow_all_agents or exists \
         (select 1 from board_agents abm where abm.board_id = b.id and abm.agent_model = {model_1})))"
    )
}

/// The write gate. Owner or editor may change a board;
/// viewer and no-relationship may not.
pub fn can_edit(role: Option<&str>) -> bool {
    matches!(role, Some("owner") | Some("editor"))
}

/// The user-path wire shape. One struct serves the
/// two shapes the wire emits: the LIST row always carries the requester's
/// role and a judgeMode (the column is not-null default 'inherit'); the
/// CREATE return carries role 'owner' and NO judgeMode —
/// `skip_serializing_if` is what reconciles them, so the key is present in
/// every list row and absent from every create response.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Board {
    pub id: String,
    pub name: String,
    pub owner_id: String,
    pub team_id: Option<String>,
    pub team_name: Option<String>,
    pub role: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub judge_mode: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub archived_at: Option<String>,
}

/// One listBoards row, in select order — the epoch-ms triple at the tail is
/// created/updated/archived.
type BoardRow = (
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    i64,
    i64,
    Option<i64>,
);

/// Every board the user can see (explicit share or via
/// a team they belong to), retired boards hidden unless `archived` asks for
/// exactly those. The role is the COALESCE of a direct member row and the
/// team-derived one (team owner acts as board owner, any other team member as
/// editor); under the visibility predicate at least one arm answers, so a
/// null role never happens in practice.
pub async fn list_boards(
    pg: &PgPool,
    user_id: &str,
    archived: bool,
) -> Result<Vec<Board>, sqlx::Error> {
    let rows: Vec<BoardRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select b.id::text, b.name, b.owner_id::text, b.team_id::text, t.name, \
                coalesce(m.role, case when tm.role = 'owner' then 'owner' when tm.role is not null then 'editor' end), \
                b.judge_mode, \
                (trunc(extract(epoch from b.created_at) * 1000))::bigint, \
                (trunc(extract(epoch from b.updated_at) * 1000))::bigint, \
                (trunc(extract(epoch from b.archived_at) * 1000))::bigint \
         from boards b \
         left join board_members m on m.board_id = b.id and m.user_id = $1::uuid \
         left join team_members tm on tm.team_id = b.team_id and tm.user_id = $1::uuid \
         left join teams t on t.id = b.team_id \
         where {} and b.archived_at is {} \
         order by b.updated_at desc",
        board_visibility_sql("$1", "$1", true),
        if archived { "not null" } else { "null" },
    )))
    .bind(user_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(
                id,
                name,
                owner_id,
                team_id,
                team_name,
                role,
                judge_mode,
                created_ms,
                updated_ms,
                archived_ms,
            )| Board {
                id,
                name,
                owner_id,
                team_id,
                team_name,
                role,
                judge_mode,
                created_at: epoch_ms_to_iso(created_ms),
                updated_at: epoch_ms_to_iso(updated_ms),
                archived_at: archived_ms.map(epoch_ms_to_iso),
            },
        )
        .collect())
}

/// Existence + name + ARCHIVAL STATE, in one place.
/// `archived_at` is the load-bearing field and the reason this lives here
/// rather than in each caller: archival is one fact about a board, and it was
/// being read (or forgotten) independently by the callers of
/// `board_allows_agent` — which is how an agent ended up unable to SEE an
/// archived board, unable to PATCH its tickets, and yet able to CREATE one on
/// it. `label` is for diagnostics and falls back to the id.
#[derive(Debug, PartialEq)]
pub struct BoardInfo {
    pub label: String,
    pub archived_at: Option<String>,
    pub exists: bool,
}

pub async fn board_info(pg: &PgPool, board_id: &str) -> Result<BoardInfo, sqlx::Error> {
    let row: Option<(Option<String>, Option<i64>)> = sqlx::query_as(
        "select name, (trunc(extract(epoch from archived_at) * 1000))::bigint \
         from boards where id = $1::uuid",
    )
    .bind(board_id)
    .fetch_optional(pg)
    .await?;
    Ok(match row {
        Some((name, archived_ms)) => BoardInfo {
            label: name
                .filter(|n| !n.is_empty())
                .map(|n| format!("\"{n}\""))
                .unwrap_or_else(|| board_id.to_string()),
            archived_at: archived_ms.map(epoch_ms_to_iso),
            exists: true,
        },
        // No row: every arm collapses to the
        // id, and exists:false is the caller's real answer.
        None => BoardInfo {
            label: board_id.to_string(),
            archived_at: None,
            exists: false,
        },
    })
}

// ── One board pass ───────────────────────────────────────────────────────────
// Callers over many tickets read these per BOARD rather than per ticket, and
// nothing here caches: board archival and agent policy are the
// very facts these predicates enforce, and a stale one is a revoked agent
// that keeps working.

/// A short uppercase ticket prefix from the board name (e.g. "Sprint Board"
/// → "SB"). Initials of the alnum words, first four; fewer than two initials
/// falls back to the alnum-only name, then to 'TASK'. ASCII by construction
/// — the split only ever keeps ASCII-alnum characters — so the 4-char cuts
/// are byte cuts, with none of the UTF-16 surrogate worry.
fn ticket_prefix(name: &str) -> String {
    let initials: String = name
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|w| !w.is_empty())
        .filter_map(|w| w.chars().next())
        .collect::<String>()
        .to_uppercase()
        .chars()
        .take(4)
        .collect();
    if initials.len() >= 2 {
        return initials;
    }
    let alnum: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .collect::<String>()
        .to_uppercase();
    let prefix: String = alnum.chars().take(4).collect();
    if prefix.is_empty() {
        "TASK".into()
    } else {
        prefix
    }
}

/// Create — a board (personal, or under a team) and its
/// creator-as-owner, in one transaction. The owner's personal assistant
/// starts ALLOWED in the SAME transaction: there is no window where
/// `GET /api/boards` lists the board to that assistant under the owner's
/// role while every board-scoped route 403s it — the read path
/// owner-proxies on purpose, the allowlist is what the write path obeys, and
/// this insert is where the two meet. An owner who wants the assistant OFF
/// this board removes it with set_board_agents; nothing here ever re-adds it.
pub async fn create_board(
    pg: &PgPool,
    user_id: &str,
    name: &str,
    team_id: Option<&str>,
) -> Result<Board, sqlx::Error> {
    let mut tx = pg.begin().await?;
    let (id, board_name, owner_id, team_id, created_ms, updated_ms): (
        String,
        String,
        String,
        Option<String>,
        i64,
        i64,
    ) = sqlx::query_as(
        "insert into boards (name, owner_id, ticket_prefix, team_id) \
         values ($1, $2::uuid, $3, $4::uuid) \
         returning id::text, name, owner_id::text, team_id::text, \
                   (trunc(extract(epoch from created_at) * 1000))::bigint, \
                   (trunc(extract(epoch from updated_at) * 1000))::bigint",
    )
    .bind(name)
    .bind(user_id)
    .bind(ticket_prefix(name))
    .bind(team_id)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "insert into board_members (board_id, user_id, role) values ($1::uuid, $2::uuid, 'owner')",
    )
    .bind(&id)
    .bind(user_id)
    .execute(&mut *tx)
    .await?;
    sqlx::query(
        "insert into board_agents (board_id, agent_model) \
         select $1::uuid, model from agent_defs where owner_user_id = $2::uuid \
         on conflict do nothing",
    )
    .bind(&id)
    .bind(user_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(Board {
        id,
        name: board_name,
        owner_id,
        team_id,
        team_name: None,
        role: Some("owner".into()),
        judge_mode: None,
        created_at: epoch_ms_to_iso(created_ms),
        updated_at: epoch_ms_to_iso(updated_ms),
        archived_at: None,
    })
}

pub async fn set_board_judge_mode(
    pg: &PgPool,
    board_id: &str,
    mode: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("update boards set judge_mode = $1, updated_at = now() where id = $2::uuid")
        .bind(mode)
        .bind(board_id)
        .execute(pg)
        .await?;
    Ok(())
}

pub async fn rename_board(pg: &PgPool, board_id: &str, name: &str) -> Result<(), sqlx::Error> {
    sqlx::query("update boards set name = $1, updated_at = now() where id = $2::uuid")
        .bind(name)
        .bind(board_id)
        .execute(pg)
        .await?;
    Ok(())
}

/// Move a board between teams (null → personal). The
/// inner Err ('unknown team') is caught by the route and re-spelled with the
/// human-typed name; the sqlx error is everything else.
pub async fn set_board_team(
    pg: &PgPool,
    board_id: &str,
    team_id: Option<&str>,
) -> Result<Result<(), String>, sqlx::Error> {
    if let Some(team_id) = team_id {
        let known: Option<(i32,)> = sqlx::query_as("select 1 from teams where id = $1::uuid")
            .bind(team_id)
            .fetch_optional(pg)
            .await?;
        if known.is_none() {
            return Ok(Err("unknown team".into()));
        }
    }
    sqlx::query("update boards set team_id = $1::uuid, updated_at = now() where id = $2::uuid")
        .bind(team_id)
        .bind(board_id)
        .execute(pg)
        .await?;
    Ok(Ok(()))
}

pub async fn archive_board(pg: &PgPool, board_id: &str, archived: bool) -> Result<(), sqlx::Error> {
    // Two statements rather than an interpolated now()/null —
    // the parameter set stays identical.
    let stmt = if archived {
        "update boards set archived_at = now(), updated_at = now() where id = $1::uuid"
    } else {
        "update boards set archived_at = null, updated_at = now() where id = $1::uuid"
    };
    sqlx::query(stmt).bind(board_id).execute(pg).await?;
    Ok(())
}

pub async fn delete_board(pg: &PgPool, board_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("delete from boards where id = $1::uuid")
        .bind(board_id)
        .execute(pg)
        .await?;
    Ok(())
}

/// One member row as listMembers serves it, in select order.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardMember {
    pub user_id: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub role: String,
}

pub async fn list_members(pg: &PgPool, board_id: &str) -> Result<Vec<BoardMember>, sqlx::Error> {
    let rows: Vec<(String, Option<String>, Option<String>, String)> = sqlx::query_as(
        "select m.user_id::text, u.email, u.name, m.role \
         from board_members m join users u on u.id = m.user_id \
         where m.board_id = $1::uuid \
         order by (m.role = 'owner') desc, u.email asc",
    )
    .bind(board_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(user_id, email, name, role)| BoardMember {
            user_id,
            email,
            name,
            role,
        })
        .collect())
}

/// The two-way answer: the row landed, or the human-readable
/// refusal the wire carries as `{ok:false, error}` (never a throw — a teammate
/// who has not signed in yet is an expected answer, not a failure).
pub enum ShareOutcome {
    Shared,
    Refused(&'static str),
}

/// Share a board with a teammate by email; they must
/// have signed in before. Upsert: re-sharing an existing member is a role
/// change, not a conflict.
pub async fn share_board(
    pg: &PgPool,
    board_id: &str,
    email: &str,
    role: &str,
) -> Result<ShareOutcome, sqlx::Error> {
    let user: Option<(String,)> =
        sqlx::query_as("select id::text from users where lower(email) = $1")
            .bind(email.trim().to_lowercase())
            .fetch_optional(pg)
            .await?;
    let Some((user_id,)) = user else {
        return Ok(ShareOutcome::Refused(
            "No user with that email has signed in yet",
        ));
    };
    sqlx::query(
        "insert into board_members (board_id, user_id, role) values ($1::uuid, $2::uuid, $3) \
         on conflict (board_id, user_id) do update set role = excluded.role",
    )
    .bind(board_id)
    .bind(user_id)
    .bind(role)
    .execute(pg)
    .await?;
    Ok(ShareOutcome::Shared)
}

pub async fn unshare_board(pg: &PgPool, board_id: &str, user_id: &str) -> Result<(), sqlx::Error> {
    // Never remove the owner via unshare.
    sqlx::query(
        "delete from board_members \
         where board_id = $1::uuid and user_id = $2::uuid and role <> 'owner'",
    )
    .bind(board_id)
    .bind(user_id)
    .execute(pg)
    .await?;
    Ok(())
}

/// The ensure-time half of org-wide access.
/// ACCESS IS MATERIALIZED, NOT DERIVED: a dozen read paths speak
/// `board_members` and nothing else, so the grant is rows — everyone who
/// exists is joined as an editor when the board is ensured, everyone who
/// arrives later at sign-in (join_org_wide_boards, users.rs).
/// One currency, zero special cases. Editor deliberately: anyone
/// can open a ticket and move it along, while owner powers stay with the
/// owner the board was created under.
pub async fn join_everyone_to_board(pg: &PgPool, board_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into board_members (board_id, user_id, role) \
         select $1::uuid, u.id, 'editor' from users u \
         on conflict (board_id, user_id) do nothing",
    )
    .bind(board_id)
    .execute(pg)
    .await?;
    Ok(())
}

// ── Board-scoped agents ──────────────────────────────────────────────────────

/// A board's agent policy: the allow-all flag
/// plus the explicit allow-list. Not a wire shape — every consumer reads the
/// two fields.
#[derive(Debug, PartialEq, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BoardAgentConfig {
    pub allow_all: bool,
    pub models: Vec<String>,
}

pub async fn get_board_agent_config(
    pg: &PgPool,
    board_id: &str,
) -> Result<BoardAgentConfig, sqlx::Error> {
    let flag: Option<(Option<bool>,)> =
        sqlx::query_as("select allow_all_agents from boards where id = $1::uuid")
            .bind(board_id)
            .fetch_optional(pg)
            .await?;
    let rows: Vec<(String,)> = sqlx::query_as(
        "select agent_model from board_agents where board_id = $1::uuid order by agent_model",
    )
    .bind(board_id)
    .fetch_all(pg)
    .await?;
    Ok(BoardAgentConfig {
        // No board row (or a null column) is allow-NOBODY.
        allow_all: flag.and_then(|(v,)| v).unwrap_or(false),
        models: rows.into_iter().map(|(m,)| m).collect(),
    })
}

pub async fn set_board_agent_config(
    pg: &PgPool,
    board_id: &str,
    allow_all: bool,
    models: &[String],
) -> Result<(), sqlx::Error> {
    let mut tx = pg.begin().await?;
    sqlx::query("update boards set allow_all_agents = $1 where id = $2::uuid")
        .bind(allow_all)
        .bind(board_id)
        .execute(&mut *tx)
        .await?;
    sqlx::query("delete from board_agents where board_id = $1::uuid")
        .bind(board_id)
        .execute(&mut *tx)
        .await?;
    // The allow-list is inserted row by row inside the same transaction; only
    // a non-allow-all board carries one.
    if !allow_all {
        for m in models {
            sqlx::query(
                "insert into board_agents (board_id, agent_model) values ($1::uuid, $2) \
                 on conflict do nothing",
            )
            .bind(board_id)
            .bind(m)
            .execute(&mut *tx)
            .await?;
        }
    }
    tx.commit().await?;
    Ok(())
}

/// Whether an agent may be assigned on a board.
/// Restrictive by default: a board allows an agent only if allow-all is on OR
/// the agent is explicitly listed. An admin-elevated personal assistant
/// passes everywhere (org-wide access) — but only when the CALLER proved it
/// is that assistant: take the AgentSubject, not its model, wherever one is
/// in hand, because a bare-model subject reads as proven and silently
/// discards the legacy flag.
///
/// ARCHIVAL IS THE THIRD END OF THE SAME RULE. The listings hide archived
/// boards and the agent-write predicate refuses writes to a ticket on one —
/// but this gate must not hand a fresh ticket ref for a retired board either.
/// An archived board is out of service, for every agent, at every door —
/// including the elevated assistant, whose bypass is a policy exemption, not
/// a licence to work retired boards.
pub async fn board_allows_agent(
    pg: &PgPool,
    board_id: &str,
    subject: &AgentSubject,
) -> Result<bool, sqlx::Error> {
    let board = board_info(pg, board_id).await?;
    if !board.exists || board.archived_at.is_some() {
        return Ok(false);
    }
    let cfg = get_board_agent_config(pg, board_id).await?;
    if cfg.allow_all || cfg.models.iter().any(|m| m == subject_model(subject)) {
        return Ok(true);
    }
    is_elevated_assistant(pg, subject).await
}

/// Validate a mixed assignee list: `user:<uuid>`
/// entries must be board members; bare strings are agents and must pass the
/// board's agent policy. The first human-readable problem, or None when all
/// pass. Bare strings become Model subjects on purpose: this is the one
/// shape that has NO caller — a policy question about a THIRD party.
pub async fn invalid_assignee(
    pg: &PgPool,
    board_id: &str,
    assignees: &[String],
) -> Result<Option<String>, sqlx::Error> {
    for a in assignees {
        if let Some(user_id) = a.strip_prefix("user:") {
            if board_role(pg, user_id, board_id).await?.is_none() {
                return Ok(Some("assignees must be members of this board".into()));
            }
        } else {
            let subject = AgentSubject::Model(a.clone());
            if !board_allows_agent(pg, board_id, &subject).await? {
                return Ok(Some(format!("agent \"{a}\" is not allowed on this board")));
            }
        }
    }
    Ok(None)
}

/// The agent-facing wire shape: no membership
/// role — and no judgeMode, which the two agent listings never select.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentBoard {
    pub id: String,
    pub name: String,
    pub owner_id: String,
    pub team_id: Option<String>,
    pub team_name: Option<String>,
    pub created_at: String,
    pub updated_at: String,
    pub archived_at: Option<String>,
}

/// One agent-listing row, in select order — the epoch-ms triple at the tail.
type AgentBoardRow = (
    String,
    String,
    String,
    Option<String>,
    Option<String>,
    i64,
    i64,
    Option<i64>,
);

fn agent_board_of(
    (id, name, owner_id, team_id, team_name, created_ms, updated_ms, archived_ms): AgentBoardRow,
) -> AgentBoard {
    AgentBoard {
        id,
        name,
        owner_id,
        team_id,
        team_name,
        created_at: epoch_ms_to_iso(created_ms),
        updated_at: epoch_ms_to_iso(updated_ms),
        archived_at: archived_ms.map(epoch_ms_to_iso),
    }
}

/// Every live board, org-wide, for the
/// elevated-assistant listing. No role.
pub async fn list_all_boards(pg: &PgPool) -> Result<Vec<AgentBoard>, sqlx::Error> {
    let rows: Vec<AgentBoardRow> = sqlx::query_as(
        "select b.id::text, b.name, b.owner_id::text, b.team_id::text, t.name, \
                (trunc(extract(epoch from b.created_at) * 1000))::bigint, \
                (trunc(extract(epoch from b.updated_at) * 1000))::bigint, \
                (trunc(extract(epoch from b.archived_at) * 1000))::bigint \
         from boards b left join teams t on t.id = b.team_id \
         where b.archived_at is null order by b.updated_at desc",
    )
    .fetch_all(pg)
    .await?;
    Ok(rows.into_iter().map(agent_board_of).collect())
}

/// Boards an agent may work on (allow-all
/// boards + boards listing it), via the policy fragment so the listing and
/// the gate can never disagree about which boards those are.
pub async fn list_boards_for_agent(
    pg: &PgPool,
    model: &str,
) -> Result<Vec<AgentBoard>, sqlx::Error> {
    let rows: Vec<AgentBoardRow> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "select distinct b.id::text, b.name, b.owner_id::text, b.team_id::text, t.name, \
                (trunc(extract(epoch from b.created_at) * 1000))::bigint, \
                (trunc(extract(epoch from b.updated_at) * 1000))::bigint, \
                (trunc(extract(epoch from b.archived_at) * 1000))::bigint \
         from boards b left join teams t on t.id = b.team_id \
         where {} \
         order by b.updated_at desc",
        agent_board_policy_sql("$1"),
    )))
    .bind(model)
    .fetch_all(pg)
    .await?;
    Ok(rows.into_iter().map(agent_board_of).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visibility_fragment_matches_the_ts_composition() {
        assert_eq!(
            board_visibility_sql("$1", "$2", false),
            "(b.archived_at is null and (exists (select 1 from board_members bvm where bvm.board_id = b.id and bvm.user_id = $1::uuid) or exists (select 1 from team_members tvm where tvm.team_id = b.team_id and tvm.user_id = $2::uuid)))"
        );
        assert_eq!(
            board_visibility_sql("$1", "$2", true),
            "(exists (select 1 from board_members bvm where bvm.board_id = b.id and bvm.user_id = $1::uuid) or exists (select 1 from team_members tvm where tvm.team_id = b.team_id and tvm.user_id = $2::uuid))"
        );
    }

    #[test]
    fn rank_orders_owner_over_editor_over_viewer() {
        // The RANK map (descending, stable — first wins): a direct
        // viewer plus team-editor answers editor, a direct viewer plus
        // team-owner answers owner.
        let mut roles = ["viewer", "editor"];
        roles.sort_by_key(|r| std::cmp::Reverse(role_rank(r)));
        assert_eq!(roles[0], "editor");
        let mut roles = ["viewer", "owner"];
        roles.sort_by_key(|r| std::cmp::Reverse(role_rank(r)));
        assert_eq!(roles[0], "owner");
    }

    #[test]
    fn agent_policy_fragment_matches_the_ts_composition() {
        assert_eq!(
            agent_board_policy_sql("$2"),
            "(b.archived_at is null and (b.allow_all_agents or exists (select 1 from board_agents abm where abm.board_id = b.id and abm.agent_model = $2)))"
        );
    }

    #[test]
    fn can_edit_is_owner_or_editor() {
        assert!(can_edit(Some("owner")));
        assert!(can_edit(Some("editor")));
        assert!(!can_edit(Some("viewer")));
        assert!(!can_edit(None));
    }

    #[test]
    fn ticket_prefix_initials_then_alnum_then_task() {
        // The three tiers, plus the 4-initial cut and the noise-only name.
        assert_eq!(ticket_prefix("Sprint Board"), "SB");
        assert_eq!(ticket_prefix("Marketing Requests"), "MR");
        // First four initials when there are more.
        assert_eq!(ticket_prefix("A B C D E F"), "ABCD");
        // One word = one initial = under the 2 bar → alnum-only name.
        assert_eq!(ticket_prefix("Trials"), "TRIA");
        // Alnum-only fallback is the whole name when it is short enough.
        assert_eq!(ticket_prefix("a"), "A");
        // Hyphens/underscores split words same as spaces.
        assert_eq!(ticket_prefix("ops-on-call"), "OOC");
        // Nothing alnum at all → the fixed prefix.
        assert_eq!(ticket_prefix("日本語"), "TASK");
        assert_eq!(ticket_prefix("!!!"), "TASK");
    }

    #[test]
    fn board_wire_shape_carries_judgemode_on_lists_and_omits_it_on_create() {
        let list_shape = Board {
            id: "b1".into(),
            name: "Sprint Board".into(),
            owner_id: "u1".into(),
            team_id: None,
            team_name: None,
            role: Some("owner".into()),
            judge_mode: Some("inherit".into()),
            created_at: "2026-08-29T00:00:00.000Z".into(),
            updated_at: "2026-08-29T00:00:00.000Z".into(),
            archived_at: None,
        };
        let v = serde_json::to_value(&list_shape).unwrap();
        assert_eq!(v["judgeMode"], serde_json::json!("inherit"));
        assert_eq!(v["role"], serde_json::json!("owner"));
        // teamName/archivedAt are PRESENT nulls on both shapes —
        // the SPA's identity checks see the keys.
        assert!(v.as_object().unwrap().contains_key("teamName"));
        assert!(v.as_object().unwrap().contains_key("archivedAt"));

        // The create return: role owner, no judgeMode key at all.
        let create_shape = Board {
            judge_mode: None,
            ..list_shape
        };
        let v = serde_json::to_value(&create_shape).unwrap();
        assert_eq!(v["role"], serde_json::json!("owner"));
        assert!(!v.as_object().unwrap().contains_key("judgeMode"));

        // The agent listing: no role, no judgeMode.
        let agent_shape = AgentBoard {
            id: "b1".into(),
            name: "Sprint Board".into(),
            owner_id: "u1".into(),
            team_id: None,
            team_name: None,
            created_at: "2026-08-29T00:00:00.000Z".into(),
            updated_at: "2026-08-29T00:00:00.000Z".into(),
            archived_at: None,
        };
        let v = serde_json::to_value(&agent_shape).unwrap();
        assert!(!v.as_object().unwrap().contains_key("role"));
        assert!(!v.as_object().unwrap().contains_key("judgeMode"));
    }
}
