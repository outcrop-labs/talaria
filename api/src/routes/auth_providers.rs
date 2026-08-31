// GET /api/auth/providers — port of ui/src/routes/api/auth/providers.ts. The
// login screen asks one question: what doors exist on this instance? Answered
// live on every call, so flipping a toggle or creating the first password
// account changes the screen without a restart:
//   • google — the Admin UI login toggle (or the AUTH_GOOGLE_ENABLED pin) AND
//     a resolvable client (Admin UI record or env);
//   • password — at least one DB-backed account exists (Admin → People);
//   • claimable — zero admins: the login screen offers /claim instead.

use crate::claim::instance_claimable;
use crate::error::thrown_internal_error;
use crate::google_client::google_login_enabled;
use crate::password_accounts::has_password_accounts;
use crate::state::AppState;
use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};

#[derive(serde::Serialize)]
struct ProviderMeta {
    id: &'static str,
    label: &'static str,
    kind: &'static str,
}

#[derive(serde::Serialize)]
struct ProvidersBody {
    providers: Vec<ProviderMeta>,
    claimable: bool,
    configured: bool,
}

pub async fn get(State(state): State<AppState>) -> Response {
    let sb = state.secretbox().await.unwrap_or_default();
    // Not error-mapped: a store outage 500s through the probes below, and a
    // settings row that cannot be read is the toggle-off it falls back to.
    let google = google_login_enabled(&state.pg, &sb).await;
    let password = match has_password_accounts(&state.pg).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[auth/providers] account probe failed: {e}");
            return thrown_internal_error();
        }
    };
    let claimable = match instance_claimable(&state.pg).await {
        Ok(v) => v,
        Err(e) => {
            tracing::error!("[auth/providers] claim probe failed: {e}");
            return thrown_internal_error();
        }
    };
    let mut providers = Vec::new();
    if google {
        providers.push(ProviderMeta {
            id: "google",
            label: "Continue with Google",
            kind: "oauth",
        });
    }
    if password {
        providers.push(ProviderMeta {
            id: "password",
            label: "Username & password",
            kind: "password",
        });
    }
    // Surfaced so the login screen can warn instead of silently failing. TS
    // computes it from env presence; here the process refuses to boot without
    // both, so the answer is always yes — an unreachable store fails loudly
    // above instead of rendering a login screen that cannot work.
    Json(ProvidersBody {
        providers,
        claimable,
        configured: true,
    })
    .into_response()
}
