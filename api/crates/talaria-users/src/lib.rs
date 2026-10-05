// Users, view denials, and fine-grained permissions — the session/auth
// plane's substrate: the assistant owner/elevation grants, has_perm (acting
// itself lives in session.rs). App discovery (the apps/<slug>/talaria.json
// manifests) sits here beside the admin console queries the route groups use.

use sqlx::{PgPool, Row};
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use talaria_agent_auth::{AgentSubject, subject_model, subject_proven};
use talaria_gateway::settings::get_setting;

/// The effective-picture column (see "Profile: photo, status, presence"
/// below), as a SQL expression over a `users` row in
/// scope (unqualified `id`, `avatar_upload_id`, `picture`). A macro so it can
/// be spliced into a `concat!` and the query stays `&'static str`.
#[macro_export]
macro_rules! effective_picture_sql {
    () => {
        "case when avatar_upload_id is not null \
           then '/api/users/' || id::text || '/avatar?v=' || left(avatar_upload_id::text, 8) \
           else picture end"
    };
}

/// The sign-in identity a provider hands us.
#[derive(Debug, Clone)]
pub struct Identity {
    pub sub: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub picture: Option<String>,
}

/// Upsert the identity into `users`: a sign-in assigns 'member' to a
/// brand-new sub and never touches an existing role — the first admin comes
/// from the claim, every later one from Admin → People. When the identity
/// carries an email that already names a person (Google sign-in for a
/// password-claimed address), the sign-in LINKS to that row — the email is
/// the person, not the sub — so one human keeps one row whichever door they
/// use. Returns the row in select order (id, sub, email, name, picture,
/// role) — the session user's input. The `picture` is the EFFECTIVE one
/// (`effective_picture_sql!`): the sign-in rewrites the stored provider
/// picture, but a person's uploaded photo still wins in the session.
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
    let row = match link_by_email(pg, identity, false).await? {
        Some(row) => row,
        None => {
            sqlx::query_as::<_, (String, String, Option<String>, Option<String>, Option<String>, String)>(
                concat!(
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
                     returning id::text, sub, email, name, ",
                    effective_picture_sql!(),
                    ", role"
                ),
            )
            .bind(&identity.sub)
            .bind(&identity.email)
            .bind(&identity.name)
            .bind(&identity.picture)
            .fetch_one(pg)
            .await?
        }
    };
    // Org-wide boards (the workspace Helpdesk) are everyone's by definition, so
    // a sign-in joins this user to any they lack. Never fatal — a user who
    // could not be joined still signs in; the next login retries.
    if let Err(e) = join_org_wide_boards(pg, &row.0).await {
        tracing::error!("[users] could not join {} to org-wide boards: {e}", row.0);
    }
    Ok(row)
}

/// Link a sign-in identity to the person its email already names, taking over
/// the row's sub so both doors lead to one row. `promote` also raises the row
/// to admin — the claim's whole job — while a plain sign-in never touches
/// role, exactly like the insert's on-conflict arm. Returns None when there
/// is no email or no same-email row: a brand-new person.
///
/// The email is a safe link key because every provider here verifies it
/// before we see it (Google refuses unverified addresses; a password account
/// is minted by an admin) — the same trust `create_password_account` links
/// on. An admin row wins over an earlier same-email member so a fork
/// resolves toward the claimed person, matching the merge migration's
/// survivor rule.
///
/// The sub takeover is guarded: when some OTHER row already holds the
/// incoming sub — an unmerged fork from before this link existed — the found
/// row keeps its own sub rather than tripping the unique constraint. The
/// fork is the boot migration's to merge, not the sign-in's to guess at.
/// (Two sign-ins racing the same brand-new sub can still collide once; the
/// loser's error page retries clean.)
pub async fn link_by_email<'e, E>(
    exe: E,
    identity: &Identity,
    promote: bool,
) -> Result<
    Option<(
        String,
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        String,
    )>,
    sqlx::Error,
>
where
    E: sqlx::Executor<'e, Database = sqlx::Postgres>,
{
    let email = identity
        .email
        .as_deref()
        .map(str::trim)
        .filter(|e| !e.is_empty());
    let Some(email) = email else {
        return Ok(None);
    };
    // Identical except for the role line: promotion is the claim's one
    // addition to a sign-in.
    let sql = if promote {
        concat!(
            "update users set \
           sub = case \
             when exists (select 1 from users o where o.sub = $1 and o.id <> users.id) then users.sub \
             else $1 \
           end, \
           email = $2, \
           name = case \
             when users.name is null or users.name = '' or users.name = users.email then $3 \
             else users.name \
           end, \
           picture = $4, \
           role = 'admin', \
           last_seen_at = now() \
         where id = ( \
           select id from users \
           where lower(email) = lower($2) \
           order by (role = 'admin') desc, id \
           limit 1 \
         ) \
         returning id::text, sub, email, name, ",
            effective_picture_sql!(),
            ", role"
        )
    } else {
        concat!(
            "update users set \
           sub = case \
             when exists (select 1 from users o where o.sub = $1 and o.id <> users.id) then users.sub \
             else $1 \
           end, \
           email = $2, \
           name = case \
             when users.name is null or users.name = '' or users.name = users.email then $3 \
             else users.name \
           end, \
           picture = $4, \
           last_seen_at = now() \
         where id = ( \
           select id from users \
           where lower(email) = lower($2) \
           order by (role = 'admin') desc, id \
           limit 1 \
         ) \
         returning id::text, sub, email, name, ",
            effective_picture_sql!(),
            ", role"
        )
    };
    let row = sqlx::query_as::<
        _,
        (
            String,
            String,
            Option<String>,
            Option<String>,
            Option<String>,
            String,
        ),
    >(sql)
    .bind(&identity.sub)
    .bind(email)
    .bind(&identity.name)
    .bind(&identity.picture)
    .fetch_optional(exe)
    .await?;
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

/// An unknown row is a member, never an error.
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

/// Everyone who has signed in, for people pickers — fields in select order,
/// ordered by lower(coalesce(email, name, '')).
pub async fn list_users(
    pg: &PgPool,
) -> Result<Vec<(String, Option<String>, Option<String>)>, sqlx::Error> {
    sqlx::query_as::<_, (String, Option<String>, Option<String>)>(
        "select id::text, email, name from users order by lower(coalesce(email, name, '')) asc",
    )
    .fetch_all(pg)
    .await
}

// ── Profile: photo, status, presence ─────────────────────────────────────────
//
// A person's photo is `users.avatar_upload_id`, never a write to `picture`:
// Google sign-in overwrites `picture` on every login (upsert_user above), so a
// photo stored there would vanish on the next sign-in. Everything that hands a
// picture to a client asks for the EFFECTIVE one — the avatar route when an
// upload is set, else the provider's picture — through `effective_picture`
// (Rust) or `effective_picture_sql!` (a RETURNING / select column). The two
// spell the same URL; the version query is the upload id's first eight chars,
// so a new photo is a new URL and the immutable cache never serves the old one.

/// The image types a profile photo may be — the raster set `serve_upload`
/// renders inline, minus AVIF (not every browser the team uses decodes it).
pub const AVATAR_MIMES: [&str; 4] = ["image/png", "image/jpeg", "image/webp", "image/gif"];

/// The largest profile photo, in bytes.
pub const AVATAR_MAX_BYTES: i64 = 5 * 1024 * 1024;

/// How long a presence ping keeps a person online. The client pings every
/// 30s while the tab is visible, so three missed pings mark them away.
pub const PRESENCE_TTL_S: u64 = 90;

/// The Redis key one person's presence ping sets.
pub fn presence_key(user_id: &str) -> String {
    format!("user:presence:{user_id}")
}

/// The picture a client should show for this person: the avatar route when
/// they uploaded a photo, else whatever their sign-in provider gave us.
pub fn effective_picture(
    user_id: &str,
    avatar_upload_id: Option<&str>,
    picture: Option<String>,
) -> Option<String> {
    match avatar_upload_id {
        Some(upload) => Some(format!(
            "/api/users/{user_id}/avatar?v={}",
            upload.get(..8).unwrap_or(upload)
        )),
        None => picture,
    }
}

/// Why an upload cannot become this person's photo. Each refusal carries the
/// status the route answers with and the sentence the person reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AvatarRefusal {
    /// Someone else's upload (or one whose uploader is gone) — 403.
    NotYours,
    /// Not a PNG, JPEG, WebP or GIF — 400.
    NotAnImage,
    /// Over AVATAR_MAX_BYTES — 400.
    TooBig,
}

impl AvatarRefusal {
    pub fn status(self) -> u16 {
        match self {
            AvatarRefusal::NotYours => 403,
            AvatarRefusal::NotAnImage | AvatarRefusal::TooBig => 400,
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            AvatarRefusal::NotYours => "that upload is not yours",
            AvatarRefusal::NotAnImage => "a profile photo must be a PNG, JPEG, WebP, or GIF image",
            AvatarRefusal::TooBig => "a profile photo must be 5 MB or smaller",
        }
    }
}

/// May `caller` claim this upload as their photo? Ownership first — a
/// stranger's upload is refused before its type is even described.
pub fn avatar_refusal(
    caller: &str,
    uploaded_by: Option<&str>,
    mime: &str,
    size: i64,
) -> Option<AvatarRefusal> {
    if uploaded_by != Some(caller) {
        return Some(AvatarRefusal::NotYours);
    }
    if !AVATAR_MIMES.contains(&mime) {
        return Some(AvatarRefusal::NotAnImage);
    }
    if size > AVATAR_MAX_BYTES {
        return Some(AvatarRefusal::TooBig);
    }
    None
}

/// Presence flags for `n` people from one MGET. A failed read (`None`) — or a
/// reply that does not line up with the ids asked about — is everyone offline:
/// a presence outage must never fail the directory it decorates.
pub fn online_flags(n: usize, read: Option<Vec<Option<String>>>) -> Vec<bool> {
    match read {
        Some(values) if values.len() == n => values.iter().map(Option::is_some).collect(),
        _ => vec![false; n],
    }
}

/// An upload's owner, mime and size — the avatar claim's inputs. None when
/// the id names no upload.
pub async fn avatar_upload_facts(
    pg: &PgPool,
    upload_id: &str,
) -> Result<Option<(Option<String>, String, i64)>, sqlx::Error> {
    sqlx::query_as("select uploaded_by::text, mime, size::bigint from uploads where id = $1::uuid")
        .bind(upload_id)
        .fetch_optional(pg)
        .await
}

/// Set (or, with None, clear) a person's photo. The caller has already run
/// `avatar_refusal`.
pub async fn set_avatar_upload(
    pg: &PgPool,
    user_id: &str,
    upload_id: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("update users set avatar_upload_id = $2::uuid where id = $1::uuid")
        .bind(user_id)
        .bind(upload_id)
        .execute(pg)
        .await?;
    Ok(())
}

/// The upload behind a person's photo — the avatar route's ONLY way to an
/// upload id, so the route never serves an id a URL named.
pub async fn user_avatar_upload(pg: &PgPool, user_id: &str) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("select avatar_upload_id::text from users where id = $1::uuid")
            .bind(user_id)
            .fetch_optional(pg)
            .await?;
    Ok(row.and_then(|(v,)| v))
}

pub async fn set_status_emoji(
    pg: &PgPool,
    user_id: &str,
    emoji: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("update users set status_emoji = $2 where id = $1::uuid")
        .bind(user_id)
        .bind(emoji)
        .execute(pg)
        .await?;
    Ok(())
}

pub async fn set_status_text(
    pg: &PgPool,
    user_id: &str,
    text: Option<&str>,
) -> Result<(), sqlx::Error> {
    sqlx::query("update users set status_text = $2 where id = $1::uuid")
        .bind(user_id)
        .bind(text)
        .execute(pg)
        .await?;
    Ok(())
}

/// The identity a client paints for one person: effective picture, status
/// emoji, status text. None when the row is gone.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProfileFace {
    pub picture: Option<String>,
    pub status_emoji: Option<String>,
    pub status_text: Option<String>,
}

pub async fn profile_face(pg: &PgPool, user_id: &str) -> Result<Option<ProfileFace>, sqlx::Error> {
    let row: Option<(
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    )> = sqlx::query_as(
        "select id::text, avatar_upload_id::text, picture, status_emoji, status_text \
             from users where id = $1::uuid",
    )
    .bind(user_id)
    .fetch_optional(pg)
    .await?;
    Ok(row.map(
        |(id, avatar, picture, status_emoji, status_text)| ProfileFace {
            picture: effective_picture(&id, avatar.as_deref(), picture),
            status_emoji,
            status_text,
        },
    ))
}

/// One directory row: who, and the face to paint for them.
#[derive(Debug, Clone)]
pub struct DirectoryRow {
    pub id: String,
    pub email: Option<String>,
    pub name: Option<String>,
    pub face: ProfileFace,
}

/// Everyone who has signed in, with photo and status — the people directory
/// (`GET /api/users`). Same order as `list_users`.
pub async fn list_directory(pg: &PgPool) -> Result<Vec<DirectoryRow>, sqlx::Error> {
    #[allow(clippy::type_complexity)] // the select's own columns, one each
    let rows: Vec<(
        String,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    )> = sqlx::query_as(
        "select id::text, email, name, avatar_upload_id::text, picture, status_emoji, status_text \
         from users order by lower(coalesce(email, name, '')) asc",
    )
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .map(
            |(id, email, name, avatar, picture, status_emoji, status_text)| DirectoryRow {
                face: ProfileFace {
                    picture: effective_picture(&id, avatar.as_deref(), picture),
                    status_emoji,
                    status_text,
                },
                id,
                email,
                name,
            },
        )
        .collect())
}

// ── View denials ─────────────────────────────────────────────────────────────

/// Manage-section routes: default DENIED for members, granted explicitly via
/// allowed_manage_views. Enabled apps extend this set with EVERY app view
/// (work and manage) — apps are explicit-grant only.
pub const MANAGE_VIEW_ROUTES: [&str; 8] = [
    "/agents",
    "/teams",
    "/models",
    "/mcp",
    "/templates",
    "/studio",
    "/observability",
    "/apps",
];

/// Apps' slug rule: lowercase letters, digits, dashes; a letter-or-digit
/// head; ≤64 chars total.
pub fn slug_ok(s: &str) -> bool {
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

/// Where app codebases live: TALARIA_APPS_DIR, else <cwd>/../apps — the
/// repo's apps/ dir when the process runs from api/.
pub fn apps_dir() -> PathBuf {
    match std::env::var("TALARIA_APPS_DIR") {
        Ok(d) => PathBuf::from(d),
        Err(_) => std::env::current_dir()
            .ok()
            .and_then(|c| c.parent().map(|p| p.to_path_buf()))
            .unwrap_or_default()
            .join("apps"),
    }
}

pub fn app_builds_dir() -> PathBuf {
    match std::env::var("TALARIA_APP_BUILDS_DIR") {
        Ok(d) => PathBuf::from(d),
        Err(_) => apps_dir()
            .parent()
            .map(|p| p.join("app-builds"))
            .unwrap_or_else(|| PathBuf::from("app-builds")),
    }
}

pub fn app_data_dir() -> PathBuf {
    match std::env::var("TALARIA_APP_DATA_DIR") {
        Ok(d) => PathBuf::from(d),
        Err(_) => apps_dir()
            .parent()
            .map(|p| p.join("app-data"))
            .unwrap_or_else(|| PathBuf::from("app-data")),
    }
}

#[derive(serde::Serialize, Clone)]
pub struct WireBuild {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

pub fn app_build_status(slug: &str) -> WireBuild {
    let p = app_builds_dir().join(slug).join("current.json");
    let Ok(raw) = std::fs::read_to_string(p) else {
        return WireBuild {
            status: "none".into(),
            key: None,
            error: None,
        };
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&raw) else {
        return WireBuild {
            status: "none".into(),
            key: None,
            error: None,
        };
    };
    WireBuild {
        status: v
            .get("status")
            .and_then(|s| s.as_str())
            .unwrap_or("none")
            .to_string(),
        key: v.get("key").and_then(|s| s.as_str()).map(String::from),
        error: v.get("error").and_then(|s| s.as_str()).map(String::from),
    }
}

/// Why enabling this app must wait or be refused. `ready` is the only
/// status that means compile + load already succeeded. In a checkout (`none`)
/// we still allow enable so `vite dev` can HMR without a prod artifact.
pub fn enable_block_reason(build: &WireBuild) -> Option<String> {
    match build.status.as_str() {
        "ready" => None,
        "failed" => Some(
            build
                .error
                .clone()
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "app failed to compile or load".into()),
        ),
        "building" => Some("app is still compiling".into()),
        _ if compile_strict() => Some("app has not compiled yet".into()),
        _ => None,
    }
}

fn compile_strict() -> bool {
    matches!(std::env::var("TALARIA_INSTALL").as_deref(), Ok("image"))
        || matches!(
            std::env::var("TALARIA_RUNTIME").as_deref(),
            Ok("prod-server")
        )
}

pub struct DiscoveredApp {
    pub slug: String,
    pub name: String,
    pub icon: String,
    pub description: String,
    pub version: String,
    pub work: Option<String>,
    pub manage: Option<String>,
    pub settings: Option<String>,
    /// The app publishes MCP tools for agents (apps/<slug>/mcp.ts).
    pub mcp: bool,
}

/// The manifests on disk, sorted by app name. Any unreadable/unparseable
/// directory is skipped. A missing scalar field yields its default; a
/// non-string value is treated as missing.
pub fn discovered_apps() -> Vec<DiscoveredApp> {
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
        // when they hold non-empty strings.
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
        // the key is absent, not null.
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

/// One enabled app as /api/apps serves it — wire order; absent surface keys
/// absent (never null).
#[derive(serde::Serialize)]
pub struct WireApp {
    pub slug: String,
    pub name: String,
    pub icon: String,
    pub description: String,
    pub version: String,
    pub surfaces: WireSurfaces,
    pub mcp: bool,
    pub build: WireBuild,
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

/// The signed-in view of installed apps: ENABLED apps only, in discovered
/// (name) order — the platform's own menu, not a secret; per-user view
/// gating happens off deniedViews client-side.
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
        .map(|a| {
            let build = app_build_status(&a.slug);
            WireApp {
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
                build,
            }
        })
        .collect()
}

/// ALL app view routes of ENABLED apps: every work surface, then every
/// manage surface — apps are explicit-grant.
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
/// Views a member may NOT reach: their explicit work-view denials (DB order)
/// PLUS every Manage view they haven't been granted. Team grants union in:
/// a team's work denial adds to the user's, a team's manage grant opens the
/// door. Admins are never restricted.
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
    let (mut denied, allowed) = row.unwrap_or_default();
    let (team_denied, team_allowed) = talaria_teams::team_view_grants_for_user(pg, user_id).await?;
    for v in team_denied {
        if !denied.contains(&v) {
            denied.push(v);
        }
    }
    let mut allowed: std::collections::HashSet<String> =
        allowed.unwrap_or_default().into_iter().collect();
    for v in team_allowed {
        allowed.insert(v);
    }
    // Managing an agent opens /agents by itself. Naming someone a manager is
    // the grant (docs/PERMISSIONS.md, "Agent managers"), and a grant that
    // leaves them unable to reach the surface where the agent lives would
    // not be one — nobody should need a second, org-wide view grant to tend
    // the agent they own. The roster behind the view shows only the agents
    // they manage; `agents.manage` is what widens it to the fleet.
    if talaria_agent_managers::manages_any_agent(pg, user_id, role).await? {
        allowed.insert("/agents".to_string());
    }
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

// ── Admin console writes ────────────────────────────────────────────────────

/// One row of the admin console's user list, in wire order — the insert order
/// below. Hand-built JSON so that order holds; timestamps take the epoch-ms →
/// ISO route.
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
    use serde_json::Value;
    use talaria_agent_auth::epoch_ms_to_iso;
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
            r.try_get::<Option<String>, _>(1)?
                .map(Value::String)
                .unwrap_or(Value::Null),
        );
        f.insert(
            "name".into(),
            r.try_get::<Option<String>, _>(2)?
                .map(Value::String)
                .unwrap_or(Value::Null),
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
            r.try_get::<Option<String>, _>(11)?
                .map(Value::String)
                .unwrap_or(Value::Null),
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
    Ok(
        sqlx::query_scalar::<_, i32>("select count(*)::int from users where role = 'admin'")
            .fetch_one(pg)
            .await
            .unwrap_or(0),
    )
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

// ── Permissions ─────────────────────────────────────────────────────────────
// The catalog and the resolution chain live in permissions.rs — one source
// for the admin GET's full entries and the session's resolved ids.

pub use talaria_permissions::{has_perm, user_permissions};

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

#[cfg(test)]
mod tests {
    use super::*;

    const UID: &str = "6f9619ff-8b86-d011-b42d-00c04fc964ff";
    const UPLOAD: &str = "a1b2c3d4-0000-4000-8000-000000000001";

    #[test]
    fn effective_picture_prefers_the_uploaded_photo() {
        // An upload wins over the provider picture — the AE4 rule: a later
        // Google sign-in rewrites `picture`, never the effective URL.
        assert_eq!(
            effective_picture(UID, Some(UPLOAD), Some("https://google.test/p.png".into()))
                .as_deref(),
            Some("/api/users/6f9619ff-8b86-d011-b42d-00c04fc964ff/avatar?v=a1b2c3d4")
        );
        // No upload: the provider picture, or nothing.
        assert_eq!(
            effective_picture(UID, None, Some("https://google.test/p.png".into())).as_deref(),
            Some("https://google.test/p.png")
        );
        assert_eq!(effective_picture(UID, None, None), None);
    }

    #[test]
    fn effective_picture_sql_spells_the_same_url() {
        // The SQL twin must build the same path and version slice the Rust
        // helper does; the shape is pinned here because no unit test can run it.
        let sql = effective_picture_sql!();
        assert!(sql.contains(
            "'/api/users/' || id::text || '/avatar?v=' || left(avatar_upload_id::text, 8)"
        ));
        assert!(sql.contains("else picture end"));
    }

    #[test]
    fn avatar_claims_check_owner_then_type_then_size() {
        let me = UID;
        let ok = avatar_refusal(me, Some(me), "image/png", 1024);
        assert_eq!(ok, None);
        for mime in AVATAR_MIMES {
            assert_eq!(
                avatar_refusal(me, Some(me), mime, AVATAR_MAX_BYTES),
                None,
                "{mime}"
            );
        }
        // Another person's upload, or one whose uploader is gone: 403, and
        // ownership outranks every other complaint.
        assert_eq!(
            avatar_refusal(me, Some("someone-else"), "application/pdf", 1),
            Some(AvatarRefusal::NotYours)
        );
        assert_eq!(
            avatar_refusal(me, None, "image/png", 1),
            Some(AvatarRefusal::NotYours)
        );
        // A PDF, an SVG (script in an "image"), or a 6 MB photo: 400.
        assert_eq!(
            avatar_refusal(me, Some(me), "application/pdf", 1),
            Some(AvatarRefusal::NotAnImage)
        );
        assert_eq!(
            avatar_refusal(me, Some(me), "image/svg+xml", 1),
            Some(AvatarRefusal::NotAnImage)
        );
        assert_eq!(
            avatar_refusal(me, Some(me), "image/png", 6 * 1024 * 1024),
            Some(AvatarRefusal::TooBig)
        );
        assert_eq!(
            avatar_refusal(me, Some(me), "image/png", AVATAR_MAX_BYTES + 1),
            Some(AvatarRefusal::TooBig)
        );
        assert_eq!(AvatarRefusal::NotYours.status(), 403);
        assert_eq!(AvatarRefusal::NotAnImage.status(), 400);
        assert_eq!(AvatarRefusal::TooBig.status(), 400);
    }

    #[test]
    fn presence_reads_degrade_to_everyone_offline() {
        // A good read: set keys are online, missing keys are not.
        assert_eq!(
            online_flags(3, Some(vec![Some("1".into()), None, Some("1".into())])),
            vec![true, false, true]
        );
        // Redis down or the read failed: every user offline, never an error.
        assert_eq!(online_flags(2, None), vec![false, false]);
        // A reply that does not line up with the ids asked about is not
        // trusted for anyone.
        assert_eq!(
            online_flags(2, Some(vec![Some("1".into())])),
            vec![false, false]
        );
        assert_eq!(online_flags(0, None), Vec::<bool>::new());
        assert_eq!(presence_key("u1"), "user:presence:u1");
    }

    #[test]
    fn slug_rules_match_the_regex() {
        assert!(slug_ok("a"));
        assert!(slug_ok("0-9"));
        assert!(slug_ok("helpdesk-2"));
        assert!(slug_ok(&"a".repeat(64)));
        assert!(!slug_ok(&"a".repeat(65)));
        assert!(!slug_ok("")); // a slug needs a head char
        assert!(!slug_ok("-x"));
        assert!(!slug_ok("Upper"));
        assert!(!slug_ok("under_score"));
        assert!(!slug_ok("sp ace"));
    }

    #[test]
    fn enable_block_reason_names_a_failed_compile() {
        let failed = WireBuild {
            status: "failed".into(),
            key: None,
            error: Some("server.ts must default-export defineAppServer({ fetch })".into()),
        };
        assert_eq!(
            enable_block_reason(&failed).as_deref(),
            Some("server.ts must default-export defineAppServer({ fetch })")
        );
        let ready = WireBuild {
            status: "ready".into(),
            key: Some("abc".into()),
            error: None,
        };
        assert_eq!(enable_block_reason(&ready), None);
        let building = WireBuild {
            status: "building".into(),
            key: None,
            error: None,
        };
        assert_eq!(
            enable_block_reason(&building).as_deref(),
            Some("app is still compiling")
        );
    }

    #[test]
    fn permission_catalog_is_the_wire_order() {
        let ids: Vec<&str> = talaria_permissions::PERMISSIONS
            .iter()
            .map(|p| p.id)
            .collect();
        assert_eq!(
            ids,
            vec![
                "agents.manage",
                "research.run",
                "plans.create",
                "work.sessions",
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
        // And the member defaults, in catalog order.
        let defaults: Vec<(&str, bool)> = talaria_permissions::PERMISSIONS
            .iter()
            .filter(|p| p.member_default)
            .map(|p| (p.id, true))
            .collect();
        assert_eq!(
            defaults,
            vec![
                ("research.run", true),
                ("plans.create", true),
                ("work.sessions", true),
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
