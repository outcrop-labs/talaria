// /api/artifacts/{id}/export/google — port of
// ui/src/routes/api/artifacts.$id.export.google.ts. Mirror an artifact into
// Google Drive.
//
// Whose Drive it lands in depends on the caller (per-user OAuth):
//   human            → their own connected Drive
//   personal agent   → its OWNER's Drive (acts as the human it works for)
//   general agent    → the shared ORG Drive (no human owner of its own)

use axum::Json;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;

use crate::agent_auth::{AgentSubject, agent_caller, refuse_legacy};
use crate::artifacts::{get_artifact, guarded, record_google_export};
use crate::audit::{AuditEntry, log_audit};
use crate::error::{house_error, house_error_msg, thrown_internal_error};
use crate::google_agent::resolve_agent_google;
use crate::google_connections::get_connection_status;
use crate::google_drive::{ExportError, export_artifact_to_drive, export_artifact_with_token};
use crate::google_org::get_org_targets;
use crate::kb_perms::{ITEM_ARTIFACT, can_read, can_read_agent, list_editors};
use crate::session::{actor_of, require_user, who_of};
use crate::state::AppState;

/// The catch block's three friendly answers. `Failed` also logs — the only
/// consumer of Google's raw sentence is the log (TS logs it in dev).
fn export_failed(e: ExportError) -> Response {
    match e {
        ExportError::NotConnected => {
            // connections.ts requireToken's GoogleNotConnected throw.
            house_error_msg(
                StatusCode::CONFLICT,
                "not_connected",
                "Connect a Google account first.",
            )
        }
        ExportError::NotExportable => {
            // 400, not 422: the artifact's kind is the client's own choice, so
            // an unexportable one is a malformed request for THIS endpoint —
            // the repo's contract is 400 for shape problems, 422 never
            // appears.
            house_error_msg(
                StatusCode::BAD_REQUEST,
                "not_exportable",
                "This artifact can’t be exported to Google Drive.",
            )
        }
        ExportError::Failed(msg) => {
            tracing::error!("[artifacts/export/google] failed: {msg}");
            house_error_msg(
                StatusCode::BAD_GATEWAY,
                "export_failed",
                "Google Drive rejected the export.",
            )
        }
    }
}

pub async fn post(
    State(state): State<AppState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    let artifact = match get_artifact(&state.pg, &id).await {
        Ok(a) => a,
        Err(e) => {
            tracing::error!("[artifacts] read failed: {e}");
            return thrown_internal_error();
        }
    };
    let Some(artifact) = artifact else {
        return house_error(StatusCode::NOT_FOUND, "not found");
    };
    let editors = match list_editors(&state.pg, ITEM_ARTIFACT, &artifact.id).await {
        Ok(e) => e,
        Err(e) => {
            tracing::error!("[artifacts] grants read failed: {e}");
            return thrown_internal_error();
        }
    };
    let sb = state.secretbox().await.unwrap_or_default();

    let agent = match agent_caller(&state.pg, &headers).await {
        Ok(c) => c,
        Err(resp) => return resp,
    };
    let file = if let Some(agent) = agent {
        // Writing into a human's Drive (personal assistant) or the shared
        // ORG Drive is acting AS someone — the same grant the Gmail and
        // Calendar agent routes refuse to a legacy caller, and this one
        // must too. The shared org key proves fleet membership, not
        // identity, so "I am <any ordinary agent>" cannot buy org Drive
        // write access.
        if let Some(denied) = refuse_legacy(&agent, "Google Drive export") {
            return denied;
        }
        let name = agent.model.clone();
        if !can_read_agent(&guarded(&artifact), &name, &editors) {
            return house_error(StatusCode::FORBIDDEN, "forbidden");
        }
        // Resolve the agent's Google identity (owner for personal assistants,
        // shared org account for general fleet agents). Pass the CALLER —
        // resolve_agent_google re-checks proof, so this route's guard is a
        // layer rather than the only one.
        let Some(google) =
            resolve_agent_google(&state.pg, &sb, &AgentSubject::Caller(agent), now_ms()).await
        else {
            return house_error_msg(
                StatusCode::CONFLICT,
                "not_connected",
                "No Google account is connected for this agent (its owner, or the org account).",
            );
        };
        // Org files go to the configured Shared Drive/folder (team-owned).
        let folder_id = if google.principal == "org" {
            match get_org_targets(&state.pg).await {
                Ok(t) => t.drive_folder_id,
                Err(e) => {
                    tracing::error!("[artifacts/export/google] org targets read failed: {e}");
                    return export_failed(ExportError::Failed(e.to_string()));
                }
            }
        } else {
            None
        };
        match export_artifact_with_token(
            &state.pg,
            &sb,
            &google.token,
            &artifact,
            folder_id.as_deref(),
        )
        .await
        {
            Ok(f) => f,
            Err(e) => return export_failed(e),
        }
    } else {
        let user = match require_user(&state, &headers).await {
            Ok(u) => u,
            Err(gate) => return gate,
        };
        if !can_read(
            &guarded(&artifact),
            Some(&user.id),
            who_of(&user).as_deref(),
            &editors,
        ) {
            return house_error(StatusCode::FORBIDDEN, "forbidden");
        }
        let connected = match get_connection_status(&state.pg, &user.id).await {
            Ok(c) => c,
            Err(e) => {
                // TS's isConnected throw lands in the catch → 502.
                return export_failed(ExportError::Failed(format!("connection status read: {e}")));
            }
        };
        if !connected.connected {
            return house_error_msg(
                StatusCode::CONFLICT,
                "not_connected",
                "Connect a Google account first (Settings → Integrations).",
            );
        }
        let file =
            match export_artifact_to_drive(&state.pg, &sb, &user.id, &artifact, now_ms()).await {
                Ok(f) => f,
                Err(e) => return export_failed(e),
            };
        let (pg, audit_actor, target_id, target_label) = (
            state.pg.clone(),
            actor_of(&user),
            id.clone(),
            artifact.title.clone(),
        );
        tokio::spawn(async move {
            log_audit(
                &pg,
                AuditEntry {
                    actor: &audit_actor,
                    action: "artifact.export_google",
                    target_type: "artifact",
                    target_id: Some(&target_id),
                    target_label: Some(&target_label),
                    before: None,
                    after: None,
                },
            )
            .await;
        });
        file
    };

    if let Err(e) = record_google_export(&state.pg, &artifact.id, &file.id, &file.url).await {
        // TS's throw lands in the catch → 502.
        return export_failed(ExportError::Failed(format!("export record: {e}")));
    }
    Json(json!({ "file": file })).into_response()
}

/// Date.now() — the one clock the TS surface reads. Centralized so tests
/// could pin it if this route ever grows one.
fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}
