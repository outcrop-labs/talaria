// Shared finish for agent drafts (email and calendar). A queued row is not
// a success until the approver's confirm-sends queue — the same read the
// Inbox uses — contains it. An insert that the owner cannot see is an error,
// and the row we just wrote is discarded so a retry is not a silent orphan.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::json;
use sqlx::PgPool;

use talaria_api_facades::google::agent::AgentPrincipal;
use talaria_api_facades::google::pending_actions::{
    QueuedAction, discard_unproven, queue_proof_failure,
};
use talaria_error::{house_error, internal};

/// The person in the live conversation, when one is streaming. None when the
/// agent is not mid-turn — a cron, a finished turn — which is not a mismatch.
pub async fn streaming_conversation_user(
    pg: &PgPool,
    agent_model: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        "select c.user_id::text \
         from messages m join conversations c on c.id = m.conversation_id \
         where c.agent_model = $1 and m.role = 'assistant' and m.status = 'streaming' \
           and m.streamed_at > now() - interval '15 minutes' \
         order by m.streamed_at desc limit 1",
    )
    .bind(agent_model)
    .fetch_optional(pg)
    .await
}

/// A personal draft queued for a different user than the one in this
/// conversation would succeed in the API and stay invisible to the person
/// who asked. Refuse before the insert.
pub fn conversation_owner_conflict(
    principal: &AgentPrincipal,
    conversation_user: Option<&str>,
) -> Option<String> {
    if principal.is_org {
        return None;
    }
    let talking_to = conversation_user?;

    match principal.owner_user_id.as_deref() {
        Some(owner) if owner == talking_to => None,
        Some(owner) => Some(format!(
            "this conversation belongs to {talking_to}, but the assistant is owned by {owner} — the draft was not queued, because it would not appear in the confirm-sends queue the person in this conversation sees"
        )),
        None => Some(
            "this assistant has no owner — a personal confirm-send with no owner is visible to nobody, so the draft was not queued"
                .into(),
        ),
    }
}

/// The response a draft route may return after `queue_action`. Success only
/// when the approver's queue contains the row. A fresh insert that fails
/// that proof is deleted; a dedupe hit is left for whoever can already see it.
pub async fn answer_queued(
    pg: &PgPool,
    queued: QueuedAction,
    already: &str,
    waiting_owner: &str,
    waiting_admin: &str,
) -> Response {
    match queue_proof_failure(pg, &queued.action).await {
        Ok(None) => {}
        Ok(Some(reason)) => {
            if !queued.already_pending
                && let Err(e) = discard_unproven(pg, &queued.action.id).await
            {
                return internal(
                    "[integrations/google/agent] unproven draft discard failed",
                    e,
                );
            }
            return house_error(StatusCode::CONFLICT, &reason);
        }
        Err(e) => {
            return internal("[integrations/google/agent] confirm-sends proof failed", e);
        }
    }
    let message = if queued.already_pending {
        already
    } else if queued.action.is_org {
        waiting_admin
    } else {
        waiting_owner
    };
    let message =
        format!("{message} Confirm with list_pending_sends before telling anyone it is ready.");
    Json(json!({
        "pending": {
            "id": queued.action.id,
            "status": queued.action.status,
            "ownerUserId": queued.action.owner_user_id,
            "isOrg": queued.action.is_org,
        },
        "approver": {
            "userId": queued.action.owner_user_id,
            "queue": if queued.action.is_org { "admin" } else { "owner" },
        },
        "message": message,
    }))
    .into_response()
}

/// An unlocked draft executes immediately. "Sent" is only honest when the
/// decide path marked the row executed — a failed Google call used to ride
/// out under the same sentence.
pub fn answer_executed(id: &str, outcome_status: &str, sent: &str, failed: &str) -> Response {
    if outcome_status == "executed" {
        return Json(json!({
            "pending": { "id": id, "status": "executed" },
            "message": sent,
        }))
        .into_response();
    }
    house_error(
        StatusCode::BAD_GATEWAY,
        &format!("{failed} ({outcome_status})"),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use talaria_api_facades::google::agent::AgentPrincipal;

    fn personal(owner: Option<&str>) -> AgentPrincipal {
        AgentPrincipal {
            is_org: false,
            owner_user_id: owner.map(str::to_string),
        }
    }

    #[test]
    fn a_conversation_with_someone_else_is_not_queued() {
        let reason = conversation_owner_conflict(&personal(Some("owner-jon")), Some("other-user"))
            .expect("mismatch");
        assert!(reason.contains("was not queued"));
        assert!(reason.contains("other-user"));
        assert!(reason.contains("owner-jon"));
    }

    #[test]
    fn the_owner_in_the_conversation_is_not_a_mismatch() {
        assert!(
            conversation_owner_conflict(&personal(Some("owner-jon")), Some("owner-jon")).is_none()
        );
        assert!(conversation_owner_conflict(&personal(Some("owner-jon")), None).is_none());
        let org = AgentPrincipal {
            is_org: true,
            owner_user_id: None,
        };
        assert!(conversation_owner_conflict(&org, Some("anyone")).is_none());
    }
}
