// Talaria apps — the ADMIN half of the registry (the read
// plane lives in users.rs): enablement, the lifecycle around an app's code
// (install, update, uninstall), the marketplace catalog, and the MCP rows.
// Apps are self-contained TypeScript codebases under apps/<slug>/ that this
// instance compiles and runs without a host rebuild; their surfaces and MCP
// dispatch stay TS (rule 10) — this crate owns the registry state around
// them, and asks the app host to build (`host`).
//
//   enablement   admin-controlled set in app_settings; disabled apps have no
//                nav presence and their server routes 404
//   lifecycle    install from git with a read-only credential, update with an
//                atomic swap and rollback, uninstall that keeps data unless
//                asked (`lifecycle`, `git`, `source`)
//   catalog      the marketplace feed — JSON, source URL configurable,
//                always through the SSRF guard
//   gateway key  the `app:<slug>` key the app host makes every model call
//                for the app with — minted on the host's ask, sealed,
//                revoked when the app stops running (`llm_key`)

pub mod git;
pub mod host;
pub mod lifecycle;
pub mod llm_key;
pub mod source;

pub use host::AppHost;
pub use lifecycle::{
    DataLeft, InstallError, InstallRequest, Installed, KeptDataChoice, KeptDataConflict,
    PlatformCredentials, SourceRecord, UpdateCheck, Updated, install_app, source_records,
    uninstall_app, update_app, update_checks,
};
pub use llm_key::{AppKeyError, ensure_app_llm_key, reconcile_app_llm_keys, revoke_app_llm_key};

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

use serde_json::Value;
use sqlx::PgPool;

use talaria_gateway::settings::{get_setting, set_setting};
use talaria_secretbox::SecretBox;
use talaria_users::{
    app_build_status, app_data_dir, discovered_apps, enable_block_reason, slug_ok,
};

const ENABLED_KEY: &str = "apps_enabled";
const CATALOG_URL_KEY: &str = "apps_catalog_url";
const DEFAULT_CATALOG: &str =
    "https://raw.githubusercontent.com/outcrop-labs/talaria-apps/main/index.json";

/// The enabled set, sorted —
/// the setting is always stored sorted.
pub async fn enabled_app_slugs(pg: &PgPool) -> Vec<String> {
    get_setting(pg, ENABLED_KEY, Value::Array(vec![]))
        .await
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|v| v.as_str())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// The tool catalog an app's last load probe wrote (ui/src/server/app-mcp.ts
/// `syncAppMcpCatalog`). It lives in the app's DATA directory and never beside
/// its build: the host serves app-builds/ to anyone (`tryServeAppBuilds`), and
/// a catalog holds every tool's name, description and schema and the probe's
/// findings.
pub const MCP_CATALOG_FILE: &str = "mcp-catalog.json";

/// Where an app's catalog file lives: `app-data/<slug>/mcp-catalog.json`.
pub fn mcp_catalog_path(slug: &str) -> PathBuf {
    app_data_dir().join(slug).join(MCP_CATALOG_FILE)
}

/// The two registry columns an app's catalog file fills: `tools` (the tools
/// that passed the probe's checks, already bounded) and `last_probe`
/// (`{state, detail, at}`, naming any tool the probe left out). None when the
/// file is not the probe's shape — the row then keeps what it has.
pub fn catalog_columns(raw: &str) -> Option<(Value, Value)> {
    let v: Value = serde_json::from_str(raw).ok()?;
    let tools = v.get("tools").filter(|t| t.is_array())?.clone();
    let probe = v.get("probe").filter(|p| {
        p.get("state").is_some_and(Value::is_string)
            && p.get("detail").is_some_and(Value::is_string)
    })?;
    Some((tools, probe.clone()))
}

fn read_mcp_catalog(slug: &str) -> Option<(Value, Value)> {
    let raw = std::fs::read_to_string(mcp_catalog_path(slug)).ok()?;
    catalog_columns(&raw)
}

/// The advisory lock every write to app MCP rows takes, for its transaction
/// (migrations hold 8_314_207, the admin claim 8_314_208). Syncs run from
/// several triggers at once (an enable, a failed build, a new catalog, the
/// boot pass), and without it one sync could read the enabled set, another
/// turn the app off, and the first then write the row back as serving.
pub const MCP_SYNC_LOCK: i64 = 8_314_209;

/// The most of a reason an app-off row keeps (Manage → MCP shows it).
const APP_OFF_MAX: usize = 300;

/// One installed app as a sync sees it: the facts on disk, and whether the
/// enabled set names it.
#[derive(Debug, Clone)]
pub struct AppFacts {
    pub slug: String,
    pub name: String,
    pub description: String,
    /// `apps/<slug>/mcp.ts` exists.
    pub mcp: bool,
    pub enabled: bool,
    /// The last compile or load failed, and why (`current.json`).
    pub failed: Option<String>,
    /// The probe's checked catalog (`mcp_catalog_path`): the `tools` and
    /// `last_probe` columns it fills. None before the probe has written one.
    pub catalog: Option<(Value, Value)>,
}

/// What a sync makes of one app's MCP registry row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppMcpRow {
    /// Enabled, built and publishing tools: the row serves, and is created if
    /// it does not exist yet.
    Serve { name: String, description: String },
    /// The app cannot serve right now. The row stays, and so does every agent
    /// assignment, team grant and user grant on it, but no agent carries it
    /// and its gateway refuses it until the app serves again. The sentence
    /// says why, on the server's card.
    Off(String),
}

fn clip(s: String) -> String {
    if s.chars().count() <= APP_OFF_MAX {
        return s;
    }
    let mut out: String = s.chars().take(APP_OFF_MAX - 1).collect();
    out.push('…');
    out
}

/// The plan, pure: what each app's row should be after a sync. A row is
/// created only for an app that serves. A row that exists for any other app
/// is turned OFF and never deleted, because deleting it cascades every grant
/// on it: a build that broke for an hour would cost an admin every agent, team
/// and person they had granted the server to. Uninstall is the one deleter
/// (`remove_app_mcp_server`). The admin's own `enabled` flag is not the
/// sync's to touch, so the plan says nothing about it.
pub fn app_mcp_plan(apps: &[AppFacts], rows: &[String]) -> BTreeMap<String, AppMcpRow> {
    let mut plan = BTreeMap::new();
    for a in apps {
        if a.enabled && a.mcp && a.failed.is_none() {
            plan.insert(
                a.slug.clone(),
                AppMcpRow::Serve {
                    name: a.name.clone(),
                    description: a.description.clone(),
                },
            );
        }
    }
    for slug in rows {
        if plan.contains_key(slug) {
            continue;
        }
        let why = match apps.iter().find(|a| &a.slug == slug) {
            None => "the app is not installed".to_string(),
            Some(a) => match &a.failed {
                Some(err) => clip(format!("the app failed to build or load: {err}")),
                None if !a.enabled => "the app is turned off".to_string(),
                None => "the app no longer publishes MCP tools".to_string(),
            },
        };
        plan.insert(slug.clone(), AppMcpRow::Off(why));
    }
    plan
}

/// What one sync changed: the app server names that serve, the rows that
/// went from serving to off or back, and the serving rows whose catalog
/// changed. The carriers of both lists are applied to, each row once.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SyncReport {
    pub serving: Vec<String>,
    /// Registry ids of rows that turned off, or back on.
    pub flipped: Vec<String>,
    /// Registry ids of rows that kept serving with a different catalog.
    pub retooled: Vec<String>,
}

/// Did a serving row's catalog change? Only a catalog the probe wrote can
/// change one: with no catalog file the row keeps its own `tools`, and a file
/// that says what the row already holds changes nothing. Compared as JSON
/// values, so key order (jsonb keeps its own) is not a change.
///
/// A running Hermes keeps the tools/list it read from a server until the
/// server says the list changed (the header of `talaria_mcp_apply`), so an
/// app update that adds, renames or removes a tool, or edits a description
/// or a schema, reaches an agent only through that notice — and a catalog
/// that did not change must send it to nobody: every host boot and every
/// probe re-sends the same file.
pub fn catalog_changed(stored: &Value, fresh: Option<&Value>) -> bool {
    fresh.is_some_and(|f| f != stored)
}

/// The installed apps, with the enabled set's verdict and each build's status.
pub fn app_facts(enabled: &BTreeSet<String>) -> Vec<AppFacts> {
    discovered_apps()
        .into_iter()
        .map(|a| {
            let build = app_build_status(&a.slug);
            let failed = (build.status == "failed").then(|| {
                build
                    .error
                    .filter(|e| !e.is_empty())
                    .unwrap_or_else(|| "app failed to compile or load".into())
            });
            AppFacts {
                enabled: enabled.contains(&a.slug),
                failed,
                catalog: read_mcp_catalog(&a.slug),
                slug: a.slug,
                name: a.name,
                description: a.description,
                mcp: a.mcp,
            }
        })
        .collect()
}

/// The enabled set a sync reads from the stored `apps_enabled` value, by
/// `talaria_users::enabled_slugs_from` — the rule every app gate reads it
/// with. A value that is not a list of slugs fails the sync like a failed
/// query: read leniently, a malformed value was "no app is enabled" and took
/// every app's server off every agent.
fn sync_enabled_set(stored: Option<Value>) -> Result<BTreeSet<String>, sqlx::Error> {
    talaria_users::enabled_slugs_from(stored)
        .map(|set| set.into_iter().collect())
        .map_err(|e| sqlx::Error::Decode(Box::new(e)))
}

async fn sync_app_mcp_rows(
    pg: &PgPool,
    facts: impl FnOnce(&BTreeSet<String>) -> Vec<AppFacts>,
) -> Result<SyncReport, sqlx::Error> {
    let mut tx = pg.begin().await?;
    sqlx::query("select pg_advisory_xact_lock($1)")
        .bind(MCP_SYNC_LOCK)
        .execute(&mut *tx)
        .await?;
    // Read under the lock, and a failed read fails the sync. It must never
    // read as "no app is enabled", which would take every app's server off
    // every agent. A stored value that is not a list of slugs is a failed
    // read too, by the rule every app gate reads it with
    // (`talaria_users::enabled_slugs_from`).
    let stored = sqlx::query_scalar::<_, Value>("select value from app_settings where key = $1")
        .bind(ENABLED_KEY)
        .fetch_optional(&mut *tx)
        .await?;
    let enabled = sync_enabled_set(stored)?;
    let apps = facts(&enabled);
    // (id, slug, app_off, tools, the admin's enabled switch)
    let rows: Vec<(String, String, Option<String>, Value, bool)> = sqlx::query_as(
        "select id::text, app_slug::text, app_off, tools, enabled from mcp_servers \
         where app_slug is not null",
    )
    .fetch_all(&mut *tx)
    .await?;
    let slugs: Vec<String> = rows.iter().map(|(_, slug, ..)| slug.clone()).collect();
    let mut synced = SyncReport::default();
    for (slug, row) in app_mcp_plan(&apps, &slugs) {
        let had = rows.iter().find(|(_, s, ..)| *s == slug);
        match row {
            AppMcpRow::Serve { name, description } => {
                let server = format!("app-{slug}");
                // $6/$7 null = no catalog file yet: an insert seeds `[]` and
                // no probe, an update keeps the row's own. `enabled` is set
                // only by the insert: on an existing row it is the admin's
                // choice, and an admin who turned the server off keeps it off
                // across every boot and resync.
                let (tools, probe) = apps
                    .iter()
                    .find(|a| a.slug == slug)
                    .and_then(|a| a.catalog.clone())
                    .unzip();
                // A row that was serving, is on, and gets a different catalog
                // tells its carriers to list the new tools. (A row coming
                // back from off applies as flipped; an admin's disabled row
                // is carried by nobody.)
                let retooled = had.is_some_and(|(_, _, off, stored, enabled)| {
                    off.is_none() && *enabled && catalog_changed(stored, tools.as_ref())
                });
                let id: String = sqlx::query_scalar(
                    "insert into mcp_servers \
                        (name, label, description, url, all_agents, app_slug, tools, tools_refreshed_at, last_probe, created_by) \
                     values ($1, $2, $3, $4, false, $5, coalesce($6, '[]'::jsonb), now(), $7, 'talaria') \
                     on conflict (name) do update set \
                        label = excluded.label, description = excluded.description, \
                        app_slug = excluded.app_slug, tools_refreshed_at = now(), \
                        tools = coalesce($6, mcp_servers.tools), \
                        last_probe = coalesce($7, mcp_servers.last_probe), \
                        app_off = null, updated_at = now() \
                     returning id::text",
                )
                .bind(&server)
                .bind(&name)
                .bind(if description.is_empty() {
                    None
                } else {
                    Some(description)
                })
                .bind(format!("talaria-app://{slug}"))
                .bind(&slug)
                .bind(tools)
                .bind(probe)
                .fetch_one(&mut *tx)
                .await?;
                if had.is_some_and(|(_, _, off, ..)| off.is_some()) {
                    synced.flipped.push(id);
                } else if retooled {
                    tracing::info!(
                        "[apps] {server}: the tool catalog changed; the agents that carry it will list it again"
                    );
                    synced.retooled.push(id);
                }
                synced.serving.push(server);
            }
            AppMcpRow::Off(why) => {
                let Some((id, _, off, ..)) = had else {
                    continue;
                };
                if off.as_deref() == Some(why.as_str()) {
                    continue;
                }
                sqlx::query(
                    "update mcp_servers set app_off = $2, updated_at = now() where id::text = $1",
                )
                .bind(id)
                .bind(&why)
                .execute(&mut *tx)
                .await?;
                if off.is_none() {
                    synced.flipped.push(id.clone());
                }
            }
        }
    }
    tx.commit().await?;
    Ok(synced)
}

/// Reconcile the registry rows of app-published MCP servers with the apps
/// (`app_mcp_plan`): an enabled app that publishes tools gets a serving row,
/// created if missing, and every other app's row is turned off with the
/// reason. A row that changes between serving and off applies to the agents
/// that carry it by grant, so its tools leave them, or return, with every
/// assignment, team grant and user grant intact. Answers the app server names
/// that serve.
///
/// THE CATALOG COMES FROM THE BUILD, never from here: the app module is TS
/// (rule 10), and its load probe writes the checked catalog to the app's data
/// directory (`mcp_catalog_path`). Every upsert copies that file into `tools`
/// and `last_probe`, so the row an enable creates already carries its tools.
/// An app the probe has not reached yet keeps whatever its row has.
///
/// One sync at a time (`MCP_SYNC_LOCK`). Runs on enable and disable, at api
/// boot, and whenever the app host asks (`POST /api/internal/apps/mcp-sync`:
/// a probe wrote a new catalog, or a failed build turned the app off).
pub async fn sync_app_mcp_servers(pg: &PgPool, sb: &SecretBox) -> Result<Vec<String>, sqlx::Error> {
    sync_app_mcp_servers_over(pg, sb, app_facts).await
}

/// The same sync over app facts the caller derives from the enabled set —
/// the disk's (`app_facts`) in production. The live test adds an app that is
/// not on disk, so it can drive one row through serving, off and back without
/// touching anyone's apps.
pub async fn sync_app_mcp_servers_over(
    pg: &PgPool,
    sb: &SecretBox,
    facts: impl FnOnce(&BTreeSet<String>) -> Vec<AppFacts>,
) -> Result<Vec<String>, sqlx::Error> {
    Ok(sync_app_mcp_servers_report(pg, sb, facts).await?.serving)
}

/// The same sync, answering everything it changed (`SyncReport`): the rows
/// whose carriers it applied to, not only the servers that serve.
pub async fn sync_app_mcp_servers_report(
    pg: &PgPool,
    sb: &SecretBox,
    facts: impl FnOnce(&BTreeSet<String>) -> Vec<AppFacts>,
) -> Result<SyncReport, sqlx::Error> {
    let synced = sync_app_mcp_rows(pg, facts).await?;
    // After the commit, so what reaches each carrier is the state it was
    // applied for. A FLIPPED row is a server name arriving in or leaving its
    // carriers' configs — the render writes it and a running Hermes
    // reconciles names within a minute. A RETOOLED row is a new catalog under
    // the same name — the gateway tells each carrier to list again on its
    // next keepalive (`bump_tool_lists`). Neither moves a container, so the
    // carriers APPLY (talaria-mcp-apply), which restarts nobody unless the
    // render moved their container for some other reason.
    if !synced.retooled.is_empty() || !synced.flipped.is_empty() {
        talaria_mcp_apply::bump_tool_lists(pg).await;
    }
    for id in synced.flipped.iter().chain(&synced.retooled) {
        talaria_mcp_apply::apply_agents_for_server(pg, sb, id).await;
    }
    Ok(synced)
}

/// Delete an app's MCP registry row, and with it, by cascade, every agent
/// assignment, team grant and user grant on it. Uninstall only; every other
/// way an app stops serving turns the row off (`sync_app_mcp_servers`). A row
/// that was still serving applies to its carriers (the name leaves their
/// configs), read before the cascade takes the grants that name them.
pub async fn remove_app_mcp_server(
    pg: &PgPool,
    sb: &SecretBox,
    slug: &str,
) -> Result<(), sqlx::Error> {
    let mut tx = pg.begin().await?;
    sqlx::query("select pg_advisory_xact_lock($1)")
        .bind(MCP_SYNC_LOCK)
        .execute(&mut *tx)
        .await?;
    let rows: Vec<(String, Option<String>)> =
        sqlx::query_as("select id::text, app_off from mcp_servers where app_slug = $1")
            .bind(slug)
            .fetch_all(&mut *tx)
            .await?;
    let mut departments = Vec::new();
    for (id, off) in &rows {
        if off.is_none() {
            departments.extend(talaria_mcp_apply::carriers_for_server(pg, id).await?);
        }
    }
    sqlx::query("delete from mcp_servers where app_slug = $1")
        .bind(slug)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    talaria_mcp_apply::enqueue_applies(&departments, pg, sb);
    Ok(())
}

/// Flip one app's enablement. Enabling an app that is not on disk
/// is an error — the set must name real code.
pub async fn set_app_enabled(
    pg: &PgPool,
    sb: &SecretBox,
    slug: &str,
    enabled: bool,
) -> Result<(), String> {
    let mut cur: BTreeSet<String> = enabled_app_slugs(pg).await.into_iter().collect();
    if enabled {
        if !discovered_apps().iter().any(|a| a.slug == slug) {
            return Err(format!("no app \"{slug}\" installed"));
        }
        if let Some(msg) = enable_block_reason(&app_build_status(slug)) {
            return Err(msg);
        }
        cur.insert(slug.to_string());
    } else {
        cur.remove(slug);
    }
    let sorted = Value::Array(cur.into_iter().map(Value::String).collect());
    set_setting(pg, ENABLED_KEY, &sorted)
        .await
        .map_err(|e| e.to_string())?;
    // Apps that publish MCP tools follow their enablement into the registry:
    // a row serves or turns off (keeping its grants), and its carriers apply.
    if let Err(e) = sync_app_mcp_servers(pg, sb).await {
        tracing::warn!("[apps] {slug}: MCP registry sync failed: {e}");
    }
    // An app that is off makes no model calls: the host's copy of its
    // gateway key stops authenticating now, and a re-enable mints a new one.
    if !enabled && let Err(e) = revoke_app_llm_key(pg, slug).await {
        tracing::warn!("[apps] {slug}: revoking the app's gateway key failed: {e}");
    }
    Ok(())
}

/// Drop enabled apps whose last compile/load failed. Called at api boot
/// (and by the UI reconciler via `apps_enabled`); GET /admin/apps does not.
pub async fn disable_broken_apps(pg: &PgPool, sb: &SecretBox) {
    for slug in enabled_app_slugs(pg).await {
        if app_build_status(&slug).status == "failed" {
            let _ = set_app_enabled(pg, sb, &slug, false).await;
        }
    }
}

pub(crate) fn app_db_container(slug: &str) -> String {
    let instance = std::env::var("TALARIA_WORKTREE")
        .or_else(|_| std::env::var("TALARIA_DEVBOX"))
        .unwrap_or_else(|_| "talaria".into());
    format!("talaria-appdb-{instance}-{slug}")
}

pub(crate) async fn stop_app_db(slug: &str) {
    let file = app_data_dir().join(slug).join("docker-compose.yml");
    let project = app_db_container(slug);
    if file.exists() {
        let file_s = file.to_string_lossy().into_owned();
        let _ = tokio::process::Command::new("docker")
            .args([
                "compose",
                "-p",
                &project,
                "-f",
                &file_s,
                "down",
                "--remove-orphans",
            ])
            .output()
            .await;
    }
    let _ = tokio::process::Command::new("docker")
        .args(["rm", "-f", &project])
        .output()
        .await;
}

pub(crate) async fn forget_app_db_password(pg: &PgPool, slug: &str) {
    let mut stored = get_setting(pg, "app_db_passwords", Value::Object(Default::default())).await;
    if let Some(obj) = stored.as_object_mut()
        && obj.remove(slug).is_some()
    {
        let _ = set_setting(pg, "app_db_passwords", &stored).await;
    }
}

/// The slug an install from `url` takes: the override when given, else the
/// repo's name, lowercased, without `.git` or a `talaria-app-` prefix. It
/// must pass `slug_installable` — the slug rule, and none of the words the
/// client router reserves (`RESERVED_APP_SLUGS`), so a repo named `hooks`
/// installs only under an override.
pub fn install_slug(url: &str, slug_override: Option<&str>) -> Result<String, String> {
    let base = url.rsplit('/').next().unwrap_or("");
    let derived = slug_override
        .unwrap_or(base)
        .trim_end_matches(".git")
        .to_lowercase();
    let derived = derived.strip_prefix("talaria-app-").unwrap_or(&derived);
    talaria_users::slug_installable(derived)?;
    Ok(derived.to_string())
}

// ── Marketplace catalog ──────────────────────────────────────────────────────

pub async fn catalog_url(pg: &PgPool) -> String {
    let stored = get_setting(pg, CATALOG_URL_KEY, Value::String(DEFAULT_CATALOG.into())).await;
    match stored.as_str() {
        Some(u) if !u.is_empty() => u.to_string(),
        _ => DEFAULT_CATALOG.to_string(),
    }
}

/// Empty (or blank) resets to the default.
pub async fn set_catalog_url(pg: &PgPool, url: Option<&str>) -> Result<(), String> {
    let v = match url.map(str::trim).filter(|u| !u.is_empty()) {
        Some(u) => Value::String(u.to_string()),
        None => Value::String(DEFAULT_CATALOG.to_string()),
    };
    set_setting(pg, CATALOG_URL_KEY, &v)
        .await
        .map_err(|e| e.to_string())
}

/// The marketplace feed. Unreachable/invalid → empty list
/// with the error, so the Discover tab can say why it's blank instead of
/// pretending it's empty.
pub async fn fetch_catalog(pg: &PgPool) -> (Vec<Value>, Option<String>) {
    let url = catalog_url(pg).await;
    let fetch = talaria_safe_fetch::safe_fetch(
        &url,
        talaria_safe_fetch::SafeFetch {
            headers: vec![("accept", "application/json")],
            timeout_ms: Some(10_000),
            max_bytes: Some(2 * 1024 * 1024),
            ..Default::default()
        },
    )
    .await;
    let resp = match fetch {
        Ok(r) => r,
        Err(e) => return (Vec::new(), Some(e.to_string())),
    };
    if !(200..300).contains(&resp.status) {
        return (
            Vec::new(),
            Some(format!("catalog fetch failed ({})", resp.status)),
        );
    }
    let Ok(j) = serde_json::from_slice::<Value>(&resp.body) else {
        return (Vec::new(), Some("expected a JSON catalog".to_string()));
    };
    let mut out = Vec::new();
    for a in j
        .get("apps")
        .and_then(|v| v.as_array())
        .cloned()
        .unwrap_or_default()
    {
        let field = |k: &str| a.get(k).and_then(|v| v.as_str()).map(String::from);
        // Admission: real slug, a name, an https repo. Everything else
        // coerces to defaults.
        let Some(slug) = field("slug").filter(|s| slug_ok(s)) else {
            continue;
        };
        let (Some(name), Some(repo)) = (field("name"), field("repo")) else {
            continue;
        };
        if !repo.starts_with("https://") {
            continue;
        }
        let mut obj = serde_json::Map::new();
        obj.insert("slug".into(), slug.into());
        obj.insert("name".into(), name.into());
        obj.insert(
            "icon".into(),
            field("icon").unwrap_or_else(|| "⬡".into()).into(),
        );
        obj.insert(
            "description".into(),
            field("description").unwrap_or_default().into(),
        );
        obj.insert("repo".into(), repo.into());
        obj.insert(
            "author".into(),
            field("author").unwrap_or_else(|| "community".into()).into(),
        );
        obj.insert(
            "official".into(),
            Value::Bool(a.get("official").and_then(Value::as_bool) == Some(true)),
        );
        // version is OMITTED unless present and non-empty.
        if let Some(v) = field("version").filter(|v| !v.is_empty()) {
            obj.insert("version".into(), v.into());
        }
        out.push(Value::Object(obj));
    }
    (out, None)
}

#[cfg(test)]
mod tests {
    use super::*;
    use talaria_users::app_builds_dir;

    #[test]
    fn a_sync_reads_the_enabled_set_by_the_gates_rule() {
        use serde_json::json;
        let set = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<BTreeSet<_>>();
        assert_eq!(sync_enabled_set(None).unwrap(), set(&[]));
        assert_eq!(sync_enabled_set(Some(Value::Null)).unwrap(), set(&[]));
        assert_eq!(
            sync_enabled_set(Some(json!(["crm", "notes"]))).unwrap(),
            set(&["crm", "notes"])
        );
        // Never "no app is enabled": each of these fails the sync, and the
        // rows stay as they were.
        for malformed in [json!("crm-suite"), json!({"crm": true}), json!(["crm", 7])] {
            let err = sync_enabled_set(Some(malformed.clone())).expect_err("malformed");
            assert!(
                err.to_string().contains("not a list of app slugs"),
                "{malformed}: {err}"
            );
        }
    }

    /// The shape the TS probe writes (`syncAppMcpCatalog`): the checked
    /// tools and the probe line, copied into the row as they are.
    #[test]
    fn only_a_different_catalog_from_the_probe_is_a_change() {
        use serde_json::json;
        let stored = json!([{ "name": "crm_find", "description": "Find a contact.",
            "inputSchema": { "type": "object", "properties": { "q": { "type": "string" } } } }]);
        // No catalog file: the row keeps its own tools, nothing changed.
        assert!(!catalog_changed(&stored, None));
        // The same catalog, keys in another order (jsonb keeps its own): no change.
        let reordered = json!([{ "inputSchema": { "properties": { "q": { "type": "string" } }, "type": "object" },
            "description": "Find a contact.", "name": "crm_find" }]);
        assert!(!catalog_changed(&stored, Some(&reordered)));
        // A tool added, renamed or removed, a description or a schema edited.
        for fresh in [
            json!([{ "name": "crm_find", "description": "Find a contact.", "inputSchema": { "type": "object", "properties": { "q": { "type": "string" } } } },
                   { "name": "crm_add", "description": "Add one.", "inputSchema": { "type": "object", "properties": {} } }]),
            json!([{ "name": "crm_search", "description": "Find a contact.", "inputSchema": { "type": "object", "properties": { "q": { "type": "string" } } } }]),
            json!([]),
            json!([{ "name": "crm_find", "description": "Find a person.", "inputSchema": { "type": "object", "properties": { "q": { "type": "string" } } } }]),
            json!([{ "name": "crm_find", "description": "Find a contact.", "inputSchema": { "type": "object", "properties": { "q": { "type": "number" } } } }]),
            json!([{ "name": "crm_find", "description": "Find a contact.", "inputSchema": { "type": "object", "properties": { "q": { "type": "string" } } },
                     "outputSchema": { "type": "object" } }]),
        ] {
            assert!(catalog_changed(&stored, Some(&fresh)), "{fresh}");
        }
        // A row created before its first probe holds `[]`; the first catalog
        // is a change.
        assert!(catalog_changed(&json!([]), Some(&stored)));
    }

    #[test]
    fn a_probe_catalog_fills_both_columns() {
        let raw = r#"{
            "tools": [{"name": "crm_find", "description": "Find a contact", "inputSchema": {"type": "object"}}],
            "probe": {"state": "error", "detail": "1 tool; 1 left out: \"Bad Name\" is not a usable tool name", "at": "2026-10-22T12:00:00.000Z"},
            "at": "2026-10-22T12:00:00.000Z"
        }"#;
        let (tools, probe) = catalog_columns(raw).expect("probe shape");
        assert_eq!(tools[0]["name"], "crm_find");
        assert_eq!(tools.as_array().map(Vec::len), Some(1));
        assert_eq!(probe["state"], "error");
        assert!(probe["detail"].as_str().unwrap().contains("left out"));
    }

    /// The catalog never lands in the tree the host serves without auth.
    #[test]
    fn the_catalog_file_lives_in_the_apps_data_not_its_build() {
        let p = mcp_catalog_path("crm");
        assert!(p.starts_with(app_data_dir().join("crm")));
        assert!(!p.starts_with(app_builds_dir()));
        assert!(p.ends_with(MCP_CATALOG_FILE));
    }

    fn app(slug: &str, enabled: bool, mcp: bool, failed: Option<&str>) -> AppFacts {
        AppFacts {
            catalog: None,
            slug: slug.into(),
            name: format!("{slug} app"),
            description: String::new(),
            mcp,
            enabled,
            failed: failed.map(String::from),
        }
    }

    fn rows(slugs: &[&str]) -> Vec<String> {
        slugs.iter().map(|s| (*s).to_string()).collect()
    }

    /// An enabled, built app that publishes tools serves, whether or not its
    /// row exists yet; nothing else ever gets a row created.
    #[test]
    fn only_an_enabled_built_app_with_tools_serves() {
        let apps = [
            app("crm", true, true, None),
            app("notes", true, true, None),
            app("off", false, true, None),
            app("broken", true, true, Some("mcp.ts: SyntaxError")),
            app("plain", true, false, None),
        ];
        let plan = app_mcp_plan(&apps, &rows(&["crm"]));
        assert_eq!(
            plan.get("crm"),
            Some(&AppMcpRow::Serve {
                name: "crm app".into(),
                description: String::new()
            })
        );
        assert!(matches!(plan.get("notes"), Some(AppMcpRow::Serve { .. })));
        assert_eq!(plan.len(), 2, "no row is created for {plan:?}");
    }

    /// A row whose app cannot serve is turned off with the reason, never
    /// dropped from the plan: the plan has no "delete", because a delete
    /// cascades every agent, team and user grant on the row.
    #[test]
    fn a_row_whose_app_cannot_serve_turns_off_and_says_why() {
        let apps = [
            app("off", false, true, None),
            app("broken", false, true, Some("mcp.ts: SyntaxError")),
            app(
                "still-enabled-broken",
                true,
                true,
                Some("database did not start"),
            ),
            app("plain", true, false, None),
        ];
        let plan = app_mcp_plan(
            &apps,
            &rows(&["off", "broken", "still-enabled-broken", "plain", "gone"]),
        );
        let off = |s: &str| match plan.get(s) {
            Some(AppMcpRow::Off(why)) => why.clone(),
            other => panic!("{s}: expected Off, got {other:?}"),
        };
        assert_eq!(off("off"), "the app is turned off");
        assert_eq!(
            off("broken"),
            "the app failed to build or load: mcp.ts: SyntaxError"
        );
        assert_eq!(
            off("still-enabled-broken"),
            "the app failed to build or load: database did not start"
        );
        assert_eq!(off("plain"), "the app no longer publishes MCP tools");
        assert_eq!(off("gone"), "the app is not installed");
        assert_eq!(plan.len(), 5);
    }

    #[test]
    fn a_long_failure_is_clipped_to_the_cards_line() {
        let err = "x".repeat(1000);
        let plan = app_mcp_plan(
            &[app("broken", false, true, Some(&err))],
            &rows(&["broken"]),
        );
        let Some(AppMcpRow::Off(why)) = plan.get("broken") else {
            panic!("expected Off");
        };
        assert_eq!(why.chars().count(), APP_OFF_MAX);
        assert!(why.ends_with('…'));
    }

    /// Anything else leaves the row alone rather than blanking it: a torn
    /// write, an older host's file, a hand edit.
    #[test]
    fn anything_but_the_probe_shape_is_no_catalog() {
        assert!(catalog_columns("").is_none());
        assert!(catalog_columns("{not json").is_none());
        assert!(catalog_columns(r#"{"tools": {}}"#).is_none());
        assert!(catalog_columns(r#"{"tools": []}"#).is_none());
        assert!(catalog_columns(r#"{"tools": [], "probe": {"state": "ok"}}"#).is_none());
        assert!(
            catalog_columns(r#"{"tools": [], "probe": {"state": "ok", "detail": "0 tools"}}"#)
                .is_some()
        );
    }
}
