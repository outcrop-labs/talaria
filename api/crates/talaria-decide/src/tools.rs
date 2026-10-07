// TOOL-OFFER PRUNING, MEASURED BEFORE ANYTHING PRUNES.
//
// THE IDEA. The agent toolkit is 81 tools, and every agent turn carries the
// definitions it was offered in its request. Those definitions are input
// tokens on every turn of every session, whether or not the turn had any use
// for `cancel_google_event`. A decision model could pick the plausibly-useful
// subset in one round trip and the turn would carry a fraction of the
// descriptions.
//
// WHY THIS FILE PRUNES NOTHING. The failure mode is specific and bad: prune
// away a tool the agent then needed and the turn breaks, in a way that reads
// as the model being stupid rather than as the gateway having hidden its
// hands. Nobody should accept that risk on the strength of an argument, so
// this measures instead.
//
// THE MEASUREMENT IS NOT "DID THE MODEL AGREE". There is no baseline opinion
// to agree with — production offers everything. The question that matters is
// WOULD PRUNING HAVE BROKEN THIS TURN: judge the offered tools, compute the
// subset that would have survived, and then check it against the tools the
// reply ACTUALLY called. A keep-set that contains every called tool is a safe
// prune; one that misses even a single called tool is the failure, and it is
// recorded as a disagreement. Reading `gap-align`-style agreement rates per
// caller is what would eventually justify turning pruning on — per caller,
// because a Workbench session and a personal assistant do not have the same
// tool spread and should not be judged by one number.
//
// COST, STATED. One Noul per offered tool. Jev fans those out into a single
// request, but it is still a large one, and measuring a token saving by
// spending tokens is only defensible while it is temporary and bounded. Hence
// the cap below, the `off` default, and its own setting rather than riding the
// port's: an operator turning on a decision model has not thereby agreed to
// put every agent turn through a 60-question request.

use serde_json::{Value, json};
use talaria_gateway::settings::get_setting_hot;
use talaria_retrieval_http::HttpFetch;
use talaria_state::AppState;

use crate::{Ask, Question, decide, shadow};

/// Its own switch, default off. See the header: configuring a decision model
/// is not consent to this.
pub const SETTING: &str = "decide_tool_shadow";

/// Is the measurement on? Exposed so the admin surface reads the same key the
/// pass does, rather than a second spelling of it.
pub async fn shadow_enabled(pg: &sqlx::PgPool) -> bool {
    get_setting_hot(pg, SETTING, json!(false))
        .await
        .as_bool()
        .unwrap_or(false)
}

/// The most tools judged in one turn. Above the real toolkit's size on
/// purpose — the catalogue is 81 today and an agent is offered some subset of
/// it, so a cap at the current count would silently skip exactly the turns
/// worth measuring. A request past this is SKIPPED rather than truncated:
/// judging a subset and reporting "the prune would have been safe" would be a
/// lie about which tools were weighed.
const MAX_TOOLS: usize = 120;

/// Keep anything that might plausibly be wanted. Deliberately low — this is
/// the prune's own threshold, and the asymmetry is the sharpest on the port:
/// keeping a tool nobody needed costs a few hundred input tokens, while
/// dropping one that was needed breaks the turn. A measurement run at a
/// permissive floor tells us whether even the generous version is safe.
///
/// The number and that reasoning live in the site census, so the ledger and
/// the threshold it is measured at cannot drift apart: an operator who moves
/// this is moving what the next rows MEAN, and reading them beside the number
/// is the only way to notice.
///
/// The census id this pass records and reads its floor under.
pub const SITE: &str = "tool-prune";

/// What the gateway hands over: a tool's name and the description the model
/// was shown, which is what a judgment has to weigh.
pub struct OfferedTool {
    pub name: String,
    pub description: String,
}

/// Read the `tools` array off a chat-completions request body. Tolerant by
/// construction: a body with no tools, or tools in a shape this does not
/// recognize, yields an empty list and the pass does nothing.
pub fn offered_tools(body: &Value) -> Vec<OfferedTool> {
    body.get("tools")
        .and_then(|t| t.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|t| {
                    let f = t.get("function").unwrap_or(t);
                    Some(OfferedTool {
                        name: f.get("name")?.as_str()?.to_string(),
                        description: f
                            .get("description")
                            .and_then(|d| d.as_str())
                            .unwrap_or_default()
                            .to_string(),
                    })
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The tool names a reply actually called. Both shapes the wire uses: the
/// modern `tool_calls[].function.name` and the retired `function_call.name`,
/// because a gateway sees whatever an upstream still speaks.
pub fn called_tools(reply: &Value) -> Vec<String> {
    let msg = &reply["choices"][0]["message"];
    let mut out: Vec<String> = msg
        .get("tool_calls")
        .and_then(|c| c.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|c| {
                    c.get("function")
                        .and_then(|f| f.get("name"))
                        .and_then(|n| n.as_str())
                        .map(str::to_string)
                })
                .collect()
        })
        .unwrap_or_default();
    if let Some(n) = msg
        .get("function_call")
        .and_then(|f| f.get("name"))
        .and_then(|n| n.as_str())
    {
        out.push(n.to_string());
    }
    out
}

/// The request as the judgment sees it: the last user turn, which is what the
/// tools would be chosen for. Earlier turns are deliberately left out — a
/// session that once used Drive should not keep every Drive tool alive forever,
/// and whether that is the right call is exactly what the measurement answers.
fn request_subject(messages: &[Value]) -> Option<Value> {
    let last_user = messages
        .iter()
        .rev()
        .find(|m| m.get("role").and_then(|r| r.as_str()) == Some("user"))?
        .get("content")
        .and_then(|c| c.as_str())?
        .to_string();
    (!last_user.trim().is_empty()).then(|| json!({ "request": last_user }))
}

/// Judge the offered tools and record whether pruning would have been safe.
/// Returns nothing: this exists to measure, and a caller that could read it
/// might be tempted to act on it.
pub fn observe(state: &AppState, body: &Value, reply: &Value, caller: &str) {
    let (state, body, reply, caller) = (
        state.clone(),
        body.clone(),
        reply.clone(),
        caller.to_string(),
    );
    tokio::spawn(async move {
        if !shadow_enabled(&state.pg).await {
            return;
        }
        let offered = offered_tools(&body);
        if offered.is_empty() || offered.len() > MAX_TOOLS {
            return;
        }
        let empty = Vec::new();
        let messages = body
            .get("messages")
            .and_then(|m| m.as_array())
            .unwrap_or(&empty);
        let Some(subject) = request_subject(messages) else {
            return;
        };
        let provider = crate::get_decide_config(&state.pg).await.provider;
        let http = talaria_retrieval_http::real_http();
        let Some(kept) = keep_set(&state, &http, subject, &offered).await else {
            return;
        };

        // THE ONE NUMBER THIS SITE EXISTS FOR: did every tool the reply
        // actually called survive the prune? A miss is the failure that would
        // have broken the turn.
        let called = called_tools(&reply);
        let safe = called.iter().all(|c| kept.contains(c));
        shadow::record_derived(
            &state.pg,
            shadow::Derived {
                site: SITE,
                // The caller, not the turn: a Workbench session and a personal
                // assistant have different tool spreads and should not be
                // judged by one number.
                subject_ref: Some(&caller),
                baseline: &format!("offered:{}", offered.len()),
                port_answer: &format!("kept:{} called:{}", kept.len(), called.len()),
                agreed: safe,
                provider: &provider,
                latency_ms: None,
            },
        )
        .await;
    });
}

/// The tools that would survive, by name. None when the port has nothing.
async fn keep_set(
    state: &AppState,
    http: &HttpFetch,
    subject: Value,
    offered: &[OfferedTool],
) -> Option<Vec<String>> {
    let mut ask = Ask::new(subject);
    for (i, t) in offered.iter().enumerate() {
        ask = ask.q(
            i.to_string(),
            Question::noul_meaning(
                format!(
                    "Could the tool below plausibly be needed to serve `request`? THE TOOL: {} — {}",
                    t.name, t.description
                ),
                "the request might call for this tool, directly or as a step toward what it asks",
                "the request has no use for this tool at all",
            ),
        );
    }
    let answers = decide(state, http, &ask).await?;
    // Read once rather than per tool: a hundred-tool turn asking the census a
    // hundred times would be a hundred identical reads.
    let gate = crate::sites::gate_of(&state.pg, SITE).await;
    let floor = gate.floor;
    Some(
        offered
            .iter()
            .enumerate()
            .filter(|(i, _)| {
                answers
                    .get(&i.to_string())
                    .filter(|j| j.calibrated)
                    .and_then(|j| j.answer.probability())
                    .is_some_and(|p| p >= floor)
            })
            .map(|(_, t)| t.name.clone())
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tools_are_read_from_either_the_wrapped_or_the_bare_shape() {
        // OpenAI wraps each tool in `function`; some upstreams send it bare,
        // and a gateway sees whatever is actually spoken.
        let wrapped = json!({ "tools": [
            { "type": "function", "function": { "name": "read_file", "description": "Read a file" } }
        ]});
        let got = offered_tools(&wrapped);
        assert_eq!(got.len(), 1);
        assert_eq!(got[0].name, "read_file");
        assert_eq!(got[0].description, "Read a file");

        let bare = json!({ "tools": [{ "name": "read_file", "description": "Read a file" }] });
        assert_eq!(offered_tools(&bare).len(), 1);
    }

    #[test]
    fn a_body_with_no_tools_or_an_unrecognized_shape_yields_nothing() {
        // Tolerant on purpose: this pass must never be the reason a completion
        // path misbehaves, so an unreadable shape means it does nothing.
        assert!(offered_tools(&json!({})).is_empty());
        assert!(offered_tools(&json!({ "tools": "all of them" })).is_empty());
        // A tool with no name cannot be judged or matched against a call.
        assert!(offered_tools(&json!({ "tools": [{ "description": "x" }] })).is_empty());
        // A missing description still judges — on the name alone.
        let no_desc = offered_tools(&json!({ "tools": [{ "name": "whoami" }] }));
        assert_eq!(no_desc.len(), 1);
        assert_eq!(no_desc[0].description, "");
    }

    #[test]
    fn called_tools_reads_both_the_modern_and_the_retired_wire_shape() {
        let modern = json!({ "choices": [{ "message": { "tool_calls": [
            { "function": { "name": "read_file" } },
            { "function": { "name": "list_tickets" } }
        ]}}]});
        assert_eq!(called_tools(&modern), vec!["read_file", "list_tickets"]);

        let retired = json!({ "choices": [{ "message": {
            "function_call": { "name": "read_file" }
        }}]});
        assert_eq!(called_tools(&retired), vec!["read_file"]);

        // A reply that called nothing is the ordinary case, not an error.
        let text_only = json!({ "choices": [{ "message": { "content": "hello" } }]});
        assert!(called_tools(&text_only).is_empty());
    }

    #[test]
    fn the_subject_is_the_last_user_turn_not_the_whole_transcript() {
        // A session that once used Drive should not keep every Drive tool
        // alive forever. Whether that is right is what the measurement
        // answers — but it is the thing being measured, so it has to be the
        // thing implemented.
        let msgs = vec![
            json!({ "role": "user", "content": "find the Q3 deck" }),
            json!({ "role": "assistant", "content": "here" }),
            json!({ "role": "user", "content": "now rotate the staging key" }),
        ];
        let s = request_subject(&msgs).expect("a user turn");
        assert_eq!(s["request"], json!("now rotate the staging key"));

        // No user turn, or an empty one, is nothing to judge against.
        assert!(request_subject(&[json!({ "role": "system", "content": "x" })]).is_none());
        assert!(request_subject(&[json!({ "role": "user", "content": "   " })]).is_none());
    }

    #[test]
    fn the_keep_floor_is_permissive_because_dropping_a_needed_tool_breaks_the_turn() {
        // The sharpest asymmetry on the port: keeping a tool nobody needed
        // costs a few hundred input tokens; dropping one that was needed
        // breaks the turn and reads as the model being stupid. A measurement
        // at a generous floor answers whether even the generous version is
        // safe — a strict one would only prove that strictness is unsafe.
        // The same floor production reads, from the same place — a literal
        // here would pass while the pass itself kept a different bar.
        let floor = crate::sites::def_of(SITE)
            .expect("tool-prune is in the census")
            .default_floor;
        let keeps = |p: f64, calibrated: bool| calibrated && p >= floor;
        assert!(keeps(0.15, true), "at the floor");
        assert!(keeps(0.2, true), "a weak maybe still keeps");
        assert!(!keeps(0.14, true));
        assert!(
            !keeps(0.99, false),
            "uncalibrated keeps nothing — and so prunes nothing"
        );
    }

    #[test]
    fn the_safety_verdict_is_whether_every_called_tool_survived() {
        let safe = |kept: &[&str], called: &[&str]| called.iter().all(|c| kept.contains(c));
        assert!(safe(&["read_file", "whoami"], &["read_file"]));
        assert!(
            safe(&["read_file"], &[]),
            "a turn that called nothing cannot be broken"
        );
        // The failure: the prune dropped something the model then reached for.
        assert!(!safe(&["whoami"], &["read_file"]));
        assert!(!safe(&["read_file"], &["read_file", "list_tickets"]));
    }
}
