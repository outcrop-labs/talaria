// THE harness contract. A harness DECLARES what it needs; it never chooses a
// transport, a model, a parser, or a failure policy. The runner (`run.rs`)
// honors the declaration, and it is the only code that talks to a model.
//
// WHY THIS FILE EXISTS
//   `PLATFORM_AGENTS` in platform_agents.rs names the harnesses and describes
//   each one's job — and carries none of the things that make a harness a
//   harness. Before this interface, the prompt, the output shape, the fallback
//   chain, the failure behavior and the guard pass were hand-written per call
//   site, free to drift from that prose. This interface is the other half of
//   that registry — the executable half.
//
//   The cost of the copies is not aesthetic. Each one answers "the model
//   returned something I could not use" DIFFERENTLY and SILENTLY: the judge
//   escalates to a human (so a weak judge model is a notification storm), the
//   blurb writer returns 0 and re-burns the same batch every ten minutes
//   forever, Muse returns null so the button just does nothing. `on_failure`
//   below is that decision, stated once, per harness, in public.
//
// PURE BY CONSTRUCTION. Types, type-erased closures, and the one derived
// floor. No database, no gateway, no settings — a harness definition (and the
// eval suite that will enumerate every one of them) exists without booting
// Talaria.
//
// THE ERASURE, and why it follows the runs engine's precedent rather than
// fighting for generics. Rust has no existential the registry could hold
// across every def without making each a monomorphized instantiation the
// registry cannot name — so, exactly as `runs/define.rs` erases a run's
// typed input into a `Value` column, a harness def erases I/O at the
// definition site: `render` takes `(&Value, &RenderContext)` and the def's
// own closure deserializes its typed input internally, and `verify`/`ground`
// do the same. The contract generics would enforce — that `render`'s input
// and `schema`'s subject are the same type — is the closure's own
// `serde_json::from_value`, whose failure reads as a failed contract.

use std::sync::Arc;

use super::schema::Schema;
use super::transport::{ToolCall, ToolDefinition, ToolPolicy};
use crate::harness_model::ModelSpec;
use serde_json::Value;

// ── The message ──────────────────────────────────────────────────────────────

/// One chat role, as every transport in Talaria spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

impl Role {
    pub fn as_str(self) -> &'static str {
        match self {
            Role::System => "system",
            Role::User => "user",
            Role::Assistant => "assistant",
            Role::Tool => "tool",
        }
    }

    pub fn is_tool(self) -> bool {
        self == Role::Tool
    }
}

/// One chat turn. Deliberately text-only: a harness that needs image parts is
/// a different contract, and inventing the slot before something needs it is
/// how the union rots.
///
/// RE-DECIDED, NOT INHERITED. The `vision` probe wants image content and is
/// the first caller ever to want any, so the question was live: widen
/// `content` to the OpenAI content-parts union, or leave vision unmeasurable.
/// It stays a string, because a HALF-WIDENED union is worse than none and
/// half is all that is reachable from here — every consumer of a message
/// list reads `content` as one string: the guard pass's grounding text and
/// tool-record extraction, the runner's last-user-message and anchor-JSON
/// reads, token estimation on both metering paths, and every harness
/// `render`. A union only one payload path honored would report
/// `[object Object]` into the ledger and ground the guard against nothing.
///
/// So `content` stays a string, and `vision` is measured through a seam that
/// does not need it (the image/probe turns build their own multimodal body).
/// What DID land here is the TOOL CHANNEL below, which is a different
/// question: two optional fields, no change to `content`, and every reader
/// listed above keeps working untouched.
#[derive(Debug, Clone)]
pub struct Message {
    pub role: Role,
    pub content: String,
    /// THE TOOL CHANNEL, and it is additive on purpose.
    ///
    /// WHY IT HAD TO EXIST. The dry-run sandbox had nowhere to put a tool
    /// call, so it wrote one into the assistant's TEXT: first
    /// `[tool] write_file({...})`, then `(called write_file)`. Models imitated
    /// whichever string they were shown — 34 replies in one sweep contained
    /// our own narration verbatim, then the arguments as prose — so
    /// `reply.tool_calls` came back empty, the loop broke, and fixtures
    /// reported "read the repository and never wrote a file" about models
    /// that had written it. Changing the wording moved the imitation; only
    /// giving the calls their own channel ends it.
    ///
    /// Renderers never set either: a harness `render` produces system and
    /// user turns. Transports that cannot express them fall back to
    /// `content`.
    pub tool_calls: Vec<ToolCall>,
    /// Set with `Role::Tool` — which call this message is the result of.
    pub tool_call_id: Option<String>,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Message {
        Message {
            role: Role::System,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn user(content: impl Into<String>) -> Message {
        Message {
            role: Role::User,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    pub fn assistant(content: impl Into<String>) -> Message {
        Message {
            role: Role::Assistant,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: None,
        }
    }

    /// A tool result answering call `id` — the other half of the tool channel.
    pub fn tool_result(content: impl Into<String>, id: impl Into<String>) -> Message {
        Message {
            role: Role::Tool,
            content: content.into(),
            tool_calls: Vec::new(),
            tool_call_id: Some(id.into()),
        }
    }

    /// An assistant turn that CALLED something — the half the loop replays.
    pub fn assistant_calls(calls: Vec<ToolCall>) -> Message {
        Message {
            role: Role::Assistant,
            content: String::new(),
            tool_calls: calls,
            tool_call_id: None,
        }
    }
}

// ── The render side ──────────────────────────────────────────────────────────

/// What `render` is told about the call it is rendering for.
///
/// `widened` is the capability-gated superpower switch, and it is how "decent
/// on a 14B local model, excellent on a frontier one" is EXPRESSED rather than
/// hoped for. A model that has earned the widen capabilities gets the richer
/// prompt or the fuller action list; one that has not gets the deterministic
/// surface. Both branches must be real answers — `widened: false` is the
/// product working, not a degraded mode with an apology in it.
///
/// `model` is there so a render can name the model in its own prompt (some
/// small models follow instructions noticeably better when addressed) and so a
/// harness can size its context to what it is talking to. It is NOT an
/// invitation to branch on model ids: that is what capabilities are for.
#[derive(Debug, Clone)]
pub struct RenderContext {
    pub widened: bool,
    pub model: String,
}

/// The per-ROLE minimum a harness declares.
///
/// DESIGN DECISION, LOCKED: the floor is per role, not global. Talaria must be
/// decent on a 7-14B self-hosted model and excellent on a large one, and those
/// two sentences are only compatible if each job says for ITSELF what it
/// cannot do without. The titler, the summarizer and the librarian declare
/// almost nothing — they have to work on whatever the self-host has, and a
/// titler that refuses to name a chat is worse than a mediocre title. The
/// judge, research and code harnesses declare real capabilities and REFUSE
/// below them, because a judge that silently degrades is a judge whose verdicts
/// are noise, and noise is worse than an honest "this model cannot do this
/// job".
#[derive(Debug, Clone)]
pub struct RoleFloor {
    /// THE REFUSAL LIST, and it is only ever read when `refuse_below` is true
    /// — the runner intersects the known-missing capabilities with this array
    /// and then does nothing with the result unless the harness refuses. So a
    /// floor that declares capabilities WITHOUT refusing is an inert
    /// declaration that reads to the next author as a hard requirement — and
    /// "needs JSON, runs anyway" spelled with or without the list runs
    /// identically. Keep this EMPTY unless `refuse_below` is true, and say
    /// what the job leans on in `requires` — which is what the fitness
    /// matrix scores, and which never blocks. The registry's tests enforce
    /// the pairing.
    pub capabilities: Vec<&'static str>,
    /// True: refuse the run and say which capability is missing. False: run
    /// anyway and let the result carry the fact. Never silently half-work.
    pub refuse_below: bool,
    /// CAPABILITIES THE PLATFORM MAY SUPPLY, so the floor asks whether the RUN
    /// can reach them rather than whether the MODEL has them.
    ///
    /// THE DISTINCTION IS THE WHOLE POINT. A slot an admin assigns is not a
    /// bare model — it is a model running inside Talaria, with the tools this
    /// org has registered and a gateway that can hand it definitions.
    /// `research-search` refusing every model without native browsing was
    /// correct about the weights and wrong about the deployment: a model
    /// measured at 100% tool calling and 100% tool selection, with a
    /// web-search server registered, does the job.
    ///
    /// Listing a capability here does NOT weaken the floor. It redirects it:
    /// the reach check still has to find a registered, enabled tool AND a
    /// model that can call it, and an org with neither gets the same refusal
    /// with a better sentence. What it stops is refusing on a fact that was
    /// true about the model and irrelevant to the run.
    ///
    /// A harness that lists a capability here MUST have a code path that
    /// actually uses the tool — see `research-search`, which picks its
    /// transport on the answer. Declaring it without one would turn a refusal
    /// into a silently worse run, which is the failure this floor was built to
    /// prevent.
    pub suppliable: Vec<&'static str>,
    /// One sentence, shown next to the model picker in Admin. Written for the
    /// admin choosing a model, not for the developer reading this file.
    pub note: &'static str,
}

impl RoleFloor {
    /// The floor every non-refusing harness declares: nothing, said plainly.
    pub fn runs_anyway(note: &'static str) -> RoleFloor {
        RoleFloor {
            capabilities: Vec::new(),
            refuse_below: false,
            suppliable: Vec::new(),
            note,
        }
    }

    /// The floor a refusing harness declares: named capabilities, acted on.
    pub fn refuses(capabilities: Vec<&'static str>, note: &'static str) -> RoleFloor {
        RoleFloor {
            capabilities,
            refuse_below: true,
            suppliable: Vec::new(),
            note,
        }
    }
}

// ── The erasure points ───────────────────────────────────────────────────────

/// Input → messages. THE ONLY THING A HARNESS AUTHOR WRITES BY HAND. Takes the
/// type-erased input (see the module header); the closure deserializes its own
/// type and a decode failure is a failed contract.
pub type RenderFn =
    Arc<dyn Fn(&Value, &RenderContext) -> Result<Vec<Message>, String> + Send + Sync>;

/// A text harness's narrowing step: receives the raw reply, returns the value,
/// or `None` to fail the contract — nothing usable, which every caller reads
/// as "keep what you had".
///
/// The value is a `Value`, not a string, because a text harness may parse a
/// HYBRID — the librarian's reply is a markdown body plus a trailing `TAGS:`
/// line, and its clean consumes the line and returns `{body, tags}` — and the
/// runner stores the result as the run's value. That is also why redaction
/// re-applies the WHOLE contract to the scrubbed text: a redacted hybrid is
/// rebuilt by the same parse, not handed back half-scrubbed.
pub type CleanFn = Arc<dyn Fn(&str) -> Result<Option<Value>, String> + Send + Sync>;

/// PreFn — the def's own restructure of a PARSED reply before the schema sees
/// it. One job in the tree needs this and it is not decoration: when a
/// provider is asked for JSON at the protocol level,
/// `response_format: {"type":"json_object"}` obliges some models to emit a
/// top-level OBJECT, which makes an envelope (`{"tickets": [...]}`) the only
/// shape a correct answer can arrive in for an array-shaped contract. The
/// harness unwraps it here rather than spending a repair turn telling the
/// model to stop doing what its provider's strict mode compels.
///
/// Runs per candidate span, after parse and before validation. Pure and
/// cheap — it is on the same hot path as the parser.
pub type PreFn = Arc<dyn Fn(&Value) -> Value + Send + Sync>;

/// THE RELATION BETWEEN THE INPUT AND THE OUTPUT — the half of a harness
/// contract a schema is structurally incapable of stating.
///
/// WHY THIS EXISTS. A schema is a module constant. It is built once, at
/// registration time, and it cannot see the run's input, so every harness
/// whose correctness is a RELATION between what was asked and what came back
/// had no way to say so — and the runner recorded `schema_valid: true` for a
/// value the caller then threw away. Four shipped bugs were that one defect in
/// different clothes:
///
///   blurb-writer  a string→string record cannot constrain the KEYS. A model
///                 that tidied `qwen3-14b` into `Qwen3 14B` passed the schema,
///                 wrote zero blurbs, and reported a 100% contract rate — then
///                 came back around on the identical batch every ten minutes
///                 forever.
///   channel-plan  the elements must be TICKETS from the transcript, not
///                 titles the model invented and not a bracketed citation
///                 marker.
///   muse:ticket   a date must be one the WRITE PATH accepts (`string` here
///                 against `datetime` on the route), so the repair turn could
///                 never fire on the likeliest small-model mistake.
///   redaction     a value that still parses after being cut in half.
///
/// THE OFFLINE SUITE ALREADY KNEW. A fixture's `check` is this same
/// assertion, and blurb-writer's own fixture rejects invented ids — so the
/// eval fixtures and the `harness_runs.schema_valid` column DISAGREED, and the
/// production one was the optimistic liar. `schema_valid` is the OBSERVED half
/// of the model-fitness matrix; a column that says a model held a contract it
/// did not hold makes the whole matrix worth less than nothing.
///
/// WHAT IT IS. Runs AFTER schema validation, only ever on a parsed value.
/// Returns `Ok(None)` when the value is usable, or ONE PLAIN SENTENCE naming
/// the problem. That sentence is fed straight back to the model as a repair
/// instruction, exactly like the parser's error, so write it as an instruction
/// to the model and not as a note to a developer: "the keys must be the model
/// ids exactly as given - 'Qwen3 14B' is not one of them" repairs; "invariant
/// violated in blurbWriter" does not.
///
/// A VERIFY FAILURE IS A CONTRACT FAILURE, in every sense the runner has: it
/// repairs on the same loop against the same counter, it sets
/// `schema_valid: false`, and it lands on the `harness_runs` row honestly.
///
/// `Err` IS A FAILED CONTRACT — harness-author code meeting model output; an
/// `Err` out of any of `render`/`clean`/`verify`/`ground` fails the contract
/// rather than escaping a runner whose whole promise is that a bad model
/// produces a RESULT.
///
/// IT IS TOLD WHAT `render` WAS TOLD. The context argument is the SAME
/// `RenderContext` the prompt was built from, and it exists because the
/// widened surface changes the contract rather than only the wording:
/// `inbox-command` offers a probed model the item's whole action list and a
/// regex-bound one a single id, so "did it stay inside what it was offered" is
/// unanswerable from `(value, input)` alone. Without it that harness — the one
/// carrying the product's safety assertion — had to leave its own eval's check
/// to a caller, and recorded `schema_valid: true` for a proposal that caller
/// dropped.
///
/// KEEP IT CHEAP AND PURE. It runs on every attempt of every run, including
/// the redaction re-check, and this module imports no database by
/// construction. A verify that needs to ask the database whether an id exists
/// is a check for the caller, not for the contract.
pub type VerifyFn =
    Arc<dyn Fn(&Value, &Value, &RenderContext) -> Result<Option<String>, String> + Send + Sync>;

/// THE GROUNDING MATERIAL for one turn — the tool record a harness can
/// honestly supply from its OWN input, which no transport is in a position to
/// derive.
///
/// WHY THIS EXISTS: `ungrounded_ref` ("cites link(s)/id(s) that did not appear
/// in any tool result this turn — may be fabricated") is the single
/// highest-value rule in the guard and it COULD NOT FIRE FROM ANY HARNESS, by
/// construction. The rule returns nothing when `backing_tools` is empty or the
/// results overflowed, and the runner derives its record from the messages IT
/// sent — which for a harness turn contain no tool messages at all — or, on
/// the fleet path, marks the record overflowed because a persona's tool loop
/// ran inside the agent. So the rule self-skipped on every harness, and the
/// one path in the product whose defining failure mode is a fabricated
/// citation had to run it OUTSIDE the runner over a record it built by hand.
///
/// A harness that HAS the material is a harness whose input already contains
/// it: research's synthesis stage is handed the search hits and the numbered
/// source registry, which ARE the tool results for that turn. This hook is how
/// it says so, and the runner then supplies an honest `Available` instead of a
/// cautious one.
///
/// HONESTY IS EXPRESSIBLE HERE; OPTIMISM IS NOT THE DEFAULT. A harness that
/// declares no `ground`, or whose `ground` returns `None` or an empty `tools`
/// list, keeps exactly the `Available` its transport earned — the rules SKIP
/// rather than run on material nobody has. Claiming a grounded turn with no
/// backing tool would turn "we cannot check this" into "we checked and it is
/// fine", which is the one direction a guard must never move.
#[derive(Debug, Clone)]
pub struct Grounding {
    /// The backing tools that GENUINELY ran for this turn, named as the tool
    /// registry names them (research's search stages are `research_search`). An
    /// EMPTY list is not grounding: the runner treats it as an absent hook and
    /// falls back to the transport's own record.
    pub tools: Vec<String>,
    /// Everything those tools returned, concatenated. Supply MORE than the
    /// prompt carried where you have it — grounding a citation against more
    /// than the model saw can only remove false positives, never add one.
    pub results: String,
    /// Did any of those tools return a transport/availability error?
    ///
    /// `None` means the harness genuinely cannot say, which SKIPS
    /// `fabricated_outage` rather than asserting "nothing errored" — the
    /// difference between a rule that is quiet and a rule that is wrong.
    pub errored: Option<bool>,
}

/// `Ok(None)` is the absent hook.
pub type GroundFn = Arc<dyn Fn(&Value) -> Result<Option<Grounding>, String> + Send + Sync>;

// ── The output contract and the failure policy ───────────────────────────────

/// The output contract. `Json` puts the runner in structured mode: it asks for
/// JSON at the protocol level when the model can honor that, parses with the
/// one balanced-brace extractor, and repairs on a malformed reply.
///
/// For `Text` with no `clean`, the value is the reply by construction. `clean`
/// is where a text harness narrows, and `verify` is the OTHER half of the
/// contract and the half neither a schema nor a `clean` can express, because
/// both are written before the input exists — see `VerifyFn` above for the
/// four bugs that were all this one gap. Both output kinds carry it: a text
/// harness's "did it answer the question I asked" is the same question as a
/// JSON harness's "are these the ids I sent".
pub enum Output {
    Text {
        clean: Option<CleanFn>,
        verify: Option<VerifyFn>,
    },
    Json {
        schema: Schema,
        /// See `PreFn` — the envelope unwrap that runs between parse and
        /// validation. `None` for every def whose answer is not packaged.
        preprocess: Option<PreFn>,
        /// Repair turns the runner may spend on this harness. `None` is the
        /// runner's default (one), spelled as a default so the def that needs
        /// none can say `Some(0)` deliberately.
        repair: Option<u32>,
        verify: Option<VerifyFn>,
    },
}

impl Output {
    pub fn is_json(&self) -> bool {
        matches!(self, Output::Json { .. })
    }
}

/// THE FALLBACK VALUE, when a harness declares one — type-erased to the two
/// shapes a harness result can be. The registry's registration check pairs the
/// variant with the output kind: a text harness falls back to `Text`, a JSON
/// harness to `Json`. The pairing matters because the caller reads the
/// fallback through the same slot as a real answer, and a JSON caller handed a
/// bare string would fail one `as_str()` later for a reason that had nothing
/// to do with the model.
#[derive(Debug, Clone)]
pub enum Fallback {
    Text(String),
    Json(Value),
}

/// What a failure MEANS here. Stated per harness because before this existed
/// each site answered it differently and in silence:
///
///   `Null`      the caller keeps what it had (titler, summarizer)
///   `Throw`     the caller must handle it (a request-path harness)
///   `Fallback`  a declared safe value (a default verdict, an empty list)
///   `Escalate`  a human decides — the runner sets `escalate` on the result
///              and the caller raises it, because only the caller knows who to
///              tell (the judge's `tellHumansTheGateStopped`). A FLAG, not a
///              phrase in the error string: a caller that has to string-match
///              to find out is a caller that stops escalating the day somebody
///              rewords the message.
///
/// `Throw` MEANS ANY FAILURE TO PRODUCE A VALUE. The runner RETURNS for
/// everything that happens before or during the call — nothing in the chain
/// routes, the floor refuses, `render` fails, the transport dies — so the
/// policy needs no restating by hand at any call site. Getting that wrong is
/// how research synthesis once saved an empty report, marked the run `done`,
/// indexed it and notified the requester after a 502, and how the channel
/// planner answered "nothing to plan yet" on a channel full of work because
/// its agent container was restarting. It throws on all of them.
///
/// THE OTHER THREE STAY CONTRACT-SCOPED, and that asymmetry is deliberate
/// rather than left over. They describe what a caller GETS when the model
/// answered and the answer was unusable — a question that does not arise when
/// no model was reached — and widening them would break both callers that use
/// them. `Fallback` on a pre-call failure would hand outreach its "nothing to
/// surface" token during a gateway outage, so a dead provider would read as a
/// normal quiet pass on every sweep. `Escalate` on a pre-call failure would
/// have the judge notify every board editor about every ticket for as long as
/// the gateway is down — exactly the notification storm the policy exists to
/// avoid. A
/// caller that wants either of those on an unreachable model has the result's
/// `answered` flag to say so explicitly, which is a sentence somebody wrote on
/// purpose rather than a policy that widened under them.
#[derive(Debug, Clone)]
pub enum OnFailure {
    Null,
    Throw,
    Fallback(Fallback),
    Escalate,
}

/// The capability-gated widening. Set it and `render` is called with
/// `widened: true` only when EVERY capability here is `value: true` with
/// `source: 'probe'` for the resolved model — Talaria's own measurement, not a
/// vendor's claim. Unknown does not widen, and neither does `declared` or
/// `learned`: widening is the direction that hands a model more authority, so
/// it is the direction that demands evidence. (The floor is deliberately laxer
/// about provenance, and the asymmetry is not an inconsistency — see the
/// runner.)
#[derive(Debug, Clone)]
pub struct Widen {
    pub requires: Vec<&'static str>,
    pub note: &'static str,
}

/// Guardrails. `rules` narrows the registry to the ids that make sense for
/// this output (a titler cannot make a zero-tool claim); `None` means every
/// enabled rule. `redact` makes the runner strip credentials and PII out of
/// the VALUE it returns, for harnesses whose output is persisted.
#[derive(Debug, Clone)]
pub struct GuardDecl {
    pub rules: Option<Vec<&'static str>>,
    pub redact: bool,
}

// ── The definition ───────────────────────────────────────────────────────────

/// One harness, declared. The type-erased form the registry holds — see the
/// module header for why the generics are gone and what took their place.
pub struct HarnessDefinition {
    pub id: &'static str,
    pub label: &'static str,
    /// One line, shown in Admin. Today's `PLATFORM_AGENTS[].job`.
    pub job: &'static str,

    /// What the model must be able to DO. The fitness suite scores against
    /// this, and the runner consults it before the call instead of discovering
    /// the answer from a 400 halfway through. Unknown is not missing — an
    /// untested model still runs.
    pub requires: Vec<&'static str>,

    /// The floor for THIS role, and what happens below it.
    pub floor: RoleFloor,

    /// Model resolution, declared not written.
    pub model: ModelSpec<'static>,

    /// Input → messages. THE ONLY THING A HARNESS AUTHOR WRITES BY HAND.
    pub render: RenderFn,

    /// The output contract. `Json` puts the runner in structured mode.
    pub output: Output,

    /// What a failure MEANS here — see `OnFailure`.
    pub on_failure: OnFailure,

    /// The capability-gated widening.
    pub widen: Option<Widen>,

    /// Guardrails — `None` means the default pass with every enabled rule and
    /// no redaction, which is what a harness whose output is never persisted
    /// wants.
    pub guard: Option<GuardDecl>,

    /// The turn's real tool record, from the harness's own input. Declare it
    /// and the runner guards with genuine `backing_tools`, so `ungrounded_ref`
    /// and `fabricated_outage` can actually fire. Omit it — which every
    /// harness that has no tool results should — and nothing changes.
    pub ground: Option<GroundFn>,

    pub temperature: Option<f64>,

    /// May the model use ITS OWN tools on this turn? See the note on
    /// `ToolPolicy` in transport.rs — this is the model's loop, a different
    /// question from `tool_defs`.
    pub tools: Option<ToolPolicy>,

    /// Tools this harness OFFERS the model on the turn — a different question
    /// from `tools` above, which is about the model's own loop. Declared here
    /// rather than assembled by a caller for the same reason the prompt is:
    /// the runner puts it on the request, the transport that cannot serve it
    /// refuses the call, and `harness_runs` records a turn that says what it
    /// really was.
    ///
    /// Today its only declarers are the `tools` and `tool-select` probes,
    /// which is the point: offering four disjoint tools and reading back which
    /// one the model called is the only honest way to measure the capability
    /// that widens the Inbox command harness. A harness that
    /// offers tools must be pinned to a GATEWAY model — a fleet persona's
    /// tool loop belongs to the agent, and the fleet transport refuses rather
    /// than pretending otherwise.
    pub tool_defs: Vec<ToolDefinition>,

    /// How long a persona transport may HOLD for an agent that is not
    /// answering yet, in ms. The proxy waits two minutes by default; an agent
    /// restarting under a config propagation refuses connections for tens of
    /// seconds, and a work session must survive a fleet re-render mid-session.
    /// Meaningless on the gateway path.
    pub hold_ms: Option<u64>,

    /// Fixtures the model-fitness suite replays. An unscored harness is an
    /// invisible one — the registry's test holds every def to at least one.
    pub evals: Vec<EvalCase>,

    /// What a replay of those fixtures runs against, when the def's feature is
    /// the tool loop. See `DryRunDecl`.
    pub dry_run: Option<DryRunDecl>,
}

impl HarnessDefinition {
    /// Identity, plus the one derived floor — see `define_harness`, which is
    /// the spelling every def should go through.
    pub fn new(
        id: &'static str,
        label: &'static str,
        job: &'static str,
        model: ModelSpec<'static>,
        render: RenderFn,
        output: Output,
        on_failure: OnFailure,
    ) -> HarnessDefinition {
        HarnessDefinition {
            id,
            label,
            job,
            requires: Vec::new(),
            floor: RoleFloor::runs_anyway(""),
            model,
            render,
            output,
            on_failure,
            widen: None,
            guard: None,
            ground: None,
            temperature: None,
            tools: None,
            tool_defs: Vec::new(),
            hold_ms: None,
            evals: Vec::new(),
            dry_run: None,
        }
    }
}

/// A JSON HARNESS REQUIRES JSON, DERIVED RATHER THAN DECLARED.
///
/// A harness with a JSON output needs structured output by construction — the
/// platform sends its schema at the protocol level and parses the reply
/// against it. That makes it a FLOOR: a model measured unable to produce
/// structured output is unfit for the task, and asking it anyway means handing
/// prose to a parser and recording the wreckage as the model's failure.
///
/// DERIVED because the rule is mechanical and the alternative is nine copies
/// of it that drift. Restating a floor per harness is how one of them comes to
/// omit it and quietly go back to the old behaviour. The registry's tests
/// assert every JSON harness carries it, so the derivation cannot be silently
/// bypassed.
///
/// WHAT "UNFIT" MEANS HERE IS NARROW, and deliberately so. The runner refuses
/// only on a capability MEASURED false by a probe or declared false by a human
/// — never on an unknown, never on a single upstream 400, never on a catalog
/// spec sheet. A fresh self-host that has probed nothing still runs every JSON
/// harness. The floor bites exactly when Talaria has real evidence the model
/// cannot do the thing the task is made of.
///
/// `refuse_below` is forced true for the same reason: a floor that names the
/// capability but declines to act on it is a comment, not a floor. An author
/// can still add capabilities and their own note; neither is overwritten.
pub fn define_harness(mut h: HarnessDefinition) -> HarnessDefinition {
    if h.output.is_json() && !h.floor.capabilities.contains(&"json") {
        h.floor.capabilities.push("json");
        h.floor.refuse_below = true;
    }
    h
}

// ── The fixture floor helpers ────────────────────────────────────────────────
//
// The pure exports the fitness fixtures lean on.

/// WHAT A FIXTURE'S `check` MAY CONCLUDE.
///
/// `Pass`      the model did the job.
/// `Fail(..)`  the model did not, and this sentence says how.
/// `Gap(..)`   THE HARNESS DID NOT GIVE THE MODEL WHAT THE JOB NEEDED, so the
///             answer cannot be scored. NOT a failure, and never attributed to
///             the model.
///
/// THE THIRD CASE IS THE ONE WORTH EXPLAINING. A fixture asserts that a coding
/// run ran the tests, or that a session filed a capability gap, or that a
/// brief named the one blocked item — and every one of those is only a fair
/// question if the run was actually given a test runner, a gap tool, and a
/// briefing that contained the item. When it was not, the model can do
/// everything right and still miss the assertion, and scoring that as a model
/// failure is measuring our own fixture and calling it a capability.
///
/// A gap is reported to US: it lands in the run's own list of things to fix,
/// not in the model's score. Return one when the ASSERTION IS UNANSWERABLE as
/// posed. Do not return one for a hard task — difficulty is what a band is
/// for.
#[derive(Debug, Clone, PartialEq)]
pub enum CheckResult {
    Pass,
    Fail(String),
    Gap(String),
}

/// Narrowing helper, so consumers never test the shape by hand.
pub fn is_gap(r: &CheckResult) -> Option<&str> {
    match r {
        CheckResult::Gap(s) => Some(s),
        _ => None,
    }
}

/// EASY is the floor a model must clear to be usable at all; STANDARD is the
/// job as it actually arrives; HARD is where a frontier model should pull
/// ahead — ambiguity, a trap, a rule that has to be applied against the grain.
/// A model failing only the hard band is a real and useful answer, and the old
/// flat rate could not express it.
///
/// Serializable because a scored case CARRIES its band into the persisted
/// sweep status — a report read back from the archive has to keep meaning what
/// it meant when it was written, so the band travels with the case rather than
/// being looked up from the registry again.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum EvalBand {
    Easy,
    Standard,
    Hard,
}

impl EvalBand {
    pub fn as_str(self) -> &'static str {
        match self {
            EvalBand::Easy => "easy",
            EvalBand::Standard => "standard",
            EvalBand::Hard => "hard",
        }
    }
}

/// A count limit, with the margin a human would give it.
///
/// A prompt that asks for "3-7 words" is an instruction, not a schema. Scored
/// as a hard boundary it failed three capable models for an EIGHT-word title —
/// one word over, on a title the product then clamps to 90 characters anyway,
/// so nothing anywhere was harmed by the extra word. That is not a measurement
/// of the model; it is a measurement of how literally it read a range.
///
/// So a count fails when the overshoot is MATERIAL. The default margin is a
/// quarter, floored at one unit, which lets 3-7 accept 2-8 and rejects the
/// one-word title and the paragraph alike — the two answers that are actually
/// a different kind of thing from the one asked for.
///
/// USE IT FOR A STATED PREFERENCE, NOT FOR A HARD EDGE. Where exceeding the
/// number breaks something — a card that clips, an action taken one time too
/// many, a batch that creates an eleventh ticket — assert the real limit
/// directly and say what breaks. `tolerance: 0` is available for the cases in
/// between, and reads as the deliberate choice it is.
pub struct CountLimit {
    pub min: Option<i64>,
    pub max: Option<i64>,
    pub unit: &'static str,
    pub asked: &'static str,
    /// The materiality margin as a fraction. `None` is the default quarter.
    pub tolerance: Option<f64>,
}

pub fn count_problem(actual: i64, limit: &CountLimit) -> Option<String> {
    // `f64::round` rounds half away from zero; half-up and half-away-from-
    // zero differ only on negative halves, which a count limit never sees.
    let slack = |n: i64| -> i64 {
        std::cmp::max(
            1,
            (n as f64 * limit.tolerance.unwrap_or(0.25)).round() as i64,
        )
    };
    let plural = |n: i64| -> String {
        if n == 1 {
            format!("1 {unit}", unit = limit.unit)
        } else {
            format!("{n} {unit}s", n = n, unit = limit.unit)
        }
    };
    if let Some(min) = limit.min
        && actual < min - slack(min)
    {
        return Some(format!(
            "{} — the prompt asks for {}",
            plural(actual),
            limit.asked
        ));
    }
    if let Some(max) = limit.max
        && actual > max + slack(max)
    {
        return Some(format!(
            "{} — the prompt asks for {}",
            plural(actual),
            limit.asked
        ));
    }
    None
}

/// THE FLOOR EVERY TEXT FIXTURE NEEDS, and the bug it closes.
///
/// A text harness's `clean` is usually trim-or-null, so any non-empty string
/// is a legitimate value and `schema_valid` is honestly true. That is correct
/// — the CONTRACT is not lying. What was lying was the task score: six
/// fixtures across the summarizer, the distiller, the briefer and outreach
/// asserted only that the answer was not too long, not markdown, not a
/// question and not a repeat of the input. Every one of those is a real
/// failure mode and every one of them is satisfied by saying almost nothing,
/// so replaying the literal string `{"nope": true}` through the whole registry
/// scored six PASSES. The eval census keeps that count, and it is a `<=` so
/// tightening a fixture never fails it.
///
/// So a one-sided fixture states its floor here: how short is too short to be
/// an answer at all, and — where the input has an unmistakable subject — one
/// of a few words the answer has to have engaged with. `mentions` is
/// deliberately a SET of alternatives and not a phrase: it must reject a
/// non-answer without scoring the model's word choice, and a fixture that only
/// one wording can pass measures our prompt rather than the model.
///
/// Returns the admin-facing sentence, or `None` when the answer clears the
/// floor.
pub struct AnswerFloor {
    pub min_chars: usize,
    pub mentions: Vec<String>,
}

pub fn below_answer_floor(value: &str, floor: &AnswerFloor) -> Option<String> {
    let text = value.trim();
    // JS `.length` counts UTF-16 code units; a fixture's floor is written in
    // that unit, and the schema layer already counts the same way for zod's
    // string bounds.
    let units = text.encode_utf16().count();
    if units < floor.min_chars {
        return Some(format!(
            "the answer is {units} characters, which is too short to be an answer to this at all ({} is the floor)",
            floor.min_chars
        ));
    }
    if floor.mentions.is_empty() {
        return None;
    }
    let lower = text.to_lowercase();
    if floor
        .mentions
        .iter()
        .any(|term| lower.contains(&term.to_lowercase()))
    {
        return None;
    }
    // JSON-array spelling for the list — the sentence is pinned by tests.
    let list = serde_json::to_string(&floor.mentions).unwrap_or_default();
    Some(format!(
        "the answer never engages with what it was given - it mentions none of {list}"
    ))
}

/// One tool call the fitness suite's dry run observed — the half of a
/// behavioural fixture that prose cannot see. `errored` is false for a call
/// that succeeded; `args` is what the call was made with, kept raw because
/// every fixture that reads it reads one field (`args.status`, `args.tags`).
///
/// A fixture table that cannot state what a check-in DID is a table of upper
/// bounds — the one-sided shape the garbage census exists to catch.
#[derive(Debug, Clone)]
pub struct CheckCall {
    pub tool: String,
    pub errored: bool,
    pub args: Value,
}

/// Everything a behavioural fixture's `check` is handed besides the reply.
///
/// The WORLD half is what the dry run's sandbox says about the state the
/// model left behind: `world` is the sandbox's world AFTER the run (kept a
/// `Value` because the three surfaces disagree — the toolkit sandbox's
/// boards and tickets, the coding workspace's `{ failure }`, the credential
/// sandbox's spend log — and the def's own fixture helpers are where each
/// gets narrowed), and `exhausted` is the turn-budget flag, false on every
/// run that finished.
#[derive(Debug, Clone, Default)]
pub struct CheckCtx {
    pub calls: Vec<CheckCall>,
    pub world: Option<Value>,
    pub exhausted: bool,
}

impl CheckCtx {
    /// The workspace oracle's verdict on the files as the model left them —
    /// `ctx.world.failure`. A missing world reads as no failure, which is
    /// what a fixture that was never dry-run wants.
    pub fn failure(&self) -> Option<&str> {
        self.world
            .as_ref()
            .and_then(|w| w.get("failure"))
            .and_then(Value::as_str)
    }

    /// Every call to one tool, failures included.
    pub fn calls_of(&self, tool: &str) -> Vec<&CheckCall> {
        self.calls.iter().filter(|c| c.tool == tool).collect()
    }

    /// …only the calls that did not error.
    pub fn successful(&self, tool: &str) -> Vec<&CheckCall> {
        self.calls
            .iter()
            .filter(|c| c.tool == tool && !c.errored)
            .collect()
    }

    pub fn any_call(&self, tool: &str) -> bool {
        self.calls.iter().any(|c| c.tool == tool)
    }

    /// Did a call to `first` precede a call to `second`? The
    /// read-before-write assertions lean on it — earliest `first` against
    /// latest `second` is exactly "some pair, in order".
    pub fn called_before(&self, first: &str, second: &str) -> bool {
        match (
            self.calls.iter().position(|c| c.tool == first),
            self.calls.iter().rposition(|c| c.tool == second),
        ) {
            (Some(i), Some(j)) => i < j,
            _ => false,
        }
    }

    /// Each tool named once, comma-joined — a sentence names each tool once.
    pub fn distinct_tools<'a>(calls: impl IntoIterator<Item = &'a CheckCall>) -> String {
        let mut seen: Vec<&str> = Vec::new();
        for c in calls {
            if !seen.contains(&c.tool.as_str()) {
                seen.push(c.tool.as_str());
            }
        }
        seen.join(", ")
    }
}

/// The two-verdict checks of the defs that cannot gap ARE this enum's
/// `Pass`/`Fail` half — `None` is a pass, `Some` is a fail — so this
/// conversion is how those tables fold onto `CheckResult`.
impl From<Option<String>> for CheckResult {
    fn from(problem: Option<String>) -> Self {
        match problem {
            None => CheckResult::Pass,
            Some(p) => CheckResult::Fail(p),
        }
    }
}

// ── The fitness plane ────────────────────────────────────────────────────────
//
// The FIXTURES a def ships and the DRY-RUN declaration that says what a
// replay of them runs against. They live here, after the fixture-floor
// helpers, so a def file can declare both without importing anything from
// fitness/.

/// A fixture's check, type-erased the same way `render` is: the value arrives
/// as the parsed reply (the def's closure deserializes its own output type)
/// and the context is `CheckCtx`.
pub type CheckFn = Arc<dyn Fn(&Value, &CheckCtx) -> CheckResult + Send + Sync>;

/// One fixture the model-fitness suite replays through the runner with a
/// candidate model pinned.
///
/// `check` is deliberately a plain assertion over the parsed value rather than
/// an expected output: most harness assertions are string facts ("3-7 words",
/// "no invented status", "never an actionId outside the allowlist"), and a
/// deterministic check keeps the suite fast, cheap and free of the
/// who-judges-the-judge regress.
///
/// Built through `EvalCase::new` (band defaults to Standard — a fixture only
/// states a band when it means one) and the `.band(..)` setter, so a def's
/// table reads in fixture order.
pub struct EvalCase {
    pub name: &'static str,
    /// The def's own typed input, as the `render` closure will receive it.
    pub input: Value,
    pub check: CheckFn,
    pub band: EvalBand,
}

impl EvalCase {
    pub fn new(name: &'static str, input: Value, check: CheckFn) -> EvalCase {
        EvalCase {
            name,
            input,
            check,
            band: EvalBand::Standard,
        }
    }

    /// Which difficulty band this fixture belongs to. Bands are reported
    /// separately, so "solid on standard, fails the hard band" is sayable
    /// instead of collapsing into one rate that hides which half a model can
    /// do.
    pub fn band(mut self, band: EvalBand) -> EvalCase {
        self.band = band;
        self
    }
}

/// One file in the coding surface's workspace.
#[derive(Debug, Clone)]
pub struct WorkspaceFile {
    pub path: String,
    pub content: String,
}

/// THE OTHER SURFACE: a file workspace with a test runner, for the coding
/// harnesses. Built per fixture from that fixture's own input, because a
/// repository and the oracle that decides whether its tests pass are properties
/// of the case rather than of the def.
pub struct WorkspaceSpec {
    pub files: Vec<WorkspaceFile>,
    /// The oracle: `None` is a suite that passes against the files as the model
    /// left them. This is `ctx.world.failure` to a fixture.
    pub passes: Arc<dyn Fn(&[WorkspaceFile]) -> Option<String> + Send + Sync>,
}

/// One credential the credential surface will let the model spend. The model
/// never sees `value` — that is the entire premise of the surface (a handle is
/// spent, not read), and the sandbox enforces it by construction.
#[derive(Debug, Clone)]
pub struct GrantedCredential {
    pub handle: String,
    pub value: String,
    /// The host pattern the credential may be spent against, as the operator
    /// wrote it in the grant.
    pub accepts: String,
}

/// THE CREDENTIAL SURFACE — a shell and outbound HTTP, where a granted handle
/// is actually spent. Declared by a harness whose subject is what the model
/// does with a credential it cannot read.
pub type CredentialSpecFn = Arc<dyn Fn(&Value) -> CredentialSpec + Send + Sync>;

#[derive(Debug, Clone, Default)]
pub struct CredentialSpec {
    pub granted: Vec<GrantedCredential>,
}

/// Overrides on the sandbox's standard world — a ticket in a particular state,
/// a gap already filed, a DM already sent. A FUNCTION OF THE INPUT (a fixed
/// world is just a closure returning the constant) because a record read once
/// per DEFINITION can only ever pose questions about one world, and the most
/// valuable fixture in a group is routinely the one that changes it.
pub type WorldFn = Arc<dyn Fn(&Value) -> Value + Send + Sync>;

/// What a replay of this def's fixtures runs against. `None` on every def whose
/// feature is not the tool loop: those fixtures are single-shot and their
/// `ctx` is the empty one.
pub struct DryRunDecl {
    /// Which toolkit tools to offer. Empty when `workspace` or `credentials` is
    /// set — the three are different surfaces and a def has one.
    pub tools: Vec<&'static str>,
    /// MODEL TURNS this def's loop may take, when the default six is not what
    /// production gives it. Bench a twelve-turn work session at six and the
    /// sweep measures a shorter job than the harness performs, then judges the
    /// model on work it was cut off in the middle of. Capped by the sweep's
    /// `MAX_TURN_CEILING`; also widens the case clock (`turns_per_case` reads
    /// it), so a longer loop gets proportionally longer to run in.
    pub max_turns: Option<u32>,
    pub world: Option<WorldFn>,
    pub workspace: Option<Arc<dyn Fn(&Value) -> WorkspaceSpec + Send + Sync>>,
    pub credentials: Option<CredentialSpecFn>,
}

impl DryRunDecl {
    pub fn tools(tools: Vec<&'static str>) -> DryRunDecl {
        DryRunDecl {
            tools,
            max_turns: None,
            world: None,
            workspace: None,
            credentials: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_json_harness_gets_the_floor_derived_and_cannot_decline_it() {
        let def = define_harness(HarnessDefinition::new(
            "blurb-writer",
            "Blurb writer",
            "writes one-line model blurbs",
            ModelSpec {
                pin: None,
                role: Some("utility"),
                chain: None,
                user_id: None,
            },
            Arc::new(|_i, _ctx| Ok(vec![Message::user("blurbs please")])),
            Output::Json {
                preprocess: None,
                schema: Schema::Record(Box::new(Schema::string())),
                repair: None,
                verify: None,
            },
            OnFailure::Null,
        ));
        assert!(def.floor.capabilities.contains(&"json"));
        assert!(def.floor.refuse_below);
        // A def that already declared the json capability — and its own note —
        // is passed through untouched.
        let mut judged = define_harness(HarnessDefinition::new(
            "judge",
            "Judge",
            "judges",
            ModelSpec {
                pin: None,
                role: None,
                chain: None,
                user_id: None,
            },
            Arc::new(|_i, _ctx| Ok(vec![Message::user("judge")])),
            Output::Json {
                preprocess: None,
                schema: Schema::string(),
                repair: None,
                verify: None,
            },
            OnFailure::Null,
        ));
        judged.floor = RoleFloor::refuses(vec!["json-strict"], "needs real structured output");
        let judged = define_harness(judged);
        // The author's list survives AND json is appended — the derivation
        // cannot be bypassed by declaring other capabilities. Only an author
        // who already listed json passes through untouched.
        assert_eq!(judged.floor.capabilities, ["json-strict", "json"]);
        assert_eq!(judged.floor.note, "needs real structured output");
        let mut already = define_harness(HarnessDefinition::new(
            "muse-ticket",
            "Muse ticket",
            "drafts a ticket",
            ModelSpec {
                pin: None,
                role: None,
                chain: None,
                user_id: None,
            },
            Arc::new(|_i, _ctx| Ok(vec![Message::user("draft")])),
            Output::Json {
                preprocess: None,
                schema: Schema::string(),
                repair: None,
                verify: None,
            },
            OnFailure::Null,
        ));
        already.floor = RoleFloor::refuses(vec!["json"], "already said it");
        let already = define_harness(already);
        assert_eq!(already.floor.capabilities, ["json"]);
        // A text harness never gets the floor.
        let text = define_harness(HarnessDefinition::new(
            "titler",
            "Titler",
            "names chats",
            ModelSpec {
                pin: None,
                role: Some("utility"),
                chain: None,
                user_id: None,
            },
            Arc::new(|_i, _ctx| Ok(vec![Message::user("name it")])),
            Output::Text {
                clean: None,
                verify: None,
            },
            OnFailure::Null,
        ));
        assert!(text.floor.capabilities.is_empty());
        assert!(!text.floor.refuse_below);
    }

    #[test]
    fn count_problem_scores_materially_not_literally() {
        let words = CountLimit {
            min: Some(3),
            max: Some(7),
            unit: "word",
            asked: "3-7 words",
            tolerance: None,
        };
        // 8 words on "3-7": one over, inside the quarter margin floored at
        // one — the answer a human would accept. This is the case three
        // capable models failed under the hard boundary.
        assert!(count_problem(8, &words).is_none());
        assert!(count_problem(2, &words).is_none());
        // The paragraph and the single word are both different KINDS of
        // answer than the one asked for.
        assert_eq!(
            count_problem(1, &words).as_deref(),
            Some("1 word — the prompt asks for 3-7 words")
        );
        assert_eq!(
            count_problem(23, &words).as_deref(),
            Some("23 words — the prompt asks for 3-7 words")
        );
        // Pluralization follows the count, not the unit's spelling. Even
        // tolerance 0 keeps the slack floor of one, so "at most 10" accepts
        // 11 and rejects 12: "tolerance: 0 for hard edges" means one unit of
        // grace, never zero.
        let tickets = CountLimit {
            min: None,
            max: Some(10),
            unit: "ticket",
            asked: "at most 10",
            tolerance: Some(0.0),
        };
        assert!(count_problem(10, &tickets).is_none());
        assert!(count_problem(11, &tickets).is_none());
        assert_eq!(
            count_problem(12, &tickets).as_deref(),
            Some("12 tickets — the prompt asks for at most 10")
        );
    }

    #[test]
    fn below_answer_floor_rejects_the_non_answer_without_scoring_wording() {
        let floor = AnswerFloor {
            min_chars: 20,
            mentions: vec!["sean".into(), "the founders".into()],
        };
        // The literal string that scored six passes by saying nothing.
        let non_answer = below_answer_floor("{\"nope\": true}", &floor).unwrap();
        assert!(non_answer.starts_with("the answer is 14 characters"));
        // Engaging with either mention alternative passes.
        assert!(
            below_answer_floor("Sean triaged it and left a note for later review", &floor)
                .is_none()
        );
        // Long enough but engaging with nothing: the fixture's real catch.
        let off = below_answer_floor(
            "A perfectly long sentence about entirely other matters",
            &floor,
        )
        .unwrap();
        assert!(off.contains("it mentions none of [\"sean\",\"the founders\"]"));
        // No mentions declared: only the length floor applies.
        let bare = AnswerFloor {
            min_chars: 20,
            mentions: Vec::new(),
        };
        assert!(
            below_answer_floor(
                "A perfectly long sentence about entirely other matters",
                &bare
            )
            .is_none()
        );
    }

    #[test]
    fn check_result_and_band_spell_themselves() {
        assert!(is_gap(&CheckResult::Pass).is_none());
        assert!(is_gap(&CheckResult::Fail("x".into())).is_none());
        assert_eq!(
            is_gap(&CheckResult::Gap("no runner".into())),
            Some("no runner")
        );
        assert_eq!(EvalBand::Hard.as_str(), "hard");
    }

    #[test]
    fn messages_carry_the_tool_channel_only_when_set() {
        let m = Message::user("hi");
        assert_eq!(m.role.as_str(), "user");
        assert!(m.tool_calls.is_empty() && m.tool_call_id.is_none());
        let call = Message::assistant_calls(vec![super::super::transport::ToolCall {
            name: "search".into(),
            args: "{}".into(),
            id: None,
        }]);
        assert_eq!(call.role, Role::Assistant);
        assert_eq!(call.tool_calls.len(), 1);
        let result = Message::tool_result("[]", "call_0");
        assert!(result.role.is_tool());
        assert_eq!(result.tool_call_id.as_deref(), Some("call_0"));
    }
}
