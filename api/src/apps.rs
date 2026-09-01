// Talaria apps — the ADMIN half of the registry (apps.ts after the read
// plane, which lives in users.rs): enablement, runtime install from git, the
// marketplace catalog, and the app-data wipe. Apps are self-contained
// codebases under apps/<slug>/ compiled INTO the deployment; their surfaces
// and MCP dispatch are app runtime (TS by the port's rule 10) — this module
// owns the registry state around them.
//
//   enablement   admin-controlled set in app_settings; disabled apps have no
//                nav presence and their server routes 404
//   install      marketplace/git: shallow-clone a repo into apps/<slug> —
//                the code becomes part of the NEXT build and, like anything
//                compiled into the deployment, runs fully trusted
//   catalog      the marketplace feed — JSON, source URL configurable,
//                always through the SSRF guard

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::Value;
use sqlx::PgPool;

use crate::gateway::settings::{get_setting, set_setting};
use crate::secretbox::SecretBox;
use crate::users::{apps_dir, discovered_apps, slug_ok};

const ENABLED_KEY: &str = "apps_enabled";
const INSTALLED_KEY: &str = "apps_installed";
const CATALOG_URL_KEY: &str = "apps_catalog_url";
const DEFAULT_CATALOG: &str =
    "https://raw.githubusercontent.com/outcrop-labs/talaria-apps/main/index.json";

/// The enabled set, sorted (enabledAppSlugs reads `[...cur].sort()`'s
/// written form — the setting is always stored sorted).
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

/// Reconcile registry rows with the ENABLED apps that publish MCP tools
/// (mcp-registry.ts syncAppMcpServers): upsert one row per app, drop rows
/// whose app went away — rolling the agents that carried them.
///
/// The one deliberate deviation from TS: the cached `tools` column. TS reads
/// it from the app's mcp.ts module (app runtime — rule 10); an update keeps
/// the row's cache and an insert seeds `[]`, because the live authority for
/// an app server's tool list is the TS dispatcher, which serves tools/list
/// from the module on every call.
pub async fn sync_app_mcp_servers(pg: &PgPool, sb: &SecretBox) {
    let want: Vec<_> = crate::users::enabled_apps(pg)
        .await
        .into_iter()
        .filter(|a| a.mcp)
        .map(|a| (a.slug, a.name, a.description))
        .collect();
    let have: Vec<(String, String)> = sqlx::query_as(
        "select id::text, app_slug::text from mcp_servers where app_slug is not null",
    )
    .fetch_all(pg)
    .await
    .unwrap_or_default();
    for (id, app_slug) in have {
        if !want.iter().any(|(slug, _, _)| *slug == app_slug) {
            crate::mcp_apply::roll_agents_for_server(pg, sb, &id).await;
            let _ = sqlx::query("delete from mcp_servers where id::text = $1")
                .bind(&id)
                .execute(pg)
                .await;
        }
    }
    for (slug, name, description) in want {
        let _ = sqlx::query(
            "insert into mcp_servers \
                (name, label, description, url, all_agents, app_slug, tools, tools_refreshed_at, created_by) \
             values ($1, $2, $3, $4, false, $5, '[]'::jsonb, now(), 'talaria') \
             on conflict (name) do update set \
                label = excluded.label, description = excluded.description, \
                app_slug = excluded.app_slug, tools_refreshed_at = now(), \
                enabled = true, updated_at = now()",
        )
        .bind(format!("app-{slug}"))
        .bind(&name)
        .bind(if description.is_empty() { None } else { Some(description) })
        .bind(format!("talaria-app://{slug}"))
        .bind(&slug)
        .execute(pg)
        .await;
    }
}

/// Flip one app's enablement (setAppEnabled). Enabling an app this build
/// doesn't ship is an error — the set must name real code.
pub async fn set_app_enabled(
    pg: &PgPool,
    sb: &SecretBox,
    slug: &str,
    enabled: bool,
) -> Result<(), String> {
    let mut cur: BTreeSet<String> = enabled_app_slugs(pg).await.into_iter().collect();
    if enabled {
        if !discovered_apps().iter().any(|a| a.slug == slug) {
            return Err(format!("no app \"{slug}\" in this build"));
        }
        cur.insert(slug.to_string());
    } else {
        cur.remove(slug);
    }
    let sorted = Value::Array(cur.into_iter().map(Value::String).collect());
    set_setting(pg, ENABLED_KEY, &sorted)
        .await
        .map_err(|e| e.to_string())?;
    // Apps that publish MCP tools follow their enablement into the registry
    // (rows appear/disappear; carriers get rolled on removal).
    sync_app_mcp_servers(pg, sb).await;
    Ok(())
}

/// Apps present on disk but NOT in this build — installed after the last
/// compile; the dev server picks them up on reload, prod needs a rebuild
/// (pendingApps).
pub async fn pending_apps() -> Vec<String> {
    let built: std::collections::HashSet<String> =
        discovered_apps().into_iter().map(|a| a.slug).collect();
    let Ok(mut entries) = tokio::fs::read_dir(apps_dir()).await else {
        return Vec::new();
    };
    let mut out = Vec::new();
    while let Ok(Some(e)) = entries.next_entry().await {
        if !e.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
            continue;
        }
        let name = e.file_name().to_string_lossy().into_owned();
        if !slug_ok(&name) || built.contains(&name) {
            continue;
        }
        if tokio::fs::metadata(e.path().join("talaria.json"))
            .await
            .map(|m| m.is_file())
            .unwrap_or(false)
        {
            out.push(name);
        }
    }
    out
}

/// Install an app by shallow-cloning its git repo into apps/<slug>
/// (installAppFromGit). The code becomes part of the NEXT build (dev picks
/// it up live) — and, like anything compiled into the deployment, runs fully
/// trusted. The UI says so.
pub async fn install_app_from_git(
    pg: &PgPool,
    url: &str,
    slug_override: Option<&str>,
) -> Result<(String, bool), String> {
    let u = url.trim();
    // /^https:\/\/[^\s]+$/
    let https_ok = u
        .strip_prefix("https://")
        .is_some_and(|rest| !rest.is_empty() && !rest.contains(char::is_whitespace));
    if !https_ok {
        return Err("install URL must be https://".into());
    }
    // basename(url) minus .git, lowercased, minus a talaria-app- prefix.
    let base = u.rsplit('/').next().unwrap_or("");
    let derived = slug_override
        .unwrap_or(base)
        .trim_end_matches(".git")
        .to_lowercase();
    let derived = derived.strip_prefix("talaria-app-").unwrap_or(&derived);
    if !slug_ok(derived) {
        return Err(format!(
            "\"{derived}\" is not a usable app slug (lowercase letters, digits, dashes)"
        ));
    }
    let target = apps_dir().join(derived);
    if !target.starts_with(apps_dir()) {
        return Err("bad target path".into());
    }
    if target.exists() {
        return Err(format!("apps/{derived} already exists"));
    }
    let clone = tokio::time::timeout(Duration::from_secs(60), async {
        tokio::process::Command::new("git")
            .args(["clone", "--depth", "1", u, &target.to_string_lossy()])
            .stdin(std::process::Stdio::null())
            .output()
            .await
    })
    .await
    .map_err(|_| "git clone …: timed out after 60s".to_string())?
    .map_err(|e| format!("git clone …: {e}"))?;
    if !clone.status.success() {
        let stderr = String::from_utf8_lossy(&clone.stderr);
        return Err(format!("git clone failed: {}", stderr.trim()));
    }
    let manifest_ok = tokio::fs::metadata(target.join("talaria.json"))
        .await
        .map(|m| m.is_file())
        .unwrap_or(false);
    if !manifest_ok {
        let _ = tokio::fs::remove_dir_all(&target).await;
        return Err("that repository is not a Talaria app (no talaria.json at its root)".into());
    }
    let mut installed = installed_sources(pg).await;
    let Some(obj) = installed.as_object_mut() else {
        return Err("apps_installed setting is corrupt".into());
    };
    obj.insert(
        derived.to_string(),
        serde_json::json!({
            "source": u,
            "installedAt": crate::agent_auth::epoch_ms_to_iso(now_ms()),
        }),
    );
    set_setting(pg, INSTALLED_KEY, &installed)
        .await
        .map_err(|e| e.to_string())?;
    let pending_build = !discovered_apps().iter().any(|a| a.slug == derived);
    Ok((derived.to_string(), pending_build))
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// Remove an app's codebase + enablement + install record (uninstallApp).
/// Data in the app store is wiped by the caller (it owns the confirm). Only
/// touches dirs inside appsDir.
pub async fn uninstall_app(pg: &PgPool, sb: &SecretBox, slug: &str) -> Result<(), String> {
    if !slug_ok(slug) {
        return Err("bad slug".into());
    }
    set_app_enabled(pg, sb, slug, false).await?;
    let target: PathBuf = apps_dir().join(slug);
    if !target.starts_with(apps_dir()) {
        return Err("bad target path".into());
    }
    let _ = tokio::fs::remove_dir_all(&target).await;
    let mut installed = installed_sources(pg).await;
    if let Some(obj) = installed.as_object_mut() {
        obj.remove(slug);
    }
    set_setting(pg, INSTALLED_KEY, &installed)
        .await
        .map_err(|e| e.to_string())?;
    Ok(())
}

/// Where each installed app came from (installedSources): slug →
/// { source, installedAt }.
pub async fn installed_sources(pg: &PgPool) -> Value {
    get_setting(pg, INSTALLED_KEY, serde_json::json!({})).await
}

/// Wipe every document in one app's store (app-store.ts wipe) — the DELETE
/// confirm's data half.
pub async fn wipe_app_data(pg: &PgPool, slug: &str) -> Result<u64, sqlx::Error> {
    sqlx::query("delete from app_data where app = $1")
        .bind(slug)
        .execute(pg)
        .await
        .map(|r| r.rows_affected())
}

// ── Marketplace catalog ──────────────────────────────────────────────────────

pub async fn catalog_url(pg: &PgPool) -> String {
    let stored = get_setting(pg, CATALOG_URL_KEY, Value::String(DEFAULT_CATALOG.into())).await;
    match stored.as_str() {
        Some(u) if !u.is_empty() => u.to_string(),
        _ => DEFAULT_CATALOG.to_string(),
    }
}

/// Empty (or blank) resets to the default (setCatalogUrl's `|| DEFAULT`).
pub async fn set_catalog_url(pg: &PgPool, url: Option<&str>) -> Result<(), String> {
    let v = match url.map(str::trim).filter(|u| !u.is_empty()) {
        Some(u) => Value::String(u.to_string()),
        None => Value::String(DEFAULT_CATALOG.to_string()),
    };
    set_setting(pg, CATALOG_URL_KEY, &v)
        .await
        .map_err(|e| e.to_string())
}

/// The marketplace feed (fetchCatalog). Unreachable/invalid → empty list
/// with the error, so the Discover tab can say why it's blank instead of
/// pretending it's empty.
pub async fn fetch_catalog(pg: &PgPool) -> (Vec<Value>, Option<String>) {
    let url = catalog_url(pg).await;
    let fetch = crate::safe_fetch::safe_fetch(
        &url,
        crate::safe_fetch::SafeFetch {
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
        // coerces to defaults like TS's String(x ?? d).
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
        // version is OMITTED unless present (TS spreads only the truthy).
        if let Some(v) = field("version").filter(|v| !v.is_empty()) {
            obj.insert("version".into(), v.into());
        }
        out.push(Value::Object(obj));
    }
    (out, None)
}
