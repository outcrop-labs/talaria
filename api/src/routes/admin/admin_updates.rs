// /api/admin/updates — the update engine's panel surface and the fleet's
// machine surface. GET is the panel's whole read (and green's first read
// after a cutover: it runs the boot reconcile, so a run that landed while
// nobody was looking finishes the moment anyone looks). POST drives the
// verbs; PUT flips the auto-update toggle.
//
// TWO KEYS OPEN THIS ROUTE, on purpose:
//   an admin session — the panel, a human.
//   the machine key   — talaria-infra's deploy script (x-talaria-key):
//     check, apply, rollback, adopt. It is the same trust as the fleet's
//     gateway keys, minted per instance, stored as a HASH (this row
//     serializes to the panel; the key itself must not ride along).
// Minting a key and flipping the toggle are ADMIN-ONLY: a key that could
// mint keys would be permanent, and the toggle is the consent story.

use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use sha2::{Digest, Sha256};
use sqlx::PgPool;

use crate::audit::{AuditEntry, log_audit};
use crate::body::{as_object, boolean_member, optional_enum_member, parse};
use crate::error::house_error;
use crate::session::{SessionUser, require_admin};
use crate::state::AppState;
use crate::update::mode::{InstallMode, install_mode};
use crate::update::registry::resolve_latest;
use crate::update::roll::{acting_gate, reconcile_boot, roll, rollback, run_in_flight, self_slot};
use crate::update::state::{RunBy, UpdateState, load, patch};

const LOG: &str = "[update]";

/// The header the machine key rides.
const MACHINE_KEY_HEADER: &str = "x-talaria-key";

/// sha256 hex of the presented key — both sides of the comparison are
/// hashes, so the stored row and the wire never meet in the clear.
fn key_hash(presented: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(presented.as_bytes());
    hex(&hasher.finalize())
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Mint a fresh machine key: 32 bytes of getrandom, hex. Returned ONCE —
/// the row keeps only its hash.
fn mint_key() -> String {
    let mut raw = [0u8; 32];
    getrandom::fill(&mut raw).expect("the OS rng answers");
    hex(&raw)
}

/// Who is asking: an admin session, a valid machine key, or nobody.
enum Caller {
    Admin(SessionUser),
    Machine,
    Rejected(Response),
}

/// The gate: admin session first (the common case, cheapest honest
/// check), machine key second. `machine_ok` narrows what the key may do —
/// call it with false for the admin-only verbs.
async fn caller(state: &AppState, headers: &HeaderMap, pg: &PgPool, machine_ok: bool) -> Caller {
    match require_admin(state, headers).await {
        Ok(user) => Caller::Admin(user),
        Err(gate) => {
            if machine_ok && machine_key_matches(headers, pg).await {
                Caller::Machine
            } else {
                Caller::Rejected(gate)
            }
        }
    }
}

/// Does the request carry the machine key, and does it hash to the stored
/// one? No header or no stored hash is simply false — the caller sentences.
async fn machine_key_matches(headers: &HeaderMap, pg: &PgPool) -> bool {
    let Some(presented) = headers
        .get(MACHINE_KEY_HEADER)
        .and_then(|v| v.to_str().ok())
        .filter(|v| !v.is_empty())
    else {
        return false;
    };
    let row = load(pg).await;
    row.machine_key_hash.as_deref() == Some(key_hash(presented).as_str())
}

/// GET — the panel's read. reconcile first (see the header), then the
/// whole row plus the mode's own sentence.
pub async fn get(State(state): State<AppState>, headers: HeaderMap) -> Response {
    match caller(&state, &headers, &state.pg, true).await {
        Caller::Rejected(gate) => gate,
        Caller::Admin(_) | Caller::Machine => {
            // Green's first read finishes a cutover; on every other install
            // this is a quiet no-op. A reconcile FAILURE does not fail the
            // read — the panel must be able to show a stuck run to the very
            // admin who can fix it.
            if let Err(e) = reconcile_boot(&state.pg).await {
                tracing::warn!("{LOG} boot reconcile on read: {e}");
            }
            let row = load(&state.pg).await;
            let mode = install_mode();
            let slot = self_slot().await.ok();
            Json(serde_json::json!({
                "mode": mode_name(mode),
                "sentence": mode.refusal(),
                "migrated": row.migrated,
                "running": {
                    "version": std::env::var("TALARIA_VERSION").ok().filter(|v| !v.is_empty()),
                    "digest": row.pinned.as_ref().map(|p| p.digest.clone()),
                    "slot": slot.map(slot_name),
                    "project": crate::update::roll::project(),
                },
                "autoUpdate": row.auto_update,
                "machineKeySet": row.machine_key_hash.is_some(),
                "available": row.last_check.as_ref().and_then(|c| c.available.clone()),
                "lastCheck": row.last_check,
                "lastRun": row.last_run,
                "history": row.history,
            }))
            .into_response()
        }
    }
}

fn mode_name(mode: InstallMode) -> &'static str {
    match mode {
        InstallMode::Image => "image",
        InstallMode::Checkout => "checkout",
        InstallMode::Dev => "dev",
        InstallMode::Off => "off",
    }
}

fn slot_name(slot: crate::fleet::docker::Slot) -> &'static str {
    match slot {
        crate::fleet::docker::Slot::A => "a",
        crate::fleet::docker::Slot::B => "b",
    }
}

/// POST {action}: check | apply | rollback | mint-key (adopt lands with
/// the adoption phase).
pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let action =
        match optional_enum_member(obj, "action", &["check", "apply", "rollback", "mint-key"]) {
            Ok(Some(a)) => a,
            Ok(None) => {
                return house_error(
                    StatusCode::BAD_REQUEST,
                    "action is required (check, apply, rollback, mint-key)",
                );
            }
            Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
        };

    // Minting is admin-only (a key that mints keys is a permanent key);
    // everything else takes the machine key too.
    let machine_ok = action != "mint-key";
    let who = caller(&state, &headers, &state.pg, machine_ok).await;
    let actor = match who {
        Caller::Rejected(gate) => return gate,
        Caller::Admin(u) => crate::session::actor_of(&u),
        Caller::Machine => "machine-key".to_string(),
    };

    match action.as_str() {
        "check" => check(&state.pg, &actor).await,
        "apply" => apply(&state, &actor).await,
        "rollback" => do_rollback(&state, &actor).await,
        "mint-key" => mint(&state.pg, &actor).await,
        _ => unreachable!("the enum member already gated the action"),
    }
}

/// Resolve the tracked tag and record what was available. Errors carry the
/// registry's sentence to the panel verbatim.
async fn check(pg: &PgPool, actor: &str) -> Response {
    let at = crate::agent_auth::epoch_ms_to_iso(now_ms());
    match resolve_latest().await {
        Ok(pin) => {
            let available = pin.clone();
            let wrote = patch(pg, |mut s| {
                s.last_check = Some(crate::update::state::CheckRecord {
                    at,
                    available: Some(available),
                    error: None,
                });
                s
            })
            .await;
            if let Err(e) = wrote {
                return house_error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    &format!("the check record did not write: {e}"),
                );
            }
            log_audit(
                pg,
                AuditEntry {
                    actor,
                    action: "update.check",
                    target_type: "settings",
                    target_id: None,
                    target_label: Some(pin.version.as_str()),
                    before: None,
                    after: None,
                },
            )
            .await;
            Json(serde_json::json!({ "available": pin })).into_response()
        }
        Err(e) => {
            // Record the failure too — the panel shows it beside the button.
            let _ = patch(pg, |mut s| {
                s.last_check = Some(crate::update::state::CheckRecord {
                    at,
                    available: None,
                    error: Some(e.clone()),
                });
                s
            })
            .await;
            house_error(
                StatusCode::BAD_GATEWAY,
                &format!("the registry check failed: {e}"),
            )
        }
    }
}

/// Apply now: resolve fresh (a stale check pins an old digest), gate, and
/// hand the choreography to a detached task — the roll's last act stops
/// this container, and the response must be gone long before that.
async fn apply(state: &AppState, actor: &str) -> Response {
    let row = load(&state.pg).await;
    if let Err(sentence) = acting_gate(&row, install_mode()) {
        return house_error(StatusCode::CONFLICT, &sentence);
    }
    let pin = match resolve_latest().await {
        Ok(pin) => pin,
        Err(e) => {
            return house_error(
                StatusCode::BAD_GATEWAY,
                &format!("the registry check failed: {e}"),
            );
        }
    };
    let conn = match state.redis().await {
        Ok(c) => c,
        Err(e) => {
            return house_error(
                StatusCode::SERVICE_UNAVAILABLE,
                &format!("redis unreachable for the roll: {e}"),
            );
        }
    };
    log_audit(
        &state.pg,
        AuditEntry {
            actor,
            action: "update.apply",
            target_type: "settings",
            target_id: None,
            target_label: Some(pin.version.as_str()),
            before: None,
            after: None,
        },
    )
    .await;
    let pg = state.pg.clone();
    let target = pin.clone();
    tokio::spawn(async move {
        if let Err(e) = roll(&pg, conn, &target, RunBy::Manual).await {
            tracing::warn!("{LOG} manual roll: {e}");
        }
    });
    Json(serde_json::json!({ "started": true, "to": pin })).into_response()
}

/// Roll back to the other slot. Same detached-task shape: the rollback's
/// last act stops THIS container too.
async fn do_rollback(state: &AppState, actor: &str) -> Response {
    let row = load(&state.pg).await;
    if let Err(sentence) = acting_gate(&row, install_mode()) {
        return house_error(StatusCode::CONFLICT, &sentence);
    }
    if let Some(run) = &row.last_run
        && run_in_flight(run)
    {
        return house_error(
            StatusCode::CONFLICT,
            "an update is in flight — wait for it to land before rolling back",
        );
    }
    let conn = match state.redis().await {
        Ok(c) => c,
        Err(e) => {
            return house_error(
                StatusCode::SERVICE_UNAVAILABLE,
                &format!("redis unreachable for the rollback: {e}"),
            );
        }
    };
    log_audit(
        &state.pg,
        AuditEntry {
            actor,
            action: "update.rollback",
            target_type: "settings",
            target_id: None,
            target_label: None,
            before: None,
            after: None,
        },
    )
    .await;
    let pg = state.pg.clone();
    tokio::spawn(async move {
        if let Err(e) = rollback(&pg, conn).await {
            tracing::warn!("{LOG} rollback: {e}");
        }
    });
    Json(serde_json::json!({ "started": true })).into_response()
}

/// Mint the machine key. The plaintext rides this one response and no row.
async fn mint(pg: &PgPool, actor: &str) -> Response {
    let key = mint_key();
    let stored = key_hash(&key);
    if let Err(e) = patch(pg, |mut s: UpdateState| {
        s.machine_key_hash = Some(stored.clone());
        s
    })
    .await
    {
        return house_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("the key hash did not store: {e}"),
        );
    }
    log_audit(
        pg,
        AuditEntry {
            actor,
            action: "update.key_minted",
            target_type: "settings",
            target_id: None,
            target_label: None,
            before: None,
            after: None,
        },
    )
    .await;
    Json(serde_json::json!({ "key": key })).into_response()
}

/// PUT {autoUpdate} — the toggle. Admin-only (see the header).
pub async fn put(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Response {
    let user = match require_admin(&state, &headers).await {
        Ok(u) => u,
        Err(gate) => return gate,
    };
    let parsed = parse(&body);
    let obj = match as_object(&parsed) {
        Ok(o) => o,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    let enabled = match boolean_member(obj, "autoUpdate") {
        Ok(e) => e,
        Err(msg) => return house_error(StatusCode::BAD_REQUEST, &msg),
    };
    if let Err(e) = patch(&state.pg, |mut s| {
        s.auto_update = enabled;
        s
    })
    .await
    {
        return house_error(
            StatusCode::INTERNAL_SERVER_ERROR,
            &format!("the toggle did not write: {e}"),
        );
    }
    log_audit(
        &state.pg,
        AuditEntry {
            actor: &crate::session::actor_of(&user),
            action: "settings.auto_update",
            target_type: "settings",
            target_id: None,
            target_label: None,
            before: None,
            after: Some(serde_json::json!({ "autoUpdate": enabled })),
        },
    )
    .await;
    Json(serde_json::json!({ "autoUpdate": enabled })).into_response()
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_key_hash_is_sha256_hex_of_the_presented_key() {
        assert_eq!(key_hash("").len(), 64);
        assert_ne!(key_hash("one"), key_hash("two"));
        // Deterministic: the stored hash compares equal on a later request.
        assert_eq!(key_hash("tlk-same"), key_hash("tlk-same"));
    }

    #[test]
    fn a_minted_key_is_64_hex_and_mints_differently_each_time() {
        let a = mint_key();
        let b = mint_key();
        assert_eq!(a.len(), 64);
        assert!(a.chars().all(|c| c.is_ascii_hexdigit()));
        assert_ne!(a, b, "a mint is not a constant an attacker learns");
    }
}
