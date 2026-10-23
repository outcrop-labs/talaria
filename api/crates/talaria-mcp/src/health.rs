// THE MCP HEALTH PASS — the schedule that keeps `last_probe` honest.
//
// Discovery only runs when a person presses a button (install, Discover,
// Check now), so before this pass a revoked key, a dead host or a provider
// that stopped refreshing an OAuth grant left the card reading "ready"
// forever while every agent call failed. Once an hour (and on demand through
// POST /api/mcp/health) every enabled EXTERNAL server is asked again:
//
//   org + headers  initialize + tools/list with the sealed org headers
//                  (`org_headers::open_headers`), the same conversation
//                  discovery has (`registry::converse_catalog`). A changed
//                  tool NAME set replaces the cached catalog, and every
//                  assignment whose tool subset names a tool no longer
//                  offered lands in the probe as `staleAssignments`.
//   org + OAuth    `oauth_token_for(.., "org")`. A refresh the provider
//                  refuses drops the grant AND flags it in
//                  `mcp_oauth_reconnect` — at the drop, whoever the caller
//                  was (usually the gateway, mid agent call) — so a grant
//                  that is gone and flagged reads `reconnect`, one that is
//                  gone and unflagged was never connected.
//   per-user       an unauthenticated handshake (401 = the host is there),
//                  then each stored per-user OAuth grant is refreshed the
//                  same way, so a refusal surfaces within the hour even for
//                  a person whose agents are idle; Settings → Connections
//                  reads the flags and says Reconnect instead of Connect.
//
// The refresh itself is single-flight per grant (the token row's lock, in
// talaria-mcp-oauth), so this pass refreshing the same grant as the gateway
// — or as a second pass — cannot spend a rotated refresh token twice.
//
// Builtin, app and Workbench servers are this deployment's own and are not
// probed here. Package servers are: an oci-http package relays like a remote
// once its container's URL is resolved (`pkg::ensure_http`, the same step
// the gateway's first call takes — a stopped container is started, which is
// what that call would have done). A stdio package has no address to ask;
// its child spawns on first use and pkg-reconcile owns its leftovers.
//
// Each server's conversation has its own deadline (CHECK_DEADLINE), and each
// verdict is written the moment it arrives, so one hung server delays only
// its own line. Passes in one process run one at a time (PASS).
//
// NOTIFICATIONS fire on a TRANSITION, exactly once: a server the last pass
// saw `ok` that this pass sees down files one in-app notification per admin.
// The pass compares against `health_state` — what the LAST HEALTH PASS saw —
// not `last_probe`, because a person's Check now also writes `last_probe`
// and must not swallow the transition the schedule is there to announce.
// The swap is one locked UPDATE, so two passes racing (the hourly tick and
// an admin's POST) cannot both see `ok` and both notify. Recovery writes the
// probe only: the card turning green is the news.

use std::collections::{BTreeSet, HashSet};
use std::sync::Arc;

use futures_util::stream::{self, StreamExt};
use serde::Serialize;
use serde_json::{Map, Value, json};
use sqlx::PgPool;

pub use talaria_mcp_oauth::REFRESH_REJECTED;
use talaria_mcp_oauth::{has_oauth_tokens, oauth_token_for};
use talaria_notify::{NotificationInput, NotifyDeps, add_notification};
use talaria_realtime::RealtimeDeps;
use talaria_secretbox::SecretBox;

use crate::pkg::PkgSpec;
use crate::registry::{
    CatalogFailure, McpServer, ProbeState, WORKBENCH_SERVER, auth_required_message,
    converse_catalog, initialize_status, list_assignments, list_mcp_servers, probe_json,
    store_catalog,
};

const LOG: &str = "[mcp-health]";

/// The notification kind a down server files. Routed by talaria-notify's
/// kind table into the in-app `gap_reported` class: a tool agents cannot
/// use is a capability gap an admin fixes, not a 02:00 page.
pub const KIND_SERVER_DOWN: &str = "mcp_server_down";

/// Where the alert and the notification send the admin.
pub const HREF: &str = "/mcp/external";

/// How many servers are asked at once. With CHECK_DEADLINE bounding each
/// conversation, eight in flight keeps a registry of forty — every one of
/// them hung — at five rounds, well inside the job's ten-minute lease,
/// without opening forty sockets at once.
const CONCURRENCY: usize = 8;

/// The longest one server's conversation (initialize + tools/list) may take
/// in a health pass, whatever its own `timeoutSecs` says. A server allowed
/// ten minutes per agent call is still "not answering" for a health check
/// after this long, and the pass must not outlive its lease waiting on it.
const CHECK_DEADLINE: std::time::Duration = std::time::Duration::from_secs(45);

/// One pass at a time in this process: the hourly tick and an admin's POST
/// (or two admins' POSTs) queue rather than ask every server twice at once.
/// Across instances the job's lease does the same for the schedule; the
/// guarantees that must hold even then — one notification per transition,
/// one refresh per grant — are row locks, not this.
static PASS: std::sync::LazyLock<tokio::sync::Mutex<()>> =
    std::sync::LazyLock::new(|| tokio::sync::Mutex::new(()));

/// One pass, summed. `servers` is the per-server line the admin trigger and
/// the exercise read; the job logs the counts.
#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct HealthReport {
    /// External servers asked.
    pub checked: usize,
    /// Servers whose tool NAME set changed since the cached catalog.
    pub changed: usize,
    /// Servers in a down state after the pass.
    pub down: usize,
    /// Servers that went from down to ok in this pass.
    pub recovered: usize,
    /// Notification rows filed (one per admin per server that went down).
    pub notified: usize,
    pub servers: Vec<ServerHealth>,
}

#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct ServerHealth {
    pub id: String,
    pub name: String,
    pub state: String,
    pub detail: String,
    /// `down` | `recovered` | null — what the transition rule decided.
    pub event: Option<&'static str>,
    pub catalog_changed: bool,
}

// ── Pure rules ──────────────────────────────────────────────────────────────

/// A state that means agents calling this server get errors.
pub fn is_down(state: &str) -> bool {
    matches!(
        state,
        "unreachable" | "error" | "auth-required" | "reconnect"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HealthEvent {
    /// Was ok, is not: notify the admins.
    Down,
    /// Was down, is ok: write the probe, tell nobody.
    Recovered,
}

/// THE TRANSITION RULE. `prev` is the state the last health pass recorded
/// (or the last discovery's, before any pass ran); None = never probed. Only
/// `ok → down` is an event worth a notification — a server that was never
/// ok was never relied on, and down → down is the same outage.
pub fn transition(prev: Option<&str>, next: ProbeState) -> Option<HealthEvent> {
    let next = next.as_str();
    match prev {
        Some("ok") if is_down(next) => Some(HealthEvent::Down),
        Some(p) if is_down(p) && next == "ok" => Some(HealthEvent::Recovered),
        _ => None,
    }
}

/// The tool NAMES a cached catalog offers (`[{name, description?}]`).
pub fn tool_names(catalog: &Value) -> BTreeSet<String> {
    catalog
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|t| t.get("name").and_then(Value::as_str))
        .map(str::to_string)
        .collect()
}

/// The subset entries the catalog no longer offers, in the subset's order.
/// An empty catalog proves nothing (never discovered, or discovery failed)
/// so it reports nothing missing — "every tool is gone" is not a thing a
/// failed probe may claim.
pub fn missing_tools(subset: &[String], offered: &BTreeSet<String>) -> Vec<String> {
    if offered.is_empty() {
        return Vec::new();
    }
    let mut seen = HashSet::new();
    subset
        .iter()
        .filter(|t| !offered.contains(*t) && seen.insert(t.as_str()))
        .cloned()
        .collect()
}

/// `[{agent, missing}]` for every assignment with a tool subset that names a
/// tool the catalog no longer offers. A null subset is "every tool" and can
/// never go stale.
pub fn stale_assignments(
    assignments: &[(String, Option<Vec<String>>)],
    offered: &BTreeSet<String>,
) -> Vec<Value> {
    assignments
        .iter()
        .filter_map(|(agent, tools)| {
            let missing = missing_tools(tools.as_deref()?, offered);
            (!missing.is_empty()).then(|| json!({ "agent": agent, "missing": missing }))
        })
        .collect()
}

/// The most names one side of the drift sentence spells out; the rest is
/// a count. A server that swapped its whole thousand-tool catalog gets a
/// probe line, not a probe document.
const DRIFT_NAMES_MAX: usize = 20;

/// The drift sentence: what appeared and what went, by name — the first
/// `DRIFT_NAMES_MAX` of each side, "and N more" for the rest.
pub fn drift_detail(before: &BTreeSet<String>, after: &BTreeSet<String>) -> Option<String> {
    let added: Vec<&str> = after.difference(before).map(String::as_str).collect();
    let removed: Vec<&str> = before.difference(after).map(String::as_str).collect();
    if added.is_empty() && removed.is_empty() {
        return None;
    }
    let mut parts = Vec::new();
    if !added.is_empty() {
        parts.push(format!("added {}", some_names(&added)));
    }
    if !removed.is_empty() {
        parts.push(format!("removed {}", some_names(&removed)));
    }
    Some(format!("the tool catalog changed: {}", parts.join("; ")))
}

fn some_names(names: &[&str]) -> String {
    let shown = names[..names.len().min(DRIFT_NAMES_MAX)].join(", ");
    match names.len().saturating_sub(DRIFT_NAMES_MAX) {
        0 => shown,
        more => format!("{shown} and {more} more"),
    }
}

/// The servers a health pass asks: enabled, external, and with an address —
/// a remote's URL, or an oci-http package whose image is pulled (its URL is
/// the container's, resolved at check time; a `talaria-pkg://` row carries
/// only the routing token).
pub fn in_scope(s: &McpServer) -> bool {
    if !s.enabled || s.builtin || s.app_slug.is_some() || s.name == WORKBENCH_SERVER {
        return false;
    }
    if let Some(spec) = s.package.as_ref().and_then(PkgSpec::of) {
        return spec.transport == "http" && spec.pull_state == "ready";
    }
    let url = s.url.to_ascii_lowercase();
    url.starts_with("http://") || url.starts_with("https://")
}

/// A conversation that ran out of CHECK_DEADLINE, in the probe's words.
fn deadline_sentence() -> String {
    format!(
        "did not finish the handshake and tools/list within {}s, the health check's limit",
        CHECK_DEADLINE.as_secs()
    )
}

/// What one state means, for the notification body.
fn state_sentence(state: ProbeState) -> &'static str {
    match state {
        ProbeState::Unreachable => "is not answering",
        ProbeState::Error => "is answering with errors",
        ProbeState::AuthRequired => "is refusing Talaria's credentials",
        ProbeState::Reconnect => "lost its connected account",
        ProbeState::Ok => "is healthy",
    }
}

// ── The pass ────────────────────────────────────────────────────────────────

/// One server's verdict before it is written.
struct Verdict {
    state: ProbeState,
    detail: String,
    extra: Map<String, Value>,
    catalog_changed: bool,
}

impl Verdict {
    fn of(state: ProbeState, detail: impl Into<String>) -> Self {
        Self {
            state,
            detail: detail.into(),
            extra: Map::new(),
            catalog_changed: false,
        }
    }
}

/// Ask every external server once, write what each said, notify the admins
/// about the ones that just went down. Never fails as a whole: one server's
/// trouble is that server's probe, not the pass's error.
pub async fn run_health_pass(
    pg: &PgPool,
    sb: &SecretBox,
    realtime: &RealtimeDeps,
    now_ms: i64,
) -> Result<HealthReport, String> {
    let _one_at_a_time = PASS.lock().await;
    let servers = list_mcp_servers(pg).await.map_err(|e| e.to_string())?;
    let targets: Vec<McpServer> = servers.into_iter().filter(in_scope).collect();
    let notify = NotifyDeps {
        pg: pg.clone(),
        realtime: realtime.clone(),
    };
    let admins = admin_ids(pg).await;
    let mut report = HealthReport::default();
    // Written as each verdict arrives, not after the slowest one: a hung
    // server's deadline holds back only its own line.
    let mut verdicts = stream::iter(targets)
        .map(|s| async move {
            let v = check_server(pg, sb, &s).await;
            (s, v)
        })
        .buffer_unordered(CONCURRENCY);
    while let Some((s, v)) = verdicts.next().await {
        report.checked += 1;
        let probe = probe_json(v.state, &v.detail, now_ms, Some(&v.extra));
        let prev = match swap_probe(pg, &s.id, &probe, v.state).await {
            Ok(prev) => prev,
            Err(e) => {
                tracing::warn!("{LOG} probe write failed for {}: {e}", s.name);
                continue;
            }
        };
        let event = transition(prev.as_deref(), v.state);
        if v.catalog_changed {
            report.changed += 1;
        }
        if is_down(v.state.as_str()) {
            report.down += 1;
        }
        match event {
            Some(HealthEvent::Down) => {
                let title = format!("MCP server \"{}\" {}", s.label, state_sentence(v.state));
                let body = format!(
                    "{}. Agents calling its tools get errors until it is fixed.",
                    v.detail.trim_end_matches('.')
                );
                for admin in &admins {
                    match add_notification(
                        &notify,
                        admin,
                        &NotificationInput {
                            kind: KIND_SERVER_DOWN,
                            title: &title,
                            body: Some(&body),
                            href: Some(HREF),
                        },
                    )
                    .await
                    {
                        Ok(()) => report.notified += 1,
                        Err(e) => tracing::warn!("{LOG} notification failed for {admin}: {e}"),
                    }
                }
            }
            Some(HealthEvent::Recovered) => report.recovered += 1,
            None => {}
        }
        report.servers.push(ServerHealth {
            id: s.id.clone(),
            name: s.name.clone(),
            state: v.state.as_str().to_string(),
            detail: probe
                .get("detail")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_string(),
            event: event.map(|e| match e {
                HealthEvent::Down => "down",
                HealthEvent::Recovered => "recovered",
            }),
            catalog_changed: v.catalog_changed,
        });
    }
    report.servers.sort_by(|a, b| a.name.cmp(&b.name));
    Ok(report)
}

async fn check_server(pg: &PgPool, sb: &SecretBox, s: &McpServer) -> Verdict {
    // An oci-http package is asked at its container, exactly as the gateway
    // and discovery ask it (`refresh_mcp_tools` shadows the row the same way).
    let resolved;
    let s = match s.package.as_ref().and_then(PkgSpec::of) {
        Some(spec) => match crate::pkg::ensure_http(sb, s, &spec).await {
            Ok(url) => {
                resolved = McpServer { url, ..s.clone() };
                &resolved
            }
            Err(e) => {
                return Verdict::of(
                    ProbeState::Unreachable,
                    format!("the package container could not be started: {e}"),
                );
            }
        },
        None => s,
    };
    // An unreadable seal is the encryption key's fault, not the server's —
    // say that, rather than probing without the key and blaming the key.
    let org_headers = match crate::org_headers::open_headers(sb, s) {
        Ok(h) => h,
        Err(e) => {
            return Verdict::of(
                ProbeState::Error,
                format!("the stored credentials could not be opened: {e}"),
            );
        }
    };
    if s.auth_mode == "per-user" {
        return check_per_user(pg, sb, s, &org_headers).await;
    }
    let bearer = if s.oauth_enabled {
        match org_bearer(pg, sb, &s.id).await {
            Ok(b) => Some(b),
            Err(v) => return v,
        }
    } else {
        None
    };
    let conversation = tokio::time::timeout(
        CHECK_DEADLINE,
        converse_catalog(s, &org_headers, bearer.as_deref()),
    )
    .await;
    let Ok(conversation) = conversation else {
        return Verdict::of(ProbeState::Unreachable, deadline_sentence());
    };
    match conversation {
        Ok(tools) => {
            let before = tool_names(&s.tools);
            let catalog = Value::Array(tools);
            let after = tool_names(&catalog);
            let drift = (before != after)
                .then(|| drift_detail(&before, &after))
                .flatten();
            let mut catalog_changed = false;
            if before != after {
                if let Err(e) = store_catalog(pg, &s.id, &catalog).await {
                    tracing::warn!("{LOG} catalog write failed for {}: {e}", s.name);
                } else {
                    // A first discovery fills an empty cache; that is not drift.
                    catalog_changed = !before.is_empty();
                }
            }
            let mut extra = Map::new();
            match list_assignments(pg, &s.id).await {
                Ok(rows) => {
                    let stale = stale_assignments(&rows, &after);
                    if !stale.is_empty() {
                        extra.insert("staleAssignments".into(), Value::Array(stale));
                    }
                }
                Err(e) => tracing::warn!("{LOG} assignments read failed for {}: {e}", s.name),
            }
            let count = format!("{} tool(s)", after.len());
            Verdict {
                state: ProbeState::Ok,
                detail: match drift.filter(|_| catalog_changed) {
                    Some(d) => format!("{count}; {d}"),
                    None => count,
                },
                extra,
                catalog_changed,
            }
        }
        Err(CatalogFailure::Unreachable(e)) => Verdict::of(ProbeState::Unreachable, e),
        Err(CatalogFailure::Auth {
            status,
            www_authenticate,
        }) => Verdict::of(
            ProbeState::AuthRequired,
            auth_required_message(pg, s, status, www_authenticate.as_deref()).await,
        ),
        Err(CatalogFailure::Status(status)) => {
            Verdict::of(ProbeState::Error, format!("upstream {status}"))
        }
        Err(CatalogFailure::List(e)) => Verdict::of(
            ProbeState::Error,
            format!("the server answered the handshake but not tools/list: {e}"),
        ),
    }
}

/// The org bearer for an OAuth server, or the verdict that says why there is
/// none. `oauth_token_for` answers None for a refresh the provider never
/// answered (the grant stays) and for one it refused (the grant is dropped
/// and flagged at the drop); with no grant at all, the flag is what tells a
/// refused grant — refused here, by the gateway, or by an earlier pass —
/// from one nobody connected yet.
async fn org_bearer(pg: &PgPool, sb: &SecretBox, server_id: &str) -> Result<String, Verdict> {
    match oauth_token_for(pg, sb, server_id, "org").await {
        Ok(Some(token)) => Ok(token),
        Ok(None) if has_oauth_tokens(pg, server_id, "org").await.unwrap_or(true) => {
            Err(Verdict::of(
                ProbeState::Error,
                "the provider's token endpoint did not answer the refresh; the next check \
                 tries again",
            ))
        }
        Ok(None) if reconnect_flagged(pg, server_id, "org").await => {
            Err(Verdict::of(ProbeState::Reconnect, REFRESH_REJECTED))
        }
        Ok(None) => Err(Verdict::of(
            ProbeState::AuthRequired,
            "this server authenticates with OAuth and the org account is not connected yet \
             — Connect on the server card first",
        )),
        Err(e) => Err(Verdict::of(
            ProbeState::Error,
            format!("the stored OAuth grant could not be read: {e}"),
        )),
    }
}

/// A per-user server has no org identity to ask its tools as, so the pass
/// proves only that the host answers — a 401 is the expected answer — and
/// then refreshes each person's stored OAuth grant, flagging the refused
/// ones for Settings → Connections.
async fn check_per_user(
    pg: &PgPool,
    sb: &SecretBox,
    s: &McpServer,
    org_headers: &Map<String, Value>,
) -> Verdict {
    let handshake = tokio::time::timeout(CHECK_DEADLINE, initialize_status(s, org_headers)).await;
    let mut v = match handshake {
        Err(_) => Verdict::of(ProbeState::Unreachable, deadline_sentence()),
        Ok(Err(e)) => Verdict::of(ProbeState::Unreachable, e),
        Ok(Ok(status)) if status < 400 || status == 401 || status == 403 => Verdict::of(
            ProbeState::Ok,
            "reachable; each person connects their own account under Settings → Connections",
        ),
        Ok(Ok(status)) => Verdict::of(ProbeState::Error, format!("upstream {status}")),
    };
    if s.oauth_enabled {
        let flagged = sweep_user_grants(pg, sb, &s.id).await;
        if flagged > 0 {
            v.detail = format!(
                "{}; {flagged} connected account(s) need reconnecting (the provider rejected \
                 their refresh)",
                v.detail
            );
            v.extra
                .insert("reconnectAccounts".into(), Value::from(flagged as u64));
        }
    }
    v
}

/// Refresh every stored per-user grant on one server (a refused one is
/// dropped and flagged inside `oauth_token_for`), then answer how many are
/// flagged for reconnect — refused now, refused earlier by the gateway, or
/// refused before and not yet reconnected.
async fn sweep_user_grants(pg: &PgPool, sb: &SecretBox, server_id: &str) -> usize {
    let subjects: Vec<String> = sqlx::query_scalar(
        "select subject from mcp_oauth_tokens where server_id::text = $1 and subject <> 'org'",
    )
    .bind(server_id)
    .fetch_all(pg)
    .await
    .unwrap_or_default();
    for subject in &subjects {
        // The answer is not needed: a live token, an unanswered refresh or
        // an unreadable row is no verdict on the grant, and a refusal has
        // already left its flag.
        let _ = oauth_token_for(pg, sb, server_id, subject).await;
    }
    sqlx::query_scalar::<_, i64>(
        "select count(*)::int8 from mcp_oauth_reconnect r where r.server_id::text = $1 \
         and r.subject <> 'org' and not exists (select 1 from mcp_oauth_tokens t \
           where t.server_id = r.server_id and t.subject = r.subject)",
    )
    .bind(server_id)
    .fetch_one(pg)
    .await
    .map(|n| n.max(0) as usize)
    .unwrap_or(0)
}

/// THE SWAP: write this pass's probe and health state in one locked UPDATE
/// and answer the state the previous pass left (falling back to the last
/// discovery's for a server no pass has seen). The row lock is what makes
/// "notify exactly once" hold when two passes overlap.
async fn swap_probe(
    pg: &PgPool,
    id: &str,
    probe: &Value,
    state: ProbeState,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(Option<String>,)> = sqlx::query_as(
        "with prev as ( \
           select id, coalesce(health_state, last_probe->>'state') as state \
           from mcp_servers where id::text = $1 for update \
         ) \
         update mcp_servers s set last_probe = $2, health_state = $3, updated_at = now() \
         from prev where s.id = prev.id returning prev.state",
    )
    .bind(id)
    .bind(probe)
    .bind(state.as_str())
    .fetch_optional(pg)
    .await?;
    Ok(row.and_then(|(s,)| s))
}

async fn admin_ids(pg: &PgPool) -> Vec<String> {
    sqlx::query_scalar("select id::text from users where role = 'admin' order by created_at")
        .fetch_all(pg)
        .await
        .unwrap_or_else(|e| {
            tracing::warn!("{LOG} admin read failed: {e}");
            Vec::new()
        })
}

// ── Reconnect flags ─────────────────────────────────────────────────────────
//
// `oauth_token_for` DELETES the token row when the provider rejects a
// refresh (that is what forces the reconnect), so the flag cannot live on
// that row: `oauth_token_for` writes it beside the row, keyed the same way,
// at the drop, and it means "this grant existed and the provider refused
// it". A flag is only read together with the absence of tokens — a fresh
// sign-in makes it moot at once (and the callback deletes it), as does a
// disconnect.

pub(crate) async fn reconnect_flagged(pg: &PgPool, server_id: &str, subject: &str) -> bool {
    sqlx::query_scalar::<_, i32>(
        "select 1 from mcp_oauth_reconnect where server_id::text = $1 and subject = $2",
    )
    .bind(server_id)
    .bind(subject)
    .fetch_optional(pg)
    .await
    .ok()
    .flatten()
    .is_some()
}

/// Forget a grant's reconnect flag — a fresh token, or a person who
/// disconnected on purpose and should see Connect, not Reconnect.
pub async fn clear_reconnect(pg: &PgPool, server_id: &str, subject: &str) {
    if let Err(e) =
        sqlx::query("delete from mcp_oauth_reconnect where server_id::text = $1 and subject = $2")
            .bind(server_id)
            .bind(subject)
            .execute(pg)
            .await
    {
        tracing::warn!("{LOG} reconnect clear failed for {server_id}: {e}");
    }
}

/// The servers on which this subject's grant was refused and not yet
/// reconnected — what Settings → Connections turns into Reconnect.
pub async fn reconnect_servers(pg: &PgPool, subject: &str) -> HashSet<String> {
    sqlx::query_scalar::<_, String>(
        "select r.server_id::text from mcp_oauth_reconnect r where r.subject = $1 \
         and not exists (select 1 from mcp_oauth_tokens t \
           where t.server_id = r.server_id and t.subject = r.subject)",
    )
    .bind(subject)
    .fetch_all(pg)
    .await
    .map(|rows| rows.into_iter().collect())
    .unwrap_or_default()
}

// ── The job ─────────────────────────────────────────────────────────────────

/// Hourly, leased (one instance asks per hour), first run two minutes after
/// boot so a restart does not wait an hour to learn a key was revoked.
pub fn register_mcp_health_job(pg: PgPool, sb: SecretBox, realtime: RealtimeDeps) {
    talaria_scheduler::register_job(mcp_health_job_spec(pg, sb, realtime));
}

pub fn mcp_health_job_spec(
    pg: PgPool,
    sb: SecretBox,
    realtime: RealtimeDeps,
) -> talaria_scheduler::JobSpec {
    talaria_scheduler::JobSpec {
        name: talaria_scheduler::JobName::McpHealth,
        every_ms: 60 * 60_000,
        first_run_delay_ms: Some(2 * 60_000),
        max_run_ms: Some(10 * 60_000),
        per_instance: false,
        run: Arc::new(move || {
            let (pg, sb, realtime) = (pg.clone(), sb.clone(), realtime.clone());
            Box::pin(async move {
                let r = run_health_pass(&pg, &sb, &realtime, talaria_agent_auth::now_ms()).await?;
                Ok((r.checked > 0).then(|| {
                    format!(
                        "{} checked, {} down, {} recovered, {} catalog change(s), {} notified",
                        r.checked, r.down, r.recovered, r.changed, r.notified
                    )
                }))
            })
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(v: &[&str]) -> BTreeSet<String> {
        v.iter().map(|s| s.to_string()).collect()
    }
    fn strings(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn transition_ok_to_every_down_state_is_one_event() {
        for next in [
            ProbeState::Unreachable,
            ProbeState::Error,
            ProbeState::AuthRequired,
            ProbeState::Reconnect,
        ] {
            assert_eq!(
                transition(Some("ok"), next),
                Some(HealthEvent::Down),
                "{next:?}"
            );
        }
    }

    #[test]
    fn transition_down_to_down_is_the_same_outage() {
        for prev in ["unreachable", "error", "auth-required", "reconnect"] {
            for next in [
                ProbeState::Unreachable,
                ProbeState::Error,
                ProbeState::AuthRequired,
                ProbeState::Reconnect,
            ] {
                assert_eq!(transition(Some(prev), next), None, "{prev} → {next:?}");
            }
        }
    }

    #[test]
    fn transition_down_to_ok_is_a_recovery() {
        for prev in ["unreachable", "error", "auth-required", "reconnect"] {
            assert_eq!(
                transition(Some(prev), ProbeState::Ok),
                Some(HealthEvent::Recovered),
                "{prev}"
            );
        }
    }

    #[test]
    fn transition_never_probed_or_steady_ok_is_silent() {
        assert_eq!(transition(None, ProbeState::Unreachable), None);
        assert_eq!(transition(None, ProbeState::Ok), None);
        assert_eq!(transition(Some("ok"), ProbeState::Ok), None);
        // An unknown stored state (hand-edited, or a future one) is not ok.
        assert_eq!(transition(Some("mystery"), ProbeState::Error), None);
        assert_eq!(transition(Some("mystery"), ProbeState::Ok), None);
    }

    #[test]
    fn is_down_names_exactly_the_failure_states() {
        for s in ["unreachable", "error", "auth-required", "reconnect"] {
            assert!(is_down(s), "{s}");
        }
        for s in ["ok", "", "mystery"] {
            assert!(!is_down(s), "{s}");
        }
    }

    #[test]
    fn missing_tools_is_the_subset_minus_the_catalog_in_order() {
        let offered = names(&["read_wiki", "ask_question"]);
        assert_eq!(
            missing_tools(
                &strings(&["bogus", "ask_question", "gone", "bogus"]),
                &offered
            ),
            strings(&["bogus", "gone"])
        );
        assert!(missing_tools(&strings(&["read_wiki"]), &offered).is_empty());
        assert!(missing_tools(&[], &offered).is_empty());
    }

    #[test]
    fn missing_tools_claims_nothing_against_an_empty_catalog() {
        assert!(missing_tools(&strings(&["anything"]), &BTreeSet::new()).is_empty());
    }

    #[test]
    fn stale_assignments_lists_only_subsets_that_lost_tools() {
        let offered = names(&["a", "b"]);
        let rows = vec![
            ("all-tools-agent".to_string(), None),
            ("fine-agent".to_string(), Some(strings(&["a"]))),
            ("stale-agent".to_string(), Some(strings(&["a", "zzz"]))),
            ("empty-subset".to_string(), Some(Vec::new())),
        ];
        assert_eq!(
            stale_assignments(&rows, &offered),
            vec![json!({ "agent": "stale-agent", "missing": ["zzz"] })]
        );
    }

    #[test]
    fn tool_names_reads_the_cached_catalog_shape() {
        let catalog = json!([{ "name": "b" }, { "name": "a", "description": "x" }, { "nope": 1 }]);
        assert_eq!(tool_names(&catalog), names(&["a", "b"]));
        assert!(tool_names(&json!(null)).is_empty());
        assert!(tool_names(&json!({})).is_empty());
    }

    #[test]
    fn drift_detail_names_what_came_and_went() {
        assert_eq!(drift_detail(&names(&["a"]), &names(&["a"])), None);
        assert_eq!(
            drift_detail(&names(&["a", "b"]), &names(&["a", "c"])).as_deref(),
            Some("the tool catalog changed: added c; removed b")
        );
        assert_eq!(
            drift_detail(&names(&[]), &names(&["a"])).as_deref(),
            Some("the tool catalog changed: added a")
        );
        assert_eq!(
            drift_detail(&names(&["a"]), &names(&[])).as_deref(),
            Some("the tool catalog changed: removed a")
        );
    }

    /// A wholesale swap names the first twenty of each side and counts the
    /// rest; exactly twenty is spelled out in full.
    #[test]
    fn drift_detail_names_at_most_twenty_and_counts_the_rest() {
        let many: Vec<String> = (0..25).map(|i| format!("t{i:02}")).collect();
        let gone: Vec<&str> = many.iter().map(String::as_str).collect();
        let sentence = drift_detail(
            &gone.iter().map(|s| s.to_string()).collect(),
            &names(&["new"]),
        )
        .unwrap();
        assert!(sentence.starts_with("the tool catalog changed: added new; removed t00, t01, "));
        assert!(sentence.ends_with(", t19 and 5 more"), "{sentence}");
        assert!(!sentence.contains("t20"), "{sentence}");
        let exactly: Vec<&str> = gone[..20].to_vec();
        let sentence = drift_detail(&names(&exactly), &names(&[])).unwrap();
        assert!(sentence.ends_with(", t19"), "{sentence}");
        assert!(!sentence.contains("more"), "{sentence}");
    }

    #[test]
    fn the_probe_document_carries_the_new_fields_after_the_old_ones() {
        let mut extra = Map::new();
        extra.insert(
            "staleAssignments".into(),
            json!([{ "agent": "m", "missing": ["x"] }]),
        );
        let probe = probe_json(ProbeState::Ok, "2 tool(s)", 0, Some(&extra));
        assert_eq!(
            serde_json::to_string(&probe).unwrap(),
            r#"{"state":"ok","detail":"2 tool(s)","at":"1970-01-01T00:00:00.000Z","staleAssignments":[{"agent":"m","missing":["x"]}]}"#
        );
        let reconnect = probe_json(ProbeState::Reconnect, REFRESH_REJECTED, 0, None);
        assert_eq!(reconnect["state"], "reconnect");
        assert_eq!(
            reconnect.as_object().unwrap().keys().collect::<Vec<_>>(),
            ["state", "detail", "at"]
        );
    }

    #[test]
    fn the_report_serializes_camel_case() {
        let r = HealthReport {
            checked: 2,
            changed: 1,
            down: 1,
            recovered: 0,
            notified: 1,
            servers: vec![ServerHealth {
                id: "i".into(),
                name: "n".into(),
                state: "unreachable".into(),
                detail: "fetch failed".into(),
                event: Some("down"),
                catalog_changed: false,
            }],
        };
        assert_eq!(
            serde_json::to_value(&r).unwrap(),
            json!({
                "checked": 2, "changed": 1, "down": 1, "recovered": 0, "notified": 1,
                "servers": [{
                    "id": "i", "name": "n", "state": "unreachable", "detail": "fetch failed",
                    "event": "down", "catalogChanged": false,
                }],
            })
        );
    }

    fn server(over: impl FnOnce(&mut McpServer)) -> McpServer {
        let mut s = McpServer {
            id: "id".into(),
            name: "deepwiki".into(),
            label: "DeepWiki".into(),
            description: None,
            url: "https://mcp.deepwiki.com/mcp".into(),
            headers_plain: Map::new(),
            headers_sealed: None,
            timeout_secs: None,
            enabled: true,
            all_agents: false,
            auth_mode: "org".into(),
            tools: json!([]),
            tools_refreshed_ms: None,
            required_headers: json!([]),
            oauth_enabled: false,
            builtin: false,
            app_slug: None,
            created_by: None,
            created_ms: 0,
            package: None,
            env_enc: None,
            last_probe: None,
            app_off: None,
        };
        over(&mut s);
        s
    }

    #[test]
    fn scope_is_enabled_external_servers_with_an_address() {
        assert!(in_scope(&server(|_| {})));
        assert!(in_scope(&server(|s| s.auth_mode = "per-user".into())));
        assert!(in_scope(&server(|s| s.url = "HTTP://10.0.0.1/mcp".into())));
        assert!(!in_scope(&server(|s| s.enabled = false)));
        assert!(!in_scope(&server(|s| s.builtin = true)));
        assert!(!in_scope(&server(|s| s.app_slug = Some("crm".into()))));
        assert!(!in_scope(&server(|s| {
            s.name = WORKBENCH_SERVER.into();
            s.url = "talaria-workbench://core".into();
        })));
    }

    /// A package row as the registry writes it: the `talaria-pkg://` routing
    /// token for a URL, the address living in the package document.
    fn package(transport: &str, pull: &str) -> McpServer {
        server(|s| {
            s.url = "talaria-pkg://x".into();
            s.package = Some(json!({
                "kind": "oci", "identifier": "ghcr.io/x/y:v1", "image": "ghcr.io/x/y:v1",
                "transport": transport,
                "containerPort": if transport == "http" { json!(8080) } else { json!(null) },
                "pull": { "state": pull },
            }));
        })
    }

    #[test]
    fn scope_takes_pulled_http_packages_and_leaves_stdio_ones() {
        // oci-http: asked at its container once the image is pulled.
        assert!(in_scope(&package("http", "ready")));
        assert!(!in_scope(&package("http", "pulling")));
        assert!(!in_scope(&package("http", "error")));
        // stdio: no address to ask; its child spawns on first use.
        assert!(!in_scope(&package("stdio", "ready")));
        // A disabled package is out like any disabled server.
        let mut off = package("http", "ready");
        off.enabled = false;
        assert!(!in_scope(&off));
        // A package document that does not parse is no package: the
        // routing token is no address either.
        assert!(!in_scope(&server(|s| {
            s.url = "talaria-pkg://x".into();
            s.package = Some(json!({ "kind": "npm" }));
        })));
    }
}
