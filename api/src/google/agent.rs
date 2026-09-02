// Resolve which Google identity an agent acts as.
//
//   personal assistant (agent_defs.owner_user_id set) → acts as its OWNER
//   general fleet agent (no owner)                     → acts as the shared ORG
//
// agent_defs.model is unique and is exactly what an agent presents over MCP as
// x-agent-name, so a single lookup binds the caller to a Google connection.

use crate::agent_auth::{AgentSubject, subject_model, subject_proven};
use crate::google::connections::get_access_token;
use crate::google::org::get_org_access_token;
use crate::secretbox::SecretBox;
use sqlx::PgPool;

/// A resolved Google identity: token + whose account it is.
pub struct AgentGoogle {
    pub token: String,
    /// Whose Drive the agent is acting in.
    pub principal: &'static str, // "owner" | "org"
    pub owner_user_id: Option<String>,
}

/// A Google access token for the calling agent, or None when the relevant
/// connection isn't set up (owner hasn't connected / org account not
/// configured). A personal assistant NEVER falls back to the org account — it
/// acts strictly as its owner, so it can't silently write into the shared
/// Drive.
///
/// Takes the CALLER, not a bare name: handing out an OAuth token is the single
/// largest grant on the agent surface (the owner's mailbox, the org Drive), so
/// the proof check lives HERE rather than only in each route's refuse_legacy.
/// The routes still guard — that reply names the container to roll — but a new
/// caller that forgets to gets null instead of the org's token.
pub async fn resolve_agent_google(
    pg: &PgPool,
    sb: &SecretBox,
    agent: &AgentSubject,
    now_ms: i64,
) -> Option<AgentGoogle> {
    // A legacy shared-key caller only ASSERTS which agent it is: it proved
    // fleet membership, not identity, so it never reaches a human's or the
    // org's Google account.
    if !subject_proven(agent) {
        return None;
    }
    let agent_model = subject_model(agent);
    let def: Option<(Option<String>,)> =
        sqlx::query_as("select owner_user_id::text from agent_defs where model = $1 limit 1")
            .bind(agent_model)
            .fetch_optional(pg)
            .await
            .ok()
            .flatten();

    if let Some((Some(owner_user_id),)) = def {
        // Personal assistant: strictly its owner's identity. A vending error
        // nulls — a dead owner connection is "not set up", not a 500.
        return match get_access_token(pg, sb, &owner_user_id, now_ms).await {
            Ok(Some(token)) => Some(AgentGoogle {
                token,
                principal: "owner",
                owner_user_id: Some(owner_user_id),
            }),
            _ => None,
        };
    }

    // General fleet agent (or unknown): the shared org account.
    match get_org_access_token(pg, sb, now_ms).await {
        Ok(Some(token)) => Some(AgentGoogle {
            token,
            principal: "org",
            owner_user_id: None,
        }),
        _ => None,
    }
}

/// The Talaria user an agent is the personal assistant OF, or None for a
/// general fleet agent. Calendar/Gmail acting-as is owner-only — general
/// agents don't get to read/send a human's mail or calendar.
pub async fn resolve_agent_owner_user(
    pg: &PgPool,
    agent_model: &str,
) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("select owner_user_id::text from agent_defs where model = $1 limit 1")
            .bind(agent_model)
            .fetch_optional(pg)
            .await?;
    Ok(row.and_then(|(owner,)| owner))
}

/// Who an agent drafts/acts FOR — without needing a live token (used for
/// queuing a pending action). Personal assistant → its owner; general agent →
/// the org.
pub struct AgentPrincipal {
    /// true → the shared org account; false → the owner_user_id's account.
    pub is_org: bool,
    pub owner_user_id: Option<String>,
}

pub async fn resolve_agent_principal(
    pg: &PgPool,
    agent_model: &str,
) -> Result<AgentPrincipal, sqlx::Error> {
    let owner = resolve_agent_owner_user(pg, agent_model).await?;
    Ok(match owner {
        Some(owner_user_id) => AgentPrincipal {
            is_org: false,
            owner_user_id: Some(owner_user_id),
        },
        None => AgentPrincipal {
            is_org: true,
            owner_user_id: None,
        },
    })
}
