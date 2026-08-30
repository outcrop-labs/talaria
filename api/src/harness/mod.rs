// THE HARNESS — the port of ui/src/server/harness/, grown slice by slice.
//
// The harness is the layer that ASKS MODELS THINGS and reads the answers
// back: one runner (`run.rs` to come), one transport, and a registry of
// definitions — each a prompt, an output contract, and what to do when the
// contract holds or breaks. It is the dependency under the scheduler's ten
// jobs (the briefer, the outreach sweep, the blurb writer are all harness
// calls) and under the work-session run kind, which is why it lands in
// batch 4 and not with the route tail.
//
// WHAT HAS CROSSED so far: the pure leaves — `text` (the one first-line
// extractor for text harnesses), `json` (the one structured-output parser:
// a brace-balancing scanner, a tolerant relaxer, and schema validation
// whose failure sentences are written to be handed back to the model),
// `schema` (the small declarative shape a def validates against — the
// zod-schema half of a TS def, with zod's issue grammar reproduced from
// probes of zod 4.3.6 itself), `prompt_rules` (the trust-boundary clause
// more than one harness states), `json_schema` (the schema put ON the
// wire: a probed reproduction of zod's `toJSONSchema` renderer, the
// provider-safe keyword subset, and the strict-eligibility walk) — the
// two halves of the contract layer: `define` (the harness declaration,
// type-erased onto `Value` the way the runs engine erases a run's typed
// input, with the derived json floor and the fixture-floor helpers) and
// `transport`'s CONTRACT half (the request/reply types, the one persona
// payload mapping, the response_format derivation, the tool-channel wire
// renderer, and the refusal sentences) — the gateway transports
// (`gateway_transport`/`gateway_stream`, crossed with the chat relay's
// parameter learner in `gateway/params.rs`) — `run` itself: the runner, its
// thirteen injected edges (`real_deps` for production, a recorded world for
// tests), and the nine steps, with the gateway turn as its real transport
// edge — `recorded` (the recorded-transcript harness, the ONE fake world
// every def's tests and the coming fitness sweep drive) — and the first defs:
// `defs/titler`, `defs/summarizer`, `defs/concluder` (the port's first widen
// branch), `defs/blurb_writer` (the only many-keyed one), `defs/librarian`
// (whose hybrid value is why `CleanFn` yields a `Value`), and `defs/judge`
// (the one that refuses below its floor, escalates on an unreadable verdict,
// and pins nothing — the configured pick arrives as a RunContext override),
// and `defs/plan_doc` (the living plan document, whose output contract is the
// whole document rather than a patch — which is why it owns the data-loss
// guard `plan_doc_regression` and answers a bad reply with Null, keeping the
// document the team already had), and `defs/channel_plan` (the ticket drafter,
// the port that made the contract layer grow `PreFn`: a provider asked for
// JSON at the protocol level is obliged to answer an array contract with a
// top-level OBJECT, so the envelope the array arrives wrapped in is unwrapped
// before the schema sees it — while a bare single ticket, which carries a
// `title`, is deliberately left for the repair turn), and `defs/outreach`
// (the proactive check-in — the one harness whose turn is a TOOL LOOP,
// declared as `tools: Own` where it used to be injected as a whole
// hand-written transport, and whose quiet answer is a fallback rather than a
// null: a silent model lands on the same NOTHING_TO_SURFACE token the prompt
// asks for, with `schema_valid` false so the fitness matrix still sees the
// miss), and `defs/work_session` (the highest-stakes output in the product —
// the reply that says a ticket is DONE, unguarded until audit 1.5 because it
// reached the persona gateway by a path the per-call-site guard wiring never
// covered; its turn is a tool loop too, its dispatch prompt is the one prompt
// that interpolates content a stranger wrote and so scopes the trust clause
// to the description's fence rather than stating it globally, and its
// fixtures are the ones that promoted `CheckCall`/`CheckCtx` into define.rs's
// fixture-floor section, because grading a work session means reading
// `args.status`, `args.tags` and whether the read preceded the write), and
// `defs/workbench` (the coding harnesses — three definitions over one builder,
// one per `code-*` role, because the fitness matrix binds a harness to a slot
// by running the real resolver over its `ModelSpec`, and one harness fills one
// column; the port's first `Gap` verdicts live here — a reply that NAMES a
// tool we offered in a syntax the loop does not parse, or a run the turn
// budget cut short, is OUR defect and cannot be scored — which is why
// `CheckCtx` carries the world half: `failure`, what the task's oracle said
// about the files as the model left them, and `exhausted`, whether the loop
// stopped on its own ceiling), and `defs/distiller` (the last pass a
// conversation ever gets — comms-decay distills, indexes into the owner's
// private brain, and archives the chat, refusing to archive what it could
// not summarize, which is why this def's failure policy is Null and nothing
// else: there is no safe placeholder for "we lost it"; its fixtures are the
// calibrated ones — the reversal check that must not fail a recorded
// reversal, the compression ratio that abstains below 600 characters, the
// nothing-durable check that scores invention in proper nouns rather than
// vocabulary — and its `restated` is the second speller of a Gap verdict
// after workbench's tool-syntax gaps), and `defs/briefer` (the daily brief's
// three writes — the opening lede, the one-line delta note, and the reply
// drafted in a colleague's thread, which is the highest-stakes thing any
// briefer write reaches: it goes to SOMEBODY ELSE, under a standing grant,
// with never-decide and never-write-as-them checked on every case rather
// than left to whichever fixture remembered; and the only fixed-model
// family in the product — an EMPTY chain, because `PLATFORM_AGENTS.briefer`
// is `assignable: false` and the owner's own assistant is the point, so
// there is no correct second choice to fall back to), and `defs/inbox_focus`
// (the focus queue's three seats — the brief on the card, the command that
// maps an owner instruction onto ONE allowlisted action, and the detached
// reply — the def family audit 1.3 was written about, where the same command
// was strict JSON on the persona path and prompt-and-pray on the gateway
// path; the port is also where the allowlist became part of the command's
// CONTRACT: `allowed_focus_action_ids` is one function called from `render`,
// `verify` and the adapter's gate, so the list the model is shown, the list
// the answer is graded against and the list execution enforces cannot drift,
// and an out-of-list proposal now costs a repair turn and a `schema_valid:
// false` instead of a green run row its caller dropped on the floor; the
// detached reply is the `tools: Own` seat — the assistant panel answers with
// its real tools while sign-offs stay on the queue cards — and the def
// carried the prompt builder and `validate_command_object` across WITH it,
// because a fixture must replay what production sends, not a copy that can
// drift; and porting its refusal test is what caught a family-wide port
// bug: `define_harness` was wrapping at construction and the def's own
// `d.floor` assignment then WIPED the derived json floor, so blurb_writer,
// channel_plan and the inbox JSON defs would have asked a model measured
// `json: false` anyway — the wrap now happens LAST, as the TS's
// `defineHarness` always did, and each of those defs carries a tripwire
// test pinning the floor), and `defs/research` (the three-stage research
// plane — the planner whose typed query elements are the fix for the
// non-greedy regex that once read a `[2]` CITATION MARKER as the query
// list, the search stage whose floor is the first SUPPLIABLE floor in the
// tree and the whole of audit 1.6 (a model with no live search does not
// fail, it answers from memory in the same confident voice — so the floor
// refuses on a measured `search: false` unless the install can reach
// search through a registered tool, which is why `RecordedWorld.reach`
// had to become injectable), and the synthesis stage, the one harness in
// Talaria that genuinely HAS a tool record — the search hits ARE the
// turn's tool results — so it declares `ground` and `ungrounded_ref`
// fires through the runner instead of the deleted hand-pass; its search
// TRANSPORTS stay TS with the batch-5 fleet/MCP plane, which is also
// where their eleven tests cross).
// Still TS: the fleet turn and the picker
// (`pickTransport`; they cross with the fleet/streaming planes in batch 5, as
// does `RunContext.signal`), `registry` (LAST — registering is the last step
// of a port, and it crosses once every def is here to fill it), and the rest
// of harness/defs/ — plus the eval-case/dry-run plane, which crosses with the
// fitness suite.

pub mod define;
pub mod defs;
pub mod json;
pub mod json_schema;
pub mod prompt_rules;
pub mod recorded;
pub mod run;
pub mod schema;
pub mod text;
pub mod transport;
