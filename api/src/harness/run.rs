// THE RUNNER — port of harness/run.ts. The one piece of code that talks to a
// model on a harness's behalf: it resolves the model, holds the capability
// floor, widens on measured facts, renders, calls, parses, repairs, guards,
// redacts, applies the declared failure policy, and meters. Everything a
// harness DECLARS in `define.rs` is honored here, and nothing else in the tree
// decides any of it.
//
// WHY THE EDGES ARE INJECTED. Thirteen closures stand between the runner and
// the world — model resolution, slot effort, routing, persona keys, three
// capability looks, the transport, the guard config, the gate, the two
// recorders, and the clock. That is not test convenience; it is the same
// seam `runs/engine.rs` uses for its world. The runner's behavior is the
// product ("a 14B model returns something a frontier model would not, and
// here is what Talaria does with it"), and the only way to hold that still is
// to write the bad reply down and assert against it — which needs a transport
// that hands back recorded answers. The REAL edges are `real_deps` below; a
// `RunContext` may replace the set wholesale (tests, and later the fitness
// suite's replay transport).
//
// WHAT DELIBERATELY DID NOT CROSS YET:
//   - `RunContext.signal`. The TS runner forwards an AbortSignal to the
//     transport; `TransportRequest` has no slot for one until the streaming
//     surfaces cross (batch 5), so cancellation lands with them rather than as
//     a dead field here.
//   - the real transport edge is `gateway_transport` — the gateway turn. The
//     fleet transport, the streamed pair and `pickTransport` cross with the
//     fleet/streaming planes in batch 5; until then a caller that needs them
//     injects the edge, which is what the seam is for.
//
// THREE RULES THIS FILE OBEYS, restated from guardrails.ts because the runner
// is where they bite:
//   FLAGGED CONTENT NEVER RE-ENTERS A MODEL'S CONTEXT — the repair turn is the
//   one place the runner puts model output back in, so it goes through the
//   gate-safe rules first, and the repair prompt carries the parser error and
//   nothing else (never the finding, never its snippet).
//   A SECRET REDACTS AND RECORDS, NEVER BLOCKS — redaction re-applies the whole
//   contract to the scrubbed value, and a value that no longer parses is no
//   value, not a half-scrubbed one handed back anyway.
//   GROUNDED FINDINGS ARE NOT EVIDENCE — they survive on the result (for
//   `needs_redaction`, for callers pinning findings to their own rows) but are
//   counted neither on the run row nor in `guard_findings`.

use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

use serde_json::{Value, json};

use super::define::{
    Fallback, Grounding as GroundMaterial, HarnessDefinition, Message, OnFailure, Output,
    RenderContext, Role,
};
use super::json::{ParseResult, parse_json, repair_prompt};
use super::json_schema::{WireSchema, prompt_shape, wire_schema_of};
use super::transport::{
    LedgerAttribution, LedgerSource, TransportKind, TransportReply, TransportRequest,
    gateway_transport, tool_wire_message,
};
use crate::capability::{CapabilityFact, capability_key, get_capabilities, missing_capabilities};
use crate::capability_reach::{Reach, reach_for_keys};
use crate::effort_prefs::{agent_slot, role_slot, slot_effort_for_model};
use crate::gateway::guard::{self, Available, Finding, GuardConfig, GuardMode, ToolRecord};
use crate::gateway::registry::routing_for;
use crate::harness_model::{self, ModelChainStep, ModelSpec};
use crate::persona;
use crate::state::AppState;

// ── The result ───────────────────────────────────────────────────────────────

/// The thrown half of `OnFailure::Throw`: the one sentence in the tree for
/// "this harness failed" (`harness "<id>" failed on "<model>": <why>`), thrown
/// AFTER the run row is written — a throwing harness is precisely the one an
/// operator needs to see in the fitness data.
#[derive(Debug)]
pub struct HarnessError(pub String);

impl std::fmt::Display for HarnessError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for HarnessError {}

/// One harness run's outcome. See the TS `HarnessResult` docs for the two
/// columns that are deliberately not comparable across harnesses
/// (`schema_valid`) and the two flags that exist because `answered`/`refused`
/// answer different questions than `raw`/`error` do.
#[derive(Debug, Clone)]
pub struct HarnessResult {
    /// The harness's value — a parsed `Value` for a JSON harness, a
    /// `Value::String` for a text one (the type-erasure `define.rs` states).
    pub value: Option<Value>,
    /// The ROUTED id — the model that answered, not the base persona the
    /// ledger names (that one is reconstructed from `tier` + the transports).
    pub model: Option<String>,
    pub step: Option<ModelChainStep>,
    pub widened: bool,
    pub repairs: u32,
    pub schema_valid: bool,
    /// DID THE MODEL ACTUALLY ANSWER — a completed transport call with
    /// something to apply the contract to. False for every way a run ends
    /// without a reply; true the moment one arrives, whatever happens to it.
    pub answered: bool,
    /// THE HARNESS DECLINED TO ASK — the floor refused, so nothing here is a
    /// fact about the model. A refusal is a SKIP, the absence of evidence.
    pub refused: bool,
    pub findings: Vec<Finding>,
    /// The model's last raw reply, bounded. A drill-down field, not an archive.
    pub raw: Option<String>,
    pub latency_ms: i64,
    /// The harness declared escalation and the contract failed. A FLAG, not a
    /// phrase in `error` — only the caller knows who to tell.
    pub escalate: bool,
    pub error: Option<String>,
}

/// The `harness_runs` row every exit writes — the production ground truth
/// behind contract rate and repair rate per harness per model over time.
#[derive(Debug, Clone)]
pub struct HarnessRunRow {
    pub harness: String,
    pub model: Option<String>,
    pub step: Option<ModelChainStep>,
    pub widened: bool,
    pub repairs: u32,
    pub schema_valid: bool,
    pub latency_ms: i64,
    /// How many guard findings this run is EVIDENCE for — grounded ones
    /// excluded, exactly as `record_findings` excludes them from
    /// `guard_findings`. The fitness page reads the two side by side.
    pub findings: i64,
    pub caller: String,
    /// WHY the run failed, in one sentence, or None when it did not.
    pub error: Option<String>,
}

// ── Injected edges ───────────────────────────────────────────────────────────

/// Every edge returns a future that owns its inputs (the closures clone what
/// they need before boxing), so the runner can await them without borrowing
/// the def or the context.
pub type BoxFut<T> = Pin<Box<dyn Future<Output = T> + Send>>;

/// The spec's 'static half comes from the def; the caller's runtime user id
/// (which enables the 'preferred' step) rides alongside rather than inside,
/// because a `ModelSpec<'static>` cannot borrow a `RunContext` field and the
/// edge's boxed future owns everything it touches. The edge merges them —
/// exactly TS's `{ ...def.model, ...(ctx.userId ? { userId } : {}) }`.
pub type ResolveModelFn = Arc<
    dyn Fn(ModelSpec<'static>, Option<String>) -> BoxFut<Option<(String, ModelChainStep)>>
        + Send
        + Sync,
>;
pub type SlotEffortFn = Arc<dyn Fn(String, String) -> BoxFut<Option<String>> + Send + Sync>;
pub type RoutingFn = Arc<dyn Fn(String) -> BoxFut<(Vec<String>, String)> + Send + Sync>;
pub type PersonaKeysFn = Arc<dyn Fn(String) -> BoxFut<Vec<String>> + Send + Sync>;
pub type MissingCapabilitiesFn =
    Arc<dyn Fn(String, Vec<String>) -> BoxFut<Vec<String>> + Send + Sync>;
pub type CapabilitiesFn =
    Arc<dyn Fn(String) -> BoxFut<HashMap<String, CapabilityFact>> + Send + Sync>;
pub type ReachFn =
    Arc<dyn Fn(Vec<String>, Vec<String>) -> BoxFut<HashMap<String, Reach>> + Send + Sync>;
pub type TransportFn =
    Arc<dyn Fn(TransportRequest) -> BoxFut<Result<TransportReply, String>> + Send + Sync>;
pub type GuardConfigFn = Arc<dyn Fn() -> BoxFut<Option<GuardConfig>> + Send + Sync>;
pub type GuardTextFn = Arc<dyn Fn(String, Option<String>) -> BoxFut<Vec<Finding>> + Send + Sync>;
pub type RecordFindingsFn = Arc<dyn Fn(Vec<Finding>, FindingMeta) -> BoxFut<()> + Send + Sync>;
pub type RecordRunFn = Arc<dyn Fn(HarnessRunRow) -> BoxFut<()> + Send + Sync>;
pub type NowFn = Arc<dyn Fn() -> i64 + Send + Sync>;

/// Where the guard's findings were filed — the half of `recordFindings`'s meta
/// the runner knows and a caller never restates.
#[derive(Debug, Clone)]
pub struct FindingMeta {
    pub caller: String,
    pub model: String,
    pub endpoint: Option<String>,
    pub mode: GuardMode,
}

/// The runner's world. Every TS `.catch(() => …)` on a dep edge has moved
/// INTO `real_deps` — the real edges degrade (routing errors to no endpoints,
/// a dead settings read to defaults) and the stub edges a test supplies never
/// fail, so the runner itself holds no error policy for its own telemetry.
pub struct HarnessDeps {
    pub resolve_model: ResolveModelFn,
    /// The admin's slot-level effort preference, validated against the model's
    /// live published levels — or None.
    pub slot_effort: SlotEffortFn,
    /// Where a model CAN land, without advancing the round-robin cursor:
    /// (endpoint names, the upstream model id).
    pub routing: RoutingFn,
    /// Capability keys a FLEET PERSONA inherits from the model behind it.
    pub persona_keys: PersonaKeysFn,
    pub missing_capabilities: MissingCapabilitiesFn,
    pub capabilities: CapabilitiesFn,
    /// CAN THE RUN reach these capabilities — natively, or through a tool this
    /// install has registered. Consulted only for capabilities a harness
    /// declares suppliable, and only when the floor is otherwise about to
    /// refuse, so the registry read never lands on a path that would not have
    /// used it.
    pub reach: ReachFn,
    pub transport: TransportFn,
    /// None is TS's `.catch(() => null)`: a guard config that could not be
    /// read means no guard pass and no findings, not a failed run.
    pub guard_config: GuardConfigFn,
    /// The gate-safe rules over plain text — `input` is the turn's own
    /// grounding material.
    pub guard_text: GuardTextFn,
    pub record_findings: RecordFindingsFn,
    pub record_run: RecordRunFn,
    pub now: NowFn,
}

/// The real edges: one PgPool (and the gateway transport's state handle)
/// cloned into thirteen closures. Nothing here fails outward — every TS
/// `.catch` on a dep call is reproduced as a degradation inside the edge.
pub fn real_deps(state: &AppState) -> HarnessDeps {
    // One pool clone per edge: each closure is `move`, and the pool is not
    // Copy. Cheap — a PgPool is a handle on an Arc'd connection pool.
    let st = state.clone();
    HarnessDeps {
        resolve_model: {
            let pg = state.pg.clone();
            Arc::new(move |spec, user_id| {
                let pg = pg.clone();
                Box::pin(async move {
                    let spec = ModelSpec {
                        user_id: user_id.as_deref().or(spec.user_id),
                        ..spec
                    };
                    harness_model::resolve_harness_model(&pg, &spec)
                        .await
                        .ok()
                        .flatten()
                        .map(|r| (r.model, r.step))
                })
            })
        },
        slot_effort: {
            let pg = state.pg.clone();
            Arc::new(move |slot, model| {
                let pg = pg.clone();
                Box::pin(async move { slot_effort_for_model(&pg, &slot, &model).await })
            })
        },
        routing: {
            let pg = state.pg.clone();
            Arc::new(move |model| {
                let pg = pg.clone();
                Box::pin(async move {
                    match routing_for(&pg, &model).await {
                        Ok(r) => (
                            r.endpoints.iter().map(|e| e.name.clone()).collect(),
                            r.upstream_model,
                        ),
                        // A routing lookup exists to make a run better; a
                        // database blip must answer "no endpoints", never
                        // fail the run.
                        Err(_) => (Vec::new(), model),
                    }
                })
            })
        },
        persona_keys: {
            let pg = state.pg.clone();
            Arc::new(move |model| {
                let pg = pg.clone();
                Box::pin(async move { persona::persona_capability_keys(&pg, &model).await })
            })
        },
        missing_capabilities: {
            let pg = state.pg.clone();
            Arc::new(move |key, asked| {
                let pg = pg.clone();
                Box::pin(async move {
                    let asked: Vec<&str> = asked.iter().map(String::as_str).collect();
                    missing_capabilities(&pg, &key, &asked).await
                })
            })
        },
        capabilities: {
            let pg = state.pg.clone();
            Arc::new(move |key| {
                let pg = pg.clone();
                Box::pin(async move { get_capabilities(&pg, &key).await })
            })
        },
        reach: {
            let pg = state.pg.clone();
            Arc::new(move |keys, wanted| {
                let pg = pg.clone();
                Box::pin(async move {
                    let wanted: Vec<&str> = wanted.iter().map(String::as_str).collect();
                    reach_for_keys(&pg, &keys, &wanted).await
                })
            })
        },
        transport: {
            let st = st.clone();
            Arc::new(move |req| {
                let st = st.clone();
                Box::pin(async move { gateway_transport(&st, &req).await })
            })
        },
        guard_config: {
            let pg = state.pg.clone();
            Arc::new(move || {
                let pg = pg.clone();
                Box::pin(async move { Some(guard::guard_config(&pg).await) })
            })
        },
        guard_text: {
            let pg = state.pg.clone();
            Arc::new(move |text, input| {
                let pg = pg.clone();
                Box::pin(async move { guard::guard_text(&pg, &text, input.as_deref()).await })
            })
        },
        record_findings: {
            let pg = state.pg.clone();
            Arc::new(move |findings, meta| {
                let pg = pg.clone();
                Box::pin(async move {
                    guard::record_findings(
                        &pg,
                        &findings,
                        &meta.caller,
                        &meta.model,
                        meta.endpoint.as_deref(),
                        meta.mode,
                    )
                    .await
                })
            })
        },
        record_run: {
            let pg = state.pg.clone();
            Arc::new(move |row| {
                let pg = pg.clone();
                Box::pin(async move {
                    // Swallowed on purpose: a run row is telemetry, and a
                    // database hiccup at meter time must not fail a run that
                    // succeeded (or re-fail one that already did). Same
                    // posture as the TS `.catch(() => {})`.
                    let _ = sqlx::query(
                        "insert into harness_runs \
                         (harness, model, chain_step, widened, repairs, schema_valid, latency_ms, findings, caller, error) \
                         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
                    )
                    .bind(&row.harness)
                    .bind(&row.model)
                    .bind(row.step)
                    .bind(row.widened)
                    .bind(row.repairs as i32)
                    .bind(row.schema_valid)
                    .bind(row.latency_ms)
                    .bind(row.findings)
                    .bind(&row.caller)
                    .bind(&row.error)
                    .execute(&pg)
                    .await;
                })
            })
        },
        now: Arc::new(now_ms),
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// THE CAPABILITY KEYS A ROUTED MODEL ANSWERS FOR — one per endpoint that
/// could take the call, because a bare model name may be served by a POOL and
/// we cannot know which member will take this one without advancing the
/// round-robin cursor.
pub fn capability_keys_of(endpoints: &[String], upstream_model: &str) -> Vec<String> {
    endpoints
        .iter()
        .map(|e| capability_key(e, upstream_model))
        .collect()
}

/// The keys for a model id, resolved through the REAL routing — the
/// convenience form for callers outside the runner. Falls back to a persona's
/// inherited keys exactly as the runner does.
pub async fn capability_keys_for(pg: &sqlx::PgPool, model: &str) -> Vec<String> {
    let (endpoints, upstream) = match routing_for(pg, model).await {
        Ok(r) => (
            r.endpoints
                .iter()
                .map(|e| e.name.clone())
                .collect::<Vec<_>>(),
            r.upstream_model,
        ),
        Err(_) => (Vec::new(), model.to_string()),
    };
    let keys = capability_keys_of(&endpoints, &upstream);
    if keys.is_empty() {
        persona::persona_capability_keys(pg, model).await
    } else {
        keys
    }
}

// ── The context ──────────────────────────────────────────────────────────────

/// What the CALLER knows about where this turn's spend belongs. The runner
/// fills in the rest of `LedgerAttribution` — `agent_model` is the resolved
/// base model and `tier` is `RunContext.tier`, neither of which a caller
/// should have to restate.
#[derive(Debug, Clone, Default)]
pub struct RunLedger {
    pub source: Option<LedgerSource>,
    /// The conversation, channel or research run.
    pub ref_id: Option<String>,
    /// The ticket, so the turn reaches the ticket's cost and not just the
    /// ledger.
    pub task_id: Option<String>,
}

#[derive(Clone, Default)]
pub struct RunContext {
    /// Ledger + findings attribution, e.g. 'platform:titler', 'ticket:<id>'.
    pub caller: String,
    /// For user-scoped harnesses: enables the chain's 'preferred' step and the
    /// member model allowlist.
    pub user_id: Option<String>,
    /// Pin the model, skipping resolution entirely. This is how the fitness
    /// suite replays a harness against a candidate model, how every harness
    /// whose model comes from the SUBJECT of the call names it, and the only
    /// supported way to bypass the chain.
    pub model: Option<String>,
    /// Which chain step produced `ctx.model`, when the caller ran the chain
    /// ITSELF and is only handing the answer over. Exists so pre-resolving
    /// does not silently erase the step from the `harness_runs` row. Ignored
    /// unless `model` is set.
    pub step: Option<ModelChainStep>,
    /// Route this turn to an ALIAS TIER of the resolved model — the alias
    /// NAME ('opus'), not the routed id. Naming the tier here is how a caller
    /// gets the routing and the ledger price together; inference from a routed
    /// id cannot recover either lookup.
    pub tier: Option<String>,
    /// THE REASONING EFFORT THIS TURN RUNS AT, when the caller's surface let a
    /// human pick one. Absent means the model's own default.
    pub effort: Option<String>,
    /// Where this turn's spend belongs. See `RunLedger`.
    pub ledger: Option<RunLedger>,
    /// Replace the whole dep set — tests supply a recorded transport and
    /// no-op recorders; the fitness suite supplies a replay transport. (The
    /// TS field is a Partial override; the Rust runner takes the set
    /// wholesale, which is the only spelling the tests have ever needed —
    /// `real_deps` is one call.)
    pub deps: Option<Arc<HarnessDeps>>,
}

/// Where the streamed deltas go as they arrive — the browser's chunks. Called
/// on the RAW reply, before any guard pass has run, because by then the bytes
/// are already on the wire. A surface that must scrub what it relays redacts
/// here, on the way out, the only place it can work.
pub type DeltaFn = Arc<dyn Fn(&str) + Send + Sync>;

/// The streaming transport: the same request every transport gets, plus the
/// `emit` it must call with each delta as the delta arrives.
pub type StreamFn =
    Arc<dyn Fn(TransportRequest, DeltaFn) -> BoxFut<Result<TransportReply, String>> + Send + Sync>;

/// What `run_harness_streamed` needs beyond a normal run. The same runner
/// with the repair loop switched off — see `run_harness_streamed`.
#[derive(Clone)]
pub struct StreamOptions {
    pub stream: StreamFn,
    pub on_delta: Option<DeltaFn>,
}

// ── Small pure helpers ───────────────────────────────────────────────────────

/// The instruction every structured call carries. ONE wording, so the two
/// halves of a feature can never disagree, and so a change to it is
/// measurable across every harness at once.
const JSON_ANCHOR: &str = "Reply with exactly one JSON value and nothing else - no explanation before or after it, and no markdown code fence.";

/// `promptShape`'s default shape budget (json-schema.ts).
const SHAPE_BUDGET: usize = 600;

/// The anchor, plus THE SHAPE when this build can render one — one line of
/// prompt that engineers away the frontier/14B difference in what "reply with
/// JSON" means. Sent even when `response_format` carries the schema at the
/// protocol level, because a provider can drop the parameter and the prompt
/// survives it.
fn json_anchor_for(wire: Option<&WireSchema>) -> String {
    match wire.and_then(|w| prompt_shape(&w.schema, SHAPE_BUDGET)) {
        Some(shape) => format!("{JSON_ANCHOR}\n\nIt must match this shape exactly:\n{shape}"),
        None => JSON_ANCHOR.to_string(),
    }
}

/// Appended to the LAST USER TURN rather than sent as a new message: a small
/// model weights the end of its prompt most heavily, and a trailing standalone
/// instruction reads as something to acknowledge instead of a constraint on
/// the answer.
fn anchor_json(messages: &[Message], anchor: &str) -> Vec<Message> {
    let mut out = messages.to_vec();
    match out.iter().rposition(|m| m.role == Role::User) {
        None => out.push(Message::user(anchor.to_string())),
        Some(i) => out[i].content = format!("{}\n\n{}", out[i].content, anchor),
    }
    out
}

/// The message list in the wire shape the guard reads (`grounding_text_of`
/// and `extract_tool_record` both speak it). The tool channel rides along
/// through `tool_wire_message` so a tool-bearing turn grounds honestly.
fn wire_messages(messages: &[Message]) -> Vec<Value> {
    messages
        .iter()
        .map(|m| {
            if m.role.is_tool() || !m.tool_calls.is_empty() {
                tool_wire_message(m)
            } else {
                json!({ "role": m.role.as_str(), "content": m.content })
            }
        })
        .collect()
}

enum Applied {
    Ok(Value),
    Err(String),
}

/// Raw reply -> the harness's value. Shared by the main loop and the redaction
/// re-run below, so a redacted value is held to exactly the same contract as
/// the original. `clean` and `verify` are harness-author code running on model
/// output; a throw out of either is a failed contract, spelled the same as a
/// null — never the one thing that escapes a runner whose whole promise is
/// that a bad model produces a RESULT rather than an exception.
fn apply_output(def: &HarnessDefinition, raw: &str, input: &Value, ctx: &RenderContext) -> Applied {
    let parsed = match parse_value(def, raw) {
        Applied::Ok(v) => v,
        Applied::Err(e) => return Applied::Err(e),
    };
    verified(def, parsed, input, ctx)
}

fn parse_value(def: &HarnessDefinition, raw: &str) -> Applied {
    match &def.output {
        Output::Json { schema, .. } => match parse_json(raw, schema) {
            ParseResult::Ok(v) => Applied::Ok(v),
            ParseResult::Err { error, .. } => Applied::Err(error),
        },
        Output::Text { clean, .. } => {
            let Some(clean) = clean else {
                // A text harness that declares no `clean` is by construction a
                // string; this is the ONLY place that assumption lives.
                return if raw.trim().is_empty() {
                    Applied::Err("the model returned nothing".into())
                } else {
                    Applied::Ok(Value::String(raw.to_string()))
                };
            };
            match clean(raw) {
                Ok(Some(v)) => Applied::Ok(v),
                Ok(None) => Applied::Err("the reply did not survive the harness clean step".into()),
                Err(e) => Applied::Err(format!("the harness clean step threw on the reply: {e}")),
            }
        }
    }
}

/// THE HALF OF THE CONTRACT A SCHEMA CANNOT STATE. Runs ONLY on a value that
/// already parsed, so a verify never re-checks a type; its sentence is the
/// contract error verbatim — which is what puts it in the repair prompt and
/// on the run row. Identical handling to a parse failure, deliberately.
fn verified(def: &HarnessDefinition, value: Value, input: &Value, ctx: &RenderContext) -> Applied {
    let verify = match &def.output {
        Output::Json { verify, .. } | Output::Text { verify, .. } => verify,
    };
    let Some(verify) = verify else {
        return Applied::Ok(value);
    };
    match verify(&value, input, ctx) {
        Ok(None) => Applied::Ok(value),
        Ok(Some(message)) => Applied::Err(if message.trim().is_empty() {
            // An empty sentence is an author mistake, not a pass; failing
            // with a generic line keeps the contract honest without
            // sending the model a repair that names no problem to fix.
            "the value did not satisfy the harness output check".to_string()
        } else {
            message
        }),
        Err(e) => Applied::Err(format!("the harness verify step threw on the reply: {e}")),
    }
}

/// Merge guard findings without double-counting. The repair gate and the
/// final guard pass scan THE SAME REPLY, so a reply refused a repair over a
/// credential must not be recorded twice for one leak — `guard_findings` is
/// the live per-model confabulation rate the fitness page reads. Identity is
/// check + snippet: the same rule on a DIFFERENT span is a second, real
/// finding and must survive.
fn merge_findings(into: &mut Vec<Finding>, found: Vec<Finding>) {
    for f in found {
        if into
            .iter()
            .any(|prev| prev.check == f.check && prev.snippet == f.snippet)
        {
            continue;
        }
        into.push(f);
    }
}

/// The failure sentence as it goes into `harness_runs`. REDACTED FIRST, THEN
/// BOUNDED — slicing first can cut a credential in half so no pattern matches
/// it and the tail lands in the table verbatim. (The bound is chars, not
/// UTF-16 units: the difference is astral-plane-only and this string is an
/// error sentence, not model prose.)
const ERROR_CAP: usize = 1_000;

fn run_error(error: Option<&str>) -> Option<String> {
    error.map(|e| {
        let redacted = guard::redact_secrets(e, None).0;
        redacted.chars().take(ERROR_CAP).collect()
    })
}

/// What the drill-down shows — bounded because a run row is telemetry, not an
/// archive, and a model that answers with 200KB of prose must not be able to
/// turn one failed run into a memory problem for whatever reads it.
const RAW_CAP: usize = 8_000;

fn raw_of(t: &str) -> Option<String> {
    if t.is_empty() {
        None
    } else {
        Some(t.chars().take(RAW_CAP).collect())
    }
}

/// The same bound `extract_tool_record` puts on a derived record, applied to a
/// declared one for the same reason: past it, `ungrounded_ref` is scanning a
/// haystack big enough to be a performance problem for a check whose whole
/// virtue is that it is cheap. Overflowing FAILS OPEN — the rule skips.
const GROUND_RESULTS_CAP: usize = 200_000;

/// The harness's own account of what really ran this turn, or None. NULL IS
/// THE SAFE ANSWER AND IT IS THE DEFAULT IN THREE WAYS: no hook, a hook that
/// returns None, and a hook that returns an EMPTY tool list — the last one
/// matters because `ungrounded_ref` already declines on empty backing tools
/// but `fabricated_outage` does not, so a toolless "grounding" must not be
/// allowed to assert `error_info`. `ground` is harness-author code over
/// harness input, so a throw is a missing record, never an escaped exception.
fn grounding_for(def: &HarnessDefinition, input: &Value) -> Option<GroundMaterial> {
    let ground = def.ground.as_ref()?;
    ground(input)
        .unwrap_or_default()
        .filter(|m| !m.tools.is_empty())
}

// ── The runner ───────────────────────────────────────────────────────────────

/// One harness run: resolve, floor, widen, render, call, parse, repair,
/// guard, redact, apply the failure policy, meter.
pub async fn run_harness(
    state: &AppState,
    def: &HarnessDefinition,
    input: &Value,
    mut ctx: RunContext,
) -> Result<HarnessResult, HarnessError> {
    let deps = ctx
        .deps
        .take()
        .unwrap_or_else(|| Arc::new(real_deps(state)));
    execute(&deps, def, input, ctx, None).await
}

/// THE STREAMING ENTRY POINT — for the surfaces where tokens landing on a
/// screen ARE the feature. The same runner with a pumping transport and the
/// repair loop switched off: a repair turn re-asks and replaces the answer,
/// and the first answer already reached the screen. `def.guard.redact` still
/// cleans the returned VALUE, but the bytes are gone — a surface that must
/// scrub what it relays does it in `on_delta`, on the way out.
pub async fn run_harness_streamed(
    state: &AppState,
    def: &HarnessDefinition,
    input: &Value,
    mut ctx: RunContext,
    opts: StreamOptions,
) -> Result<HarnessResult, HarnessError> {
    let deps = ctx
        .deps
        .take()
        .unwrap_or_else(|| Arc::new(real_deps(state)));
    execute(&deps, def, input, ctx, Some(opts)).await
}

/// Everything `finish` needs besides the parts filled in at the end. The
/// shape every early exit spells explicitly, `empty()` being the floor.
#[derive(Default)]
struct Core {
    value: Option<Value>,
    model: Option<String>,
    step: Option<ModelChainStep>,
    widened: bool,
    repairs: u32,
    schema_valid: bool,
    escalate: bool,
    answered: bool,
    refused: bool,
    raw: Option<String>,
    error: Option<String>,
}

/// Every exit writes a harness_runs row, including the ones that never reach
/// a model. A harness that resolves nothing, or refuses on a capability, is
/// exactly the thing the fitness UI has to be able to see.
async fn finish(
    deps: &HarnessDeps,
    def_id: &str,
    caller: &str,
    findings: Vec<Finding>,
    started: i64,
    core: Core,
) -> HarnessResult {
    let latency_ms = ((deps.now)() - started).max(0);
    let row = HarnessRunRow {
        harness: def_id.to_string(),
        model: core.model.clone(),
        step: core.step,
        widened: core.widened,
        repairs: core.repairs,
        schema_valid: core.schema_valid,
        latency_ms,
        findings: findings.iter().filter(|f| !f.grounded).count() as i64,
        caller: caller.to_string(),
        error: run_error(core.error.as_deref()),
    };
    (deps.record_run)(row).await;
    HarnessResult {
        value: core.value,
        model: core.model,
        step: core.step,
        widened: core.widened,
        repairs: core.repairs,
        schema_valid: core.schema_valid,
        answered: core.answered,
        refused: core.refused,
        findings,
        raw: core.raw,
        latency_ms,
        escalate: core.escalate,
        error: core.error,
    }
}

/// THE FAILURE POLICY ON A RETURN PATH. `run_harness` returns rather than
/// throws for every failure that happens BEFORE or DURING the call, so
/// `OnFailure::Throw` covers the contract failure and everything before it —
/// and ONLY Throw widens: the other three policies describe what a caller
/// gets when a model ANSWERED and the answer was unusable (`answered` is how
/// a caller asks for either deliberately).
async fn fail(
    deps: &HarnessDeps,
    def: &HarnessDefinition,
    caller: &str,
    findings: Vec<Finding>,
    started: i64,
    core: Core,
) -> Result<HarnessResult, HarnessError> {
    let result = finish(deps, def.id, caller, findings, started, core).await;
    if matches!(def.on_failure, OnFailure::Throw) {
        let on = result
            .model
            .as_ref()
            .map(|m| format!(" on \"{m}\""))
            .unwrap_or_default();
        return Err(HarnessError(format!(
            "harness \"{}\" failed{}: {}",
            def.id,
            on,
            result
                .error
                .as_deref()
                .unwrap_or("the harness produced no value")
        )));
    }
    Ok(result)
}

/// The deps-injected entry. `run_harness`/`run_harness_streamed` are the
/// production spellings (they inject `real_deps`); this one exists for the
/// recorded world — every def's tests, and later the fitness sweep, drive the
/// SAME runner the product drives rather than a copy of it.
pub(crate) async fn execute(
    deps: &HarnessDeps,
    def: &HarnessDefinition,
    input: &Value,
    ctx: RunContext,
    streaming: Option<StreamOptions>,
) -> Result<HarnessResult, HarnessError> {
    let caller = ctx.caller.clone();
    let started = (deps.now)();
    let mut findings: Vec<Finding> = Vec::new();
    let empty = Core::default();

    // 1 ─ Resolve the model. None is an ANSWER, not an exception: on an
    // install whose gateway serves nothing this spec can reach, the harness
    // has no model and the caller keeps what it had.
    let model: String;
    let step: Option<ModelChainStep>;
    if let Some(pinned) = &ctx.model {
        // An explicit pin has no chain step — unless the caller ran the chain
        // itself and said which step won.
        model = pinned.clone();
        step = ctx.step;
    } else {
        let spec = ModelSpec {
            pin: def.model.pin,
            role: def.model.role,
            chain: def.model.chain,
            user_id: def.model.user_id,
        };
        match (deps.resolve_model)(spec, ctx.user_id.clone()).await {
            Some((m, s)) => {
                model = m;
                step = Some(s);
            }
            None => {
                return fail(
                    deps,
                    def,
                    &caller,
                    findings,
                    started,
                    Core {
                        error: Some(format!(
                            "no model available for harness \"{}\" - nothing in its chain routes on this gateway",
                            def.id
                        )),
                        ..empty
                    },
                )
                .await;
            }
        }
    }

    // THE ID ACTUALLY CALLED. `model` stays the BASE persona — it is what the
    // ledger has to name, because `recordUsage` prices a row by finding the
    // agent def and then the alias named by `tier`, and a routed id matches
    // neither. `routed` is what goes on the wire, what the capability lookup
    // asks about, and what the result and the run row name.
    let routed = match &ctx.tier {
        Some(tier) => format!("{model}-{tier}"),
        None => model.clone(),
    };

    // THE SLOT THAT PRODUCED THIS MODEL, when one did — the admin's effort
    // preference hangs off the WORK, not the model. Only the pin and role
    // steps carry a slot the admin configured; a model the chain found by
    // itself has no slot and no preference, which is correct.
    let slot = match step {
        Some("pin") => def.model.pin.map(agent_slot),
        Some("role") => def.model.role.map(role_slot),
        Some("utility") => Some(role_slot("utility")),
        _ => None,
    };
    // PRECEDENCE, in one line: the nearer the ask, the stronger it is — a
    // conversation pick beats an agent-configured default, and both beat the
    // slot preference.
    let effort = match &ctx.effort {
        Some(e) => Some(e.clone()),
        None => match &slot {
            Some(slot) => (deps.slot_effort)(slot.clone(), routed.clone()).await,
            None => None,
        },
    };

    // Capability facts are keyed 'endpoint:model', because capability is a
    // property of the ENDPOINT serving the model. A bare name may be served
    // by a POOL, and we cannot know which member will take this call without
    // advancing the round-robin cursor — so both questions below are answered
    // UNANIMOUSLY over the pool: missing only if every member says missing,
    // earned only if every member says earned.
    let (route_endpoints, upstream_model) = (deps.routing)(routed.clone()).await;
    let mut keys = capability_keys_of(&route_endpoints, &upstream_model);
    let endpoint = if route_endpoints.len() == 1 {
        route_endpoints.first().cloned()
    } else {
        None
    };

    // A FLEET PERSONA is not a gateway catalog model, so routing answers with
    // no endpoints for one — which is precisely the condition that used to
    // leave `keys` empty and made `widened` a constant false on the very path
    // the widening feature was built for. A persona is BACKED by a real
    // endpoint + upstream model, so it inherits that model's probe.
    if keys.is_empty() {
        keys = (deps.persona_keys)(routed.clone()).await;
    }

    // 2 ─ The floor. UNKNOWN IS NOT MISSING: an untested model runs, because
    // a fresh self-host has probed nothing and Talaria cannot refuse to work
    // until an admin gets around to benchmarking.
    let mut asked: Vec<&str> = Vec::new();
    for cap in def
        .requires
        .iter()
        .copied()
        .chain(def.floor.capabilities.iter().copied())
        .chain(def.output.is_json().then_some("json"))
    {
        if !asked.contains(&cap) {
            asked.push(cap);
        }
    }
    let mut missing: Vec<&str> = asked.clone();
    for key in &keys {
        let m =
            (deps.missing_capabilities)(key.clone(), asked.iter().map(|s| s.to_string()).collect())
                .await;
        missing.retain(|c| m.iter().any(|x| x == c));
    }
    if keys.is_empty() {
        missing.clear();
    }
    let blocking: Vec<&str> = missing
        .iter()
        .copied()
        .filter(|c| def.floor.capabilities.contains(c))
        .collect();
    if !blocking.is_empty() && def.floor.refuse_below {
        // A LEARNED FACT SHAPES THE REQUEST. IT DOES NOT REFUSE THE RUN. The
        // gateway writes `json: false` the first time an upstream 400s on
        // `response_format` — evidence about ONE PARAMETER, not a measurement
        // of whether the model can produce JSON. A refusal needs DELIBERATE
        // evidence: a probe that measured the model, or a human who declared
        // it. NEITHER 'learned' NOR 'catalog' IS SUCH EVIDENCE. Same
        // unanimity rule as `missing` above — every key in the pool has to
        // carry it, because refusing is the harmful direction.
        let facts = {
            let mut all: Vec<HashMap<String, CapabilityFact>> = Vec::new();
            for key in &keys {
                all.push((deps.capabilities)(key.clone()).await);
            }
            all
        };
        let deliberate = |cap: &str| {
            facts.iter().all(|f| {
                f.get(cap)
                    .map(|fact| !fact.value && fact.source != "learned" && fact.source != "catalog")
                    .unwrap_or(false)
            })
        };
        let measured: Vec<&str> = blocking.iter().copied().filter(|c| deliberate(c)).collect();

        // THE PLATFORM MAY SUPPLY WHAT THE MODEL LACKS — this is where
        // "capability of the model" stops being the question and "capability
        // of the run" becomes it. Asked only when it could change the answer.
        let suppliable: Vec<String> = measured
            .iter()
            .filter(|c| def.floor.suppliable.contains(c))
            .map(|c| c.to_string())
            .collect();
        let supplied: Vec<String> = if suppliable.is_empty() {
            Vec::new()
        } else {
            (deps.reach)(keys.clone(), suppliable)
                .await
                .values()
                .filter(|r| r.reached)
                .map(|r| r.capability.clone())
                .collect()
        };
        let unreachable: Vec<&str> = measured
            .iter()
            .copied()
            .filter(|c| !supplied.iter().any(|s| s == c))
            .collect();

        if !unreachable.is_empty() {
            return fail(
                deps,
                def,
                &caller,
                findings,
                started,
                Core {
                    model: Some(routed.clone()),
                    step,
                    // NOT AN ERROR ABOUT THE MODEL. The floor declined to ask,
                    // so the sweep records an absence rather than a failure.
                    refused: true,
                    error: Some(format!(
                        "\"{}\" cannot run harness \"{}\": it is known not to support {}. {}",
                        routed,
                        def.id,
                        unreachable.join(", "),
                        def.floor.note
                    )),
                    ..empty
                },
            )
            .await;
        }
    }

    // 3 ─ Widening. Step 2 asks "is this model KNOWN to be unable"; this asks
    // "is it KNOWN to be able". Unknown answers neither, and the safe
    // direction is the same both times — keep running, on the deterministic
    // surface. ONLY A PROBE WIDENS, because widening is the direction that
    // HANDS A MODEL MORE AUTHORITY, and the evidence for that has to be
    // Talaria's own measurement, not a vendor's model card.
    let mut widened = false;
    if let Some(widen) = &def.widen
        && !keys.is_empty()
    {
        let facts = {
            let mut all: Vec<HashMap<String, CapabilityFact>> = Vec::new();
            for key in &keys {
                all.push((deps.capabilities)(key.clone()).await);
            }
            all
        };
        widened = widen.requires.iter().all(|cap| {
            facts.iter().all(|f| {
                f.get(*cap)
                    .map(|fact| fact.value && fact.source == "probe")
                    .unwrap_or(false)
            })
        });
    }

    // 4 ─ Render. The only harness-authored code that runs before the call.
    // ONE `RenderContext` for the run: `render` builds the prompt from it and
    // `verify` checks the answer against it, so the surface a harness offered
    // and the surface it grades against can never be two different objects.
    let render_context = RenderContext {
        widened,
        model: routed.clone(),
    };
    let base = match (def.render)(input, &render_context) {
        Ok(messages) if !messages.is_empty() => messages,
        _ => {
            return fail(
                deps,
                def,
                &caller,
                findings,
                started,
                Core {
                    model: Some(routed.clone()),
                    step,
                    widened,
                    error: Some(format!("harness \"{}\" rendered no messages", def.id)),
                    ..empty
                },
            )
            .await;
        }
    };

    // THE TURN'S GROUNDING MATERIAL — everything this run put in front of the
    // model, for the "was this span the model's at all" question. `base`, NOT
    // `sent`: `sent` accumulates the model's own rejected reply, so grounding
    // attempt two against attempt one would let a model launder an invented
    // card by emitting it twice. Computed ONCE — the repair gate, the guard
    // pass and the redaction have to agree with each other anyway.
    let ground_text = guard::grounding_text_of(&wire_messages(&base));

    // 5/6 ─ Call, parse, repair.
    let structured = def.output.is_json();
    // A STREAMED RUN NEVER REPAIRS (the first answer already reached the
    // screen), and neither does a TEXT harness: the one repair wording lives
    // in json.rs and ends "send the corrected JSON value only", which is a
    // nonsense instruction to a titler.
    let max_repairs = match (&def.output, streaming.is_some()) {
        (Output::Json { repair, .. }, false) => repair.unwrap_or(1),
        _ => 0,
    };
    // ONE structured-output strategy on both transports: send the HARNESS'S
    // OWN SCHEMA at the protocol level AND anchor the instruction in the
    // prompt. Still gated on `missing`, and the gate now means the LEARNED
    // case only — a model MEASURED unable never reaches this line (the floor
    // refused it above).
    let wire: Option<WireSchema> = match &def.output {
        Output::Json { schema, .. } => wire_schema_of(def.id, schema),
        Output::Text { .. } => None,
    };
    let mut json_mode = structured && !missing.contains(&"json");
    let mut sent = if structured {
        anchor_json(&base, &json_anchor_for(wire.as_ref()))
    } else {
        base.clone()
    };
    let mut repairs: u32 = 0;
    let mut value: Option<Value> = None;
    let mut schema_valid = false;
    let mut failure: Option<String>;
    let mut text = String::new();
    // Set from the reply the contract is about to be applied to, so a repair
    // turn that came back empty makes it false again — the result IS about
    // that empty reply. The transport-error path leaves it false whatever
    // partial arrived: a partial is not an answer.
    let mut answered: bool;
    let mut kind: TransportKind;
    let mut tool_names: Vec<String>;

    // The ledger row this turn belongs to, resolved ONCE. `agent_model` is
    // the base persona and `tier` the alias name, because that is the pair
    // `recordUsage` prices from.
    let ledger = LedgerAttribution {
        agent_model: model.clone(),
        source: ctx
            .ledger
            .as_ref()
            .and_then(|l| l.source)
            .unwrap_or(LedgerSource::Chat),
        ref_id: ctx.ledger.as_ref().and_then(|l| l.ref_id.clone()),
        task_id: ctx.ledger.as_ref().and_then(|l| l.task_id.clone()),
        tier: ctx.tier.clone(),
    };

    loop {
        let request = TransportRequest {
            model: routed.clone(),
            messages: sent.clone(),
            temperature: def.temperature,
            json_mode,
            json_schema: wire.clone(),
            tools: def.tools,
            tool_defs: def.tool_defs.clone(),
            ledger: Some(ledger.clone()),
            effort: effort.clone(),
            hold_ms: def.hold_ms,
            caller: caller.clone(),
        };
        // The streamed deltas are accumulated as well as handed on, so a
        // transport that pumps into a browser and never assembles the text
        // itself may resolve with an empty `text` and the guard pass still
        // sees the whole reply. Rebuilt per attempt, outside the reply's
        // error path, so a transport that dies mid-stream leaves the partial
        // for `raw` — the diagnosis an operator most wants to interrogate.
        let reply = match &streaming {
            Some(opts) => {
                let acc = Arc::new(std::sync::Mutex::new(String::new()));
                let sink = acc.clone();
                let on_delta = opts.on_delta.clone();
                let emit: DeltaFn = Arc::new(move |delta: &str| {
                    if let Ok(mut s) = sink.lock() {
                        s.push_str(delta);
                    }
                    if let Some(f) = &on_delta {
                        f(delta);
                    }
                });
                match (opts.stream)(request, emit).await {
                    Ok(reply) => (reply, acc.lock().map(|s| s.clone()).unwrap_or_default()),
                    Err(err) => {
                        let streamed = acc.lock().map(|s| s.clone()).unwrap_or_default();
                        return transport_failure(
                            deps, def, &caller, findings, started, empty, &routed, step, widened,
                            repairs, &text, &streamed, &err,
                        )
                        .await;
                    }
                }
            }
            None => match (deps.transport)(request).await {
                Ok(reply) => (reply, String::new()),
                Err(err) => {
                    return transport_failure(
                        deps, def, &caller, findings, started, empty, &routed, step, widened,
                        repairs, &text, "", &err,
                    )
                    .await;
                }
            },
        };
        let (reply, streamed) = reply;
        text = if reply.text.is_empty() {
            streamed
        } else {
            reply.text.clone()
        };
        answered = !text.trim().is_empty();
        kind = reply.kind;
        tool_names = reply.tool_names.clone();
        if reply.contract_dropped && json_mode {
            // The upstream refused JSON mode and the gateway dropped the
            // parameter rather than failing the call. We stop asking for the
            // rest of the run — the prompt anchor is already in every
            // structured request, so the repair turn below is a plain
            // text-mode ask, the deliberate fallback path.
            json_mode = false;
        }

        // Parse, then `verify` against THE ORIGINAL INPUT - never the
        // repaired message list, which by the second turn contains the
        // model's own rejected answer.
        let applied = apply_output(def, &text, input, &render_context);
        let applied_error = match applied {
            Applied::Ok(v) => {
                value = Some(v);
                schema_valid = true;
                failure = None;
                break;
            }
            Applied::Err(e) => e,
        };
        failure = Some(applied_error.clone());
        if repairs >= max_repairs {
            break;
        }

        // THE REPAIR TURN IS THE ONE PLACE THIS RUNNER PUTS MODEL OUTPUT BACK
        // INTO A MODEL'S CONTEXT, so it goes through the gate-safe rules
        // first. A flagged reply is not repaired; it fails, which is the
        // correct outcome for a reply we would refuse to hand back anyway.
        // Note what is NOT interpolated into the repair prompt: the finding,
        // its message, or above all its `snippet` — a verbatim excerpt of the
        // flagged content.
        let gate = (deps.guard_text)(text.clone(), Some(ground_text.clone())).await;
        if !gate.is_empty() {
            merge_findings(&mut findings, gate);
            failure = Some(format!(
                "{applied_error} (not repaired: the reply was flagged by the guard)"
            ));
            break;
        }

        repairs += 1;
        sent.push(Message::assistant(text.clone()));
        sent.push(Message::user(repair_prompt(&applied_error)));
    }

    // 7 ─ Guard, with an HONEST `Available` for the transport that actually
    // ran. gateway: we hold the history, so results and error info are
    // genuinely available — unless the harness offered tool definitions and
    // the model called one (names, no results: the fleet's situation, the
    // fleet's answer). fleet: tool NAMES only, so the rules that need
    // material are SKIPPED rather than guessed. ground: the harness handed
    // over the turn's REAL tool record from its own input, which OVERRIDES
    // both branches above — the only way `ungrounded_ref` can fire from a
    // harness at all.
    let config = (deps.guard_config)().await;
    let material = grounding_for(def, input);
    if let Some(config) = config
        .as_ref()
        .filter(|c| c.mode != GuardMode::Off)
        .filter(|_| !text.is_empty())
    {
        let overflowed = material
            .as_ref()
            .map(|m| m.results.encode_utf16().count() > GROUND_RESULTS_CAP)
            .unwrap_or(false);
        // NAMES WITHOUT RESULTS. A tool CALL is not a tool RESULT: nothing
        // executed it, so feeding the names in as backing tools with empty
        // results would make `ungrounded_ref` fire on every id in the reply.
        // `overflowed: true` is the guard's own "I have the names, not the
        // material" state, and it fails those rules OPEN.
        let names_only = kind == TransportKind::Fleet || !tool_names.is_empty();
        let available = if let Some(m) = &material {
            Available {
                results: true,
                error_info: m.errored.is_some(),
            }
        } else if names_only {
            Available {
                results: false,
                error_info: false,
            }
        } else {
            guard::FULL
        };
        let derived = if names_only {
            let calls: Vec<Value> = tool_names
                .iter()
                .map(|name| json!({ "function": { "name": name } }))
                .collect();
            let mut record = guard::extract_tool_record(&[json!({
                "role": "assistant",
                "tool_calls": calls,
            })]);
            record.overflowed = true;
            record
        } else {
            guard::extract_tool_record(&wire_messages(&sent))
        };
        let tool_record = match &material {
            Some(m) => ToolRecord {
                backing_tools: m.tools.clone(),
                results_text: if overflowed {
                    String::new()
                } else {
                    m.results.clone()
                },
                any_error: m.errored == Some(true),
                overflowed,
            },
            None => derived,
        };
        let grounding = guard::Grounding::new(&ground_text);
        let narrowed =
            guard::narrow_guard_config(config, def.guard.as_ref().and_then(|g| g.rules.as_deref()));
        let gctx = guard::GuardContext {
            answer: &text,
            tool_record: &tool_record,
            input_text: &ground_text,
            policed_hosts: &config.policed_hosts,
            grounding: &grounding,
        };
        let hits = guard::run_guardrails(&gctx, &narrowed, &available);
        merge_findings(&mut findings, hits);
    }
    if !findings.is_empty()
        && let Some(config) = &config
    {
        (deps.record_findings)(
            findings.clone(),
            FindingMeta {
                caller: caller.clone(),
                model: routed.clone(),
                endpoint: if kind == TransportKind::Fleet {
                    Some("fleet".to_string())
                } else {
                    endpoint.clone()
                },
                mode: config.mode,
            },
        )
        .await;
    }

    // Redaction happens on the RAW REPLY and the contract is then re-applied,
    // so a redacted value is a value that still satisfies the schema — never a
    // half-scrubbed object. THE WHOLE CONTRACT, `verify` included: a value can
    // survive being cut in half and still parse. GROUNDED THE SAME WAY THE
    // FINDING WAS: the finding declines to blame the model for an identifier
    // out of its own prompt, and the redactor must not rewrite it anyway.
    // Credentials come out regardless of grounding; `redact_secrets` and
    // `secret_leak` own that asymmetry between them.
    if def.guard.as_ref().map(|g| g.redact).unwrap_or(false)
        && value.is_some()
        && guard::needs_redaction(&findings)
    {
        let (safe_text, redacted) =
            guard::redact_secrets(&text, Some(&guard::Grounding::new(&ground_text)));
        if redacted {
            match apply_output(def, &safe_text, input, &render_context) {
                Applied::Ok(v) => value = Some(v),
                Applied::Err(_) => {
                    value = None;
                    schema_valid = false;
                    failure = Some(
                        "the output contained a credential and the redacted form no longer satisfies the contract"
                            .to_string(),
                    );
                }
            }
        }
    }

    // 8 ─ The declared failure policy.
    let mut error = if value.is_none() {
        failure
            .clone()
            .or_else(|| Some("the harness produced no value".to_string()))
    } else {
        None
    };
    let mut escalate = false;
    if value.is_none() {
        match &def.on_failure {
            OnFailure::Throw => {
                // Through the same `fail` as every pre-call exit, so the row
                // is written before the throw either way.
                return fail(
                    deps,
                    def,
                    &caller,
                    findings,
                    started,
                    Core {
                        value: None,
                        model: Some(routed.clone()),
                        step,
                        widened,
                        repairs,
                        schema_valid: false,
                        escalate: false,
                        answered,
                        refused: false,
                        raw: raw_of(&text),
                        error,
                    },
                )
                .await;
            }
            OnFailure::Fallback(fallback) => {
                // schema_valid stays FALSE. The fallback is the caller's
                // declared safe answer, not evidence that the model produced
                // one, and conflating the two would quietly inflate every
                // contract rate in the fitness matrix.
                value = Some(match fallback {
                    Fallback::Text(s) => Value::String(s.clone()),
                    Fallback::Json(v) => v.clone(),
                });
            }
            OnFailure::Escalate => {
                escalate = true;
                error = Some(format!(
                    "{} - escalate to a human",
                    error.take().unwrap_or_default()
                ));
            }
            OnFailure::Null => {}
        }
    }

    // 9 ─ Meter. The transports own the token ledger; all that is left here
    // is the harness_runs row.
    Ok(finish(
        deps,
        def.id,
        &caller,
        findings,
        started,
        Core {
            value,
            model: Some(routed),
            step,
            widened,
            repairs,
            schema_valid,
            escalate,
            answered,
            refused: false,
            raw: raw_of(&text),
            error,
        },
    )
    .await)
}

/// The transport-death exit, shared by the blocking and streaming arms so the
/// sentence and the row stay identical between them. Whatever arrived before
/// the throw is the `raw` — a transport that died mid-stream still leaves the
/// partial reply, and that partial IS the diagnosis. `answered` stays false
/// regardless, from the empty core: a partial is not an answer.
#[allow(clippy::too_many_arguments)]
async fn transport_failure(
    deps: &HarnessDeps,
    def: &HarnessDefinition,
    caller: &str,
    findings: Vec<Finding>,
    started: i64,
    empty: Core,
    routed: &str,
    step: Option<ModelChainStep>,
    widened: bool,
    repairs: u32,
    text: &str,
    streamed: &str,
    err: &str,
) -> Result<HarnessResult, HarnessError> {
    let partial = if text.is_empty() { streamed } else { text };
    fail(
        deps,
        def,
        caller,
        findings,
        started,
        Core {
            model: Some(routed.to_string()),
            step,
            widened,
            repairs,
            raw: raw_of(partial),
            error: Some(format!(
                "harness \"{}\" could not reach \"{}\": {}",
                def.id, routed, err
            )),
            ..empty
        },
    )
    .await
}

// ── Tests ────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::define::{CleanFn, GroundFn, GuardDecl, RoleFloor, Widen, define_harness};
    use crate::harness::schema::{Field, Schema};
    use crate::harness::transport::ToolPolicy;
    use crate::persona::PersonaRow;
    use crate::state::AppState;
    use std::sync::Mutex;

    // The runner is exercised end to end against RECORDED REPLIES. That is not
    // a convenience: the whole point of the harness layer is that a 14B model
    // returns something a frontier model would not, and the only way to hold
    // that behavior still is to write the bad reply down and assert what the
    // runner does with it. Every edge is a closure on `HarnessDeps`, so nothing
    // here touches a database, a gateway or a fleet.
    //
    // Three things are deliberately REAL rather than faked, because faking them
    // would turn the assertion into a restatement of the fake:
    //   - the guard pass (`run_guardrails` runs the actual rule registry, via
    //     `gate_safe` for the repair gate)
    //   - the parser (`parse_json`, against the def's own `Schema`)
    //   - the persona resolver (`persona_index`, over recorded agent config
    //     rows — tier resolution is the whole question, so a fake that returned
    //     keys would be testing nothing)

    // ── The defs ─────────────────────────────────────────────────────────────

    fn verdict_schema() -> Schema {
        Schema::Object(vec![
            Field::required(
                "verdict",
                Schema::Enum(vec!["pass".into(), "revise".into()]),
            ),
            Field::required("summary", Schema::string()),
        ])
    }

    fn spec(pin: &'static str) -> ModelSpec<'static> {
        ModelSpec {
            pin: Some(pin),
            role: None,
            chain: None,
            user_id: None,
        }
    }

    /// A judge-shaped harness: a real output contract, a real floor, and a
    /// capability it refuses to work without.
    fn judge() -> HarnessDefinition {
        let mut d = define_harness(HarnessDefinition::new(
            "judge",
            "Judge",
            "Reviews an agent-reported outcome against the ticket.",
            spec("judge"),
            Arc::new(|input: &Value, ctx: &RenderContext| {
                let ticket = input["ticket"].as_str().unwrap_or_default();
                Ok(vec![
                    Message::system(if ctx.widened {
                        "You may also cite the diff."
                    } else {
                        "Judge the reported outcome."
                    }),
                    Message::user(ticket),
                ])
            }),
            Output::Json {
                schema: verdict_schema(),
                repair: None,
                verify: None,
            },
            OnFailure::Null,
        ));
        d.requires = vec!["json", "json-strict"];
        d.floor = RoleFloor::refuses(
            vec!["json"],
            "A judge that cannot return a structured verdict escalates everything, which is a notification storm rather than a review.",
        );
        d.temperature = Some(0.0);
        d
    }

    /// A titler-shaped harness: text out, almost nothing declared, and it must
    /// run on whatever the self-host has.
    fn titler() -> HarnessDefinition {
        let mut d = define_harness(HarnessDefinition::new(
            "titler",
            "Titler",
            "Names a conversation once its first exchange lands.",
            spec("titler"),
            Arc::new(|input: &Value, _ctx: &RenderContext| {
                Ok(vec![Message::user(
                    input["transcript"].as_str().unwrap_or_default(),
                )])
            }),
            Output::Text {
                // First non-empty line, quotes and fences stripped — the whole
                // of the TS `clean`, as one closure.
                clean: Some(Arc::new(|raw: &str| {
                    let line = raw.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
                    let stripped =
                        line.trim_matches(|c: char| c == '"' || c == '\'' || c.is_whitespace());
                    if stripped.is_empty() {
                        Ok(None)
                    } else {
                        Ok(Some(Value::String(stripped.to_string())))
                    }
                }) as CleanFn),
                verify: None,
            },
            OnFailure::Null,
        ));
        d.floor = RoleFloor::runs_anyway("Runs on anything — a mediocre title beats no title.");
        d
    }

    /// THE BLURB-WRITER BUG, as a harness. A `record<string, string>` cannot
    /// constrain the KEYS, so a model that tidied `qwen3-14b` into `Qwen3 14B`
    /// passed the schema, wrote zero usable blurbs and reported a 100%
    /// contract rate — then came back around on the identical batch every ten
    /// minutes forever. `verify` is the half that catches it.
    fn blurber() -> HarnessDefinition {
        let mut d = define_harness(HarnessDefinition::new(
            "blurb-writer",
            "Blurb writer",
            "Writes the one-line description under each model in the picker.",
            spec("blurb-writer"),
            Arc::new(|input: &Value, _ctx: &RenderContext| {
                let ids: Vec<String> = input["ids"]
                    .as_array()
                    .map(|a| {
                        a.iter()
                            .filter_map(|v| v.as_str().map(String::from))
                            .collect()
                    })
                    .unwrap_or_default();
                Ok(vec![Message::user(format!("Describe: {}", ids.join(", ")))])
            }),
            Output::Json {
                schema: Schema::Record(Box::new(Schema::string())),
                repair: None,
                verify: Some(Arc::new(
                    |value: &Value, input: &Value, _ctx: &RenderContext| {
                        let asked: Vec<&str> = input["ids"]
                            .as_array()
                            .map(|a| a.iter().filter_map(|v| v.as_str()).collect())
                            .unwrap_or_default();
                        let invented: Vec<String> = value
                            .as_object()
                            .map(|o| {
                                o.keys()
                                    .filter(|k| !asked.contains(&k.as_str()))
                                    .map(|k| format!("\"{k}\""))
                                    .collect()
                            })
                            .unwrap_or_default();
                        if invented.is_empty() {
                            return Ok(None);
                        }
                        // Written as an INSTRUCTION TO THE MODEL, because that is
                        // where it goes: the repair turn feeds it straight back.
                        Ok(Some(format!(
                            "the keys must be the model ids exactly as they were given - {} is not one of them. The ids are: {}.",
                            invented.join(", "),
                            asked.join(", ")
                        )))
                    },
                )),
            },
            OnFailure::Null,
        ));
        d.requires = vec!["json"];
        d.floor =
            RoleFloor::runs_anyway("A blurb is a nicety; no blurb is better than a wrong one.");
        d
    }

    // ── The recorded world (recorded.rs, the one copy) ─────────────────────
    //
    // The scaffolding this corpus was ported against became `recorded.rs` —
    // the recorded-transcript harness — because 22 def test files were about
    // to need the same fake world and a second copy of a fake is worse than
    // a second copy of real code. These aliases keep the corpus's vocabulary;
    // the thin wrappers below keep its ergonomics.

    use crate::harness::recorded::{
        RecordedModel as ModelAnswer, RecordedReply as Reply, RecordedRun as Recorder,
        RecordedWorld as World, facts, probe, recorded_run, replies, sourced,
    };

    fn world(w: World) -> Recorder {
        recorded_run(w)
    }

    fn ctx(r: &Recorder) -> RunContext {
        r.ctx("test:harness")
    }

    async fn run(
        def: &HarnessDefinition,
        input: &Value,
        r: &Recorder,
    ) -> Result<HarnessResult, HarnessError> {
        execute(&r.deps(), def, input, ctx(r), None).await
    }

    fn req_at(r: &Recorder, i: usize) -> TransportRequest {
        r.requests.lock().expect("requests")[i].clone()
    }

    fn n_requests(r: &Recorder) -> usize {
        r.requests.lock().expect("requests").len()
    }

    fn run_at(r: &Recorder, i: usize) -> HarnessRunRow {
        r.runs.lock().expect("runs")[i].clone()
    }

    fn n_runs(r: &Recorder) -> usize {
        r.runs.lock().expect("runs").len()
    }

    fn recorded(r: &Recorder) -> Vec<Finding> {
        r.findings.lock().expect("findings").clone()
    }

    fn checks(res: &HarnessResult) -> Vec<&str> {
        res.findings.iter().map(|f| f.check).collect()
    }

    fn ticket(t: &str) -> Value {
        json!({ "ticket": t })
    }

    // ── The happy path ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn parses_the_value_records_the_step_and_writes_one_row() {
        let r = world(World::default());
        let res = run(&judge(), &ticket("ship the thing"), &r).await.unwrap();

        assert_eq!(
            res.value,
            Some(json!({ "verdict": "pass", "summary": "looks right" }))
        );
        assert!(res.schema_valid);
        assert_eq!(res.repairs, 0);
        assert_eq!(res.model, Some("pl-main".into()));
        // Which fallback actually carried the harness is part of the answer,
        // not a detail — a subsystem limping along on 'first-routable' for a
        // month is invisible without it.
        assert_eq!(res.step, Some("pin"));
        assert_eq!(res.error, None);
        assert_eq!(n_runs(&r), 1);
        let row = run_at(&r, 0);
        assert_eq!(row.harness, "judge");
        assert_eq!(row.model, Some("pl-main".into()));
        assert_eq!(row.step, Some("pin"));
        assert!(!row.widened);
        assert_eq!(row.repairs, 0);
        assert!(row.schema_valid);
        assert_eq!(row.latency_ms, 7);
        assert_eq!(row.findings, 0);
        assert_eq!(row.caller, "test:harness");
        assert_eq!(row.error, None);
    }

    #[tokio::test]
    async fn asks_for_json_at_the_protocol_level_and_anchors_it() {
        // One strategy on every transport. The anchor is what survives a
        // gateway that drops response_format.
        let r = world(World::default());
        run(&judge(), &ticket("ship the thing"), &r).await.unwrap();
        let req = req_at(&r, 0);
        assert!(req.json_mode);
        assert_eq!(req.temperature, Some(0.0));
        let last = req.messages.last().expect("messages");
        assert!(last.content.contains("exactly one JSON value"));
    }

    #[tokio::test]
    async fn no_protocol_json_when_the_model_is_known_to_refuse_it() {
        let r = world(World {
            facts: facts(&[("spark", "json", probe(false))]),
            ..Default::default()
        });
        // The floor would refuse the judge here, so use a harness that only
        // DEGRADES below its floor — the degraded path still has to work.
        let mut soft = judge();
        soft.floor.refuse_below = false;
        let res = run(&soft, &ticket("x"), &r).await.unwrap();
        let req = req_at(&r, 0);
        assert!(!req.json_mode);
        assert!(
            req.messages
                .last()
                .expect("messages")
                .content
                .contains("exactly one JSON value")
        );
        assert_eq!(
            res.value,
            Some(json!({ "verdict": "pass", "summary": "looks right" }))
        );
    }

    #[tokio::test]
    async fn cleans_a_text_harness_rather_than_schema_parsing_it() {
        let r = world(World {
            replies: replies(&["\"Migrating the ledger to Postgres\"\n\nHope that helps!"]),
            ..Default::default()
        });
        let res = run(&titler(), &json!({ "transcript": "a chat" }), &r)
            .await
            .unwrap();
        assert_eq!(
            res.value,
            Some(Value::String("Migrating the ledger to Postgres".into()))
        );
        assert!(!req_at(&r, 0).json_mode);
    }

    // ── Repair ───────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn re_asks_once_on_a_malformed_reply_and_reports_it() {
        let r = world(World {
            // The shape a 14B model actually emits: a preamble, then the
            // object, then more prose — and a field the schema rejects.
            replies: replies(&[
                "Sure! Here is my verdict:\n\n{\"verdict\": \"maybe\", \"summary\": \"unclear\"}\n\nLet me know if you want more.",
                "{\"verdict\":\"revise\",\"summary\":\"the tests are missing\"}",
            ]),
            ..Default::default()
        });
        let res = run(&judge(), &ticket("ship the thing"), &r).await.unwrap();

        assert_eq!(
            res.value,
            Some(json!({ "verdict": "revise", "summary": "the tests are missing" }))
        );
        assert!(res.schema_valid);
        assert_eq!(res.repairs, 1);
        assert_eq!(run_at(&r, 0).repairs, 1);
    }

    #[tokio::test]
    async fn hands_the_model_its_own_reply_plus_the_parser_error() {
        let r = world(World {
            replies: replies(&[
                "{\"verdict\": \"maybe\", \"summary\": \"unclear\"}",
                "{\"verdict\":\"pass\",\"summary\":\"fine\"}",
            ]),
            ..Default::default()
        });
        run(&judge(), &ticket("ship the thing"), &r).await.unwrap();

        let first = req_at(&r, 0);
        let repair = req_at(&r, 1);
        assert_eq!(repair.messages[0].content, first.messages[0].content);
        assert_eq!(repair.messages[1].content, first.messages[1].content);
        let rejected = &repair.messages[repair.messages.len() - 2];
        assert_eq!(rejected.role.as_str(), "assistant");
        assert_eq!(
            rejected.content,
            "{\"verdict\": \"maybe\", \"summary\": \"unclear\"}"
        );
        // The repair prompt names the FIELD, not a stack trace — that is the
        // difference between a small model fixing it and rewriting it.
        assert!(
            repair
                .messages
                .last()
                .expect("messages")
                .content
                .contains("field 'verdict'")
        );
    }

    #[tokio::test]
    async fn gives_up_after_the_declared_number_of_repairs() {
        let r = world(World {
            replies: replies(&[
                "not json",
                "still not json",
                "{\"verdict\":\"pass\",\"summary\":\"too late\"}",
            ]),
            ..Default::default()
        });
        let res = run(&judge(), &ticket("ship the thing"), &r).await.unwrap();

        assert_eq!(n_requests(&r), 2); // one call, one repair — never the third
        assert_eq!(res.value, None);
        assert!(!res.schema_valid);
        assert_eq!(res.repairs, 1);
        assert!(
            res.error
                .as_deref()
                .unwrap()
                .contains("no JSON object or array was found")
        );
    }

    #[tokio::test]
    async fn honors_a_harness_that_asks_for_more_than_one_round() {
        let r = world(World {
            replies: replies(&[
                "nope",
                "still nope",
                "{\"verdict\":\"pass\",\"summary\":\"third time\"}",
            ]),
            ..Default::default()
        });
        let mut patient = judge();
        patient.output = Output::Json {
            schema: verdict_schema(),
            repair: Some(2),
            verify: None,
        };
        let res = run(&patient, &ticket("x"), &r).await.unwrap();
        assert_eq!(res.repairs, 2);
        assert_eq!(
            res.value,
            Some(json!({ "verdict": "pass", "summary": "third time" }))
        );
    }

    #[tokio::test]
    async fn refuses_to_repair_a_reply_the_guard_flagged() {
        // The repair turn is the one place this runner puts model output back
        // into a model's context. The cardinal invariant — flagged content
        // never re-enters a model's context — has to hold here or it holds
        // nowhere.
        let r = world(World {
            replies: replies(&[
                "my key is AKIAIOSFODNN7EXAMPLE and here is the verdict",
                "{\"verdict\":\"pass\",\"summary\":\"fine\"}",
            ]),
            ..Default::default()
        });
        let res = run(&judge(), &ticket("x"), &r).await.unwrap();

        assert_eq!(n_requests(&r), 1);
        assert_eq!(res.value, None);
        assert!(res.error.as_deref().unwrap().contains("not repaired"));
        assert!(res.findings.iter().any(|f| f.check == "secret_leak"));
    }

    #[tokio::test]
    async fn counts_a_refused_repair_as_one_leak_not_two() {
        // The repair gate and the final guard pass scan the same reply.
        // Recording the leak twice would make `guard_findings` — the live
        // per-model confabulation rate — double precisely when the repair path
        // protects us, so the safety feature would read as a safety regression.
        let r = world(World {
            replies: replies(&["my key is AKIAIOSFODNN7EXAMPLE and here is the verdict"]),
            ..Default::default()
        });
        let res = run(&judge(), &ticket("x"), &r).await.unwrap();

        assert_eq!(
            res.findings
                .iter()
                .filter(|f| f.check == "secret_leak")
                .count(),
            1
        );
        assert_eq!(
            recorded(&r)
                .iter()
                .filter(|f| f.check == "secret_leak")
                .count(),
            1
        );
        assert_eq!(run_at(&r, 0).findings, 1);
    }

    #[tokio::test]
    async fn never_puts_a_guard_finding_into_the_repair_prompt() {
        // The invariant, stated as an assertion: the repair turn carries the
        // PARSER error. A finding's `snippet` is a verbatim excerpt of the
        // flagged content, so interpolating one would feed the credential back
        // to the model while ostensibly enforcing the rule against it.
        let r = world(World {
            replies: replies(&[
                "{\"verdict\":\"maybe\"}",
                "{\"verdict\":\"pass\",\"summary\":\"ok\"}",
            ]),
            ..Default::default()
        });
        let res = run(&judge(), &ticket("x"), &r).await.unwrap();

        assert_eq!(res.repairs, 1);
        let turn = req_at(&r, 1)
            .messages
            .last()
            .expect("messages")
            .content
            .clone();
        assert!(turn.contains("field 'verdict'"));
        let lower = turn.to_lowercase();
        for word in ["guard", "flagged", "redacted", "leak"] {
            assert!(
                !lower.contains(word),
                "repair prompt must not say {word}: {turn}"
            );
        }
    }

    // ── Failure policy ───────────────────────────────────────────────────────

    fn bad() -> World {
        World {
            replies: replies(&["nope", "nope"]),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn null_returns_nothing_and_says_why() {
        let res = run(&judge(), &ticket("x"), &world(bad())).await.unwrap();
        assert_eq!(res.value, None);
        assert!(res.error.is_some());
    }

    #[tokio::test]
    async fn a_declared_fallback_never_counts_as_a_valid_contract() {
        // schemaValid staying false is the point: counting a fallback as a
        // pass would quietly inflate every contract rate in the matrix.
        let r = world(bad());
        let mut with_fallback = judge();
        with_fallback.on_failure = OnFailure::Fallback(Fallback::Json(json!({
            "verdict": "revise",
            "summary": "the judge could not read the outcome"
        })));
        let res = run(&with_fallback, &ticket("x"), &r).await.unwrap();
        assert_eq!(
            res.value,
            Some(json!({ "verdict": "revise", "summary": "the judge could not read the outcome" }))
        );
        assert!(!res.schema_valid);
        assert!(!run_at(&r, 0).schema_valid);
    }

    #[tokio::test]
    async fn escalate_marks_the_result_for_the_caller_to_raise() {
        let mut esc = judge();
        esc.on_failure = OnFailure::Escalate;
        let res = run(&esc, &ticket("x"), &world(bad())).await.unwrap();
        assert_eq!(res.value, None);
        // A FLAG, not a phrase. Only the caller knows who to tell, and a
        // caller that has to string-match the error to find out stops
        // escalating the day somebody rewords it.
        assert!(res.escalate);
        assert!(
            res.error
                .as_deref()
                .unwrap()
                .contains("escalate to a human")
        );
    }

    #[tokio::test]
    async fn never_sets_escalate_for_the_other_three_policies() {
        assert!(
            !run(&judge(), &ticket("x"), &world(bad()))
                .await
                .unwrap()
                .escalate
        );
        let mut fb = judge();
        fb.on_failure = OnFailure::Fallback(Fallback::Json(json!({
            "verdict": "revise", "summary": "n/a"
        })));
        assert!(
            !run(&fb, &ticket("x"), &world(bad()))
                .await
                .unwrap()
                .escalate
        );
        // And a run that SUCCEEDED is never an escalation, whatever it
        // declared.
        let mut esc = judge();
        esc.on_failure = OnFailure::Escalate;
        assert!(
            !run(&esc, &ticket("x"), &world(World::default()))
                .await
                .unwrap()
                .escalate
        );
    }

    #[tokio::test]
    async fn throw_still_writes_the_run_row_before_it_throws() {
        let r = world(bad());
        let mut throwing = judge();
        throwing.on_failure = OnFailure::Throw;
        let err = run(&throwing, &ticket("x"), &r).await.unwrap_err();
        assert!(err.0.contains("harness \"judge\" failed"));
        assert_eq!(n_runs(&r), 1);
        assert!(!run_at(&r, 0).schema_valid);
    }

    // ── 'throw' means EVERY failure to produce a value ───────────────────────
    //
    // It used to mean a contract failure and nothing else. The two callers
    // that did not restate the policy by hand both shipped a bug — research
    // synthesis saved an empty report and marked the run `done` after a 502,
    // and the channel planner answered "nothing to plan yet" on a channel full
    // of work because its agent container was restarting.

    fn throwing() -> HarnessDefinition {
        let mut d = judge();
        d.on_failure = OnFailure::Throw;
        d
    }

    #[tokio::test]
    async fn throws_on_a_transport_error() {
        let r = world(World {
            transport_error: Some("gateway completion 503".into()),
            ..Default::default()
        });
        let err = run(&throwing(), &ticket("x"), &r).await.unwrap_err();
        assert!(err.0.contains("503"));
        // The row lands first, every time. A throwing harness is precisely
        // the one an operator has to be able to find in the fitness data.
        assert_eq!(n_runs(&r), 1);
        assert!(run_at(&r, 0).error.as_deref().unwrap().contains("503"));
    }

    #[tokio::test]
    async fn throws_when_nothing_in_the_chain_routes() {
        let r = world(World {
            model: ModelAnswer::NoModel,
            ..Default::default()
        });
        let err = run(&throwing(), &ticket("x"), &r).await.unwrap_err();
        assert!(err.0.contains("no model available"));
        assert_eq!(n_runs(&r), 1);
    }

    #[tokio::test]
    async fn throws_on_a_capability_refusal_naming_the_capability() {
        let r = world(World {
            facts: facts(&[("spark", "json", sourced(false, "probe"))]),
            ..Default::default()
        });
        let err = run(&throwing(), &ticket("x"), &r).await.unwrap_err();
        assert!(err.0.contains("known not to support json"));
        assert_eq!(n_requests(&r), 0);
    }

    #[tokio::test]
    async fn throws_when_the_render_produces_nothing() {
        let r = world(World::default());
        let mut renders_nothing = throwing();
        renders_nothing.render = Arc::new(|_input, _ctx| Ok(Vec::new()));
        let err = run(&renders_nothing, &ticket("x"), &r).await.unwrap_err();
        assert!(err.0.contains("rendered no messages"));
    }

    #[tokio::test]
    async fn names_no_model_in_the_sentence_when_there_was_none() {
        let r = world(World {
            model: ModelAnswer::NoModel,
            ..Default::default()
        });
        let err = run(&throwing(), &ticket("x"), &r).await.unwrap_err();
        assert!(err.0.starts_with("harness \"judge\" failed: "));
    }

    #[tokio::test]
    async fn leaves_the_other_three_policies_contract_scoped() {
        // Widening them would break both callers that use them: a fallback
        // would hand outreach its "nothing to surface" token during a gateway
        // outage, so a dead provider would read as a normal quiet pass on
        // every sweep. `answered` is how a caller asks for either on purpose.
        let r = world(World {
            model: ModelAnswer::NoModel,
            ..Default::default()
        });
        let mut fb = judge();
        fb.on_failure = OnFailure::Fallback(Fallback::Json(json!({
            "verdict": "revise", "summary": "n/a"
        })));
        let fallen = run(&fb, &ticket("x"), &r).await.unwrap();
        assert_eq!(fallen.value, None);
        assert!(!fallen.answered);

        let mut esc = judge();
        esc.on_failure = OnFailure::Escalate;
        let none = world(World {
            model: ModelAnswer::NoModel,
            ..Default::default()
        });
        assert!(!run(&esc, &ticket("x"), &none).await.unwrap().escalate);
    }

    // ── `answered`: did the model actually answer ────────────────────────────
    //
    // `raw !== null` had become the de-facto test in three adapters, and `raw`
    // is a bounded drill-down field that answers a different question — it
    // survives a stream that died three tokens in, so a transport failure read
    // to all three as a model that answered badly.

    #[tokio::test]
    async fn answered_is_false_when_nothing_was_reached() {
        assert!(
            !run(
                &judge(),
                &ticket("x"),
                &world(World {
                    model: ModelAnswer::NoModel,
                    ..Default::default()
                })
            )
            .await
            .unwrap()
            .answered
        );
        assert!(
            !run(
                &judge(),
                &ticket("x"),
                &world(World {
                    facts: facts(&[("spark", "json", probe(false))]),
                    ..Default::default()
                })
            )
            .await
            .unwrap()
            .answered
        );
        let r = world(World::default());
        let mut renders_nothing = judge();
        renders_nothing.render = Arc::new(|_input, _ctx| Ok(Vec::new()));
        assert!(
            !run(&renders_nothing, &ticket("x"), &r)
                .await
                .unwrap()
                .answered
        );
    }

    #[tokio::test]
    async fn answered_is_false_when_a_stream_dies_mid_flight() {
        // THE CASE `raw !== null` GOT WRONG. The partial IS the diagnosis and
        // stays on `raw`, but the turn produced no answer — channel-plan's
        // route has to say 502 here rather than "nothing to plan yet".
        let r = world(World::default());
        let mut plain = titler();
        plain.guard = Some(GuardDecl {
            rules: None,
            redact: false,
        });
        let res = execute(
            &r.deps(),
            &plain,
            &json!({ "transcript": "x" }),
            ctx(&r),
            Some(StreamOptions {
                stream: Arc::new(|_req, emit| {
                    Box::pin(async move {
                        emit("Migrating the ");
                        Err("socket hang up".to_string())
                    })
                }),
                on_delta: None,
            }),
        )
        .await
        .unwrap();

        assert!(!res.answered);
        assert_eq!(res.raw, Some("Migrating the ".into()));
        assert!(res.error.as_deref().unwrap().contains("socket hang up"));
    }

    #[tokio::test]
    async fn answered_is_true_for_a_reply_the_contract_rejected() {
        // The model still spoke.
        let res = run(
            &judge(),
            &ticket("x"),
            &world(World {
                replies: replies(&["Sure! I will get right on that."]),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
        assert_eq!(res.value, None);
        assert!(res.answered);
        assert_eq!(res.raw, Some("Sure! I will get right on that.".into()));
    }

    #[tokio::test]
    async fn answered_is_false_when_the_model_said_nothing_at_all() {
        let res = run(
            &titler(),
            &json!({ "transcript": "x" }),
            &world(World {
                replies: replies(&["   "]),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
        assert!(!res.answered);
        assert!(res.error.as_deref().unwrap().contains("did not survive"));
    }

    #[tokio::test]
    async fn answered_is_true_on_a_run_that_worked() {
        assert!(
            run(&judge(), &ticket("x"), &world(World::default()))
                .await
                .unwrap()
                .answered
        );
    }

    // ── verify: the half of a contract a schema cannot state ─────────────────

    const CORRECT: &str = "{\"qwen3-14b\":\"A capable mid-size model.\",\"llama-3.3-70b\":\"Meta's flagship open model.\"}";
    const TIDIED: &str = "{\"Qwen3 14B\":\"A capable mid-size model.\",\"Llama 3.3 70B\":\"Meta's flagship open model.\"}";

    fn ids() -> Value {
        json!({ "ids": ["qwen3-14b", "llama-3.3-70b"] })
    }

    #[tokio::test]
    async fn verify_passes_a_value_whose_relation_to_the_input_holds() {
        let r = world(World {
            replies: replies(&[CORRECT]),
            ..Default::default()
        });
        let res = run(&blurber(), &ids(), &r).await.unwrap();
        assert_eq!(
            res.value,
            Some(json!({
                "qwen3-14b": "A capable mid-size model.",
                "llama-3.3-70b": "Meta's flagship open model."
            }))
        );
        assert!(res.schema_valid);
        assert_eq!(res.repairs, 0);
    }

    #[tokio::test]
    async fn verify_fails_then_repairs_what_the_schema_accepted() {
        let r = world(World {
            replies: replies(&[TIDIED, CORRECT]),
            ..Default::default()
        });
        let res = run(&blurber(), &ids(), &r).await.unwrap();

        assert_eq!(
            res.value,
            Some(json!({
                "qwen3-14b": "A capable mid-size model.",
                "llama-3.3-70b": "Meta's flagship open model."
            }))
        );
        assert_eq!(res.repairs, 1);
        assert!(res.schema_valid);
        // Same loop, same counter, same repair prompt — a verify failure IS a
        // contract failure, and the sentence the harness wrote is the
        // instruction the model gets back.
        let turn = req_at(&r, 1)
            .messages
            .last()
            .expect("messages")
            .content
            .clone();
        assert!(turn.contains("the keys must be the model ids exactly as they were given"));
        assert!(turn.contains("\"Qwen3 14B\""));
    }

    #[tokio::test]
    async fn an_unrepaired_verify_failure_is_a_contract_failure_on_result_and_row() {
        let r = world(World {
            replies: replies(&[TIDIED, TIDIED]),
            ..Default::default()
        });
        let res = run(&blurber(), &ids(), &r).await.unwrap();

        assert_eq!(res.value, None);
        assert!(!res.schema_valid);
        // The honest row is the entire point: before this the run recorded
        // `schema_valid: true` for a batch that produced no blurbs, so the
        // harness re-burned it every ten minutes and the fitness matrix called
        // the model a 100% performer for doing it.
        let row = run_at(&r, 0);
        assert!(!row.schema_valid);
        assert!(
            row.error
                .as_deref()
                .unwrap()
                .contains("the keys must be the model ids")
        );
        // It answered — twice. That is a different fact from holding the
        // contract.
        assert!(res.answered);
        assert_eq!(res.repairs, 1);
    }

    #[tokio::test]
    async fn verify_receives_the_original_input_on_every_attempt() {
        // By the second turn `sent` carries the model's own rejected answer. A
        // verify graded against that would drift further from the caller's
        // actual request on every repair, which is the opposite of what a
        // repair is for.
        let seen: Arc<Mutex<Vec<Value>>> = Arc::new(Mutex::new(Vec::new()));
        let recording = seen.clone();
        let mut def = blurber();
        def.output = Output::Json {
            schema: Schema::Record(Box::new(Schema::string())),
            repair: None,
            verify: Some(Arc::new(move |_value, input, _ctx| {
                recording.lock().expect("seen").push(input.clone());
                Ok(Some("never satisfied".to_string()))
            })),
        };
        let r = world(World {
            replies: replies(&[CORRECT, CORRECT]),
            ..Default::default()
        });
        run(&def, &ids(), &r).await.unwrap();

        let seen = seen.lock().expect("seen").clone();
        assert_eq!(seen.len(), 2);
        assert_eq!(seen[0], ids());
        assert_eq!(seen[1], ids());
    }

    #[tokio::test]
    async fn a_verify_that_throws_is_a_contract_failure() {
        // Harness-author code meeting model output, exactly like `render`,
        // `clean` and `ground` — held to the same rule, because a runner whose
        // whole promise is that a bad model produces a RESULT cannot have one
        // function that throws out of it.
        let mut boom = blurber();
        boom.output = Output::Json {
            schema: Schema::Record(Box::new(Schema::string())),
            repair: None,
            verify: Some(Arc::new(|_value, _input, _ctx| {
                Err("Cannot read properties of undefined (reading 'length')".to_string())
            })),
        };
        let r = world(World {
            replies: replies(&[CORRECT]),
            ..Default::default()
        });
        let res = run(&boom, &ids(), &r).await.unwrap();
        assert_eq!(res.value, None);
        assert!(!res.schema_valid);
        assert!(res.error.as_deref().unwrap().contains("verify step threw"));
        assert_eq!(n_runs(&r), 1);
    }

    #[tokio::test]
    async fn verify_never_runs_on_a_value_that_did_not_parse() {
        // It is defined over the parsed value, so there has to be one. A model
        // that answered in prose is a parse failure and the parser's sentence
        // is the repair.
        let calls = Arc::new(Mutex::new(0));
        let counting = calls.clone();
        let mut def = blurber();
        def.output = Output::Json {
            schema: Schema::Record(Box::new(Schema::string())),
            repair: None,
            verify: Some(Arc::new(move |_value, _input, _ctx| {
                *counting.lock().expect("calls") += 1;
                Ok(None)
            })),
        };
        let res = run(
            &def,
            &ids(),
            &world(World {
                replies: replies(&["Sure! Which models did you mean?"]),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
        assert_eq!(*calls.lock().expect("calls"), 0);
        assert!(
            res.error
                .as_deref()
                .unwrap()
                .contains("no JSON object or array was found")
        );
    }

    #[tokio::test]
    async fn verify_re_checks_the_redacted_value() {
        // A value can survive being cut and still parse: the schema says the
        // field is a string, and only the harness can say the string still has
        // to be the thing that was asked for. The contract is re-applied whole
        // after redaction, `verify` included.
        let leaky =
            "{\"qwen3-14b\":\"Ships with key AKIAIOSFODNN7EXAMPLE.\",\"llama-3.3-70b\":\"Fine.\"}";
        let mut strict = blurber();
        strict.guard = Some(GuardDecl {
            rules: None,
            redact: true,
        });
        strict.output = Output::Json {
            schema: Schema::Record(Box::new(Schema::string())),
            repair: None,
            verify: Some(Arc::new(|value, _input, _ctx| {
                let tainted = value
                    .as_object()
                    .map(|o| {
                        o.values()
                            .any(|b| b.as_str().is_some_and(|s| s.contains("[redacted")))
                    })
                    .unwrap_or(false);
                if tainted {
                    Ok(Some(
                        "every blurb must describe the model, not the redaction that replaced its text."
                            .to_string(),
                    ))
                } else {
                    Ok(None)
                }
            })),
        };
        let r = world(World {
            replies: replies(&[leaky]),
            ..Default::default()
        });
        let res = run(&strict, &ids(), &r).await.unwrap();

        assert_eq!(res.value, None);
        assert!(!res.schema_valid);
        assert!(
            res.error
                .as_deref()
                .unwrap()
                .contains("no longer satisfies the contract")
        );
    }

    #[tokio::test]
    async fn verify_works_on_a_text_harness_too() {
        // "Did it answer the question I asked" is the same question as "are
        // these the ids I sent", and `clean` cannot ask it either — it is
        // written before the input exists.
        let mut echoing = titler();
        echoing.output = Output::Text {
            clean: Some(Arc::new(|raw: &str| {
                let t = raw.trim();
                if t.is_empty() {
                    Ok(None)
                } else {
                    Ok(Some(Value::String(t.to_string())))
                }
            })),
            verify: Some(Arc::new(|value, input, _ctx| {
                let title = value.as_str().unwrap_or_default();
                let transcript = input["transcript"].as_str().unwrap_or_default();
                if transcript.starts_with(title) {
                    Ok(Some(
                        "the title must name what the conversation is about, not repeat its opening words."
                            .to_string(),
                    ))
                } else {
                    Ok(None)
                }
            })),
        };
        let res = run(
            &echoing,
            &json!({ "transcript": "Migrating the ledger to Postgres, and what that costs" }),
            &world(World {
                replies: replies(&["Migrating the ledger"]),
                ..Default::default()
            }),
        )
        .await
        .unwrap();
        assert_eq!(res.value, None);
        assert!(
            res.error
                .as_deref()
                .unwrap()
                .contains("must name what the conversation is about")
        );
        // A text harness does not repair: the one repair wording lives in
        // json.rs and ends "send the corrected JSON value only".
        assert_eq!(res.repairs, 0);
    }

    // ── Capability gating ────────────────────────────────────────────────────

    #[tokio::test]
    async fn refuses_below_the_floor_names_the_capability_never_calls() {
        let r = world(World {
            facts: facts(&[("spark", "json", probe(false))]),
            ..Default::default()
        });
        let res = run(&judge(), &ticket("x"), &r).await.unwrap();

        assert_eq!(n_requests(&r), 0);
        assert_eq!(res.value, None);
        assert!(res.error.as_deref().unwrap().contains("json"));
        assert!(res.error.as_deref().unwrap().contains(judge().floor.note));
        // The refusal is still a run: an operator has to be able to see that
        // this model is being asked to do a job it cannot do — and the row
        // carries the SENTENCE, not just the false.
        let row = run_at(&r, 0);
        assert_eq!(row.harness, "judge");
        assert_eq!(row.model, Some("pl-main".into()));
        assert_eq!(row.step, Some("pin"));
        assert!(!row.widened);
        assert_eq!(row.repairs, 0);
        assert!(!row.schema_valid);
        assert_eq!(row.latency_ms, 7);
        assert_eq!(row.findings, 0);
        assert_eq!(row.caller, "test:harness");
        assert!(
            row.error
                .as_deref()
                .unwrap()
                .contains("cannot run harness \"judge\"")
        );
    }

    #[tokio::test]
    async fn sends_the_harness_own_schema_not_the_loose_json_object() {
        // THE BUG THIS LOCKS. Every structured call carried
        // `response_format: { type: 'json_object' }`, hardcoded in three
        // places. Anthropic's OpenAI-compatible layer answers that with a 400,
        // so every JSON harness 400'd on every Claude model — and the fitness
        // suite scored it as the MODEL failing to hold a contract. The schema
        // was in the harness the whole time; it was only ever used to reject
        // the reply afterwards.
        let r = world(World::default());
        run(&judge(), &ticket("x"), &r).await.unwrap();

        let req = req_at(&r, 0);
        assert!(req.json_mode);
        let wire = req.json_schema.expect("wire schema");
        assert_eq!(wire.schema["type"], "object");
        assert!(wire.schema["properties"].get("verdict").is_some());
        assert!(wire.schema["properties"].get("summary").is_some());
        assert!(
            !wire.name.is_empty()
                && wire
                    .name
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        );
    }

    #[tokio::test]
    async fn does_not_refuse_on_a_fact_the_gateway_merely_learned() {
        // The gateway writes `json: false` the first time an upstream rejects
        // `response_format`. Read as a floor, one 400 turned the QA gate off
        // for every board for the 30-day learned TTL. That fact is evidence
        // about ONE PARAMETER: it still shapes the request (no protocol JSON,
        // prompt anchor only), and the model still gets to answer.
        let r = world(World {
            facts: facts(&[("spark", "json", sourced(false, "learned"))]),
            ..Default::default()
        });
        let res = run(&judge(), &ticket("x"), &r).await.unwrap();
        assert_eq!(n_requests(&r), 1);
        let req = req_at(&r, 0);
        assert!(!req.json_mode);
        assert!(
            req.messages
                .last()
                .expect("messages")
                .content
                .contains("exactly one JSON value")
        );
        assert_eq!(
            res.value,
            Some(json!({ "verdict": "pass", "summary": "looks right" }))
        );
    }

    #[tokio::test]
    async fn does_refuse_on_a_fact_somebody_deliberately_measured() {
        for source in ["probe", "declared"] {
            let r = world(World {
                facts: facts(&[("spark", "json", sourced(false, source))]),
                ..Default::default()
            });
            let res = run(&judge(), &ticket("x"), &r).await.unwrap();
            assert_eq!(n_requests(&r), 0, "refused on a {source} fact");
            assert!(
                res.error
                    .as_deref()
                    .unwrap()
                    .contains("cannot run harness \"judge\""),
                "refused on a {source} fact"
            );
        }
    }

    #[tokio::test]
    async fn runs_anyway_when_the_harness_degrades_rather_than_refuses() {
        let r = world(World {
            facts: facts(&[("spark", "json", probe(false))]),
            ..Default::default()
        });
        let mut soft = judge();
        soft.floor.refuse_below = false;
        let res = run(&soft, &ticket("x"), &r).await.unwrap();
        assert_eq!(n_requests(&r), 1);
        assert!(res.value.is_some());
    }

    #[tokio::test]
    async fn unknown_is_not_missing_an_unprobed_model_still_runs() {
        // A fresh self-host has probed nothing. Talaria cannot refuse to work
        // until an admin gets around to running a benchmark.
        let r = world(World::default());
        let res = run(&judge(), &ticket("x"), &r).await.unwrap();
        assert_eq!(n_requests(&r), 1);
        assert!(res.value.is_some());
    }

    #[tokio::test]
    async fn needs_the_whole_pool_to_agree_before_it_refuses() {
        // A bare model name may be served by several endpoints and we cannot
        // know which one takes this call without perturbing the round-robin
        // cursor. One member saying "no" is not knowledge about the call we
        // are about to make.
        let r = world(World {
            endpoints: Some(vec!["spark".into(), "spark-2".into()]),
            facts: facts(&[("spark", "json", probe(false))]),
            ..Default::default()
        });
        let res = run(&judge(), &ticket("x"), &r).await.unwrap();
        assert_eq!(n_requests(&r), 1);
        assert!(res.value.is_some());
    }

    // ── Widening ─────────────────────────────────────────────────────────────

    fn wide_judge() -> HarnessDefinition {
        let mut d = judge();
        d.widen = Some(Widen {
            requires: vec!["tool-select"],
            note: "Models that pick the right tool get the full action list.",
        });
        d
    }

    #[tokio::test]
    async fn widens_only_when_the_capability_is_known_true() {
        let r = world(World {
            facts: facts(&[("spark", "tool-select", probe(true))]),
            ..Default::default()
        });
        let res = run(&wide_judge(), &ticket("x"), &r).await.unwrap();
        assert!(res.widened);
        assert!(req_at(&r, 0).messages[0].content.contains("cite the diff"));
        assert!(run_at(&r, 0).widened);
    }

    #[tokio::test]
    async fn does_not_widen_on_unknown() {
        // The floor asks "is this model known to be UNABLE"; widening asks
        // "is it known to be ABLE". Unknown answers neither, and in both
        // cases the safe move is the same one: keep running, on the
        // deterministic surface.
        let r = world(World::default());
        let res = run(&wide_judge(), &ticket("x"), &r).await.unwrap();
        assert!(!res.widened);
        assert!(
            req_at(&r, 0).messages[0]
                .content
                .contains("Judge the reported outcome")
        );
    }

    #[tokio::test]
    async fn does_not_widen_unless_every_endpoint_in_the_pool_earned_it() {
        let r = world(World {
            endpoints: Some(vec!["spark".into(), "spark-2".into()]),
            facts: facts(&[("spark", "tool-select", probe(true))]),
            ..Default::default()
        });
        assert!(!run(&wide_judge(), &ticket("x"), &r).await.unwrap().widened);
    }

    #[tokio::test]
    async fn a_harness_with_no_widen_declaration_never_widens() {
        let r = world(World {
            facts: facts(&[("spark", "tool-select", probe(true))]),
            ..Default::default()
        });
        assert!(!run(&judge(), &ticket("x"), &r).await.unwrap().widened);
    }

    // Provenance: only a fact THIS PLATFORM MEASURED may widen. The floor one
    // branch up refuses on anything that is not `learned`, so a human or a
    // catalog saying "this model cannot do JSON" is grounds to stop. Widening
    // is the other direction — it HANDS A MODEL MORE AUTHORITY — so it takes
    // a measurement and nothing else. The asymmetry is deliberate and a
    // reader will assume it is a bug; these are the assertions that say
    // otherwise.

    #[tokio::test]
    async fn ignores_a_declared_fact() {
        // The day anything imports a model catalog as `declared: true`, this
        // is what stops a marketing claim from arming the widened surface on
        // every install that synced it.
        let r = world(World {
            facts: facts(&[("spark", "tool-select", sourced(true, "declared"))]),
            ..Default::default()
        });
        let res = run(&wide_judge(), &ticket("x"), &r).await.unwrap();
        assert!(!res.widened);
        assert!(
            req_at(&r, 0).messages[0]
                .content
                .contains("Judge the reported outcome")
        );
        // Not a refusal either: an unmeasured model still runs, on the narrow
        // surface, which is the whole "decent on a 14B" half of the
        // requirement.
        assert!(res.value.is_some());
    }

    #[tokio::test]
    async fn ignores_a_learned_fact_for_the_same_reason() {
        let r = world(World {
            facts: facts(&[("spark", "tool-select", sourced(true, "learned"))]),
            ..Default::default()
        });
        assert!(!run(&wide_judge(), &ticket("x"), &r).await.unwrap().widened);
    }

    #[tokio::test]
    async fn honors_a_probe_fact() {
        // The one source that is Talaria measuring the model itself.
        let r = world(World {
            facts: facts(&[("spark", "tool-select", sourced(true, "probe"))]),
            ..Default::default()
        });
        let res = run(&wide_judge(), &ticket("x"), &r).await.unwrap();
        assert!(res.widened);
        assert!(run_at(&r, 0).widened);
    }

    #[tokio::test]
    async fn needs_the_whole_pool_measured_not_one_probe_and_one_claim() {
        let r = world(World {
            endpoints: Some(vec!["spark".into(), "spark-2".into()]),
            facts: facts(&[
                ("spark:pl-main", "tool-select", sourced(true, "probe")),
                ("spark-2:pl-main", "tool-select", sourced(true, "declared")),
            ]),
            ..Default::default()
        });
        assert!(!run(&wide_judge(), &ticket("x"), &r).await.unwrap().widened);
    }

    // ── Tool definitions on the request ──────────────────────────────────────

    fn weather_tool() -> crate::harness::transport::ToolDefinition {
        crate::harness::transport::ToolDefinition {
            name: "get_weather".into(),
            description: "Current weather for a city.".into(),
            parameters: json!({
                "type": "object",
                "properties": { "city": { "type": "string" } }
            }),
        }
    }

    #[tokio::test]
    async fn carries_the_definitions_distinct_from_the_tool_policy() {
        // The two are different sentences about different tools: the policy
        // governs the model's OWN loop (a persona's, running inside the
        // agent), and these are tools we hand over and watch being called.
        // `tool-select` — the probe that gates the Inbox widening — is
        // unrunnable without this slot.
        let r = world(World::default());
        let mut offering = judge();
        offering.tool_defs = vec![weather_tool()];
        run(&offering, &ticket("x"), &r).await.unwrap();
        let req = req_at(&r, 0);
        assert_eq!(req.tool_defs.len(), 1);
        assert_eq!(req.tool_defs[0].name, "get_weather");
        assert_eq!(req.tools, None);
    }

    #[tokio::test]
    async fn sends_no_slot_at_all_when_the_harness_declares_none() {
        // So a transport cannot see an empty offer.
        let r = world(World::default());
        run(&judge(), &ticket("x"), &r).await.unwrap();
        assert!(req_at(&r, 0).tool_defs.is_empty());
    }

    #[tokio::test]
    async fn guards_a_reported_tool_call_as_names_without_results() {
        // Nothing executed the call, so there is no result text to ground a
        // citation against. Counting the name as a backing tool with an empty
        // results record would make `ungrounded_ref` fire on every id in the
        // reply; the fleet path has always been handled this way and the
        // gateway now joins it.
        let with_call = Reply::Full(TransportReply {
            kind: TransportKind::Gateway,
            text: "Filed 3f0c8a52-6b1d-4a7e-9d21-0f8e5c4b2a91 for you.".into(),
            tool_names: vec!["create_ticket".into()],
            tool_calls: Some(vec![crate::harness::transport::ToolCall {
                name: "create_ticket".into(),
                id: None,
                args: "{\"title\":\"x\"}".into(),
            }]),
            usage: None,
            contract_dropped: false,
        });
        let r = world(World {
            replies: vec![with_call],
            ..Default::default()
        });
        let mut offering = titler();
        offering.tool_defs = vec![weather_tool()];
        let res = run(&offering, &json!({ "transcript": "x" }), &r)
            .await
            .unwrap();
        assert!(!checks(&res).contains(&"ungrounded_ref"));
    }

    // ── Fleet personas ───────────────────────────────────────────────────────

    /// Penny: a 14B local main, a frontier "opus" tier, no fallbacks.
    fn penny() -> PersonaRow {
        PersonaRow {
            agent: "assistant-operations".into(),
            config: json!({
                "main": { "endpoint": "spark", "model": "qwen3-14b" },
                "aliases": [{ "name": "opus", "endpoint": "anthropic", "model": "claude-opus-4" }],
            }),
        }
    }

    fn on_persona(id: &str) -> ModelAnswer {
        ModelAnswer::Resolved(id.into(), "pin")
    }

    #[tokio::test]
    async fn inherits_the_probe_of_the_model_behind_it_and_widens() {
        // A persona is not a gateway catalog model, so routing answers with
        // no endpoints for one — which is precisely the condition that used
        // to leave `keys` empty and made widening a structural impossibility
        // on the very path the feature was built for.
        let r = world(World {
            model: on_persona("assistant-operations"),
            personas: vec![penny()],
            facts: facts(&[("spark", "tool-select", probe(true))]),
            ..Default::default()
        });
        let res = run(&wide_judge(), &ticket("x"), &r).await.unwrap();

        assert!(res.widened);
        assert!(req_at(&r, 0).messages[0].content.contains("cite the diff"));
        assert!(run_at(&r, 0).widened);
    }

    #[tokio::test]
    async fn does_not_widen_on_a_persona_nobody_has_probed_but_runs() {
        let r = world(World {
            model: on_persona("assistant-operations"),
            personas: vec![penny()],
            ..Default::default()
        });
        let res = run(&wide_judge(), &ticket("x"), &r).await.unwrap();

        assert!(!res.widened);
        assert_eq!(n_requests(&r), 1);
        assert!(res.value.is_some());
    }

    #[tokio::test]
    async fn resolves_the_tier_being_called_not_the_agent_main() {
        // "assistant-operations-opus" is a different, larger model than
        // "assistant-operations" even though one id is a prefix of the other.
        // Crediting main's probe to the tier would widen on a fact about
        // something else entirely.
        let f = facts(&[("anthropic", "tool-select", probe(true))]);
        let base = world(World {
            model: on_persona("assistant-operations"),
            personas: vec![penny()],
            facts: f.clone(),
            ..Default::default()
        });
        let tier = world(World {
            model: on_persona("assistant-operations-opus"),
            personas: vec![penny()],
            facts: f,
            ..Default::default()
        });

        assert!(
            !run(&wide_judge(), &ticket("x"), &base)
                .await
                .unwrap()
                .widened
        );
        assert!(
            run(&wide_judge(), &ticket("x"), &tier)
                .await
                .unwrap()
                .widened
        );
    }

    #[tokio::test]
    async fn yields_no_keys_for_a_tier_the_agent_does_not_have() {
        // Inheriting the wrong model's capabilities is worse than inheriting
        // none: unknown is safe in both directions by design, and a wrong
        // fact is not.
        let r = world(World {
            model: on_persona("assistant-operations-turbo"),
            personas: vec![penny()],
            facts: facts(&[("spark", "tool-select", probe(true))]),
            ..Default::default()
        });
        let res = run(&wide_judge(), &ticket("x"), &r).await.unwrap();

        assert!(!res.widened);
        assert_eq!(n_requests(&r), 1); // unknown still RUNS
    }

    #[tokio::test]
    async fn an_agent_whose_own_id_looks_like_another_agent_tier_keeps_its_config() {
        let impostor = PersonaRow {
            agent: "assistant-operations-opus".into(),
            config: json!({ "main": { "endpoint": "spark", "model": "qwen3-14b" } }),
        };
        let r = world(World {
            model: on_persona("assistant-operations-opus"),
            personas: vec![penny(), impostor],
            facts: facts(&[("anthropic", "tool-select", probe(true))]),
            ..Default::default()
        });
        // Penny's "opus" alias points at anthropic, but this id belongs to a
        // real agent of its own whose main is the 14B — the agent's own
        // config wins.
        assert!(!run(&wide_judge(), &ticket("x"), &r).await.unwrap().widened);
    }

    /// A persona with a fallback provider — the model that answers is
    /// genuinely unknowable in advance.
    fn fallback_agent() -> PersonaRow {
        PersonaRow {
            agent: "engineer-engineering".into(),
            config: json!({
                "main": { "endpoint": "spark", "model": "qwen3-14b" },
                "fallbacks": [{ "endpoint": "thunder", "model": "llama3-8b" }],
            }),
        }
    }

    #[tokio::test]
    async fn keeps_the_pool_unanimous_across_a_fallback_chain() {
        // The persona moves to a fallback provider when the main errors, so
        // which model answers is genuinely unknowable in advance — the same
        // situation as a gateway model served by several endpoints, and it
        // gets the same answer.
        let half = world(World {
            model: on_persona("engineer-engineering"),
            personas: vec![fallback_agent()],
            facts: facts(&[("spark", "tool-select", probe(true))]),
            ..Default::default()
        });
        let all = world(World {
            model: on_persona("engineer-engineering"),
            personas: vec![fallback_agent()],
            facts: facts(&[
                ("spark", "tool-select", probe(true)),
                ("thunder", "tool-select", probe(true)),
            ]),
            ..Default::default()
        });

        assert!(
            !run(&wide_judge(), &ticket("x"), &half)
                .await
                .unwrap()
                .widened
        );
        assert!(
            run(&wide_judge(), &ticket("x"), &all)
                .await
                .unwrap()
                .widened
        );
    }

    #[tokio::test]
    async fn refuses_below_the_floor_when_every_model_behind_the_persona_fails_it() {
        // The keys feed the floor as well as the widening: a persona backed
        // by a model known to be unable to return JSON is a judge that would
        // escalate everything.
        let r = world(World {
            model: on_persona("assistant-operations"),
            personas: vec![penny()],
            facts: facts(&[("spark", "json", probe(false))]),
            ..Default::default()
        });
        let res = run(&judge(), &ticket("x"), &r).await.unwrap();

        assert_eq!(n_requests(&r), 0);
        assert!(res.error.as_deref().unwrap().contains("json"));
    }

    #[tokio::test]
    async fn does_not_refuse_when_one_pool_member_is_merely_unprobed() {
        let with_fallback = PersonaRow {
            agent: "engineer-engineering".into(),
            config: json!({
                "main": { "endpoint": "spark", "model": "qwen3-14b" },
                "fallbacks": [{ "endpoint": "thunder", "model": "llama3-8b" }],
            }),
        };
        let r = world(World {
            model: on_persona("engineer-engineering"),
            personas: vec![with_fallback],
            facts: facts(&[("spark", "json", probe(false))]),
            ..Default::default()
        });
        let res = run(&judge(), &ticket("x"), &r).await.unwrap();

        assert_eq!(n_requests(&r), 1);
        assert!(res.value.is_some());
    }

    #[tokio::test]
    async fn carries_on_when_the_config_lookup_dies() {
        // Resolving a persona hits the database. That lookup exists to make a
        // run BETTER; it is not a precondition for running one, and a
        // database blip must never be the reason a harness fails. (The Rust
        // edge's type says the same thing structurally — see `World`.)
        let r = world(World {
            model: on_persona("assistant-operations"),
            personas_throw: true,
            ..Default::default()
        });
        let res = run(&wide_judge(), &ticket("x"), &r).await.unwrap();

        assert!(!res.widened);
        assert_eq!(
            res.value,
            Some(json!({ "verdict": "pass", "summary": "looks right" }))
        );
        assert_eq!(res.error, None);
    }

    #[tokio::test]
    async fn never_consults_the_persona_index_for_a_routed_model() {
        // The gateway's own endpoints are the authoritative answer when there
        // are any; the persona lookup is strictly the empty-handed case.
        let r = world(World {
            endpoints: Some(vec!["spark".into()]),
            personas_throw: true,
            facts: facts(&[("spark", "tool-select", probe(true))]),
            ..Default::default()
        });
        assert!(run(&wide_judge(), &ticket("x"), &r).await.unwrap().widened);
    }

    // ── What a red cell says ─────────────────────────────────────────────────

    #[tokio::test]
    async fn records_the_sentence_behind_a_failed_contract() {
        let r = world(World {
            replies: replies(&["not json", "still not json"]),
            ..Default::default()
        });
        let res = run(&judge(), &ticket("x"), &r).await.unwrap();

        let row = run_at(&r, 0);
        assert_eq!(row.error, res.error);
        assert!(
            row.error
                .as_deref()
                .unwrap()
                .contains("no JSON object or array was found")
        );
    }

    #[tokio::test]
    async fn leaves_the_column_null_on_a_run_that_worked() {
        let r = world(World::default());
        run(&judge(), &ticket("x"), &r).await.unwrap();
        assert_eq!(run_at(&r, 0).error, None);
    }

    #[tokio::test]
    async fn scrubs_a_credential_the_parser_quoted_back_out() {
        // Parser errors quote the model's own rejected value, so this string
        // is model output too — and unlike `raw` it is kept forever.
        // Redaction happens BEFORE the length bound, because slicing first
        // can cut a credential in half so that no pattern matches the tail.
        let r = world(World {
            replies: replies(&[
                "{\"verdict\":\"AKIAIOSFODNN7EXAMPLE\",\"summary\":\"x\"}",
                "nope",
            ]),
            ..Default::default()
        });
        let res = run(&judge(), &ticket("x"), &r).await.unwrap();

        let error = res.error.as_deref().unwrap();
        assert!(error.contains("AKIAIOSFODNN7EXAMPLE")); // the live result is untouched
        let row_error = run_at(&r, 0).error.unwrap();
        assert!(!row_error.contains("AKIAIOSFODNN7EXAMPLE"));
        assert!(row_error.contains("[redacted AWS access key]"));
    }

    #[tokio::test]
    async fn bounds_a_pathological_failure_sentence() {
        let long = "x".repeat(5_000);
        let r = world(World {
            replies: vec![
                Reply::Text(format!("{{\"verdict\":\"{long}\",\"summary\":\"y\"}}")),
                Reply::Text("nope".into()),
            ],
            ..Default::default()
        });
        run(&judge(), &ticket("x"), &r).await.unwrap();
        assert!(
            run_at(&r, 0)
                .error
                .as_deref()
                .map(|e| e.chars().count() <= 1_000)
                .unwrap()
        );
    }

    // ── The guard pass ───────────────────────────────────────────────────────

    const LEAKY: &str =
        "{\"verdict\":\"pass\",\"summary\":\"deployed with key AKIAIOSFODNN7EXAMPLE\"}";

    #[tokio::test]
    async fn records_findings_against_the_harness_output() {
        let r = world(World {
            replies: replies(&[LEAKY]),
            ..Default::default()
        });
        let res = run(&judge(), &ticket("x"), &r).await.unwrap();

        assert!(checks(&res).contains(&"secret_leak"));
        assert!(recorded(&r).iter().any(|f| f.check == "secret_leak"));
        assert_eq!(run_at(&r, 0).findings, 1);
        // Observe mode never touches the value — that is the default posture.
        assert!(
            res.value.unwrap()["summary"]
                .as_str()
                .unwrap()
                .contains("AKIAIOSFODNN7EXAMPLE")
        );
    }

    #[tokio::test]
    async fn redacts_the_value_and_re_applies_the_contract() {
        let r = world(World {
            replies: replies(&[LEAKY]),
            ..Default::default()
        });
        let mut redacting = judge();
        redacting.guard = Some(GuardDecl {
            rules: None,
            redact: true,
        });
        let res = run(&redacting, &ticket("x"), &r).await.unwrap();
        assert_eq!(
            res.value.unwrap()["summary"],
            json!("deployed with key [redacted AWS access key]")
        );
        assert!(res.schema_valid);
    }

    #[tokio::test]
    async fn narrows_the_registry_to_the_rules_a_harness_declared() {
        let r = world(World {
            replies: replies(&[LEAKY]),
            ..Default::default()
        });
        let mut narrowed = judge();
        narrowed.guard = Some(GuardDecl {
            rules: Some(vec!["zero_tool_claim"]),
            redact: false,
        });
        let res = run(&narrowed, &ticket("x"), &r).await.unwrap();
        assert!(res.findings.is_empty());
        assert!(recorded(&r).is_empty());
    }

    #[tokio::test]
    async fn runs_nothing_when_the_guard_is_off() {
        let r = world(World {
            replies: replies(&[LEAKY]),
            guard_mode: GuardMode::Off,
            ..Default::default()
        });
        let res = run(&judge(), &ticket("x"), &r).await.unwrap();
        assert!(res.findings.is_empty());
        assert!(recorded(&r).is_empty());
    }

    #[tokio::test]
    async fn skips_the_rules_a_fleet_transport_cannot_honestly_supply() {
        // The persona's tool loop runs inside the agent, so the stream gives
        // tool NAMES and nothing else. A rule that needs tool RESULTS is
        // skipped rather than run on missing data — and 'think' is not a
        // backing tool, so the zero-tool claim still stands.
        let fleet_reply = Reply::Full(TransportReply {
            kind: TransportKind::Fleet,
            text: "Created the ticket 3f0c8a52-6b1d-4a7e-9d21-0f8e5c4b2a91.".into(),
            tool_names: vec!["think".into()],
            tool_calls: None,
            usage: None,
            contract_dropped: false,
        });
        let r = world(World {
            replies: vec![fleet_reply],
            ..Default::default()
        });
        let res = run(&titler(), &json!({ "transcript": "x" }), &r)
            .await
            .unwrap();

        assert!(!checks(&res).contains(&"ungrounded_ref"));
        assert!(checks(&res).contains(&"zero_tool_claim"));
    }

    // ── Grounding the guard against the run's own input ──────────────────────
    //
    // `groundingTextOf` shipped wired to the gateway's own completion path and
    // THIS RUNNER NOT — so the one path that guards every harness was the one
    // path that grounded nothing, and it is the path that also REDACTS THE
    // VALUE.

    /// Luhn-valid, so `pii_leak` reads it as a payment card. It is an ORDER
    /// NUMBER in the ticket, which is the whole measured problem: business
    /// identifiers share the shapes the detectors match.
    const ORDER: &str = "4242424242424242";

    fn support_judge() -> HarnessDefinition {
        let mut d = judge();
        d.id = "support-judge";
        // The ticket text goes in the SYSTEM message. That is deliberate and
        // it is what separates this from the pre-grounding behavior.
        d.render = Arc::new(|input: &Value, _ctx: &RenderContext| {
            let t = input["ticket"].as_str().unwrap_or_default();
            Ok(vec![
                Message::system(format!("Judge the outcome. Ticket: {t}")),
                Message::user("Did the agent resolve it?"),
            ])
        });
        d.guard = Some(GuardDecl {
            rules: None,
            redact: true,
        });
        d
    }

    fn quoted() -> String {
        format!(
            "{{\"verdict\":\"pass\",\"summary\":\"refunded order {ORDER} as the customer asked\"}}"
        )
    }

    #[tokio::test]
    async fn files_nothing_for_an_identifier_that_was_in_its_own_prompt() {
        let r = world(World {
            replies: replies(&[&quoted()]),
            ..Default::default()
        });
        let res = run(
            &support_judge(),
            &ticket(&format!("customer disputes order {ORDER}")),
            &r,
        )
        .await
        .unwrap();

        assert!(res.findings.is_empty());
        assert!(recorded(&r).is_empty());
        // The guard rate the fitness page reads has to agree with
        // `guard_findings`.
        assert_eq!(run_at(&r, 0).findings, 0);
    }

    #[tokio::test]
    async fn and_does_not_rewrite_it_out_of_the_persisted_value() {
        // The worse half of the bug: the finding is out-of-band telemetry,
        // the value is the artifact a human reads. A distillation in which
        // the order number has become `[redacted card number]` disagrees
        // with the chat it summarized.
        let r = world(World {
            replies: replies(&[&quoted()]),
            ..Default::default()
        });
        let res = run(
            &support_judge(),
            &ticket(&format!("customer disputes order {ORDER}")),
            &r,
        )
        .await
        .unwrap();
        assert!(
            res.value.unwrap()["summary"]
                .as_str()
                .unwrap()
                .contains(ORDER)
        );
    }

    #[tokio::test]
    async fn still_flags_and_redacts_a_number_nowhere_in_the_prompt() {
        let r = world(World {
            replies: replies(&[&quoted()]),
            ..Default::default()
        });
        let res = run(
            &support_judge(),
            &ticket("customer disputes a delivery"),
            &r,
        )
        .await
        .unwrap();

        assert!(checks(&res).contains(&"pii_leak"));
        assert!(
            res.value.unwrap()["summary"]
                .as_str()
                .unwrap()
                .contains("[redacted card number]")
        );
        assert_eq!(run_at(&r, 0).findings, 1);
    }

    #[tokio::test]
    async fn redacts_an_operator_pasted_credential_and_blames_nobody() {
        // `secret_leak` is `groundable: finding` — the other half of the
        // split. The key really is a key, so the copy Talaria writes down
        // loses it; the model did not invent it, so no row says it did.
        let key = "AKIAIOSFODNN7EXAMPLE";
        let r = world(World {
            replies: replies(&[&format!(
                "{{\"verdict\":\"pass\",\"summary\":\"rotated {key} as instructed\"}}"
            )]),
            ..Default::default()
        });
        let res = run(
            &support_judge(),
            &ticket(&format!("the leaked key is {key}")),
            &r,
        )
        .await
        .unwrap();

        let f = &res.findings[0];
        assert_eq!(f.check, "secret_leak");
        assert!(f.grounded);
        assert_eq!(
            res.value.unwrap()["summary"],
            json!("rotated [redacted AWS access key] as instructed")
        );
        assert_eq!(run_at(&r, 0).findings, 0);
    }

    #[tokio::test]
    async fn spends_the_repair_turn_on_a_prompt_quoting_reply() {
        // The repair gate reads the same material. Grounding it is what stops
        // the run's one second chance being burned on a finding that will
        // never be filed: the reply below is malformed AND carries the order
        // number, and before this the gate refused it and the harness
        // produced nothing.
        let malformed = format!("sure - order {ORDER}: {{\"verdict\":\"maybe\"");
        let r = world(World {
            replies: replies(&[&malformed, &quoted()]),
            ..Default::default()
        });
        let res = run(
            &support_judge(),
            &ticket(&format!("customer disputes order {ORDER}")),
            &r,
        )
        .await
        .unwrap();

        assert_eq!(res.repairs, 1);
        assert!(res.schema_valid);
        assert!(
            res.value.unwrap()["summary"]
                .as_str()
                .unwrap()
                .contains(ORDER)
        );
    }

    // ── Everything that can go wrong before a model is reached ───────────────

    #[tokio::test]
    async fn no_model_returns_a_result_and_still_writes_a_row() {
        let r = world(World {
            model: ModelAnswer::NoModel,
            ..Default::default()
        });
        let res = run(&judge(), &ticket("x"), &r).await.unwrap();

        assert_eq!(res.value, None);
        assert_eq!(res.model, None);
        assert!(res.error.as_deref().unwrap().contains("no model available"));
        assert_eq!(run_at(&r, 0).model, None);
    }

    #[tokio::test]
    async fn a_transport_failure_is_a_result_not_an_exception() {
        let r = world(World {
            transport_error: Some("gateway completion 503".into()),
            ..Default::default()
        });
        let res = run(&judge(), &ticket("x"), &r).await.unwrap();
        assert_eq!(res.value, None);
        assert!(res.error.as_deref().unwrap().contains("503"));
        assert_eq!(n_runs(&r), 1);
    }

    #[tokio::test]
    async fn takes_an_explicit_model_pin_without_consulting_the_chain() {
        // How the fitness suite replays a harness against a candidate model.
        let r = world(World {
            model: ModelAnswer::NoModel,
            ..Default::default()
        });
        let mut c = ctx(&r);
        c.caller = "fitness".into();
        c.model = Some("candidate-14b".into());
        let res = execute(&r.deps(), &judge(), &ticket("x"), c, None)
            .await
            .unwrap();
        assert_eq!(res.model, Some("candidate-14b".into()));
        assert_eq!(res.step, None);
        assert_eq!(req_at(&r, 0).model, "candidate-14b");
    }

    #[tokio::test]
    async fn fails_cleanly_when_a_render_produces_nothing() {
        let r = world(World::default());
        let mut empty = judge();
        empty.render = Arc::new(|_input, _ctx| Ok(Vec::new()));
        let res = run(&empty, &ticket("x"), &r).await.unwrap();
        assert!(
            res.error
                .as_deref()
                .unwrap()
                .contains("rendered no messages")
        );
        assert_eq!(n_requests(&r), 0);
    }

    // `render` and `clean` are the two pieces of HARNESS-AUTHOR code the
    // runner executes, so they are the two places an exception can escape a
    // function whose entire promise is that a bad model — or a bad harness —
    // produces a result. Both are contract failures, spelled the same way as
    // any other.

    #[tokio::test]
    async fn a_render_that_throws_is_a_result_not_an_exception() {
        let r = world(World::default());
        let mut boom = judge();
        boom.render = Arc::new(|_input, _ctx| {
            Err("Cannot read properties of undefined (reading 'title')".to_string())
        });
        let res = run(&boom, &ticket("x"), &r).await.unwrap();
        assert!(
            res.error
                .as_deref()
                .unwrap()
                .contains("rendered no messages")
        );
        assert_eq!(n_runs(&r), 1);
    }

    #[tokio::test]
    async fn a_clean_step_that_throws_fails_the_contract() {
        let r = world(World {
            replies: replies(&["a perfectly good title"]),
            ..Default::default()
        });
        let mut boom = titler();
        if let Output::Text { clean, .. } = &mut boom.output {
            *clean = Some(Arc::new(|_raw: &str| Err("nope".to_string())));
        }
        let res = run(&boom, &json!({ "transcript": "x" }), &r).await.unwrap();
        assert_eq!(res.value, None);
        assert!(res.error.as_deref().unwrap().contains("clean step threw"));
        assert_eq!(n_runs(&r), 1);
    }

    // ── Tool passthrough ─────────────────────────────────────────────────────
    //
    // Five callers once hand-wrote a persona transport because
    // `TransportRequest` had no slot for what they needed. All five are
    // deleted and every assertion below is one of the slots that replaced
    // them — these cases are what makes the deletion safe to keep.

    #[tokio::test]
    async fn tells_the_transport_when_the_model_may_use_its_own_tools() {
        // The work session, the outreach check-in and the briefing follow-up
        // are tool loops. `tools: []` does not weaken them — it disarms the
        // agent and then trips `zero_tool_claim` for calling no tool.
        let r = world(World {
            replies: replies(&["picked it up, running the tests now"]),
            ..Default::default()
        });
        let mut looping = titler();
        looping.tools = Some(ToolPolicy::Own);
        run(&looping, &json!({ "transcript": "x" }), &r)
            .await
            .unwrap();
        assert_eq!(req_at(&r, 0).tools, Some(ToolPolicy::Own));
    }

    #[tokio::test]
    async fn leaves_tools_absent_for_every_ordinary_harness() {
        let r = world(World::default());
        run(&judge(), &ticket("x"), &r).await.unwrap();
        assert_eq!(req_at(&r, 0).tools, None);
    }

    #[tokio::test]
    async fn carries_the_hold_deadline_a_slow_persona_needs() {
        // An agent restarting under a config propagation refuses connections
        // for tens of seconds; `proxyChat` waits two minutes by default and a
        // work session wants ten.
        let r = world(World {
            replies: replies(&["ok"]),
            ..Default::default()
        });
        let mut patient = titler();
        patient.hold_ms = Some(600_000);
        run(&patient, &json!({ "transcript": "x" }), &r)
            .await
            .unwrap();
        assert_eq!(req_at(&r, 0).hold_ms, Some(600_000));
    }

    #[tokio::test]
    async fn carries_the_callers_reasoning_effort_pick_or_nothing() {
        // The chat routes validate the level against the model's own metadata
        // before handing it here; the runner's only job is to not drop it on
        // the way to the transport.
        let picked = world(World {
            replies: replies(&["a title"]),
            ..Default::default()
        });
        let mut c = ctx(&picked);
        c.effort = Some("high".into());
        execute(
            &picked.deps(),
            &titler(),
            &json!({ "transcript": "x" }),
            c,
            None,
        )
        .await
        .unwrap();
        assert_eq!(req_at(&picked, 0).effort, Some("high".into()));
        // Absent when nobody picked one: the transports read absence as "send
        // no parameter", which is the model's own default effort.
        let plain = world(World {
            replies: replies(&["a title"]),
            ..Default::default()
        });
        run(&titler(), &json!({ "transcript": "x" }), &plain)
            .await
            .unwrap();
        assert_eq!(req_at(&plain, 0).effort, None);
    }

    #[tokio::test]
    async fn fails_rather_than_quietly_dropping_tools_a_transport_cannot_serve() {
        // The rule the whole request type is built on. A field that cannot be
        // honored has to fail the call, because a disarmed tool-loop harness
        // does not read as broken — it reads as an agent that decided not to
        // act. Both refusals are asserted before either touches the network:
        // the throw is the first statement in each transport.
        let cfg = std::sync::Arc::new(
            crate::config::Config::from_parts(
                "postgres://nobody:nobody@127.0.0.1:1/none".into(),
                "redis://127.0.0.1:1/1".into(),
                "test-root".into(),
                String::new(),
                String::new(),
                "5274".into(),
            )
            .expect("test config"),
        );
        // connect_lazy: no I/O at construction, and both refusals fire before
        // any query could run.
        let state = AppState::new(crate::db::pool(&cfg), cfg);
        let req = TransportRequest {
            model: "qwen3-14b".into(),
            messages: vec![Message::user("x")],
            temperature: None,
            json_mode: false,
            json_schema: None,
            tools: Some(ToolPolicy::Own),
            tool_defs: Vec::new(),
            ledger: None,
            effort: None,
            hold_ms: None,
            caller: "t".into(),
        };
        let err = super::super::transport::gateway_transport(&state, &req)
            .await
            .unwrap_err();
        assert!(err.contains("served by the ORG GATEWAY"));
        let err = super::super::transport::gateway_stream(&state, &req, |_| {})
            .await
            .unwrap_err();
        assert!(err.contains("served by the ORG GATEWAY"));
    }

    #[tokio::test]
    async fn still_sends_temperature_and_json_mode_alongside_them() {
        // THE REGRESSION THE FIVE SHIMS SHIPPED, asserted from the other
        // side: a harness that declares `temperature: 0` and runs at the
        // provider's default is a harness whose declaration is decorative,
        // and nothing said so.
        let r = world(World::default());
        let mut looping = judge();
        looping.tools = Some(ToolPolicy::Own);
        run(&looping, &ticket("x"), &r).await.unwrap();
        let req = req_at(&r, 0);
        assert_eq!(req.tools, Some(ToolPolicy::Own));
        assert_eq!(req.temperature, Some(0.0));
        assert!(req.json_mode);
    }

    // ── Ledger attribution ───────────────────────────────────────────────────

    #[tokio::test]
    async fn defaults_to_the_attribution_a_harness_turn_has_always_had() {
        let r = world(World::default());
        run(&judge(), &ticket("x"), &r).await.unwrap();
        let ledger = req_at(&r, 0).ledger.expect("ledger");
        assert_eq!(ledger.agent_model, "pl-main");
        assert_eq!(ledger.source, LedgerSource::Chat);
        assert_eq!(ledger.ref_id, None);
        assert_eq!(ledger.task_id, None);
        assert_eq!(ledger.tier, None);
    }

    #[tokio::test]
    async fn restores_researchs_own_source_and_run_id() {
        // The persona stages metered as `source: 'chat'` with no refId after
        // the port, so a run's cost stopped being answerable at all.
        let r = world(World::default());
        let mut c = ctx(&r);
        c.caller = "research:run-9".into();
        c.ledger = Some(RunLedger {
            source: Some(LedgerSource::Research),
            ref_id: Some("run-9".into()),
            task_id: None,
        });
        let res = execute(&r.deps(), &judge(), &ticket("x"), c, None)
            .await
            .unwrap();
        assert!(res.value.is_some());
        let ledger = req_at(&r, 0).ledger.expect("ledger");
        assert_eq!(ledger.agent_model, "pl-main");
        assert_eq!(ledger.source, LedgerSource::Research);
        assert_eq!(ledger.ref_id, Some("run-9".into()));
        assert_eq!(ledger.task_id, None);
        assert_eq!(ledger.tier, None);
    }

    #[tokio::test]
    async fn carries_the_work_sessions_task_id() {
        // Without it the spend misses the ticket.
        let r = world(World {
            replies: replies(&["on it"]),
            ..Default::default()
        });
        let mut c = ctx(&r);
        c.caller = "ticket:t-41".into();
        c.model = Some("engineer-engineering".into());
        c.ledger = Some(RunLedger {
            source: Some(LedgerSource::Chat),
            ref_id: Some("t-41".into()),
            task_id: Some("t-41".into()),
        });
        execute(&r.deps(), &titler(), &json!({ "transcript": "x" }), c, None)
            .await
            .unwrap();
        let ledger = req_at(&r, 0).ledger.expect("ledger");
        assert_eq!(ledger.agent_model, "engineer-engineering");
        assert_eq!(ledger.ref_id, Some("t-41".into()));
        assert_eq!(ledger.task_id, Some("t-41".into()));
    }

    // ── Routing a persona TIER ───────────────────────────────────────────────

    fn on_tier(r: &Recorder) -> RunContext {
        let mut c = ctx(r);
        c.caller = "plan:c-1".into();
        c.model = Some("engineer-engineering".into());
        c.tier = Some("opus".into());
        c
    }

    #[tokio::test]
    async fn calls_the_tier_id_and_attributes_the_spend_to_the_base_agent() {
        // `recordUsage` prices a row by finding `agent_defs.model =
        // agentModel` and then the alias named by `tier`. Hand it the routed
        // id with a null tier and BOTH lookups miss: the row lands on an
        // agent that does not exist, with no endpoint class, which means no
        // price. A tier draft becomes free.
        let r = world(World {
            replies: replies(&["drafted"]),
            ..Default::default()
        });
        let res = execute(
            &r.deps(),
            &titler(),
            &json!({ "transcript": "x" }),
            on_tier(&r),
            None,
        )
        .await
        .unwrap();

        assert_eq!(req_at(&r, 0).model, "engineer-engineering-opus");
        let ledger = req_at(&r, 0).ledger.expect("ledger");
        assert_eq!(ledger.agent_model, "engineer-engineering");
        assert_eq!(ledger.tier, Some("opus".into()));
        // The RESULT names the id that answered, because that is the model
        // the fitness matrix is scoring.
        assert_eq!(res.model, Some("engineer-engineering-opus".into()));
        assert_eq!(
            run_at(&r, 0).model,
            Some("engineer-engineering-opus".into())
        );
    }

    #[tokio::test]
    async fn asks_the_capability_index_about_the_tier_not_the_agent_main() {
        // "engineer-engineering-opus" is a different, usually larger model
        // than "engineer-engineering"; crediting main's probe to the tier
        // would widen on a fact about something else.
        let asked: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let r = world(World {
            replies: replies(&["drafted"]),
            empty_routing: true,
            persona_asks: Some(asked.clone()),
            ..Default::default()
        });
        execute(
            &r.deps(),
            &titler(),
            &json!({ "transcript": "x" }),
            on_tier(&r),
            None,
        )
        .await
        .unwrap();
        assert_eq!(
            *asked.lock().expect("asks"),
            vec!["engineer-engineering-opus".to_string()]
        );
    }

    // ── A caller that ran the chain itself ───────────────────────────────────
    //
    // `routes/api/muse.ts` must know the model BEFORE it opens the stream —
    // `x-muse-model` is a header — so it resolves the chain and hands the
    // answer over. `ctx.step` is what stops that from silently costing the
    // fitness page its `chain_step` column.

    #[tokio::test]
    async fn keeps_the_step_on_the_run_row() {
        let r = world(World {
            replies: replies(&["drafted"]),
            ..Default::default()
        });
        let mut c = ctx(&r);
        c.caller = "muse:test".into();
        c.model = Some("pl-fast".into());
        c.step = Some("first-routable");
        execute(&r.deps(), &titler(), &json!({ "transcript": "x" }), c, None)
            .await
            .unwrap();
        let row = run_at(&r, 0);
        assert_eq!(row.model, Some("pl-fast".into()));
        assert_eq!(row.step, Some("first-routable"));
    }

    #[tokio::test]
    async fn records_no_step_for_a_caller_that_genuinely_pinned() {
        // The fitness suite names a candidate model; there was no chain, so
        // inventing a step would put a fabricated fallback in the operator's
        // data.
        let r = world(World {
            replies: replies(&["drafted"]),
            ..Default::default()
        });
        let mut c = ctx(&r);
        c.caller = "fitness:test".into();
        c.model = Some("pl-fast".into());
        execute(&r.deps(), &titler(), &json!({ "transcript": "x" }), c, None)
            .await
            .unwrap();
        let row = run_at(&r, 0);
        assert_eq!(row.model, Some("pl-fast".into()));
        assert_eq!(row.step, None);
    }

    #[tokio::test]
    async fn ignores_a_step_offered_without_a_model() {
        let r = world(World {
            replies: replies(&["drafted"]),
            ..Default::default()
        });
        let mut c = ctx(&r);
        c.caller = "muse:test".into();
        c.step = Some("first-routable");
        execute(&r.deps(), &titler(), &json!({ "transcript": "x" }), c, None)
            .await
            .unwrap();
        // `world` resolves to pl-main via 'pin'; the caller's stray step
        // loses.
        let row = run_at(&r, 0);
        assert_eq!(row.model, Some("pl-main".into()));
        assert_eq!(row.step, Some("pin"));
    }

    // ── Streaming ────────────────────────────────────────────────────────────

    /// A transport that pumps deltas and never assembles the text itself —
    /// the honest shape of a route that pipes chunks straight into a
    /// Response.
    fn pump(deltas: Vec<&'static str>) -> StreamFn {
        Arc::new(move |_req, emit: DeltaFn| {
            let deltas = deltas.clone();
            Box::pin(async move {
                for d in &deltas {
                    emit(d);
                }
                Ok(TransportReply {
                    kind: TransportKind::Gateway,
                    text: String::new(),
                    tool_names: Vec::new(),
                    tool_calls: None,
                    usage: None,
                    contract_dropped: false,
                })
            })
        })
    }

    #[tokio::test]
    async fn hands_every_delta_on_and_still_guards_the_whole_reply() {
        let r = world(World::default());
        let chunks: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = chunks.clone();
        let res = execute(
            &r.deps(),
            &titler(),
            &json!({ "transcript": "x" }),
            {
                let mut c = ctx(&r);
                c.caller = "muse".into();
                c
            },
            Some(StreamOptions {
                stream: pump(vec!["Migrating ", "the ledger ", "to Postgres"]),
                on_delta: Some(Arc::new(move |d: &str| {
                    sink.lock().expect("chunks").push(d.to_string());
                })),
            }),
        )
        .await
        .unwrap();

        assert_eq!(
            *chunks.lock().expect("chunks"),
            vec!["Migrating ", "the ledger ", "to Postgres"]
        );
        // The reply resolved with `text: ''` — the accumulated deltas ARE the
        // reply, which is what lets a route pump into a browser and assemble
        // nothing.
        assert_eq!(
            res.value,
            Some(Value::String("Migrating the ledger to Postgres".into()))
        );
        assert!(res.schema_valid);
        assert_eq!(n_runs(&r), 1);
    }

    #[tokio::test]
    async fn redacts_the_value_it_returns_though_the_relayed_bytes_are_gone() {
        // The guard cleans "what Talaria persists or hasn't yet relayed". The
        // stream already showed the original; a surface that must scrub what
        // it relays does that in `on_delta`, the only place that can work.
        let r = world(World::default());
        let chunks: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = chunks.clone();
        let mut redacting = titler();
        redacting.guard = Some(GuardDecl {
            rules: None,
            redact: true,
        });
        let res = execute(
            &r.deps(),
            &redacting,
            &json!({ "transcript": "x" }),
            {
                let mut c = ctx(&r);
                c.caller = "muse".into();
                c
            },
            Some(StreamOptions {
                stream: pump(vec!["deployed with key ", "AKIAIOSFODNN7EXAMPLE"]),
                on_delta: Some(Arc::new(move |d: &str| {
                    sink.lock().expect("chunks").push(d.to_string());
                })),
            }),
        )
        .await
        .unwrap();

        assert!(checks(&res).contains(&"secret_leak"));
        assert_eq!(
            res.value,
            Some(Value::String(
                "deployed with key [redacted AWS access key]".into()
            ))
        );
        assert!(
            chunks
                .lock()
                .expect("chunks")
                .join("")
                .contains("AKIAIOSFODNN7EXAMPLE")
        );
    }

    #[tokio::test]
    async fn never_repairs_the_first_answer_already_reached_the_screen() {
        // Repairing would stream one document and hand back another. The
        // contract failure is recorded honestly instead, which is the number
        // the fitness page needs about a model on a streaming surface.
        let r = world(World::default());
        let calls = Arc::new(Mutex::new(0));
        let counted = calls.clone();
        let res = execute(
            &r.deps(),
            &judge(),
            &ticket("x"),
            {
                let mut c = ctx(&r);
                c.caller = "muse".into();
                c
            },
            Some(StreamOptions {
                stream: Arc::new(move |_req, emit: DeltaFn| {
                    let counted = counted.clone();
                    Box::pin(async move {
                        *counted.lock().expect("calls") += 1;
                        emit("here is my verdict: maybe");
                        Ok(TransportReply {
                            kind: TransportKind::Gateway,
                            text: String::new(),
                            tool_names: Vec::new(),
                            tool_calls: None,
                            usage: None,
                            contract_dropped: false,
                        })
                    })
                }),
                on_delta: None,
            }),
        )
        .await
        .unwrap();

        assert_eq!(*calls.lock().expect("calls"), 1);
        assert_eq!(res.repairs, 0);
        assert_eq!(res.value, None);
        assert!(!run_at(&r, 0).schema_valid);
    }

    #[tokio::test]
    async fn keeps_the_partial_reply_when_the_stream_dies_mid_flight() {
        // `raw` is what the fitness drill-down shows behind a red cell, and
        // the failure an operator most wants to interrogate is exactly this
        // one.
        let r = world(World::default());
        let res = execute(
            &r.deps(),
            &titler(),
            &json!({ "transcript": "x" }),
            {
                let mut c = ctx(&r);
                c.caller = "muse".into();
                c
            },
            Some(StreamOptions {
                stream: Arc::new(|_req, emit: DeltaFn| {
                    Box::pin(async move {
                        emit("Migrating the ledger ");
                        emit("to Postg");
                        Err("socket hang up".to_string())
                    })
                }),
                on_delta: None,
            }),
        )
        .await
        .unwrap();

        assert_eq!(res.value, None);
        assert_eq!(res.raw, Some("Migrating the ledger to Postg".into()));
        assert!(res.error.as_deref().unwrap().contains("socket hang up"));
        assert_eq!(n_runs(&r), 1);
    }

    #[tokio::test]
    async fn streamed_carries_tools_ledger_and_tier_slots_as_blocking_does() {
        let r = world(World::default());
        let seen: Arc<Mutex<Option<TransportRequest>>> = Arc::new(Mutex::new(None));
        let record = seen.clone();
        let mut looping = titler();
        looping.tools = Some(ToolPolicy::Own);
        execute(
            &r.deps(),
            &looping,
            &json!({ "transcript": "x" }),
            {
                let mut c = ctx(&r);
                c.caller = "outreach:check-in".into();
                c.model = Some("assistant-operations".into());
                c.ledger = Some(RunLedger {
                    source: None,
                    ref_id: Some("c-3".into()),
                    task_id: None,
                });
                c
            },
            Some(StreamOptions {
                stream: Arc::new(move |req, emit: DeltaFn| {
                    let record = record.clone();
                    Box::pin(async move {
                        *record.lock().expect("seen") = Some(req);
                        emit("here is what needs you");
                        Ok(TransportReply {
                            kind: TransportKind::Fleet,
                            text: String::new(),
                            tool_names: vec!["get_ticket".into()],
                            tool_calls: None,
                            usage: None,
                            contract_dropped: false,
                        })
                    })
                }),
                on_delta: None,
            }),
        )
        .await
        .unwrap();
        let seen = seen
            .lock()
            .expect("seen")
            .clone()
            .expect("recorded request");
        assert_eq!(seen.model, "assistant-operations");
        assert_eq!(seen.tools, Some(ToolPolicy::Own));
        let ledger = seen.ledger.expect("ledger");
        assert_eq!(ledger.agent_model, "assistant-operations");
        assert_eq!(ledger.ref_id, Some("c-3".into()));
    }

    // ── The grounding hook ───────────────────────────────────────────────────

    const HIT: &str =
        "https://talaria.internal/research/sources/aa11bb22 — the vendor published a SOC 2 Type II";

    fn synthesis_with_ground(ground: Option<GroundFn>) -> HarnessDefinition {
        let mut d = define_harness(HarnessDefinition::new(
            "research-synthesis-test",
            "Synthesis",
            "Writes the report from the findings.",
            spec("titler"),
            Arc::new(|_input: &Value, _ctx: &RenderContext| Ok(vec![Message::user("write it up")])),
            Output::Text {
                clean: None,
                verify: None,
            },
            OnFailure::Null,
        ));
        d.floor = RoleFloor::runs_anyway("Runs on anything.");
        d.guard = Some(GuardDecl {
            rules: Some(vec!["ungrounded_ref", "fabricated_outage"]),
            redact: false,
        });
        d.ground = ground;
        d
    }

    /// A synthesis-shaped harness: its input carries the search hits, which
    /// ARE the tool results for the turn.
    fn synthesis() -> HarnessDefinition {
        synthesis_with_ground(Some(Arc::new(|input: &Value| {
            let notes = input["notes"].as_str().unwrap_or_default().to_string();
            let failed = input["failed"].as_bool();
            Ok(Some(GroundMaterial {
                tools: vec!["research_search".into()],
                results: notes,
                errored: failed,
            }))
        })))
    }

    fn policed() -> World {
        World {
            policed_hosts: vec!["talaria.internal".into()],
            ..Default::default()
        }
    }

    fn synthesis_input(failed: bool) -> Value {
        json!({ "notes": HIT, "failed": failed })
    }

    fn synthesis_world(reply: &str) -> Recorder {
        let mut w = policed();
        w.replies = replies(&[reply]);
        world(w)
    }

    #[tokio::test]
    async fn fires_ungrounded_ref_on_a_link_no_search_result_contained() {
        // The definitive research failure mode — the persona's soul and
        // memory bleeding an internal link into a document a human will
        // trust because it looks cited — and until this hook existed the
        // rule could not run on a single harness in the product.
        let r = synthesis_world("See https://talaria.internal/tickets/9f9f9f9f for the detail.");
        let mut c = ctx(&r);
        c.caller = "research:r1".into();
        let res = execute(&r.deps(), &synthesis(), &synthesis_input(false), c, None)
            .await
            .unwrap();

        assert!(checks(&res).contains(&"ungrounded_ref"));
        assert!(recorded(&r).iter().any(|f| f.check == "ungrounded_ref"));
    }

    #[tokio::test]
    async fn does_not_fire_on_a_link_the_search_results_really_returned() {
        let r = synthesis_world(
            "The vendor's report is at https://talaria.internal/research/sources/aa11bb22 [1].",
        );
        let mut c = ctx(&r);
        c.caller = "research:r1".into();
        let res = execute(&r.deps(), &synthesis(), &synthesis_input(false), c, None)
            .await
            .unwrap();
        assert!(res.findings.is_empty());
    }

    #[tokio::test]
    async fn grounds_a_persona_turn_too() {
        // The synthesis stage runs on the requesting agent's own persona,
        // whose stream reports tool names only. The search hits are not that
        // persona's tools — they are the pipeline's — so `ground` overrides
        // the fleet branch rather than deferring to it.
        let fleet = Reply::Full(TransportReply {
            kind: TransportKind::Fleet,
            text: "See https://talaria.internal/tickets/9f9f9f9f.".into(),
            tool_names: Vec::new(),
            tool_calls: None,
            usage: None,
            contract_dropped: false,
        });
        let mut w = policed();
        w.replies = vec![fleet];
        let r = world(w);
        let mut c = ctx(&r);
        c.caller = "research:r1".into();
        let res = execute(&r.deps(), &synthesis(), &synthesis_input(false), c, None)
            .await
            .unwrap();
        assert!(checks(&res).contains(&"ungrounded_ref"));
    }

    #[tokio::test]
    async fn lets_a_real_stage_failure_ground_an_outage_claim() {
        let claim = "One angle could not be researched: the search endpoint was unreachable.";
        let honest_r = synthesis_world(claim);
        let mut honest_c = ctx(&honest_r);
        honest_c.caller = "research:r1".into();
        let honest = execute(
            &honest_r.deps(),
            &synthesis(),
            &synthesis_input(true),
            honest_c,
            None,
        )
        .await
        .unwrap();
        assert!(!checks(&honest).contains(&"fabricated_outage"));

        let invented_r = synthesis_world(claim);
        let mut invented_c = ctx(&invented_r);
        invented_c.caller = "research:r1".into();
        let invented = execute(
            &invented_r.deps(),
            &synthesis(),
            &synthesis_input(false),
            invented_c,
            None,
        )
        .await
        .unwrap();
        assert!(checks(&invented).contains(&"fabricated_outage"));
    }

    #[tokio::test]
    async fn skips_fabricated_outage_when_the_harness_cannot_say_whether_anything_errored() {
        // `errored: null` is the difference between a rule that is quiet and
        // a rule that is wrong. Claiming "nothing errored" on material that
        // does not record errors would flag every honest report of a failure.
        let cagey = synthesis_with_ground(Some(Arc::new(|input: &Value| {
            let notes = input["notes"].as_str().unwrap_or_default().to_string();
            Ok(Some(GroundMaterial {
                tools: vec!["research_search".into()],
                results: notes,
                errored: None,
            }))
        })));
        let r = synthesis_world(
            "One angle could not be researched: the search endpoint was unreachable.",
        );
        let mut c = ctx(&r);
        c.caller = "research:r1".into();
        let res = execute(&r.deps(), &cagey, &synthesis_input(false), c, None)
            .await
            .unwrap();
        assert!(!checks(&res).contains(&"fabricated_outage"));
        // …and the grounding it CAN supply still works.
        assert!(!checks(&res).contains(&"ungrounded_ref"));
    }

    // The hook must make honesty expressible, not optimism the default.

    #[tokio::test]
    async fn a_harness_with_no_ground_still_skips_ungrounded_ref() {
        let r = synthesis_world("See https://talaria.internal/tickets/9f9f9f9f.");
        let mut c = ctx(&r);
        c.caller = "research:r1".into();
        let res = execute(
            &r.deps(),
            &synthesis_with_ground(None),
            &synthesis_input(false),
            c,
            None,
        )
        .await
        .unwrap();
        assert!(res.findings.is_empty());
    }

    #[tokio::test]
    async fn a_hollow_ground_is_treated_as_no_hook_at_all() {
        // An empty tool list is not grounding. `ungrounded_ref` already
        // declines on one, but `fabricated_outage` does not — so accepting
        // it would let a harness with no material assert `errorInfo: true`
        // and start flagging outage reports it cannot check.
        let hollow = synthesis_with_ground(Some(Arc::new(|_input: &Value| {
            Ok(Some(GroundMaterial {
                tools: Vec::new(),
                results: String::new(),
                errored: Some(false),
            }))
        })));
        let fleet = Reply::Full(TransportReply {
            kind: TransportKind::Fleet,
            text: "The MCP server is unreachable.".into(),
            tool_names: Vec::new(),
            tool_calls: None,
            usage: None,
            contract_dropped: false,
        });
        let mut w = policed();
        w.replies = vec![fleet];
        let r = world(w);
        let mut c = ctx(&r);
        c.caller = "research:r1".into();
        let res = execute(
            &r.deps(),
            &hollow,
            &json!({ "notes": "", "failed": false }),
            c,
            None,
        )
        .await
        .unwrap();
        assert!(res.findings.is_empty());
    }

    #[tokio::test]
    async fn a_ground_that_throws_is_a_missing_record_not_an_exception() {
        let boom = synthesis_with_ground(Some(Arc::new(|_input: &Value| {
            Err("Cannot read properties of undefined (reading 'sources')".to_string())
        })));
        let r = synthesis_world("See https://talaria.internal/tickets/9f9f9f9f.");
        let mut c = ctx(&r);
        c.caller = "research:r1".into();
        let res = execute(&r.deps(), &boom, &synthesis_input(false), c, None)
            .await
            .unwrap();
        assert_eq!(
            res.value,
            Some(Value::String(
                "See https://talaria.internal/tickets/9f9f9f9f.".into()
            ))
        );
        assert!(res.findings.is_empty());
    }

    #[tokio::test]
    async fn fails_open_on_grounding_too_large_to_scan() {
        // guardrails's own choice for an overflowing tool record, restated
        // for a declared one rather than re-decided.
        let huge = synthesis_with_ground(Some(Arc::new(|_input: &Value| {
            Ok(Some(GroundMaterial {
                tools: vec!["research_search".into()],
                results: "x".repeat(200_001),
                errored: Some(false),
            }))
        })));
        let r = synthesis_world("See https://talaria.internal/tickets/9f9f9f9f.");
        let mut c = ctx(&r);
        c.caller = "research:r1".into();
        let res = execute(
            &r.deps(),
            &huge,
            &json!({ "notes": "", "failed": false }),
            c,
            None,
        )
        .await
        .unwrap();
        assert!(!checks(&res).contains(&"ungrounded_ref"));
    }
}
