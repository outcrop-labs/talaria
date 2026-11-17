// The `mcp_call` approval: a call an agent made to one of Google's Workspace
// MCP servers that the pack held for a person (talaria-google-mcp's policy —
// every write, and every tool Google adds later). The MCP gateway queued the
// exact call here, with the agent's principal; Google's MCP servers heard
// nothing. Approving it replays that call, once, as that principal:
//
//   personal (an owner principal) → the OWNER approves; the owner's own grant
//   org account                   → an ADMIN approves; the org grant
//   agent identity                → an ADMIN approves; the agent's own grant
//
// — the authority `decide_action` checks before it claims the row, the same
// as every other kind. A conversation unlock (talaria-chips, `<server>/<tool>`)
// can run one later call of the same tool without a new card, and only for a
// personal principal; the gateway decides that, never this file.
//
// THE REPLAY RE-CHECKS before anything reaches Google (`decide`):
//   · the arguments still name no workspace secret («secret:…») — the gateway
//     refused one at queue time; a row is never trusted to have been made
//     only by it;
//   · the registry row still exists, is still the pack's (`auth_mode =
//     'google'`, named `google-<product>` for the product queued), and is
//     still turned on;
//   · Google's catalog, as last listed, still has the tool;
//   · the pack's policy for it — an admin's override, else the default — is
//     not DENY now;
// and only then vends a token for `Purpose::HostedMcp` through the one door
// (a grant minted by a client declared not marked for AI agents never
// reaches Google) and sends ONE stateless `tools/call` to the product's
// endpoint as the code names it (`pack::endpoint`), never a stored URL.
//
// WHAT COMES BACK (talaria-google-mcp's `replay`): a completed tool result
// marks the row executed and keeps the result; Google's refusal or a tool
// failure fails it in Google's words; an elicitation or a multi-round-trip
// request fails it too (owner decision D6: until one is captured live,
// Talaria answers none); a 401 releases it — nothing ran, and the next
// approval vends a fresh token. A call that never got an answer FAILS rather
// than releasing: a write may have run, and approving it again could run it
// twice.

use serde_json::{Value, json};
use sqlx::PgPool;
use talaria_google_agent::{
    AgentPrincipal, PrincipalKind, Purpose, Vend, principal_grant, vend_principal_token,
};
use talaria_google_connections::{
    GrantRow, McpRefusal, clear_mcp_flag, forget_access_token, record_mcp_refusal,
};
use talaria_google_errors::quote_raw;
use talaria_google_mcp::replay::{ReplayAnswer, call_tool, stored_result};
use talaria_google_mcp::{Product, pack, policy};
use talaria_secretbox::SecretBox;

use crate::{DecideOutcome, TOKEN_UNAVAILABLE, release_claim, token_refusal_message};

/// The pending-action kind of a held Google Workspace MCP call.
pub const MCP_CALL: &str = "mcp_call";

/// The most of the arguments the summary line shows.
const SUMMARY_ARGS_CHARS: usize = 160;

/// A held call, as its row's payload carries it.
#[derive(Clone, Debug, PartialEq)]
pub struct HeldCall {
    /// The registry row's id and name (`google-<product>`).
    pub server_id: String,
    pub server: String,
    pub product: Product,
    pub tool: String,
    /// The arguments exactly as the agent sent them: an object.
    pub arguments: Value,
}

/// The payload a held call is queued with: which server and tool, the
/// arguments as sent, and what the catalog said of the tool then (its
/// annotations, and when the catalog was listed) for the person deciding.
pub fn held_payload(
    server_id: &str,
    server: &str,
    product: &Product,
    tool: &str,
    arguments: &Value,
    annotations: Option<&Value>,
    catalog_refreshed_ms: Option<i64>,
) -> Value {
    json!({
        "server": server,
        "serverId": server_id,
        "product": product.id,
        "productName": product.name,
        "tool": tool,
        "arguments": arguments,
        "annotations": annotations,
        "catalogRefreshedMs": catalog_refreshed_ms,
    })
}

/// A held call read back from its payload; None when the payload is not one
/// (a missing field, a product the pack does not know, arguments that are
/// not an object).
pub fn read_held(payload: &Value) -> Option<HeldCall> {
    let s = |k: &str| {
        payload
            .get(k)
            .and_then(Value::as_str)
            .filter(|v| !v.is_empty())
            .map(str::to_string)
    };
    let arguments = payload.get("arguments")?;
    if !arguments.is_object() {
        return None;
    }
    Some(HeldCall {
        server_id: s("serverId")?,
        server: s("server")?,
        product: pack::product(&s("product")?)?,
        tool: s("tool")?,
        arguments: arguments.clone(),
    })
}

/// The one line a person reads first: the product, the tool, and the start
/// of the arguments as sent.
pub fn held_summary(product: &Product, tool: &str, arguments: &Value) -> String {
    let args = arguments.to_string();
    let shown: String = args.chars().take(SUMMARY_ARGS_CHARS).collect();
    let more = if args.chars().count() > SUMMARY_ARGS_CHARS {
        "\u{2026}"
    } else {
        ""
    };
    format!("{} \u{b7} {tool} {shown}{more}", product.name)
}

/// Where approved replays go, and on what client. Production is `google()`:
/// the product's endpoint as the code names it, the relay client (no
/// redirects, the dialled address re-checked), and the same address guard
/// the gateway runs.
pub struct McpReplay {
    endpoint: Box<dyn Fn(&Product) -> String + Send + Sync>,
    client: reqwest::Client,
    guard: bool,
}

impl McpReplay {
    /// The production replay.
    pub fn google() -> Self {
        Self {
            endpoint: Box::new(pack::endpoint),
            client: talaria_safe_fetch::relay_client(),
            guard: true,
        }
    }

    /// A replay to `endpoint` on a plain client with no address guard: a
    /// test's loopback stand-in.
    #[cfg(test)]
    pub(crate) fn to(endpoint: String) -> Self {
        Self {
            endpoint: Box::new(move |_| endpoint.clone()),
            client: reqwest::Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .expect("a test client"),
            guard: false,
        }
    }
}

/// Why the replay will not send, before any token is vended: the row's own
/// record, the secret check, the registry row, the catalog, the policy. The
/// sentence always says nothing was sent. Pure over what was read.
pub fn replay_refusal(
    held: &HeldCall,
    row: Option<&RegistryFacts>,
    override_: Option<policy::Policy>,
) -> Option<String> {
    let p = &held.product;
    let nothing = "Nothing was sent to Google's Workspace MCP servers.";
    if talaria_workspace_secrets::mentions_handle(&held.arguments.to_string()) {
        return Some(pack::SECRET_HANDLE_REFUSAL.to_string());
    }
    let Some(row) = row else {
        return Some(format!(
            "Google's {} MCP server was removed from this workspace after this call was queued. {nothing}",
            p.name
        ));
    };
    if row.auth_mode != pack::AUTH_MODE
        || row.name != held.server
        || pack::product_of_server(&row.name) != Some(*p)
    {
        return Some(format!(
            "The registry row this call was queued on is no longer Google's {} MCP server as the pack made it. {nothing}",
            p.name
        ));
    }
    if !row.serving {
        return Some(format!(
            "Google's {} MCP server is turned off in this workspace. {nothing}",
            p.name
        ));
    }
    let entry = policy::catalog_entry(&row.tools, &held.tool);
    if entry.is_none() {
        return Some(pack::absent_sentence(p, &held.tool));
    }
    if policy::policy_for(p.id, &held.tool, entry, override_) == policy::Policy::Deny {
        return Some(pack::denied_sentence(
            p,
            &held.tool,
            !policy::denied_by_name(&held.tool),
        ));
    }
    None
}

/// The registry row a held call names, as the replay re-reads it.
#[derive(Clone, Debug)]
pub struct RegistryFacts {
    pub name: String,
    pub auth_mode: String,
    /// Turned on, and not an app's server switched off.
    pub serving: bool,
    /// The cached catalog.
    pub tools: Value,
}

async fn registry_facts(pg: &PgPool, server_id: &str) -> Result<Option<RegistryFacts>, String> {
    let row: Option<(String, String, bool, Value)> = sqlx::query_as(
        "select name, auth_mode, (enabled and app_off is null), tools \
         from mcp_servers where id = $1::uuid",
    )
    .bind(server_id)
    .fetch_optional(pg)
    .await
    .map_err(|e| format!("mcp_call registry read: {e}"))?;
    Ok(row.map(|(name, auth_mode, serving, tools)| RegistryFacts {
        name,
        auth_mode,
        serving,
        tools,
    }))
}

async fn tool_override(
    pg: &PgPool,
    server_id: &str,
    tool: &str,
) -> Result<Option<policy::Policy>, String> {
    let row: Option<String> = sqlx::query_scalar(
        "select policy from mcp_tool_policies where server_id = $1::uuid and tool = $2",
    )
    .bind(server_id)
    .bind(tool)
    .fetch_optional(pg)
    .await
    .map_err(|e| format!("mcp_call policy read: {e}"))?;
    Ok(row.as_deref().and_then(policy::Policy::parse))
}

/// What one replay answer does to the row: the status the decision answers,
/// and the sentence (Google's words, quoted) — or, for `executed`, the result
/// the row keeps. `release` hands the row back to `pending` (nothing ran);
/// otherwise the row is final. Pure.
#[derive(Clone, Debug, PartialEq)]
pub enum Settle {
    Executed(Value),
    Failed(String),
    Release { status: &'static str, why: String },
}

/// Read a replay answer for the row (see the module header). `p` names the
/// server in the sentences; `answer` is `Err` when no answer came.
pub fn settle(p: &Product, tool: &str, answer: &Result<ReplayAnswer, String>) -> Settle {
    let quoted = |t: &Option<String>| {
        t.as_deref()
            .map(|w| format!(": \u{ab}{w}\u{bb}"))
            .unwrap_or_default()
    };
    match answer {
        Ok(ReplayAnswer::Done(result)) => Settle::Executed(stored_result(result)),
        Ok(ReplayAnswer::Failed { status: 401, text }) => Settle::Release {
            status: "token_refused",
            why: format!(
                "Google's {} MCP server refused the token (HTTP 401){}. Nothing ran; approve again to retry with a fresh token.",
                p.name,
                quoted(text)
            ),
        },
        Ok(ReplayAnswer::Failed { status, text }) => Settle::Failed(format!(
            "Google's {} MCP server refused \u{201c}{tool}\u{201d} (HTTP {status}){}.",
            p.name,
            quoted(text)
        )),
        Ok(ReplayAnswer::AskedForInput { how, text }) => Settle::Failed(format!(
            "Google's {} MCP server asked for more before it would run \u{201c}{tool}\u{201d} ({how}){}. Talaria does not answer those yet, so nothing more was sent and the call did not complete.",
            p.name,
            quoted(text)
        )),
        Ok(ReplayAnswer::Unreadable { status, text }) => Settle::Failed(format!(
            "Google's {} MCP server answered \u{201c}{tool}\u{201d} with something that is not a completed tool result (HTTP {status}){}. It may or may not have run: check in Google before asking again.",
            p.name,
            quoted(text)
        )),
        Err(e) => Settle::Failed(format!(
            "The call to Google's {} MCP server got no answer ({}). It may or may not have run: check in Google before asking again.",
            p.name,
            quote_raw(e)
        )),
    }
}

/// Approve one claimed `mcp_call` row (the row is `executing` under
/// `actor_id`): re-check, vend, replay once, settle. `Err` is a hard failure
/// (a database error) with the row released.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn decide(
    pg: &PgPool,
    sb: &SecretBox,
    action_id: &str,
    actor_id: &str,
    payload: &Value,
    agent_model: Option<&str>,
    owner_user_id: Option<String>,
    principal_kind: &str,
    now_ms: i64,
    replay: &McpReplay,
) -> Result<DecideOutcome, String> {
    let Some(held) = read_held(payload) else {
        return fail(
            pg,
            action_id,
            actor_id,
            "This held call's record cannot be read, so it was not run. Nothing was sent to Google's Workspace MCP servers.",
        )
        .await;
    };
    let p = held.product;
    // Re-read the registry row and its policy. A read that fails hands the
    // row back: nothing ran.
    let facts = match registry_facts(pg, &held.server_id).await {
        Ok(f) => f,
        Err(e) => {
            release_claim(pg, action_id, NOT_CHECKED).await?;
            return Err(e);
        }
    };
    let override_ = match tool_override(pg, &held.server_id, &held.tool).await {
        Ok(o) => o,
        Err(e) => {
            release_claim(pg, action_id, NOT_CHECKED).await?;
            return Err(e);
        }
    };
    if let Some(why) = replay_refusal(&held, facts.as_ref(), override_) {
        return fail(pg, action_id, actor_id, &why).await;
    }

    // The token of the principal the call was queued for — an unknown stored
    // kind is not guessed into the org account here: Google's MCP servers
    // take only the grant the person was shown.
    let Some(kind) = PrincipalKind::parse(principal_kind) else {
        return fail(
            pg,
            action_id,
            actor_id,
            "This held call names no principal Talaria knows, so it was not run. Nothing was sent to Google's Workspace MCP servers.",
        )
        .await;
    };
    let principal = AgentPrincipal::from_kind(kind, owner_user_id);
    let agent_model = agent_model.unwrap_or_default();
    let reconnect = match kind {
        PrincipalKind::Agent => "Reconnect this agent's Google account on its record to run this.",
        PrincipalKind::Org => "Reconnect the org Google account to run this.",
        PrincipalKind::Owner => "Reconnect Google in Settings \u{2192} Connections to run this.",
    };
    let token =
        match vend_principal_token(pg, sb, &principal, agent_model, Purpose::HostedMcp, now_ms)
            .await
        {
            Ok(Vend::Token(g)) => g.token,
            Ok(Vend::NotConnected) => {
                release_claim(pg, action_id, reconnect).await?;
                return Ok(DecideOutcome {
                    status: "not_connected".into(),
                    message: Some(reconnect.to_string()),
                });
            }
            // The door's own sentence: the grant's client is declared not
            // marked for AI agents, and nothing was sent.
            Ok(Vend::NotAgentic(sentence)) => {
                release_claim(pg, action_id, &sentence).await?;
                return Ok(DecideOutcome {
                    status: "not_connected".into(),
                    message: Some(sentence),
                });
            }
            Err(e) => {
                let raw = e.to_string();
                let Some(message) = token_refusal_message(&e, reconnect) else {
                    release_claim(pg, action_id, TOKEN_UNAVAILABLE).await?;
                    return Err(raw);
                };
                tracing::warn!("[google/pending] {action_id} token refused: {raw}");
                release_claim(pg, action_id, &message).await?;
                return Ok(DecideOutcome {
                    status: crate::token_refusal_status(&e).into(),
                    message: Some(message),
                });
            }
        };
    let endpoint = (replay.endpoint)(&p);
    if replay.guard
        && talaria_safe_fetch::assert_fetchable_url(&endpoint)
            .await
            .is_err()
    {
        let why = format!(
            "Talaria refused to dial Google's {} MCP server's address. Nothing was sent to Google's Workspace MCP servers.",
            p.name
        );
        release_claim(pg, action_id, &why).await?;
        return Ok(DecideOutcome {
            status: "not_connected".into(),
            message: Some(why),
        });
    }

    let answer = call_tool(
        &replay.client,
        &endpoint,
        &token,
        &held.tool,
        &held.arguments,
    )
    .await;
    drop(token);
    // What Google's answer says about the grant, as the gateway records it:
    // a 401 or 403 on the grant in Google's words, an acceptance forgets a
    // flag a persisting 401 left.
    let grant = principal_grant(&principal, agent_model);
    if let Some(grant) = grant.as_ref() {
        record_on_grant(pg, grant, &held.tool, &answer, now_ms).await;
    }
    match settle(&p, &held.tool, &answer) {
        Settle::Executed(result) => {
            finish(pg, action_id, actor_id, "executed", &result).await?;
            Ok(DecideOutcome {
                status: "executed".into(),
                message: None,
            })
        }
        Settle::Failed(why) => fail(pg, action_id, actor_id, &why).await,
        Settle::Release { status, why } => {
            release_claim(pg, action_id, &why).await?;
            Ok(DecideOutcome {
                status: status.into(),
                message: Some(why),
            })
        }
    }
}

/// What a released row says when the replay's own re-reads failed (the
/// detail is the log's).
const NOT_CHECKED: &str = "Nothing ran: Talaria could not re-read this server's registry row or policy. Approve again to retry.";

/// Google's answer, on the grant the token came from: a 401 or 403 recorded
/// in Google's words (never flagged here — the gateway's retry decides a
/// flag), a 401 also dropping the cached token so the next vend refreshes;
/// an accepted call forgets a flag. Best effort: the decision stands either
/// way.
async fn record_on_grant(
    pg: &PgPool,
    grant: &GrantRow,
    tool: &str,
    answer: &Result<ReplayAnswer, String>,
    now_ms: i64,
) {
    match answer {
        Ok(ReplayAnswer::Done(_)) => {
            if let Err(e) = clear_mcp_flag(pg, grant).await {
                tracing::warn!("[google/pending] forgetting the grant's flag failed: {e}");
            }
        }
        Ok(ReplayAnswer::Failed { status, text }) if *status == 401 || *status == 403 => {
            let refusal = McpRefusal {
                status: *status,
                text: text.clone(),
                tool: Some(tool.to_string()),
            };
            if let Err(e) = record_mcp_refusal(pg, grant, &refusal, false, now_ms).await {
                tracing::warn!("[google/pending] recording Google's refusal failed: {e}");
            }
            if *status == 401
                && let Err(e) = forget_access_token(pg, grant).await
            {
                tracing::warn!("[google/pending] dropping the cached token failed: {e}");
            }
        }
        _ => {}
    }
}

/// Mark the claimed row final, with what it keeps.
async fn finish(
    pg: &PgPool,
    action_id: &str,
    actor_id: &str,
    status: &str,
    result: &Value,
) -> Result<(), String> {
    sqlx::query(
        "update google_pending_actions \
         set status = $4, result = $2, decided_at = now(), decided_by = $3::uuid \
         where id = $1::uuid and status = 'executing'",
    )
    .bind(action_id)
    .bind(result)
    .bind(actor_id)
    .bind(status)
    .execute(pg)
    .await
    .map_err(|e| format!("google pending action update: {e}"))?;
    Ok(())
}

/// Fail the claimed row with `why` — already in its final words, Google's
/// quoted where Google said anything.
async fn fail(
    pg: &PgPool,
    action_id: &str,
    actor_id: &str,
    why: &str,
) -> Result<DecideOutcome, String> {
    finish(pg, action_id, actor_id, "failed", &json!({ "error": why })).await?;
    Ok(DecideOutcome {
        status: "failed".into(),
        message: Some(why.to_string()),
    })
}

/// The result an executed row kept: what the agent's unlocked call answers
/// with. None for a row that is not executed.
pub async fn executed_result(pg: &PgPool, action_id: &str) -> Result<Option<Value>, String> {
    sqlx::query_scalar(
        "select result from google_pending_actions where id = $1::uuid and status = 'executed'",
    )
    .bind(action_id)
    .fetch_optional(pg)
    .await
    .map(Option::flatten)
    .map_err(|e| format!("google pending action result read: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use talaria_google_mcp::pack::product;

    fn cal() -> Product {
        product("calendar").unwrap()
    }

    fn held(tool: &str, args: Value) -> HeldCall {
        HeldCall {
            server_id: "00000000-0000-4000-8000-000000000001".into(),
            server: "google-calendar".into(),
            product: cal(),
            tool: tool.into(),
            arguments: args,
        }
    }

    fn row(tools: Value) -> RegistryFacts {
        RegistryFacts {
            name: "google-calendar".into(),
            auth_mode: "google".into(),
            serving: true,
            tools,
        }
    }

    fn catalog() -> Value {
        json!([
            { "name": "create_event", "annotations": { "readOnlyHint": false } },
            { "name": "list_events", "annotations": { "readOnlyHint": true } },
            { "name": "trash_event", "annotations": { "destructiveHint": true } },
        ])
    }

    #[test]
    fn a_payload_round_trips_and_a_broken_one_reads_as_none() {
        let p = cal();
        let args = json!({ "summary": "Standup", "attendees": ["a@x.test"] });
        let payload = held_payload(
            "00000000-0000-4000-8000-000000000001",
            "google-calendar",
            &p,
            "create_event",
            &args,
            Some(&json!({ "readOnlyHint": false })),
            Some(1_700_000_000_000),
        );
        assert_eq!(read_held(&payload), Some(held("create_event", args)));
        for broken in [
            json!({}),
            json!({ "serverId": "x", "server": "google-calendar", "product": "classroom", "tool": "t", "arguments": {} }),
            json!({ "serverId": "x", "server": "google-calendar", "product": "calendar", "tool": "t", "arguments": [] }),
            json!({ "serverId": "x", "server": "google-calendar", "product": "calendar", "tool": "", "arguments": {} }),
        ] {
            assert_eq!(read_held(&broken), None, "{broken}");
        }
    }

    #[test]
    fn the_summary_names_the_product_the_tool_and_the_start_of_the_arguments() {
        let s = held_summary(&cal(), "create_event", &json!({ "summary": "Standup" }));
        assert_eq!(
            s,
            "Google Calendar \u{b7} create_event {\"summary\":\"Standup\"}"
        );
        let long = held_summary(
            &cal(),
            "create_event",
            &json!({ "description": "x".repeat(400) }),
        );
        assert!(long.ends_with('\u{2026}'));
        assert!(long.chars().count() < 220);
    }

    /// The replay re-checks the registry row and the policy, every case.
    #[test]
    fn the_replay_rechecks_the_row_and_the_policy_before_sending() {
        let ok = held("create_event", json!({ "summary": "Standup" }));
        assert_eq!(replay_refusal(&ok, Some(&row(catalog())), None), None);
        // An admin's ALLOW or HOLD leaves it runnable; DENY stops it.
        assert_eq!(
            replay_refusal(&ok, Some(&row(catalog())), Some(policy::Policy::Allow)),
            None
        );
        let denied =
            replay_refusal(&ok, Some(&row(catalog())), Some(policy::Policy::Deny)).unwrap();
        assert!(
            denied.contains("turned off for agents here by an admin"),
            "{denied}"
        );
        // The deny list, whatever a row says.
        let trash = held("trash_event", json!({}));
        let floor =
            replay_refusal(&trash, Some(&row(catalog())), Some(policy::Policy::Allow)).unwrap();
        assert!(floor.contains("never run for an agent"), "{floor}");
        // Gone, turned off, no longer the pack's, renamed, product changed.
        assert!(
            replay_refusal(&ok, None, None)
                .unwrap()
                .contains("was removed")
        );
        let mut off = row(catalog());
        off.serving = false;
        assert!(
            replay_refusal(&ok, Some(&off), None)
                .unwrap()
                .contains("turned off")
        );
        let mut org = row(catalog());
        org.auth_mode = "org".into();
        assert!(
            replay_refusal(&ok, Some(&org), None)
                .unwrap()
                .contains("no longer")
        );
        let mut renamed = row(catalog());
        renamed.name = "google-gmail".into();
        assert!(
            replay_refusal(&ok, Some(&renamed), None)
                .unwrap()
                .contains("no longer")
        );
        // The catalog no longer lists the tool.
        let absent = replay_refusal(&ok, Some(&row(json!([]))), None).unwrap();
        assert!(absent.contains("did not list"), "{absent}");
        // A secret handle in the arguments, however it got there.
        let secret = held(
            "create_event",
            json!({ "description": "«secret:deploy.github_pat»" }),
        );
        assert_eq!(
            replay_refusal(&secret, Some(&row(catalog())), None).as_deref(),
            Some(pack::SECRET_HANDLE_REFUSAL)
        );
        // Every refusal says nothing was sent.
        for why in [
            denied,
            floor,
            absent,
            replay_refusal(&ok, None, None).unwrap(),
            replay_refusal(&ok, Some(&off), None).unwrap(),
        ] {
            assert!(why.to_lowercase().contains("nothing was sent"), "{why}");
        }
    }

    #[test]
    fn every_replay_answer_settles_the_row_in_googles_words() {
        let p = cal();
        let done = json!({ "content": [{ "type": "text", "text": "STUB created" }] });
        assert_eq!(
            settle(&p, "create_event", &Ok(ReplayAnswer::Done(done.clone()))),
            Settle::Executed(done)
        );
        // A tool failure, quoted.
        let failed = settle(
            &p,
            "create_event",
            &Ok(ReplayAnswer::Failed {
                status: 200,
                text: Some("Invalid start time.".into()),
            }),
        );
        assert_eq!(
            failed,
            Settle::Failed(
                "Google's Google Calendar MCP server refused \u{201c}create_event\u{201d} (HTTP 200): \u{ab}Invalid start time.\u{bb}.".into()
            )
        );
        // A 401 releases (nothing ran), quoting Google.
        match settle(
            &p,
            "create_event",
            &Ok(ReplayAnswer::Failed {
                status: 401,
                text: Some("Request had invalid authentication credentials.".into()),
            }),
        ) {
            Settle::Release { status, why } => {
                assert_eq!(status, "token_refused");
                assert!(
                    why.contains("\u{ab}Request had invalid authentication credentials.\u{bb}")
                );
                assert!(why.contains("Nothing ran"));
            }
            other => panic!("{other:?}"),
        }
        // A 403 is final.
        assert!(matches!(
            settle(
                &p,
                "create_event",
                &Ok(ReplayAnswer::Failed {
                    status: 403,
                    text: None
                })
            ),
            Settle::Failed(_)
        ));
        // D6: an elicitation fails the row with Google's words.
        match settle(
            &p,
            "create_event",
            &Ok(ReplayAnswer::AskedForInput {
                how: "elicitation/create".into(),
                text: Some("Invite 3 external guests?".into()),
            }),
        ) {
            Settle::Failed(why) => {
                assert!(
                    why.contains("(elicitation/create): \u{ab}Invite 3 external guests?\u{bb}"),
                    "{why}"
                );
                assert!(why.contains("did not complete"));
            }
            other => panic!("{other:?}"),
        }
        // No answer, or an unreadable one: FAILED, never released — a write
        // may have run.
        for answer in [
            Err("the call to https://calendarmcp.googleapis.com/mcp/v1 did not complete (timed out)".to_string()),
            Ok(ReplayAnswer::Unreadable { status: 200, text: Some("<html>".into()) }),
        ] {
            match settle(&p, "create_event", &answer) {
                Settle::Failed(why) => assert!(why.contains("may or may not have run"), "{why}"),
                other => panic!("{other:?}"),
            }
        }
    }
}

/// The replay against Postgres and a loopback stand-in for Google's MCP
/// server (cargo test -p talaria-google-pending --lib -- --ignored; CI runs
/// it against its scratch database). Each test builds its OWN schema with
/// the tables the path touches, copied from the migrated `public` ones, and
/// a stand-in that answers with the shape each case names — Google's
/// captured 401 body, or the MCP specification's result and elicitation
/// shapes (NOT Google captures: no accepted call has been made here).
#[cfg(test)]
mod live {
    use super::*;
    use crate::decide_action_with;
    use sqlx::postgres::{PgConnectOptions, PgPoolOptions};
    use std::collections::HashMap;
    use std::str::FromStr;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    const OWNER: &str = "00000000-0000-4000-8000-0000000000b1";
    const ADMIN: &str = "00000000-0000-4000-8000-0000000000b2";
    const SERVER: &str = "00000000-0000-4000-8000-0000000000c1";
    const BOGUS_BEARER: &str =
        include_str!("../../talaria-google-mcp/fixtures/list-labels-401-bogus.json");

    fn test_box() -> SecretBox {
        let kek = talaria_secretbox::derive_kek("mcp-call-live-test");
        let dek = talaria_secretbox::new_dek().expect("entropy");
        SecretBox::from_parts(kek, HashMap::from([(1u32, dek)]), Some(1))
    }

    /// What the stand-in answers: (HTTP status, content type, body).
    type Reply = (u16, &'static str, String);

    /// A loopback stand-in: every request's head and body kept, each
    /// answered with `reply`'s current value.
    struct Stub {
        url: String,
        seen: Arc<Mutex<Vec<(String, String)>>>,
        reply: Arc<Mutex<Reply>>,
    }

    async fn stub() -> Stub {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/mcp/v1", listener.local_addr().unwrap());
        let seen: Arc<Mutex<Vec<(String, String)>>> = Arc::default();
        let reply: Arc<Mutex<Reply>> =
            Arc::new(Mutex::new((200, "application/json", String::new())));
        let (s2, r2) = (seen.clone(), reply.clone());
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = Vec::new();
                let mut chunk = [0u8; 8192];
                let (head, body) = loop {
                    let n = sock.read(&mut chunk).await.unwrap_or(0);
                    buf.extend_from_slice(&chunk[..n]);
                    let text = String::from_utf8_lossy(&buf).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let len: usize = text[..end]
                            .lines()
                            .find_map(|l| {
                                let (k, v) = l.split_once(':')?;
                                k.eq_ignore_ascii_case("content-length")
                                    .then(|| v.trim().parse().ok())?
                            })
                            .unwrap_or(0);
                        if buf.len() >= end + 4 + len || n == 0 {
                            break (text[..end].to_string(), text[end + 4..].to_string());
                        }
                    } else if n == 0 {
                        break (text, String::new());
                    }
                };
                s2.lock().unwrap().push((head, body));
                let (status, ctype, body) = r2.lock().unwrap().clone();
                let out = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: {ctype}\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = sock.write_all(out.as_bytes()).await;
            }
        });
        Stub { url, seen, reply }
    }

    impl Stub {
        fn answer(&self, status: u16, ctype: &'static str, body: &str) {
            *self.reply.lock().unwrap() = (status, ctype, body.to_string());
        }
        fn calls(&self) -> Vec<(String, Value)> {
            self.seen
                .lock()
                .unwrap()
                .drain(..)
                .map(|(head, body)| (head, serde_json::from_str(&body).unwrap_or(Value::Null)))
                .collect()
        }
    }

    /// A throwaway schema with the tables the replay touches, the pack's
    /// Calendar row (its catalog lists `create_event` and a deny-list tool),
    /// and grants for the owner and the org account whose access caches are
    /// valid, so no refresh reaches Google.
    async fn scratch(tag: &str, sb: &SecretBox) -> (PgPool, PgPool, String) {
        let url = std::env::var("DATABASE_URL")
            .expect("set DATABASE_URL (source ui/.env) to run the ignored live tests");
        let admin = PgPool::connect(&url).await.expect("connect");
        let schema = format!("mcp_call_live_{tag}_{}", std::process::id());
        let mut ddl = vec![
            format!("drop schema if exists {schema} cascade"),
            format!("create schema {schema}"),
            format!(
                "create table {schema}.app_settings (key text primary key, value jsonb not null, \
                 updated_at timestamptz not null default now())"
            ),
            format!(
                "create table {schema}.mcp_tool_policies (server_id uuid not null, tool text not null, \
                 policy text not null check (policy in ('allow','hold','deny')), set_by uuid, \
                 updated_at timestamptz not null default now(), primary key (server_id, tool))"
            ),
        ];
        for table in [
            "google_pending_actions",
            "mcp_servers",
            "google_connections",
            "google_org_connection",
            "agent_google_connections",
        ] {
            ddl.push(format!(
                "create table {schema}.{table} (like public.{table} including all)"
            ));
        }
        for sql in ddl {
            // AssertSqlSafe: the schema is this test's tag and a pid; the
            // tables are the literals above.
            sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
                .execute(&admin)
                .await
                .expect(&sql);
        }
        let opts = PgConnectOptions::from_str(&url)
            .expect("DATABASE_URL parses")
            .options([("search_path", schema.as_str())]);
        let pg = PgPoolOptions::new()
            .max_connections(4)
            .connect_with(opts)
            .await
            .expect("scratch pool");
        sqlx::query(
            "insert into mcp_servers (id, name, label, url, auth_mode, all_agents, enabled, tools, created_by) \
             values ($1::uuid, 'google-calendar', 'Google Calendar (Google MCP, Developer Preview)', \
                     'https://calendarmcp.googleapis.com/mcp/v1', 'google', false, true, $2, 'test')",
        )
        .bind(SERVER)
        .bind(json!([
            { "name": "create_event", "annotations": { "readOnlyHint": false } },
            { "name": "list_events", "annotations": { "readOnlyHint": true } },
            { "name": "delete_event", "annotations": { "destructiveHint": true } },
            { "name": "trash_event", "annotations": { "destructiveHint": true } },
        ]))
        .execute(&pg)
        .await
        .expect("the pack row");
        let later = talaria_agent_auth::epoch_ms_to_iso(talaria_agent_auth::now_ms() + 50 * 60_000);
        sqlx::query(
            "insert into google_connections (user_id, google_sub, email, scope, refresh_token_enc, \
             access_token_enc, access_expires_at) values ($1::uuid, 'g-owner', 'owner@example.test', \
             'https://www.googleapis.com/auth/calendar.events', $2, $3, $4::timestamptz)",
        )
        .bind(OWNER)
        .bind(sb.seal("1//refresh-owner").unwrap())
        .bind(sb.seal("ya29.owner-access").unwrap())
        .bind(&later)
        .execute(&pg)
        .await
        .expect("the owner's grant");
        sqlx::query(
            "insert into google_org_connection (id, google_sub, email, scope, refresh_token_enc, \
             access_token_enc, access_expires_at) values (1, 'g-org', 'org@example.test', \
             'https://www.googleapis.com/auth/calendar', $1, $2, $3::timestamptz)",
        )
        .bind(sb.seal("1//refresh-org").unwrap())
        .bind(sb.seal("ya29.org-access").unwrap())
        .bind(&later)
        .execute(&pg)
        .await
        .expect("the org grant");
        (pg, admin, schema)
    }

    async fn drop_schema(admin: &PgPool, schema: &str) {
        let sql = format!("drop schema if exists {schema} cascade");
        let _ = sqlx::query(sqlx::AssertSqlSafe(sql.as_str()))
            .execute(admin)
            .await;
    }

    /// One held call, queued as the gateway queues it.
    async fn held_row(pg: &PgPool, tool: &str, args: Value, principal: &str) -> String {
        let p = pack::product("calendar").unwrap();
        let payload = held_payload(SERVER, "google-calendar", &p, tool, &args, None, None);
        let (owner, is_org) = match principal {
            "owner" => (Some(OWNER), false),
            _ => (None, true),
        };
        sqlx::query_scalar(
            "insert into google_pending_actions (kind, summary, payload, agent_model, owner_user_id, is_org, principal_kind) \
             values ('mcp_call', $1, $2, 'pa-ops', $3::uuid, $4, $5) returning id::text",
        )
        .bind(held_summary(&p, tool, &args))
        .bind(&payload)
        .bind(owner)
        .bind(is_org)
        .bind(principal)
        .fetch_one(pg)
        .await
        .expect("a held row")
    }

    async fn row_state(pg: &PgPool, id: &str) -> (String, Option<Value>) {
        sqlx::query_as("select status, result from google_pending_actions where id = $1::uuid")
            .bind(id)
            .fetch_one(pg)
            .await
            .expect("the row")
    }

    const RESULT: &str = r#"{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"STUB: event created"}],"structuredContent":{"id":"stub-ev-1"}}}"#;

    #[tokio::test]
    #[ignore = "live DB: set DATABASE_URL"]
    async fn mcp_call_live_an_approved_call_runs_once_as_its_principal() {
        let sb = test_box();
        let (pg, admin, schema) = scratch("approve", &sb).await;
        let stub = stub().await;
        let replay = McpReplay::to(stub.url.clone());
        let now = talaria_agent_auth::now_ms();
        let decide = |id: String, actor: &'static str, is_admin: bool, decision: &'static str| {
            let (pg, sb, replay) = (&pg, &sb, &replay);
            async move {
                decide_action_with(pg, sb, &id, actor, is_admin, decision, now, replay)
                    .await
                    .expect("no hard failure")
                    .expect("the row exists")
            }
        };

        // An owner row: an admin may not decide it; its owner approves, and
        // the call goes to Google ONCE, with the owner's token and the
        // arguments exactly as queued.
        stub.answer(200, "application/json", RESULT);
        let args = json!({ "summary": "Standup", "startTime": "2026-11-18T09:00:00Z" });
        let id = held_row(&pg, "create_event", args.clone(), "owner").await;
        assert_eq!(
            decide(id.clone(), ADMIN, true, "approve").await.status,
            "forbidden"
        );
        assert!(
            stub.calls().is_empty(),
            "nothing sent for a refused decider"
        );
        let out = decide(id.clone(), OWNER, false, "approve").await;
        assert_eq!(out.status, "executed", "{out:?}");
        let calls = stub.calls();
        assert_eq!(calls.len(), 1);
        let (head, rpc) = &calls[0];
        assert!(
            head.lines()
                .any(|l| l.eq_ignore_ascii_case("authorization: Bearer ya29.owner-access")),
            "{head}"
        );
        assert_eq!(rpc["method"], "tools/call");
        assert_eq!(rpc["params"]["name"], "create_event");
        assert_eq!(rpc["params"]["arguments"], args);
        let (status, result) = row_state(&pg, &id).await;
        assert_eq!(status, "executed");
        assert_eq!(result.unwrap()["structuredContent"]["id"], "stub-ev-1");
        assert_eq!(
            executed_result(&pg, &id).await.unwrap().unwrap()["content"][0]["text"],
            "STUB: event created"
        );
        // Deciding it again runs nothing.
        assert_eq!(decide(id, OWNER, false, "approve").await.status, "executed");
        assert!(stub.calls().is_empty());

        // An org row: a member may not decide it; an admin's approval runs
        // it with the org account's token.
        let id = held_row(
            &pg,
            "create_event",
            json!({ "summary": "All hands" }),
            "org",
        )
        .await;
        assert_eq!(
            decide(id.clone(), OWNER, false, "approve").await.status,
            "forbidden"
        );
        assert_eq!(decide(id, ADMIN, true, "approve").await.status, "executed");
        let calls = stub.calls();
        assert_eq!(calls.len(), 1);
        assert!(
            calls[0]
                .0
                .lines()
                .any(|l| l.eq_ignore_ascii_case("authorization: Bearer ya29.org-access"))
        );

        // A rejection sends nothing.
        let id = held_row(&pg, "create_event", json!({}), "owner").await;
        assert_eq!(
            decide(id.clone(), OWNER, false, "reject").await.status,
            "rejected"
        );
        assert!(stub.calls().is_empty());
        assert_eq!(row_state(&pg, &id).await.0, "rejected");
        drop_schema(&admin, &schema).await;
    }

    #[tokio::test]
    #[ignore = "live DB: set DATABASE_URL"]
    async fn mcp_call_live_google_refusals_elicitations_and_rechecks_settle_the_row() {
        let sb = test_box();
        let (pg, admin, schema) = scratch("settle", &sb).await;
        let stub = stub().await;
        let replay = McpReplay::to(stub.url.clone());
        let now = talaria_agent_auth::now_ms();
        let approve = |id: String| {
            let (pg, sb, replay) = (&pg, &sb, &replay);
            async move {
                decide_action_with(pg, sb, &id, OWNER, false, "approve", now, replay)
                    .await
                    .expect("no hard failure")
                    .expect("the row exists")
            }
        };

        // The tool fails: the row fails with the tool's own words.
        stub.answer(
            200,
            "application/json",
            r#"{"jsonrpc":"2.0","id":1,"result":{"content":[{"type":"text","text":"STUB: Invalid start time."}],"isError":true}}"#,
        );
        let id = held_row(&pg, "create_event", json!({}), "owner").await;
        let out = approve(id.clone()).await;
        assert_eq!(out.status, "failed");
        assert!(
            out.message
                .as_deref()
                .unwrap()
                .contains("\u{ab}STUB: Invalid start time.\u{bb}")
        );
        let (status, result) = row_state(&pg, &id).await;
        assert_eq!(status, "failed");
        assert!(
            result.unwrap()["error"]
                .as_str()
                .unwrap()
                .contains("STUB: Invalid start time.")
        );
        assert_eq!(stub.calls().len(), 1);

        // D6: an elicitation fails the row; nothing is answered.
        stub.answer(
            200,
            "text/event-stream",
            "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":\"e1\",\"method\":\"elicitation/create\",\"params\":{\"message\":\"SPEC SHAPE: Invite 3 external guests?\",\"requestedSchema\":{\"type\":\"object\"}}}\n\n",
        );
        let id = held_row(&pg, "create_event", json!({}), "owner").await;
        let out = approve(id.clone()).await;
        assert_eq!(out.status, "failed");
        let why = out.message.unwrap();
        assert!(
            why.contains("(elicitation/create): \u{ab}SPEC SHAPE: Invite 3 external guests?\u{bb}"),
            "{why}"
        );
        assert_eq!(stub.calls().len(), 1, "one call, and no answer to the ask");

        // Google's captured 401: released, nothing ran, the cached token
        // dropped and Google's words on the grant.
        stub.answer(401, "application/json", BOGUS_BEARER);
        let id = held_row(&pg, "create_event", json!({}), "owner").await;
        let out = approve(id.clone()).await;
        assert_eq!(out.status, "token_refused");
        assert!(
            out.message
                .as_deref()
                .unwrap()
                .contains("Request had invalid authentication credentials.")
        );
        let (status, result) = row_state(&pg, &id).await;
        assert_eq!(status, "pending", "a 401 hands the row back");
        assert!(
            result.unwrap()["error"]
                .as_str()
                .unwrap()
                .contains("Nothing ran")
        );
        let (access, refusal): (Option<String>, Option<Value>) = sqlx::query_as(
            "select access_token_enc, last_mcp_refusal from google_connections where user_id = $1::uuid",
        )
        .bind(OWNER)
        .fetch_one(&pg)
        .await
        .unwrap();
        assert_eq!(access, None, "the cached token is dropped");
        let refusal = refusal.unwrap();
        assert_eq!(refusal["status"], 401);
        assert_eq!(refusal["tool"], "create_event");
        assert_eq!(stub.calls().len(), 1);

        // The re-checks, each with NOTHING sent: an admin's DENY, a tool on
        // the deny list whatever a row says, a tool the catalog dropped, a
        // server turned off, a server removed, a secret handle.
        stub.answer(200, "application/json", RESULT);
        // (Re-cache a valid token: the 401 above dropped it.)
        let later = talaria_agent_auth::epoch_ms_to_iso(talaria_agent_auth::now_ms() + 50 * 60_000);
        sqlx::query("update google_connections set access_token_enc = $2, access_expires_at = $3::timestamptz where user_id = $1::uuid")
            .bind(OWNER)
            .bind(sb.seal("ya29.owner-access").unwrap())
            .bind(&later)
            .execute(&pg)
            .await
            .unwrap();
        sqlx::query("insert into mcp_tool_policies (server_id, tool, policy) values ($1::uuid, 'delete_event', 'deny'), ($1::uuid, 'trash_event', 'allow')")
            .bind(SERVER)
            .execute(&pg)
            .await
            .unwrap();
        for (tool, args, says) in [
            (
                "delete_event",
                json!({}),
                "turned off for agents here by an admin",
            ),
            ("trash_event", json!({}), "never run for an agent"),
            ("update_event", json!({}), "did not list"),
            (
                "create_event",
                json!({ "description": "«secret:deploy.github_pat»" }),
                "workspace secret",
            ),
        ] {
            let id = held_row(&pg, tool, args, "owner").await;
            let out = approve(id.clone()).await;
            assert_eq!(out.status, "failed", "{tool}");
            assert!(
                out.message.as_deref().unwrap().contains(says),
                "{tool}: {out:?}"
            );
            assert_eq!(row_state(&pg, &id).await.0, "failed");
        }
        sqlx::query("update mcp_servers set enabled = false where id = $1::uuid")
            .bind(SERVER)
            .execute(&pg)
            .await
            .unwrap();
        let id = held_row(&pg, "create_event", json!({}), "owner").await;
        assert!(approve(id).await.message.unwrap().contains("turned off"));
        sqlx::query("delete from mcp_servers where id = $1::uuid")
            .bind(SERVER)
            .execute(&pg)
            .await
            .unwrap();
        let id = held_row(&pg, "create_event", json!({}), "owner").await;
        assert!(approve(id).await.message.unwrap().contains("was removed"));
        assert!(
            stub.calls().is_empty(),
            "no re-check failure reached the stand-in"
        );
        drop_schema(&admin, &schema).await;
    }

    /// A retried call converges on the row still waiting: one approval can
    /// never run the same call twice. A different argument, tool or agent is
    /// a new ask.
    #[tokio::test]
    #[ignore = "live DB: set DATABASE_URL"]
    async fn mcp_call_live_a_retried_call_converges_on_the_waiting_row() {
        use crate::{QueueAction, queue_action};
        let sb = test_box();
        let (pg, admin, schema) = scratch("dedupe", &sb).await;
        let p = pack::product("calendar").unwrap();
        let queue = |tool: &'static str, args: Value, agent: &'static str| {
            let pg = &pg;
            async move {
                let payload = held_payload(SERVER, "google-calendar", &p, tool, &args, None, None);
                let summary = held_summary(&p, tool, &args);
                queue_action(
                    pg,
                    talaria_realtime::RealtimeDeps::publish_only(None),
                    &QueueAction {
                        kind: MCP_CALL,
                        summary: &summary,
                        payload: &payload,
                        agent_model: agent,
                        owner_user_id: Some(OWNER),
                        is_org: false,
                        principal_kind: "owner",
                    },
                )
                .await
                .expect("queued")
            }
        };
        // Identical calls arriving together: one row, whoever wins the lock.
        let (a, b, c) = tokio::join!(
            queue("create_event", json!({ "summary": "Together" }), "pa-ops"),
            queue("create_event", json!({ "summary": "Together" }), "pa-ops"),
            queue("create_event", json!({ "summary": "Together" }), "pa-ops"),
        );
        assert_eq!(a.action.id, b.action.id);
        assert_eq!(a.action.id, c.action.id);
        assert_eq!(
            [a.already_pending, b.already_pending, c.already_pending]
                .iter()
                .filter(|x| !**x)
                .count(),
            1,
            "exactly one of them inserted"
        );
        let first = queue("create_event", json!({ "summary": "Standup" }), "pa-ops").await;
        assert!(!first.already_pending);
        let again = queue("create_event", json!({ "summary": "Standup" }), "pa-ops").await;
        assert!(again.already_pending);
        assert_eq!(again.action.id, first.action.id);
        for (tool, args, agent) in [
            ("create_event", json!({ "summary": "Retro" }), "pa-ops"),
            ("update_event", json!({ "summary": "Standup" }), "pa-ops"),
            ("create_event", json!({ "summary": "Standup" }), "other-ops"),
        ] {
            let other = queue(tool, args, agent).await;
            assert!(!other.already_pending, "{tool} {agent}");
            assert_ne!(other.action.id, first.action.id);
        }
        // While an approval runs it, a retry converges on it too: no second
        // card whose approval would run the write twice.
        sqlx::query("update google_pending_actions set status = 'executing' where id = $1::uuid")
            .bind(&first.action.id)
            .execute(&pg)
            .await
            .unwrap();
        let running = queue("create_event", json!({ "summary": "Standup" }), "pa-ops").await;
        assert!(running.already_pending);
        assert_eq!(running.action.id, first.action.id);
        assert_eq!(running.action.status, "executing");
        // Once decided, the same call is a fresh ask.
        sqlx::query("update google_pending_actions set status = 'rejected' where id = $1::uuid")
            .bind(&first.action.id)
            .execute(&pg)
            .await
            .unwrap();
        let fresh = queue("create_event", json!({ "summary": "Standup" }), "pa-ops").await;
        assert!(!fresh.already_pending);
        drop_schema(&admin, &schema).await;
    }

    #[tokio::test]
    #[ignore = "live DB: set DATABASE_URL"]
    async fn mcp_call_live_no_token_or_an_unmarked_client_never_reaches_google() {
        let sb = test_box();
        let (pg, admin, schema) = scratch("door", &sb).await;
        let stub = stub().await;
        stub.answer(200, "application/json", RESULT);
        let replay = McpReplay::to(stub.url.clone());
        let now = talaria_agent_auth::now_ms();

        // The owner's grant stamped by a client declared NOT marked for AI
        // agents: the door's sentence, the row back to pending, nothing sent
        // — and never the org account's grant instead.
        sqlx::query("update google_connections set oauth_client_id = 'unmarked.apps.googleusercontent.com' where user_id = $1::uuid")
            .bind(OWNER)
            .execute(&pg)
            .await
            .unwrap();
        sqlx::query("insert into app_settings (key, value) values ('google_client_evidence', $1)")
            .bind(json!({ "unmarked.apps.googleusercontent.com": { "declaredAgentic": false } }))
            .execute(&pg)
            .await
            .unwrap();
        let id = held_row(&pg, "create_event", json!({}), "owner").await;
        let out = decide_action_with(&pg, &sb, &id, OWNER, false, "approve", now, &replay)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(out.status, "not_connected");
        let why = out.message.unwrap();
        assert!(why.contains("unmarked.apps.googleusercontent.com"), "{why}");
        assert!(
            why.contains("Nothing was sent to Google's Workspace MCP servers."),
            "{why}"
        );
        assert_eq!(row_state(&pg, &id).await.0, "pending");

        // The owner disconnected: not connected, nothing sent, and still not
        // the org account.
        sqlx::query("delete from google_connections where user_id = $1::uuid")
            .bind(OWNER)
            .execute(&pg)
            .await
            .unwrap();
        let out = decide_action_with(&pg, &sb, &id, OWNER, false, "approve", now, &replay)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(out.status, "not_connected");
        assert_eq!(row_state(&pg, &id).await.0, "pending");
        assert!(stub.calls().is_empty(), "nothing reached the stand-in");
        drop_schema(&admin, &schema).await;
    }
}
