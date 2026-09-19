// THE HARNESS — the layer that ASKS MODELS THINGS and reads the answers
// back: one runner (`run.rs`), one transport, and a registry of definitions —
// each a prompt, an output contract, and what to do when the contract holds
// or breaks. It is the dependency under the scheduler's ten jobs (the
// briefer, the outreach sweep, the blurb writer are all harness calls) and
// under the work-session run kind.
//
// The leaves: `text` (the one first-line extractor for text harnesses),
// `json` (the one structured-output parser: a brace-balancing scanner, a
// tolerant relaxer, and schema validation whose failure sentences are written
// to be handed back to the model), `schema` (the small declarative shape a
// def validates against — its error sentences are zod-shaped, the wire
// contract the repair turn quotes), `prompt_rules` (the trust-boundary clause
// more than one harness states), `json_schema` (the schema put ON the wire:
// the provider-safe keyword subset and the strict-eligibility walk), `define`
// (the harness declaration, type-erased onto `Value` the way the runs engine
// erases a run's typed input, with the derived json floor and the fixture-
// floor helpers), `transport` (the request/reply types, the persona payload
// mapping, the response_format derivation, the tool-channel wire renderer,
// the refusal sentences, and the gateway transports), `run` (the runner: its
// thirteen injected edges — `real_deps` for production, a recorded world for
// tests — and its nine steps, with the gateway turn as the real transport
// edge), `recorded` (the recorded-transcript harness — the ONE fake world
// every def's tests and the fitness sweep drive), and `registry` — every def
// in the admin panel's two-block order, plus the tests that hold the
// executable half and `PLATFORM_AGENTS` together: the misspelled-rule-id
// check, the no-anonymous-guard check, the empty-chain lock on every
// subject-of-the-call harness, and the verify census (contracts a zod
// `.refine` would state live as `verify` closures, because `Schema` has no
// post-parse refine hook). The definitions live in `defs/`, one module per
// family.

pub mod define;
pub mod defs;
pub use talaria_harness_json as json;
pub use talaria_harness_json_schema as json_schema;
pub use talaria_harness_prompt_rules as prompt_rules;
pub mod recorded;
pub mod registry;
pub mod run;
pub use talaria_harness_schema as schema;
pub use talaria_harness_text as text;
pub mod transport;

#[cfg(test)]
mod prompt_rules_coverage {
    use super::define::RenderContext;
    use super::prompt_rules::{STATES_THE_BOUNDARY, UNTRUSTED_INPUT};

    #[test]
    fn every_named_harness_renders_the_clause() {
        for name in STATES_THE_BOUNDARY {
            let Some(h) = super::registry::builtin_by_id(name) else {
                panic!("STATES_THE_BOUNDARY names \"{name}\", which is not a registered harness");
            };
            let case = h
                .def
                .evals
                .first()
                .unwrap_or_else(|| panic!("\"{name}\" has no eval fixture to render"));
            let ctx = RenderContext {
                widened: false,
                model: "test".into(),
            };
            let msgs = (h.def.render)(&case.input, &ctx)
                .unwrap_or_else(|e| panic!("\"{name}\" failed to render its own fixture: {e}"));
            let carries = msgs.iter().any(|m| m.content.contains(UNTRUSTED_INPUT));
            assert!(
                carries,
                "\"{name}\" is named in STATES_THE_BOUNDARY but its rendered prompt does not carry UNTRUSTED_INPUT"
            );
        }
    }
}
