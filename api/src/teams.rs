// Teams — a group of users that collectively own/access boards. The member
// rows cascade on team delete; boards survive as personal boards (team_id
// set null, not cascaded) — the delete stays owner-gated.

use crate::agent_auth::epoch_ms_to_iso;
use sqlx::PgPool;

/// One team row as the LIST serves it — the select's key order
/// (id, name, role, createdAt, memberCount). The CREATE response is spread
/// in a different order ({id, name, createdAt, role, memberCount}) and is
/// built by the route, not this struct.
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Team {
    pub id: String,
    pub name: String,
    pub role: String,
    pub created_at: String,
    pub member_count: i32,
}

#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TeamMember {
    pub user_id: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub role: String,
}

pub async fn list_teams(pg: &PgPool, user_id: &str) -> Result<Vec<Team>, sqlx::Error> {
    let rows: Vec<(String, String, String, i64, i32)> = sqlx::query_as(
        "select t.id::text, t.name, m.role, \
                (trunc(extract(epoch from t.created_at) * 1000))::bigint, \
                (select count(*)::int from team_members x where x.team_id = t.id) \
         from teams t join team_members m on m.team_id = t.id and m.user_id = $1::uuid \
         order by t.name asc",
    )
    .bind(user_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(id, name, role, created_ms, member_count)| Team {
            id,
            name,
            role,
            member_count,
            created_at: epoch_ms_to_iso(created_ms),
        })
        .collect())
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
