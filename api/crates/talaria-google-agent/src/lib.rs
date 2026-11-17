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
//
// ONE DOOR for an agent's Google token (`vend_principal_token`): the principal
// names the one stored grant its token may come from (`principal_grant`), the
// grant refreshes with the client that minted it (`vend_grant`), and the
// PURPOSE decides whether that client qualifies. A REST call takes any grant.
// A call to Google's Workspace MCP servers (`Purpose::HostedMcp`) takes one
// unless its minting client is declared NOT marked "Use this client for an
// AI-powered agent" with no acceptance from Google on record — then the
// token goes nowhere near those servers, and the answer names the fix. Unknown goes to Google, whose answer
// is relayed. Eligibility follows the minting CLIENT, never the table the row
// sits in: under a flipped client every stored grant already qualifies, with
// no reconnect. The agent routes (`resolve_agent_google`), the approval
// executor (talaria-google-pending), the queue-time write refusal
// (`outbound_write_refusal`) and the agent record (`agent_google_identity`)
// all read through it.

use serde_json::Value;
use sqlx::PgPool;
use talaria_agent_auth::{AgentSubject, epoch_ms_to_iso, subject_model, subject_proven};
use talaria_google_client::{
    AgenticStanding, agentic_standing, client_evidence, resolve_google_client,
};
use talaria_google_connections::{GrantRow, TokenError, Vended, vend_grant};
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
    /// The OAuth client that minted the grant behind the token (None while a
    /// cached token's grant has not yet recorded its client).
    pub client_id: Option<String>,
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

// ── The one door ─────────────────────────────────────────────────────────────

/// What a token is for. The purpose decides which grants qualify.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Purpose {
    /// Google's REST APIs: any stored grant of the principal's.
    Rest,
    /// Google's Workspace MCP servers, which take only a token minted by a
    /// client marked "Use this client for an AI-powered agent".
    HostedMcp,
}

/// What the door hands out.
pub enum Vend {
    /// A token, whose account it is, and the client that minted it.
    Token(AgentGoogle),
    /// Nothing to hand out: no grant stored for the principal (an owner with
    /// no connection of their own never falls back to the org account), an
    /// unproven caller, or a registry read that failed.
    NotConnected,
    /// `HostedMcp` only: the grant's minting client is declared not marked
    /// for AI agents, so the token is not handed out for Google's Workspace
    /// MCP servers. The sentence names the client and the fix. (The grant
    /// may already have been refreshed at Google's token endpoint: the door
    /// learns the minting client from the vend.)
    NotAgentic(String),
}

/// What `HostedMcp` reads about the clients after the vend: the evidence map
/// (`google_client_evidence`) and the client in the connections slot now —
/// the one a reconnect would mint on.
pub struct ClientFacts {
    pub evidence: Value,
    pub connections_client: Option<String>,
}

/// The clients as they stand, for `HostedMcp` and the agent record.
pub async fn client_facts(pg: &PgPool, sb: &SecretBox) -> ClientFacts {
    let (evidence, connections) =
        futures_util::join!(client_evidence(pg), resolve_google_client(pg, sb));
    ClientFacts {
        evidence,
        connections_client: connections.map(|c| c.client_id),
    }
}

/// The one stored grant a principal's token may come from: the owner's own
/// connection, the org account, or the agent's own identity. An owner with
/// no user names no grant — never the org account's.
pub fn principal_grant(principal: &AgentPrincipal, agent_model: &str) -> Option<GrantRow> {
    match principal.kind {
        PrincipalKind::Owner => principal.owner_user_id.clone().map(GrantRow::User),
        PrincipalKind::Org => Some(GrantRow::Org),
        PrincipalKind::Agent => Some(GrantRow::Agent(agent_model.to_string())),
    }
}

/// What the door says when it withholds a token from Google's Workspace MCP
/// servers — the call it would have made was not made. It speaks only of
/// those servers: the vend may already have refreshed the grant at Google's
/// token endpoint.
pub const MCP_NOT_SENT: &str = "Nothing was sent to Google's Workspace MCP servers.";

/// Why a grant minted by `minted_by` must not go to Google's Workspace MCP
/// servers, or None when it may — as the agent record shows it, where no
/// call was attempted. Only a client declared NOT marked for AI agents, with
/// no acceptance from Google on record, is refused here (Google rejects such
/// a client after November 11, 2026); an unknown or unrecorded client goes
/// to Google, and Google's answer is relayed. The fix is a reconnect when
/// the connections slot now holds a client that is not declared unmarked
/// (it mints the new grant), else an admin's. The door's own refusal
/// (`Vend::NotAgentic`) adds `MCP_NOT_SENT` between the two.
pub fn hosted_mcp_refusal(
    kind: PrincipalKind,
    minted_by: Option<&str>,
    facts: &ClientFacts,
) -> Option<String> {
    refusal_parts(kind, minted_by, facts).map(|(why, fix)| format!("{why} {fix}"))
}

/// `hosted_mcp_refusal`'s two halves: why Google would refuse, and the fix.
fn refusal_parts(
    kind: PrincipalKind,
    minted_by: Option<&str>,
    facts: &ClientFacts,
) -> Option<(String, String)> {
    let minted_by = minted_by.filter(|s| !s.is_empty())?;
    let unmarked =
        |id: &str| agentic_standing(&facts.evidence, id) == AgenticStanding::DeclaredNotAgentic;
    if !unmarked(minted_by) {
        return None;
    }
    let why = format!(
        "This Google connection was made with OAuth client {minted_by}, which is declared as not having \u{201c}Use this client for an AI-powered agent\u{201d} ticked, so Google's Workspace MCP servers would refuse it."
    );
    let fix = match facts.connections_client.as_deref() {
        Some(conn) if conn != minted_by && !unmarked(conn) => match kind {
            PrincipalKind::Owner => format!(
                "Its owner can reconnect Google in Settings \u{2192} Connections, which makes the connection with the connections client {conn}."
            ),
            PrincipalKind::Org => format!(
                "An admin can reconnect the org Google account under Admin \u{2192} Org, which makes the connection with the connections client {conn}."
            ),
            PrincipalKind::Agent => format!(
                "An admin can reconnect this agent's own Google account on its record, which makes the connection with the connections client {conn}."
            ),
        },
        _ => "Reconnecting would not help: an admin needs a client marked for AI agents in the connections slot under Admin \u{2192} Google Workspace.".to_string(),
    };
    Some((why, fix))
}

/// The door, with its two reads as parameters: `vend` is the stored grant's
/// token (`vend_grant`), `facts` the clients (`client_facts`, read only for
/// `HostedMcp`, and only once a token was had). The decision table drives
/// this with stubs; `vend_principal_token` with the database.
pub async fn vend_with<V, VF, F, FF>(
    principal: &AgentPrincipal,
    agent_model: &str,
    purpose: Purpose,
    vend: V,
    facts: F,
) -> Result<Vend, TokenError>
where
    V: FnOnce(GrantRow) -> VF,
    VF: std::future::Future<Output = Result<Option<Vended>, TokenError>>,
    F: FnOnce() -> FF,
    FF: std::future::Future<Output = ClientFacts>,
{
    let Some(row) = principal_grant(principal, agent_model) else {
        return Ok(Vend::NotConnected);
    };
    let Some(vended) = vend(row).await? else {
        return Ok(Vend::NotConnected);
    };
    // An unrecorded client is unknown: it goes to Google, no read needed.
    if purpose == Purpose::HostedMcp
        && vended.client_id.is_some()
        && let Some((why, fix)) =
            refusal_parts(principal.kind, vended.client_id.as_deref(), &facts().await)
    {
        return Ok(Vend::NotAgentic(format!("{why} {MCP_NOT_SENT} {fix}")));
    }
    Ok(Vend::Token(AgentGoogle {
        token: vended.token,
        principal: principal.kind.as_str(),
        owner_user_id: principal.owner_user_id.clone(),
        client_id: vended.client_id,
    }))
}

/// A token for `principal` — the registry's, or the one an approval row
/// stored — for `purpose`. `Err` is the token itself failing (Google refusing
/// the grant or the client, a client configured nowhere, the refresh not
/// getting through): never folded into "not connected".
pub async fn vend_principal_token(
    pg: &PgPool,
    sb: &SecretBox,
    principal: &AgentPrincipal,
    agent_model: &str,
    purpose: Purpose,
    now_ms: i64,
) -> Result<Vend, TokenError> {
    vend_with(
        principal,
        agent_model,
        purpose,
        |row| async move { vend_grant(pg, sb, &row, now_ms).await },
        || client_facts(pg, sb),
    )
    .await
}

/// The model a caller may be vended a token for: a proven caller's, never a
/// legacy shared-key caller's. Handing out an OAuth token is the single
/// largest grant on the agent surface, so the proof check lives at the door.
pub fn admitted(agent: &AgentSubject) -> Option<&str> {
    subject_proven(agent).then(|| subject_model(agent))
}

/// The door for a calling agent: proof first, then the registry's principal
/// (a read that fails answers `NotConnected` — handing out the org token
/// because a lookup blipped is the silent fallback this refuses).
pub async fn vend_for_caller(
    pg: &PgPool,
    sb: &SecretBox,
    agent: &AgentSubject,
    purpose: Purpose,
    now_ms: i64,
) -> Result<Vend, TokenError> {
    let Some(agent_model) = admitted(agent) else {
        return Ok(Vend::NotConnected);
    };
    let Ok(principal) = load_principal(pg, agent_model).await else {
        return Ok(Vend::NotConnected);
    };
    vend_principal_token(pg, sb, &principal, agent_model, purpose, now_ms).await
}

/// A REST token for the calling agent — the door with `Purpose::Rest`.
/// `Ok(None)` when the relevant connection isn't set up. A personal
/// assistant NEVER falls back to the org account.
///
/// `Err` is the token itself failing. It is NOT folded into "not connected":
/// the caller answers it through `google_token_fail`, which quotes what
/// Google said.
///
/// Takes the CALLER, not a bare name: the proof check lives at the door.
pub async fn resolve_agent_google(
    pg: &PgPool,
    sb: &SecretBox,
    agent: &AgentSubject,
    now_ms: i64,
) -> Result<Option<AgentGoogle>, TokenError> {
    Ok(
        match vend_for_caller(pg, sb, agent, Purpose::Rest, now_ms).await? {
            Vend::Token(google) => Some(google),
            // `NotAgentic` is HostedMcp's alone; REST takes any grant.
            Vend::NotConnected | Vend::NotAgentic(_) => None,
        },
    )
}

/// Why an outbound write must not queue. `Ok(None)` means the principal's
/// Google identity is connected (or it is the org account, whose missing
/// connection still surfaces at approval). Reads the grant the door would
/// vend from, without vending it.
pub async fn outbound_write_refusal(
    pg: &PgPool,
    agent_model: &str,
) -> Result<Option<&'static str>, sqlx::Error> {
    let principal = load_principal(pg, agent_model).await?;
    let connected = match principal_grant(&principal, agent_model) {
        // The org account's connection is not this check's: it fails at
        // approval, the cutover behaviour `write_refusal` keeps.
        Some(GrantRow::Org) | None => false,
        Some(row) => grant_face(pg, &row).await?.connected,
    };
    Ok(write_refusal(&principal, connected))
}

/// Would the door have a token for this agent to take to Google's Workspace
/// MCP servers? Its principal's grant holds a refresh token, and that grant's
/// minting client is not declared unticked for AI agents with no acceptance
/// from Google on record (an unrecorded client counts, as the door counts
/// it). Reads the grant's face and the evidence only: no token, no refresh.
/// An owner with no user names no grant (false), never the org account's.
/// The MCP render's "needs a connection" mark for a Google Workspace MCP
/// server.
pub async fn hosted_mcp_ready(pg: &PgPool, agent_model: &str) -> Result<bool, sqlx::Error> {
    let principal = load_principal(pg, agent_model).await?;
    let Some(row) = principal_grant(&principal, agent_model) else {
        return Ok(false);
    };
    let face = grant_face(pg, &row).await?;
    if !face.connected {
        return Ok(false);
    }
    let Some(client) = face.client_id.as_deref().filter(|c| !c.is_empty()) else {
        return Ok(true);
    };
    let evidence = client_evidence(pg).await;
    Ok(agentic_standing(&evidence, client) != AgenticStanding::DeclaredNotAgentic)
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

/// What the admin record shows: the grant this kind's token comes from (the
/// door's `principal_grant`) — the owner's connection, the agent's, or the
/// org account's. `client_id` and `flagged` describe that grant: the OAuth
/// client that minted it, and Google's words from the last refusal of that
/// client. `agentic` is what is known of that client's AI-agent mark, and
/// `mcp_refusal` why the door would withhold this grant from Google's
/// Workspace MCP servers, and the fix (`hosted_mcp_refusal`; None: it would
/// go to Google).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentGoogleIdentity {
    pub kind: PrincipalKind,
    pub principal_user_id: Option<String>,
    pub connected: bool,
    pub email: Option<String>,
    pub legacy_owner_user_id: Option<String>,
    pub client_id: Option<String>,
    pub flagged: Option<String>,
    pub agentic: Option<AgenticStanding>,
    pub mcp_refusal: Option<String>,
}

pub async fn agent_google_identity(
    pg: &PgPool,
    sb: &SecretBox,
    agent_model: &str,
) -> Result<AgentGoogleIdentity, sqlx::Error> {
    let principal = load_principal(pg, agent_model).await?;
    let legacy_owner_user_id = legacy_owner(pg, agent_model).await?;
    let face = match principal_grant(&principal, agent_model) {
        Some(row) => grant_face(pg, &row).await?,
        None => ConnectionFace::default(),
    };
    let (agentic, mcp_refusal) = match face.client_id.as_deref() {
        Some(id) => {
            let facts = client_facts(pg, sb).await;
            (
                Some(agentic_standing(&facts.evidence, id)),
                hosted_mcp_refusal(principal.kind, Some(id), &facts),
            )
        }
        None => (None, None),
    };
    Ok(AgentGoogleIdentity {
        kind: principal.kind,
        principal_user_id: principal.owner_user_id,
        connected: face.connected,
        email: face.email,
        legacy_owner_user_id,
        client_id: face.client_id,
        flagged: face.flagged,
        agentic,
        mcp_refusal,
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

/// One stored grant as the agent record shows it.
#[derive(Default)]
struct ConnectionFace {
    connected: bool,
    email: Option<String>,
    client_id: Option<String>,
    flagged: Option<String>,
}

/// Any store's grant, read for its face — no token, no refresh.
async fn grant_face(pg: &PgPool, row: &GrantRow) -> Result<ConnectionFace, sqlx::Error> {
    type FaceRow = (
        Option<String>,
        Option<String>,
        Option<String>,
        Option<String>,
    );
    let read: Option<FaceRow> = match row {
        GrantRow::User(user_id) => {
            sqlx::query_as(
                "select email, refresh_token_enc, oauth_client_id, reconnect_reason \
                 from google_connections where user_id = $1::uuid",
            )
            .bind(user_id)
            .fetch_optional(pg)
            .await?
        }
        GrantRow::Org => {
            sqlx::query_as(
                "select email, refresh_token_enc, oauth_client_id, reconnect_reason \
                 from google_org_connection where id = 1",
            )
            .fetch_optional(pg)
            .await?
        }
        GrantRow::Agent(model) => {
            sqlx::query_as(
                "select email, refresh_token_enc, oauth_client_id, reconnect_reason \
                 from agent_google_connections where agent_model = $1",
            )
            .bind(model)
            .fetch_optional(pg)
            .await?
        }
    };
    Ok(match read {
        Some((email, enc, client_id, flagged)) => {
            let connected = enc.is_some_and(|s| !s.is_empty());
            if !connected {
                // A grant cleared by Google is not connected, names no
                // client and no refusal.
                return Ok(ConnectionFace::default());
            }
            ConnectionFace {
                connected,
                email,
                client_id,
                flagged,
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

/// The one door's decision table: every principal × purpose × the grant it
/// holds (minted by a client declared agentic, verified by Google, declared
/// not agentic, unknown, not yet recorded, or no grant at all), driven
/// through `vend_with` itself with stub reads. There is no stage-2 agent
/// grant store to vary: full separation (D2) is not built.
#[cfg(test)]
mod door {
    use std::sync::Mutex;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::json;
    use talaria_agent_auth::AgentCaller;

    use super::*;

    const MODEL: &str = "scout-ops";
    const USER: &str = "user-1";
    /// The connections slot now: declared agentic.
    const CONN: &str = "conn-now";
    const DECLARED: &str = "client-declared";
    /// Declared unticked, but Google accepted a check: Google's answer wins.
    const VERIFIED: &str = "client-verified";
    /// Declared unticked; Google refused a check, which proves nothing.
    const UNMARKED: &str = "client-unmarked";
    const UNKNOWN: &str = "client-unknown";

    fn evidence() -> Value {
        let check = |ok: bool| json!({ "ok": ok, "status": if ok { 200 } else { 403 }, "text": if ok { Value::Null } else { json!("Google's words") }, "at": "2026-11-16T00:00:00.000Z" });
        let mut m = serde_json::Map::new();
        m.insert(CONN.into(), json!({ "declaredAgentic": true }));
        m.insert(DECLARED.into(), json!({ "declaredAgentic": true }));
        m.insert(
            VERIFIED.into(),
            json!({ "declaredAgentic": false, "lastCheck": check(true) }),
        );
        m.insert(
            UNMARKED.into(),
            json!({ "declaredAgentic": false, "lastCheck": check(false) }),
        );
        Value::Object(m)
    }

    fn facts(connections: Option<&str>) -> ClientFacts {
        ClientFacts {
            evidence: evidence(),
            connections_client: connections.map(String::from),
        }
    }

    /// The grant a principal holds in this case.
    #[derive(Clone, Copy, Debug)]
    enum Held {
        /// A live grant, minted by this client (None: not yet recorded).
        Grant(Option<&'static str>),
        /// No grant stored.
        Nothing,
    }

    const HELD: [Held; 6] = [
        Held::Grant(Some(DECLARED)),
        Held::Grant(Some(VERIFIED)),
        Held::Grant(Some(UNMARKED)),
        Held::Grant(Some(UNKNOWN)),
        Held::Grant(None),
        Held::Nothing,
    ];

    fn principals() -> [AgentPrincipal; 3] {
        [
            AgentPrincipal::from_kind(PrincipalKind::Owner, Some(USER.into())),
            AgentPrincipal::from_kind(PrincipalKind::Org, None),
            AgentPrincipal::from_kind(PrincipalKind::Agent, None),
        ]
    }

    struct Run {
        vend: Result<Vend, TokenError>,
        /// Every grant the door asked for.
        asked: Vec<GrantRow>,
        facts_reads: usize,
    }

    /// The door over a store where `principal`'s own grant is `held` and
    /// every OTHER grant — the org account's included — is a live grant from
    /// a declared-agentic client, so a fallback would show.
    async fn run(
        principal: &AgentPrincipal,
        purpose: Purpose,
        held: Held,
        connections: Option<&'static str>,
    ) -> Run {
        let own = principal_grant(principal, MODEL);
        let asked = Mutex::new(Vec::new());
        let reads = AtomicUsize::new(0);
        let vend = vend_with(
            principal,
            MODEL,
            purpose,
            |row: GrantRow| {
                asked.lock().unwrap().push(row.clone());
                let minted = if Some(&row) == own.as_ref() {
                    match held {
                        Held::Grant(client) => Some(client),
                        Held::Nothing => None,
                    }
                } else {
                    Some(Some(DECLARED))
                };
                async move {
                    Ok(minted.map(|client| Vended {
                        token: format!("token-for-{row:?}"),
                        client_id: client.map(String::from),
                    }))
                }
            },
            || {
                reads.fetch_add(1, Ordering::SeqCst);
                async move { facts(connections) }
            },
        )
        .await;
        Run {
            vend,
            asked: asked.into_inner().unwrap(),
            facts_reads: reads.load(Ordering::SeqCst),
        }
    }

    #[tokio::test]
    async fn every_principal_purpose_and_grant() {
        let mut cases = 0;
        for principal in principals() {
            let own = principal_grant(&principal, MODEL).expect("each names a grant");
            for purpose in [Purpose::Rest, Purpose::HostedMcp] {
                for held in HELD {
                    cases += 1;
                    let at = format!("{:?} {purpose:?} {held:?}", principal.kind);
                    let r = run(&principal, purpose, held, Some(CONN)).await;
                    // Only the principal's own grant is ever asked for, once.
                    assert_eq!(r.asked, vec![own.clone()], "{at}");
                    let minted = match held {
                        Held::Nothing => {
                            assert!(matches!(r.vend, Ok(Vend::NotConnected)), "{at}");
                            assert_eq!(r.facts_reads, 0, "{at}");
                            continue;
                        }
                        Held::Grant(minted) => minted,
                    };
                    // The clients are read for HostedMcp alone, and only when
                    // the grant names its client.
                    let reads = usize::from(purpose == Purpose::HostedMcp && minted.is_some());
                    assert_eq!(r.facts_reads, reads, "{at}");
                    let refused = purpose == Purpose::HostedMcp && minted == Some(UNMARKED);
                    match r.vend {
                        Ok(Vend::NotAgentic(why)) => {
                            assert!(refused, "{at}: refused {why}");
                            assert!(why.contains(UNMARKED), "{at}: {why}");
                            assert!(
                                why.contains(
                                    "\u{201c}Use this client for an AI-powered agent\u{201d}"
                                ),
                                "{at}: Google's checkbox, quoted: {why}"
                            );
                            assert!(
                                why.contains(CONN),
                                "{at}: the reconnect names its client: {why}"
                            );
                            let fix = match principal.kind {
                                PrincipalKind::Owner => "Settings \u{2192} Connections",
                                PrincipalKind::Org => "Admin \u{2192} Org",
                                PrincipalKind::Agent => "on its record",
                            };
                            assert!(why.contains(fix), "{at}: {why}");
                            // The door says what it withheld, and only that:
                            // the vend may have refreshed at Google's token
                            // endpoint, so never "nothing was sent to Google".
                            assert!(why.contains(MCP_NOT_SENT), "{at}: {why}");
                            assert!(!why.contains("sent to Google."), "{at}: {why}");
                        }
                        Ok(Vend::Token(google)) => {
                            assert!(!refused, "{at}");
                            assert_eq!(google.token, format!("token-for-{own:?}"), "{at}");
                            assert_eq!(google.client_id.as_deref(), minted, "{at}");
                            assert_eq!(google.principal, principal.kind.as_str(), "{at}");
                            assert_eq!(google.owner_user_id, principal.owner_user_id, "{at}");
                        }
                        Ok(Vend::NotConnected) => panic!("{at}: a held grant read as none"),
                        Err(e) => panic!("{at}: {e}"),
                    }
                }
            }
        }
        assert_eq!(cases, 3 * 2 * HELD.len());
    }

    #[tokio::test]
    async fn an_owner_with_no_connection_never_gets_the_org_token() {
        // Owner with a user but no grant: the org grant is live, and unasked.
        let owner = AgentPrincipal::from_kind(PrincipalKind::Owner, Some(USER.into()));
        for purpose in [Purpose::Rest, Purpose::HostedMcp] {
            let r = run(&owner, purpose, Held::Nothing, Some(CONN)).await;
            assert!(matches!(r.vend, Ok(Vend::NotConnected)));
            assert_eq!(r.asked, vec![GrantRow::User(USER.into())]);
        }
        // Owner with no user at all: no grant named, nothing asked.
        let orphan = AgentPrincipal::from_kind(PrincipalKind::Owner, None);
        assert_eq!(principal_grant(&orphan, MODEL), None);
        for purpose in [Purpose::Rest, Purpose::HostedMcp] {
            let r = run(&orphan, purpose, Held::Grant(Some(DECLARED)), Some(CONN)).await;
            assert!(matches!(r.vend, Ok(Vend::NotConnected)));
            assert!(r.asked.is_empty(), "{:?}", r.asked);
            assert_eq!(r.facts_reads, 0);
        }
    }

    #[tokio::test]
    async fn a_token_failure_propagates_for_either_purpose_and_reads_no_clients() {
        for principal in principals() {
            for purpose in [Purpose::Rest, Purpose::HostedMcp] {
                let reads = AtomicUsize::new(0);
                let vend = vend_with(
                    &principal,
                    MODEL,
                    purpose,
                    |_row| async {
                        Err::<Option<Vended>, _>(TokenError::ClientRejected(
                            "invalid_client".into(),
                        ))
                    },
                    || {
                        reads.fetch_add(1, Ordering::SeqCst);
                        async { facts(Some(CONN)) }
                    },
                )
                .await;
                assert!(
                    matches!(vend, Err(TokenError::ClientRejected(_))),
                    "{:?} {purpose:?}",
                    principal.kind
                );
                assert_eq!(reads.load(Ordering::SeqCst), 0);
            }
        }
    }

    #[test]
    fn the_fix_is_an_admins_when_no_reconnect_would_mint_on_a_marked_client() {
        // The connections slot holds the unmarked client itself, another
        // client declared unmarked, or nothing at all.
        let mut also_unmarked = evidence();
        also_unmarked["other-unmarked"] = json!({ "declaredAgentic": false });
        for connections in [Some(UNMARKED), Some("other-unmarked"), None] {
            let facts = ClientFacts {
                evidence: also_unmarked.clone(),
                connections_client: connections.map(String::from),
            };
            for kind in [
                PrincipalKind::Owner,
                PrincipalKind::Org,
                PrincipalKind::Agent,
            ] {
                let why = hosted_mcp_refusal(kind, Some(UNMARKED), &facts).expect("refused");
                assert!(
                    why.contains("Reconnecting would not help"),
                    "{connections:?} {kind:?}: {why}"
                );
                // The record's sentence: no call was attempted, so it claims
                // nothing about what was sent.
                assert!(!why.contains("sent"), "{why}");
                assert!(why.contains("Admin \u{2192} Google Workspace"), "{why}");
            }
        }
        // A connections client nobody has spoken for still gets the reconnect:
        // Google decides on it.
        let facts = ClientFacts {
            evidence: evidence(),
            connections_client: Some("conn-unsaid".into()),
        };
        let why = hosted_mcp_refusal(PrincipalKind::Owner, Some(UNMARKED), &facts).unwrap();
        assert!(
            why.contains("conn-unsaid") && why.contains("reconnect"),
            "{why}"
        );
        // Nothing refused for every other standing, or with no recorded client.
        for minted in [
            Some(DECLARED),
            Some(VERIFIED),
            Some(UNKNOWN),
            Some(""),
            None,
        ] {
            assert_eq!(
                hosted_mcp_refusal(PrincipalKind::Agent, minted, &facts),
                None,
                "{minted:?}"
            );
        }
    }

    #[test]
    fn only_a_proven_caller_is_admitted() {
        let caller = |legacy: bool| {
            AgentSubject::Caller(AgentCaller {
                id: (!legacy).then(|| "def-1".to_string()),
                model: MODEL.into(),
                legacy,
            })
        };
        assert_eq!(admitted(&caller(false)), Some(MODEL));
        assert_eq!(admitted(&caller(true)), None);
        assert_eq!(admitted(&AgentSubject::Model(MODEL.into())), Some(MODEL));
    }
}
