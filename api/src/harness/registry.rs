// THE ACTIVITY-HARNESS REGISTRY: one place that can name every harness in the
// product, so that nothing has to go looking for them file by file again.
//
// WHY THIS FILE EXISTS
//   `PLATFORM_AGENTS` (platform_agents.rs) is the METADATA half of this
//   registry — id, label, whether an admin may assign a model — and
//   `harness/defs/*` is the EXECUTABLE half. Until this file the two were
//   joined only by a string an author remembered to spell the same way twice.
//   Two things iterate the executable half and neither can grep: the in-UI
//   model-fitness suite replays every declared eval fixture against a
//   candidate model, and the admin panel shows each harness's floor, its
//   widening and which model is carrying it. The registry's tests hold the two
//   halves together: they fail if a harness id drifts from its platform-agent
//   id, if a guard rule id is misspelled (which silently disables EVERY rule
//   for that harness — see `narrow_guard_config`), or if a floor declares
//   capabilities it never refuses on.
//
// THE LAYERS — the source vocabulary keeps three (builtin < app-shipped <
// admin-custom, merged by id); only the first is real here, registered
// statically. The admin-custom layer is deliberately empty on both sides (an
// activity harness is CODE — `render`, `evals[].check` — and Talaria does not
// run code out of a database row). `builtin_by_id` is the scheduler's lookup.
//
// THE PLATFORM_AGENTS CROSS-CHECK, and the two places the lists do NOT line
// up. Both are real and neither is forced:
//
//   'briefer' is a platform agent with NO harness pinning it. It is
//   `assignable: false` on purpose — the briefer is the owner's own personal
//   assistant, and its persona and privacy are the feature — so the inbox
//   harnesses take their model as an explicit `RunContext.model` from the
//   caller rather than resolving a pin. Giving them `pin: "briefer"` would
//   invent an assignment slot the product deliberately does not offer.
//
//   THE CALLER-PINNED HARNESSES have no platform-agent entry, for the same
//   reason from the other side: there is nothing for an admin to assign,
//   because the model is decided by the SUBJECT of the call, not by a slot.
//   The Inbox trio and the briefer run on the owner's own assistant;
//   'work-session' runs on the agent assigned to the ticket; 'channel-plan'
//   and 'plan-doc' run on the channel's or the plan's own agent (including a
//   TIER of it, picked in the Plan modal); 'outreach:check-in' runs on the
//   agent doing the reaching out. Every one of them takes its model as an
//   explicit `RunContext.model`. They still belong in this registry, because
//   the fitness suite has to be able to score them — "can this model map an
//   instruction onto an action", "can this model work a ticket" and "can this
//   model rewrite a plan document without gutting it" are exactly the
//   questions an admin picking a model for an agent needs answered.
//
//   The judge is the third exception and the interesting one: it HAS a platform
//   agent, but its model lives in `judge_config` (see the header of
//   defs/judge.rs) so its definition declares no pin. `platform_agent_of`
//   below is where that exception is written down once.
//
// A HARNESS THAT IS NOT IN `BUILTINS` IS INVISIBLE, and invisible in the two
// ways that matter most: the fitness suite cannot replay its eval fixtures, so
// every assertion its author wrote is dead code, and the admin panel cannot
// show its floor.

use std::sync::LazyLock;

use super::define::{EvalBand, HarnessDefinition};
use super::defs;

/// Where a registered harness came from — the admin panel's wire vocabulary,
/// so a source serializes the same way on both sides. Only `Builtin` is
/// constructed today.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HarnessSource {
    Builtin,
    /// App-shipped, by slug — not constructed today; see the header.
    App(&'static str),
    Custom,
}

/// One harness as the registry hands it out: the definition (already
/// type-erased in Rust — see define.rs's "THE ERASURE" for why there is no
/// `use` closure to protect a generic pair) plus its layer.
#[derive(Clone, Copy)]
pub struct RegisteredHarness {
    pub def: &'static HarnessDefinition,
    pub source: HarnessSource,
}

impl RegisteredHarness {
    /// Fixture names — the inputs stay on the def, where the sweep reads them.
    pub fn eval_names(&self) -> impl Iterator<Item = &'static str> + '_ {
        self.def.evals.iter().map(|e| e.name)
    }

    /// Each fixture's difficulty band, by name. Read by the fitness sweep when
    /// it records a fixture it did not run: a skipped case still belongs to a
    /// band, and the alternative was defaulting every skip to 'standard' and
    /// quietly mis-reporting which half of a suite went unmeasured.
    pub fn band_of(&self, name: &str) -> EvalBand {
        self.def
            .evals
            .iter()
            .find(|e| e.name == name)
            .map(|e| e.band)
            .unwrap_or(EvalBand::Standard)
    }
}

/// The harnesses Talaria ships, each constructed once. Order is the order the
/// admin panel shows them in, in two blocks: first the ones an admin ASSIGNS a
/// model to, then the ones whose model is decided by the subject of the call.
/// An admin reading the panel top to bottom therefore reads "here is what you
/// control" before "here is what your agents are doing with the models you
/// already gave them".
static DEFS: [LazyLock<HarnessDefinition>; 35] = [
    // ── Assigned in Admin ─────────────────────────────────────────────────────
    LazyLock::new(defs::titler::titler_harness),
    LazyLock::new(defs::summarizer::summarizer_harness),
    LazyLock::new(defs::librarian::librarian_harness),
    LazyLock::new(defs::blurb_writer::blurb_writer_harness),
    LazyLock::new(defs::distiller::distiller_harness),
    LazyLock::new(defs::concluder::concluder_harness),
    LazyLock::new(defs::muse::muse_cron_harness),
    LazyLock::new(defs::muse::muse_agent_harness),
    LazyLock::new(defs::muse::muse_ticket_harness),
    LazyLock::new(defs::muse::muse_skill_form_harness),
    LazyLock::new(defs::muse::muse_template_form_harness),
    LazyLock::new(defs::muse::muse_draft_harness),
    LazyLock::new(defs::judge::judge_harness),
    // ── The model comes from the subject of the call ─────────────────────────
    // The owner's own personal assistant: the Inbox trio and the daily brief.
    LazyLock::new(defs::inbox_focus::inbox_brief_harness),
    LazyLock::new(defs::inbox_focus::inbox_command_harness),
    LazyLock::new(defs::inbox_focus::inbox_reply_harness),
    // The daily brief's two writers. Same agent and the same unassignable
    // model rule as the trio above; different contract, because what these
    // write is appended permanently rather than replaced on the next
    // fingerprint.
    LazyLock::new(defs::briefer::daily_brief_lede_harness),
    LazyLock::new(defs::briefer::daily_brief_note_harness),
    // The sharpest of the family: this one writes to somebody OTHER than the
    // owner, in their name. Same unassignable model rule, and the reason is
    // strongest here.
    LazyLock::new(defs::briefer::assistant_reply_harness),
    // The agent assigned to the ticket.
    LazyLock::new(defs::work_session::work_session_harness),
    // THE HERMES FAMILY — "can this model be a workspace agent", as opposed to
    // "can it run one of Talaria's own features". Every other harness here
    // measures a platform feature; a fleet persona is handed the toolkit and a
    // sentence of English, and what it does next is the job. See
    // `defs/hermes_knowledge.rs` for why that needed its own family.
    LazyLock::new(defs::hermes_knowledge::hermes_knowledge_harness),
    // The second of the family — the six document tools, none of which had ever
    // been asked of a model. See `defs/hermes_documents.rs`.
    LazyLock::new(defs::hermes_documents::hermes_documents_harness),
    // The third: who can SEE things. Runs as a personal assistant because five of
    // its six tools refuse a general org agent outright.
    LazyLock::new(defs::hermes_governance::hermes_governance_harness),
    // The fourth: calendar and mail, where a wrong answer reaches somebody
    // outside the company.
    LazyLock::new(defs::hermes_google::hermes_google_harness),
    // The fifth and last: commissioning research. Narrow on purpose — it measures
    // the DELEGATION, never the report, which the research-* harnesses own.
    LazyLock::new(defs::hermes_research::hermes_research_harness),
    // Can it spend a credential it is not allowed to see? The platform half of
    // that arrangement is enforced in code; this is the MODEL half, which until
    // now nothing measured. See `defs/secret_handles.rs`.
    LazyLock::new(defs::secret_handles::secret_handles_harness),
    // The three coding harnesses, one per Workbench effort slot — the fitness
    // matrix binds a harness to a slot, one harness per column.
    LazyLock::new(defs::workbench::workbench_light_harness),
    LazyLock::new(defs::workbench::workbench_standard_harness),
    LazyLock::new(defs::workbench::workbench_heavy_harness),
    LazyLock::new(defs::channel_plan::channel_plan_harness),
    LazyLock::new(defs::plan_doc::plan_doc_harness),
    LazyLock::new(defs::outreach::outreach_check_in_harness),
    // The researching agent, and one of the TWO harnesses in the tree that
    // REFUSE below their floor (research-search, on 'search'; the judge is the
    // other, on json/json-strict/instruction-following) — a model with no web
    // search does not error, it answers from memory and the brief comes out
    // confident and uncited.
    LazyLock::new(defs::research::queries_harness),
    LazyLock::new(defs::research::search_harness),
    LazyLock::new(defs::research::synthesis_harness),
];

static BUILTINS: LazyLock<Vec<RegisteredHarness>> = LazyLock::new(|| {
    DEFS.iter()
        .map(|d| RegisteredHarness {
            def: d,
            source: HarnessSource::Builtin,
        })
        .collect()
});

/// The builtin layer alone — the whole shipped set, in panel order; the tests
/// below assert against it, and `builtin_by_id` narrows it to the scheduler's
/// lookup.
pub fn builtin_activity_harnesses() -> &'static [RegisteredHarness] {
    &BUILTINS
}

/// The scheduler's lookup: one harness by id, from the shipped set — nothing
/// outside the builtins can appear here.
pub fn builtin_by_id(id: &str) -> Option<&'static RegisteredHarness> {
    BUILTINS.iter().find(|h| h.def.id == id)
}

/// Which platform agent's model assignment drives this harness, if any.
///
/// `model.pin` for the assigned block above. The judge is the exception,
/// written down here rather than rediscovered: it is a platform agent, but its
/// model lives in `judge_config` so that the Guard panel and the Platform
/// panel cannot disagree about which model is judging, and its definition
/// therefore declares no pin.
///
/// None means nothing in Admin assigns this harness a model, which is now the
/// larger half of the registry: every harness whose model comes from the
/// subject of the call. The tests lock the exact list so that a harness which
/// SHOULD have had a pin cannot join it by omission.
pub fn platform_agent_of(harness: &RegisteredHarness) -> Option<&'static str> {
    if harness.def.id == "judge" {
        return Some("judge");
    }
    harness.def.model.pin
}

#[cfg(test)]
mod tests {
    use super::super::define::Output;
    use super::*;
    use crate::gateway::guard::rule_ids;
    use crate::platform_agents::PLATFORM_AGENTS;

    const EXPECTED_IDS: &[&str] = &[
        "titler",
        "summarizer",
        "librarian",
        "blurb-writer",
        "distiller",
        "concluder",
        "muse:cron",
        "muse:agent",
        "muse:ticket",
        "muse:skill-form",
        "muse:template-form",
        "muse:draft",
        "judge",
        "inbox-brief",
        "inbox-command",
        "inbox-reply",
        "briefer:daily-open",
        "briefer:daily-delta",
        "briefer:reply",
        "work-session",
        "hermes:knowledge",
        "hermes:documents",
        "hermes:governance",
        "hermes:google",
        "hermes:research",
        "secrets:handles",
        "workbench:light",
        "workbench:standard",
        "workbench:heavy",
        "channel-plan",
        "plan-doc",
        "outreach:check-in",
        "research-queries",
        "research-search",
        "research-synthesis",
    ];

    #[test]
    fn lists_every_harness_exactly_once() {
        let harnesses = builtin_activity_harnesses();
        let ids: Vec<&str> = harnesses.iter().map(|h| h.def.id).collect();
        let unique: std::collections::HashSet<&str> = ids.iter().copied().collect();
        assert_eq!(unique.len(), ids.len(), "a harness id appears twice");
        assert_eq!(ids, EXPECTED_IDS);
    }

    #[test]
    fn names_itself_for_a_human() {
        for h in builtin_activity_harnesses() {
            assert!(!h.def.label.trim().is_empty(), "{} has no label", h.def.id);
            assert!(!h.def.job.trim().is_empty(), "{} has no job line", h.def.id);
            assert!(
                !h.def.floor.note.trim().is_empty(),
                "{} has no floor note",
                h.def.id
            );
        }
    }

    #[test]
    fn narrows_the_guard_to_rule_ids_that_exist() {
        // The sharpest silent failure in the whole declaration surface:
        // `narrow_guard_config` turns a rule on only when the harness names it,
        // so a typo does not disable one rule — it disables ALL of them, with
        // no error anywhere and a `guard` block in the file that reads as
        // protection.
        let ids = rule_ids();
        for h in builtin_activity_harnesses() {
            let Some(guard) = &h.def.guard else { continue };
            for rule in guard.rules.iter().flatten() {
                assert!(
                    ids.contains(rule),
                    "{} declares guard rule \"{rule}\", which is not in the registry",
                    h.def.id
                );
            }
        }
    }

    #[test]
    fn makes_every_harness_name_its_rules() {
        // The sibling of the case above. `narrow_guard_config` returns the
        // FULL config when `rules` is None, so deleting a guard block does not
        // weaken a harness, it runs EVERY enabled rule on it. That is what put
        // the judge on `zero_tool_claim` and `fabricated_outage` — rules
        // structurally wrong for a verdict, which describes claimed work rather
        // than doing any — and inflated `guard_findings.model` for whichever
        // model an admin had chosen to judge with. No harness may leave the
        // block off; if a future harness genuinely wants all rules, it says so
        // by listing them.
        for h in builtin_activity_harnesses() {
            let Some(guard) = &h.def.guard else {
                panic!(
                    "{} declares no guard block, which silently opts it into every enabled rule",
                    h.def.id
                );
            };
            assert!(
                guard.rules.as_ref().is_some_and(|r| !r.is_empty()),
                "{} has a guard block that names no rules — same effect as having none",
                h.def.id
            );
        }
    }

    #[test]
    fn keeps_the_refusal_list_empty_unless_it_actually_refuses() {
        // The runner reads `floor.capabilities` only when `refuse_below` is
        // true. Declaring capabilities without refusing is inert, and it reads
        // to the next author as a hard requirement. The ask belongs in
        // `requires`.
        for h in builtin_activity_harnesses() {
            if !h.def.floor.refuse_below {
                assert!(
                    h.def.floor.capabilities.is_empty(),
                    "{} declares a floor it never enforces",
                    h.def.id
                );
            }
        }
    }

    #[test]
    fn never_puts_a_capability_in_the_floor_that_is_not_also_required() {
        // The floor is the non-negotiable SUBSET of `requires`.
        for h in builtin_activity_harnesses() {
            for cap in &h.def.floor.capabilities {
                assert!(
                    h.def.requires.contains(cap),
                    "{} refuses on \"{cap}\" without requiring it",
                    h.def.id
                );
            }
        }
    }

    #[test]
    fn spells_the_utility_fallback_exactly_one_way() {
        // `role: "utility"` and the 'utility' step resolve the same model
        // through the same allowlist gate, and differ only in the `chain_step`
        // recorded on the run — so two harnesses resolving identically would
        // report differently to the fitness page. One spelling: the step.
        // `role` stays available for a harness that genuinely has a role of its
        // own.
        for h in builtin_activity_harnesses() {
            assert_ne!(
                h.def.model.role,
                Some("utility"),
                "{} declares role: \"utility\" instead of the \"utility\" step",
                h.def.id
            );
        }
    }

    #[test]
    fn gives_every_widening_a_reason_an_admin_can_read() {
        for h in builtin_activity_harnesses() {
            let Some(widen) = &h.def.widen else { continue };
            assert!(!widen.requires.is_empty(), "{} widens on nothing", h.def.id);
            assert!(
                !widen.note.trim().is_empty(),
                "{} has no widening note",
                h.def.id
            );
        }
    }

    /// `output.verify` presence, spelled once for the census below.
    fn verifies(out: &Output) -> bool {
        match out {
            Output::Text { verify, .. } | Output::Json { verify, .. } => verify.is_some(),
        }
    }

    #[test]
    fn keeps_the_census_of_contracts_a_schema_cannot_state() {
        // THE CENSUS: the list is the document, and moving a harness on or off
        // it has to be a deliberate edit here.
        //
        // WHAT IT PROTECTS. `harness_runs.schema_valid` is the OBSERVED half of
        // the model-fitness matrix, and it is only worth reading if it agrees
        // with the offline eval fixtures. For a harness whose correctness is a
        // RELATION between the input and the output, a schema — a module
        // constant built before the input exists — cannot state the contract,
        // so the runner would record `true` for a value the caller then
        // dropped, while the harness's own eval `check` rejected the identical
        // value. Four shipped bugs were that one gap; `output.verify` is where
        // each of them now lives.
        //
        // THREE ENTRIES COME FROM REFINES. Contracts a zod `.refine` would
        // state on the schema itself (muse's handle floor, the complete-record
        // rules) live here instead: `Schema` has no post-parse refine hook, so
        // `verify` — the only slot that runs after the schema — carries them,
        // messages verbatim. Same behavior, same repair turn, different slot;
        // they belong in this census for the same reason (deleting one
        // silently turns the column back into an optimistic liar).
        //
        // Deleting a verify fails this test. That is the point: it is the one
        // change that nothing else in the tree would notice.
        const VERIFIES: &[(&str, &str)] = &[
            (
                "blurb-writer",
                "the keys must be the model ids that were sent - a display-name key writes nothing",
            ),
            (
                "muse:agent",
                "a one-character handle is not a usable agent id — the client's null check, become a repair instruction",
            ),
            (
                "muse:ticket",
                "a relative date resolves against the \"now\" in the context, not the model's own idea of today",
            ),
            (
                "muse:skill-form",
                "return BOTH the skill's name and its content, and the name must survive coercion to at least 2 characters",
            ),
            (
                "muse:template-form",
                "return ALL THREE of the template's name, guidance and body — a partial template saves nothing",
            ),
            (
                "channel-plan",
                "a tag must name a label some workflow in the input map defines, or dispatch misroutes on it",
            ),
            (
                "inbox-command",
                "the proposed actionId must be one the owner's instruction authorized, on the surface this run offered",
            ),
        ];
        let mut declared: Vec<&str> = builtin_activity_harnesses()
            .iter()
            .filter(|h| verifies(&h.def.output))
            .map(|h| h.def.id)
            .collect();
        declared.sort_unstable();
        let mut expected: Vec<&str> = VERIFIES.iter().map(|(id, _)| *id).collect();
        expected.sort_unstable();
        assert_eq!(declared, expected);
        for (id, why) in VERIFIES {
            assert!(
                !why.trim().is_empty(),
                "{id} has a census entry with no reason"
            );
        }
    }

    #[test]
    fn ships_eval_fixtures() {
        // AN UNSCORED HARNESS IS AN INVISIBLE ONE: the fitness suite replays
        // `evals`, and a def without any is a column the matrix can never fill.
        // Every def declares at least one fixture, by name, so a fixture with
        // an empty name cannot smuggle a def past the count either.
        for h in builtin_activity_harnesses() {
            assert!(
                !h.def.evals.is_empty(),
                "{} ships no eval fixtures — it is invisible to the fitness suite",
                h.def.id
            );
            for e in &h.def.evals {
                assert!(
                    !e.name.trim().is_empty(),
                    "{} has an unnamed fixture",
                    h.def.id
                );
            }
        }
    }

    #[test]
    fn keeps_fixture_names_distinct_within_a_def() {
        // The sweep records by (harness, fixture name) and resumes by it; a
        // duplicated name makes the second fixture a silent re-run of the
        // first's slot and the resume ledger lie about what is left.
        for h in builtin_activity_harnesses() {
            let names: Vec<&str> = h.eval_names().collect();
            let unique: std::collections::HashSet<&str> = names.iter().copied().collect();
            assert_eq!(
                unique.len(),
                names.len(),
                "{} has two fixtures with the same name",
                h.def.id
            );
        }
    }

    #[test]
    fn asks_for_a_temperature_in_range_or_none_at_all() {
        for h in builtin_activity_harnesses() {
            let Some(t) = h.def.temperature else { continue };
            assert!(
                (0.0..=1.0).contains(&t),
                "{} asks for temperature {t}",
                h.def.id
            );
        }
    }

    // ── the PLATFORM_AGENTS cross-check ────────────────────────────────────────

    fn agent_map() -> Vec<(&'static str, Option<&'static str>)> {
        builtin_activity_harnesses()
            .iter()
            .map(|h| (h.def.id, platform_agent_of(h)))
            .collect()
    }

    #[test]
    fn gives_every_assignable_platform_agent_at_least_one_harness_to_drive() {
        let agent_of = agent_map();
        for agent in PLATFORM_AGENTS {
            if !agent.assignable {
                continue;
            }
            let driven = agent_of
                .iter()
                .filter(|(_, a)| *a == Some(agent.id))
                .count();
            assert!(
                driven > 0,
                "platform agent \"{}\" has no harness — its model assignment goes nowhere",
                agent.id
            );
        }
    }

    #[test]
    fn records_the_places_the_lists_deliberately_do_not_line_up() {
        // Locked so that a change is a decision rather than a surprise. See the
        // header for the argument behind each one.
        let harnesses = builtin_activity_harnesses();
        let agent_of = agent_map();

        // 'briefer' is assignable: false — the Inbox trio and the daily brief
        // both run on the owner's own assistant, so there is no pin for a
        // harness to declare. It HAS harnesses; what it does not have is an
        // assignment slot, which is a different sentence and the one that
        // matters.
        assert!(
            !PLATFORM_AGENTS
                .iter()
                .find(|a| a.id == "briefer")
                .expect("the briefer agent exists")
                .assignable
        );
        assert!(agent_of.iter().all(|(_, a)| *a != Some("briefer")));

        // ...and from the other side: the harnesses with no platform agent are
        // exactly the ones whose model comes from the SUBJECT of the call — the
        // owner's assistant, the agent on the ticket, the agent in the channel
        // or the plan, the researching agent. Nothing for an admin to assign,
        // but the fitness suite still has to be able to score every one of them.
        //
        // This list growing is how a harness that should have declared a pin
        // gets caught: adding an id here is a claim that no admin slot names its
        // model.
        //
        // THERE ARE THREE KINDS, not two. A harness's model comes from a
        // PLATFORM AGENT (an admin assigns it on Models → Platform), from a
        // MODEL ROLE (an admin assigns it on the same page, under a different
        // registry — `MODEL_ROLES`), or from the SUBJECT of the call. Only the
        // third has nothing for an admin to assign, and only the third may
        // therefore declare no way to resolve a model.
        let role_assigned: Vec<&str> = harnesses
            .iter()
            .filter(|h| platform_agent_of(h).is_none() && h.def.model.role.is_some())
            .map(|h| h.def.id)
            .collect();
        assert_eq!(
            role_assigned,
            ["workbench:light", "workbench:standard", "workbench:heavy"]
        );

        let subject_of_call: Vec<&str> = harnesses
            .iter()
            .filter(|h| platform_agent_of(h).is_none() && h.def.model.role.is_none())
            .map(|h| h.def.id)
            .collect();
        assert_eq!(
            subject_of_call,
            [
                "inbox-brief",
                "inbox-command",
                "inbox-reply",
                "briefer:daily-open",
                "briefer:daily-delta",
                "briefer:reply",
                "work-session",
                // The Hermes family and 'secrets:handles': their model is the
                // agent in the conversation — same third kind as work-session,
                // not a fourth.
                "hermes:knowledge",
                "hermes:documents",
                "hermes:governance",
                "hermes:google",
                "hermes:research",
                "secrets:handles",
                "channel-plan",
                "plan-doc",
                "outreach:check-in",
                "research-queries",
                "research-search",
                "research-synthesis",
            ]
        );
    }

    #[test]
    fn gives_a_subject_of_the_call_harness_no_chain_to_fall_back_on() {
        // The reason this is an assertion and not a preference: a work-session
        // turn resolved off a fallback chain runs on a model that is not the
        // assigned agent and is still filed to the ticket as that agent's work.
        // Empty means the runner returns "no model available for harness <id>"
        // instead — a sentence an operator can act on. (A fallback would not
        // even serve the fitness suite: it PINS the candidate model, which is
        // its entire question, so it never reads chain steps.)
        for h in builtin_activity_harnesses() {
            // A ROLE-ASSIGNED harness is excluded, and it is the one legitimate
            // exception: 'workbench:*' names a `MODEL_ROLES` role, which an
            // admin assigns exactly like a platform agent — the model is not
            // the subject of the call, so the argument above does not apply.
            if platform_agent_of(h).is_some() || h.def.model.role.is_some() {
                continue;
            }
            assert!(
                h.def.model.chain.is_some_and(|c| c.is_empty()),
                "{} declares a fallback chain, but its model comes from the subject of the call",
                h.def.id
            );
            assert_eq!(
                h.def.model.pin, None,
                "{} declares a pin it can never use",
                h.def.id
            );
            assert_eq!(
                h.def.model.role, None,
                "{} declares a role it can never use",
                h.def.id
            );
        }
    }

    #[test]
    fn keeps_the_judge_exception_in_one_place() {
        // The judge HAS a platform agent but declares no pin: its model lives
        // in judge_config so the Guard panel and the Platform panel cannot
        // disagree.
        let judge = builtin_by_id("judge").expect("the judge is registered");
        assert_eq!(judge.def.model.pin, None);
        assert_eq!(platform_agent_of(judge), Some("judge"));
    }

    #[test]
    fn keeps_the_label_a_harness_shows_and_the_label_its_agent_shows_in_step() {
        // Only where the mapping is one-to-one: 'muse' drives six harnesses and
        // each names its own job ("Muse — ticket edit"), which is right.
        let harnesses = builtin_activity_harnesses();
        for agent in PLATFORM_AGENTS {
            let driven: Vec<&RegisteredHarness> = harnesses
                .iter()
                .filter(|h| platform_agent_of(h) == Some(agent.id))
                .collect();
            if driven.len() != 1 {
                continue;
            }
            assert_eq!(
                driven[0].def.label, agent.label,
                "platform agent \"{}\" and its harness disagree about their own name",
                agent.id
            );
        }
    }
}
