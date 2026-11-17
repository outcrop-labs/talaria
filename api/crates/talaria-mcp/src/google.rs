// Google's Workspace MCP servers in the registry — the pack's rows
// (`auth_mode = 'google'`, named `google-<product>`; talaria-google-mcp's
// `pack`). What this module adds to the registry's brain:
//
//   WHO SPEAKS  the calling agent's own Google principal (`agent_principals`:
//               owner | org | agent), never `assistant_owner_for`, and only
//               through the one door (talaria-google-agent's
//               `vend_principal_token`, `Purpose::HostedMcp`). An unproven
//               (legacy shared-key) caller is refused outright. An owner
//               with no Google connection of their own is NOT CONNECTED —
//               the org account is never used instead. A grant whose minting
//               client is declared not marked for AI agents never reaches
//               Google: the door's own sentence answers.
//   WHAT RUNS   the pack's policy per tool (`policy::gateway_policy`): the
//               pinned read list AND the cached `readOnlyHint` run; the
//               owner's deny list never runs; every other tool — a write, a
//               tool Google adds later — is HELD: the gateway queues the
//               exact call for a person (the `mcp_call` approval) and Google
//               hears nothing until they approve. An admin's per-tool
//               override (`mcp_tool_policies`, `tool_overrides`) wins over
//               the default, except that no override lifts the deny list.
//   WHERE       the product's endpoint as the code names it
//               (`pack::endpoint`), never the row's stored URL.
//
// The registry's assignment, team and owner-access rules apply unchanged;
// this only decides the account and the allowance once those have.

use serde_json::Value;
use sqlx::PgPool;
use talaria_agent_auth::{AgentSubject, subject_model, subject_proven};
use talaria_google_agent::{
    AgentPrincipal, PrincipalKind, Purpose, Vend, principal_grant, resolve_agent_principal,
    vend_principal_token,
};
use talaria_google_connections::{GrantRow, McpGrantFacts, TokenError, grant_mcp_facts};
use talaria_google_errors::token_failure_sentence;
use talaria_google_mcp::catalog::{CatalogError, fetch_catalog};
use talaria_google_mcp::policy::Policy;
use talaria_google_mcp::{Product, pack, policy};
use talaria_secretbox::SecretBox;

use crate::not_connected::ConnectScope;
use crate::registry::McpServer;

pub use talaria_google_mcp::pack::AUTH_MODE;

/// The surface name a token failure's sentence uses.
const SURFACE: &str = "Google Workspace MCP";

/// Is this a pack row?
pub fn is_google(server: &McpServer) -> bool {
    server.auth_mode == AUTH_MODE
}

/// The product a pack row serves; None for any other row, and for a pack
/// row whose name no product claims (refused by the gateway).
pub fn product_of(server: &McpServer) -> Option<Product> {
    if is_google(server) {
        pack::product_of_server(&server.name)
    } else {
        None
    }
}

/// What the gateway needs to relay a call to a pack row: the product, the
/// grant the bearer came from (where Google's refusals are recorded), whose
/// it is, and the scopes Google gave that grant.
#[derive(Clone, Debug)]
pub struct GoogleCall {
    pub product: Product,
    pub grant: GrantRow,
    pub principal: PrincipalKind,
    /// The grant's stored scope, space-joined; None when not recorded.
    pub scope: Option<String>,
    /// The grant carries a flag from a 401 a fresh token did not cure
    /// (`last_mcp_refusal.flagged`): an accepted call forgets it.
    pub flagged: bool,
    /// The admin's per-tool overrides on this server (`tool_overrides`).
    pub overrides: Vec<(String, Policy)>,
}

/// An admin's per-tool overrides on a pack server (`mcp_tool_policies`). A
/// stored policy this build does not know is skipped, never guessed: the tool
/// keeps the pack's default.
pub async fn tool_overrides(
    pg: &PgPool,
    server_id: &str,
) -> Result<Vec<(String, Policy)>, sqlx::Error> {
    let rows: Vec<(String, String)> = sqlx::query_as(
        "select tool, policy from mcp_tool_policies where server_id = $1::uuid order by tool",
    )
    .bind(server_id)
    .fetch_all(pg)
    .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(tool, policy)| Policy::parse(&policy).map(|p| (tool, p)))
        .collect())
}

/// The connect scope for a principal kind.
pub fn connect_scope(kind: PrincipalKind) -> ConnectScope {
    match kind {
        PrincipalKind::Owner => ConnectScope::GoogleOwner,
        PrincipalKind::Org => ConnectScope::GoogleOrg,
        PrincipalKind::Agent => ConnectScope::GoogleAgent,
    }
}

/// The account a pack row's call speaks as, decided. Its `Debug` never
/// prints the bearer.
pub enum Speak {
    /// Speak with this bearer, minted for this grant.
    As { token: String, grant: GrantRow },
    /// Refuse: the caller is not proven.
    Refuse,
    /// Not connected, under this scope, with the door's or Google's sentence
    /// when there is more to say than "nobody connected it".
    Missing {
        scope: ConnectScope,
        detail: Option<String>,
    },
}

impl std::fmt::Debug for Speak {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Speak::As { grant, .. } => f
                .debug_struct("As")
                .field("token", &"<redacted>")
                .field("grant", grant)
                .finish(),
            Speak::Refuse => f.write_str("Refuse"),
            Speak::Missing { scope, detail } => f
                .debug_struct("Missing")
                .field("scope", scope)
                .field("detail", detail)
                .finish(),
        }
    }
}

/// The decision, pure: the caller's proof, its principal, and what the door
/// answered (None when the principal could not be read — never the org
/// account's token instead).
pub fn speak_of(
    proven: bool,
    principal: Option<&AgentPrincipal>,
    agent_model: &str,
    vend: Option<Result<Vend, TokenError>>,
) -> Speak {
    if !proven {
        return Speak::Refuse;
    }
    let Some(principal) = principal else {
        // The registry read failed: nothing is vended, and the agent is told
        // its owner's account is not connected — the safest scope to name.
        return Speak::Missing {
            scope: ConnectScope::GoogleOwner,
            detail: None,
        };
    };
    let scope = connect_scope(principal.kind);
    let missing = |detail| Speak::Missing { scope, detail };
    match vend {
        None | Some(Ok(Vend::NotConnected)) => missing(None),
        Some(Ok(Vend::NotAgentic(sentence))) => missing(Some(sentence)),
        Some(Err(e)) => missing(Some(token_failure_sentence(&e, SURFACE))),
        Some(Ok(Vend::Token(g))) => match principal_grant(principal, agent_model) {
            Some(grant) => Speak::As {
                token: g.token,
                grant,
            },
            // The door vends only from the principal's grant; a token with
            // no grant to name is not one this relay can account for.
            None => missing(None),
        },
    }
}

/// Resolve who a pack row's call speaks as, through the one door.
pub async fn speak_as(pg: &PgPool, sb: &SecretBox, subject: &AgentSubject, now_ms: i64) -> Speak {
    if !subject_proven(subject) {
        return Speak::Refuse;
    }
    let agent_model = subject_model(subject);
    let principal = resolve_agent_principal(pg, agent_model).await.ok();
    let vend = match &principal {
        Some(p) => {
            Some(vend_principal_token(pg, sb, p, agent_model, Purpose::HostedMcp, now_ms).await)
        }
        None => None,
    };
    speak_of(true, principal.as_ref(), agent_model, vend)
}

/// The grant's stored scope (for the missing-scope check) and its MCP flag,
/// in one read; a read that fails is unknown, and Google decides.
pub async fn facts_of(pg: &PgPool, grant: &GrantRow) -> McpGrantFacts {
    grant_mcp_facts(pg, grant).await.unwrap_or_default()
}

/// Does this agent's Google principal need a connection before the door
/// would vend it a token for Google's Workspace MCP servers — no grant, or a
/// grant minted by a client declared not marked for AI agents? No token is
/// read and nothing is refreshed: the render's "needs a connection" mark.
pub async fn needs_connection(pg: &PgPool, agent_model: &str) -> bool {
    !talaria_google_agent::hosted_mcp_ready(pg, agent_model)
        .await
        .unwrap_or(false)
}

/// A pack row's catalog, asked of Google: with the org account's bearer when
/// the door vends one for Google's MCP servers, else (and after the org
/// bearer is refused) with no credential at all. `Ok` carries the tools and
/// whether the listing was authenticated.
pub async fn google_catalog(
    pg: &PgPool,
    sb: &SecretBox,
    product: &Product,
    now_ms: i64,
) -> Result<(Vec<Value>, bool), CatalogError> {
    let org = AgentPrincipal::from_kind(PrincipalKind::Org, None);
    let bearer = match vend_principal_token(pg, sb, &org, "", Purpose::HostedMcp, now_ms).await {
        Ok(Vend::Token(g)) => Some(g.token),
        _ => None,
    };
    if let Some(b) = bearer.as_deref() {
        match fetch_catalog(product, Some(b)).await {
            Ok(tools) => return Ok((tools, true)),
            Err(CatalogError::Refused { status, text }) if status == 401 || status == 403 => {
                tracing::info!(
                    "[mcp/google] {}: the org account's listing was refused ({status}{}); listing without a credential",
                    product.id,
                    text.map(|t| format!(": {t}")).unwrap_or_default()
                );
            }
            Err(e) => return Err(e),
        }
    }
    fetch_catalog(product, None).await.map(|t| (t, false))
}

/// The probe sentence for a fresh catalog: the tools, how many run without
/// a person and how many are held for one, by the pack's defaults.
pub fn catalog_detail(product: &Product, tools: &[Value], authenticated: bool) -> String {
    let catalog = Value::Array(tools.to_vec());
    let reads = policy::allowed_tools(product.id, &catalog).len();
    let held = policy::held_tools(product.id, &catalog, &[]).len();
    format!(
        "{} tool(s), {reads} read-only allowed, {held} held for a person{}",
        tools.len(),
        if authenticated {
            ""
        } else {
            " (listed without a credential)"
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use talaria_google_agent::{ClientFacts, vend_with};
    use talaria_google_connections::Vended;

    const CONN: &str = "conn-a.apps.googleusercontent.com";
    const UNMARKED: &str = "unmarked-b.apps.googleusercontent.com";

    fn facts() -> ClientFacts {
        ClientFacts {
            evidence: serde_json::json!({
                CONN: { "declaredAgentic": true },
                UNMARKED: { "declaredAgentic": false },
            }),
            connections_client: Some(CONN.into()),
        }
    }

    /// What each store holds in a case: the grant's minting client, or no
    /// grant at all.
    #[derive(Clone, Copy, Debug)]
    enum Row {
        Agentic,
        Unmarked,
        None,
    }

    fn token_for(row: &GrantRow) -> String {
        match row {
            GrantRow::User(u) => format!("tok-user-{u}"),
            GrantRow::Org => "tok-org".into(),
            GrantRow::Agent(m) => format!("tok-agent-{m}"),
        }
    }

    /// Drive the real door (`vend_with`) with a stub store, then the
    /// decision — the gateway's two steps for one call. `reads` counts the
    /// grants the door asked the store for.
    fn decide(principal: &AgentPrincipal, model: &str, row: Row, reads: &Cell<usize>) -> Speak {
        let rt = tokio::runtime::Builder::new_current_thread()
            .build()
            .expect("a test runtime");
        let vend = rt.block_on(vend_with(
            principal,
            model,
            Purpose::HostedMcp,
            |grant: GrantRow| {
                reads.set(reads.get() + 1);
                let out = match row {
                    Row::None => None,
                    Row::Agentic => Some(Vended {
                        token: token_for(&grant),
                        client_id: Some(CONN.into()),
                    }),
                    Row::Unmarked => Some(Vended {
                        token: token_for(&grant),
                        client_id: Some(UNMARKED.into()),
                    }),
                };
                async move { Ok(out) }
            },
            || async { facts() },
        ));
        speak_of(true, Some(principal), model, Some(vend))
    }

    /// Every principal × {agentic row, declared-not-agentic row, none}.
    #[test]
    fn every_principal_against_every_grant_it_can_hold() {
        let owner = AgentPrincipal::from_kind(PrincipalKind::Owner, Some("u1".into()));
        let org = AgentPrincipal::from_kind(PrincipalKind::Org, None);
        let agent = AgentPrincipal::from_kind(PrincipalKind::Agent, None);
        for (principal, scope, grant) in [
            (
                &owner,
                ConnectScope::GoogleOwner,
                GrantRow::User("u1".into()),
            ),
            (&org, ConnectScope::GoogleOrg, GrantRow::Org),
            (
                &agent,
                ConnectScope::GoogleAgent,
                GrantRow::Agent("m".into()),
            ),
        ] {
            let reads = Cell::new(0);
            match decide(principal, "m", Row::Agentic, &reads) {
                Speak::As { token, grant: g } => {
                    assert_eq!(g, grant);
                    assert_eq!(token, token_for(&grant));
                }
                other => panic!("{:?}: {other:?}", principal.kind),
            }
            match decide(principal, "m", Row::Unmarked, &reads) {
                Speak::Missing {
                    scope: s,
                    detail: Some(d),
                } => {
                    assert_eq!(s, scope);
                    assert!(d.contains(UNMARKED), "{d}");
                    assert!(
                        d.contains("Nothing was sent to Google's Workspace MCP servers."),
                        "{d}"
                    );
                }
                other => panic!("{:?}: {other:?}", principal.kind),
            }
            match decide(principal, "m", Row::None, &reads) {
                Speak::Missing {
                    scope: s,
                    detail: None,
                } => assert_eq!(s, scope),
                other => panic!("{:?}: {other:?}", principal.kind),
            }
            assert_eq!(reads.get(), 3, "one grant read per call");
        }
    }

    #[test]
    fn an_owner_never_falls_back_to_the_org_account() {
        // An owner principal with no user names no grant: the door reads
        // nothing, and the answer is the owner's not-connected sentence even
        // though the org store would hand out a token.
        let orphan = AgentPrincipal::from_kind(PrincipalKind::Owner, None);
        let reads = Cell::new(0);
        match decide(&orphan, "m", Row::Agentic, &reads) {
            Speak::Missing {
                scope: ConnectScope::GoogleOwner,
                detail: None,
            } => {}
            other => panic!("{other:?}"),
        }
        assert_eq!(reads.get(), 0, "no store was read");
        // And a principal that could not be read is not the org account.
        match speak_of(true, None, "m", None) {
            Speak::Missing {
                scope: ConnectScope::GoogleOwner,
                detail: None,
            } => {}
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_assistant_whose_principal_is_org_follows_org() {
        // A personal assistant (it has an owner) whose registry row says
        // org: the principal is what the door reads, so the org grant speaks.
        let org = AgentPrincipal::from_kind(PrincipalKind::Org, Some("owner-u".into()));
        assert_eq!(org.owner_user_id, None, "org never carries an owner");
        let reads = Cell::new(0);
        match decide(&org, "pa-model", Row::Agentic, &reads) {
            Speak::As { token, grant } => {
                assert_eq!(grant, GrantRow::Org);
                assert_eq!(token, "tok-org");
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn an_unproven_caller_is_refused_before_anything_is_read() {
        let owner = AgentPrincipal::from_kind(PrincipalKind::Owner, Some("u1".into()));
        assert!(matches!(
            speak_of(false, Some(&owner), "m", Some(Ok(Vend::NotConnected))),
            Speak::Refuse
        ));
        assert!(matches!(speak_of(false, None, "m", None), Speak::Refuse));
    }

    #[test]
    fn a_token_failure_is_quoted_never_folded_into_not_connected() {
        let agent = AgentPrincipal::from_kind(PrincipalKind::Agent, None);
        let e = TokenError::ClientRejected(
            r#"Google token refresh failed (401): {"error":"invalid_client","error_description":"The OAuth client was not found."}"#.into(),
        );
        match speak_of(true, Some(&agent), "m", Some(Err(e))) {
            Speak::Missing {
                scope: ConnectScope::GoogleAgent,
                detail: Some(d),
            } => assert!(d.contains("The OAuth client was not found."), "{d}"),
            other => panic!("{other:?}"),
        }
    }
}
