// Agent-caller authentication — port of ui/src/server/agent-auth.ts. An
// agent presents ITS OWN credential: a `tak_` secret minted per agent_defs
// row, sha256-stored in agent_keys. Identity is resolved FROM THE
// CREDENTIAL — x-agent-name is a cross-check that can narrow access but
// never grant it.
//
// The org-wide TALARIA_AGENT_KEY names nobody: while
// TALARIA_AGENT_KEY_LEGACY ≠ 'off' those callers resolve `legacy: true` —
// identified but untrusted, and a name carrying human privilege (a personal
// or elevated assistant) is refused outright. Legacy means the identity was
// asserted, not proven; `id` is what a surface keys proof off, and it stays
// None for them.

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

/// An agent identified either by a resolved caller (carries proof) or by a
/// bare model string from a surface that hasn't been threaded yet
/// (agent-auth.ts AgentSubject). Privilege checks take this and consult
/// `legacy`; a plain string is treated as proven, which is safe because a
/// legacy caller can never present a privileged name.
#[derive(Debug, Clone)]
pub enum AgentSubject {
    Caller(AgentCaller),
    Model(String),
}

pub fn subject_model(subject: &AgentSubject) -> &str {
    match subject {
        AgentSubject::Caller(c) => &c.model,
        AgentSubject::Model(m) => m,
    }
}

/// False only for a legacy shared-key caller: identified, but not proven, so
/// it gets no elevation, no owner-proxying and no OAuth tokens.
pub fn subject_proven(subject: &AgentSubject) -> bool {
    match subject {
        AgentSubject::Model(_) => true,
        AgentSubject::Caller(c) => !c.legacy,
    }
}

/// Ready-to-return refusal for a surface that acts as a HUMAN (Google, owner
/// proxying) when the caller is legacy (agent-auth.ts refuseLegacy). Says
/// what is wrong AND what to do — a bare 403 here reads as a broken
/// integration rather than a migration step.
pub fn refuse_legacy(caller: &AgentCaller, what: &str) -> Option<Response> {
    if !caller.legacy {
        return None;
    }
    Some(house_error_msg(
        StatusCode::FORBIDDEN,
        "forbidden",
        &format!(
            "{what} needs the agent's own credential. \"{}\" authenticated with the org-wide TALARIA_AGENT_KEY, which proves fleet membership but not identity. {ROLL_IT}",
            caller.model
        ),
    ))
}

// ── Minting ──────────────────────────────────────────────────────────────────

/// Issue a fresh credential for an agent, invalidating any previous one
/// (agent-auth.ts rotateAgentApiKey). The plaintext is returned here and never
/// again: the row keeps a sha256 (auth never decrypts) and a sealed copy (a
/// wiped fleet/.env is recoverable).
pub async fn rotate_agent_api_key(
    pg: &PgPool,
    sb: &crate::secretbox::SecretBox,
    agent_id: &str,
) -> Result<String, String> {
    let mut raw = [0u8; 24];
    getrandom::fill(&mut raw).map_err(|e| format!("key material unavailable: {e}"))?;
    let hex: String = raw.iter().map(|b| format!("{b:02x}")).collect();
    let secret = format!("{KEY_PREFIX}{hex}");
    let sealed = sb
        .seal(&secret)
        .map_err(|e| format!("agent key seal failed: {e}"))?;
    sqlx::query(
        "insert into agent_keys (agent_id, key_hash, key_enc) values ($1::uuid, $2, $3) \
         on conflict (agent_id) do update \
         set key_hash = excluded.key_hash, key_enc = excluded.key_enc, created_at = now()",
    )
    .bind(agent_id)
    .bind(sha256_hex(&secret))
    .bind(&sealed)
    .execute(pg)
    .await
    .map_err(|e| format!("agent key write failed: {e}"))?;
    Ok(secret)
}

/// The agent's credential, minting one on first use (agent-auth.ts
/// ensureAgentApiKey). Stable across renders — re-minting on every render
/// would lock out every running container.
pub async fn ensure_agent_api_key(
    pg: &PgPool,
    sb: &crate::secretbox::SecretBox,
    agent_id: &str,
) -> Result<String, String> {
    let existing: Option<(String,)> =
        sqlx::query_as("select key_enc from agent_keys where agent_id::text = $1")
            .bind(agent_id)
            .fetch_optional(pg)
            .await
            .map_err(|e| format!("agent key read failed: {e}"))?;
    match existing {
        Some((key_enc,)) => sb
            .open(&key_enc)
            .map_err(|e| format!("agent key unseal failed: {e}")),
        None => rotate_agent_api_key(pg, sb, agent_id).await,
    }
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

/// The calling agent or a ready-to-return 401/403, for agent-only routes
/// (agent-auth.ts requireAgent).
pub async fn require_agent(pg: &PgPool, headers: &HeaderMap) -> Result<AgentCaller, Response> {
    match agent_caller(pg, headers).await {
        Ok(Some(caller)) => Ok(caller),
        Ok(None) => Err(house_error(StatusCode::UNAUTHORIZED, "unauthorized")),
        Err(resp) => Err(resp),
    }
}

/// Identity for the FLEET-PLANE endpoints whose subject is in the URL
/// (register, heartbeat — agent-auth.ts fleetCaller). Same validation as
/// `agent_caller`, except a legacy caller that sent no x-agent-name resolves
/// with an EMPTY model instead of a 400: the pre-per-key plugin doesn't send
/// the header, and the subject is the URL anyway. A caller we CAN name must
/// still match the subject — that is what stops agent A reading agent B's
/// work. (TS hands back `{ model: null }` for the unnamed shape; here the
/// same caller arrives with `model: ""`.)
pub async fn fleet_caller(
    pg: &PgPool,
    headers: &HeaderMap,
) -> Result<Option<AgentCaller>, Response> {
    resolve(pg, headers, false).await
}

/// Any credential the fleet holds — a per-agent one, or the org-wide key
/// while the window is open (agent-auth.ts checkFleetKey). For the
/// fleet-plane endpoints that carry their subject in the URL and so need no
/// caller identity. Retirement is `agent_defs.enabled = false` and it does
/// NOT delete the agent_keys row, so the join onto agent_defs is what makes
/// "retiring an agent revokes its access" true here as well.
pub async fn check_fleet_key(pg: &PgPool, headers: &HeaderMap) -> Result<bool, sqlx::Error> {
    let Some(secret) = presented(headers) else {
        return Ok(false);
    };
    if secret.starts_with(KEY_PREFIX) {
        let row: Option<(i32,)> = sqlx::query_as(
            "select 1 from agent_keys k join agent_defs d on d.id = k.agent_id \
             where k.key_hash = $1 and d.enabled",
        )
        .bind(sha256_hex(&secret))
        .fetch_optional(pg)
        .await?;
        return Ok(row.is_some());
    }
    let shared = std::env::var("TALARIA_AGENT_KEY")
        .unwrap_or_default()
        .trim()
        .to_string();
    Ok(!shared.is_empty() && legacy_open() && eq(&secret, &shared))
}

fn legacy_open() -> bool {
    std::env::var("TALARIA_AGENT_KEY_LEGACY")
        .unwrap_or_else(|_| "on".into())
        .trim()
        != "off"
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
    let window_open = legacy_open();
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
    warn_legacy(&model);
    // id stays None: the identity is asserted, not proven.
    Ok(Some(AgentCaller {
        id: None,
        model,
        legacy: true,
    }))
}

// ── The migration bookkeeping ────────────────────────────────────────────────
// Who is still presenting the shared key, in THIS process (agent-auth.ts
// legacySeen). Deduped for the log but kept as data, because "has every agent
// moved over yet?" is the question that decides when
// TALARIA_AGENT_KEY_LEGACY=off is safe — and a one-shot console.warn an
// operator scrolled past can't answer it. Process-local by design in TS, and
// the port keeps that property: during coexistence each runtime counts its
// own servings, and the admin surface that reads this notes it.

/// Wire shape (camelCase — the admin instance route serves these verbatim).
#[derive(Debug, Clone, serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacySighting {
    pub model: String,
    pub count: u64,
    pub first_at: String,
    pub last_at: String,
}

static LEGACY_SEEN: LazyLock<Mutex<HashMap<String, LegacySighting>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn warn_legacy(model: &str) {
    {
        let mut seen = LEGACY_SEEN.lock().expect("legacy sightings");
        let now = epoch_ms_to_iso(now_ms());
        let entry = seen
            .entry(model.to_string())
            .or_insert_with(|| LegacySighting {
                model: model.to_string(),
                count: 0,
                first_at: now.clone(),
                last_at: now.clone(),
            });
        entry.count += 1;
        entry.last_at = now;
    }
    warn_once(
        &format!("legacy:{model}"),
        &format!(
            "[agent-auth] \"{model}\" authenticated with the org-wide TALARIA_AGENT_KEY (deprecated — self-declared identity, so no elevation, owner-proxying or OAuth). Re-render the fleet and roll this agent onto its own credential."
        ),
    );
}

/// Shared-key sightings since this process started, model-sorted. (TS sorts
/// `localeCompare`; fleet model ids are ASCII `slug-dept`, where that and
/// byte order agree.)
pub fn legacy_usage() -> Vec<LegacySighting> {
    let seen = LEGACY_SEEN.lock().expect("legacy sightings");
    let mut out: Vec<LegacySighting> = seen.values().cloned().collect();
    out.sort_by(|a, b| a.model.cmp(&b.model));
    out
}

/// Wall-clock epoch millis — `Date.now()`, for the sighting timestamps.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `new Date(ms).toISOString()`: `YYYY-MM-DDTHH:MM:SS.sssZ`, always UTC.
/// Civil-from-days (Hinnant's algorithm) — no clock crate in this house.
pub fn epoch_ms_to_iso(ms: i64) -> String {
    let secs = ms.div_euclid(1000);
    let millis = ms.rem_euclid(1000);
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);
    let (h, m, s) = (tod / 3600, (tod % 3600) / 60, tod % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 {
        yoe + era * 400 + 1
    } else {
        yoe + era * 400
    };
    format!("{year:04}-{month:02}-{d:02}T{h:02}:{m:02}:{s:02}.{millis:03}Z")
}

/// Days-from-civil (Hinnant) — the inverse of the era arithmetic above.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y.rem_euclid(400); // [0, 399]
    let mp = (m + 9) % 12; // Mar=0
    let doy = (153 * mp + 2) / 5 + d - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// The inverse of `epoch_ms_to_iso`: `Date.parse` for the ONE shape this
/// system stores — `toISOString()` output, `YYYY-MM-DDTHH:MM:SS(.sss)?Z`.
/// Anything else is None, and every caller wants exactly that: a stored `at`
/// that no longer parses is expired, not eternal.
pub fn iso_to_epoch_ms(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    let num = |range: std::ops::Range<usize>| -> Option<i64> {
        let mut v: i64 = 0;
        for &c in b.get(range)? {
            if !c.is_ascii_digit() {
                return None;
            }
            v = v * 10 + (c - b'0') as i64;
        }
        Some(v)
    };
    if b.len() < 20
        || b[4] != b'-'
        || b[7] != b'-'
        || (b[10] != b'T' && b[10] != b't')
        || b[13] != b':'
        || b[16] != b':'
    {
        return None;
    }
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (h, m, sec) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }
    if h > 23 || m > 59 || sec > 59 {
        return None;
    }
    let mut i = 19;
    let mut millis: i64 = 0;
    if b.get(i) == Some(&b'.') {
        i += 1;
        let start = i;
        let mut scale = 100;
        while i < b.len() && i - start < 3 && b[i].is_ascii_digit() {
            millis += (b[i] - b'0') as i64 * scale;
            scale /= 10;
            i += 1;
        }
        if i == start {
            return None; // '.' with no digits
        }
    }
    // The writers always spell UTC; a bare local-time string is not one of ours.
    match b.get(i) {
        Some(&b'Z') | Some(&b'z') => i += 1,
        _ => return None,
    }
    if i != b.len() {
        return None; // trailing garbage
    }
    let days = days_from_civil(year, month, day);
    Some((days * 86_400 + h * 3600 + m * 60 + sec) * 1000 + millis)
}

/// Wire shape (camelCase — the admin instance route serves these verbatim).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyAgentStatus {
    pub model: String,
    pub key_minted: bool,
    pub last_used_at: Option<String>,
    pub migrated: bool,
}

/// Wire shape (camelCase — the admin instance route serves these verbatim).
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LegacyMigrationStatus {
    /// TALARIA_AGENT_KEY_LEGACY is not 'off'.
    pub window_open: bool,
    pub agents: Vec<LegacyAgentStatus>,
    /// Enabled managed agents that have NEVER authenticated with their own
    /// credential — flipping the flag locks exactly these out.
    pub pending: Vec<String>,
    /// Shared-key callers this process has served.
    pub legacy_seen: Vec<LegacySighting>,
}

/// The migration answer, from the data rather than from memory
/// (agent-auth.ts legacyMigrationStatus): agent_keys.last_used_at is written
/// on every per-agent authentication, so an agent that has one has proved it
/// is running on its own secret. Timestamps cross the wire as epoch millis
/// (sqlx here carries no timestamptz decoder by design) and format through
/// the same `toISOString` the TS side uses.
pub async fn legacy_migration_status(pg: &PgPool) -> Result<LegacyMigrationStatus, sqlx::Error> {
    let rows: Vec<(String, bool, Option<i64>)> = sqlx::query_as(
        "select d.model, k.agent_id is not null, (extract(epoch from k.last_used_at) * 1000)::bigint \
         from agent_defs d left join agent_keys k on k.agent_id = d.id \
         where d.enabled and d.managed order by d.model",
    )
    .fetch_all(pg)
    .await?;
    let agents: Vec<LegacyAgentStatus> = rows
        .into_iter()
        .map(|(model, key_minted, last_ms)| LegacyAgentStatus {
            model,
            key_minted,
            last_used_at: last_ms.map(epoch_ms_to_iso),
            migrated: last_ms.is_some(),
        })
        .collect();
    let pending = agents
        .iter()
        .filter(|a| !a.migrated)
        .map(|a| a.model.clone())
        .collect();
    Ok(LegacyMigrationStatus {
        window_open: legacy_open(),
        agents,
        pending,
        legacy_seen: legacy_usage(),
    })
}

/// One operator-readable line, or None when there is nothing to say
/// (agent-auth.ts legacyMigrationWarning). Rendered into the fleet render's
/// warnings (where an admin actually looks) and logged. This is the guard for
/// the migration order — flipping the flag before the fleet is rolled is a
/// total outage, and nothing else says so.
pub fn legacy_migration_warning(s: &LegacyMigrationStatus) -> Option<String> {
    if s.pending.is_empty() {
        return None;
    }
    let who = s.pending.join(", ");
    if !s.window_open {
        // The trap sprung: every pending agent is ALREADY failing auth. Say it
        // instead of leaving a pile of undiagnosable 401s to be read.
        return Some(format!(
            "TALARIA_AGENT_KEY_LEGACY=off but {} agent(s) have never authenticated with their own credential — they are locked out right now: {who}. Roll their containers (render → rollRunningAgents), or set the flag back to 'on' until they have.",
            s.pending.len()
        ));
    }
    Some(format!(
        "per-agent credential migration: {}/{} done. Still on the org-wide key (or never seen since): {who}. Roll their containers before setting TALARIA_AGENT_KEY_LEGACY=off — flipping it first is a fleet-wide outage.",
        s.agents.len() - s.pending.len(),
        s.agents.len()
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iso_formatter_is_to_iso_string() {
        assert_eq!(epoch_ms_to_iso(0), "1970-01-01T00:00:00.000Z");
        // Arbitrary instants, pinned against node's own toISOString.
        assert_eq!(
            epoch_ms_to_iso(1_796_970_954_478),
            "2026-12-11T06:35:54.478Z"
        );
        assert_eq!(
            epoch_ms_to_iso(1_796_970_954_578),
            "2026-12-11T06:35:54.578Z"
        );
        // Leap-year February: 2024-02-29T12:00:00.000Z.
        assert_eq!(
            epoch_ms_to_iso(1_709_208_000_000),
            "2024-02-29T12:00:00.000Z"
        );
        // Year boundary: 2025-12-31T23:59:59.999Z.
        assert_eq!(
            epoch_ms_to_iso(1_767_225_599_999),
            "2025-12-31T23:59:59.999Z"
        );
        // Pre-epoch: 1969-12-31T23:59:59.500Z.
        assert_eq!(epoch_ms_to_iso(-500), "1969-12-31T23:59:59.500Z");
    }

    #[test]
    fn iso_parser_is_the_inverse_of_the_formatter() {
        for ms in [
            0i64,
            -500,
            1_796_970_954_478,
            1_709_208_000_000,
            1_767_225_599_999,
        ] {
            assert_eq!(iso_to_epoch_ms(&epoch_ms_to_iso(ms)), Some(ms), "{ms}");
        }
        // The other spellings Date.parse tolerates that our writers never emit.
        assert_eq!(
            iso_to_epoch_ms("2026-08-29T07:00:00Z"),
            Some(1_787_986_800_000)
        );
        // 1-2 digit fractions parse (truncated to millis).
        assert_eq!(
            iso_to_epoch_ms("2026-08-29T07:00:00.5Z"),
            Some(1_787_986_800_500)
        );
        assert_eq!(
            iso_to_epoch_ms("2026-08-29T07:00:00.42Z"),
            Some(1_787_986_800_420)
        );
        // Not our shape: expired, never eternal.
        for bad in [
            "",
            "nonsense",
            "2026-08-29",
            "2026-08-29T07:00:00",          // no Z
            "2026-08-29T07:00:00+02:00",    // offset form
            "2026-13-01T00:00:00.000Z",     // month
            "2026-08-32T00:00:00.000Z",     // day
            "2026-08-29T24:00:00.000Z",     // hour
            "2026-08-29T07:00:00.000Zjunk", // trailing
            "2026/08/29T07:00:00.000Z",     // separators
        ] {
            assert!(iso_to_epoch_ms(bad).is_none(), "{bad}");
        }
    }

    #[test]
    fn migration_warning_says_who_and_when_to_flip() {
        let status = |window_open: bool, migrated: usize, pending: &[&str]| {
            // One row per agent, like the DB query: migrated first, then the
            // stragglers (the counts in the warning are over BOTH).
            let mut agents = (0..migrated)
                .map(|_| LegacyAgentStatus {
                    model: "moved-on".into(),
                    key_minted: true,
                    last_used_at: Some("2026-08-29T07:00:00.000Z".into()),
                    migrated: true,
                })
                .collect::<Vec<_>>();
            agents.extend(pending.iter().map(|p| LegacyAgentStatus {
                model: (*p).into(),
                key_minted: true,
                last_used_at: None,
                migrated: false,
            }));
            LegacyMigrationStatus {
                window_open,
                agents,
                pending: pending.iter().map(|p| p.to_string()).collect(),
                legacy_seen: vec![],
            }
        };
        // Nothing pending → nothing to say.
        assert_eq!(legacy_migration_warning(&status(true, 1, &[])), None);
        // Window open: the counts and the "flip is an outage" warning.
        assert_eq!(
            legacy_migration_warning(&status(true, 2, &["analyst-eng", "scout-research"])),
            Some("per-agent credential migration: 2/4 done. Still on the org-wide key (or never seen since): analyst-eng, scout-research. Roll their containers before setting TALARIA_AGENT_KEY_LEGACY=off — flipping it first is a fleet-wide outage.".into())
        );
        // Window closed with stragglers: the trap has already sprung.
        assert_eq!(
            legacy_migration_warning(&status(false, 0, &["analyst-eng"])),
            Some("TALARIA_AGENT_KEY_LEGACY=off but 1 agent(s) have never authenticated with their own credential — they are locked out right now: analyst-eng. Roll their containers (render → rollRunningAgents), or set the flag back to 'on' until they have.".into())
        );
    }

    #[test]
    fn migration_status_serializes_camel_case() {
        let s = LegacyMigrationStatus {
            window_open: true,
            agents: vec![LegacyAgentStatus {
                model: "helpdesk-triage".into(),
                key_minted: true,
                last_used_at: Some("2026-08-29T07:00:00.000Z".into()),
                migrated: true,
            }],
            pending: vec![],
            legacy_seen: vec![LegacySighting {
                model: "helpdesk-triage".into(),
                count: 3,
                first_at: "2026-08-29T07:00:00.000Z".into(),
                last_at: "2026-08-29T08:00:00.000Z".into(),
            }],
        };
        assert_eq!(
            serde_json::to_string(&s).expect("serializes"),
            "{\"windowOpen\":true,\"agents\":[{\"model\":\"helpdesk-triage\",\"keyMinted\":true,\"lastUsedAt\":\"2026-08-29T07:00:00.000Z\",\"migrated\":true}],\"pending\":[],\"legacySeen\":[{\"model\":\"helpdesk-triage\",\"count\":3,\"firstAt\":\"2026-08-29T07:00:00.000Z\",\"lastAt\":\"2026-08-29T08:00:00.000Z\"}]}"
        );
    }
}
