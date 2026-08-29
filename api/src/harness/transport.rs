// THE TRANSPORTS, and the request that reaches them — the port of
// harness/transport.ts. This slice is the CONTRACT LAYER: the request and
// reply types, the one mapping onto a persona payload, the `response_format`
// derivation, the tool-channel wire renderer, and the refusal sentences. The
// async transports (the gateway turn, the streamed pair, the fleet turn and
// the picker) land with the runner, over the gateway pieces that already
// crossed (`build_upstream`, `fetch_upstream`, `record_gateway_usage`).
//
// WHY THIS FILE EXISTS — ONE MISSING FEATURE THAT WORE FIVE COATS. Five files
// USED TO hand-write their own persona transport because `runHarness` could
// not serve them, and four agents working independently converged on the same
// four asks:
//
//   work-dispatch.ts     `sessionTransport`          tools + taskId + a 10-minute hold
//   briefing.ts          a tee transport             tools + streaming
//   outreach.ts          `personaTurnWithOwnTools`   tools
//   plan-persona-turn.ts `planPersonaTransport`      refId + tier + tier routing
//   routes/api/muse.ts   a replay transport          streaming
//
// Every one of them was the fleet transport differing on an axis
// `TransportRequest` had no slot for. Those slots are here now: `tools`,
// `ledger`, `hold_ms`, and (for the streaming pair) the streaming transport
// type. AND THE SIXTH ASK, which is `tool_defs` + `TransportReply.tool_calls`
// — without the slot, the `tool-select` probe could not run, could not write
// its fact, and the capability-gated widening it gates could never fire in
// production.
//
// AND THE REASON THE SHIMS WERE A PROBLEM RATHER THAN A STYLE COMPLAINT:
// THREE OF THE FIVE SILENTLY DROPPED `req.temperature` AND `req.json_mode`. A
// harness that declared `temperature: 0` ran at the provider's default and
// nothing anywhere said so — the exact class of failure the whole harness
// layer exists to end, reintroduced by the workaround for the harness layer.
// So the mapping from a request onto a persona payload lives in ONE function
// here, and a transport that genuinely CANNOT honor a field says so by
// failing the call rather than by ignoring it.

use super::define::Message;
use super::json_schema::WireSchema;
use serde_json::{Map, Value};

/// Which side of the house a turn ran on. The runner records it; the guard
/// pass is told what the transport could honestly observe on each.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    Gateway,
    Fleet,
}

impl TransportKind {
    pub fn as_str(self) -> &'static str {
        match self {
            TransportKind::Gateway => "gateway",
            TransportKind::Fleet => "fleet",
        }
    }
}

/// Whether the model may use ITS OWN tools on this turn.
///
/// `None` (the request default, spelled `None` rather than a `None` variant) is
/// right for every single-shot structured harness: the runner asks one
/// question and parses one answer, so a tool call can only be the model
/// wandering off. It is sent as an OpenAI-level `tools: []` /
/// `tool_choice: 'none'` rather than left to the prompt.
///
/// `Own` is for the three turns whose whole FEATURE is the tool loop — an
/// agent working a ticket, an outreach check-in that acts through
/// `message_user`, a briefing follow-up the owner expects to answer "what's
/// blocking t-41?" from live data. On those, suppressing tools does not merely
/// weaken the answer: it disarms the agent and then trips `zero_tool_claim`
/// for having called no tool.
///
/// IT IS NOT THE SAME QUESTION AS `TransportRequest.tool_defs`, and conflating
/// the two would have been the easy mistake. This policy is about tools WE DO
/// NOT HOLD — a persona's loop runs inside the agent container, so all we can
/// do is suppress it at the protocol level and read the names the stream
/// reports. `tool_defs` is about tools we hand over ourselves and watch being
/// called. A turn may legitimately offer definitions while still refusing the
/// model its own loop; that is two sentences about two different sets of
/// tools.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolPolicy {
    None,
    Own,
}

impl ToolPolicy {
    pub fn as_str(self) -> &'static str {
        match self {
            ToolPolicy::None => "none",
            ToolPolicy::Own => "own",
        }
    }
}

/// One tool we OFFER the model for a single turn: the OpenAI function shape
/// without the envelope, which every transport that can serve it puts back on.
#[derive(Debug, Clone)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    /// JSON Schema for the arguments, exactly as the provider expects it.
    pub parameters: Value,
}

/// A tool call the model MADE, as the provider reported it.
#[derive(Debug, Clone, PartialEq)]
pub struct ToolCall {
    pub name: String,
    /// THE CORRELATION ID. A tool RESULT has to name the call it answers
    /// (`tool_call_id`), so this is what ties the two halves of a tool round
    /// together.
    ///
    /// ASSIGNED ONCE, IN `read_tool_calls`, whether or not the provider sent
    /// one — and that is the whole point. It used to be left absent when a
    /// provider omitted it, on the theory that a missing id beats a fabricated
    /// one, and every consumer then fabricated its OWN: the wire renderer
    /// wrote `call_0` into the assistant turn while a caller wrote the tool
    /// NAME into the result. One fact, two spellings, and the replay referred
    /// to a call the provider had never been shown:
    ///
    ///   messages.3.tool.tool_call_id: Field required
    ///
    /// It cost every tool-loop harness on Anthropic and OpenAI. A fabricated
    /// id is fine; two of them is not, so there is exactly one place that
    /// invents one. Still optional because a hand-built `Message` may not
    /// carry one — but nothing that comes off the wire is missing it.
    pub id: Option<String>,
    /// The raw JSON arguments string, verbatim. Kept unparsed because "the
    /// model called the right tool with arguments that are not JSON" is a
    /// distinct and real observation, and parsing here would turn it into a
    /// failure.
    pub args: String,
}

/// Where this turn's spend lands in `usage_events`.
///
/// The runner used to meter every persona turn as a plain chat turn belonging
/// to nothing, because no other harness had anything to attribute to. Three
/// call sites did, and each lost something real:
///
///   task_id    a work-session turn's spend lands in the ledger but never in
///              the TICKET'S cost, which is the number a ticket owner reads.
///   ref_id +   research's persona stages metered with no run id, so a run's
///   source     cost stopped being answerable at all.
///   tier       the ledger prices a row by looking up the agent's base def
///              and then the alias named by `tier`. Hand it the ROUTED id
///              with a null tier and both lookups miss: the row is attributed
///              to an agent that does not exist, with no endpoint class,
///              which means no price. A tier draft becomes free.
///
/// Hence `agent_model` is the BASE persona id and `tier` is the alias NAME,
/// never the routed id — `TransportRequest.model` is the id actually called.
#[derive(Debug, Clone)]
pub struct LedgerAttribution {
    /// The agent as the ledger names it: the base persona id, never a tier id.
    pub agent_model: String,
    pub source: LedgerSource,
    /// The conversation, channel or research run this spend belongs to.
    pub ref_id: Option<String>,
    /// The ticket this spend belongs to, so it reaches the ticket's cost.
    pub task_id: Option<String>,
    /// The alias NAME (`opus`), as the ledger's classifier wants it.
    pub tier: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LedgerSource {
    Chat,
    Channel,
    Ticket,
    Research,
}

impl LedgerSource {
    pub fn as_str(self) -> &'static str {
        match self {
            LedgerSource::Chat => "chat",
            LedgerSource::Channel => "channel",
            LedgerSource::Ticket => "ticket",
            LedgerSource::Research => "research",
        }
    }
}

/// Prompt/completion token counts as the transports report them (None when
/// the provider sent no usage block — the caller estimates).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TokenPair {
    pub prompt_tokens: i64,
    pub completion_tokens: i64,
}

/// One turn, as the runner hands it to a transport. Every field is spent by
/// the mapping below or by the transports — see the module header for the
/// three shims that dropped fields because there was nowhere to put them.
pub struct TransportRequest {
    /// The id actually called — a tier id when a tier was routed.
    pub model: String,
    pub messages: Vec<Message>,
    pub temperature: Option<f64>,
    /// Ask for structured output at the PROTOCOL level (response_format).
    /// False means the runner has already anchored the instruction in the
    /// prompt instead — see the runner's `anchor_json`.
    pub json_mode: bool,
    /// THE HARNESS'S OWN SCHEMA, when this build could render one. Present,
    /// the request carries `response_format: {type:'json_schema',…}` and the
    /// provider constrains decoding to the shape; absent, it falls back to
    /// the loose `{type:'json_object'}`.
    ///
    /// Only the fallback is optional — every JSON harness Talaria ships
    /// renders a schema, so the loose form is reserved for a definition this
    /// build cannot express. It matters because the loose form is not merely
    /// weaker: Anthropic's compat layer REJECTS it.
    pub json_schema: Option<WireSchema>,
    /// Absent means the policy is `None`. Read through `tool_policy_of`,
    /// never by hand.
    pub tools: Option<ToolPolicy>,
    /// Tool DEFINITIONS this turn offers the model — OURS, not its own. See
    /// the note on `ToolPolicy` for why this is a separate slot rather than a
    /// wider policy: the policy governs a loop we do not hold, this governs
    /// one we do.
    ///
    /// A TRANSPORT THAT CANNOT OFFER THEM FAILS THE CALL — the fleet refusal
    /// when the loop is the agent's, the dropped-parameter refusal when the
    /// gateway stripped `tools` on the way out. Same rule as
    /// `gateway_tools_refusal`, for a sharper reason: a probe scored on a
    /// question the model was never asked writes a capability fact that never
    /// expires.
    pub tool_defs: Vec<ToolDefinition>,
    /// Absent means the default persona attribution. Read through
    /// `ledger_of`, never by hand.
    pub ledger: Option<LedgerAttribution>,
    /// THE REASONING EFFORT THIS TURN RUNS AT — a human's pick from the
    /// levels the model's metadata vouches for. Sent as `reasoning_effort`
    /// wherever the turn can honor it, and dropped without a murmur where it
    /// cannot: a TOOL-OFFERING gateway turn must keep its `'none'` (the
    /// provider rejects the combination), and a field the request cannot
    /// honor is not a reason to fail a chat.
    ///
    /// Unlike `temperature` there is no "silently dropped" hazard to defend
    /// against: effort is a dial, not a contract — a turn that runs at the
    /// model's default effort still answers the question it was asked.
    pub effort: Option<String>,
    /// How long a persona transport may HOLD for an agent that is not
    /// answering yet, in ms. The proxy defaults to two minutes; a restarting
    /// agent under a config propagation refuses connections for tens of
    /// seconds, and a work session must survive a fleet re-render mid-run.
    /// Meaningless on the gateway path — there is no container to wait for —
    /// which is why ignoring it there is not a dropped field.
    pub hold_ms: Option<u64>,
    pub caller: String,
}

impl TransportRequest {
    /// Every message's content, concatenated — the character count both
    /// metering paths estimate the prompt from.
    pub fn prompt_chars(&self) -> usize {
        self.messages.iter().map(|m| m.content.len()).sum()
    }
}

/// What a transport hands back. The tool fields are the honest-observation
/// contract: `tool_names` is what every transport can report (the fleet path
/// sees names only); `tool_calls` is filled only by a transport that ran the
/// loop ITSELF.
///
/// `tool_calls: None` IS NOT `Some(vec![])`, and the difference is what the
/// tool probes score. None means nobody was in a position to observe a call
/// (a persona's loop, a plain completion that offered nothing); empty means
/// we offered tools and the model called none, which is a failed trial rather
/// than a missing measurement.
#[derive(Debug, Clone)]
pub struct TransportReply {
    pub kind: TransportKind,
    pub text: String,
    /// The tool NAMES this turn produced. On the fleet path that is all there
    /// is; on the gateway path it is `tool_calls` with the arguments dropped,
    /// derived through `tool_names_of` so the two cannot drift apart.
    pub tool_names: Vec<String>,
    pub tool_calls: Option<Vec<ToolCall>>,
    pub usage: Option<TokenPair>,
    /// The call asked for JSON at the protocol level and the constraint did
    /// not survive to the upstream (audit 1.2). Honored, not ignored: the
    /// runner stops trusting json mode for the rest of the run and anchors
    /// the instruction in the prompt instead.
    pub contract_dropped: bool,
}

// ── Reading a request without dropping half of it ────────────────────────────

pub fn tool_policy_of(req: &TransportRequest) -> ToolPolicy {
    req.tools.unwrap_or(ToolPolicy::None)
}

/// The definitions this turn offers, never null. An empty list and an absent
/// one are the same request — nothing was offered — which is why every
/// refusal is written against `.is_empty()` rather than against the field.
pub fn tool_defs_of(req: &TransportRequest) -> &[ToolDefinition] {
    &req.tool_defs
}

/// `tool_names` from `tool_calls`, in one place, so a reply cannot report a
/// call under one field and not the other.
pub fn tool_names_of(calls: &[ToolCall]) -> Vec<String> {
    calls.iter().map(|c| c.name.clone()).collect()
}

/// The default attribution, which is exactly what the shared fleet transport
/// metered before this slot existed: a harness turn on a persona IS a chat
/// turn with that persona, belonging to no conversation.
pub fn ledger_of(req: &TransportRequest) -> LedgerAttribution {
    req.ledger.clone().unwrap_or(LedgerAttribution {
        agent_model: req.model.clone(),
        source: LedgerSource::Chat,
        ref_id: None,
        task_id: None,
        tier: None,
    })
}

/// `proxyChat`'s payload shape — structural, one flat record the persona
/// gateway reads. THE ONE MAPPING from a `TransportRequest` onto it: every
/// field of the request is spent here, because three of the five hand-written
/// persona transports dropped `temperature` and `json_mode` by copying the
/// fleet transport and editing the one axis their author cared about. A
/// harness that declares `temperature: 0` and runs at the provider's 0.7 is a
/// harness whose declaration is decorative.
pub fn persona_payload(req: &TransportRequest) -> Value {
    let mut payload = Map::new();
    payload.insert("model".into(), Value::String(req.model.clone()));
    payload.insert(
        "messages".into(),
        Value::Array(
            req.messages
                .iter()
                .map(|m| serde_json::json!({ "role": m.role.as_str(), "content": m.content }))
                .collect(),
        ),
    );
    // Suppressing the tools WE offer cannot reach the persona's own internal
    // loop — which is exactly why the guard pass still runs `zero_tool_claim`
    // over the tool names the stream reported — but it is real, and it
    // belongs here rather than in one caller.
    if tool_policy_of(req) == ToolPolicy::None {
        payload.insert("tools".into(), Value::Array(Vec::new()));
        payload.insert("tool_choice".into(), Value::String("none".into()));
    }
    if let Some(t) = req.temperature {
        payload.insert(
            "temperature".into(),
            serde_json::Number::from_f64(t)
                .map(Value::Number)
                .unwrap_or(Value::Null),
        );
    }
    if req.json_mode {
        payload.insert(
            "response_format".into(),
            serde_json::json!({"type": "json_object"}),
        );
    }
    // The effort pick travels like temperature: one request field, forwarded
    // to the persona's own gateway, which hands it to the provider it fronts.
    // The agent container owns the model, so honoring the level is its call —
    // Talaria already refused it on the way IN (the routes validate against
    // the model's declared levels), which is the only side of the contract
    // Talaria can see.
    if let Some(e) = &req.effort {
        payload.insert("reasoning_effort".into(), Value::String(e.clone()));
    }
    Value::Object(payload)
}

// ── The refusal sentences ─────────────────────────────────────────────────────
//
// AN ERROR, NOT A SILENT NO-OP, and this is the rule the whole request type
// is built on: a field that cannot be honored fails the call. Dropping
/// `tools: 'own'` would run a tool-loop harness as a single-shot completion,
/// which does not read as broken — it reads as an agent that decided not to
/// act, and then gets flagged `zero_tool_claim` for saying it did something.
pub fn gateway_tools_refusal(model: &str) -> String {
    format!(
        "harness turn on \"{model}\" asked for the model's own tools, but that model is served by the ORG GATEWAY, \
         which runs no tool loop. Pin a fleet persona for this harness, or declare tools: none."
    )
}

/// The gateway learned (or has just been told by a 400) that this endpoint
/// rejects `tools`, stripped the parameter, and answered anyway. The call
/// SUCCEEDED and the reply is a perfectly ordinary completion — of a question
/// that no longer mentioned any of the tools we were asking the model to
/// choose between.
///
/// Answering that quietly is precisely audit 1.2 in the tool-calling clothes:
/// the probe would read "called no tool" off a turn where no tool was ever
/// offered and write `tools: false` — permanently, since probe facts do not
/// expire — about a model that may well call tools perfectly.
pub fn tool_defs_dropped_refusal(model: &str, endpoint: &str) -> String {
    format!(
        "harness turn on \"{model}\" offered the model tool definitions, but endpoint \"{endpoint}\" rejects the \"tools\" parameter, \
         so the call would have reached the model without them. Refusing rather than answering a question the model was never asked."
    )
}

/// A streamed turn cannot offer tools either, for a duller reason than the
/// other refusals: nothing assembles tool-call deltas on this path and
/// nothing needs to.
pub fn stream_tool_defs_refusal(model: &str) -> String {
    format!(
        "harness turn on \"{model}\" offered tool definitions on a STREAMING transport, which assembles text deltas only. \
         Run a tool-offering turn through the blocking transport."
    )
}

/// A persona's tool loop belongs to the agent: the proxy hands the payload to
/// the agent's own gateway, which runs the agent's tool loop with the agent's
/// own tools, and the stream reports tool NAMES with no arguments and no way
/// to tell ours from its own. So definitions we sent are neither guaranteed
/// to reach the model nor observable when they do, which is the exact shape
/// of "a field that cannot be honored". The tool probes read this and SKIP a
/// persona candidate rather than scoring one — a skip is honest, a `false`
/// here would not be.
pub fn fleet_tool_defs_refusal(model: &str) -> String {
    format!(
        "harness turn on \"{model}\" offered tool definitions, but that model is a FLEET PERSONA: its tool loop runs inside the agent \
         container, so tools we offer cannot be guaranteed to reach it and the calls it makes come back as bare names. \
         Offer tool definitions to a gateway model, or declare none."
    )
}

// ── The response_format this request should carry ────────────────────────────

/// The `response_format` this request should carry, preferring the harness's
/// schema over the loose object form. One definition, four call sites — the
/// four used to each hardcode `{type:'json_object'}`, which is how every
/// structured call to Anthropic came to 400.
pub fn response_format_of(req: &TransportRequest) -> Option<Value> {
    if !req.json_mode {
        return None;
    }
    // STRICT OR NOTHING, and this is the second thing a live run taught that
    // no amount of local testing would have. `strict: false` is not a weaker
    // request every provider accepts — Anthropic rejects it outright
    // ("json_schema.strict: Input should be True"), so a schema that cannot
    // be sent strictly cannot be sent to Anthropic AT ALL. Sending it anyway
    // 400'd three harnesses on every call, and the fitness suite scored that
    // as the model failing its contract.
    //
    // So a non-strict-eligible schema falls back to `json_object` rather
    // than going out as a request some providers refuse. On a provider that
    // also refuses `json_object` the run still succeeds — the runner carries
    // the JSON anchor in the prompt on every structured call, and the
    // gateway's learned parameter ratchet drops the format after the first
    // refusal.
    if let Some(ws) = &req.json_schema
        && ws.strict
    {
        return Some(serde_json::json!({
            "type": "json_schema",
            "json_schema": { "name": ws.name, "schema": ws.schema, "strict": true }
        }));
    }
    // AN ARRAY CONTRACT MUST NOT ASK FOR `json_object`.
    //
    // `json_object` is not "reply in JSON" — it is defined as "the model MUST
    // return a JSON OBJECT", and providers constrain decoding to enforce it.
    // Send it alongside a schema rooted at an array and the request is
    // self-defeating: the wire format forbids the only answer the contract
    // accepts.
    //
    // `channel-plan` is that harness — a top-level array of ticket proposals,
    // non-strict, so it took this fallback on every call. A sweep of a small
    // model failed five of its fixtures with `expected array, got object`,
    // INCLUDING after the repair turn, and every one of those was scored
    // against the model. The model was right and did what we asked; the
    // request was wrong.
    //
    // So an array root sends NO `response_format` at all. That is not a
    // downgrade from a working state — it is the removal of an instruction
    // that could only ever produce a contract violation. The runner anchors
    // the JSON instruction in the prompt on every structured call and the
    // harness parses and repairs.
    if let Some(ws) = &req.json_schema
        && root_type(&ws.schema) != Some("object")
    {
        return None;
    }
    Some(serde_json::json!({"type": "json_object"}))
}

/// The declared top-level type of a wire schema, when it declares one. Only
/// 'object' may be asked for as `json_object`; everything else — an array
/// root today, a `oneOf` root tomorrow — is a shape that mode cannot express.
fn root_type(schema: &Value) -> Option<&str> {
    schema.get("type").and_then(|t| t.as_str())
}

// ── The tool channel, on the wire ────────────────────────────────────────────

/// The OpenAI wire shape for one offered tool. Kept here, in the file that
/// owns the request, rather than at the call sites that build a body.
pub fn tool_wire_shape(def: &ToolDefinition) -> Value {
    serde_json::json!({
        "type": "function",
        "function": { "name": def.name, "description": def.description, "parameters": def.parameters }
    })
}

/// What the model called, out of a completion body. A call with no name is
/// not a call we can score or guard on, so it is dropped rather than carried
/// as an empty string that every reader then has to defend against. Ids are
/// assigned here when the provider omitted one — the ONE fallback (see
/// `ToolCall.id`).
pub fn read_tool_calls(raw: Option<&Vec<Value>>) -> Vec<ToolCall> {
    let Some(raw) = raw else { return Vec::new() };
    raw.iter()
        .enumerate()
        .filter_map(|(i, tc)| {
            let name = tc
                .get("function")
                .and_then(|f| f.get("name"))
                .and_then(|n| n.as_str())?;
            let args = tc
                .get("function")
                .and_then(|f| f.get("arguments"))
                .and_then(|a| a.as_str())
                .unwrap_or_default();
            Some(ToolCall {
                name: name.to_string(),
                args: args.to_string(),
                id: Some(
                    tc.get("id")
                        .and_then(|id| id.as_str())
                        .map(String::from)
                        .unwrap_or_else(|| format!("call_{i}")),
                ),
            })
        })
        .collect()
}

/// ONE MESSAGE, on the wire, INCLUDING its tool channel.
///
/// An assistant turn that called tools carries `tool_calls`; a result carries
/// `role: 'tool'` and the id it answers. This is the shape every
/// OpenAI-compatible provider speaks and, more to the point, the shape models
/// are TRAINED on — a tool conversation replayed as prose is a conversation
/// they will answer in prose. 34 replies in one sweep came back containing
/// Talaria's own narration of a call, because that is what the transcript had
/// shown them.
///
/// A message with neither field renders exactly as before.
pub fn tool_wire_message(m: &Message) -> Value {
    if m.role.is_tool() {
        return serde_json::json!({
            "role": "tool",
            "content": m.content,
            "tool_call_id": m.tool_call_id.clone().unwrap_or_default(),
        });
    }
    if m.tool_calls.is_empty() {
        return serde_json::json!({ "role": m.role.as_str(), "content": m.content });
    }
    serde_json::json!({
        "role": m.role.as_str(),
        "content": m.content,
        "tool_calls": m.tool_calls.iter().enumerate().map(|(i, c)| {
            serde_json::json!({
                "id": tool_call_id_of(c, i),
                "type": "function",
                "function": { "name": c.name, "arguments": c.args }
            })
        }).collect::<Vec<_>>(),
    })
}

/// THE ID A TOOL RESULT MUST ANSWER, from the call it answers.
///
/// ONE FALLBACK, EXPORTED, because two of them is the bug this exists to
/// stop. `read_tool_calls` assigns an id to everything that comes off the
/// wire, but a reply built by hand — a fleet transport, a test, a future
/// transport — can still carry a call without one, and every loop that
/// replays a tool round has to reach the SAME answer as `tool_wire_message`
/// does when it renders the assistant turn. When they disagreed, the result
/// named a call the provider had never been shown and Anthropic answered
/// `messages.3.tool.tool_call_id: Field required`.
///
/// `index` is the call's position IN ITS ASSISTANT MESSAGE, which is what
/// `tool_wire_message` numbers from — a loop that pushes one call per message
/// passes 0.
pub fn tool_call_id_of(call: &ToolCall, index: usize) -> String {
    call.id.clone().unwrap_or_else(|| format!("call_{index}"))
}

/// Does this conversation CONTAIN a tool round — an assistant turn that
/// called something, or a result answering one? A turn that replays one has
/// to be sent through the renderer that speaks the tool channel, whether or
/// not it is OFFERING tools on this turn. (The closing turn of a search loop
/// offers none and still carries the whole tool conversation behind it;
/// flattening it to `{role, content}` is the bug that 400'd Anthropic on the
/// fourth turn of a loop whose first three had worked.)
pub fn replays_tools(req: &TransportRequest) -> bool {
    req.messages
        .iter()
        .any(|m| m.role.is_tool() || !m.tool_calls.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::define::{Message, Role};

    fn req(json_mode: bool) -> TransportRequest {
        TransportRequest {
            model: "pl-main".into(),
            messages: vec![Message::user("hello")],
            temperature: Some(0.0),
            json_mode,
            json_schema: None,
            tools: None,
            tool_defs: Vec::new(),
            ledger: None,
            effort: None,
            hold_ms: None,
            caller: "test".into(),
        }
    }

    #[test]
    fn the_default_tool_policy_is_none_and_every_absent_field_has_a_reader() {
        // The readers exist so a transport cannot drop half a request by
        // reading fields by hand — the exact defect three of the five
        // hand-written shims shipped.
        assert_eq!(tool_policy_of(&req(false)), ToolPolicy::None);
        assert!(tool_defs_of(&req(false)).is_empty());
        let led = ledger_of(&req(false));
        assert_eq!(led.agent_model, "pl-main");
        assert_eq!(led.source.as_str(), "chat");
        assert!(led.ref_id.is_none() && led.task_id.is_none() && led.tier.is_none());
    }

    #[test]
    fn a_declared_attribution_wins_over_the_default() {
        let mut r = req(false);
        r.ledger = Some(LedgerAttribution {
            agent_model: "engineer-engineering".into(),
            source: LedgerSource::Ticket,
            ref_id: Some("r-9".into()),
            task_id: Some("t-41".into()),
            tier: Some("opus".into()),
        });
        let led = ledger_of(&r);
        // The BASE persona id, never the routed id — a routed id here prices
        // as an agent that does not exist, which means no price at all.
        assert_eq!(led.agent_model, "engineer-engineering");
        assert_eq!(led.source.as_str(), "ticket");
        assert_eq!(led.tier.as_deref(), Some("opus"));
    }

    #[test]
    fn persona_payload_spends_every_field_of_the_request() {
        let mut r = req(true);
        r.tools = Some(ToolPolicy::None);
        r.effort = Some("low".into());
        let p = persona_payload(&r);
        assert_eq!(p["model"], "pl-main");
        assert_eq!(p["messages"][0]["role"], "user");
        assert_eq!(p["temperature"], 0.0);
        assert_eq!(p["response_format"]["type"], "json_object");
        // 'none' is enforced at the protocol level, not left to the prompt.
        assert_eq!(p["tools"], serde_json::json!([]));
        assert_eq!(p["tool_choice"], "none");
        assert_eq!(p["reasoning_effort"], "low");
        // An own-tools turn does not get the suppression — the persona's own
        // loop is the point of the turn.
        let mut own = req(false);
        own.tools = Some(ToolPolicy::Own);
        let p = persona_payload(&own);
        assert!(p.get("tools").is_none());
        assert!(p.get("tool_choice").is_none());
    }

    #[test]
    fn response_format_is_strict_or_object_or_nothing() {
        // No json mode: nothing, the prompt anchor carries it.
        assert!(response_format_of(&req(false)).is_none());
        // Strict schema: the full wire form.
        let mut r = req(true);
        r.json_schema = Some(WireSchema {
            name: "judge".into(),
            schema: serde_json::json!({"type": "object"}),
            strict: true,
        });
        let f = response_format_of(&r).unwrap();
        assert_eq!(f["type"], "json_schema");
        assert_eq!(f["json_schema"]["name"], "judge");
        assert_eq!(f["json_schema"]["strict"], true);
        // Non-strict OBJECT schema: the loose fallback.
        let mut loose = req(true);
        loose.json_schema = Some(WireSchema {
            name: "blurb_writer".into(),
            schema: serde_json::json!({"type": "object", "additionalProperties": {"type": "string"}}),
            strict: false,
        });
        assert_eq!(response_format_of(&loose).unwrap()["type"], "json_object");
        // AN ARRAY ROOT MUST NOT ASK FOR json_object — the wire format would
        // forbid the only answer the contract accepts (channel-plan's five
        // falsely-failed fixtures).
        let mut arr = req(true);
        arr.json_schema = Some(WireSchema {
            name: "channel_plan".into(),
            schema: serde_json::json!({"type": "array", "items": {"type": "object"}}),
            strict: false,
        });
        assert!(response_format_of(&arr).is_none());
    }

    #[test]
    fn tool_names_cannot_drift_from_tool_calls() {
        let calls = vec![
            ToolCall {
                name: "search".into(),
                args: "{}".into(),
                id: Some("call_0".into()),
            },
            ToolCall {
                name: "write".into(),
                args: "{}".into(),
                id: None,
            },
        ];
        assert_eq!(
            tool_names_of(&calls),
            ["search".to_string(), "write".to_string()]
        );
    }

    #[test]
    fn read_tool_calls_assigns_the_one_fallback_id_and_drops_nameless_calls() {
        let raw = serde_json::json!([
            { "id": "prov_1", "function": { "name": "search", "arguments": "{\"q\":\"x\"}" } },
            { "function": { "name": "write" } },
            { "function": { "arguments": "{}" } }
        ]);
        let arr = raw.as_array().unwrap().clone();
        let calls = read_tool_calls(Some(&arr));
        assert_eq!(calls.len(), 2, "a call with no name is not a call");
        assert_eq!(calls[0].name, "search");
        assert_eq!(calls[0].id.as_deref(), Some("prov_1"));
        // The provider omitted the id: assigned HERE, once, positionally —
        // every consumer reaches the same answer through tool_call_id_of.
        assert_eq!(calls[1].id.as_deref(), Some("call_1"));
        assert_eq!(calls[1].args, "");
        assert!(read_tool_calls(None).is_empty());
    }

    #[test]
    fn tool_wire_message_renders_the_tool_channel_not_prose() {
        let assistant = Message {
            role: Role::Assistant,
            content: "".into(),
            tool_calls: vec![ToolCall {
                name: "search".into(),
                args: "{\"q\":\"x\"}".into(),
                id: None,
            }],
            tool_call_id: None,
        };
        let wire = tool_wire_message(&assistant);
        assert_eq!(wire["tool_calls"][0]["id"], "call_0");
        assert_eq!(wire["tool_calls"][0]["function"]["name"], "search");
        assert_eq!(
            wire["tool_calls"][0]["function"]["arguments"],
            "{\"q\":\"x\"}"
        );
        let result = Message {
            role: Role::Tool,
            content: "[]".into(),
            tool_calls: Vec::new(),
            tool_call_id: Some("call_0".into()),
        };
        assert_eq!(tool_wire_message(&result)["tool_call_id"], "call_0");
        // A plain turn renders exactly as before.
        let plain = tool_wire_message(&Message::user("hi"));
        assert_eq!(plain, serde_json::json!({"role": "user", "content": "hi"}));
    }

    #[test]
    fn replays_tools_is_true_for_a_closing_turn_over_a_tool_conversation() {
        // The case that 400'd Anthropic: no defs offered, but the history
        // carries a tool round, so the turn must go through the tool renderer.
        let mut r = req(false);
        r.messages.push(Message {
            role: Role::Tool,
            content: "results".into(),
            tool_calls: Vec::new(),
            tool_call_id: Some("call_0".into()),
        });
        assert!(replays_tools(&r));
        assert!(!replays_tools(&req(false)));
    }

    #[test]
    fn the_refusals_name_the_model_and_the_reason() {
        assert!(gateway_tools_refusal("pl-main").contains("ORG GATEWAY"));
        assert!(tool_defs_dropped_refusal("pl-main", "spark").contains("spark"));
        assert!(stream_tool_defs_refusal("pl-main").contains("STREAMING"));
        assert!(fleet_tool_defs_refusal("pl-main").contains("FLEET PERSONA"));
    }
}
