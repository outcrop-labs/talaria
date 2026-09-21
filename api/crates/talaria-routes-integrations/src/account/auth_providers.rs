// GET /api/auth/providers. The login screen asks one question: what doors
// exist on this instance? Answered live on every call, so flipping a toggle
// or creating the first password account changes the screen without a
// restart:
//   • google — the Admin UI login toggle (or the AUTH_GOOGLE_ENABLED pin) AND
//     a resolvable client (Admin UI record or env);
//   • password — at least one DB-backed account exists (Admin → People);
//   • claimable — zero admins: the login screen offers /claim instead.

use axum::Json;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use talaria_api_facades::google::client::google_login_enabled;
use talaria_claim::instance_claimable;
use talaria_error::internal;
use talaria_password_accounts::has_password_accounts;
use talaria_state::AppState;

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
        Err(e) => return internal("[auth/providers] account probe failed", e),
    };
    let claimable = match instance_claimable(&state.pg).await {
        Ok(v) => v,
        Err(e) => return internal("[auth/providers] claim probe failed", e),
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
    // Surfaced so the login screen can warn instead of silently failing. The
    // process refuses to boot without both, so the answer is always yes — an
    // unreachable store fails loudly above instead of rendering a login
    // screen that cannot work.
    Json(ProvidersBody {
        providers,
        claimable,
        configured: true,
    })
    .into_response()
}
