// The shared half of omp's auth-broker protocol, as Talaria serves it.
//
// Each Developer Agent's omp is pointed at `/api/workbench/auth` with the
// agent's own key as the bearer, so the bearer IS the scope: an agent reads
// exactly its own coding accounts and nothing else. That is the whole reason
// Talaria serves this protocol itself rather than running omp's broker as a
// sidecar — omp's broker is a vault with no notion of which agent is asking.
//
// WHAT THE PROTOCOL NEEDS FROM US (the client's own tolerances, read off
// `@oh-my-pi/pi-ai`'s broker client, decide what is implemented here):
//   required  GET  /v1/healthz, GET /v1/snapshot (ETag + optional ?wait),
//             POST /v1/credential/{id}/refresh, .../disable,
//             POST+DELETE .../block, DELETE .../blocks
//   optional  GET /v1/snapshot/stream and GET /v1/credentials/disabled both
//             404 by design — the client falls back to long-polling the
//             snapshot for the rest of its life, which is exactly what we
//             want it to do. So do the /v1/usage* endpoints: a usage fetch
//             failure is caught and cached as "unknown", and a 404 on
//             /v1/usage/observed turns client-side reporting off for good.
//   refused   POST /v1/credential. A credential arriving FROM a sandbox would
//             let an agent write its own entitlements; logins are a human
//             action in the UI, so this answers 403 with that reason rather
//             than quietly accepting one.
//
// The refresh token never leaves this process: a snapshot carries the access
// token and `__remote__` in the refresh slot, and the client calls back here
// when it expires. Refreshing is the bridge's job (it runs omp's own
// refresher), and the result re-seals before the answer is written.
//
// WHAT THE SANDBOX CAN SEE. The access token (or, for an api_key account,
// the key itself) is in the snapshot, and code running in the sandbox can
// read it as easily as omp can. That is the deal a coding account makes, and
// it is said in docs/WORKBENCH.md; what this file adds is the trail — every
// read of an account's credential writes `coding.snapshot_read` to the audit
// log, rated to one row per account per ten minutes so a harness that
// long-polls does not drown the log.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use talaria_agent_auth::require_agent;
use talaria_coding_accounts as ca;
use talaria_error::house_error;
use talaria_state::AppState;

/// omp replaces a broker-held refresh token with this before a snapshot goes
/// out. The client's schema REQUIRES the sentinel for every oauth credential,
/// which is what makes the broker the only possible refresher.
pub const REMOTE_REFRESH_SENTINEL: &str = "__remote__";

/// The agent behind a broker call: its id (the scope every account read is
/// keyed by) and its model (how the audit trail names it).
pub struct BrokerAgent {
    pub id: String,
    pub model: String,
}

/// The calling agent, or a 401. A legacy shared-key caller is refused:
/// coding accounts are per agent, and an unproven caller has no agent to be.
pub async fn broker_caller(state: &AppState, headers: &HeaderMap) -> Result<BrokerAgent, Response> {
    let caller = require_agent(&state.pg, headers).await?;
    let id = caller.id.ok_or_else(|| {
        house_error(
            StatusCode::FORBIDDEN,
            "coding accounts need a per-agent credential; this caller presented the fleet key",
        )
    })?;
    Ok(BrokerAgent {
        id,
        model: caller.model,
    })
}

/// The caller's agent id alone — what the per-credential routes key on.
pub async fn broker_agent(state: &AppState, headers: &HeaderMap) -> Result<String, Response> {
    broker_caller(state, headers).await.map(|agent| agent.id)
}

/// How often one account's credential leaving for the sandbox is written to
/// the audit trail. omp long-polls the snapshot for as long as it runs, so a
/// row per read would be a row every half-minute per account; one per ten
/// minutes still tells an operator when the sandbox could see the secret.
pub const SNAPSHOT_AUDIT_INTERVAL: Duration = Duration::from_secs(10 * 60);

/// Last audited read per account id, this process only. A multi-instance
/// deploy writes at most one row per instance per window, which is still a
/// bounded trail; a Redis key would make it exactly one, at the cost of a
/// round trip on the hot path of every snapshot.
static SNAPSHOT_AUDIT_LAST: OnceLock<Mutex<HashMap<i64, Instant>>> = OnceLock::new();

/// Whether a read at `now` earns an audit row, given when this account was
/// last audited: never, or a full interval ago.
pub fn snapshot_audit_due(last: Option<Instant>, now: Instant) -> bool {
    last.is_none_or(|at| now.duration_since(at) >= SNAPSHOT_AUDIT_INTERVAL)
}

/// Claim this read as the audited one for its window, or learn that the
/// window's row was already written. Entries whose window has lapsed are
/// dropped on the way past, so the map stays the size of the accounts read
/// in the last ten minutes.
fn claim_snapshot_audit(account_id: i64, now: Instant) -> bool {
    let map = SNAPSHOT_AUDIT_LAST.get_or_init(|| Mutex::new(HashMap::new()));
    let mut last = map.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if !snapshot_audit_due(last.get(&account_id).copied(), now) {
        return false;
    }
    last.retain(|_, at| now.duration_since(*at) < SNAPSHOT_AUDIT_INTERVAL);
    last.insert(account_id, now);
    true
}

/// The sandbox read this account's credential. One audit row per account per
/// window — actor the agent, target the account, never a value from the
/// credential itself. Called on the way out of every snapshot read, after the
/// entry was built, so a row never claims a read that did not happen.
pub fn audit_snapshot_read(state: &AppState, agent: &BrokerAgent, account: &ca::OpenAccount) {
    if !claim_snapshot_audit(account.id, Instant::now()) {
        return;
    }
    talaria_audit::spawn_audit(
        &state.pg,
        format!("agent:{}", agent.model),
        "coding.snapshot_read",
        "coding_account",
        Some(account.id.to_string()),
        Some(json!({ "agentId": agent.id, "provider": account.provider })),
    );
}

/// 404 with the protocol's own error shape — how we decline the optional
/// endpoints so the client takes its documented fallback instead of retrying.
pub fn unsupported(what: &str) -> Response {
    (
        StatusCode::NOT_FOUND,
        axum::Json(json!({ "error": format!("{what} is not served by this broker") })),
    )
        .into_response()
}

/// The snapshot's credential projection: the access token as is, the refresh
/// token replaced by the sentinel, and only the keys omp's schema accepts —
/// it rejects unknown members outright, so a stray field would fail
/// validation inside the agent and take the whole snapshot down.
pub fn snapshot_credential(credential: &Value) -> Option<Value> {
    let kind = credential.get("type").and_then(Value::as_str)?;
    if kind == "api_key" {
        let key = credential.get("key").and_then(Value::as_str)?;
        let mut out = serde_json::Map::new();
        out.insert("type".into(), json!("api_key"));
        out.insert("key".into(), json!(key));
        if let Some(src) = credential.get("source").and_then(Value::as_str) {
            out.insert("source".into(), json!(src));
        }
        return Some(Value::Object(out));
    }
    if kind != "oauth" {
        return None;
    }
    let access = credential
        .get("access")
        .and_then(Value::as_str)
        .filter(|a| !a.is_empty())?;
    let mut out = serde_json::Map::new();
    out.insert("type".into(), json!("oauth"));
    out.insert("access".into(), json!(access));
    out.insert("refresh".into(), json!(REMOTE_REFRESH_SENTINEL));
    out.insert(
        "expires".into(),
        json!(
            credential
                .get("expires")
                .and_then(Value::as_i64)
                .unwrap_or(0)
        ),
    );
    for key in [
        "apiEndpoint",
        "enterpriseUrl",
        "projectId",
        "email",
        "accountId",
        "orgId",
        "orgName",
    ] {
        if let Some(v) = credential
            .get(key)
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
        {
            out.insert(key.into(), json!(v));
        }
    }
    if let Some(ms) = credential.get("authorizedAt").and_then(Value::as_i64) {
        out.insert("authorizedAt".into(), json!(ms));
    }
    Some(Value::Object(out))
}

/// One snapshot entry for an open account.
pub async fn snapshot_entry(state: &AppState, account: &ca::OpenAccount) -> Option<Value> {
    let credential = snapshot_credential(&account.credential)?;
    let expires = account.credential.get("expires").and_then(Value::as_i64);
    // `rotatesInMs` is how long the client may hold this access token before
    // asking us to refresh. Null means "no rotation known", which is correct
    // for a credential that does not expire.
    let rotates_in = expires.and_then(|at| {
        let now = talaria_agent_auth::now_ms();
        (at > 0 && at < 10_000_000_000_000).then(|| (at - now).max(0))
    });
    let blocks = ca::blocks_for(&state.pg, account.id)
        .await
        .unwrap_or_default();
    let mut entry = serde_json::Map::new();
    entry.insert("id".into(), json!(account.id));
    entry.insert("provider".into(), json!(account.provider));
    entry.insert("credential".into(), credential);
    entry.insert(
        "identityKey".into(),
        account
            .identity_key
            .as_deref()
            .map(|k| json!(k))
            .unwrap_or(Value::Null),
    );
    entry.insert(
        "rotatesInMs".into(),
        rotates_in.map(|ms| json!(ms)).unwrap_or(Value::Null),
    );
    if !blocks.is_empty() {
        entry.insert("blocks".into(), json!(blocks));
    }
    Some(Value::Object(entry))
}

/// The agent's snapshot generation — bumped by a database trigger on every
/// insert, update and delete of a credential or a block, so a revocation moves
/// it even though it removes a row rather than touching one.
pub async fn generation(state: &AppState, agent_id: &str) -> i64 {
    sqlx::query_scalar::<_, i64>(
        "select coalesce((select rev from agent_coding_rev where agent_id::text = $1), 0)",
    )
    .bind(agent_id)
    .fetch_one(&state.pg)
    .await
    .unwrap_or(0)
}

/// The `refresher` block every snapshot and stream frame carries: what the
/// client should expect of our own refresh loop. We refresh on demand rather
/// than on a sweep, which the protocol expresses as a disabled refresher —
/// the client then calls `/refresh` itself when a credential expires, which is
/// exactly the behaviour we want.
pub fn refresher_block() -> Value {
    json!({
        "enabled": false,
        "intervalMs": 0,
        "skewMs": 5 * 60_000,
        "nextSweepInMs": 0,
    })
}

/// Resolve `{id}` against the calling agent, refusing another agent's row
/// with a 404 rather than a 403 — an agent should not learn that someone
/// else's credential id exists.
pub async fn owned_account(
    state: &AppState,
    agent_id: &str,
    id: i64,
) -> Result<ca::OpenAccount, Response> {
    let sb = state
        .secretbox()
        .await
        .map_err(|_| house_error(StatusCode::INTERNAL_SERVER_ERROR, "secretbox unavailable"))?;
    ca::open_account(&state.pg, &sb, agent_id, id)
        .await
        .map_err(|e| talaria_error::internal("[workbench/auth] credential read failed", e))?
        .ok_or_else(|| house_error(StatusCode::NOT_FOUND, "unknown credential"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_snapshot_never_carries_the_real_refresh_token() {
        let cred = json!({
            "type": "oauth",
            "access": "at-live",
            "refresh": "rt-secret",
            "expires": 1_700_000_000_000i64,
            "email": "dev@example.com",
        });
        let out = snapshot_credential(&cred).expect("projected");
        assert_eq!(out["refresh"], json!(REMOTE_REFRESH_SENTINEL));
        assert_eq!(out["access"], json!("at-live"));
        assert_eq!(out["email"], json!("dev@example.com"));
        assert_eq!(
            out.as_object().unwrap().len(),
            5,
            "only the keys omp's schema accepts"
        );
    }

    #[test]
    fn unknown_members_are_dropped_because_the_client_rejects_them() {
        let cred = json!({
            "type": "oauth",
            "access": "a",
            "refresh": "r",
            "expires": 1,
            "somethingWeAdded": "boom",
        });
        let out = snapshot_credential(&cred).expect("projected");
        assert!(out.get("somethingWeAdded").is_none());
    }

    #[test]
    fn an_api_key_credential_keeps_its_own_shape() {
        let cred = json!({ "type": "api_key", "key": "sk-x", "source": "login" });
        let out = snapshot_credential(&cred).expect("projected");
        assert_eq!(out["type"], json!("api_key"));
        assert_eq!(out["key"], json!("sk-x"));
        assert!(
            out.get("refresh").is_none(),
            "an api key has no refresh slot"
        );
    }

    #[test]
    fn a_credential_with_no_access_token_is_not_servable() {
        assert!(snapshot_credential(&json!({ "type": "oauth", "access": "" })).is_none());
        assert!(snapshot_credential(&json!({ "type": "weird" })).is_none());
    }

    #[test]
    fn the_refresher_block_tells_the_client_to_ask_us() {
        let b = refresher_block();
        assert_eq!(b["enabled"], json!(false), "no sweep; refresh on demand");
    }

    #[test]
    fn a_snapshot_read_is_audited_once_per_window_not_once_per_poll() {
        let t0 = Instant::now();
        assert!(
            snapshot_audit_due(None, t0),
            "the first read is always logged"
        );
        assert!(
            !snapshot_audit_due(Some(t0), t0 + Duration::from_secs(30)),
            "a poll half a minute later is the same window"
        );
        assert!(
            !snapshot_audit_due(
                Some(t0),
                t0 + SNAPSHOT_AUDIT_INTERVAL - Duration::from_secs(1)
            ),
            "still inside the window"
        );
        assert!(
            snapshot_audit_due(Some(t0), t0 + SNAPSHOT_AUDIT_INTERVAL),
            "a full interval later earns the next row"
        );
    }

    #[test]
    fn the_claim_is_per_account_and_the_second_poll_loses() {
        // Ids no real account has, so a parallel test cannot share the slot.
        let (a, b) = (i64::MIN + 7, i64::MIN + 8);
        let t0 = Instant::now();
        assert!(claim_snapshot_audit(a, t0));
        assert!(!claim_snapshot_audit(a, t0 + Duration::from_secs(5)));
        assert!(
            claim_snapshot_audit(b, t0),
            "another account is its own window"
        );
        assert!(claim_snapshot_audit(a, t0 + SNAPSHOT_AUDIT_INTERVAL));
    }
}
