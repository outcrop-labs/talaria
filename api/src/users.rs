// Users, view denials, and fine-grained permissions — port of the parts of
// ui/src/server/users.ts + permissions.ts + apps.ts (appViewRoutes) that the
// session/auth plane needs, plus the batch-3 substrate: actingUser, the
// assistant owner/elevation grants, hasPerm. The admin console queries port
// with the route groups that use them.
//
// Two recorded divergences, both order-only and both inherited from the
// gateway precedent (docs/RUST-MIGRATION.md):
//   • discoveredApps() sorts names with localeCompare; this is byte order —
//     agrees on ASCII names, which is every app slug's neighborhood.
//   • TS discovers apps via build-time import.meta.glob; this reads
//     apps/<slug>/talaria.json from disk. In dev both see the same directory;
//     the difference is only visible to a build that compiled an app in and
//     then had its source tree scrubbed — not a state a deployment can reach.

use crate::agent_auth::{AgentSubject, subject_model, subject_proven};
use crate::gateway::settings::get_setting;
use crate::state::AppState;
use axum::http::HeaderMap;
use axum::response::Response;
use sqlx::{PgPool, Row};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

/// The sign-in identity a provider hands us — users.ts's Identity.
#[derive(Debug, Clone)]
pub struct Identity {
    pub sub: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub picture: Option<String>,
}

/// Upsert the identity into `users` (users.ts's upsertUser): a sign-in assigns
/// 'member' to a brand-new sub and never touches an existing role — the first
/// admin comes from the claim, every later one from Admin → People. Returns
/// the row in select order (id, sub, email, name, picture, role) — the
/// SessionUser constructor's input, exactly TS's `upsertUserRow`.
pub async fn upsert_user(
    pg: &PgPool,
    identity: &Identity,
) -> Result<
    (
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
    ),
    sqlx::Error,
> {
    let row = sqlx::query_as::<_, (String, String, Option<String>, Option<String>, Option<String>, String)>(
        "insert into users (sub, email, name, picture, role, last_seen_at) \
         values ($1, $2, $3, $4, 'member', now()) \
         on conflict (sub) do update set \
           email = excluded.email, \
           name = case \
             when users.name is null or users.name = '' or users.name = users.email then excluded.name \
             else users.name \
           end, \
           picture = excluded.picture, \
           last_seen_at = now() \
         returning id::text, sub, email, name, picture, role",
    )
    .bind(&identity.sub)
    .bind(&identity.email)
    .bind(&identity.name)
    .bind(&identity.picture)
    .fetch_one(pg)
    .await?;
    // Org-wide boards (the workspace Helpdesk) are everyone's by definition, so
    // a sign-in joins this user to any they lack. Never fatal — a user who
    // could not be joined still signs in; the next login retries.
    if let Err(e) = join_org_wide_boards(pg, &row.0).await {
        tracing::error!("[users] could not join {} to org-wide boards: {e}", row.0);
    }
    Ok(row)
}

pub async fn join_org_wide_boards(pg: &PgPool, user_id: &str) -> Result<u64, sqlx::Error> {
    sqlx::query(
        "insert into board_members (board_id, user_id, role) \
         select b.id, $1::uuid, 'editor' from boards b where b.org_wide \
         on conflict (board_id, user_id) do nothing",
    )
    .bind(user_id)
    .execute(pg)
    .await
    .map(|r| r.rows_affected())
}

/// users.ts getUserRole: an unknown row is a member, never an error.
pub async fn get_user_role(pg: &PgPool, user_id: &str) -> Result<String, sqlx::Error> {
    let row: Option<(String,)> = sqlx::query_as("select role from users where id = $1::uuid")
        .bind(user_id)
        .fetch_optional(pg)
        .await?;
    Ok(row.map(|(role,)| role).unwrap_or_else(|| "member".into()))
}

/// Preferred gateway model for AI drafting (muse); None = server default.
pub async fn get_preferred_model(
    pg: &PgPool,
    user_id: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("select preferred_model as m from users where id = $1::uuid")
            .bind(user_id)
            .fetch_optional(pg)
            .await?;
    Ok(row.and_then(|(m,)| m))
}

/// Everyone who has signed in, for people pickers — users.ts's listUsers.
/// Fields in select order; the order-by is the TS query's own lower(coalesce).
pub async fn list_users(
    pg: &PgPool,
) -> Result<Vec<(String, Option<String>, Option<String>)>, sqlx::Error> {
    sqlx::query_as::<_, (String, Option<String>, Option<String>)>(
        "select id::text, email, name from users order by lower(coalesce(email, name, '')) asc",
    )
    .fetch_all(pg)
    .await
}

// ── View denials ─────────────────────────────────────────────────────────────

/// Manage-section routes: default DENIED for members, granted explicitly via
/// allowed_manage_views. Enabled apps extend this set with EVERY app view
/// (work and manage) — apps are explicit-grant only.
const MANAGE_VIEW_ROUTES: [&str; 6] = [
    "/agents",
    "/models",
    "/mcp",
    "/templates",
    "/observability",
    "/apps",
];

/// Apps' slug rule (apps.ts SLUG_RE): lowercase letters, digits, dashes; a
/// letter-or-digit head; ≤64 chars total.
pub(crate) fn slug_ok(s: &str) -> bool {
    let b = s.as_bytes();
    if b.is_empty() || b.len() > 64 {
        return false;
    }
    let head = b[0].is_ascii_lowercase() || b[0].is_ascii_digit();
    let rest = b[1..]
        .iter()
        .all(|&c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == b'-');
    head && rest
}

/// Where app codebases live (apps.ts appsDir): TALARIA_APPS_DIR, else
/// <cwd>/../apps — the repo's apps/ dir when the process runs from ui/ (TS)
/// or api/ (here).
pub(crate) fn apps_dir() -> PathBuf {
    match std::env::var("TALARIA_APPS_DIR") {
        Ok(d) => PathBuf::from(d),
        Err(_) => std::env::current_dir()
            .ok()
            .and_then(|c| c.parent().map(|p| p.to_path_buf()))
            .unwrap_or_default()
            .join("apps"),
    }
}

pub(crate) struct DiscoveredApp {
    pub(crate) slug: String,
    pub(crate) name: String,
    pub(crate) icon: String,
    pub(crate) description: String,
    pub(crate) version: String,
    pub(crate) work: Option<String>,
    pub(crate) manage: Option<String>,
    pub(crate) settings: Option<String>,
    /// The app publishes MCP tools for agents (apps/<slug>/mcp.ts).
    pub(crate) mcp: bool,
}

/// The manifests on disk, sorted by app name — this port's discoveredApps().
/// Any unreadable/unparseable directory is skipped, like the glob that sees
/// nothing there. Defaults follow the TS String(x ?? default) coercion.
pub(crate) fn discovered_apps() -> Vec<DiscoveredApp> {
    let Ok(entries) = std::fs::read_dir(apps_dir()) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for e in entries.flatten() {
        if !e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let slug = e.file_name().to_string_lossy().into_owned();
        if !slug_ok(&slug) {
            continue;
        }
        let Ok(raw) = std::fs::read_to_string(e.path().join("talaria.json")) else {
            continue;
        };
        let Ok(j) = serde_json::from_str::<serde_json::Value>(&raw) else {
            continue;
        };
        // A truthy name is what admits an app at all; surface keys count only
        // when they hold non-empty strings (JS truthiness in the manifest
        // spread).
        let Some(name) = j
            .get("name")
            .and_then(|v| v.as_str())
            .filter(|n| !n.is_empty())
        else {
            continue;
        };
        let str_field = |key: &str, default: &str| {
            j.get(key)
                .and_then(|v| v.as_str())
                .map(String::from)
                .unwrap_or_else(|| default.to_string())
        };
        // Surface keys are OMITTED unless they hold a non-empty string —
        // TS spreads only the truthy ones, so the key is absent, not null.
        let surface = |key: &str| {
            j.get("surfaces")
                .and_then(|s| s.get(key))
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(String::from)
        };
        out.push(DiscoveredApp {
            mcp: e.path().join("mcp.ts").exists(),
            slug,
            name: name.to_string(),
            icon: str_field("icon", "⬡"),
            description: str_field("description", ""),
            version: str_field("version", "0.0.0"),
            work: surface("work"),
            manage: surface("manage"),
            settings: surface("settings"),
        });
    }
    out.sort_by(|a, b| a.name.cmp(&b.name));
    out
}

/// One enabled app as /api/apps serves it — AppManifest's wire order, absent
/// surface keys absent (never null).
#[derive(serde::Serialize)]
pub struct WireApp {
    pub slug: String,
    pub name: String,
    pub icon: String,
    pub description: String,
    pub version: String,
    pub surfaces: WireSurfaces,
    pub mcp: bool,
}

#[derive(serde::Serialize)]
pub struct WireSurfaces {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub work: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub manage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings: Option<String>,
}

/// The signed-in view of installed apps (apps.ts enabledApps): ENABLED apps
/// only, in discovered (name) order — the platform's own menu, not a secret;
/// per-user view gating happens off deniedViews client-side.
pub async fn enabled_apps(pg: &PgPool) -> Vec<WireApp> {
    let enabled: HashSet<String> =
        get_setting(pg, "apps_enabled", serde_json::Value::Array(vec![]))
            .await
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
    discovered_apps()
        .into_iter()
        .filter(|a| enabled.contains(&a.slug))
        .map(|a| WireApp {
            slug: a.slug,
            name: a.name,
            icon: a.icon,
            description: a.description,
            version: a.version,
            surfaces: WireSurfaces {
                work: a.work,
                manage: a.manage,
                settings: a.settings,
            },
            mcp: a.mcp,
        })
        .collect()
}

/// ALL app view routes of ENABLED apps (apps.ts appViewRoutes): every work
/// surface, then every manage surface — apps are explicit-grant.
pub async fn app_view_routes(pg: &PgPool) -> Vec<String> {
    let enabled: HashSet<String> =
        get_setting(pg, "apps_enabled", serde_json::Value::Array(vec![]))
            .await
            .as_array()
            .map(|a| {
                a.iter()
                    .filter_map(|v| v.as_str())
                    .map(String::from)
                    .collect()
            })
            .unwrap_or_default();
    let apps: Vec<_> = discovered_apps()
        .into_iter()
        .filter(|a| enabled.contains(&a.slug))
        .collect();
    let mut out = Vec::new();
    for a in apps.iter().filter(|a| a.work.is_some()) {
        out.push(format!("/x/{}", a.slug));
    }
    for a in apps.iter().filter(|a| a.manage.is_some()) {
        out.push(format!("/x/{}/manage", a.slug));
    }
    out
}

/// Views a member may NOT reach (users.ts deniedViews): their explicit
/// work-view denials (DB order) PLUS every Manage view they haven't been
/// granted. Admins are never restricted.
pub async fn denied_views(
    pg: &PgPool,
    user_id: &str,
    role: &str,
) -> Result<Vec<String>, sqlx::Error> {
    if role == "admin" {
        return Ok(Vec::new());
    }
    let row: Option<(Vec<String>, Option<Vec<String>>)> =
        sqlx::query_as("select denied_views, allowed_manage_views from users where id = $1::uuid")
            .bind(user_id)
            .fetch_optional(pg)
            .await?;
    let (denied, allowed) = row.unwrap_or_default();
    let allowed: HashSet<String> = allowed.unwrap_or_default().into_iter().collect();
    let mut out = denied;
    out.extend(
        MANAGE_VIEW_ROUTES
            .into_iter()
            .map(String::from)
            .chain(app_view_routes(pg).await)
            .filter(|v| !allowed.contains(v)),
    );
    Ok(out)
}

// ── Admin console writes (users.ts listUsersAdmin and the set* family) ───────

/// One row of the admin console's user list, in the TS select's wire order.
/// Hand-built JSON for that order (node-pg's row is a literal of the SELECT);
/// timestamps take the epoch-ms → ISO route to stay byte-exact with node's
/// Date serialization.
pub async fn list_users_admin(pg: &PgPool) -> Result<Vec<serde_json::Value>, sqlx::Error> {
    let rows = sqlx::query(
        "select u.id::text, u.email, u.name, u.role, u.can_mint_keys, u.denied_views, \
                coalesce(u.allowed_manage_views, '{}'), \
                (trunc(extract(epoch from u.last_seen_at) * 1000))::bigint, \
                (trunc(extract(epoch from u.created_at) * 1000))::bigint, \
                exists(select 1 from user_password_credentials c where c.user_id = u.id), \
                coalesce(array_agg(a.agent_model) filter (where a.agent_model is not null), '{}'), \
                min(d.model), coalesce(bool_or(d.elevated), false) \
           from users u \
           left join user_agent_access a on a.user_id = u.id \
           left join agent_defs d on d.owner_user_id = u.id \
          group by u.id \
          order by lower(coalesce(u.email, u.name, '')) asc",
    )
    .fetch_all(pg)
    .await?;
    use crate::agent_auth::epoch_ms_to_iso;
    use serde_json::Value;
    let iso = |ms: Option<i64>| match ms {
        Some(ms) => Value::String(epoch_ms_to_iso(ms)),
        None => Value::Null,
    };
    let mut out = Vec::new();
    for r in &rows {
        let mut f = serde_json::Map::new();
        f.insert("id".into(), Value::String(r.try_get(0)?));
        f.insert(
            "email".into(),
            r.try_get::<Option<String>, _>(1)?.map(Value::String).unwrap_or(Value::Null),
        );
        f.insert(
            "name".into(),
            r.try_get::<Option<String>, _>(2)?.map(Value::String).unwrap_or(Value::Null),
        );
        f.insert("role".into(), Value::String(r.try_get(3)?));
        f.insert("canMintKeys".into(), Value::Bool(r.try_get(4)?));
        f.insert(
            "deniedViews".into(),
            serde_json::to_value(r.try_get::<Vec<String>, _>(5)?).unwrap_or(Value::Array(vec![])),
        );
        f.insert(
            "allowedManageViews".into(),
            serde_json::to_value(r.try_get::<Vec<String>, _>(6)?).unwrap_or(Value::Array(vec![])),
        );
        f.insert("lastSeenAt".into(), iso(r.try_get(7)?));
        f.insert("createdAt".into(), iso(r.try_get(8)?));
        f.insert("hasPasswordAccount".into(), Value::Bool(r.try_get(9)?));
        f.insert(
            "agentModels".into(),
            serde_json::to_value(r.try_get::<Vec<String>, _>(10)?).unwrap_or(Value::Array(vec![])),
        );
        f.insert(
            "assistantModel".into(),
            r.try_get::<Option<String>, _>(11)?.map(Value::String).unwrap_or(Value::Null),
        );
        f.insert("assistantElevated".into(), Value::Bool(r.try_get(12)?));
        out.push(Value::Object(f));
    }
    Ok(out)
}

pub async fn set_user_role(pg: &PgPool, user_id: &str, role: &str) -> Result<(), sqlx::Error> {
    sqlx::query("update users set role = $1 where id = $2::uuid")
        .bind(role)
        .bind(user_id)
        .execute(pg)
        .await?;
    Ok(())
}

/// Admins currently holding the role — the last-admin guard's input.
pub async fn admin_count(pg: &PgPool) -> Result<i32, sqlx::Error> {
    Ok(sqlx::query_scalar::<_, i32>("select count(*)::int from users where role = 'admin'")
        .fetch_one(pg)
        .await
        .unwrap_or(0))
}

/// Grant/revoke the ability to mint LLM-gateway API keys (admins always may).
pub async fn set_user_can_mint_keys(
    pg: &PgPool,
    user_id: &str,
    can_mint: bool,
) -> Result<(), sqlx::Error> {
    sqlx::query("update users set can_mint_keys = $1 where id = $2::uuid")
        .bind(can_mint)
        .bind(user_id)
        .execute(pg)
        .await?;
    Ok(())
}

/// Replace a user's agent allow-list. Empty = all agents (open by default).
pub async fn set_user_agent_access(
    pg: &PgPool,
    user_id: &str,
    models: &[String],
) -> Result<(), sqlx::Error> {
    let mut tx = pg.begin().await?;
    sqlx::query("delete from user_agent_access where user_id = $1::uuid")
        .bind(user_id)
        .execute(tx.as_mut())
        .await?;
    for m in models {
        sqlx::query(
            "insert into user_agent_access (user_id, agent_model) values ($1::uuid, $2) \
             on conflict do nothing",
        )
        .bind(user_id)
        .bind(m)
        .execute(tx.as_mut())
        .await?;
    }
    tx.commit().await?;
    Ok(())
}

/// Replace a member's granted Manage views — filtered to the valid routes,
/// so a stale id typed into a request body can't manufacture a grant.
pub async fn set_allowed_manage_views(
    pg: &PgPool,
    user_id: &str,
    views: &[String],
) -> Result<(), sqlx::Error> {
    let valid: Vec<String> = MANAGE_VIEW_ROUTES
        .into_iter()
        .map(String::from)
        .chain(app_view_routes(pg).await)
        .collect();
    let filtered: Vec<&str> = views
        .iter()
        .filter(|v| valid.contains(v))
        .map(String::as_str)
        .collect();
    sqlx::query("update users set allowed_manage_views = $1 where id = $2::uuid")
        .bind(&filtered)
        .bind(user_id)
        .execute(pg)
        .await?;
    Ok(())
}

pub async fn set_denied_views(
    pg: &PgPool,
    user_id: &str,
    views: &[String],
) -> Result<(), sqlx::Error> {
    sqlx::query("update users set denied_views = $1 where id = $2::uuid")
        .bind(views)
        .bind(user_id)
        .execute(pg)
        .await?;
    Ok(())
}

/// Flip org-wide elevation on a user's personal assistant. Returns false if
/// the user has no assistant. Elevating requires the owner to be an admin
/// (the route checks that half).
pub async fn set_assistant_elevated(
    pg: &PgPool,
    user_id: &str,
    elevated: bool,
) -> Result<bool, sqlx::Error> {
    let n = sqlx::query("update agent_defs set elevated = $1 where owner_user_id = $2::uuid")
        .bind(elevated)
        .bind(user_id)
        .execute(pg)
        .await?
        .rows_affected();
    Ok(n > 0)
}

// ── Permissions (permissions.ts) ─────────────────────────────────────────────
// The catalog and the resolution chain live in permissions.rs since the admin
// console routes landed — one source for the admin GET's full entries and the
// session's resolved ids.

pub use crate::permissions::{has_perm, user_permissions};

// ── Who a request acts AS (users.ts actingUser and the assistant grants) ─────

#[allow(dead_code)] // crosses with the write-plane batches' act-as routes
#[derive(Debug, Clone)]
pub struct ActingUser {
    pub id: String,
    pub role: String,
    /// For attribution: the human, or "<assistant> (for <human>)".
    pub label: String,
    pub via_assistant: bool,
    /// Admin-elevated assistant (org-wide view/edit; owner must be an admin).
    /// Always false for humans — a human admin's access is unchanged.
    pub elevated: bool,
}

/// Who a request acts AS: the signed-in human — or, for a PERSONAL assistant
/// calling with its own credential, its owner (the identity-proxy model: your
/// assistant manages your boards for you). General agents resolve to None;
/// governance actions stay human(-proxied). An agent-credential REJECTION
/// also resolves to None (TS: `instanceof Response → null`) — the dual-auth
/// routes that need the refusal itself read `agent_caller` directly.
#[allow(dead_code)] // crosses with the write-plane batches' act-as routes
pub async fn acting_user(
    state: &AppState,
    headers: &HeaderMap,
) -> Result<Option<ActingUser>, Response> {
    match crate::agent_auth::agent_caller(&state.pg, headers).await {
        Err(_) => return Ok(None),
        Ok(Some(agent)) => {
            // Proxying a human — and inheriting their admin role — is the one
            // thing a self-declared name must never buy. agent-auth already
            // refuses a legacy caller that CLAIMS a personal-assistant name,
            // so this is unreachable today and stays as the guarantee for
            // anything that loosens the door.
            if agent.legacy {
                return Ok(None);
            }
            // (id, role, email, name, elevated) — the owner row a personal
            // assistant proxies.
            type OwnerRow = (String, String, Option<String>, Option<String>, bool);
            let row: Option<OwnerRow> = sqlx::query_as(
                "select u.id::text, u.role, u.email, u.name, d.elevated from agent_defs d \
                 join users u on u.id = d.owner_user_id \
                 where d.model = $1 and d.owner_user_id is not null",
            )
            .bind(&agent.model)
            .fetch_optional(&state.pg)
            .await
            .map_err(|e| internal(&e))?;
            let Some((id, role, email, name, elevated)) = row else {
                return Ok(None); // not a personal assistant → no proxied identity
            };
            let who = email.or(name).unwrap_or_else(|| id.clone());
            return Ok(Some(ActingUser {
                id,
                role: role.clone(),
                label: format!("{} (for {who})", agent.model),
                via_assistant: true,
                // Elevation only bites while the owner is still an admin —
                // demote the human and the assistant's reach collapses.
                elevated: elevated && role == "admin",
            }));
        }
        Ok(None) => {}
    }
    let user = match crate::session::get_session_user(state, headers).await {
        Ok(Some(u)) => u,
        Ok(None) => return Ok(None),
        Err(e) => {
            tracing::error!("[users] session read failed: {e}");
            return Err(internal_msg());
        }
    };
    Ok(Some(ActingUser {
        label: user
            .email
            .clone()
            .or(user.name.clone())
            .unwrap_or_else(|| "user".into()),
        id: user.id,
        role: user.role,
        via_assistant: false,
        elevated: false,
    }))
}

/// True only for a personal assistant an admin explicitly promoted AND whose
/// owner is currently an admin. Gates org-wide agent access. Takes the
/// SUBJECT, never a bare name: elevation is the largest grant an agent
/// identity carries, so it is never handed to an identity that was merely
/// asserted (legacy shared-key caller).
pub async fn is_elevated_assistant(
    pg: &PgPool,
    subject: &AgentSubject,
) -> Result<bool, sqlx::Error> {
    if !subject_proven(subject) {
        return Ok(false);
    }
    let row: Option<(i32,)> = sqlx::query_as(
        "select 1 from agent_defs d join users u on u.id = d.owner_user_id \
         where d.model = $1 and d.elevated and u.role = 'admin'",
    )
    .bind(subject_model(subject))
    .fetch_optional(pg)
    .await?;
    Ok(row.is_some())
}

/// model → owner_user_id for every PERSONAL assistant. Listing helper — for a
/// per-CALLER decision use `assistant_owner_for`.
pub async fn personal_assistant_owners(
    pg: &PgPool,
) -> Result<HashMap<String, String>, sqlx::Error> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "select model, owner_user_id::text from agent_defs where owner_user_id is not null",
    )
    .fetch_all(pg)
    .await?;
    Ok(rows.into_iter().collect())
}

/// The owner a personal assistant acts for, or None. The identity-proxy reach
/// this answers is the OWNER'S OWN view (their memberships, their DMs), not
/// org-wide: that larger grant is `elevated` on the agent_defs row and stays
/// gated by `is_elevated_assistant`. Demands a PROVEN subject — a legacy
/// caller gets None: identified, but not proven to BE that assistant.
pub async fn assistant_owner_for(
    pg: &PgPool,
    subject: &AgentSubject,
) -> Result<Option<String>, sqlx::Error> {
    if !subject_proven(subject) {
        return Ok(None);
    }
    Ok(personal_assistant_owners(pg)
        .await?
        .get(subject_model(subject))
        .cloned())
}

#[allow(dead_code)] // acting_user's error arm
fn internal(e: &sqlx::Error) -> Response {
    tracing::error!("[users] database read failed: {e}");
    internal_msg()
}

#[allow(dead_code)] // acting_user's error arm
fn internal_msg() -> Response {
    crate::error::thrown_internal_error()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slug_rules_match_the_regex() {
        assert!(slug_ok("a"));
        assert!(slug_ok("0-9"));
        assert!(slug_ok("helpdesk-2"));
        assert!(slug_ok(&"a".repeat(64)));
        assert!(!slug_ok(&"a".repeat(65)));
        assert!(!slug_ok("")); // regex needs a head char
        assert!(!slug_ok("-x"));
        assert!(!slug_ok("Upper"));
        assert!(!slug_ok("under_score"));
        assert!(!slug_ok("sp ace"));
    }

    #[test]
    fn permission_catalog_is_the_wire_order() {
        let ids: Vec<&str> = crate::permissions::PERMISSIONS
            .iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(
            ids,
            vec![
                "agents.manage",
                "research.run",
                "plans.create",
                "boards.create",
                "comms.channels",
                "comms.relays",
                "kb.edit",
                "kb.official",
                "artifacts.create",
                "artifacts.publish",
                "files.upload",
                "templates.manage",
                "models.mint-keys",
            ]
        );
        // And the member-default map matches permissions.ts exactly.
        let defaults: Vec<(&str, bool)> = crate::permissions::PERMISSIONS
            .iter()
            .filter(|p| p.member_default)
            .map(|p| (p.id, true))
            .collect();
        assert_eq!(
            defaults,
            vec![
                ("research.run", true),
                ("plans.create", true),
                ("boards.create", true),
                ("comms.channels", true),
                ("comms.relays", true),
                ("kb.edit", true),
                ("artifacts.create", true),
                ("files.upload", true),
            ]
        );
    }
}
