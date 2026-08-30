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
// `title`, is deliberately left for the repair turn).
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
