// The MCP gateway's HOLD branch for Google's Workspace MCP servers (the
// pack's rows, `auth_mode = 'google'`). A `tools/call` whose policy is HOLD
// (talaria-google-mcp's `policy`: every write, every tool Google adds later,
// and any tool an admin set to hold) is NOT relayed. It is queued — the exact
// call, as the agent sent it — in the confirm-sends queue as an `mcp_call`
// (talaria-google-pending), and the agent is answered with a normal tool
// result that names the approval id and says it is not done:
//
//   personal principal → the owner approves; org account or agent identity →
//   an admin approves. Google's MCP servers hear nothing until then, and the
//   approval replays the call once, as that principal, after re-checking the
//   registry row and the pack's policy.
//
// It runs AFTER the pack's own gate (a denied tool, a tool Google's catalog
// does not list, a workspace secret handle in the arguments, a scope the
// grant lacks — all refused, never queued) and AFTER the agent's tool subset
// and method gates, so a hold never widens what the registry granted; and
// BEFORE the credential spend boundary, so nothing is spent into a held call.
//
// THE UNLOCK FAST PATH (owner decision D7). A personal principal's
// conversation that holds an unspent grant for `<server>/<tool>` — left by
// approving the same tool there within the last thirty minutes — runs ONE
// more call without a new card: the row is queued, then approved in the
// owner's name at once, and the agent reads Google's result. An org-account
// or agent-identity call never rides an unlock: an admin decides each one.
//
// The arguments are kept as the agent sent them (plaintext jsonb, readable
// only through the approver's routes, as a drafted email's body is today).
// They are never logged, and the call record keeps only the sentence.

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};
use talaria_agent_auth::{AgentSubject, subject_model};
use talaria_api_facades::google::agent::{
    PrincipalKind, outbound_write_refusal, resolve_agent_principal,
};
use talaria_api_facades::google::mcp::{pack, policy};
use talaria_api_facades::google::pending_actions::{
    MCP_CALL, QueueAction, decide_action, discard_unproven, executed_result, held_payload,
    held_summary, queue_action, queue_proof_failure,
};
use talaria_api_facades::mcp::calls::Outcome;
use talaria_api_facades::mcp::google::GoogleCall;
use talaria_api_facades::mcp::jsonrpc::rpc_error;
use talaria_api_facades::mcp::not_connected::NOT_CONNECTED_CODE;
use talaria_api_facades::mcp::registry::McpServer;
use talaria_error::internal;
use talaria_realtime_watch::RealtimeDeps;
use talaria_secretbox::SecretBox;
use talaria_state::AppState;

/// The most of a held call's arguments the queue keeps (serialized). A
/// drafted email's body is capped at 50 000 characters; a held call carries
/// at most a little more.
const MAX_HELD_ARGUMENTS_BYTES: usize = 64 * 1024;

/// What the hold branch answered, for the call record: the response, the
/// outcome, the HTTP status and the sentence.
pub(super) struct HoldAnswer {
    pub response: Response,
    pub outcome: Outcome,
    pub status: u16,
    pub sentence: Option<String>,
}

/// Is this `tools/call` one the pack HOLDS for a person? Pure: the cached
/// catalog and the admin's overrides on the call the gateway is answering.
pub(super) fn held_tool(rpc: Option<&Value>, server: &McpServer, g: &GoogleCall) -> Option<String> {
    let rpc = rpc?;
    if rpc.get("method").and_then(Value::as_str) != Some("tools/call") {
        return None;
    }
    let tool = rpc.pointer("/params/name").and_then(Value::as_str)?;
    (policy::gateway_policy(g.product.id, tool, &server.tools, &g.overrides)
        == policy::Policy::Hold)
        .then(|| tool.to_string())
}

/// Why a held call's arguments cannot be queued, or None. Pure.
pub(super) fn arguments_refusal(rpc: &Value) -> Option<&'static str> {
    match rpc.pointer("/params/arguments") {
        None | Some(Value::Object(_)) => {}
        Some(_) => {
            return Some(
                "A tool's arguments must be a JSON object. Nothing was queued, and nothing was sent to Google's Workspace MCP servers.",
            );
        }
    }
    let size = rpc
        .pointer("/params/arguments")
        .map(|a| a.to_string().len())
        .unwrap_or(0);
    (size > MAX_HELD_ARGUMENTS_BYTES).then_some(
        "These arguments are too large to hold for approval (over 64 KiB). Nothing was queued, and nothing was sent to Google's Workspace MCP servers.",
    )
}

/// The agent's answer for a held call: a normal tool result (not an error)
/// whose text names the approval id and says it is not done. No
/// `structuredContent`: the gateway drops a held tool's `outputSchema` from
/// the listing, because this is not the shape Google declares.
pub(super) fn held_reply(id: &Value, sentence: &str) -> Value {
    json!({
        "jsonrpc": "2.0",
        "id": id,
        "result": {
            "content": [{ "type": "text", "text": sentence }],
            "isError": false,
        },
    })
}

/// A tool result for an unlocked call: Google's own result when it ran,
/// else the sentence the approval would have shown, as a tool error.
fn unlocked_reply(id: &Value, result: Option<Value>, failed: Option<&str>) -> Value {
    match (result, failed) {
        (Some(result), None) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
        (_, failed) => json!({
            "jsonrpc": "2.0",
            "id": id,
            "result": {
                "content": [{ "type": "text", "text": failed.unwrap_or("The unlocked call did not run.") }],
                "isError": true,
            },
        }),
    }
}

fn answer(body: Value, outcome: Outcome, sentence: Option<String>) -> HoldAnswer {
    HoldAnswer {
        response: (StatusCode::OK, Json(body)).into_response(),
        outcome,
        status: 200,
        sentence,
    }
}

fn refused(id: &Value, code: i64, outcome: Outcome, sentence: &str) -> HoldAnswer {
    answer(
        rpc_error(id, code, sentence),
        outcome,
        Some(sentence.to_string()),
    )
}

fn failed_inside(context: &str, e: impl std::fmt::Display) -> HoldAnswer {
    HoldAnswer {
        response: internal(context, e),
        outcome: Outcome::Error,
        status: 500,
        sentence: Some("the gateway could not hold this call; nothing was sent".into()),
    }
}

/// Hold `tool` (the pack's policy says HOLD) for a person: queue it, prove the
/// approver's queue shows it, land its card, and answer the agent — or, on a
/// personal principal's unspent unlock, run it once now (D7).
pub(super) async fn hold(
    state: &AppState,
    sb: &SecretBox,
    subject: &AgentSubject,
    server: &McpServer,
    g: &GoogleCall,
    rpc: &Value,
    tool: &str,
) -> HoldAnswer {
    let pg = &state.pg;
    let id = rpc.get("id").cloned().unwrap_or(Value::Null);
    let agent_model = subject_model(subject);
    if let Some(why) = arguments_refusal(rpc) {
        return refused(&id, -32602, Outcome::Denied, why);
    }
    let arguments = rpc
        .pointer("/params/arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let principal = match resolve_agent_principal(pg, agent_model).await {
        Ok(p) => p,
        Err(e) => return failed_inside("[mcp/google] principal read failed", e),
    };
    // The row must name the principal whose grant the gateway just vended
    // for: a change in between is answered, never guessed across.
    if principal.kind != g.principal {
        return refused(
            &id,
            -32603,
            Outcome::Error,
            "This agent's Google principal changed while the call was in flight. Nothing was queued, and nothing was sent to Google's Workspace MCP servers; call the tool again.",
        );
    }
    // The REST tools' queue-time refusal, for parity: an owner with no
    // Google of their own, an agent identity not connected.
    match outbound_write_refusal(pg, agent_model).await {
        Ok(Some(why)) => return refused(&id, NOT_CONNECTED_CODE, Outcome::NotConnected, why),
        Ok(None) => {}
        Err(e) => return failed_inside("[mcp/google] principal connection read failed", e),
    }

    let p = g.product;
    let entry = policy::catalog_entry(&server.tools, tool);
    let payload = held_payload(
        &server.id,
        &server.name,
        &p,
        tool,
        &arguments,
        entry.and_then(|e| e.get("annotations")),
        server.tools_refreshed_ms,
    );
    let summary = held_summary(&p, tool, &arguments);
    let redis = state.redis().await.ok();
    let queued = match queue_action(
        pg,
        RealtimeDeps::publish_only(redis.clone()),
        &QueueAction {
            kind: MCP_CALL,
            summary: &summary,
            payload: &payload,
            agent_model,
            owner_user_id: principal.owner_user_id.as_deref(),
            is_org: principal.is_org,
            principal_kind: principal.kind.as_str(),
        },
    )
    .await
    {
        Ok(q) => q,
        Err(e) => return failed_inside("[mcp/google] hold queue failed", e),
    };
    let action_id = queued.action.id.clone();

    // D7: one more call of a tool its owner approved in this conversation in
    // the last half hour runs now — a personal principal only, and only
    // while a grant for exactly this server and tool is unspent. The grant
    // is spent under a row lock, after the row exists, so a call nobody
    // could run does not burn it.
    if principal.kind == PrincipalKind::Owner
        && let Some(owner) = principal.owner_user_id.as_deref()
        && let Ok(Some(conv)) = talaria_chips::live_conversation_id(pg, agent_model).await
        && talaria_chips::tool_unlocked(
            pg,
            &conv,
            &talaria_chips::mcp_unlock_key(&server.name, tool),
            &[],
        )
        .await
        .unwrap_or(false)
    {
        tracing::info!(
            "[mcp/google] {agent_model}: {}/{tool} rides its owner's unlock ({action_id})",
            server.name
        );
        let now = talaria_agent_auth::now_ms();
        return match decide_action(pg, sb, &action_id, owner, false, "approve", now).await {
            Ok(Some(outcome)) if outcome.status == "executed" => {
                match executed_result(pg, &action_id).await {
                    Ok(result) => answer(unlocked_reply(&id, result, None), Outcome::Ok, None),
                    Err(e) => failed_inside("[mcp/google] unlocked result read failed", e),
                }
            }
            Ok(Some(outcome)) => {
                let mut why = format!("Unlocked {tool} did not run ({})", outcome.status);
                if let Some(m) = outcome.message.as_deref().filter(|m| !m.is_empty()) {
                    why.push_str(": ");
                    why.push_str(m);
                }
                if outcome.released() {
                    why.push_str(&format!(
                        " It is back in the approval queue as {action_id}; read_pending_send shows why as lastError. Do not report it as done."
                    ));
                }
                answer(
                    unlocked_reply(&id, None, Some(&why)),
                    Outcome::Error,
                    Some(why.clone()),
                )
            }
            Ok(None) => refused(
                &id,
                -32603,
                Outcome::Error,
                "The unlocked call's approval row disappeared before it could run. Do not report it as done.",
            ),
            Err(e) => failed_inside("[mcp/google] unlocked call failed", e),
        };
    }

    // Not ready until the approver's own queue shows it.
    match queue_proof_failure(pg, &queued.action).await {
        Ok(None) => {}
        Ok(Some(reason)) => {
            if !queued.already_pending
                && let Err(e) = discard_unproven(pg, &action_id).await
            {
                return failed_inside("[mcp/google] unproven hold discard failed", e);
            }
            return refused(&id, -32603, Outcome::Error, &reason);
        }
        Err(e) => return failed_inside("[mcp/google] hold queue proof failed", e),
    }
    // The card, where the agent is speaking. An MCP tool's name reaches the
    // tool-events door prefixed by the agent's MCP client, so the chip is
    // landed here; the queue, the inbox and the brief show the row either way.
    let chip = talaria_chips::mcp_call_chip(&action_id, &summary);
    if let Err(e) = talaria_chips::surface_tool_chips(
        pg,
        redis,
        agent_model,
        principal.owner_user_id.as_deref(),
        vec![chip],
    )
    .await
    {
        tracing::warn!("[mcp/google] {action_id}: the approval chip did not land: {e}");
    }
    let sentence = pack::held_sentence(
        &p,
        tool,
        &action_id,
        principal.is_org,
        queued.already_pending,
    );
    tracing::info!(
        "[mcp/google] {agent_model}: {}/{tool} held as {action_id}{}",
        server.name,
        if queued.already_pending {
            " (already waiting)"
        } else {
            ""
        }
    );
    answer(held_reply(&id, &sentence), Outcome::Held, Some(sentence))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_held_answer_is_a_tool_result_naming_the_id_never_an_error() {
        let reply = held_reply(&json!(7), "Queued for approval as p-1. Not done yet.");
        assert_eq!(reply["id"], 7);
        assert_eq!(reply["result"]["isError"], false);
        assert_eq!(
            reply["result"]["content"][0]["text"],
            "Queued for approval as p-1. Not done yet."
        );
        assert!(reply.get("error").is_none());
        assert!(reply["result"].get("structuredContent").is_none());
    }

    #[test]
    fn held_arguments_must_be_an_object_of_bounded_size() {
        let call = |args: Value| json!({ "method": "tools/call", "params": { "name": "x", "arguments": args } });
        assert_eq!(arguments_refusal(&call(json!({ "a": 1 }))), None);
        assert_eq!(
            arguments_refusal(&json!({ "method": "tools/call", "params": { "name": "x" } })),
            None,
            "absent arguments are an empty object"
        );
        for bad in [json!([1]), json!("x"), json!(3), Value::Null] {
            assert!(arguments_refusal(&call(bad.clone())).is_some(), "{bad}");
        }
        let big = json!({ "body": "x".repeat(MAX_HELD_ARGUMENTS_BYTES) });
        assert!(arguments_refusal(&call(big)).unwrap().contains("too large"));
    }

    #[test]
    fn an_unlocked_call_answers_googles_result_or_the_approvals_sentence() {
        let result = json!({ "content": [{ "type": "text", "text": "STUB" }] });
        let ok = unlocked_reply(&json!(1), Some(result.clone()), None);
        assert_eq!(ok["result"], result);
        let bad = unlocked_reply(
            &json!(1),
            None,
            Some("Unlocked create_event did not run (failed)"),
        );
        assert_eq!(bad["result"]["isError"], true);
        assert_eq!(
            bad["result"]["content"][0]["text"],
            "Unlocked create_event did not run (failed)"
        );
    }
}
