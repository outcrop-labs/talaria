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
// more than one harness states), and `json_schema` (the schema put ON the
// wire: a probed reproduction of zod's `toJSONSchema` renderer, the
// provider-safe keyword subset, and the strict-eligibility walk). Still TS:
// `define`, `transport`, `run`, `registry`, `gateway-params`, `recorded`,
// `model` (the persona engine's harness-facing half crossed earlier as
// `harness_model.rs`), and every def under harness/defs/.

pub mod json;
pub mod json_schema;
pub mod prompt_rules;
pub mod schema;
pub mod text;
