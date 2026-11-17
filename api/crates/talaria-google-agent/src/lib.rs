// Resolve which Google identity an agent acts as.
//
// The registry (`agent_principals`) is the source of truth once a row exists:
//   owner → that user's Google account, never the org account
//   org   → the shared org account
//   agent → the agent's own Google connection
//
// A def with no registry row still follows the legacy column: owner_user_id
// set means owner, otherwise org. After the admin surface writes a row, the
// column is advisory.

use sqlx::PgPool;
use talaria_agent_auth::{AgentSubject, epoch_ms_to_iso, subject_model, subject_proven};
use talaria_google_connections::{
    GrantRow, TokenError, get_access_token, get_connection_status, vend_grant,
};
use talaria_google_org::get_org_access_token;
use talaria_secretbox::SecretBox;

/// Whose Google account an agent acts with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrincipalKind {
    Owner,
    Org,
    Agent,
}

impl PrincipalKind {
    pub fn parse(raw: &str) -> Option<Self> {
        match raw {
            "owner" => Some(Self::Owner),
            "org" => Some(Self::Org),
            "agent" => Some(Self::Agent),
            _ => None,
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Owner => "owner",
            Self::Org => "org",
            Self::Agent => "agent",
        }
    }

    /// An admin approves org actions and per-agent identities. A personal
    /// assistant's owner approves theirs.
    pub fn admin_approves(self) -> bool {
        matches!(self, Self::Org | Self::Agent)
    }
}

/// A resolved Google identity: token + whose account it is.
pub struct AgentGoogle {
    pub token: String,
    /// Whose Drive the agent is acting in: "owner" | "org" | "agent".
    pub principal: &'static str,
    pub owner_user_id: Option<String>,
}

/// Who an agent drafts/acts FOR — without needing a live token (used for
/// queuing a pending action).
///
/// `is_org` means an admin approves, not "use the org token". A per-agent
/// identity is admin-approved and executed with the agent's own token.
pub struct AgentPrincipal {
    pub kind: PrincipalKind,
    pub is_org: bool,
    pub owner_user_id: Option<String>,
}

impl AgentPrincipal {
    pub fn from_kind(kind: PrincipalKind, user_id: Option<String>) -> Self {
        let owner_user_id = match kind {
            PrincipalKind::Owner => user_id,
            PrincipalKind::Org | PrincipalKind::Agent => None,
        };
        Self {
            kind,
            is_org: kind.admin_approves(),
            owner_user_id,
        }
    }
}

/// Named fix for an outbound write that would otherwise fall through to the
/// org account. The sentence is the whole refusal — no code to decode.
pub const OWNER_WRITE_REFUSAL: &str = "This agent acts as its owner, and that owner has not connected Google. Connect it in Settings → Connections — this agent will not use the shared org account.";

pub const AGENT_WRITE_REFUSAL: &str = "This agent is set to its own Google account, and that account is not connected. An admin can connect it on the agent record, or set the principal back to the org account.";

/// `None` means the write may queue. Owner-without-Google and
/// agent-without-connection refuse. Org does not: a missing org connection
/// still fails at approval, which is the cutover behavior.
pub fn write_refusal(principal: &AgentPrincipal, connected: bool) -> Option<&'static str> {
    match principal.kind {
        PrincipalKind::Owner if principal.owner_user_id.is_none() || !connected => {
            Some(OWNER_WRITE_REFUSAL)
        }
        PrincipalKind::Agent if !connected => Some(AGENT_WRITE_REFUSAL),
        _ => None,
    }
}

/// Legacy column → principal. Owner column wins; otherwise the org account.
pub fn principal_from_legacy(owner_user_id: Option<String>) -> AgentPrincipal {
    match owner_user_id {
        Some(id) => AgentPrincipal::from_kind(PrincipalKind::Owner, Some(id)),
        None => AgentPrincipal::from_kind(PrincipalKind::Org, None),
    }
}

/// Registry row → principal. An unknown kind is not guessed.
pub fn principal_from_registry(kind: &str, user_id: Option<String>) -> Option<AgentPrincipal> {
    let kind = PrincipalKind::parse(kind)?;
    Some(AgentPrincipal::from_kind(kind, user_id))
}

/// The Talaria user an agent is the personal assistant OF, or None for a
/// general fleet agent or a per-agent identity. Calendar/Gmail acting-as is
/// owner-only — general agents don't get to read/send a human's mail.
pub async fn resolve_agent_owner_user(
    pg: &PgPool,
    agent_model: &str,
) -> Result<Option<String>, sqlx::Error> {
    Ok(load_principal(pg, agent_model).await?.owner_user_id)
}

pub async fn resolve_agent_principal(
    pg: &PgPool,
    agent_model: &str,
) -> Result<AgentPrincipal, sqlx::Error> {
    load_principal(pg, agent_model).await
}

/// A Google access token for the calling agent. `Ok(None)` when the relevant
/// connection isn't set up. A personal assistant NEVER falls back to the org
/// account. A registry read that fails also answers `Ok(None)` — handing out
/// the org token because a lookup blipped is the silent fallback this refuses.
///
/// `Err` is the token itself failing — Google refusing the grant or the
/// client, or the refresh not getting through. It is NOT folded into "not
/// connected": the caller answers it through `google_token_fail`, which
/// quotes what Google said. (It used to be `.unwrap_or_default()`, so an
/// `invalid_client` read to the agent as "No Google account is connected".)
///
/// Takes the CALLER, not a bare name: handing out an OAuth token is the single
/// largest grant on the agent surface, so the proof check lives HERE.
pub async fn resolve_agent_google(
    pg: &PgPool,
    sb: &SecretBox,
    agent: &AgentSubject,
    now_ms: i64,
) -> Result<Option<AgentGoogle>, TokenError> {
    if !subject_proven(agent) {
        return Ok(None);
    }
    let agent_model = subject_model(agent);
    let Ok(principal) = load_principal(pg, agent_model).await else {
        return Ok(None);
    };
    let token = match principal.kind {
        PrincipalKind::Owner => match principal.owner_user_id.as_deref() {
            Some(user_id) => get_access_token(pg, sb, user_id, now_ms).await?,
            None => None,
        },
        PrincipalKind::Org => get_org_access_token(pg, sb, now_ms).await?,
        PrincipalKind::Agent => get_agent_access_token(pg, sb, agent_model, now_ms).await?,
    };
    Ok(token.map(|token| AgentGoogle {
        token,
        principal: principal.kind.as_str(),
        owner_user_id: principal.owner_user_id,
    }))
}

/// Why an outbound write must not queue. `Ok(None)` means the principal's
/// Google identity is connected (or it is the org account, whose missing
/// connection still surfaces at approval).
pub async fn outbound_write_refusal(
    pg: &PgPool,
    agent_model: &str,
) -> Result<Option<&'static str>, sqlx::Error> {
    let principal = load_principal(pg, agent_model).await?;
    let connected = match principal.kind {
        PrincipalKind::Owner => match principal.owner_user_id.as_deref() {
            Some(user_id) => get_connection_status(pg, user_id).await?.connected,
            None => false,
        },
        PrincipalKind::Agent => agent_connection_present(pg, agent_model).await?,
        PrincipalKind::Org => true,
    };
    Ok(write_refusal(&principal, connected))
}

async fn load_principal(pg: &PgPool, agent_model: &str) -> Result<AgentPrincipal, sqlx::Error> {
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        "select principal_kind, principal_user_id::text \
         from agent_principals where agent_model = $1",
    )
    .bind(agent_model)
    .fetch_optional(pg)
    .await?;
    if let Some((kind, user_id)) = row
        && let Some(principal) = principal_from_registry(&kind, user_id)
    {
        return Ok(principal);
    }
    Ok(principal_from_legacy(legacy_owner(pg, agent_model).await?))
}

async fn legacy_owner(pg: &PgPool, agent_model: &str) -> Result<Option<String>, sqlx::Error> {
    let row: Option<(Option<String>,)> =
        sqlx::query_as("select owner_user_id::text from agent_defs where model = $1 limit 1")
            .bind(agent_model)
            .fetch_optional(pg)
            .await?;
    Ok(row.and_then(|(owner,)| owner))
}

/// What the admin record shows. `connected` is the identity this kind
/// actually uses — the owner's connection, the agent's, or (for org) not
/// this row's concern. `client_id` and `flagged` describe that same
/// connection: the OAuth client that minted its grant, and Google's words
/// from the last refusal of that client.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentGoogleIdentity {
    pub kind: PrincipalKind,
    pub principal_user_id: Option<String>,
    pub connected: bool,
    pub email: Option<String>,
    pub legacy_owner_user_id: Option<String>,
    pub client_id: Option<String>,
    pub flagged: Option<String>,
}

pub async fn agent_google_identity(
    pg: &PgPool,
    agent_model: &str,
) -> Result<AgentGoogleIdentity, sqlx::Error> {
    let principal = load_principal(pg, agent_model).await?;
    let legacy_owner_user_id = legacy_owner(pg, agent_model).await?;
    let face = match principal.kind {
        PrincipalKind::Owner => match principal.owner_user_id.as_deref() {
            Some(user_id) => {
                let status = get_connection_status(pg, user_id).await?;
                ConnectionFace {
                    connected: status.connected,
                    email: status.email,
                    client_id: status.client_id,
                    flagged: status.flagged,
                }
            }
            None => ConnectionFace::default(),
        },
        PrincipalKind::Agent => agent_connection_face(pg, agent_model).await?,
        PrincipalKind::Org => ConnectionFace::default(),
    };
    Ok(AgentGoogleIdentity {
        kind: principal.kind,
        principal_user_id: principal.owner_user_id,
        connected: face.connected,
        email: face.email,
        legacy_owner_user_id,
        client_id: face.client_id,
        flagged: face.flagged,
    })
}

/// Replace the registry row. The caller has already validated the kind and
/// that an owner id, when required, names a user.
pub async fn upsert_principal(
    pg: &PgPool,
    agent_model: &str,
    kind: PrincipalKind,
    user_id: Option<&str>,
) -> Result<(), sqlx::Error> {
    let user_id = match kind {
        PrincipalKind::Owner => user_id,
        PrincipalKind::Org | PrincipalKind::Agent => None,
    };
    sqlx::query(
        "insert into agent_principals (agent_model, principal_kind, principal_user_id, updated_at) \
         values ($1, $2, $3::uuid, now()) \
         on conflict (agent_model) do update set \
           principal_kind = excluded.principal_kind, \
           principal_user_id = excluded.principal_user_id, \
           updated_at = now()",
    )
    .bind(agent_model)
    .bind(kind.as_str())
    .bind(user_id)
    .execute(pg)
    .await?;
    Ok(())
}

/// Drop the agent's own Google connection, revoking its grant at Google
/// unless another stored connection holds the same grant — an identity
/// connected with its manager's own account shares the manager's grant, and
/// revoking it would cut the manager off. If the principal was that
/// connection, restore the seeded default (owner column, else org) — an
/// explicit admin action, not a silent fallback on a write.
pub async fn disconnect_agent_google(
    pg: &PgPool,
    sb: &SecretBox,
    agent_model: &str,
) -> Result<(AgentPrincipal, talaria_google_connections::RevokeOutcome), sqlx::Error> {
    use talaria_google_connections::{GrantRow, forget_grant};
    let outcome = forget_grant(pg, sb, &GrantRow::Agent(agent_model.to_string())).await?;
    let current = load_principal(pg, agent_model).await?;
    if current.kind != PrincipalKind::Agent {
        return Ok((current, outcome));
    }
    let restored = principal_from_legacy(legacy_owner(pg, agent_model).await?);
    upsert_principal(
        pg,
        agent_model,
        restored.kind,
        restored.owner_user_id.as_deref(),
    )
    .await?;
    Ok((restored, outcome))
}

pub async fn remember_agent_oauth_state(
    pg: &PgPool,
    state: &str,
    agent_model: &str,
) -> Result<(), sqlx::Error> {
    sqlx::query(
        "delete from agent_google_oauth_states where created_at < now() - interval '30 minutes'",
    )
    .execute(pg)
    .await?;
    sqlx::query(
        "insert into agent_google_oauth_states (state, agent_model) \
         values ($1, $2) \
         on conflict (state) do update set agent_model = excluded.agent_model, created_at = now()",
    )
    .bind(state)
    .bind(agent_model)
    .execute(pg)
    .await?;
    Ok(())
}

pub async fn take_agent_oauth_state(
    pg: &PgPool,
    state: &str,
) -> Result<Option<String>, sqlx::Error> {
    sqlx::query_scalar(
        "delete from agent_google_oauth_states where state = $1 returning agent_model",
    )
    .bind(state)
    .fetch_optional(pg)
    .await
}

/// What an OAuth exchange hands the agent-connection saver.
pub struct SaveAgentConnection<'a> {
    pub google_sub: &'a str,
    pub email: Option<&'a str>,
    pub scope: &'a str,
    pub refresh_token: Option<&'a str>,
    pub access_token: Option<&'a str>,
    pub expires_in_seconds: Option<i64>,
    pub connected_by: Option<&'a str>,
    /// The OAuth client the exchange ran on (`SaveConnection`'s).
    pub oauth_client_id: &'a str,
    pub now_ms: i64,
}

/// Store an agent's own Google connection. A missing refresh token keeps the
/// stored one and the client that minted it, as the per-user save does.
pub async fn save_agent_connection(
    pg: &PgPool,
    sb: &SecretBox,
    agent_model: &str,
    input: &SaveAgentConnection<'_>,
) -> Result<(), String> {
    let seal_opt = |v: Option<&str>| -> Result<Option<String>, String> {
        match v {
            Some(v) => Ok(Some(
                sb.seal(v)
                    .map_err(|e| format!("agent google token seal: {e}"))?,
            )),
            None => Ok(None),
        }
    };
    let refresh_enc = seal_opt(input.refresh_token)?;
    let access_enc = seal_opt(input.access_token)?;
    let expires_at = match (input.access_token, input.expires_in_seconds) {
        (Some(t), Some(secs)) if !t.is_empty() && secs != 0 => {
            Some(epoch_ms_to_iso(input.now_ms + secs * 1000))
        }
        _ => None,
    };
    sqlx::query(
        "insert into agent_google_connections \
             (agent_model, google_sub, email, scope, refresh_token_enc, access_token_enc, access_expires_at, connected_by, oauth_client_id, updated_at) \
         values ($1, $2, $3, $4, $5, $6, $7::timestamptz, $8::uuid, $9, now()) \
         on conflict (agent_model) do update set \
             google_sub = excluded.google_sub, \
             email = excluded.email, \
             scope = excluded.scope, \
             refresh_token_enc = coalesce(excluded.refresh_token_enc, agent_google_connections.refresh_token_enc), \
             oauth_client_id = case when excluded.refresh_token_enc is null \
                 then agent_google_connections.oauth_client_id else excluded.oauth_client_id end, \
             reconnect_reason = case when excluded.refresh_token_enc is null \
                 then agent_google_connections.reconnect_reason end, \
             reconnect_flagged_at = case when excluded.refresh_token_enc is null \
                 then agent_google_connections.reconnect_flagged_at end, \
             access_token_enc = excluded.access_token_enc, \
             access_expires_at = excluded.access_expires_at, \
             connected_by = excluded.connected_by, \
             updated_at = now()",
    )
    .bind(agent_model)
    .bind(input.google_sub)
    .bind(input.email)
    .bind(input.scope)
    .bind(&refresh_enc)
    .bind(&access_enc)
    .bind(&expires_at)
    .bind(input.connected_by)
    .bind(input.oauth_client_id)
    .execute(pg)
    .await
    .map_err(|e| format!("agent google connection save: {e}"))?;
    Ok(())
}

/// A valid access token for the agent's own Google identity
/// (`vend_grant` on its row): refreshed with the client that minted it.
/// `Ok(None)` ⇒ not connected.
pub async fn get_agent_access_token(
    pg: &PgPool,
    sb: &SecretBox,
    agent_model: &str,
    now_ms: i64,
) -> Result<Option<String>, TokenError> {
    Ok(
        vend_grant(pg, sb, &GrantRow::Agent(agent_model.to_string()), now_ms)
            .await?
            .map(|v| v.token),
    )
}

async fn agent_connection_present(pg: &PgPool, agent_model: &str) -> Result<bool, sqlx::Error> {
    let row: Option<(Option<String>,)> = sqlx::query_as(
        "select refresh_token_enc from agent_google_connections where agent_model = $1",
    )
    .bind(agent_model)
    .fetch_optional(pg)
    .await?;
    Ok(row
        .and_then(|(enc,)| enc)
        .is_some_and(|enc| !enc.is_empty()))
}

/// One connection as the agent record shows it.
#[derive(Default)]
struct ConnectionFace {
    connected: bool,
    email: Option<String>,
    client_id: Option<String>,
    flagged: Option<String>,
}

async fn agent_connection_face(
    pg: &PgPool,
    agent_model: &str,
) -> Result<ConnectionFace, sqlx::Error> {
    type FaceRow = (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let row: Option<FaceRow> = sqlx::query_as(
        "select email, refresh_token_enc, oauth_client_id, reconnect_reason \
         from agent_google_connections where agent_model = $1",
    )
    .bind(agent_model)
    .fetch_optional(pg)
    .await?;
    Ok(match row {
        Some((email, enc, client_id, flagged)) => {
            let connected = enc.is_some_and(|s| !s.is_empty());
            ConnectionFace {
                connected,
                email,
                // A grant cleared by Google names no client and no refusal.
                client_id: client_id.filter(|_| connected),
                flagged: flagged.filter(|_| connected),
            }
        }
        None => ConnectionFace::default(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_owner_column_maps_to_owner_and_absence_maps_to_org() {
        let owner = principal_from_legacy(Some("user-1".into()));
        assert_eq!(owner.kind, PrincipalKind::Owner);
        assert!(!owner.is_org);
        assert_eq!(owner.owner_user_id.as_deref(), Some("user-1"));

        let org = principal_from_legacy(None);
        assert_eq!(org.kind, PrincipalKind::Org);
        assert!(org.is_org);
        assert!(org.owner_user_id.is_none());
    }

    #[test]
    fn registry_kind_wins_and_an_unknown_kind_is_not_guessed() {
        let owner = principal_from_registry("owner", Some("u".into())).expect("owner");
        assert_eq!(owner.kind, PrincipalKind::Owner);
        assert_eq!(owner.owner_user_id.as_deref(), Some("u"));

        let org = principal_from_registry("org", Some("ignored".into())).expect("org");
        assert_eq!(org.kind, PrincipalKind::Org);
        assert!(org.owner_user_id.is_none());

        let agent = principal_from_registry("agent", None).expect("agent");
        assert_eq!(agent.kind, PrincipalKind::Agent);
        assert!(agent.is_org);
        assert!(agent.owner_user_id.is_none());

        assert!(principal_from_registry("nope", None).is_none());
    }

    #[test]
    fn owner_without_google_refuses_and_never_names_the_org_as_a_fallback() {
        let owner = AgentPrincipal::from_kind(PrincipalKind::Owner, Some("u".into()));
        assert!(write_refusal(&owner, true).is_none());
        let refusal = write_refusal(&owner, false).expect("refusal");
        assert!(refusal.contains("will not use the shared org account"));
        assert!(refusal.contains("Settings"));

        let missing = AgentPrincipal::from_kind(PrincipalKind::Owner, None);
        assert!(write_refusal(&missing, true).is_some());
    }

    #[test]
    fn org_and_connected_agent_do_not_refuse_an_outbound_write() {
        let org = AgentPrincipal::from_kind(PrincipalKind::Org, None);
        assert!(write_refusal(&org, false).is_none());

        let agent = AgentPrincipal::from_kind(PrincipalKind::Agent, None);
        assert!(write_refusal(&agent, true).is_none());
        let refusal = write_refusal(&agent, false).expect("agent refusal");
        assert!(refusal.contains("own Google account"));
    }
}
