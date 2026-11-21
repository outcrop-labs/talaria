// Agent-caller authentication. An agent presents ITS OWN credential: a
// `tak_` secret minted per agent_defs row, sha256-stored in agent_keys.
// Identity is resolved FROM THE
// CREDENTIAL — x-agent-name is a cross-check that can narrow access but
// never grant it.
//
// The org-wide TALARIA_AGENT_KEY names nobody: while
// TALARIA_AGENT_KEY_LEGACY ≠ 'off' those callers resolve `legacy: true` —
// identified but untrusted, and a name carrying human privilege (a personal
// or elevated assistant) is refused outright. Legacy means the identity was
// asserted, not proven; `id` is what a surface keys proof off, and it stays
// None for them.

use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use sqlx::PgPool;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};
use talaria_auth::sha256_hex;
use talaria_error::{house_error, house_error_msg, internal};

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

impl AgentCaller {
    /// The caller's identity was PROVEN: its own `tak_` key, never the
    /// org-wide shared one, which only asserts which agent it is. A door that
    /// lets an agent cite a person's word (an asked move out of review,
    /// talaria-tasks `asked`) keys off this and nothing looser.
    pub fn proven(&self) -> bool {
        !self.legacy && self.id.is_some()
    }
}

/// An agent identified either by a resolved caller (carries proof) or by a
/// bare model string from a surface that hasn't been threaded yet. Privilege
/// checks take this and consult
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
/// proxying) when the caller is legacy. Says
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

/// Is this agent (by model) a Developer Agent: enabled, with its Developer
/// switch (`agent_defs.developer`) on? THE one check behind everything the
/// switch grants — the gateway's Workbench access (talaria-mcp) and the
/// sandbox's git door (`/api/secrets/git-credential` and
/// `/api/secrets/git-push-check`) — so the switch and what it opens can never
/// disagree. An unknown or disabled agent is not one.
pub async fn agent_is_developer(pg: &PgPool, agent_model: &str) -> Result<bool, sqlx::Error> {
    let on: Option<bool> =
        sqlx::query_scalar("select developer from agent_defs where model = $1 and enabled")
            .bind(agent_model)
            .fetch_optional(pg)
            .await?;
    Ok(on.unwrap_or(false))
}

// ── Minting ──────────────────────────────────────────────────────────────────

/// Issue a fresh credential for an agent, invalidating any previous one.
/// The plaintext is returned here and never
/// again: the row keeps a sha256 (auth never decrypts) and a sealed copy (a
/// wiped fleet/.env is recoverable).
pub async fn rotate_agent_api_key(
    pg: &PgPool,
    sb: &talaria_secretbox::SecretBox,
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
    .map_err(|e| talaria_error::store_fault("[agent-auth] agent key write failed", e))?;
    Ok(secret)
}

/// The agent's credential, minting one on first use. Stable across renders
/// — re-minting on every render
/// would lock out every running container.
pub async fn ensure_agent_api_key(
    pg: &PgPool,
    sb: &talaria_secretbox::SecretBox,
    agent_id: &str,
) -> Result<String, String> {
    let existing: Option<(String,)> =
        sqlx::query_as("select key_enc from agent_keys where agent_id::text = $1")
            .bind(agent_id)
            .fetch_optional(pg)
            .await
            .map_err(|e| talaria_error::store_fault("[agent-auth] agent key read failed", e))?;
    match existing {
        // The BARE secretbox message, no wrapper sentence — /api/me/assistant
        // PATCH's 400 body carries it verbatim.
        Some((key_enc,)) => sb.open(&key_enc).map_err(|e| e.to_string()),
        None => rotate_agent_api_key(pg, sb, agent_id).await,
    }
}

/// The credential as presented: x-api-key first, else a `Bearer ` prefix.
/// (Case-sensitive single-space `Bearer ` with a trim — a plain
/// strip_prefix, NOT the gateway route's case-insensitive match.)
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
    talaria_password::constant_time_eq(a.as_bytes(), b.as_bytes())
}

/// Repeat on a slow cadence rather than once per process (15 min) — a
/// single line from whenever the server started can't
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

/// Resolve the calling agent from its credential (agentCaller):
///   • Ok(None)     no agent credential presented — dual-auth routes fall
///                  through to session auth exactly as before
///   • Err(resp)    a credential WAS presented and rejected; return it,
///                  because falling through would turn a forgery into a
///                  quiet 401
///   • Ok(Some)     identified (always a real, enabled agent)
///
/// ⚠ This resolution is also mcp/'s fleet-wide auth oracle: the MCP toolkit
/// (HTTP mode) holds no DB, so it verifies a connecting agent's credential by
/// issuing an authenticated GET /api/agent/whoami and reading the status code
/// — this function is what answers it. Narrowing that route (admin-only,
/// session-only, moved, renamed) takes the fleet toolkit dark with no error
/// pointing there; repoint TALARIA_MCP_VERIFY_PATH first. See mcp/README.md §
/// Authentication. (The oracle lived on GET /api/users until the purpose-built
/// route existed.)
pub async fn agent_caller(
    pg: &PgPool,
    headers: &HeaderMap,
) -> Result<Option<AgentCaller>, Response> {
    resolve(pg, headers).await
}

/// The calling agent or a ready-to-return 401/403, for agent-only routes.
pub async fn require_agent(pg: &PgPool, headers: &HeaderMap) -> Result<AgentCaller, Response> {
    match agent_caller(pg, headers).await {
        Ok(Some(caller)) => Ok(caller),
        Ok(None) => Err(house_error(StatusCode::UNAUTHORIZED, "unauthorized")),
        Err(resp) => Err(resp),
    }
}

fn legacy_open() -> bool {
    std::env::var("TALARIA_AGENT_KEY_LEGACY")
        .unwrap_or_else(|_| "on".into())
        .trim()
        != "off"
}

async fn resolve(pg: &PgPool, headers: &HeaderMap) -> Result<Option<AgentCaller>, Response> {
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
        .map_err(|e| internal("[agent-auth] key lookup failed", e))?;
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
        let _ = note_key_used(pg, &id);
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
        return Err(house_error(
            StatusCode::BAD_REQUEST,
            "x-agent-name required",
        ));
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
    .map_err(|e| internal("[agent-auth] name lookup failed", e))?;
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

// ── The last-used stamps ─────────────────────────────────────────────────────
//
// A credential's `last_used_at` is written on the call path of every agent
// and every LLM-key call, and nothing that reads it needs it to the second:
// the legacy migration status asks "ever used?", secret-health and Settings →
// API keys show "used N ago". So a stamp lands at most once per key per
// window, decided twice:
//   · in process, before anything else (`StampCoalescer`): inside the window
//     the call spawns nothing and checks out no connection;
//   · in the UPDATE's own WHERE, for the processes that share the database:
//     a stamp younger than four minutes matches no row, and an UPDATE that
//     matches nothing takes no transaction id and so commits nothing.
// The guard is a minute shorter than the window because the two measure on
// different clocks: the coalescer from the instant a call arrives, the guard
// from the database's `now()` at the stamp's BEGIN, which comes after the
// spawn and a pool checkout (up to the pool's acquire timeout, 15 s by
// default). A guard as long as the window would turn away a due stamp
// whenever the previous one waited longer for its connection, and the
// coalescer, having recorded the attempt, would then hold the key for a
// second whole window. With the minute's slack the coalescer alone paces a
// process, and the guard only keeps two processes from both stamping.
// The stamp that does land is telemetry, written in a transaction that does
// not wait on its WAL flush (`stamp_async`). A crash can lose the last
// fraction of a second of stamps; the next call writes one again. A key's
// FIRST stamp still lands on its first call, so "ever used" reads exactly as
// before; "used N ago" may lag the truth by up to the window (plus the
// moment the stamp waits for a connection).

/// How far a last-used stamp may lag: one per key per window in a process.
/// The UPDATEs' guard, `interval '4 minutes'`, is deliberately a minute
/// shorter (see above).
pub const STAMP_WINDOW: Duration = Duration::from_secs(5 * 60);

/// Entries kept before the coalescer sweeps out the ones past their window
/// (keys this process saw once and not since).
const STAMP_SWEEP_AT: usize = 4_096;

/// At most one stamp attempt per key per window, decided in process with no
/// database round trip.
pub struct StampCoalescer {
    window: Duration,
    last: Mutex<HashMap<String, Instant>>,
}

impl StampCoalescer {
    pub fn new(window: Duration) -> StampCoalescer {
        StampCoalescer {
            window,
            last: Mutex::new(HashMap::new()),
        }
    }

    /// Whether a stamp for `key` is due at `now`; a due one is recorded as
    /// the key's attempt. A key's first call is due, then none until a
    /// whole window has passed since its last due one.
    pub fn due_at(&self, key: &str, now: Instant) -> bool {
        let mut last = self
            .last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if last
            .get(key)
            .is_some_and(|at| now.saturating_duration_since(*at) < self.window)
        {
            return false;
        }
        if last.len() >= STAMP_SWEEP_AT {
            let window = self.window;
            last.retain(|_, at| now.saturating_duration_since(*at) < window);
        }
        last.insert(key.to_string(), now);
        true
    }

    /// The attempt for `key` did not land: its next call is due again.
    pub fn forget(&self, key: &str) {
        self.last
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(key);
    }
}

/// A loss-tolerant stamp: `sql`, with `id` its one bind, in a transaction
/// that does not wait for its WAL flush (`synchronous_commit = off`, for
/// this transaction only). For last-used stamps and nothing else: never
/// user data, secrets or anything a person would miss. `what` names the
/// table, for the probe. Rows written.
pub async fn stamp_async(
    pg: &PgPool,
    what: &'static str,
    sql: &'static str,
    id: &str,
) -> Result<u64, sqlx::Error> {
    let mut tx = pg.begin().await?;
    sqlx::query("set local synchronous_commit = off")
        .execute(&mut *tx)
        .await?;
    let seen = stamp_probe::read(&mut tx).await?;
    let rows = sqlx::query(sql)
        .bind(id)
        .execute(&mut *tx)
        .await?
        .rows_affected();
    tx.commit().await?;
    stamp_probe::record(seen, what, id, rows);
    Ok(rows)
}

/// What each async stamp did, for a test that asks. Off unless a test turns
/// it on: then each stamp reads `synchronous_commit` back inside its own
/// transaction and records it with its rows. Production pays one atomic
/// load per stamp.
pub mod stamp_probe {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicBool, Ordering};

    static ON: AtomicBool = AtomicBool::new(false);
    static SEEN: Mutex<Vec<Seen>> = Mutex::new(Vec::new());

    /// One stamp's transaction, as it committed.
    #[derive(Debug, Clone, PartialEq, Eq)]
    pub struct Seen {
        pub what: &'static str,
        pub id: String,
        /// `synchronous_commit`, read inside the transaction.
        pub synchronous_commit: String,
        pub rows: u64,
    }

    /// Record every stamp from here on.
    pub fn start() {
        ON.store(true, Ordering::SeqCst);
    }

    /// Every stamp recorded since `start`.
    pub fn seen() -> Vec<Seen> {
        SEEN.lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .clone()
    }

    pub(crate) async fn read(
        tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    ) -> Result<Option<String>, sqlx::Error> {
        if !ON.load(Ordering::SeqCst) {
            return Ok(None);
        }
        sqlx::query_scalar("show synchronous_commit")
            .fetch_one(&mut **tx)
            .await
            .map(Some)
    }

    pub(crate) fn record(seen: Option<String>, what: &'static str, id: &str, rows: u64) {
        if let Some(synchronous_commit) = seen {
            SEEN.lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .push(Seen {
                    what,
                    id: id.to_string(),
                    synchronous_commit,
                    rows,
                });
        }
    }
}

/// The agent-key stamp: a row stamped less than four minutes ago (a minute
/// inside `STAMP_WINDOW`, so a due attempt is never turned away) is not
/// matched, so it is not written. `$1::uuid` against the uuid column keeps it
/// on the primary key.
const STAMP_AGENT_KEY: &str = "update agent_keys set last_used_at = now() \
     where agent_id = $1::uuid \
       and (last_used_at is null or last_used_at < now() - interval '4 minutes')";

static AGENT_KEY_STAMPS: LazyLock<StampCoalescer> =
    LazyLock::new(|| StampCoalescer::new(STAMP_WINDOW));

/// Stamp `agent_keys.last_used_at` for an agent that just authenticated with
/// its own key — the database half alone, asynchronously committed. Rows
/// written: 0 when the stamp is still inside the window.
pub async fn stamp_agent_key(pg: &PgPool, agent_id: &str) -> Result<u64, sqlx::Error> {
    stamp_async(pg, "agent_keys", STAMP_AGENT_KEY, agent_id).await
}

/// Note that an agent just authenticated with its own key: at most once a
/// window, a detached stamp, so a bookkeeping write never holds up the call
/// it rode in on. None when this process stamped the agent within the
/// window. The handle is for a test to wait on; `resolve` drops it.
pub fn note_key_used(pg: &PgPool, agent_id: &str) -> Option<tokio::task::JoinHandle<()>> {
    note_key_used_at(pg, agent_id, Instant::now())
}

/// [`note_key_used`] at a given instant: the seam a test drives its clock
/// through.
pub fn note_key_used_at(
    pg: &PgPool,
    agent_id: &str,
    now: Instant,
) -> Option<tokio::task::JoinHandle<()>> {
    if !AGENT_KEY_STAMPS.due_at(agent_id, now) {
        return None;
    }
    let agent_id = agent_id.to_string();
    let pool = pg.clone();
    Some(tokio::spawn(async move {
        if let Err(e) = stamp_agent_key(&pool, &agent_id).await {
            AGENT_KEY_STAMPS.forget(&agent_id);
            tracing::warn!("[agent-auth] last_used_at update failed for {agent_id}: {e}");
        }
    }))
}

// ── The migration bookkeeping ────────────────────────────────────────────────
// Who is still presenting the shared key, in THIS process. Deduped for the
// log but kept as data, because "has every agent moved over yet?" is the
// question that decides when TALARIA_AGENT_KEY_LEGACY=off is safe — and a
// one-shot warn an operator scrolled past can't answer it. Process-local by
// design: it counts this process's own servings, and the admin surface that
// reads this says so.

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

/// Shared-key sightings since this process started, model-sorted via the
/// shared collating comparator (the order every model list uses).
pub fn legacy_usage() -> Vec<LegacySighting> {
    let seen = LEGACY_SEEN.lock().expect("legacy sightings");
    let mut out: Vec<LegacySighting> = seen.values().cloned().collect();
    out.sort_by(|a, b| talaria_collate::collating_cmp(&a.model, &b.model));
    out
}

/// Epoch millis as `YYYY-MM-DDTHH:MM:SS.sssZ`, always UTC — the spelling
/// every stored `at` uses. Civil-from-days (Hinnant's algorithm) — no clock
/// crate in this house.
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

/// The inverse of `epoch_ms_to_iso`: parses the ONE shape this system
/// stores — `YYYY-MM-DDTHH:MM:SS(.sss)?Z`, always UTC.
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

/// The migration answer, from the data rather than from memory:
/// agent_keys.last_used_at is written on an agent's first per-agent
/// authentication and then about once per `STAMP_WINDOW`, so an agent
/// that has one has proved it is running on its own secret.
/// Timestamps cross the wire as epoch millis (sqlx here carries no
/// timestamptz decoder by design) and format through `epoch_ms_to_iso`.
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

/// One operator-readable line, or None when there is nothing to say.
/// Rendered into the fleet render's
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

/// Wall-clock epoch milliseconds. THE one definition — 55 copies of this
/// four-line body were pasted across the tree before it moved here (the
/// epoch helpers already lived in this crate). `unwrap_or(0)`: a clock
/// before the epoch is unreadable, and 0 is the "no time" every caller
/// already treats as absent.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// The current time as an ISO-8601 string, the spelling every wire field uses.
/// Three crates had their own `now_iso` (two of them wrapping this crate's own
/// `epoch_ms_to_iso`); the pair belongs with `now_ms`.
pub fn now_iso() -> String {
    epoch_ms_to_iso(now_ms())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// One stamp per key per window, on an injected clock: the first call is
    /// due, every call inside the window is not (each key on its own), the
    /// first call a whole window after the last due one is due again, and a
    /// forgotten attempt is due at once.
    #[test]
    fn the_coalescer_lets_one_stamp_a_window_through_per_key() {
        let c = StampCoalescer::new(STAMP_WINDOW);
        let t0 = Instant::now();
        let secs = |s: u64| t0 + Duration::from_secs(s);
        assert!(c.due_at("a", t0), "a key's first call stamps");
        assert!(c.due_at("b", t0), "every key has its own window");
        for s in [0, 1, 60, 299] {
            assert!(!c.due_at("a", secs(s)), "inside the window at {s}s");
        }
        assert!(c.due_at("a", secs(300)), "a whole window on");
        assert!(
            !c.due_at("a", secs(599)),
            "the window restarts at the due call"
        );
        assert!(c.due_at("a", secs(600)));
        // An instant before the recorded one (two threads racing their
        // clocks) is inside the window, never a wrap-around.
        assert!(!c.due_at("a", secs(10)));
        assert!(!c.due_at("b", secs(299)));
        c.forget("b");
        assert!(c.due_at("b", secs(299)), "a failed stamp is tried again");
    }

    /// The sweep keeps the map bounded without forgetting a key still inside
    /// its window.
    #[test]
    fn the_coalescer_sweeps_only_keys_past_their_window() {
        let c = StampCoalescer::new(STAMP_WINDOW);
        let t0 = Instant::now();
        assert!(c.due_at("fresh", t0 + Duration::from_secs(400)));
        for i in 0..STAMP_SWEEP_AT {
            assert!(c.due_at(&format!("old-{i}"), t0));
        }
        let late = t0 + Duration::from_secs(450);
        assert!(c.due_at("trigger", late), "the sweep runs on this insert");
        let len = c.last.lock().unwrap().len();
        assert_eq!(len, 2, "only the keys inside their window are left");
        assert!(!c.due_at("fresh", late), "a key inside its window is kept");
    }

    /// Only the caller's own key proves who it is: the shared key's caller
    /// (legacy, no id) never does, and neither does a row missing its id.
    #[test]
    fn only_an_own_key_caller_is_proven() {
        let caller = |id: Option<&str>, legacy| AgentCaller {
            id: id.map(str::to_string),
            model: "eng-engineering".into(),
            legacy,
        };
        assert!(caller(Some("a-1"), false).proven());
        assert!(!caller(None, true).proven());
        assert!(!caller(Some("a-1"), true).proven());
        assert!(!caller(None, false).proven());
    }

    #[test]
    fn iso_formatter_is_to_iso_string() {
        assert_eq!(epoch_ms_to_iso(0), "1970-01-01T00:00:00.000Z");
        // Arbitrary instants, exact-string pins.
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
        // A fractionless spelling our writers never emit still parses.
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
