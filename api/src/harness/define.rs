// THE harness contract. A harness DECLARES what it needs; it never chooses a
// transport, a model, a parser, or a failure policy. The runner (`run.rs`, to
// come) honors the declaration, and it is the only code that talks to a model.
//
// WHY THIS FILE EXISTS
//   `PLATFORM_AGENTS` in platform-agents.ts already names nine harnesses and
//   describes each one's job — and carries none of the things that make a
//   harness a harness. The prompt, the output shape, the fallback chain, the
//   failure behavior and the guard pass live in eight other files, hand-written
//   nine times over, and `PLATFORM_AGENTS[].auto` is a PROSE DESCRIPTION of a
//   chain implemented elsewhere and free to drift from it (it already has:
//   'pl-main when judging is enabled without a pick' is spelled out in judge.ts
//   and in six other files besides). This interface is the other half of that
//   registry — the executable half.
//
//   The cost of the nine copies is not aesthetic. Each one answers "the model
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
// fighting for generics. TS defs are `HarnessDefinition<I, O>` with the
// author's own input type and zod schema threaded through `render`, `verify`
// and `evals`. Rust has no existential the registry could hold across 23 defs
// without making every one a monomorphized instantiation the registry cannot
// name — so, exactly as `runs/define.rs` erases a run's typed input into a
// `Value` column, a harness def erases I/O at the definition site: `render`
// takes `(&Value, &RenderContext)` and the def's own closure deserializes its
// typed input internally, and `verify`/`ground` do the same. The contract the
// TS generics enforced — that `render`'s input and `schema`'s subject are the
// same type — becomes the closure's own `serde_json::from_value`, whose
// failure reads as a failed contract exactly like a TS throw.
//
// DEFERRED TO THE FITNESS PLANE (batch 5's tail): `EvalCase`/`EvalContext`,
// `NO_TOOLS`, and the `dryRun`/`evals` slots — they import the fitness
// toolbox's world, and this module must stay importable without it. The two
// floor helpers those fixtures lean on (`count_problem`,
// `below_answer_floor`) cross NOW because they are this module's own pure
// exports.

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
/// RE-DECIDED, NOT INHERITED, when the tool probes were armed. The `vision`
/// probe wants image content and is the first caller ever to want any, so the
/// question was live: widen `content` to the OpenAI content-parts union, or
/// leave vision unmeasurable. It stays a string, because a HALF-WIDENED union
/// is worse than none and half is all that is reachable from here —
/// `completeViaGateway`'s signature takes `content: string`, and every
/// consumer of a message list in the tree reads `.content` as one:
/// `groundingTextOf` and `extractToolRecord` for the guard pass,
/// `lastUserMessage` and `anchorJson` in the runner, `estimateTokens` on both
/// metering paths, and 23 harness `render`s. A union that only the persona
/// payload honored would report `[object Object]` into the ledger and ground
/// the guard against nothing.
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
    /// declaration that reads to the next author as a hard requirement, which
    /// is how the eight ports arrived with two spellings of "needs JSON, runs
    /// anyway": one wrote `capabilities: []` and five wrote
    /// `capabilities: ['json']`, and both ran identically. Keep this EMPTY
    /// unless `refuse_below` is true, and say what the job leans on in
    /// `requires` — which is what the fitness matrix scores, and which never
    /// blocks. The registry's tests enforce the pairing.
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
/// type and a decode failure is a failed contract, exactly like a TS `render`
/// that throws.
pub type RenderFn =
    Arc<dyn Fn(&Value, &RenderContext) -> Result<Vec<Message>, String> + Send + Sync>;

/// A text harness's narrowing step: receives the raw reply, returns the value,
/// or `None` to fail the contract (which is exactly what the titler's
/// quote-and-fence stripping always did by hand).
///
/// The value is a `Value`, not a string, because a text harness may parse a
/// HYBRID — the librarian's reply is a markdown body plus a trailing `TAGS:`
/// line, and its clean consumes the line and returns `{body, tags}`. That is
/// the TS contract (`clean` returns the typed value; the runner stores it as
/// the run's value), and it is why redaction re-applies the WHOLE contract to
/// the scrubbed text: a redacted hybrid is rebuilt by the same parse, not
/// handed back half-scrubbed.
pub type CleanFn = Arc<dyn Fn(&str) -> Result<Option<Value>, String> + Send + Sync>;

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
/// `Err` IS THE TS THROW — harness-author code meeting model output, and a
/// throw out of any of `render`/`clean`/`verify`/`ground` is a failed
/// contract rather than the one exception that escapes a runner whose whole
/// promise is that a bad model produces a RESULT.
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
/// ran inside the agent. So the rule self-skipped on all 23 harnesses, and the
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

/// The erasure of TS `ground: (input: I) => Grounding | null` — `Ok(None)` is
/// the absent hook.
pub type GroundFn = Arc<dyn Fn(&Value) -> Result<Option<Grounding>, String> + Send + Sync>;

// ── The output contract and the failure policy ───────────────────────────────

/// The output contract. `Json` puts the runner in structured mode: it asks for
/// JSON at the protocol level when the model can honor that, parses with the
/// one balanced-brace extractor, and repairs on a malformed reply (audit 1.4 —
/// nothing in the tree retried before this).
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
/// `Throw` MEANS ANY FAILURE TO PRODUCE A VALUE, which is what a caller reads
/// it to mean and what it did not do. The runner RETURNS for everything that
/// happens before or during the call — nothing in the chain routes, the floor
/// refuses, `render` fails, the transport dies — so `throw` used to cover the
/// contract failure and nothing else, and the policy had to be restated by
/// hand at every call site. Five callers restated it; the two that did not
/// BOTH shipped a bug: research synthesis saved an empty report, marked the
/// run `done`, indexed it and notified the requester after a 502, and the
/// channel planner answered "nothing to plan yet" on a channel full of work
/// because its agent container was restarting. It now throws on all of them.
///
/// THE OTHER THREE STAY CONTRACT-SCOPED, and that asymmetry is deliberate
/// rather than left over. They describe what a caller GETS when the model
/// answered and the answer was unusable — a question that does not arise when
/// no model was reached — and widening them would break both callers that use
/// them. `Fallback` on a pre-call failure would hand outreach its "nothing to
/// surface" token during a gateway outage, so a dead provider would read as a
/// normal quiet pass on every sweep. `Escalate` on a pre-call failure would
/// have the judge notify every board editor about every ticket for as long as
/// the gateway is down — the notification storm this whole audit is about. A
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

    /// Model resolution, declared not written — the chain that was hand-copied
    /// into seven files (audit 1.10).
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
    /// that widens the Inbox command harness (audit 1.8). A harness that
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
// The two pure exports the fitness fixtures lean on. They cross now because
// they are this module's own — the eval-case TYPE stays behind with the
// fitness plane.

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
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
    // Math.round on a positive fraction rounds half UP in JS; Rust's f64
    // round() rounds half away from zero — identical for the positives a
    // count limit ever sees.
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
    // JSON.stringify's array spelling, so the sentence an admin reads matches
    // the one the TS suite wrote.
    let list = serde_json::to_string(&floor.mentions).unwrap_or_default();
    Some(format!(
        "the answer never engages with what it was given - it mentions none of {list}"
    ))
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
        // tolerance 0 keeps the slack floor of one — TS's Math.max(1, ·) — so
        // "at most 10" accepts 11 and rejects 12: "tolerance: 0 for hard
        // edges" means one unit of grace, never zero.
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
