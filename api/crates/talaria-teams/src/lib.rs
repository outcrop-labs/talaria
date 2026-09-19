// Teams — an org principal: people + agents. Membership expands at auth
// time onto multiplayer surfaces, platform views/perms, and MCP tools.
// Human member rows cascade on team delete; boards survive as personal
// boards (team_id set null, not cascaded) — the delete stays owner-gated.

use serde_json::{Value, json};
use sqlx::PgPool;
use std::collections::HashMap;
use talaria_agent_auth::epoch_ms_to_iso;

/// One team row as the LIST serves it — the select's key order
/// (id, name, role, createdAt, memberCount, agentCount, description).
/// The CREATE response is spread in a different order and is built by
/// the route, not this struct.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Team {
    pub id: String,
    pub name: String,
    pub role: String,
    pub created_at: String,
    pub member_count: i32,
    pub agent_count: i32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamMember {
    pub user_id: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub role: String,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamAgent {
    pub agent_model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamDirectoryEntry {
    pub id: String,
    pub name: String,
    pub member_count: i32,
    pub agent_count: i32,
}

fn team_of_row(
    id: String,
    name: String,
    role: String,
    created_ms: i64,
    member_count: i32,
    agent_count: i32,
    description: Option<String>,
) -> Team {
    Team {
        id,
        name,
        role,
        member_count,
        agent_count,
        description: description.filter(|s| !s.is_empty()),
        created_at: epoch_ms_to_iso(created_ms),
    }
}

pub async fn list_teams(pg: &PgPool, user_id: &str) -> Result<Vec<Team>, sqlx::Error> {
    let rows: Vec<(String, String, String, i64, i32, i32, Option<String>)> = sqlx::query_as(
        "select t.id::text, t.name, m.role, \
                (trunc(extract(epoch from t.created_at) * 1000))::bigint, \
                (select count(*)::int from team_members x where x.team_id = t.id), \
                (select count(*)::int from team_agents a where a.team_id = t.id), \
                t.description \
         from teams t join team_members m on m.team_id = t.id and m.user_id = $1::uuid \
         order by t.name asc",
    )
    .bind(user_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, name, role, created_ms, member_count, agent_count, description)| {
                team_of_row(
                    id,
                    name,
                    role,
                    created_ms,
                    member_count,
                    agent_count,
                    description,
                )
            },
        )
        .collect())
}

/// Every org team. `role` is the caller's standing, or "" when they are not
/// a member — the Manage list needs the whole roster.
pub async fn list_all_teams(pg: &PgPool, user_id: &str) -> Result<Vec<Team>, sqlx::Error> {
    let rows: Vec<(
        String,
        String,
        Option<String>,
        i64,
        i32,
        i32,
        Option<String>,
    )> = sqlx::query_as(
        "select t.id::text, t.name, m.role, \
                (trunc(extract(epoch from t.created_at) * 1000))::bigint, \
                (select count(*)::int from team_members x where x.team_id = t.id), \
                (select count(*)::int from team_agents a where a.team_id = t.id), \
                t.description \
         from teams t \
         left join team_members m on m.team_id = t.id and m.user_id = $1::uuid \
         order by t.name asc",
    )
    .bind(user_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, name, role, created_ms, member_count, agent_count, description)| {
                team_of_row(
                    id,
                    name,
                    role.unwrap_or_default(),
                    created_ms,
                    member_count,
                    agent_count,
                    description,
                )
            },
        )
        .collect())
}

/// Id + name + counts for share pickers — any signed-in caller.
pub async fn list_team_directory(pg: &PgPool) -> Result<Vec<TeamDirectoryEntry>, sqlx::Error> {
    let rows: Vec<(String, String, i32, i32)> = sqlx::query_as(
        "select t.id::text, t.name, \
                (select count(*)::int from team_members x where x.team_id = t.id), \
                (select count(*)::int from team_agents a where a.team_id = t.id) \
         from teams t order by t.name asc",
    )
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, name, member_count, agent_count)| TeamDirectoryEntry {
            id,
            name,
            member_count,
            agent_count,
        })
        .collect())
}

pub async fn get_team(
    pg: &PgPool,
    team_id: &str,
) -> Result<Option<(String, String, Option<String>, i64)>, sqlx::Error> {
    sqlx::query_as(
        "select id::text, name, description, \
                (trunc(extract(epoch from created_at) * 1000))::bigint \
         from teams where id = $1::uuid",
    )
    .bind(team_id)
    .fetch_optional(pg)
    .await
}

/// The caller's role in a team, None when not a member.
pub async fn team_role(
    pg: &PgPool,
    user_id: &str,
    team_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as(
        "select role from team_members where team_id = $1::uuid and user_id = $2::uuid",
    )
    .bind(team_id)
    .bind(user_id)
    .fetch_optional(pg)
    .await?;
    Ok(row.map(|r| r.0))
}

pub async fn team_ids_for_user(pg: &PgPool, user_id: &str) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<(String,)> =
        sqlx::query_as("select team_id::text from team_members where user_id = $1::uuid")
            .bind(user_id)
            .fetch_all(pg)
            .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

pub async fn team_ids_for_agent(
    pg: &PgPool,
    agent_model: &str,
) -> Result<Vec<String>, sqlx::Error> {
    let rows: Vec<(String,)> =
        sqlx::query_as("select team_id::text from team_agents where agent_model = $1")
            .bind(agent_model)
            .fetch_all(pg)
            .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// Create a team and seat the creator as its owner, in one transaction —
/// (id, name, createdAt-epoch-ms) for the route's spread-order response.
pub async fn create_team(
    pg: &PgPool,
    user_id: &str,
    name: &str,
) -> Result<(String, String, i64), sqlx::Error> {
    let mut tx = pg.begin().await?;
    let team: (String, String, i64) = sqlx::query_as(
        "insert into teams (name, created_by) values ($1, $2::uuid) \
         returning id::text, name, (trunc(extract(epoch from created_at) * 1000))::bigint",
    )
    .bind(name)
    .bind(user_id)
    .fetch_one(&mut *tx)
    .await?;
    sqlx::query(
        "insert into team_members (team_id, user_id, role) values ($1::uuid, $2::uuid, 'owner')",
    )
    .bind(&team.0)
    .bind(user_id)
    .execute(&mut *tx)
    .await?;
    tx.commit().await?;
    Ok(team)
}

pub async fn rename_team(pg: &PgPool, team_id: &str, name: &str) -> Result<(), sqlx::Error> {
    sqlx::query("update teams set name = $1 where id = $2::uuid")
        .bind(name)
        .bind(team_id)
        .execute(pg)
        .await?;
    Ok(())
}

pub async fn set_team_description(
    pg: &PgPool,
    team_id: &str,
    description: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("update teams set description = $1 where id = $2::uuid")
        .bind(description)
        .bind(team_id)
        .execute(pg)
        .await?;
    Ok(())
}

pub async fn delete_team(pg: &PgPool, team_id: &str) -> Result<(), sqlx::Error> {
    sqlx::query("delete from teams where id = $1::uuid")
        .bind(team_id)
        .execute(pg)
        .await?;
    Ok(())
}

pub async fn list_team_members(pg: &PgPool, team_id: &str) -> Result<Vec<TeamMember>, sqlx::Error> {
    let rows: Vec<(String, Option<String>, Option<String>, String)> = sqlx::query_as(
        "select m.user_id::text, u.email, u.name, m.role \
         from team_members m join users u on u.id = m.user_id \
         where m.team_id = $1::uuid \
         order by (m.role = 'owner') desc, u.email asc",
    )
    .bind(team_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(user_id, email, name, role)| TeamMember {
            user_id,
            email,
            name,
            role,
        })
        .collect())
}

/// Add (or re-role) a member by email. Ok(None) = added; Ok(Some(sentence))
/// = the fixed 400 the route answers when nobody has signed in with that
/// email; Err = a DB failure, which the route 500s. The lookup is by the
/// trimmed+lowercased email.
pub async fn add_team_member(
    pg: &PgPool,
    team_id: &str,
    email: &str,
    role: &str,
) -> Result<Option<String>, sqlx::Error> {
    let user: Option<(String,)> =
        sqlx::query_as("select id::text from users where lower(email) = $1")
            .bind(email.trim().to_lowercase())
            .fetch_optional(pg)
            .await?;
    let Some((user_id,)) = user else {
        return Ok(Some("No user with that email has signed in yet".into()));
    };
    sqlx::query(
        "insert into team_members (team_id, user_id, role) values ($1::uuid, $2::uuid, $3) \
         on conflict (team_id, user_id) do update set role = excluded.role",
    )
    .bind(team_id)
    .bind(user_id)
    .bind(role)
    .execute(pg)
    .await?;
    Ok(None)
}

/// Remove a member — never an owner (the `role <> 'owner'` guard makes it a
/// silent no-op).
pub async fn remove_team_member(
    pg: &PgPool,
    team_id: &str,
    user_id: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "delete from team_members \
         where team_id = $1::uuid and user_id = $2::uuid and role <> 'owner'",
    )
    .bind(team_id)
    .bind(user_id)
    .execute(pg)
    .await?;
    Ok(())
}

pub async fn list_team_agents(pg: &PgPool, team_id: &str) -> Result<Vec<TeamAgent>, sqlx::Error> {
    let rows: Vec<(String, Option<String>)> = sqlx::query_as(
        "select a.agent_model, d.display_name \
         from team_agents a \
         left join agent_defs d on d.model = a.agent_model \
         where a.team_id = $1::uuid \
         order by coalesce(d.display_name, a.agent_model) asc",
    )
    .bind(team_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(agent_model, label)| TeamAgent { agent_model, label })
        .collect())
}

/// Ok(None) = added; Ok(Some(sentence)) = unknown agent (400).
pub async fn add_team_agent(
    pg: &PgPool,
    team_id: &str,
    agent_model: &str,
) -> Result<Option<String>, sqlx::Error> {
    let known: Option<(i32,)> = sqlx::query_as("select 1 from agent_defs where model = $1")
        .bind(agent_model)
        .fetch_optional(pg)
        .await?;
    if known.is_none() {
        return Ok(Some("unknown agent".into()));
    }
    sqlx::query(
        "insert into team_agents (team_id, agent_model) values ($1::uuid, $2) \
         on conflict do nothing",
    )
    .bind(team_id)
    .bind(agent_model)
    .execute(pg)
    .await?;
    Ok(None)
}

pub async fn remove_team_agent(
    pg: &PgPool,
    team_id: &str,
    agent_model: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query("delete from team_agents where team_id = $1::uuid and agent_model = $2")
        .bind(team_id)
        .bind(agent_model)
        .execute(pg)
        .await?;
    Ok(())
}

/// Raw team access overrides — the Admin chips' source, not the resolved
/// effective set. Views are the stored arrays; perms are a sparse {id: bool}.
pub async fn get_team_access(pg: &PgPool, team_id: &str) -> Result<Value, sqlx::Error> {
    let row: Option<(Vec<String>, Vec<String>)> =
        sqlx::query_as("select denied_views, allowed_manage_views from teams where id = $1::uuid")
            .bind(team_id)
            .fetch_optional(pg)
            .await?;
    let (denied_views, allowed_manage_views) = row.unwrap_or_default();
    let rows: Vec<(String, bool)> =
        sqlx::query_as("select perm, allowed from team_permissions where team_id = $1::uuid")
            .bind(team_id)
            .fetch_all(pg)
            .await?;
    let mut permissions = serde_json::Map::new();
    for (perm, allowed) in rows {
        permissions.insert(perm, Value::Bool(allowed));
    }
    Ok(json!({
        "deniedViews": denied_views,
        "allowedManageViews": allowed_manage_views,
        "permissions": permissions,
    }))
}

pub async fn set_team_denied_views(
    pg: &PgPool,
    team_id: &str,
    views: &[String],
) -> Result<(), sqlx::Error> {
    sqlx::query("update teams set denied_views = $1 where id = $2::uuid")
        .bind(views)
        .bind(team_id)
        .execute(pg)
        .await?;
    Ok(())
}

pub async fn set_team_allowed_manage_views(
    pg: &PgPool,
    team_id: &str,
    views: &[String],
) -> Result<(), sqlx::Error> {
    sqlx::query("update teams set allowed_manage_views = $1 where id = $2::uuid")
        .bind(views)
        .bind(team_id)
        .execute(pg)
        .await?;
    Ok(())
}

pub async fn set_team_perm_override(
    pg: &PgPool,
    team_id: &str,
    perm: &str,
    allowed: Option<bool>,
) -> Result<(), sqlx::Error> {
    match allowed {
        None => {
            sqlx::query("delete from team_permissions where team_id = $1::uuid and perm = $2")
                .bind(team_id)
                .bind(perm)
                .execute(pg)
                .await?;
        }
        Some(allowed) => {
            sqlx::query(
                "insert into team_permissions (team_id, perm, allowed) values ($1::uuid, $2, $3) \
                 on conflict (team_id, perm) do update set allowed = $3",
            )
            .bind(team_id)
            .bind(perm)
            .bind(allowed)
            .execute(pg)
            .await?;
        }
    }
    Ok(())
}

/// Team-level perm overrides for a user, collapsed with bool_or: any allow
/// wins; a perm only denied across teams is false. Unmentioned perms absent.
pub async fn team_perm_overrides_for_user(
    pg: &PgPool,
    user_id: &str,
) -> Result<HashMap<String, bool>, sqlx::Error> {
    let rows: Vec<(String, bool)> = sqlx::query_as(
        "select tp.perm, bool_or(tp.allowed) \
         from team_permissions tp \
         join team_members tm on tm.team_id = tp.team_id and tm.user_id = $1::uuid \
         group by tp.perm",
    )
    .bind(user_id)
    .fetch_all(pg)
    .await?;
    Ok(rows.into_iter().collect())
}

/// Union of work-view denials and manage-view grants across the user's teams.
pub async fn team_view_grants_for_user(
    pg: &PgPool,
    user_id: &str,
) -> Result<(Vec<String>, Vec<String>), sqlx::Error> {
    let rows: Vec<(Vec<String>, Vec<String>)> = sqlx::query_as(
        "select t.denied_views, t.allowed_manage_views \
         from teams t join team_members m on m.team_id = t.id and m.user_id = $1::uuid",
    )
    .bind(user_id)
    .fetch_all(pg)
    .await?;
    let mut denied = Vec::new();
    let mut allowed = Vec::new();
    for (d, a) in rows {
        for v in d {
            if !denied.contains(&v) {
                denied.push(v);
            }
        }
        for v in a {
            if !allowed.contains(&v) {
                allowed.push(v);
            }
        }
    }
    Ok((denied, allowed))
}
