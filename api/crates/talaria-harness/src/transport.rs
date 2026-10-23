// THE TRANSPORTS, and the request that reaches them — the CONTRACT LAYER:
// the request and reply types, the one mapping onto a persona payload, the
// `response_format` derivation, the tool-channel wire renderer, and the
// refusal sentences. Then the GATEWAY half of the async layer — the blocking
// tool turn, the plain completion, and the streamed pair — over the same
// gateway pieces the chat relay runs on (`resolve_route`, `build_upstream`,
// `fetch_upstream`, `record_gateway_usage`). The FLEET half (the persona
// turn, the persona stream pump, and the `transport_kind` rule) lives at the
// bottom of the file, over `proxy_chat`, `proxy_session_turn` and
// `list_agents` — two doors into one persona: the stateless completions
// arm, and the Hermes SESSION arm a caller keys when the turn must be
// attributable from the container's side (`PersonaSession`).
//
// WHY THE REQUEST TYPE HAS EVERY SLOT IT HAS — each one is an ask a call
// site once had to serve by hand-writing its own copy of the fleet transport,
// because `TransportRequest` had no slot for it: `tools`, `ledger`,
// `hold_ms`, the streaming transport type for the streaming pair, and
// `tool_defs` + `TransportReply.tool_calls` — without which the
// `tool-select` probe could not run, could not write its fact, and the
// capability-gated widening it gates could never fire in production.
//
// AND THE REASON HAND-WRITTEN COPIES WERE A PROBLEM RATHER THAN A STYLE
// COMPLAINT: THEY QUIETLY DROPPED `req.temperature` AND `req.json_mode`. A
// harness that declared `temperature: 0` ran at the provider's default and
// nothing anywhere said so — the exact class of failure the whole harness
// layer exists to end, reintroduced by the workaround for the harness layer.
// So the mapping from a request onto a persona payload lives in ONE function
// here, and a transport that genuinely CANNOT honor a field says so by
// failing the call rather than by ignoring it.
//
// THE PERSONA'S COMPLETIONS ARM IS THE NAMED EXCEPTION, and it is named here
// rather than discovered. Hermes' API server reads `model`, `messages`,
// `stream` and `model_options` off a chat-completions body and nothing else
// (Hermes v0.22, `gateway/platforms/api_server_openai_routes.py`): no
// per-request `temperature`, no `response_format`. The persona samples at its
// own settings. `persona_payload` therefore sends neither, and says what
// that costs: a JSON request comes back `contract_dropped`, so the runner
// stops asking for protocol-level JSON and leans on the prompt anchor every
// structured request already carries — the deliberate fallback a gateway
// that strips `response_format` gets. Temperature has no equivalent to fall
// back to; a harness that needs a pinned temperature belongs on a gateway
// model, not a persona.

use super::define::{Message, Role};
use serde_json::{Map, Value};
use talaria_gateway::fleet_chat::persona_model_options;
use talaria_harness_json_schema::WireSchema;

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
/// The gateway relay books every call a persona makes; this is the flow
/// those calls belong to, recorded by the fleet transport against the
/// turn's conversation key (`turn_attribution`). The runner used to meter
/// every persona turn as a plain chat turn belonging to nothing, and three
/// call sites each lost something real:
///
///   task_id    a work-session turn's spend lands in the ledger but never in
///              the TICKET'S cost, which is the number a ticket owner reads.
///   ref_id +   research's persona stages metered with no run id, so a run's
///   source     cost stopped being answerable at all.
///
/// Hence `agent_model` is the BASE persona id — the name the relay books an
/// agent's key under — never the routed id `TransportRequest.model` calls.
/// `tier` is the alias NAME the turn was routed to; the relay prices the
/// model each call actually ran on, so the ledger no longer reads it.
#[derive(Debug, Clone)]
pub struct LedgerAttribution {
    /// The agent as the ledger names it: the base persona id, never a tier id.
    pub agent_model: String,
    pub source: LedgerSource,
    /// The conversation, channel or research run this spend belongs to.
    pub ref_id: Option<String>,
    /// The ticket this spend belongs to, so it reaches the ticket's cost.
    pub task_id: Option<String>,
    /// The alias NAME (`opus`) the turn was routed to.
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
/// the mapping below or by the transports — see the module header for why a
/// field a transport cannot honor fails the call instead.
#[derive(Clone)]
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
    /// levels the model's metadata vouches for. Sent as `reasoning_effort` to
    /// a gateway model and as `model_options.reasoning_effort` to a persona
    /// (on both of its arms — the only spelling Hermes reads), and dropped
    /// without a murmur where it cannot be honored: a TOOL-OFFERING gateway
    /// turn must keep its `'none'` (the provider rejects the combination),
    /// and a field the request cannot honor is not a reason to fail a chat.
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
    /// A tap the transport pings whenever the turn shows LIFE — per stream
    /// chunk on the persona path. For callers with an idle-based deadline
    /// (a work session awaiting an agent that may work for hours); absent
    /// everywhere else, and a transport that has nothing to ping simply
    /// ignores it.
    pub liveness: Option<std::sync::Arc<dyn Fn() + Send + Sync>>,
    /// A Redis channel name the persona pump publishes each parsed stream
    /// event to — the watch route's live half. See RunContext::watch.
    pub watch: Option<String>,
}

impl TransportRequest {
    /// Every message's content, concatenated — the character count both
    /// metering paths estimate the prompt from. UTF-16 units, like JS
    /// `.length` and like the chat relay's own count: the estimate is
    /// chars/4, and a caller and a relay that disagreed on the unit would
    /// disagree on the ledger row for the same turn.
    pub fn prompt_chars(&self) -> usize {
        self.messages
            .iter()
            .map(|m| m.content.encode_utf16().count())
            .sum()
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
    /// not survive to the upstream. Honored, not ignored: the
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

/// The default attribution: a harness turn on a persona IS a chat turn with
/// that persona, belonging to no conversation.
pub fn ledger_of(req: &TransportRequest) -> LedgerAttribution {
    req.ledger.clone().unwrap_or(LedgerAttribution {
        agent_model: req.model.clone(),
        source: LedgerSource::Chat,
        ref_id: None,
        task_id: None,
        tier: None,
    })
}

/// `proxy_chat`'s payload shape — structural, one flat record the persona
/// gateway reads. THE ONE MAPPING from a `TransportRequest` onto it: every
/// field of the request is decided here, because a hand-copied mapping drops
/// exactly the fields its author did not care about.
///
/// DECIDED, NOT ALL SENT. `temperature` and `json_mode`/`json_schema` are
/// read here and deliberately left off the body: Hermes' API server has no
/// per-request equivalent for either (see the module header), so sending
/// them only made a harness's declaration look honored. The JSON half is
/// reported back as `contract_dropped` by the persona transport, so the
/// runner knows the protocol constraint did not hold.
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
    // `tools: []` / `tool_choice: none` do NOT reach the persona: Hermes' API
    // server reads neither off the body (see the module header), and its own
    // loop keeps its own tools. The real enforcement is the guard pass, which
    // runs `zero_tool_claim` over the tool names the stream reported.
    // Removing these two fields is a follow-up.
    if tool_policy_of(req) == ToolPolicy::None {
        payload.insert("tools".into(), Value::Array(Vec::new()));
        payload.insert("tool_choice".into(), Value::String("none".into()));
    }
    // The effort pick rides in `model_options`, the one place Hermes reads a
    // per-request effort from; the container hands it to the provider it
    // fronts. Talaria already refused an unvouched level on the way IN (the
    // routes validate against the model's declared levels), which is the
    // only side of the contract Talaria can see.
    if let Some(options) = persona_model_options(req.effort.as_deref()) {
        payload.insert("model_options".into(), options);
    }
    Value::Object(payload)
}

/// What a persona turn reports about the JSON contract it was asked for:
/// dropped, always, when one was asked — `persona_payload` cannot put
/// `response_format` anywhere Hermes reads it.
pub fn persona_contract_dropped(req: &TransportRequest) -> bool {
    req.json_mode || req.json_schema.is_some()
}

/// A Hermes SESSION the persona turn lands on, instead of the stateless
/// completions arm — the fleet transport's other door, chosen by the CALLER
/// and not by the request, because it is an attribution key and not a model
/// parameter: the container keeps the transcript under this id, and the
/// agent's talaria-events plugin reports every tool frame against it. A work
/// session keys it by its RUN id, so a frame says which run it belongs to and
/// the thrash brake can never abort the wrong one; a pinned chat keys it by
/// the conversation id (the chat path's own call, in `talaria-chat-persist`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonaSession {
    /// The session id as the container knows it — the `{"id"}` of
    /// `POST /api/sessions`, created on first use and resumed after.
    pub id: String,
    /// A turn has gone through this session before, so it MUST still be in
    /// the container: one that is not there means the state volume was lost
    /// (a re-hire on the same slug, a swapped volume, a host restore) —
    /// `proxy_session_turn`'s `history_expected`. The step then starts the
    /// session again (`open_persona_turn`).
    pub resumed: bool,
}

/// The session arm's body, cut from the request's messages — THE ONE MAPPING
/// for that door, the way `persona_payload` is for the other. The system
/// rows fold into the turn's ephemeral system prompt (Hermes takes one per
/// turn), and the trailing run of user rows is the message, joined with
/// blank lines (two prompts queued behind one another are both unanswered).
///
/// `None` when the request carries HISTORY — an assistant or tool row ahead
/// of the trailing user run, or no user row at all. A session turn sends
/// only the new message and the container holds the transcript, so a request
/// that brought its own transcript has nowhere to put it: the caller sends
/// that request the completions way, where the whole array travels. The
/// chat path applies the same rule per conversation (`pinned_history_fits`);
/// this is it per request.
pub fn session_turn_parts(messages: &[Message]) -> Option<(Option<String>, String)> {
    let tail_len = messages
        .iter()
        .rev()
        .take_while(|m| m.role == Role::User)
        .count();
    let (head, tail) = messages.split_at(messages.len() - tail_len);
    if tail.is_empty() || head.iter().any(|m| m.role != Role::System) {
        return None;
    }
    let system: Vec<&str> = head
        .iter()
        .map(|m| m.content.as_str())
        .filter(|c| !c.trim().is_empty())
        .collect();
    let message = tail
        .iter()
        .map(|m| m.content.as_str())
        .filter(|c| !c.trim().is_empty())
        .collect::<Vec<_>>()
        .join("\n\n");
    if message.is_empty() {
        return None;
    }
    Some(((!system.is_empty()).then(|| system.join("\n\n")), message))
}

/// What a session turn CANNOT carry, named. The body is a message and an
/// ephemeral system prompt — Hermes samples at the persona's own settings
/// and runs its own loop — so a request that declared `temperature`, asked
/// for structured output, or asked the tools suppressed fails the call, by
/// the rule at the top of this file: a field that cannot be honored is an
/// error, never a quiet drop. `effort` is not a contract and refuses
/// nothing; it rides in the session body's `model_options`, the same
/// spelling the completions arm sends. `None` means the request fits the
/// door.
pub fn session_turn_refusal(req: &TransportRequest) -> Option<String> {
    let field = if req.temperature.is_some() {
        "temperature"
    } else if req.json_mode || req.json_schema.is_some() {
        "response_format"
    } else if tool_policy_of(req) == ToolPolicy::None {
        "tools: none"
    } else {
        return None;
    };
    Some(format!(
        "harness turn on \"{}\" declared {field} on a Hermes SESSION turn, which carries only the message and its \
         ephemeral system prompt. Declare the field away, or run the turn on the completions arm.",
        req.model
    ))
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
/// Answering that quietly is the silent-strip failure in tool-calling
/// clothes: the probe would read "called no tool" off a turn where no tool
/// was ever offered and write `tools: false` — permanently, since probe
/// facts do not expire — about a model that may well call tools perfectly.
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
/// schema over the loose object form. One definition for every body this file
/// builds — call sites each hardcoding `{type:'json_object'}` is how every
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

// ── The gateway transports, async over the relay's own pieces ────────────────
//
// The GATEWAY half of the async layer, on `resolve_route` / `build_upstream` /
// `fetch_upstream` / `record_gateway_usage`. The FLEET half — the persona
// turn, `pump_persona_stream`, and the `transport_kind` rule that chooses
// between the sides — lives at the bottom of this file.

use futures_util::StreamExt;
use talaria_gateway::registry::resolve_route;
use talaria_gateway::upstream::{Reply, build_upstream, fetch_upstream};
use talaria_gateway::usage::{
    ReportedSpend, TokenCounts, estimate_tokens, record_gateway_usage_with_spend, reported_spend,
};
use talaria_state::AppState;

/// `gateway completion {status}: {body}` — the failure sentence both blocking
/// paths throw with, body cut at 300 characters (on chars, not bytes: a cut
/// mid-codepoint would panic the formatter and say less).
fn completion_error(status: u16, body: String) -> String {
    let head: String = body.chars().take(300).collect();
    format!("gateway completion {status}: {head}")
}

/// The ledger row both paths write. Spawned, never awaited on the turn's
/// path, so a ledger hiccup never fails a turn that already succeeded.
fn meter(
    state: &AppState,
    route: &talaria_gateway::registry::ResolvedRoute,
    caller: &str,
    usage: Option<TokenPair>,
    prompt_chars: usize,
    text_chars: usize,
    spend: ReportedSpend,
) {
    let pg = state.pg.clone();
    let counts = match usage {
        Some(u) => TokenCounts {
            prompt_tokens: u.prompt_tokens,
            completion_tokens: u.completion_tokens,
            ..TokenCounts::default()
        },
        None => TokenCounts {
            prompt_tokens: estimate_tokens(prompt_chars),
            completion_tokens: estimate_tokens(text_chars),
            ..TokenCounts::default()
        },
    };
    let estimated = usage.is_none();
    let caller = caller.to_string();
    let endpoint_name = route.endpoint.name.clone();
    let endpoint_class = route.endpoint.class.clone();
    let upstream_model = route.upstream_model.clone();
    tokio::spawn(async move {
        let _ = record_gateway_usage_with_spend(
            &pg,
            &caller,
            &endpoint_name,
            &endpoint_class,
            &upstream_model,
            &counts,
            estimated,
            Some(&spend),
        )
        .await;
    });
}

/// The gateway call that OFFERS TOOLS, and the reason it is not the plain
/// completion helper: that path's body has no slot for tool definitions and
/// its return is a string, so it can neither ask the question nor report the
/// answer. This walks the same route the same way `gateway_stream` does —
/// `build_upstream` for the provider key and the request_defaults merge,
/// `fetch_upstream` for the 400-recovery loop and the contract-drop record,
/// `record_gateway_usage` for the ledger row.
///
/// `tool_choice: 'auto'` rather than the policy's `'none'`: `ToolPolicy`
/// governs the model's OWN tools and this turn is about ours, so suppressing
/// them here would send four definitions and forbid calling any of them.
///
/// ALSO SERVES THE CLOSING TURN OF A TOOL LOOP — `gateway_transport` routes
/// here whenever the conversation replays a tool round even with no defs
/// offered, so the one renderer that speaks the tool channel builds the
/// history (see `gateway_transport` for the Anthropic 400 that flat
/// `{role, content}` rendering produced).
pub async fn gateway_tool_turn(
    state: &AppState,
    req: &TransportRequest,
    defs: &[ToolDefinition],
) -> Result<TransportReply, String> {
    let route = resolve_route(&state.pg, &req.model)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("model \"{}\" is not on the gateway", req.model))?;

    let mut body = Map::new();
    body.insert("model".into(), Value::String(req.model.clone()));
    // `tool_wire_message`, NOT a flatten to `{role, content}`. This is the one
    // renderer that speaks the tool channel — flattening dropped
    // `tool_calls`/`tool_call_id` on the floor, leaving a caller no way to
    // show the model what it had already called except by narrating it into
    // prose. Models imitate whatever the transcript shows them:
    // `research-search` narrated `Called web_search({...})` and got that
    // string back as the model's final answer on a live sweep. See the note
    // on `Message.tool_calls`.
    body.insert(
        "messages".into(),
        Value::Array(req.messages.iter().map(tool_wire_message).collect()),
    );
    body.insert("stream".into(), Value::Bool(false));
    // NO TOOLS OFFERED IS A REAL CASE HERE, and it is the closing turn of a
    // search loop: the conversation replays a tool round, but this turn asks
    // for prose. Sending `tools: []` with `tool_choice: 'auto'` is a request
    // some providers reject and none of them needs, so the fields are simply
    // absent.
    if !defs.is_empty() {
        body.insert(
            "tools".into(),
            Value::Array(defs.iter().map(tool_wire_shape).collect()),
        );
        body.insert("tool_choice".into(), Value::String("auto".into()));
    }
    if let Some(t) = req.temperature {
        body.insert(
            "temperature".into(),
            serde_json::Number::from_f64(t)
                .map(Value::Number)
                .unwrap_or(Value::Null),
        );
    }
    // FUNCTION TOOLS AND A REASONING EFFORT ARE INCOMPATIBLE ON
    // /v1/chat/completions, and the provider says so in the sentence it
    // refuses with:
    //
    //   Function tools with reasoning_effort are not supported for
    //   gpt-5.6-terra in /v1/chat/completions. To use function tools, use
    //   /v1/responses or set reasoning_effort to 'none'.
    //
    // WE WERE NOT SENDING IT. The model's OWN default effort is what
    // conflicts, so there was nothing for the strip-and-retry ratchet to
    // remove — it bails when the named parameter is not in the body — and a
    // sweep of that model filed 27 cases across four tool-loop harnesses as
    // "could not reach this model" on an endpoint that was answering
    // everything else fine.
    //
    // So the tool turn says the one thing that makes tools work. It is sent
    // to EVERY provider rather than gated on a vendor check, because a table
    // of which providers accept which parameters is the thing this file
    // refuses to maintain: an endpoint that does not know the field answers
    // 400 naming it, it IS in the body, and the existing ratchet strips it
    // for good on the retry. The quirk costs one 400 once, on providers that
    // have it, and nothing thereafter.
    //
    // A USER-PICKED EFFORT (`req.effort`) IS DELIBERATELY NOT SENT HERE, on
    // either branch of that rule: with defs offered the explicit 'none' below
    // is the one value the provider accepts, and without them the turn
    // replays a tool conversation whose history can trip the same restriction.
    // The pick is not lost — the closing, tool-free turn of the loop still
    // carries it — and the alternative is a 400 the ratchet would answer by
    // stripping effort from every future turn on that model.
    if !defs.is_empty() {
        body.insert("reasoning_effort".into(), Value::String("none".into()));
    }
    if let Some(f) = response_format_of(req) {
        body.insert("response_format".into(), f);
    }

    let mut call = build_upstream(state, &route, &Value::Object(body)).await?;
    let reply = fetch_upstream(&state.pg, &mut call, Some(&route)).await?;
    if !reply.is_ok() {
        return Err(completion_error(reply.status(), reply.text().await));
    }
    // BEFORE THE BODY IS READ, because a dropped `tools` parameter makes the
    // body an answer to a different question. `build_upstream` pre-strips a
    // remembered rejection and `fetch_upstream` strips a live one, and both
    // record the drop on the call — so this one check covers the remembered
    // case and the 400 case.
    if call.contract_drops.iter().any(|d| d.capability == "tools") {
        return Err(tool_defs_dropped_refusal(&req.model, &route.endpoint.name));
    }

    let raw = reply.text().await;
    let j: Value =
        serde_json::from_str(&raw).map_err(|e| format!("gateway completion parse: {e}"))?;
    let message = j
        .pointer("/choices/0/message")
        .cloned()
        .unwrap_or(Value::Null);
    let text = message
        .get("content")
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();
    let wire_calls = message
        .get("tool_calls")
        .and_then(|t| t.as_array())
        .cloned();
    let tool_calls = read_tool_calls(wire_calls.as_ref());
    let usage = j.get("usage").filter(|u| !u.is_null()).map(|u| TokenPair {
        prompt_tokens: u.get("prompt_tokens").and_then(|v| v.as_i64()).unwrap_or(0),
        completion_tokens: u
            .get("completion_tokens")
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
    });
    let text_chars = text.encode_utf16().count();
    meter(
        state,
        &route,
        &req.caller,
        usage,
        req.prompt_chars(),
        text_chars,
        reported_spend(&route.endpoint.provider, &j),
    );
    Ok(TransportReply {
        kind: TransportKind::Gateway,
        tool_names: tool_names_of(&tool_calls),
        tool_calls: Some(tool_calls),
        usage,
        contract_dropped: call.contract_drops.iter().any(|d| d.capability == "json"),
        text,
    })
}

/// THE BLOCKING GATEWAY TRANSPORT.
pub async fn gateway_transport(
    state: &AppState,
    req: &TransportRequest,
) -> Result<TransportReply, String> {
    if tool_policy_of(req) == ToolPolicy::Own {
        return Err(gateway_tools_refusal(&req.model));
    }
    // OFFERING TOOLS IS NOT THE ONLY REASON TO RENDER THEM, and this cost
    // every research-search fixture on Anthropic.
    //
    // A search loop ends with a CLOSING TURN that offers no tools — the
    // model has searched, now it writes the answer — and that turn
    // carries the whole tool conversation behind it. With no defs it fell
    // through to the plain completion helper, which flattens every message
    // to `{role, content}` and drops `tool_call_id` on the floor. Anthropic
    // then refused the request it had itself produced two turns earlier:
    //
    //   messages.3.tool.tool_call_id: Field required
    //
    // Three tool turns came back 200 and the fourth 400'd, which is why this
    // read as "the model cannot reach the search tool" — the search had
    // already worked.
    if !req.tool_defs.is_empty() || replays_tools(req) {
        return gateway_tool_turn(state, req, &req.tool_defs).await;
    }

    // The plain single-shot turn: no tool channel anywhere in the history, so
    // the flat `{role, content}` rendering IS the history. Effort is
    // forwarded here — the plain turn has no function tools to conflict with
    // it (see the note in `gateway_tool_turn` for why the tool turn cannot).
    let route = resolve_route(&state.pg, &req.model)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("model \"{}\" is not on the gateway", req.model))?;
    let mut body = Map::new();
    body.insert("model".into(), Value::String(req.model.clone()));
    body.insert(
        "messages".into(),
        Value::Array(
            req.messages
                .iter()
                .map(|m| serde_json::json!({ "role": m.role.as_str(), "content": m.content }))
                .collect(),
        ),
    );
    body.insert("stream".into(), Value::Bool(false));
    if let Some(t) = req.temperature {
        body.insert(
            "temperature".into(),
            serde_json::Number::from_f64(t)
                .map(Value::Number)
                .unwrap_or(Value::Null),
        );
    }
    if let Some(e) = &req.effort {
        body.insert("reasoning_effort".into(), Value::String(e.clone()));
    }
    if let Some(f) = response_format_of(req) {
        body.insert("response_format".into(), f);
    }
    let mut call = build_upstream(state, &route, &Value::Object(body)).await?;
    let reply = fetch_upstream(&state.pg, &mut call, Some(&route)).await?;
    if !reply.is_ok() {
        return Err(completion_error(reply.status(), reply.text().await));
    }
    let raw = reply.text().await;
    let j: Value =
        serde_json::from_str(&raw).map_err(|e| format!("gateway completion parse: {e}"))?;
    let text = j
        .pointer("/choices/0/message/content")
        .and_then(|c| c.as_str())
        .unwrap_or("")
        .to_string();
    let usage = j.get("usage").filter(|u| !u.is_null()).map(|u| TokenPair {
        prompt_tokens: u.get("prompt_tokens").and_then(|v| v.as_i64()).unwrap_or(0),
        completion_tokens: u
            .get("completion_tokens")
            .and_then(|v| v.as_i64())
            .unwrap_or(0),
    });
    let text_chars = text.encode_utf16().count();
    meter(
        state,
        &route,
        &req.caller,
        usage,
        req.prompt_chars(),
        text_chars,
        reported_spend(&route.endpoint.provider, &j),
    );
    // The ledger row meters with the real usage; the runner sees none of it —
    // the plain path has always been chars-estimated, and that is the number
    // the runner's metering expects.
    Ok(TransportReply {
        kind: TransportKind::Gateway,
        tool_names: Vec::new(),
        tool_calls: None,
        usage: None,
        contract_dropped: call.contract_drops.iter().any(|d| d.capability == "json"),
        text,
    })
}

/// The gateway transport, STREAMED — the piece that makes "a harness may
/// stream" true of both transports rather than only of personas. The Muse's
/// six prose kinds draft against an ORG GATEWAY model, so without this they
/// would keep a hand-written upstream pair however good the streamed runner
/// gets.
///
/// The blocking helper cannot serve it: it asks for `stream: false` and hands
/// back a finished string, which is the one thing a streaming surface must
/// not wait for. So this walks the same route, asks for `stream: true`, and
/// emits each delta as it lands — metering exactly as the blocking paths do,
/// and reporting the same `contract_dropped` signal from the same drops, so a
/// dropped `response_format` is as visible here as it is there.
pub async fn gateway_stream(
    state: &AppState,
    req: &TransportRequest,
    mut emit: impl FnMut(&str) + Send,
) -> Result<TransportReply, String> {
    if tool_policy_of(req) == ToolPolicy::Own {
        return Err(gateway_tools_refusal(&req.model));
    }
    // A STREAMED TURN OFFERS NO TOOLS. Assembling tool calls out of deltas is
    // a second, fiddlier parser for a case no surface has: the streaming
    // harnesses are the Muse's prose kinds and the briefing follow-up, and
    // the one caller that needs tool definitions (the probe) is blocking by
    // construction. Better to refuse the field here than to grow a parser
    // that nothing exercises and that would report a half-assembled call as
    // a real one.
    if !req.tool_defs.is_empty() {
        return Err(stream_tool_defs_refusal(&req.model));
    }
    let route = resolve_route(&state.pg, &req.model)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("model \"{}\" is not on the gateway", req.model))?;
    let mut body = Map::new();
    body.insert("model".into(), Value::String(req.model.clone()));
    body.insert(
        "messages".into(),
        Value::Array(
            req.messages
                .iter()
                .map(|m| serde_json::json!({ "role": m.role.as_str(), "content": m.content }))
                .collect(),
        ),
    );
    body.insert("stream".into(), Value::Bool(true));
    if let Some(t) = req.temperature {
        body.insert(
            "temperature".into(),
            serde_json::Number::from_f64(t)
                .map(Value::Number)
                .unwrap_or(Value::Null),
        );
    }
    if let Some(e) = &req.effort {
        body.insert("reasoning_effort".into(), Value::String(e.clone()));
    }
    if let Some(f) = response_format_of(req) {
        body.insert("response_format".into(), f);
    }
    let mut call = build_upstream(state, &route, &Value::Object(body)).await?;
    let reply = fetch_upstream(&state.pg, &mut call, Some(&route)).await?;
    if !reply.is_ok() {
        return Err(completion_error(reply.status(), reply.text().await));
    }
    let res = match reply {
        Reply::Live(r) => r,
        // A synthesized reply is a relayed 400 — is_ok already said no, so
        // this arm is the same failure one turn later, kept structural so
        // the stream below cannot see a body-less reply.
        Reply::Synthesized { status, text, .. } => return Err(completion_error(status, text)),
    };

    let mut text = String::new();
    let mut usage: Option<TokenPair> = None;
    let mut spend_body = serde_json::json!({});
    let mut buffered = String::new();
    let mut stream = res.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| e.to_string())?;
        buffered.push_str(&String::from_utf8_lossy(&chunk));
        let mut lines: Vec<String> = buffered.split('\n').map(str::to_string).collect();
        // The last element is a partial line: keep it for the next chunk.
        // Splitting on '\n' and parsing every piece is how a JSON frame gets
        // torn in half.
        buffered = lines.pop().unwrap_or_default();
        for line in &lines {
            let Some(data) = line.strip_prefix("data:").map(str::trim) else {
                continue;
            };
            if data == "[DONE]" {
                continue;
            }
            let Ok(frame) = serde_json::from_str::<Value>(data) else {
                continue;
            };
            if let Some(u) = frame.get("usage").filter(|u| !u.is_null()) {
                usage = Some(TokenPair {
                    prompt_tokens: u.get("prompt_tokens").and_then(|v| v.as_i64()).unwrap_or(0),
                    completion_tokens: u
                        .get("completion_tokens")
                        .and_then(|v| v.as_i64())
                        .unwrap_or(0),
                });
                spend_body["usage"] = u.clone();
            }
            if let Some(id) = frame.get("id").and_then(|v| v.as_str()) {
                spend_body["id"] = serde_json::Value::String(id.to_string());
            }
            if let Some(piece) = frame
                .pointer("/choices/0/delta/content")
                .and_then(|c| c.as_str())
            {
                text.push_str(piece);
                emit(piece);
            }
        }
    }

    let text_chars = text.encode_utf16().count();
    meter(
        state,
        &route,
        &req.caller,
        usage,
        req.prompt_chars(),
        text_chars,
        reported_spend(&route.endpoint.provider, &spend_body),
    );
    Ok(TransportReply {
        kind: TransportKind::Gateway,
        tool_names: Vec::new(),
        tool_calls: None,
        usage,
        contract_dropped: call.contract_drops.iter().any(|d| d.capability == "json"),
        text,
    })
}

// ── The FLEET half — a persona turn through the agent's own gateway ──────────

use talaria_fleet_agents::list_agents;
use talaria_gateway::fleet_chat::{
    AgentStreamEvent, AgentStreamParser, ByteStream, ChatStream, proxy_chat, proxy_session_turn,
};
use talaria_gateway::usage::TurnAttribution;
use talaria_me::gateway_models;

/// A persona turn's assembled result. The parser reports reasoning text too;
/// it is dropped — only content, tool names and usage feed the turn.
pub struct PersonaTurn {
    pub text: String,
    pub tool_names: Vec<String>,
    pub usage: Option<TokenPair>,
    /// The failure frame's reason when the agent gateway reported one mid-
    /// stream — the turn produced nothing and this says why.
    pub error: Option<String>,
}

impl PersonaTurn {
    fn fold(&mut self, ev: AgentStreamEvent, emit: &mut Option<&mut (dyn FnMut(&str) + Send)>) {
        match ev {
            AgentStreamEvent::Content { text } => {
                self.text.push_str(&text);
                if let Some(emit) = emit.as_mut() {
                    emit(&text);
                }
            }
            AgentStreamEvent::Tool { name, .. } => self.tool_names.push(name),
            AgentStreamEvent::Usage {
                prompt_tokens,
                completion_tokens,
            } => {
                self.usage = Some(TokenPair {
                    prompt_tokens,
                    completion_tokens,
                });
            }
            AgentStreamEvent::Reasoning { .. } => {}
            // The persona transport never steers or stops a run — and nothing
            // here can be stopped from outside, so the interruption is not news
            // to a turn assembled from what did arrive.
            AgentStreamEvent::RunStarted { .. } | AgentStreamEvent::Stopped => {}
            AgentStreamEvent::Error { message } => {
                if self.error.is_none() {
                    self.error = Some(message);
                }
            }
        }
    }
}

/// Drain a persona stream into text, tool names and usage, emitting each
/// delta on the way past. `emit` is the ONLY thing the streaming caller does
/// differently — one loop, not one copy per surface.
pub async fn pump_persona_stream(
    body: ByteStream,
    emit: Option<&mut (dyn FnMut(&str) + Send)>,
) -> Result<PersonaTurn, String> {
    pump_persona_stream_alive(body, emit, None, None).await
}

/// The same pump with a LIVENESS TAP: every chunk from the agent's stream is
/// the agent working, and a caller whose deadline is idle-based re-arms on
/// each one. This is the work session's "the turn is alive" signal — the
/// chat transport's own law (frames arriving means the turn runs) made
/// available to the runs engine.
pub async fn pump_persona_stream_alive(
    mut body: ByteStream,
    mut emit: Option<&mut (dyn FnMut(&str) + Send)>,
    liveness: Option<&std::sync::Arc<dyn Fn() + Send + Sync>>,
    watch: Option<&std::sync::Arc<dyn Fn(&str) + Send + Sync>>,
) -> Result<PersonaTurn, String> {
    let mut parser = AgentStreamParser::new();
    let mut turn = PersonaTurn {
        text: String::new(),
        tool_names: Vec::new(),
        usage: None,
        error: None,
    };
    while let Some(chunk) = body.next().await {
        if let Some(ping) = liveness {
            ping();
        }
        let chunk = chunk.map_err(|e| format!("persona stream: {e}"))?;
        for ev in parser.feed(&chunk) {
            if let Some(tap) = watch
                && let Some(line) = watch_line(&ev)
            {
                tap(&line);
            }
            turn.fold(ev, &mut emit);
        }
    }
    for ev in parser.finish() {
        if let Some(tap) = watch
            && let Some(line) = watch_line(&ev)
        {
            tap(&line);
        }
        turn.fold(ev, &mut emit);
    }
    Ok(turn)
}

/// One watch line per parsed stream event — the terminal the browser shows:
/// the agent's words as they land, tool calls as they start, the turn's end.
/// Reasoning is included (it is the agent thinking out loud); usage is not a
/// terminal line and is skipped. One line of JSON each, newline-free by
/// construction, appended to the tail and published to the channel verbatim.
///
/// TOOL frames carry `id` (lifecycle correlation) and `p` — the persona's
/// DISPLAY-REDACTED preview of the call's primary argument (Hermes builds it
/// from the args: the whole terminal command, the whole execute_code body —
/// which is where the agent's harness steering is legible). The preview is
/// clamped here so one call cannot flood the tail; the full turn is the
/// transcript artifact's business, not the live stream's.
fn watch_line(ev: &talaria_gateway::fleet_chat::AgentStreamEvent) -> Option<String> {
    use talaria_gateway::fleet_chat::AgentStreamEvent;
    let v = match ev {
        AgentStreamEvent::Content { text } => serde_json::json!({ "t": "d", "v": text }),
        AgentStreamEvent::Reasoning { text } => serde_json::json!({ "t": "r", "v": text }),
        AgentStreamEvent::Tool {
            id,
            name,
            label,
            status,
        } => {
            let mut frame = serde_json::Map::new();
            frame.insert("t".into(), serde_json::json!("tool"));
            frame.insert("v".into(), serde_json::json!(name));
            if let Some(status) = status {
                frame.insert("s".into(), serde_json::json!(status));
            }
            if let Some(id) = id {
                frame.insert("id".into(), serde_json::json!(id));
            }
            if !label.is_empty() {
                frame.insert("p".into(), serde_json::json!(clamp_label(label)));
            }
            serde_json::Value::Object(frame)
        }
        // Neither is a terminal line: usage is a count, and a run the watcher
        // cannot stop has nothing to say about having been stopped.
        AgentStreamEvent::Usage { .. }
        | AgentStreamEvent::RunStarted { .. }
        | AgentStreamEvent::Stopped => return None,
        AgentStreamEvent::Error { message } => {
            serde_json::json!({ "t": "err", "v": message })
        }
    };
    Some(v.to_string())
}

/// The watch-line preview clamp: 2,000 UTF-16 units on a char boundary —
/// enough for a long steering prompt, small enough that a pathological
/// argument cannot dwarf the tail.
fn clamp_label(label: &str) -> String {
    talaria_body::truncate_utf16(label, 2_000).to_string()
}

/// The flow a persona turn's spend belongs to, for the transport to record:
/// every call the agent makes for the turn is then metered at the relay
/// under this source, conversation or run, and ticket. The relay prices the
/// model that actually served each call, so the ledger's `tier` is not
/// needed here.
fn turn_attribution(pg: &sqlx::PgPool, req: &TransportRequest) -> TurnAttribution {
    let led = ledger_of(req);
    TurnAttribution {
        pg: pg.clone(),
        agent_model: led.agent_model,
        source: led.source.as_str().to_string(),
        ref_id: led.ref_id,
        task_id: led.task_id,
    }
}

/// Open the persona's stream through the door the caller chose: the
/// completions arm when no session is keyed, the Hermes session arm when one
/// is — created on first use, resumed after. A request carrying history a
/// session cannot hold (`session_turn_parts`) goes the completions way,
/// logged, not an error. A RESUMED session the container no longer has
/// (`proxy_session_turn` answers `None` — the state volume was lost) starts
/// again, empty, and only a session Hermes will not recreate falls back to a
/// completion. The turn still happens; only the continuity before it is
/// lost.
async fn open_persona_turn(
    req: &TransportRequest,
    session: Option<&PersonaSession>,
    attribution: TurnAttribution,
) -> Result<ChatStream, String> {
    let Some(session) = session else {
        return Ok(proxy_chat(&persona_payload(req), req.hold_ms, Some(attribution)).await);
    };
    if let Some(refusal) = session_turn_refusal(req) {
        return Err(refusal);
    }
    let Some((system_message, message)) = session_turn_parts(&req.messages) else {
        tracing::warn!(
            "[transport] {} ({}): the request carries history a session turn cannot hold — \
             sent as a completion instead of on session {}",
            req.model,
            req.caller,
            session.id
        );
        return Ok(proxy_chat(&persona_payload(req), req.hold_ms, Some(attribution)).await);
    };
    match proxy_session_turn(
        &req.model,
        &session.id,
        system_message.as_deref(),
        &Value::String(message.clone()),
        req.effort.as_deref(),
        req.hold_ms,
        session.resumed,
        Some(attribution.clone()),
    )
    .await
    {
        Some(stream) => Ok(stream),
        // The room is gone. A session turn's request carries no history of
        // its own, so a completion would answer from exactly what a fresh
        // session would — and only the session keeps this step for the next
        // one. So the session starts again here, empty, and the run's later
        // steps continue in it. (The transport never makes that empty room
        // itself: the chat path, which does hold a transcript, must not find
        // one on its next turn.) An id Hermes refuses to recreate — a deleted
        // session — leaves the completion.
        None => {
            tracing::warn!(
                "[transport] {} ({}): session {} was expected to exist and did not — this turn \
                 starts it again, without the earlier transcript",
                req.model,
                req.caller,
                session.id
            );
            match proxy_session_turn(
                &req.model,
                &session.id,
                system_message.as_deref(),
                &Value::String(message),
                req.effort.as_deref(),
                req.hold_ms,
                false,
                Some(attribution.clone()),
            )
            .await
            {
                Some(stream) => Ok(stream),
                None => Ok(proxy_chat(&persona_payload(req), req.hold_ms, Some(attribution)).await),
            }
        }
    }
}

async fn persona_turn(
    state: &AppState,
    req: &TransportRequest,
    emit: Option<&mut (dyn FnMut(&str) + Send)>,
    session: Option<&PersonaSession>,
) -> Result<TransportReply, String> {
    // See `fleet_tool_defs_refusal`: the loop on the other side of the proxy
    // is the agent's, so a definition we send is neither guaranteed to arrive
    // nor visible when it is called. Refused here rather than in the two
    // exported transports so the streamed persona path cannot grow a quiet
    // exception.
    if !tool_defs_of(req).is_empty() {
        return Err(fleet_tool_defs_refusal(&req.model));
    }
    let upstream = open_persona_turn(req, session, turn_attribution(&state.pg, req)).await?;
    if !upstream.ok() {
        return Err(format!("persona gateway {}", upstream.status));
    }
    // A CANNED STREAM IS AN OUTAGE WEARING A 200 — see the `canned` field on
    // `proxy_chat`'s reply.
    // It is a failed call, so it fails like one: the harness gets the error,
    // and the sentence can never be persisted as an agent's work.
    if let Some(canned) = upstream.canned {
        return Err(match canned {
            "mock" => format!(
                "\"{}\" is not a rendered agent (the agents service answered in mock mode)",
                req.model
            ),
            _ => format!(
                "persona \"{}\" did not come back within the hold window",
                req.model
            ),
        });
    }
    let watch_tap = req.watch.clone().map(|channel| {
        let state = state.clone();
        std::sync::Arc::new(move |line: &str| {
            let state = state.clone();
            let channel = channel.clone();
            let line = format!("{line}\n");
            tokio::spawn(async move {
                // PUBLISH for the live watchers, APPEND for the replay tail
                // (one line per event). Quiet on every error: observability
                // must never fail the turn it observes.
                if let Ok(mut conn) = state.redis().await {
                    let _ = redis::cmd("PUBLISH")
                        .arg(&channel)
                        .arg(line.trim_end())
                        .query_async::<()>(&mut conn)
                        .await;
                    let _ = redis::cmd("APPEND")
                        .arg(format!("{channel}:tail"))
                        .arg(&line)
                        .query_async::<()>(&mut conn)
                        .await;
                }
            });
        }) as std::sync::Arc<dyn Fn(&str) + Send + Sync>
    });
    let turn = pump_persona_stream_alive(
        upstream.body,
        emit,
        req.liveness.as_ref(),
        watch_tap.as_ref(),
    )
    .await?;
    // A failure frame is a failed call, not an empty one — the canned-stream
    // rule one layer up, for the error that arrives wearing a 200. The harness
    // gets the reason (the fitness sweep narrows on it; a work session
    // surfaces it) instead of scoring an empty answer as the model's work.
    if let Some(message) = &turn.error {
        return Err(format!("persona \"{}\" errored: {message}", req.model));
    }
    Ok(TransportReply {
        kind: TransportKind::Fleet,
        text: turn.text,
        tool_names: turn.tool_names,
        tool_calls: None,
        usage: turn.usage,
        contract_dropped: persona_contract_dropped(req),
    })
}

/// The blocking fleet side.
pub async fn fleet_transport(
    state: &AppState,
    req: &TransportRequest,
) -> Result<TransportReply, String> {
    persona_turn(state, req, None, None).await
}

/// The blocking fleet side, on a Hermes SESSION the caller keys. The same
/// pump, the same meter, the same guard-visible reply as `fleet_transport`;
/// only the door differs — see `PersonaSession` for why a caller wants it
/// and `open_persona_turn` for the two ways it falls back to the other one.
pub async fn fleet_session_transport(
    state: &AppState,
    req: &TransportRequest,
    session: &PersonaSession,
) -> Result<TransportReply, String> {
    persona_turn(state, req, None, Some(session)).await
}

/// The same call, with each delta handed on as it lands — the briefing
/// panel's tee: one branch to the owner's screen, one to the guard.
/// Streaming is a property of the TRANSPORT, never of the harness contract.
pub async fn fleet_stream(
    state: &AppState,
    req: &TransportRequest,
    emit: impl FnMut(&str) + Send,
) -> Result<TransportReply, String> {
    let mut emit = emit;
    persona_turn(state, req, Some(&mut emit), None).await
}

/// Which side of the house a model lives on. The rule, stated once:
///
///    a model the ORG GATEWAY serves goes through `gateway_transport`;
///    a model that is a LIVE FLEET PERSONA goes through the proxy;
///    the gateway wins when a model is somehow both.
///
/// The gateway wins because it is the metered, fully-inspectable path: it
/// knows the endpoint, it writes the ledger row itself, and the runner gets
/// the whole message history to guard against. A persona's tool loop runs
/// inside the agent container, so that path can only ever offer tool names.
///
/// "Live" is load-bearing — `list_agents` returns an empty vec when the fleet
/// has never been rendered, so a mock manifest matches nothing here and the
/// call lands on the gateway, which says so precisely.
///
/// TIERS ROUTE TOO. The Plan modal's model-tier dropdown and `routed_model_for`
/// turn out ids like `<live agent id>-<alias>`, split at the LAST hyphen —
/// `list_agents` HIDES tier entries from the picker, so matching on its list
/// alone would classify a tier as a gateway model and fail the call with
/// "model X is not on the gateway". A caller that KNOWS it is routing a tier
/// should still say so with `RunContext.tier`, which needs no inference.
pub async fn transport_kind(state: &AppState, model: &str) -> TransportKind {
    if gateway_models(&state.pg)
        .await
        .map(|models| models.iter().any(|m| m.id == model))
        .unwrap_or(false)
    {
        return TransportKind::Gateway;
    }
    let fleet = list_agents().await;
    if fleet.iter().any(|a| a.id == model) {
        return TransportKind::Fleet;
    }
    if let Some(cut) = model.rfind('-')
        && cut > 0
        && fleet.iter().any(|a| a.id == model[..cut])
    {
        return TransportKind::Fleet;
    }
    // Neither: let the gateway say so. `gateway_transport` throws a precise
    // "model X is not on the gateway", which is a better failure than a
    // persona stream that looks like an answer.
    TransportKind::Gateway
}

/// The blocking picker. THE TRANSPORT RULE in one place so no harness author
/// ever restates it.
pub async fn dispatch_transport(
    state: &AppState,
    req: &TransportRequest,
) -> Result<TransportReply, String> {
    match transport_kind(state, &req.model).await {
        TransportKind::Fleet => fleet_transport(state, req).await,
        TransportKind::Gateway => gateway_transport(state, req).await,
    }
}

/// The same picker for a caller that KEYS THE PERSONA'S SESSION. The fleet
/// side lands on that session; the gateway side runs exactly as it always
/// does, because the session is an attribution key for a CONTAINER — the
/// transcript Hermes keeps, the plugin that reports against it — and a
/// gateway model has neither: its ledger row is written by the gateway
/// itself and there is no plugin whose frames could be misattributed. (A
/// harness that reaches here with `tools: own` on a gateway model is refused
/// by `gateway_transport` as it always was.) Reached through a dep override
/// rather than a request slot so the one place that keys a session — a work
/// session, by its run id — is the one place that says so.
pub async fn dispatch_session_transport(
    state: &AppState,
    req: &TransportRequest,
    session: &PersonaSession,
) -> Result<TransportReply, String> {
    match transport_kind(state, &req.model).await {
        TransportKind::Fleet => fleet_session_transport(state, req, session).await,
        TransportKind::Gateway => gateway_transport(state, req).await,
    }
}

/// The same choice for the streaming pair. It resolves the kind through the
/// one function above rather than repeating the gateway/fleet/tier lookup,
/// because two copies of "which side is this model on" is how a blocking call
/// and a streaming call of the SAME model end up on different transports —
/// and that failure looks like the stream being broken rather than like a
/// routing bug.
pub async fn dispatch_stream(
    state: &AppState,
    req: &TransportRequest,
    emit: impl FnMut(&str) + Send,
) -> Result<TransportReply, String> {
    match transport_kind(state, &req.model).await {
        TransportKind::Fleet => fleet_stream(state, req, emit).await,
        TransportKind::Gateway => gateway_stream(state, req, emit).await,
    }
}

// ── The probe-side transport helpers ─────────────────────────────────────────
// `offers_tool_definitions`, `runs_own_tool_loop`, `gateway_image_turn` and
// `persona_probe_turn` — the four doors the fitness suite reaches the model
// through that are NOT the harness runner. All four derive from
// `transport_kind` on purpose, so they can never disagree with the transport
// that would actually take the call.

/// CAN this model be offered tool definitions at all — asked BEFORE a call is
/// made, and derived from `transport_kind` rather than restated, so it can
/// never disagree with the transport that would actually refuse.
pub async fn offers_tool_definitions(state: &AppState, model: &str) -> bool {
    transport_kind(state, model).await == TransportKind::Gateway
}

/// CAN this model run a turn with ITS OWN tool loop — the exact complement of
/// the question above, and the pair is the whole transport rule read from both
/// ends: a persona has a tool loop and cannot be handed tool definitions; the
/// gateway can be handed definitions and has no loop. Neither does both.
///
/// THE FITNESS SWEEP ASKS THIS, and until it did, harnesses whose whole
/// feature is the tool loop were replayed against every ORG GATEWAY candidate,
/// refused before a single token was spent, and recorded as 0% first-pass. A
/// model that was never called scoring zero is the same class of
/// confidently-wrong number the probe suite's `skipped` outcome exists to
/// prevent (see evals' harness_skip_reason).
pub async fn runs_own_tool_loop(state: &AppState, model: &str) -> bool {
    transport_kind(state, model).await == TransportKind::Fleet
}

/// A GATEWAY TURN THAT CARRIES AN IMAGE. The blocking transport cannot serve
/// this: a `TransportRequest` has nowhere to put an image (`Message.content`
/// is a string by construction; see define.rs), so the vision probe's ask
/// takes messages and image URLs directly.
///
/// The LAST user turn carries the image, because that is the turn the
/// question is in — a system prompt with pictures attached is a shape
/// providers accept and models ignore.
pub async fn gateway_image_turn(
    state: &AppState,
    model: &str,
    messages: &[Message],
    images: &[String],
    caller: &str,
    timeout_ms: Option<u64>,
) -> Result<String, String> {
    let route = resolve_route(&state.pg, model)
        .await
        .map_err(|e| e.to_string())?
        .ok_or_else(|| format!("model \"{model}\" is not on the gateway"))?;
    let mut wire: Vec<Value> = messages
        .iter()
        .map(|m| serde_json::json!({ "role": m.role.as_str(), "content": m.content }))
        .collect();
    let target = wire
        .iter()
        .rposition(|m| m.get("role").and_then(Value::as_str) == Some("user"))
        .or(if wire.is_empty() {
            None
        } else {
            Some(wire.len() - 1)
        });
    if let Some(target) = target {
        let role = wire[target].get("role").cloned().unwrap_or(Value::Null);
        let text = wire[target]
            .get("content")
            .and_then(Value::as_str)
            .unwrap_or("")
            .to_string();
        let mut parts = vec![serde_json::json!({ "type": "text", "text": text })];
        parts.extend(
            images
                .iter()
                .map(|url| serde_json::json!({ "type": "image_url", "image_url": { "url": url } })),
        );
        wire[target] = serde_json::json!({ "role": role, "content": parts });
    }
    let body = serde_json::json!({ "model": model, "messages": wire, "stream": false });
    let mut call = build_upstream(state, &route, &body).await?;
    let fetched = fetch_upstream(&state.pg, &mut call, Some(&route));
    let reply = match timeout_ms {
        // The clock wraps the fetch rather than threading a timeout field
        // through the upstream call — one bound on the whole turn.
        Some(ms) => tokio::time::timeout(std::time::Duration::from_millis(ms), fetched)
            .await
            .map_err(|_| format!("gateway completion timed out after {ms}ms"))?,
        None => fetched.await,
    };
    let reply = reply?;
    if !reply.is_ok() {
        return Err(format!(
            "gateway completion {}: {}",
            reply.status(),
            reply.text().await.chars().take(300).collect::<String>()
        ));
    }
    let j: Value = serde_json::from_str(&reply.text().await)
        .map_err(|e| format!("gateway completion parse: {e}"))?;
    let text = j
        .pointer("/choices/0/message/content")
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string();
    let usage = j.get("usage").filter(|u| !u.is_null()).map(|u| TokenPair {
        prompt_tokens: u.get("prompt_tokens").and_then(Value::as_i64).unwrap_or(0),
        completion_tokens: u
            .get("completion_tokens")
            .and_then(Value::as_i64)
            .unwrap_or(0),
    });
    let prompt_chars: usize = messages
        .iter()
        .map(|m| talaria_body::utf16_len(&m.content))
        .sum();
    meter(
        state,
        &route,
        caller,
        usage,
        prompt_chars,
        talaria_body::utf16_len(&text),
        reported_spend(&route.endpoint.provider, &j),
    );
    Ok(text)
}

/// A PERSONA TURN THE PROBE SUITE CAN DRIVE — one blocking round trip against
/// a fleet agent, returning the assembled turn rather than a stream.
///
/// WHY IT IS NOT `fleet_transport`. Two reasons, and the second is the
/// load-bearing one. A probe wants the raw turn — text AND the tool names the
/// persona reported — without a harness's contract wrapped around it. And a
/// probe may carry an IMAGE, which a `TransportRequest` has nowhere to put.
/// So this takes messages and images directly, exactly as
/// `gateway_image_turn` does on the other side of the `offers_tool_definitions`
/// fork, and the vision probe can be asked of a persona and a gateway model
/// with the same call shape.
pub async fn persona_probe_turn(
    model: &str,
    messages: &[Message],
    images: &[String],
    timeout_ms: Option<u64>,
) -> Result<PersonaTurn, String> {
    let mut wire: Vec<Value> = messages
        .iter()
        .map(|m| serde_json::json!({ "role": m.role.as_str(), "content": m.content }))
        .collect();
    if !images.is_empty() {
        // The last user turn carries the image: it is the turn the question is
        // in, and a system prompt with pictures attached is a shape providers
        // accept and models ignore.
        let target = wire
            .iter()
            .rposition(|m| m.get("role").and_then(Value::as_str) == Some("user"))
            .or(if wire.is_empty() {
                None
            } else {
                Some(wire.len() - 1)
            });
        if let Some(target) = target {
            let role = wire[target].get("role").cloned().unwrap_or(Value::Null);
            let text = wire[target]
                .get("content")
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string();
            let mut parts = vec![serde_json::json!({ "type": "text", "text": text })];
            parts.extend(images.iter().map(
                |url| serde_json::json!({ "type": "image_url", "image_url": { "url": url } }),
            ));
            wire[target] = serde_json::json!({ "role": role, "content": parts });
        }
    }
    let payload = serde_json::json!({ "model": model, "messages": wire });
    // No flow to name: a probe's calls are metered at the relay under the
    // agent, `unattended`.
    let upstream = proxy_chat(&payload, timeout_ms, None).await;
    if !upstream.ok() {
        return Err(format!("persona gateway {}", upstream.status));
    }
    if let Some(canned) = upstream.canned {
        return Err(match canned {
            "mock" => format!(
                "\"{model}\" is not a rendered agent (the agents service answered in mock mode)"
            ),
            _ => format!("persona \"{model}\" did not come back within the hold window"),
        });
    }
    pump_persona_stream(upstream.body, None).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::define::{Message, Role};

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
            liveness: None,
            watch: None,
        }
    }

    /// THE SESSION ARM'S ONE MAPPING: system rows become the ephemeral
    /// prompt, the trailing user run becomes the message, and a request
    /// that carries history — the one thing a session turn cannot send —
    /// is handed back to the completions arm rather than sent with its
    /// transcript cut off.
    #[test]
    fn session_turn_parts_cut_the_prompt_and_refuse_history() {
        // The work session's shape: one user message, no system row.
        let one = session_turn_parts(&[Message::user("turn 3: continue")]);
        assert_eq!(one, Some((None, "turn 3: continue".into())));

        // System rows fold into one prompt; two queued user rows are one
        // message with a blank line between them.
        let folded = session_turn_parts(&[
            Message::system("mode prompt"),
            Message::system("   "),
            Message::system("sender block"),
            Message::user("first"),
            Message::user("second"),
        ]);
        assert_eq!(
            folded,
            Some((
                Some("mode prompt\n\nsender block".into()),
                "first\n\nsecond".into()
            ))
        );

        // An assistant row ahead of the trailing run is history the
        // container would not have: not a session turn.
        let mut history = vec![Message::system("s"), Message::user("q")];
        history.push(Message {
            role: Role::Assistant,
            content: "a".into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        });
        history.push(Message::user("q2"));
        assert_eq!(session_turn_parts(&history), None);
        // So is a request with nothing to say.
        assert_eq!(session_turn_parts(&[Message::system("s")]), None);
        assert_eq!(session_turn_parts(&[Message::user("  ")]), None);
    }

    /// A field the session body cannot carry fails the call, by name —
    /// never a quiet drop — and the work session's own shape (no
    /// temperature, text output, its own tools) passes.
    #[test]
    fn a_session_turn_refuses_what_its_body_cannot_carry() {
        let mut r = req(false);
        r.temperature = None;
        r.tools = Some(ToolPolicy::Own);
        assert_eq!(session_turn_refusal(&r), None);

        let mut t = r.clone();
        t.temperature = Some(0.0);
        assert!(
            session_turn_refusal(&t)
                .unwrap()
                .contains("declared temperature on a Hermes SESSION turn")
        );
        let mut j = r.clone();
        j.json_mode = true;
        assert!(
            session_turn_refusal(&j)
                .unwrap()
                .contains("response_format")
        );
        let mut n = r.clone();
        n.tools = None;
        assert!(session_turn_refusal(&n).unwrap().contains("tools: none"));
        // Effort is a dial, not a contract: it rides the session body's
        // `model_options` and refuses nothing.
        let mut e = r.clone();
        e.effort = Some("high".into());
        assert_eq!(session_turn_refusal(&e), None);
    }

    #[test]
    fn the_default_tool_policy_is_none_and_every_absent_field_has_a_reader() {
        // The readers exist so a transport cannot drop half a request by
        // reading fields by hand.
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
        // Hermes reads neither off a chat-completions body: they are decided
        // here and left off, and the JSON half is reported as dropped.
        assert!(p.get("temperature").is_none());
        assert!(p.get("response_format").is_none());
        assert!(persona_contract_dropped(&r));
        assert!(!persona_contract_dropped(&req(false)));
        // Still sent, though Hermes ignores both: the guard pass's
        // `zero_tool_claim` is what enforces 'none' on a persona.
        assert_eq!(p["tools"], serde_json::json!([]));
        assert_eq!(p["tool_choice"], "none");
        // The effort rides where Hermes reads it, never top-level.
        assert_eq!(p["model_options"]["reasoning_effort"], "low");
        assert!(p.get("reasoning_effort").is_none());
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
