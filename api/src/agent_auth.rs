// Agent-caller authentication — port of the resolution path of
// ui/src/server/agent-auth.ts (the parts the dual-auth routes use:
// `agentCaller`). An agent presents ITS OWN credential: a `tak_` secret
// minted per agent_defs row, sha256-stored in agent_keys. Identity is
// resolved FROM THE CREDENTIAL — x-agent-name is a cross-check that can
// narrow access but never grant it.
//
// The org-wide TALARIA_AGENT_KEY names nobody: while
// TALARIA_AGENT_KEY_LEGACY ≠ 'off' those callers resolve `legacy: true` —
// identified but untrusted, and a name carrying human privilege (a personal
// or elevated assistant) is refused outright. Legacy means the identity was
// asserted, not proven; `id` is what a surface keys proof off, and it stays
// None for them.
//
// The migration bookkeeping (legacyUsage/legacyMigrationStatus — the
// sightings map that answers "has every agent moved to its own key?") lives
// with the admin surfaces that read it and ports with them (batch 3); the
// throttled WARNING is here because the refusal path emits it.

use crate::auth::sha256_hex;
use crate::error::{house_error, house_error_msg};
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// Mirrors the `tlk_` convention so an unrecognized Bearer token is
/// distinguishable from an agent credential we simply don't know.
const KEY_PREFIX: &str = "tak_";

/// The one instruction every legacy refusal ends with — the failure has to
/// say what to DO (fleet-render's AGENT_KEY_VAR is the variable being named).
const ROLL_IT: &str =
    "Re-render the fleet and roll this container so it presents its own TALARIA_AGENT_KEY_<SLUG>.";

#[derive(Debug, Clone)]
pub struct AgentCaller {
    /// agent_defs.id — None only for a legacy shared-key caller. Some means
    /// PROVEN, and is what a surface should key privilege off.
    pub id: Option<String>,
    /// Fleet model id (`<slug>-<department>`): what per-agent control keys
    /// off. Always a real, enabled agent_defs.model, legacy or not.
    pub model: String,
    /// True when the org-wide key authenticated this caller.
    pub legacy: bool,
}

/// The credential as presented: x-api-key first, else a `Bearer ` prefix.
/// (Case-sensitive single-space `Bearer ` with a trim — exactly
/// agent-auth.ts's startsWith, NOT the gateway route's case-insensitive
/// regex.)
pub fn presented(headers: &HeaderMap) -> Option<String> {
    if let Some(x) = headers
        .get("x-api-key")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        return Some(x.to_string());
    }
    let auth = headers
        .get(axum::http::header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())?;
    let bearer = auth
        .strip_prefix("Bearer ")
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    Some(bearer.to_string())
}

/// The name the caller CLAIMS to be. Never an identity on its own.
fn declared_name(headers: &HeaderMap) -> Option<String> {
    let name = headers
        .get("x-agent-name")
        .and_then(|v| v.to_str().ok())
        .map(str::trim)
        .filter(|s| !s.is_empty())?;
    (name.len() <= 200).then(|| name.to_string())
}

fn eq(a: &str, b: &str) -> bool {
    crate::session::constant_time_eq(a.as_bytes(), b.as_bytes())
}

/// Repeat on a slow cadence rather than once per process (15 min, the TS
/// WARN_EVERY_MS) — a single line from whenever the server started can't
/// answer "is this still happening?".
fn warn_once(key: &str, line: &str) {
    static LAST: LazyLock<Mutex<HashMap<String, Instant>>> =
        LazyLock::new(|| Mutex::new(HashMap::new()));
    let mut last = LAST.lock().expect("warn map");
    let now = Instant::now();
    if now.duration_since(
        last.get(key)
            .copied()
            .unwrap_or(now - Duration::from_secs(1 << 30)),
    ) < Duration::from_secs(15 * 60)
    {
        return;
    }
    last.insert(key.to_string(), now);
    tracing::warn!("{line}");
}

/// Resolve the calling agent from its credential (agentCaller — requireName):
///   • Ok(None)     no agent credential presented — dual-auth routes fall
///                  through to session auth exactly as before
///   • Err(resp)    a credential WAS presented and rejected; return it,
///                  because falling through would turn a forgery into a
///                  quiet 401
///   • Ok(Some)     identified (always a real, enabled agent)
pub async fn agent_caller(
    pg: &PgPool,
    headers: &HeaderMap,
) -> Result<Option<AgentCaller>, Response> {
    resolve(pg, headers, true).await
}

async fn resolve(
    pg: &PgPool,
    headers: &HeaderMap,
    require_name: bool,
) -> Result<Option<AgentCaller>, Response> {
    let Some(secret) = presented(headers) else {
        return Ok(None);
    };
    let claimed = declared_name(headers);

    if secret.starts_with(KEY_PREFIX) {
        let row: Option<(String, String, bool)> = sqlx::query_as(
            "select d.id::text, d.model, d.enabled from agent_keys k \
                 join agent_defs d on d.id = k.agent_id where k.key_hash = $1",
        )
        .bind(sha256_hex(&secret))
        .fetch_optional(pg)
        .await
        .map_err(|e| {
            tracing::error!("[agent-auth] key lookup failed: {e}");
            house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
        })?;
        let Some((id, model, enabled)) = row else {
            return Err(house_error(
                StatusCode::UNAUTHORIZED,
                "unknown agent credential",
            ));
        };
        if !enabled {
            return Err(house_error(StatusCode::FORBIDDEN, "this agent is retired"));
        }
        // The cross-check: a credential that says one thing and a header that
        // says another is a misconfiguration at best and impersonation at
        // worst; refuse rather than silently pick one of the two identities.
        if let Some(claimed) = claimed.as_deref()
            && claimed != model
        {
            return Err(house_error(
                StatusCode::FORBIDDEN,
                &format!("x-agent-name \"{claimed}\" does not match the presenting agent"),
            ));
        }
        // Detached last_used_at — the migration-status answer keys off it.
        let agent_id = id.clone();
        let pool = pg.clone();
        tokio::spawn(async move {
            if let Err(e) =
                sqlx::query("update agent_keys set last_used_at = now() where agent_id::text = $1")
                    .bind(&agent_id)
                    .execute(&pool)
                    .await
            {
                tracing::warn!("[agent-auth] last_used_at update failed for {agent_id}: {e}");
            }
        });
        return Ok(Some(AgentCaller {
            id: Some(id),
            model,
            legacy: false,
        }));
    }

    // ── The org-wide shared key: the legacy window ─────────────────────────
    let shared = std::env::var("TALARIA_AGENT_KEY")
        .unwrap_or_default()
        .trim()
        .to_string();
    // Not a credential we issued — leave the request to whatever else
    // authenticates this route (session cookie, gateway key).
    if shared.is_empty() || !eq(&secret, &shared) {
        return Ok(None);
    }
    let window_open = std::env::var("TALARIA_AGENT_KEY_LEGACY")
        .unwrap_or_else(|_| "on".into())
        .trim()
        != "off";
    if !window_open {
        let claimed_note = claimed
            .as_deref()
            .map(|c| format!(" (claimed: \"{c}\")"))
            .unwrap_or_default();
        return Err(house_error_msg(
            StatusCode::UNAUTHORIZED,
            "the org-wide agent key is retired — present the agent's own credential",
            &format!(
                "The org-wide TALARIA_AGENT_KEY no longer authenticates anyone{claimed_note}. {ROLL_IT}"
            ),
        ));
    }
    // The shared key proves fleet membership and nothing else. An unnamed
    // caller gets no identity and therefore no per-agent access — refused,
    // not waved through.
    let Some(claimed) = claimed else {
        if require_name {
            return Err(house_error(
                StatusCode::BAD_REQUEST,
                "x-agent-name required",
            ));
        }
        return Ok(Some(AgentCaller {
            id: None,
            model: String::new(),
            legacy: true,
        }));
    };
    // The claimed name is resolved, never taken on faith: a retired or
    // invented name must not authenticate.
    let def: Option<(String, String, String, bool, bool, bool)> = sqlx::query_as(
        "select id::text, slug, model, enabled, owner_user_id is not null, elevated \
             from agent_defs where model = $1",
    )
    .bind(&claimed)
    .fetch_optional(pg)
    .await
    .map_err(|e| {
        tracing::error!("[agent-auth] name lookup failed: {e}");
        house_error(StatusCode::INTERNAL_SERVER_ERROR, "internal error")
    })?;
    let Some((_id, slug, model, enabled, personal, elevated)) = def else {
        return Err(house_error(
            StatusCode::FORBIDDEN,
            &format!("unknown agent \"{claimed}\""),
        ));
    };
    if !enabled {
        return Err(house_error(StatusCode::FORBIDDEN, "this agent is retired"));
    }
    // A name that carries HUMAN privilege can't be asserted, only proven.
    if personal || elevated {
        // Also LOG it: dual-auth routes turn a rejection into a plain 401 for
        // the caller, so the log is where an operator finds out.
        let slug_up = slug.to_uppercase();
        tracing::error!(
            "[agent-auth] \"{model}\" presented the org-wide TALARIA_AGENT_KEY but acts for a human — refused. Re-render the fleet and roll its container onto TALARIA_AGENT_KEY_{slug_up}."
        );
        return Err(house_error_msg(
            StatusCode::FORBIDDEN,
            "this agent must present its own credential",
            &format!(
                "\"{model}\" acts for a human (personal assistant / elevated), so the org-wide TALARIA_AGENT_KEY cannot authenticate it — it proves fleet membership, not identity. Re-render the fleet and roll this container so it presents TALARIA_AGENT_KEY_{slug_up}."
            ),
        ));
    }
    warn_once(
        &format!("legacy:{model}"),
        &format!(
            "[agent-auth] \"{model}\" authenticated with the org-wide TALARIA_AGENT_KEY (deprecated — self-declared identity, so no elevation, owner-proxying or OAuth). Re-render the fleet and roll this agent onto its own credential."
        ),
    );
    // id stays None: the identity is asserted, not proven.
    Ok(Some(AgentCaller {
        id: None,
        model,
        legacy: true,
    }))
}
