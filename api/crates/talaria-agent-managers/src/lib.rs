// Agent managers — who OWNS an agent.
//
// `agents.manage` says a person may run the fleet: hire, wire endpoints,
// register MCP servers, keep the role library. It never said WHICH agents
// they may change, so every holder could rewrite every agent's soul, rotate
// its secrets and retire it. An agent is somebody's: the people named here
// are the ones who may change this one.
//
// The rule, in three lines:
//
//   * an admin manages every agent (the instance is theirs to run, and a
//     manager who leaves the org must not take an agent with them);
//   * a named manager manages that agent — being named IS the grant, no
//     `agents.manage` needed, and it opens the /agents view for them;
//   * nobody else may change it, `agents.manage` or not.
//
// `agent_defs.owner_user_id` — the personal-assistant owner — rides as a
// legacy arm of the same question. The migration writes those owners in as
// managers, and an assistant minted by some path that forgets the row still
// answers to its human.

use serde::Serialize;
use sqlx::PgPool;
use std::collections::{HashMap, HashSet};

/// One manager as the API serves it. `role` is the person's org role, so the
/// roster can say which managers are admins (who hold reach regardless).
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Manager {
    pub user_id: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub role: String,
}

/// Postgres casts the binds (`$1::uuid`), and a path segment that is not a
/// uuid makes the cast — not the predicate — fail, which would surface as a
/// 500 on a typo'd URL. A shape check first turns that into the plain "no,
/// you don't manage that" it actually is.
fn is_uuid(s: &str) -> bool {
    let b = s.as_bytes();
    b.len() == 36
        && b.iter().enumerate().all(|(i, c)| match i {
            8 | 13 | 18 | 23 => *c == b'-',
            _ => c.is_ascii_hexdigit(),
        })
}

/// May this person change this agent? The one question every per-agent
/// mutation asks.
pub async fn manages_agent(
    pg: &PgPool,
    user_id: &str,
    role: &str,
    agent_id: &str,
) -> Result<bool, sqlx::Error> {
    if role == "admin" {
        return Ok(true);
    }
    if !is_uuid(agent_id) {
        return Ok(false);
    }
    let found: Option<i32> = sqlx::query_scalar(
        "select 1 from agent_defs d \
         where d.id = $1::uuid \
           and (d.owner_user_id = $2::uuid \
                or exists (select 1 from agent_managers m \
                           where m.agent_id = d.id and m.user_id = $2::uuid))",
    )
    .bind(agent_id)
    .bind(user_id)
    .fetch_optional(pg)
    .await?;
    Ok(found.is_some())
}

/// The same question keyed by the agent's gateway model id.
pub async fn manages_agent_model(
    pg: &PgPool,
    user_id: &str,
    role: &str,
    model: &str,
) -> Result<bool, sqlx::Error> {
    if role == "admin" {
        return Ok(true);
    }
    if model.is_empty() {
        return Ok(false);
    }
    let found: Option<i32> = sqlx::query_scalar(
        "select 1 from agent_defs d \
         where d.model = $1 \
           and (d.owner_user_id = $2::uuid \
                or exists (select 1 from agent_managers m \
                           where m.agent_id = d.id and m.user_id = $2::uuid))",
    )
    .bind(model)
    .bind(user_id)
    .fetch_optional(pg)
    .await?;
    Ok(found.is_some())
}

/// The same question keyed by the agent's slug — the skills plane's handle
/// for an agent (`/api/skills/{owner}/{name}`).
pub async fn manages_agent_slug(
    pg: &PgPool,
    user_id: &str,
    role: &str,
    slug: &str,
) -> Result<bool, sqlx::Error> {
    if role == "admin" {
        return Ok(true);
    }
    if slug.is_empty() {
        return Ok(false);
    }
    let found: Option<i32> = sqlx::query_scalar(
        "select 1 from agent_defs d \
         where d.slug = $1 \
           and (d.owner_user_id = $2::uuid \
                or exists (select 1 from agent_managers m \
                           where m.agent_id = d.id and m.user_id = $2::uuid))",
    )
    .bind(slug)
    .bind(user_id)
    .fetch_optional(pg)
    .await?;
    Ok(found.is_some())
}

/// Does this person manage ANY agent? What opens the /agents view and the
/// roster read for someone who holds no `agents.manage`.
pub async fn manages_any_agent(
    pg: &PgPool,
    user_id: &str,
    role: &str,
) -> Result<bool, sqlx::Error> {
    if role == "admin" {
        return Ok(true);
    }
    let found: Option<i32> = sqlx::query_scalar(
        "select 1 from agent_defs d \
         where d.owner_user_id = $1::uuid \
            or exists (select 1 from agent_managers m \
                       where m.agent_id = d.id and m.user_id = $1::uuid) \
         limit 1",
    )
    .bind(user_id)
    .fetch_optional(pg)
    .await?;
    Ok(found.is_some())
}

/// Does this person read the WHOLE fleet — every agent, every endpoint —
/// rather than only the agents they manage? Admins and `agents.manage`.
///
/// This is a WIDENING, not a gate. The roster and the container read are
/// gated on the session; this says how much of the fleet the answer carries,
/// and the per-agent routes never ask it — `agents.manage` runs the fleet,
/// it does not confer the right to change any particular agent.
pub async fn reads_whole_fleet(
    pg: &PgPool,
    user_id: &str,
    role: &str,
) -> Result<bool, sqlx::Error> {
    if role == "admin" {
        return Ok(true);
    }
    talaria_permissions::has_perm(pg, user_id, role, "agents.manage").await
}

/// The def ids this person manages — what narrows the roster to their own
/// agents. Admins are not special-cased here: a caller that holds fleet-wide
/// reach skips the filter rather than asking for every id.
pub async fn managed_agent_ids(pg: &PgPool, user_id: &str) -> Result<HashSet<String>, sqlx::Error> {
    let rows: Vec<(String,)> = sqlx::query_as(
        "select d.id::text from agent_defs d \
         where d.owner_user_id = $1::uuid \
            or exists (select 1 from agent_managers m \
                       where m.agent_id = d.id and m.user_id = $1::uuid)",
    )
    .bind(user_id)
    .fetch_all(pg)
    .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

/// One agent's managers, in roster order (name, then email).
pub async fn list_managers(pg: &PgPool, agent_id: &str) -> Result<Vec<Manager>, sqlx::Error> {
    if !is_uuid(agent_id) {
        return Ok(Vec::new());
    }
    let rows: Vec<(String, Option<String>, Option<String>, String)> = sqlx::query_as(
        "select u.id::text, u.email, u.name, u.role \
         from agent_managers m join users u on u.id = m.user_id \
         where m.agent_id = $1::uuid \
         order by lower(coalesce(u.name, u.email, '')) asc",
    )
    .bind(agent_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(|(user_id, email, name, role)| Manager {
            user_id,
            email,
            name,
            role,
        })
        .collect())
}

/// Every agent's managers in one read, keyed by def id — the roster's
/// version of `list_managers` (one query, not one per tile).
pub async fn managers_by_agent(pg: &PgPool) -> Result<HashMap<String, Vec<Manager>>, sqlx::Error> {
    let rows: Vec<(String, String, Option<String>, Option<String>, String)> = sqlx::query_as(
        "select m.agent_id::text, u.id::text, u.email, u.name, u.role \
         from agent_managers m join users u on u.id = m.user_id \
         order by m.agent_id, lower(coalesce(u.name, u.email, '')) asc",
    )
    .fetch_all(pg)
    .await?;
    let mut out: HashMap<String, Vec<Manager>> = HashMap::new();
    for (agent_id, user_id, email, name, role) in rows {
        out.entry(agent_id).or_default().push(Manager {
            user_id,
            email,
            name,
            role,
        });
    }
    Ok(out)
}

/// Name one more manager. Idempotent — naming someone twice is not an error,
/// and the first naming keeps its `added_by`.
pub async fn add_manager(
    pg: &PgPool,
    agent_id: &str,
    user_id: &str,
    added_by: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "insert into agent_managers (agent_id, user_id, added_by) \
         values ($1::uuid, $2::uuid, $3::uuid) on conflict do nothing",
    )
    .bind(agent_id)
    .bind(user_id)
    .bind(added_by)
    .execute(pg)
    .await?;
    Ok(())
}

/// Replace the whole roster, in one transaction — the PUT's shape, so a
/// reader never sees the gap between the delete and the insert. The caller
/// has already refused an empty set: an agent keeps at least one manager.
pub async fn set_managers(
    pg: &PgPool,
    agent_id: &str,
    user_ids: &[String],
    added_by: Option<&str>,
) -> Result<(), sqlx::Error> {
    let mut tx = pg.begin().await?;
    sqlx::query(
        "delete from agent_managers where agent_id = $1::uuid and not (user_id = any($2::uuid[]))",
    )
    .bind(agent_id)
    .bind(user_ids)
    .execute(&mut *tx)
    .await?;
    for user_id in user_ids {
        sqlx::query(
            "insert into agent_managers (agent_id, user_id, added_by) \
             values ($1::uuid, $2::uuid, $3::uuid) on conflict do nothing",
        )
        .bind(agent_id)
        .bind(user_id)
        .bind(added_by)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Which of these user ids exist — the PUT validates its roster before it
/// writes, so a typo'd id is a 400 and not a silent no-op.
pub async fn existing_user_ids(
    pg: &PgPool,
    user_ids: &[String],
) -> Result<HashSet<String>, sqlx::Error> {
    let rows: Vec<(String,)> =
        sqlx::query_as("select id::text from users where id = any($1::uuid[])")
            .bind(user_ids)
            .fetch_all(pg)
            .await?;
    Ok(rows.into_iter().map(|(id,)| id).collect())
}

#[cfg(test)]
mod tests {
    use super::is_uuid;

    #[test]
    fn uuid_shape_accepts_the_real_thing_and_refuses_a_path_typo() {
        assert!(is_uuid("3f2504e0-4f89-11d3-9a0c-0305e82c3301"));
        assert!(is_uuid("3F2504E0-4F89-11D3-9A0C-0305E82C3301"));
        // The shapes a URL actually carries when someone mistypes it.
        assert!(!is_uuid(""));
        assert!(!is_uuid("undefined"));
        assert!(!is_uuid("3f2504e0-4f89-11d3-9a0c-0305e82c330"));
        assert!(!is_uuid("3f2504e0-4f89-11d3-9a0c-0305e82c33011"));
        assert!(!is_uuid("3f2504e0:4f89-11d3-9a0c-0305e82c3301"));
        assert!(!is_uuid("3f2504e0-4f89-11d3-9a0c-0305e82c330g"));
    }
}
