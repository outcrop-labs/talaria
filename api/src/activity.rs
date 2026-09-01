// The fleet-wide activity feed — port of ui/src/server/activity-feed.ts. One
// merged stream of what's been happening across the workspace, scoped to the
// requesting user. Three sources (plus an admins-only fourth):
//   • ticket events (task_activity)  — only boards the user can access
//   • channel messages               — only channels the user is a member of
//   • agent config changes (agent_versions) — visible to everyone (the fleet
//     is shared infrastructure; /agents itself isn't admin-gated)
//   • audit events                    — governance data, admins only
// No new tables — this is a read model over what the app already records.

use crate::agent_auth::epoch_ms_to_iso;
use crate::boards::board_visibility_sql;
use sqlx::PgPool;

/// The event kinds the route's `kinds` filter admits; anything else in the
/// query string is dropped (activity-feed.ts's KINDS set).
pub const KINDS: [&str; 4] = ["ticket", "channel", "fleet", "audit"];

#[derive(Debug, Clone, serde::Serialize)]
pub struct ActivityEvent {
    /// toISOString of the source row's timestamp.
    pub at: String,
    pub kind: &'static str,
    /// Who did it — email, agent model, or 'system'.
    pub actor: String,
    /// Where it happened — board name, #channel, or agent name.
    pub context: String,
    /// What happened, human-sized.
    pub detail: String,
    /// The event's own type within its source — 'dispatch', 'status', 'gap',
    /// an audit action like 'fleet.render' — for row labels and sub-filters.
    #[serde(rename = "type")]
    pub event_type: String,
    pub href: String,
}

/// The number the events sort by, kept next to the shaped event so the final
/// merge orders by time, not by ISO text.
struct Stamped {
    at_ms: i64,
    event: ActivityEvent,
}

pub async fn activity_feed(
    pg: &PgPool,
    user_id: &str,
    kinds: &[String],
    limit: i64,
    is_admin: bool,
) -> Result<Vec<ActivityEvent>, sqlx::Error> {
    // Audit events are governance data — admins only; the default set
    // excludes them.
    let default_kinds = ["ticket", "channel", "fleet"];
    let selected: Vec<&str> = if !kinds.is_empty() {
        kinds.iter().map(String::as_str).collect()
    } else {
        default_kinds.to_vec()
    };
    let want = |k: &str| selected.contains(&k) && (k != "audit" || is_admin);
    // per = Math.min(limit, 80). Inlined into the LIMIT clauses below — it is
    // min(route constant, 80), never caller text (the same discipline as
    // gateway/usage.rs's PRICED).
    let per = limit.min(80);

    let mut stamped: Vec<Stamped> = Vec::new();

    // (at_ms, actor, type, board, title, detail, board_id, task_id)
    type TicketRow = (i64, String, String, String, String, String, String, String);

    if want("ticket") {
        // AssertSqlSafe: the interpolations are this crate's visibility-SQL
        // builder and the capped LIMIT — no caller text.
        let sql = format!(
            "select (trunc(extract(epoch from a.created_at) * 1000))::bigint as at_ms, \
                    a.actor, coalesce(a.type, 'activity') as type, b.name as board, t.title, \
                    a.description as detail, b.id::text as board_id, t.id::text as task_id \
             from task_activity a \
             join tasks t on t.id = a.task_id \
             join boards b on b.id = t.board_id \
             where {} \
             order by a.created_at desc limit {}",
            board_visibility_sql("$1", "$2", false),
            per
        );
        let rows: Vec<TicketRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
            .bind(user_id)
            .bind(user_id)
            .fetch_all(pg)
            .await?;
        for (at_ms, actor, ty, board, title, detail, board_id, task_id) in rows {
            stamped.push(Stamped {
                at_ms,
                event: ActivityEvent {
                    at: epoch_ms_to_iso(at_ms),
                    kind: "ticket",
                    actor,
                    context: board,
                    detail: format!("{title}: {detail}"),
                    event_type: ty,
                    href: format!("/boards/{board_id}/{task_id}"),
                },
            });
        }
    }

    if want("channel") {
        let sql = format!(
            "select (trunc(extract(epoch from m.created_at) * 1000))::bigint as at_ms, \
                    m.author, m.author_type, c.name as channel, left(m.content, 160) as detail \
             from channel_messages m \
             join channels c on c.id = m.channel_id \
             join channel_members cm on cm.channel_id = c.id and cm.user_id = $1::uuid \
             where m.status = 'complete' and m.content <> '' \
             order by m.created_at desc limit {}",
            per
        );
        let rows: Vec<(i64, String, String, String, String)> =
            sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
                .bind(user_id)
                .fetch_all(pg)
                .await?;
        for (at_ms, author, author_type, channel, detail) in rows {
            stamped.push(Stamped {
                at_ms,
                event: ActivityEvent {
                    at: epoch_ms_to_iso(at_ms),
                    kind: "channel",
                    actor: author,
                    context: format!("#{channel}"),
                    detail,
                    event_type: if author_type == "agent" {
                        "agent"
                    } else {
                        "message"
                    }
                    .into(),
                    href: "/channels".into(),
                },
            });
        }
    }

    if want("fleet") {
        let sql = format!(
            "select (trunc(extract(epoch from v.created_at) * 1000))::bigint as at_ms, \
                    coalesce(v.created_by, 'system') as actor, \
                    d.display_name as agent, v.version, coalesce(v.note, '') as note \
             from agent_versions v \
             join agent_defs d on d.id = v.agent_id \
             order by v.created_at desc limit {}",
            per
        );
        let rows: Vec<(i64, String, String, i32, String)> =
            sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
                .fetch_all(pg)
                .await?;
        for (at_ms, actor, agent, version, note) in rows {
            stamped.push(Stamped {
                at_ms,
                event: ActivityEvent {
                    at: epoch_ms_to_iso(at_ms),
                    kind: "fleet",
                    actor,
                    context: agent,
                    detail: if note.is_empty() {
                        format!("config v{version}")
                    } else {
                        format!("config v{version}: {note}")
                    },
                    event_type: "config".into(),
                    href: "/agents".into(),
                },
            });
        }
    }

    // (at_ms, actor, action, target_type, target_label, after)
    type AuditRow = (
        i64,
        String,
        String,
        String,
        Option<String>,
        Option<serde_json::Value>,
    );

    if want("audit") {
        let sql = format!(
            "select (trunc(extract(epoch from created_at) * 1000))::bigint as at_ms, \
                    actor, action, target_type, target_label, after \
             from audit_log order by created_at desc limit {}",
            per
        );
        let rows: Vec<AuditRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql.as_str()))
            .fetch_all(pg)
            .await?;
        for (at_ms, actor, action, target_type, target_label, after) in rows {
            // `${targetLabel ?? targetType}${after ? ` → ${JSON.stringify(after)}` : ''}`
            // .slice(0, 200) — JS slices UTF-16 code units; the labels and
            // actions here are ASCII, so 200 chars is the same cut.
            let mut detail = target_label.unwrap_or(target_type.clone());
            if let Some(a) = &after {
                detail = format!("{detail} → {a}");
            }
            stamped.push(Stamped {
                at_ms,
                event: ActivityEvent {
                    at: epoch_ms_to_iso(at_ms),
                    kind: "audit",
                    actor,
                    context: action.split('.').next().unwrap_or("audit").to_string(),
                    detail: detail.chars().take(200).collect(),
                    event_type: action,
                    href: "/observability/audit".into(),
                },
            });
        }
    }

    // Merge newest-first (JS sort is stable; so is sort_by — ties keep source
    // order: tickets, channels, fleet, audit), then the caller's limit.
    stamped.sort_by_key(|s| std::cmp::Reverse(s.at_ms));
    Ok(stamped
        .into_iter()
        .take(limit.max(0) as usize)
        .map(|s| s.event)
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kinds_catalog_is_the_route_filter() {
        assert_eq!(KINDS, ["ticket", "channel", "fleet", "audit"]);
    }
}
