// /api/mcp/oauth/callback.
// The OAuth redirect target. No session requirement — identity was bound to
// the state row when the flow started; the state is single-use and expiring.

use crate::audit::{AuditEntry, log_audit};
use crate::mcp::apply::{roll_agent_for_user, roll_agents_for_server};
use crate::mcp::oauth::handle_oauth_callback;
use crate::state::AppState;
use axum::extract::{Query, State};
use axum::http::{StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde::Deserialize;

/// Both interpolations carry attacker-reachable text (the provider's
/// error_description, a thrown message), and this page is unauthenticated by
/// design and built to be opened from the app — so escape them, and pin a CSP
/// that allows nothing but the inline script/style this page is built from.
fn esc(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&#39;")
}

fn page(title: &str, body: &str) -> Response {
    let html = format!(
        concat!(
            "<!doctype html><meta charset=\"utf-8\"><title>{title}</title>\n",
            "<body style=\"font-family:ui-monospace,monospace;background:#0b0c0e;color:#e6e6e6;display:grid;place-items:center;height:100vh;margin:0\">\n",
            "<div style=\"text-align:center\"><p style=\"font-size:15px\">{body}</p>\n",
            "<p style=\"font-size:12px;opacity:.6\">This window closes itself.</p></div>\n",
            "<script>\n",
            "try {{ window.opener && window.opener.postMessage({{ type: 'talaria:mcp-oauth-done' }}, window.location.origin) }} catch {{}}\n",
            "setTimeout(()=>{{ if (window.opener) window.close(); else location.href='/mcp' }}, 1200)\n",
            "</script></body>"
        ),
        title = esc(title),
        body = esc(body),
    );
    (
        StatusCode::OK,
        [
            (header::CONTENT_TYPE, "text/html; charset=utf-8".to_string()),
            // 'unsafe-inline' covers the postMessage/close script and the inline
            // styles; everything else — remote script, fetch, frames — is denied.
            (
                header::CONTENT_SECURITY_POLICY,
                "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; base-uri 'none'; form-action 'none'".to_string(),
            ),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff".to_string()),
            (header::REFERRER_POLICY, "no-referrer".to_string()),
        ],
        html,
    )
        .into_response()
}

#[derive(Deserialize)]
pub struct CallbackQuery {
    error: Option<String>,
    error_description: Option<String>,
    state: Option<String>,
    code: Option<String>,
}

pub async fn get(State(state): State<AppState>, Query(q): Query<CallbackQuery>) -> Response {
    if let Some(err) = &q.error {
        return page(
            "Connection failed",
            &format!(
                "The provider said: {}",
                q.error_description.as_deref().unwrap_or(err)
            ),
        );
    }
    let (Some(state_param), Some(code)) = (q.state.as_deref(), q.code.as_deref()) else {
        return page("Connection failed", "Missing code or state.");
    };
    let sb = match state.secretbox().await {
        Ok(sb) => sb,
        Err(e) => return page("Connection failed", &e),
    };
    let handled = handle_oauth_callback(&state.pg, &sb, state_param, code).await;
    let (subject, server_id) = match handled {
        Ok(pair) => pair,
        Err(e) => return page("Connection failed", &e),
    };
    log_audit(
        &state.pg,
        AuditEntry {
            actor: if subject == "org" { "org" } else { &subject },
            action: "mcp.oauth_connect",
            target_type: "mcp-server",
            target_id: Some(&server_id),
            target_label: None,
            before: None,
            after: None,
        },
    )
    .await;
    let (pg, sb) = (state.pg.clone(), sb.clone());
    let subject = subject.clone();
    let server_id = server_id.clone();
    tokio::spawn(async move {
        // Connected servers appear in configs…
        let _ = crate::fleet::render::render_fleet(&pg, &sb, None).await;
        // Running agents wire MCP at start — roll the affected ones so the
        // connection is usable without anyone bouncing containers.
        if subject == "org" {
            roll_agents_for_server(&pg, &sb, &server_id).await;
        } else {
            roll_agent_for_user(&pg, &sb, &subject).await;
        }
    });
    page(
        "Connected",
        "Connected — your agents are picking it up now (a graceful restart runs behind the scenes).",
    )
}
