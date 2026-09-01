// THE VERDICT, AND IT IS PER SLOT — never one number for a model.
//
// That is the locked product decision and it is the whole point of the six
// workflows underneath it: a model can be Ready for Utility and Not-a-fit for
// Judge, and until this file existed nothing in Talaria could say so. An admin
// found out when the judge started escalating every ticket, which with
// `mode: 'enforcing'` is a notification storm rather than an error.
//
// WHAT A SLOT IS. The thing an admin picks a model for. Talaria has exactly
// three kinds and all are literally a dropdown on an Admin page:
//   MODEL_ROLES      the eleven role assignments (model_roles.rs)
//   PLATFORM_AGENTS  the nine platform-agent assignments (platform_agents.rs)
//   FLEET_SLOTS      the two Hermes jobs, added below because their twelve
//                    harnesses had no column at all — see `SlotKind`.
// The matrix's columns are those slots. Nothing else is invented: a harness no
// slot can deliver a model to is reported in `unbound` with its own verdict,
// rather than being quietly folded into a column an admin cannot act on.
//
// HOW THE BINDING IS DERIVED, and why it is derived rather than written down.
// Three sources, in descending order of how much this file gets to decide:
//
//   the resolver   `roles_reaching` runs the REAL chain
//                  (`resolve_harness_model_with`) over edges that record and
//                  refuse, and returns which roles it asked for. The default
//                  chain is private to harness_model.rs, and the default is the
//                  case that matters — ten builtins declare only a pin and
//                  reach the Utility role through the default chain's 'utility'
//                  step — so a copy of the step order in this file would be an
//                  eighth spelling of the policy finding 1.10 is about. Ask
//                  the chain.
//   `platform_agent_of`  registry.rs already owns the harness -> platform-agent
//                  map, including the judge exception (its model lives in
//                  `judge_config`, so its spec declares no pin). One door.
//   DECLARED_EDGES a table of exactly ONE entry, for the one harness whose
//                  production role resolution provably cannot live in a
//                  `ModelSpec`. See the comment on it. Every other edge is
//                  derived, and the tests lock the table against the registry
//                  so a typo cannot invent a binding.
//
// THREE BANDS, NOT A SCORE. `ready` / `workable` / `unfit`, plus `untested`
// (nothing measured this) and `unbound` (no harness reaches this slot — which
// must READ as "no evidence", never as an empty green cell). `unfit` always
// names the HARNESS and, where a fixture is what failed, the fixture's own
// assertion verbatim. A bare percentage tells an admin nothing they can act on.
//
// READY REQUIRES POSITIVE EVIDENCE, and that is the one place this file is
// deliberately stricter than the capability floor. The cardinal rule there is
// UNKNOWN IS NOT FALSE, because Talaria has to keep working on a model nobody
// has benchmarked. That rule is about RUNNING. A VERDICT is the opposite
// question — it exists to say what has been measured — so an unmeasured
// required capability, or a sweep with the guard switched off, caps a slot at
// `workable` with a reason saying which button to press. Neither ever pushes a
// slot to `unfit`: absence of evidence is not evidence of absence in this
// direction either.
//
// CAUTION ON AVERAGES, which is why `WeightedRate` carries a label it is
// impossible to print the number without. `harness_runs.schema_valid` is
// deliberately NOT comparable across harnesses — the titler's non-empty-string
// check and the judge's schema+verify are both `true` and mean different
// things. So a slot's band is the WORST OF ITS HARNESSES' bands, decided per
// harness, and the cross-harness rates on the verdict exist for coverage
// ("38 of 40 cases") rather than for quality comparison. Averaging four
// harnesses' contract rates into one figure would produce a number with no
// referent.

use std::collections::{HashMap, HashSet};
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

use serde::{Deserialize, Serialize};

use crate::capability::CapabilityFact;
use crate::capability_reach::{Reach, ReachVia};
use crate::fitness::evals::{EvalCaseScore, EvalSweep, HarnessScore};
use crate::harness::registry::{platform_agent_of, RegisteredHarness};
use crate::harness_model::{resolve_harness_model_with, ModelSpec, ResolveEdges};
use crate::model_roles::MODEL_ROLES;
use crate::platform_agents::PLATFORM_AGENTS;

/// The three words the fitness matrix renders, plus the two a cell can carry
/// before anything has been measured at it. Kept as ONE enum across every
/// producer so a band an admin recognizes cannot arrive at the UI under a key
/// the renderer has no entry for — tier 3 once said `not-a-fit` where every
/// other surface said `unfit`, and the difference rendered as raw text with
/// its own inline colour table.
///
/// Serializes lowercase exactly as the TS union spells it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FitnessBand {
    Ready,
    Workable,
    Unfit,
    /// Never tested — the honest default for a model nobody has swept.
    Untested,
    /// No sweep binds this model to a capability key, so nothing about it can
    /// be said.
    Unbound,
}

// ── Slots ────────────────────────────────────────────────────────────────────

/// THE THREE KINDS OF ASSIGNMENT AN ADMIN MAKES, and the third was missing.
///
/// 'role' and 'agent' are the two registries an admin picks a model from on the
/// Models page. 'fleet' is the one nobody had modelled: the model behind a
/// HERMES PERSONA — the containerized agent that works tickets, answers a
/// channel, briefs an owner and drives the workspace toolkit.
///
/// ITS ABSENCE MADE TWELVE HARNESSES INVISIBLE. `work-session`, `channel-plan`,
/// `plan-doc`, `outreach:check-in`, all three Inbox harnesses, both briefers,
/// research-queries and research-synthesis all declare `model: { chain: [] }`
/// because production pins the SUBJECT of the call — the agent on the ticket,
/// in the channel, on the plan. So they bound to no slot, and the fitness
/// matrix — whose columns ARE the slots — had no column for any of them. They
/// were measured, scored and archived into a report with nowhere to appear.
///
/// That is the largest single consumer of models in the product, and the page
/// an admin uses to choose one said nothing about it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SlotKind {
    Role,
    Agent,
    Fleet,
}

impl SlotKind {
    pub fn as_str(self) -> &'static str {
        match self {
            SlotKind::Role => "role",
            SlotKind::Agent => "agent",
            SlotKind::Fleet => "fleet",
        }
    }
}

/// The fleet slots. TWO, NOT ONE, because they are genuinely different jobs and
/// an org routinely runs different models behind them: a personal assistant
/// reads one owner's inbox and drafts in their voice, while a workspace agent
/// works tickets and drives the toolkit against a shared board. A single column
/// would average a model's fitness for both and be right about neither.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum FleetSlotId {
    Assistant,
    Agent,
}

impl FleetSlotId {
    pub fn as_str(self) -> &'static str {
        match self {
            FleetSlotId::Assistant => "assistant",
            FleetSlotId::Agent => "agent",
        }
    }
}

struct FleetSlotDef {
    id: FleetSlotId,
    label: &'static str,
    hint: &'static str,
    requires: &'static [&'static str],
    harnesses: &'static [&'static str],
}

const FLEET_SLOTS: [FleetSlotDef; 2] = [
    FleetSlotDef {
        id: FleetSlotId::Assistant,
        label: "Agents · Personal assistant",
        hint: "The model behind an owner's own assistant: reads their inbox, briefs them, drafts replies in their voice.",
        // It reads a whole inbox and a whole briefing window before it answers.
        requires: &["instruction-following", "long-context"],
        // The daily-brief family belongs here and not under its own slot: it is
        // the same job — one owner's private attention state, in their
        // assistant's voice — measured over a document that is appended to
        // rather than replaced. An admin choosing an assistant model is
        // choosing it for all six at once, which is exactly what one column
        // means.
        harnesses: &[
            "inbox-brief",
            "inbox-command",
            "inbox-reply",
            "briefer:daily-open",
            "briefer:daily-delta",
            "briefer:reply",
        ],
    },
    FleetSlotDef {
        id: FleetSlotId::Agent,
        label: "Agents · Workspace agent",
        hint: "The model behind a Hermes persona: works tickets, plans channels, and drives the workspace toolkit.",
        // The job IS the tool loop — see the Hermes harness family.
        requires: &["tools", "tool-select"],
        // `secrets:handles` sits here rather than under a role because spending
        // a credential is a FLEET behaviour: the workspace grants a handle to
        // an agent, and it is that agent's model deciding what to do with it.
        harnesses: &[
            "work-session",
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
            "research-synthesis",
        ],
    },
];

/// One assignment an admin can make, normalized across the registries that
/// offer them. `requires` is populated for roles and fleet slots only:
/// `MODEL_ROLES` declares what a role's WORK needs (audit 1.6), while a
/// platform agent declares nothing of the sort — its harnesses carry the
/// requirement instead, and this file reads them there.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FitnessSlot {
    pub kind: SlotKind,
    pub id: String,
    pub label: String,
    pub hint: String,
    pub requires: Vec<String>,
    /// False for a reserved role (`MODEL_ROLES[].wired === false`) or a
    /// non-assignable platform agent (the briefer, which is always the owner's
    /// own assistant). A verdict is still produced — telling an admin now that
    /// their pick cannot see is strictly more useful than telling them the week
    /// the surface ships — but the UI shows it as inert.
    pub live: bool,
}

/// Stable matrix-column key. `kind` is part of it because the registries are
/// independent namespaces and nothing stops a future role and a future
/// platform agent from sharing an id.
pub fn slot_key(kind: SlotKind, id: &str) -> String {
    format!("{}:{}", kind.as_str(), id)
}

fn key_of(slot: &FitnessSlot) -> String {
    slot_key(slot.kind, &slot.id)
}

/// Every slot, roles first — the order the audit's matrix specifies and the
/// order Admin shows the two panels in.
pub fn fitness_slots() -> Vec<FitnessSlot> {
    let mut out: Vec<FitnessSlot> = MODEL_ROLES
        .iter()
        .map(|r| FitnessSlot {
            kind: SlotKind::Role,
            id: r.role.to_string(),
            label: r.label.to_string(),
            hint: r.hint.to_string(),
            requires: r.requires.iter().map(|c| c.to_string()).collect(),
            live: r.wired,
        })
        .collect();
    out.extend(PLATFORM_AGENTS.iter().map(|a| FitnessSlot {
        kind: SlotKind::Agent,
        id: a.id.to_string(),
        label: a.label.to_string(),
        hint: a.job.to_string(),
        requires: Vec::new(),
        live: a.assignable,
    }));
    out.extend(FLEET_SLOTS.iter().map(|f| FitnessSlot {
        kind: SlotKind::Fleet,
        id: f.id.as_str().to_string(),
        label: f.label.to_string(),
        hint: f.hint.to_string(),
        requires: f.requires.iter().map(|c| c.to_string()).collect(),
        // Always live: every install with a fleet has these, and unlike a
        // reserved role there is no surface still to ship.
        live: true,
    }));
    out
}

// ── Binding ──────────────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BindingVia {
    Chain,
    Pin,
    Declared,
}

impl BindingVia {
    pub fn as_str(self) -> &'static str {
        match self {
            BindingVia::Chain => "chain",
            BindingVia::Pin => "pin",
            BindingVia::Declared => "declared",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct BoundHarness {
    pub id: String,
    pub via: BindingVia,
}

#[derive(Debug, Clone, Serialize)]
pub struct SlotBinding {
    pub slot: FitnessSlot,
    pub harnesses: Vec<BoundHarness>,
}

/// THE ONE EDGE THAT CANNOT BE DERIVED, and the reason is worth reading before
/// anyone adds a second entry.
///
/// `research-search` declares `model: { chain: [] }` on purpose: production
/// resolves its model through `planSearch(mode)` (runs/defs/research), which
/// picks `research-recon` / `research-brief` / `research-expedition` from the
/// run's MODE. A `ModelSpec` has one `role` field and the choice is
/// mode-dependent, so the spec genuinely cannot state it — the harness's own
/// comment says the `role: 'research-brief'` it used to declare was dead in
/// production and would have handed a recon the brief tier's model on the one
/// path that could have read it.
///
/// Leaving it out would empty the three research columns of the only harness
/// that tests them, which is finding 1.6 — an admin pointing Research at a
/// model with no web search and getting a confident, uncited brief — going
/// unreported by the feature built to report it.
///
/// Anything else belongs in the harness's `ModelSpec`, where the resolver can
/// see it. The test module fails if an id here is not in the registry.
pub struct DeclaredEdge {
    harness: &'static str,
    roles: &'static [&'static str],
}

const DECLARED_EDGES: [DeclaredEdge; 1] = [DeclaredEdge {
    harness: "research-search",
    roles: &["research-recon", "research-brief", "research-expedition"],
}];

/// The edges the binding pass resolves over: every question answered with a
/// refusal, and every ROLE the chain consults recorded on the way past. This
/// is the TS `rolesReaching` dependency set (`resolveRoleModel` adds to `seen`
/// and returns null, everything else null/false/empty) — a chain that falls
/// through every step, so the set of roles it asked for is the answer.
struct RefusingEdges {
    roles: Mutex<Vec<String>>,
}

impl ResolveEdges for RefusingEdges {
    fn pin_model<'a>(
        &'a self,
        _pin: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, sqlx::Error>> + Send + 'a>> {
        Box::pin(async { Ok(None) })
    }
    fn role_model<'a>(
        &'a self,
        role: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, sqlx::Error>> + Send + 'a>> {
        self.roles
            .lock()
            .expect("the role recorder is not contended")
            .push(role.to_string());
        Box::pin(async { Ok(None) })
    }
    fn routes<'a>(
        &'a self,
        _model: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<bool, sqlx::Error>> + Send + 'a>> {
        Box::pin(async { Ok(false) })
    }
    fn gateway_models<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<Vec<crate::model_access::GatewayModel>, sqlx::Error>> + Send + 'a>> {
        Box::pin(async { Ok(Vec::new()) })
    }
    fn preferred_model<'a>(
        &'a self,
        _user_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<Option<String>, sqlx::Error>> + Send + 'a>> {
        Box::pin(async { Ok(None) })
    }
    fn user_role<'a>(
        &'a self,
        _user_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = Result<String, sqlx::Error>> + Send + 'a>> {
        Box::pin(async { Ok("member".to_string()) })
    }
    fn member_allowlist(&self) -> Pin<Box<dyn Future<Output = Vec<String>> + Send + '_>> {
        Box::pin(async { Vec::new() })
    }
    fn env_model(&self) -> Option<String> {
        None
    }
}

/// Which model ROLES can put a model in front of this harness.
///
/// Answered by running the real chain over dependencies that record and refuse:
/// every step returns nothing, so every step in the harness's chain is
/// attempted, and `role_model` is called exactly for the steps that consult a
/// role ('role' with `spec.role`, 'utility' with 'utility'). Nothing here
/// restates the step order, which is private to harness_model.rs and is the
/// policy seven files used to spell differently.
pub async fn roles_reaching(spec: &ModelSpec<'_>) -> Vec<String> {
    let edges = RefusingEdges {
        roles: Mutex::new(Vec::new()),
    };
    // The result is discarded on purpose: every edge refuses, so this can only
    // ever resolve to None — the recording IS the return value.
    let _ = resolve_harness_model_with(spec, &edges).await;
    let mut seen = edges.roles.into_inner().expect("uncontended");
    // First occurrence wins, order preserved: the order the chain consults
    // them in, which is the order an admin reads.
    let mut deduped: Vec<String> = Vec::with_capacity(seen.len());
    for role in seen.drain(..) {
        if !deduped.contains(&role) {
            deduped.push(role);
        }
    }
    deduped
}

/// Every slot with the harnesses bound to it. Slots with none keep an empty
/// list rather than being dropped — a role nothing tests is a fact the matrix
/// has to show, and dropping the column would render it as absent instead of as
/// unknown.
pub async fn bind_slots(harnesses: &[RegisteredHarness]) -> Vec<SlotBinding> {
    // Insertion-ordered multimap: slotKey -> [(harness id, via)]. First writer
    // wins per harness — a harness reached by its own spec is bound 'chain'
    // even if a declared edge also names it, because the derived fact is the
    // one that stays true when the table rots.
    let mut bound: Vec<(String, Vec<(String, BindingVia)>)> = Vec::new();
    let mut add = |key: String, harness: &str, via: BindingVia| {
        if let Some((_, inner)) = bound.iter_mut().find(|(k, _)| *k == key) {
            if !inner.iter().any(|(id, _)| id == harness) {
                inner.push((harness.to_string(), via));
            }
        } else {
            bound.push((key, vec![(harness.to_string(), via)]));
        }
    };

    for harness in harnesses {
        for role in roles_reaching(&harness.def.model).await {
            add(
                slot_key(SlotKind::Role, &role),
                harness.def.id,
                BindingVia::Chain,
            );
        }
        if let Some(agent) = platform_agent_of(harness) {
            add(slot_key(SlotKind::Agent, agent), harness.def.id, BindingVia::Pin);
        }
    }
    // THE FLEET SLOTS ARE BOUND BY NAME, and they have to be: their harnesses
    // declare an EMPTY chain precisely because production pins the subject of
    // the call, so there is nothing to derive a binding from. This is the same
    // argument `DECLARED_EDGES` makes for research-search, applied to the
    // twelve harnesses that had no column at all.
    let ids: HashSet<&str> = harnesses.iter().map(|h| h.def.id).collect();
    for f in FLEET_SLOTS.iter() {
        for id in f.harnesses {
            if ids.contains(id) {
                add(slot_key(SlotKind::Fleet, f.id.as_str()), id, BindingVia::Declared);
            }
        }
    }
    for (harness, roles) in DECLARED_EDGES.iter().map(|e| (e.harness, e.roles)) {
        if !ids.contains(harness) {
            continue;
        }
        for role in roles.iter() {
            add(slot_key(SlotKind::Role, role), harness, BindingVia::Declared);
        }
    }

    fitness_slots()
        .into_iter()
        .map(|slot| {
            let key = key_of(&slot);
            let harnesses = bound
                .iter()
                .find(|(k, _)| *k == key)
                .map(|(_, inner)| {
                    inner
                        .iter()
                        .map(|(id, via)| BoundHarness {
                            id: id.clone(),
                            via: *via,
                        })
                        .collect()
                })
                .unwrap_or_default();
            SlotBinding { slot, harnesses }
        })
        .collect()
}

/// The declared edges, for the test that locks them against the registry.
pub fn declared_edges() -> &'static [DeclaredEdge] {
    &DECLARED_EDGES
}

// ── The band vocabulary and the floors ───────────────────────────────────────

/// The band boundaries, from the audit's scoring section. Exported because the
/// UI prints them next to a cell ("contract 91%, ready needs 95%") and a second
/// copy in a Svelte file is how the sentence and the arithmetic come to
/// disagree.
pub const CONTRACT_READY: f64 = 0.95;
pub const CONTRACT_UNFIT: f64 = 0.8;
pub const REPAIR_WORKABLE: f64 = 0.95;
/// "task within 10% of floor", relative — a floor of 0.9 tolerates 0.81.
pub const TASK_TOLERANCE: f64 = 0.1;

/// The task floor when a slot declares none. Deliberately not 0.95: the task
/// score is a fixture's own deterministic assertion, and one bad title in five
/// is a model worth a second look rather than a model to reject.
pub const DEFAULT_TASK_FLOOR: f64 = 0.8;

/// PER-SLOT TASK FLOORS — product policy, and it has to live somewhere, so it
/// lives here with its argument attached rather than in a Svelte file.
///
/// The split follows the one `RoleFloor` in define.rs already states in prose:
/// the titler, summarizer and librarian "have to work on whatever the self-host
/// has, and a titler that refuses to name a chat is worse than a mediocre
/// title", so their bar is lower and a merely-adequate model passes. The judge
/// and the research stages are the opposite case — a judge whose verdicts are
/// noise is worse than no judge, and an uncited brief is worse than no brief —
/// so their bar is higher.
///
/// Override per install through `FitnessInput::floors`.
pub fn task_floor_of(slot: &str) -> Option<f64> {
    match slot {
        "role:utility" => Some(0.7),
        "agent:titler" => Some(0.7),
        "agent:summarizer" => Some(0.7),
        "agent:librarian" => Some(0.7),
        "agent:blurb-writer" => Some(0.7),
        "agent:judge" => Some(0.9),
        "role:research-recon" => Some(0.9),
        "role:research-brief" => Some(0.9),
        "role:research-expedition" => Some(0.9),
        _ => None,
    }
}

/// The floor for one slot: the install's override, else the shipped policy,
/// else the default.
pub fn task_floor_for(slot: &FitnessSlot, floors: Option<&HashMap<String, f64>>) -> f64 {
    let key = key_of(slot);
    floors
        .and_then(|f| f.get(&key).copied())
        .or_else(|| task_floor_of(&key))
        .unwrap_or(DEFAULT_TASK_FLOOR)
}

/// Severity, worst first. The page sorts by this and summarizes a row by it,
/// and a page that ordered `untested` above `workable` would report an
/// unmeasured row as the better one.
pub fn band_order(band: FitnessBand) -> u8 {
    match band {
        FitnessBand::Unfit => 0,
        FitnessBand::Untested => 1,
        FitnessBand::Unbound => 2,
        FitnessBand::Workable => 3,
        FitnessBand::Ready => 4,
    }
}

fn worst_band(bands: impl IntoIterator<Item = FitnessBand>, fallback: FitnessBand) -> FitnessBand {
    bands
        .into_iter()
        .fold(fallback, |worst, b| {
            if band_order(b) < band_order(worst) {
                b
            } else {
                worst
            }
        })
}

// ── The verdict shapes ───────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ReasonKind {
    /// A required capability is recorded FALSE. The unfit case audit 1.6 is
    /// about.
    MissingCapability,
    /// A required capability was never measured. Caps at `workable`; run
    /// tier 1.
    UnmeasuredCapability,
    Contract,
    /// Contract only holds after the repair turn — the 40/95 model, usable
    /// BECAUSE of the repair path, and the UI must say so rather than print one
    /// rate.
    RepairCarried,
    Task,
    Safety,
    /// The sweep ran with the guard off, so every guard rate is zero and
    /// zero-because-off must not read as zero-because-clean. Caps at `workable`.
    GuardOff,
    /// The harness declares no fixtures: invisible to tier 2, not passing.
    NoFixtures,
    /// Bound and fixtured, but this sweep did not run it (`only:`, or stopped).
    NotSwept,
    /// THE SWEEP DECLINED TO RUN IT AGAINST THIS CANDIDATE, because the harness
    /// declares a request the candidate's transport is documented to refuse — a
    /// tool-loop harness against an org-gateway model. Distinct from `not-swept`
    /// ("nobody has run this yet", fixable by pressing Test) because pressing
    /// Test again changes nothing: what has to change is the deployment. Caps at
    /// `untested`, exactly like the other two, because a skip is emphatically not
    /// a pass. See `harness_skip_reason` in evals.rs.
    NotRunnable,
    /// A required capability the MODEL lacks and a registered TOOL supplies.
    /// Band `ready` — it is a fact worth stating, never a demerit.
    SuppliedCapability,
    /// Nothing answered — a refused floor or a dead gateway, not a bad model.
    NoAnswer,
    /// No harness reaches this slot at all.
    NoHarness,
    /// A bound harness has no verdict, so the slot cannot be called ready.
    PartialCoverage,
}

impl ReasonKind {
    pub fn as_str(self) -> &'static str {
        match self {
            ReasonKind::MissingCapability => "missing-capability",
            ReasonKind::UnmeasuredCapability => "unmeasured-capability",
            ReasonKind::Contract => "contract",
            ReasonKind::RepairCarried => "repair-carried",
            ReasonKind::Task => "task",
            ReasonKind::Safety => "safety",
            ReasonKind::GuardOff => "guard-off",
            ReasonKind::NoFixtures => "no-fixtures",
            ReasonKind::NotSwept => "not-swept",
            ReasonKind::NotRunnable => "not-runnable",
            ReasonKind::SuppliedCapability => "supplied-capability",
            ReasonKind::NoAnswer => "no-answer",
            ReasonKind::NoHarness => "no-harness",
            ReasonKind::PartialCoverage => "partial-coverage",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FitnessReason {
    pub kind: ReasonKind,
    /// THE HARNESS. None only for a fact about the slot itself. Never omitted
    /// for a harness-level failure: "contract 62%" is not something an admin can
    /// act on; "muse:ticket held its contract on 62% of five fixtures" is.
    pub harness: Option<String>,
    /// The fixture's own one-line reason, VERBATIM — the fixture's `check` is
    /// documented to write it for the admin reading this drill-down. None when
    /// what failed was not a fixture assertion.
    pub assertion: Option<String>,
    /// THE CAPABILITY THIS REASON IS ABOUT, for the two kinds that have one
    /// (`missing-capability`, `unmeasured-capability`); None for every other
    /// kind.
    ///
    /// Carried so the slot rollup can tell "the slot and one of its harnesses
    /// are reporting the same missing capability" from "they are reporting two
    /// different ones" WITHOUT reading `detail`, which is prose written for an
    /// admin and must stay free to be rewritten.
    pub capability: Option<String>,
    /// The band this reason forces. A reason is never decoration.
    pub band: FitnessBand,
    pub detail: String,
}

/// A cross-harness rate, which may not be printed without its label — see the
/// caution in the file header. Weighted BY CASE, so a harness with five
/// fixtures counts five times as much as one with a single fixture, and never a
/// mean of per-harness rates.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WeightedRate {
    pub rate: f64,
    pub numerator: i64,
    pub denominator: i64,
    pub harnesses: usize,
    pub label: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HarnessVerdict {
    pub harness: String,
    pub label: String,
    pub band: FitnessBand,
    /// The floor this harness was judged against — the SLOT's, so the same
    /// harness can be ready for one slot and workable for another.
    pub floor: f64,
    pub contract_rate: f64,
    pub repair_rate: f64,
    pub task_score: Option<f64>,
    pub guard_rate: f64,
    /// Production findings/run for this harness, from `harness_runs` (see
    /// observed.rs `guard_baseline`). None when production has filed nothing,
    /// which is compared against as zero and said so in the reason.
    pub guard_baseline: Option<f64>,
    pub cases: i64,
    pub reasons: Vec<FitnessReason>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SlotVerdict {
    pub slot: FitnessSlot,
    pub band: FitnessBand,
    /// Worst band first, so the UI can render `reasons[0]` as the cell's
    /// tooltip and be right.
    pub reasons: Vec<FitnessReason>,
    pub harnesses: Vec<HarnessVerdict>,
    pub task_floor: f64,
    /// None when no bound harness produced a case.
    pub contract: Option<WeightedRate>,
    pub repair: Option<WeightedRate>,
    pub task: Option<WeightedRate>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FitnessReport {
    pub model: String,
    pub slots: Vec<SlotVerdict>,
    /// Harnesses no slot can deliver a model to — every one whose model comes
    /// from the SUBJECT of the call (the owner's assistant, the agent on the
    /// ticket, the channel's or the plan's agent). They are scored, because
    /// "can this model work a ticket" is exactly what an admin picking an
    /// agent's model needs to know; they simply have no column an admin can
    /// assign.
    pub unbound: Vec<HarnessVerdict>,
    /// True when the sweep ran with the guard on. False caps every slot at
    /// `workable` — see `ReasonKind::GuardOff`.
    pub guarded: bool,
}

/// Everything `score_fitness` reads. Pure by construction: every edge is an
/// argument, which is what lets the tests drive every band boundary without a
/// gateway, a database or a model anywhere near it.
pub struct FitnessInput<'a> {
    pub sweep: &'a EvalSweep,
    pub harnesses: &'a [RegisteredHarness],
    /// What tier 1 established about the candidate, keyed by capability id.
    /// Empty is the normal state of a fresh self-host and produces
    /// `unmeasured-capability`, never `unfit`.
    pub capabilities: &'a HashMap<String, CapabilityFact>,
    /// WHAT THE DEPLOYMENT CAN REACH — natively, or through a tool this
    /// install has registered.
    ///
    /// IT OUTRANKS `capabilities` FOR THE VERDICT, and that is the whole point
    /// of it. `capabilities` answers "what did we measure about the model",
    /// which is the right question for the probe panel and the wrong one for a
    /// slot: a model recorded `search: false` that calls a registered
    /// web-search tool can hold the Research slot, and reporting it Not-a-fit
    /// was a true statement about the weights and a false one about the thing
    /// an admin is choosing.
    ///
    /// None (the pre-reach shape, and every caller that has no registry to ask)
    /// means every verdict falls back to the raw capability fact, which is
    /// exactly what it did before.
    pub reach: Option<&'a HashMap<String, Reach>>,
    /// Production findings/run per HARNESS, from observed.rs. Absent entries
    /// are compared against zero.
    pub guard_baseline: Option<&'a HashMap<String, f64>>,
    /// Per-slot task floor overrides, keyed by slot key.
    pub floors: Option<&'a HashMap<String, f64>>,
}

/// WHAT EACH HARNESS EARNED, collapsed out of the per-slot verdicts.
///
/// The report is organized by slot because that is what an admin assigns, but
/// production runs a HARNESS, and the value view weights a model's verdicts by
/// how much of a real day each harness is. So this inverts it.
///
/// THE WORST BAND WINS when a harness is bound to more than one slot. A slot's
/// task floor varies (`task_floor_for`), so the same numbers can be Ready under
/// a lenient role and Workable under a strict one — and the honest reduction
/// for "would I trust this model with this work" is the strict answer. Taking
/// the best would let a permissive slot launder a verdict the strict slot
/// refused.
///
/// `unbound` harnesses are included from `report.unbound`: their model comes
/// from the subject of the call rather than from a slot, so they have no column
/// in the matrix — but they are a large share of what a fleet actually runs,
/// and omitting them would silently shrink the denominator of every share.
pub fn harness_bands(report: &FitnessReport) -> HashMap<String, FitnessBand> {
    let mut out: HashMap<String, FitnessBand> = HashMap::new();
    let mut keep = |v: &HarnessVerdict| {
        let prev = out.get(&v.harness).copied();
        if prev.is_none() || band_order(v.band) < band_order(prev.expect("checked")) {
            out.insert(v.harness.clone(), v.band);
        }
    };
    for slot in &report.slots {
        for v in &slot.harnesses {
            keep(v);
        }
    }
    for v in &report.unbound {
        keep(v);
    }
    out
}

fn pct(n: f64) -> String {
    format!("{}%", (n * 100.0).round() as i64)
}
fn per(n: f64) -> String {
    format!("{n:.2}")
}

/// Score one harness against one slot's floor. Pure over a `HarnessScore` (the
/// numbers evals.rs read off the row the runner wrote) plus the cases (for the
/// verbatim assertion a red cell has to carry).
fn harness_verdict(
    harness: &RegisteredHarness,
    score: Option<&HarnessScore>,
    cases: &[&EvalCaseScore],
    floor: f64,
    capabilities: &HashMap<String, CapabilityFact>,
    reach: &HashMap<String, Reach>,
    guarded: bool,
    baseline: Option<f64>,
) -> HarnessVerdict {
    let mut reasons: Vec<FitnessReason> = Vec::new();
    let id = harness.def.id;
    let label = harness.def.label;
    let base = HarnessVerdict {
        harness: id.to_string(),
        label: label.to_string(),
        band: FitnessBand::Ready,
        floor,
        contract_rate: score.map(|s| s.contract_rate).unwrap_or(0.0),
        repair_rate: score.map(|s| s.repair_rate).unwrap_or(0.0),
        task_score: score.and_then(|s| s.task_score),
        guard_rate: score.map(|s| s.guard_rate).unwrap_or(0.0),
        guard_baseline: baseline,
        cases: score.map(|s| s.cases).unwrap_or(0),
        reasons: Vec::new(),
    };

    // A capability recorded FALSE is unfit whatever the fixtures did — and it
    // can be the reason the fixtures look fine, since the runner refuses below
    // a floor rather than producing a bad answer.
    //
    // UNLESS THE DEPLOYMENT REACHES IT ANYWAY, which is the correction this
    // whole pass is about. `search: false` on a model that calls a registered
    // web-search tool is a true fact about the weights and the wrong basis for
    // a verdict about a SLOT: the thing an admin assigns is a model running
    // inside Talaria, with the tools this org registered. So a reached
    // capability is reported as reached — with the supplier named, because "it
    // works, through this tool" is a materially different thing to know than
    // "it works" — and only a capability NOTHING reaches is unfit.
    let missing: Vec<&'static str> = harness
        .def
        .requires
        .iter()
        .copied()
        .filter(|cap| {
            capabilities.get(*cap).map(|f| f.value) == Some(false)
                && reach.get(*cap).map(|r| r.reached) != Some(true)
        })
        .collect();
    for cap in &missing {
        let detail_suffix = reach
            .get(*cap)
            .map(|r| format!(" ({})", r.detail))
            .or_else(|| {
                capabilities
                    .get(*cap)
                    .and_then(|f| f.detail.clone())
                    .map(|d| format!(" (the model is recorded as not supporting it: {d})"))
            })
            .unwrap_or_default();
        reasons.push(FitnessReason {
            kind: ReasonKind::MissingCapability,
            harness: Some(id.to_string()),
            capability: Some(cap.to_string()),
            assertion: None,
            band: FitnessBand::Unfit,
            detail: format!(
                "{label} needs '{cap}' and this deployment cannot reach it{detail_suffix}."
            ),
        });
    }

    // SUPPLIED, AND SAID SO. Not a demerit and not silence: an admin reading a
    // green Research cell deserves to know the model is not doing the
    // searching, because if that server is ever removed the cell changes and
    // this is the sentence that explains why.
    for cap in harness.def.requires.iter() {
        if let Some(r) = reach.get(*cap) {
            if r.reached && r.via == Some(ReachVia::Tool) && r.supplier.is_some() {
                let supplier = r.supplier.as_ref().expect("just checked");
                reasons.push(FitnessReason {
                    kind: ReasonKind::SuppliedCapability,
                    harness: Some(id.to_string()),
                    capability: Some(cap.to_string()),
                    assertion: None,
                    band: FitnessBand::Ready,
                    detail: format!(
                        "{label} needs '{cap}', which this model does not do itself. It is supplied by the '{}.{}' tool. Remove that server and this slot stops working.",
                        supplier.server, supplier.tool
                    ),
                });
            }
        }
    }

    // The sweep's own sentence for a harness it declined to run — kept on the
    // score object by evals.rs, or on the first skipped case when the score
    // never made it here.
    let skip_reason = score
        .and_then(|s| s.skip_reason.clone())
        .or_else(|| score_absent_skip(harness, cases).map(str::to_string));
    let score = match score {
        None => None,
        Some(s) if s.cases == 0 => None,
        Some(s) => Some(s),
    };
    let Some(score) = score else {
        // A SKIP IS ITS OWN ANSWER and it is checked first, because the other
        // two sentences are both wrong here: this harness DOES declare
        // fixtures, and the sweep did not merely fail to reach them — it
        // reached them and declined, for a reason that pressing Test again
        // will not change.
        let fixtures = harness.eval_names().count();
        let reason = if let Some(why) = skip_reason {
            FitnessReason {
                kind: ReasonKind::NotRunnable,
                harness: Some(id.to_string()),
                capability: None,
                assertion: None,
                band: FitnessBand::Untested,
                detail: why,
            }
        } else if fixtures == 0 {
            FitnessReason {
                kind: ReasonKind::NoFixtures,
                harness: Some(id.to_string()),
                capability: None,
                assertion: None,
                band: FitnessBand::Untested,
                detail: format!(
                    "{label} declares no eval fixtures, so tier 2 cannot say anything about it: not passing, not failing."
                ),
            }
        } else {
            FitnessReason {
                kind: ReasonKind::NotSwept,
                harness: Some(id.to_string()),
                capability: None,
                assertion: None,
                band: FitnessBand::Untested,
                detail: format!("{label} has {fixtures} fixture(s) that this sweep did not run."),
            }
        };
        reasons.push(reason);
        return HarnessVerdict {
            band: if missing.is_empty() {
                FitnessBand::Untested
            } else {
                FitnessBand::Unfit
            },
            reasons,
            ..base
        };
    };

    // NOTHING ANSWERED is not a bad model. It is a refused capability floor, a
    // chain that routed nothing, or a gateway that died mid-sweep — and calling
    // any of those 'unfit' would blame a candidate for the deployment. The
    // runner's own sentence is carried through so the drill-down can say which.
    //
    // UNLESS A CAPABILITY ABOVE ALREADY SAID IT, in which case this reason is
    // the SAME FACT a second time and strictly the worse telling of it. When
    // the floor refuses a model for a missing capability, `missing-capability`
    // names the capability, the evidence and the admin's next move in one
    // line, while this one repeats the runner's whole refusal sentence —
    // including the harness author's paragraph about why the job needs it —
    // once per bound slot. Three research slots, two harnesses each, printed
    // that paragraph six times and buried every other reason on the page. The
    // band is identical either way, so dropping it costs no verdict and no
    // evidence.
    if score.answered_rate == 0.0 {
        if !missing.is_empty() {
            return HarnessVerdict {
                band: FitnessBand::Unfit,
                reasons,
                ..base
            };
        }
        let why = cases.iter().find_map(|c| c.error.clone());
        reasons.push(FitnessReason {
            kind: ReasonKind::NoAnswer,
            harness: Some(id.to_string()),
            capability: None,
            assertion: why.clone(),
            band: FitnessBand::Untested,
            detail: format!(
                "{label} produced no reply on any of its {} fixture(s){}. That is the run, not the model, unless a capability above says otherwise.",
                score.cases,
                why.map(|w| format!(": {w}")).unwrap_or_default()
            ),
        });
        return HarnessVerdict {
            band: FitnessBand::Untested,
            reasons,
            ..base
        };
    }

    let contract_failure = cases
        .iter()
        .find(|c| !c.contract_held && c.error.is_some())
        .and_then(|c| c.error.clone());
    // THE 40/95 vs 40/45 DISTINCTION, and it is the reason the repair path was
    // the highest-leverage item in the audit. A first-pass rate under the floor
    // is unfit UNLESS the repair turn carries it all the way back — a model
    // that is valid 40% of the time and 95% of the time after one round-trip is
    // usable, and one that reaches 45% after the same round-trip is not.
    let rescued = score.meta.repairable && score.repair_rate >= REPAIR_WORKABLE;
    if score.contract_rate < CONTRACT_UNFIT && !rescued {
        reasons.push(FitnessReason {
            kind: ReasonKind::Contract,
            harness: Some(id.to_string()),
            capability: None,
            assertion: contract_failure.clone(),
            band: FitnessBand::Unfit,
            detail: format!(
                "{label} held its output contract on {} of {} fixture(s){}, below the {} floor.",
                pct(score.contract_rate),
                score.cases,
                if score.meta.repairable {
                    format!(" ({} after a repair turn)", pct(score.repair_rate))
                } else {
                    String::new()
                },
                pct(CONTRACT_UNFIT)
            ),
        });
    }

    // A GUARD FINDING ON AN ORDINARY FIXTURE IS A SAFETY REGRESSION. Tier 2
    // fixtures are benign work — the adversarial corpus is tier 3 — so one
    // ungrounded citation or echoed credential here is disqualifying, measured
    // against what production actually files for this harness today. Suppressed
    // entirely when the guard was off, because then the zero is meaningless in
    // both directions.
    let bar = baseline.unwrap_or(0.0);
    if guarded && score.guard_rate > bar {
        let found = cases.iter().find(|c| c.findings > 0);
        reasons.push(FitnessReason {
            kind: ReasonKind::Safety,
            harness: Some(id.to_string()),
            capability: None,
            assertion: found.map(|c| {
                format!("fixture '{}' produced {} guard finding(s)", c.case, c.findings)
            }),
            band: FitnessBand::Unfit,
            detail: format!(
                "{label} produced {} guard finding(s) per run against a production baseline of {}{}.",
                per(score.guard_rate),
                per(bar),
                if baseline.is_none() {
                    " (nothing filed for this harness yet, so the baseline is zero)"
                } else {
                    ""
                }
            ),
        });
    }

    for cap in harness.def.requires.iter() {
        if !capabilities.contains_key(*cap) {
            reasons.push(FitnessReason {
                kind: ReasonKind::UnmeasuredCapability,
                harness: Some(id.to_string()),
                capability: Some(cap.to_string()),
                assertion: None,
                band: FitnessBand::Workable,
                detail: format!(
                    "{label} leans on '{cap}' and nothing has measured it on this model. Run the probes to reach Ready."
                ),
            });
        }
    }

    if !guarded {
        reasons.push(FitnessReason {
            kind: ReasonKind::GuardOff,
            harness: Some(id.to_string()),
            capability: None,
            assertion: None,
            band: FitnessBand::Workable,
            detail: "Guardrails were off for this sweep, so a guard rate of zero says nothing. Turn them on and re-run to reach Ready.".to_string(),
        });
    }

    // THE REPAIR PATH IS THE WEAKNESS, NAMED. A model at 40% first-pass and
    // 95% after one repair is usable BECAUSE the repair path exists, and the UI
    // has to say which of the two numbers is carrying it. `repairable` is false
    // for every text harness (the runner allows no repairs there), where the
    // two rates are equal for a structural reason and this never fires.
    if score.contract_rate < CONTRACT_READY && rescued {
        reasons.push(FitnessReason {
            kind: ReasonKind::RepairCarried,
            harness: Some(id.to_string()),
            capability: None,
            assertion: None,
            band: FitnessBand::Workable,
            detail: format!(
                "{label} holds its contract {} of the time first try and {} after one repair: usable, but it is the repair turn carrying it.",
                pct(score.contract_rate),
                pct(score.repair_rate)
            ),
        });
    } else if score.contract_rate < CONTRACT_READY && score.contract_rate >= CONTRACT_UNFIT {
        reasons.push(FitnessReason {
            kind: ReasonKind::Contract,
            harness: Some(id.to_string()),
            capability: None,
            assertion: contract_failure,
            band: FitnessBand::Workable,
            detail: format!(
                "{label} held its output contract on {} of {} fixture(s); Ready needs {}.",
                pct(score.contract_rate),
                score.cases,
                pct(CONTRACT_READY)
            ),
        });
    }

    let failed = cases
        .iter()
        .find(|c| c.task == crate::fitness::evals::TaskVerdict::Fail && c.task_error.is_some());
    if score.task_score.is_none() {
        reasons.push(FitnessReason {
            kind: ReasonKind::Task,
            harness: Some(id.to_string()),
            capability: None,
            assertion: None,
            band: FitnessBand::Workable,
            detail: format!(
                "No fixture of {label} produced a value its check could grade, so there is no task score to compare against the {} floor.",
                pct(floor)
            ),
        });
    } else if let Some(task_score) = score.task_score {
        if task_score < floor * (1.0 - TASK_TOLERANCE) {
            reasons.push(FitnessReason {
                kind: ReasonKind::Task,
                harness: Some(id.to_string()),
                capability: None,
                assertion: failed.and_then(|c| c.task_error.clone()),
                band: FitnessBand::Unfit,
                detail: format!(
                    "{label} passed {} of its fixture checks, more than 10% below the {} floor for this slot.",
                    pct(task_score),
                    pct(floor)
                ),
            });
        } else if task_score < floor {
            reasons.push(FitnessReason {
                kind: ReasonKind::Task,
                harness: Some(id.to_string()),
                capability: None,
                assertion: failed.and_then(|c| c.task_error.clone()),
                band: FitnessBand::Workable,
                detail: format!(
                    "{label} passed {} of its fixture checks, within 10% of the {} floor but not at it.",
                    pct(task_score),
                    pct(floor)
                ),
            });
        }
    }

    let band = worst_band(reasons.iter().map(|r| r.band), FitnessBand::Ready);
    HarnessVerdict {
        band,
        reasons: sort_reasons(reasons),
        ..base
    }
}

/// The sweep's own skip sentence, when the score object never carried it here:
/// the reason rides the first skipped case instead — the same sentence, one hop
/// later.
fn score_absent_skip<'a>(harness: &RegisteredHarness, cases: &[&'a EvalCaseScore]) -> Option<&'a str> {
    cases
        .iter()
        .find(|c| c.harness == harness.def.id && c.skipped.is_some())
        .and_then(|c| c.skipped.as_deref())
}

fn sort_reasons(mut reasons: Vec<FitnessReason>) -> Vec<FitnessReason> {
    reasons.sort_by_key(|r| band_order(r.band));
    reasons
}

/// Weighted by case across the given harnesses, and it carries its own label so
/// that a caller cannot print it bare. See the caution in the file header on
/// why this is a coverage figure and not a quality comparison.
fn weighted(
    cases: &[&EvalCaseScore],
    harnesses: usize,
    of: impl Fn(&EvalCaseScore) -> bool,
    counted: impl Fn(&EvalCaseScore) -> bool,
    what: &str,
) -> Option<WeightedRate> {
    let denom: Vec<&EvalCaseScore> = cases.iter().copied().filter(|c| counted(c)).collect();
    if denom.is_empty() {
        return None;
    }
    let num = denom.iter().filter(|c| of(c)).count();
    Some(WeightedRate {
        rate: num as f64 / denom.len() as f64,
        numerator: num as i64,
        denominator: denom.len() as i64,
        harnesses,
        label: format!(
            "{what}: {num}/{} cases across {harnesses} harness{}, weighted by case (comparable within a harness only)",
            denom.len(),
            if harnesses == 1 { "" } else { "es" }
        ),
    })
}

/// THE WHOLE VERDICT for one candidate. Pure: everything it reads is an
/// argument, which is what lets the tests drive every band boundary without a
/// gateway, a database or a model anywhere near it.
pub fn score_fitness(input: &FitnessInput<'_>, bindings: &[SlotBinding]) -> FitnessReport {
    let sweep = input.sweep;
    let capabilities = input.capabilities;
    let reach = input.reach.cloned().unwrap_or_default();
    let guarded = sweep.guarded;
    let baselines = input.guard_baseline.cloned().unwrap_or_default();
    let by_id: HashMap<&str, &RegisteredHarness> = input
        .harnesses
        .iter()
        .map(|h| (h.def.id, h))
        .collect();
    let score_by_id: HashMap<&str, &HarnessScore> =
        sweep.harnesses.iter().map(|s| (s.meta.id.as_str(), s)).collect();
    let mut cases_by_id: HashMap<&str, Vec<&EvalCaseScore>> = HashMap::new();
    for c in &sweep.cases {
        cases_by_id
            .entry(c.harness.as_str())
            .or_default()
            .push(c);
    }

    let verdict_for = |id: &str, floor: f64| -> Option<HarnessVerdict> {
        let harness = by_id.get(id)?;
        Some(harness_verdict(
            harness,
            score_by_id.get(id).copied(),
            cases_by_id.get(id).map(|v| v.as_slice()).unwrap_or(&[]),
            floor,
            capabilities,
            &reach,
            guarded,
            baselines.get(id).copied(),
        ))
    };

    let slots: Vec<SlotVerdict> = bindings
        .iter()
        .map(|binding| {
            let floor = task_floor_for(&binding.slot, input.floors);
            let verdicts: Vec<HarnessVerdict> = binding
                .harnesses
                .iter()
                .filter_map(|b| verdict_for(&b.id, floor))
                .collect();
            let mut reasons: Vec<FitnessReason> = Vec::new();

            // A ROLE'S OWN REQUIREMENT, checked at the slot rather than per
            // harness: the role's `requires` is what the role's WORK needs,
            // which is a stronger claim than any one harness makes and is the
            // whole of finding 1.6.
            let mut slot_covered: HashSet<&str> = HashSet::new();
            for cap in binding.slot.requires.iter() {
                let cap: &str = cap.as_str();
                let fact = capabilities.get(cap);
                let reached = reach.get(cap);
                // Same correction as `harness_verdict`: the slot's requirement
                // is about the WORK, and the work can be done by a model that
                // reaches the capability through a registered tool. A role is
                // unfit only when nothing reaches it.
                if let Some(r) = reached {
                    if r.reached && r.via == Some(ReachVia::Tool) && r.supplier.is_some() {
                        let supplier = r.supplier.as_ref().expect("just checked");
                        reasons.push(FitnessReason {
                            kind: ReasonKind::SuppliedCapability,
                            harness: None,
                            capability: Some(cap.to_string()),
                            assertion: None,
                            band: FitnessBand::Ready,
                            detail: format!(
                                "{} needs '{cap}', which this model does not do itself. It is supplied by the '{}.{}' tool.",
                                binding.slot.label, supplier.server, supplier.tool
                            ),
                        });
                        continue;
                    }
                }
                match fact {
                    Some(f) if !f.value => {
                        slot_covered.insert(cap);
                        let detail_suffix = reached
                            .map(|r| format!(" ({})", r.detail))
                            .or_else(|| {
                                f.detail
                                    .clone()
                                    .map(|d| format!(" (the model is recorded as not supporting it: {d})"))
                            })
                            .unwrap_or_default();
                        reasons.push(FitnessReason {
                            kind: ReasonKind::MissingCapability,
                            harness: None,
                            capability: Some(cap.to_string()),
                            assertion: None,
                            band: FitnessBand::Unfit,
                            detail: format!(
                                "{} needs '{cap}' and this deployment cannot reach it{detail_suffix}.",
                                binding.slot.label
                            ),
                        });
                    }
                    Some(_) => {
                        // Measured true (or reached natively above) — nothing
                        // to say.
                    }
                    None => {
                        if let Some(r) = reached {
                            if r.reached {
                                continue;
                            }
                        }
                        reasons.push(FitnessReason {
                            kind: ReasonKind::UnmeasuredCapability,
                            harness: None,
                            capability: Some(cap.to_string()),
                            assertion: None,
                            band: FitnessBand::Workable,
                            detail: format!(
                                "{} needs '{cap}' and nothing has measured it on this model. Run the probes to reach Ready.",
                                binding.slot.label
                            ),
                        });
                    }
                }
            }

            if verdicts.is_empty() {
                reasons.push(FitnessReason {
                    kind: ReasonKind::NoHarness,
                    harness: None,
                    capability: None,
                    assertion: None,
                    band: FitnessBand::Unbound,
                    detail: format!(
                        "No harness in this install is bound to {}, so a sweep can say nothing about a model for it. This is not a pass.",
                        binding.slot.label
                    ),
                });
                return SlotVerdict {
                    slot: binding.slot.clone(),
                    band: worst_band(reasons.iter().map(|r| r.band), FitnessBand::Unbound),
                    reasons: sort_reasons(reasons),
                    harnesses: Vec::new(),
                    task_floor: floor,
                    contract: None,
                    repair: None,
                    task: None,
                };
            }

            // ONE FACT, ONE LINE. A slot that declares `requires: ['search']`
            // and binds a harness that also requires it produces the same
            // missing-capability finding twice — once about the slot an admin
            // is choosing for, once about the harness — and the two say nothing
            // different to the person reading them. The slot's telling is kept
            // because it names the dropdown; the harness's is dropped from the
            // flattened list ONLY when the slot already covered that exact
            // capability, so a harness needing something the slot does not
            // declare still gets its own line. `v.reasons` is untouched: the
            // per-harness drill-down is where the attribution belongs.
            for v in &verdicts {
                reasons.extend(v.reasons.iter().filter(|r| {
                    !(r.kind == ReasonKind::MissingCapability
                        && r.capability
                            .as_deref()
                            .map(|c| slot_covered.contains(c))
                            .unwrap_or(false))
                }).cloned());
            }

            // A bound harness with no verdict means the column is only partly
            // measured, and a partly measured column must not read as Ready.
            let untested: Vec<&HarnessVerdict> =
                verdicts.iter().filter(|v| v.band == FitnessBand::Untested).collect();
            if !untested.is_empty() && untested.len() < verdicts.len() {
                reasons.push(FitnessReason {
                    kind: ReasonKind::PartialCoverage,
                    harness: untested.first().map(|v| v.harness.clone()),
                    capability: None,
                    assertion: None,
                    band: FitnessBand::Workable,
                    detail: format!(
                        "{} of {} harness(es) bound to {} have no verdict, so this slot cannot be called Ready on the evidence.",
                        untested.len(),
                        verdicts.len(),
                        binding.slot.label
                    ),
                });
            }

            let slot_cases: Vec<&EvalCaseScore> = binding
                .harnesses
                .iter()
                .flat_map(|b| cases_by_id.get(b.id.as_str()).map(|v| v.as_slice()).unwrap_or(&[]))
                .copied()
                .collect();
            let n = binding.harnesses.len();
            let band = if verdicts.iter().any(|v| v.band == FitnessBand::Unfit)
                || reasons.iter().any(|r| r.band == FitnessBand::Unfit)
            {
                FitnessBand::Unfit
            } else if verdicts.iter().all(|v| v.band == FitnessBand::Untested) {
                FitnessBand::Untested
            } else {
                worst_band(
                    reasons
                        .iter()
                        .map(|r| r.band)
                        .filter(|b| *b != FitnessBand::Untested),
                    FitnessBand::Ready,
                )
            };

            SlotVerdict {
                slot: binding.slot.clone(),
                band,
                reasons: sort_reasons(reasons),
                harnesses: verdicts,
                task_floor: floor,
                contract: weighted(
                    &slot_cases,
                    n,
                    |c| c.first_pass,
                    |_| true,
                    "contract held first try",
                ),
                repair: weighted(
                    &slot_cases,
                    n,
                    |c| c.contract_held,
                    |_| true,
                    "contract held at all",
                ),
                task: weighted(
                    &slot_cases,
                    n,
                    |c| c.task == crate::fitness::evals::TaskVerdict::Pass,
                    |c| c.task != crate::fitness::evals::TaskVerdict::Unscored,
                    "fixture checks passed",
                ),
            }
        })
        .collect();

    let bound_ids: HashSet<&str> = bindings
        .iter()
        .flat_map(|b| b.harnesses.iter().map(|h| h.id.as_str()))
        .collect();
    let unbound: Vec<HarnessVerdict> = input
        .harnesses
        .iter()
        .filter(|h| !bound_ids.contains(h.def.id))
        // Judged at the default floor: there is no slot whose policy could
        // apply, which is exactly what makes them unbound.
        .filter_map(|h| verdict_for(h.def.id, DEFAULT_TASK_FLOOR))
        .collect();

    FitnessReport {
        model: sweep.model.clone(),
        slots,
        unbound,
        guarded,
    }
}

// ── Tests ────────────────────────────────────────────────────────────────────
//
// The scorer is PURE by construction — every edge is an argument — so this
// module drives the real band boundaries with no gateway, no database and no
// model anywhere near it. The registry is real where the question is about the
// registry (the binding, the declared-edge table) and synthetic where the
// question is about arithmetic.
#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::Arc;

    use serde_json::{json, Value};

    use super::*;
    use crate::capability_reach::Supplier;
    use crate::fitness::evals::{
        BandScores, EvalCaseScore, EvalSweep, EvalSweepState, HarnessMeta, HarnessScore,
        SweepConcurrency, TaskVerdict,
    };
    use crate::harness::define::{
        CheckCtx, CheckResult, EvalBand, EvalCase, HarnessDefinition, OnFailure, Output,
        RenderContext,
    };
    use crate::harness::registry::{builtin_activity_harnesses, HarnessSource};
    use crate::harness::schema::Schema;

    // ── Fixtures ─────────────────────────────────────────────────────────────

    /// A synthetic harness, leaked so it can pose as a registry entry. Built
    /// through `HarnessDefinition::new` rather than `define_harness` on purpose:
    /// `define_harness` derives the json floor, and a floor is a RUNTIME refusal
    /// the scorer never consults — keeping it empty keeps these fixtures about
    /// the arithmetic and nothing else.
    fn harness(id: &'static str) -> RegisteredHarness {
        harness_over(id, &[], 1)
    }

    fn harness_over(id: &'static str, requires: &[&'static str], evals: usize) -> RegisteredHarness {
        let mut def = HarnessDefinition::new(
            id,
            id,
            "Answers.",
            ModelSpec { pin: None, role: None, chain: Some(&[]), user_id: None },
            Arc::new(|_input: &Value, _ctx: &RenderContext| Ok(Vec::new())),
            Output::Json { schema: Schema::string(), preprocess: None, repair: None, verify: None },
            OnFailure::Null,
        );
        def.requires = requires.to_vec();
        def.evals = (0..evals)
            .map(|_| {
                EvalCase::new(
                    "one",
                    json!({ "q": "x" }),
                    Arc::new(|_value: &Value, _ctx: &CheckCtx| CheckResult::Pass),
                )
            })
            .collect();
        RegisteredHarness { def: Box::leak(Box::new(def)), source: HarnessSource::Builtin }
    }

    fn score_of(id: &str) -> HarnessScore {
        HarnessScore {
            meta: HarnessMeta {
                id: id.to_string(),
                label: id.to_string(),
                source: "builtin".to_string(),
                output_kind: "json".into(),
                tools: "none".into(),
                requires: Vec::new(),
                verifies: true,
                repairable: true,
            },
            cases: 10,
            skipped: 0,
            gaps: 0,
            gap_reasons: Vec::new(),
            skip_reason: None,
            scored: 10,
            contract_rate: 1.0,
            repair_rate: 1.0,
            repair_yield: None,
            task_score: Some(1.0),
            band_scores: BandScores { easy: None, standard: Some(1.0), hard: None },
            guard_rate: 0.0,
            answered_rate: 1.0,
            latency_p50: 100,
            latency_p95: 100,
            prompt_tokens: 0,
            completion_tokens: 0,
            cost_usd: None,
            estimated: false,
            timeouts: 0,
            optimistic: 0,
        }
    }

    fn kase(harness_id: &str) -> EvalCaseScore {
        EvalCaseScore {
            harness: harness_id.to_string(),
            case: "one".to_string(),
            band: EvalBand::Standard,
            skipped: None,
            contract_held: true,
            first_pass: true,
            repairs: 0,
            answered: true,
            task: TaskVerdict::Pass,
            task_error: None,
            gap: None,
            findings: 0,
            latency_ms: 100,
            started_at: "2026-08-01T00:00:00.000Z".to_string(),
            wall_ms: 0,
            prompt_tokens: 0,
            completion_tokens: 0,
            cost_usd: None,
            estimated: false,
            timed_out: false,
            optimistic: false,
            error: None,
            prompt: None,
            raw: None,
            turns: None,
            calls: None,
            upstream: None,
        }
    }

    fn sweep() -> EvalSweep {
        EvalSweep {
            model: "candidate".to_string(),
            state: EvalSweepState::Done,
            started_at: None,
            finished_at: None,
            done: 0,
            total: 0,
            error: None,
            harnesses: Vec::new(),
            cases: Vec::new(),
            unfixtured: Vec::new(),
            guarded: true,
            concurrency: SweepConcurrency { requested: 1, ended: 1, low: 1, narrowed_because: None },
            measured: Vec::new(),
        }
    }

    fn fact(value: bool) -> CapabilityFact {
        CapabilityFact {
            value,
            source: "probe".to_string(),
            at: "2026-08-01T00:00:00.000Z".to_string(),
            detail: None,
            score: Some(if value { 1.0 } else { 0.0 }),
        }
    }

    fn tool_reach(cap: &str, server: &str, tool: &str) -> Reach {
        Reach {
            capability: cap.to_string(),
            reached: true,
            via: Some(ReachVia::Tool),
            supplier: Some(Supplier { server: server.to_string(), tool: tool.to_string() }),
            detail: "x".to_string(),
        }
    }

    fn no_reach(cap: &str, detail: &str) -> Reach {
        Reach {
            capability: cap.to_string(),
            reached: false,
            via: None,
            supplier: None,
            detail: detail.to_string(),
        }
    }

    // ── Slot/binding fixtures ────────────────────────────────────────────────

    fn utility_slot() -> FitnessSlot {
        FitnessSlot {
            kind: SlotKind::Role,
            id: "utility".to_string(),
            label: "Utility".to_string(),
            hint: String::new(),
            requires: Vec::new(),
            live: true,
        }
    }

    fn binding_of(slot: FitnessSlot, ids: &[&str]) -> SlotBinding {
        SlotBinding {
            slot,
            harnesses: ids
                .iter()
                .map(|id| BoundHarness { id: id.to_string(), via: BindingVia::Chain })
                .collect(),
        }
    }

    /// One synthetic slot with one harness bound, so a band boundary can be
    /// driven without dragging twenty real slots through every assertion.
    fn one_slot(h: &RegisteredHarness) -> Vec<SlotBinding> {
        vec![binding_of(utility_slot(), &[h.def.id])]
    }

    fn slot_over(h: &RegisteredHarness, id: &'static str, label: &str, requires: &[&'static str]) -> Vec<SlotBinding> {
        vec![binding_of(
            FitnessSlot { id: id.to_string(), label: label.to_string(), requires: requires.iter().map(|r| r.to_string()).collect::<Vec<_>>(), ..utility_slot() },
            &[h.def.id],
        )]
    }

    fn no_caps() -> HashMap<String, CapabilityFact> {
        HashMap::new()
    }

    fn run(
        sweep: &EvalSweep,
        harnesses: &[RegisteredHarness],
        caps: HashMap<String, CapabilityFact>,
        bindings: Vec<SlotBinding>,
    ) -> FitnessReport {
        run_with(sweep, harnesses, caps, None, None, bindings)
    }

    fn run_with(
        sweep: &EvalSweep,
        harnesses: &[RegisteredHarness],
        caps: HashMap<String, CapabilityFact>,
        reach: Option<HashMap<String, Reach>>,
        baseline: Option<HashMap<String, f64>>,
        bindings: Vec<SlotBinding>,
    ) -> FitnessReport {
        score_fitness(
            &FitnessInput {
                sweep,
                harnesses,
                capabilities: &caps,
                reach: reach.as_ref(),
                guard_baseline: baseline.as_ref(),
                floors: None,
            },
            &bindings,
        )
    }

    fn find<'a>(bindings: &'a [SlotBinding], key: &str) -> &'a SlotBinding {
        bindings
            .iter()
            .find(|b| slot_key(b.slot.kind, &b.slot.id) == key)
            .unwrap_or_else(|| panic!("no binding for {key}"))
    }

    fn harness_ids(b: &SlotBinding) -> Vec<&str> {
        b.harnesses.iter().map(|h| h.id.as_str()).collect()
    }

    // ── Slots and binding ────────────────────────────────────────────────────

    #[test]
    fn slots_cover_both_assignment_registries_and_nothing_else() {
        let slots = fitness_slots();
        assert_eq!(slots.iter().filter(|s| s.kind == SlotKind::Role).count(), 11);
        assert_eq!(slots.iter().filter(|s| s.kind == SlotKind::Agent).count(), 9);
        // The reserved roles and the non-assignable briefer are still slots —
        // they just say they are inert.
        assert!(!slots.iter().find(|s| s.id == "vision").expect("vision role").live);
        assert!(!slots
            .iter()
            .find(|s| s.kind == SlotKind::Agent && s.id == "briefer")
            .expect("briefer agent")
            .live);
    }

    #[test]
    fn keys_a_role_and_an_agent_apart_even_if_the_ids_ever_collide() {
        assert_eq!(slot_key(SlotKind::Role, "utility"), "role:utility");
        assert_eq!(slot_key(SlotKind::Agent, "utility"), "agent:utility");
    }

    // ── roles_reaching ───────────────────────────────────────────────────────

    #[tokio::test]
    async fn finds_the_utility_role_through_the_default_chain_which_ten_builtins_use() {
        // The titler declares only a pin. Nothing in this module spells the
        // default chain — the real resolver is asked, which is the point.
        let roles = roles_reaching(&ModelSpec { pin: Some("titler"), role: None, chain: None, user_id: None }).await;
        assert!(roles.contains(&"utility".to_string()), "roles: {roles:?}");
    }

    #[tokio::test]
    async fn finds_a_role_a_spec_names_explicitly() {
        let roles = roles_reaching(&ModelSpec { pin: None, role: Some("code-heavy"), chain: None, user_id: None }).await;
        assert!(roles.contains(&"code-heavy".to_string()));
        assert!(roles.contains(&"utility".to_string()));
    }

    #[tokio::test]
    async fn finds_nothing_for_an_empty_chain_the_model_comes_from_the_subject() {
        let roles = roles_reaching(&empty_spec()).await;
        assert!(roles.is_empty(), "roles: {roles:?}");
    }

    #[tokio::test]
    async fn does_not_invent_a_role_binding_from_a_chain_with_no_role_step() {
        let roles = roles_reaching(&ModelSpec { chain: Some(&["env", "first-routable"]), ..empty_spec() }).await;
        assert!(roles.is_empty(), "roles: {roles:?}");
    }

    fn empty_spec() -> ModelSpec<'static> {
        ModelSpec { pin: None, role: None, chain: Some(&[]), user_id: None }
    }

    // ── bind_slots ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn binds_the_pinned_utility_harnesses_to_role_utility_and_their_agent_slot() {
        let bindings = bind_slots(builtin_activity_harnesses()).await;
        let utility = harness_ids(find(&bindings, "role:utility"));
        for id in ["titler", "summarizer", "librarian", "blurb-writer"] {
            assert!(utility.contains(&id), "role:utility missing {id}: {utility:?}");
        }
        let titler = find(&bindings, "agent:titler");
        assert_eq!(titler.harnesses.len(), 1);
        assert_eq!(titler.harnesses[0].id, "titler");
        assert_eq!(titler.harnesses[0].via, BindingVia::Pin);
    }

    #[tokio::test]
    async fn binds_the_judge_through_the_registry_whose_model_lives_in_judge_config() {
        let bindings = bind_slots(builtin_activity_harnesses()).await;
        let judge = find(&bindings, "agent:judge");
        assert_eq!(judge.harnesses.len(), 1);
        assert_eq!(judge.harnesses[0].id, "judge");
        assert_eq!(judge.harnesses[0].via, BindingVia::Pin);
        // And NOT to role:utility — its chain is ['env', 'first-routable'].
        let utility = harness_ids(find(&bindings, "role:utility"));
        assert!(!utility.contains(&"judge"), "judge bound to role:utility: {utility:?}");
    }

    #[tokio::test]
    async fn carries_the_one_declared_edge_so_the_research_columns_are_not_empty() {
        let bindings = bind_slots(builtin_activity_harnesses()).await;
        for role in ["research-recon", "research-brief", "research-expedition"] {
            let b = find(&bindings, &format!("role:{role}"));
            assert_eq!(b.harnesses.len(), 1, "role:{role}");
            assert_eq!(b.harnesses[0].id, "research-search");
            assert_eq!(b.harnesses[0].via, BindingVia::Declared);
        }
    }

    #[tokio::test]
    async fn leaves_no_harness_without_a_column() {
        // THE BUG THIS PINS, and it was quiet for a long time. The matrix's
        // columns ARE the slots, so a harness bound to no slot is measured,
        // scored, archived — and invisible. Twelve were: the work session, the
        // channel plan, the plan doc, outreach, all three Inbox harnesses, both
        // briefers, and both research stages. Every one declares
        // `model: { chain: [] }` because production pins the SUBJECT of the
        // call, so there was nothing to derive a binding from and no registry an
        // admin picks that model out of.
        let hs = builtin_activity_harnesses();
        let bindings = bind_slots(hs).await;
        let bound: HashSet<&str> = bindings
            .iter()
            .flat_map(|b| b.harnesses.iter().map(|h| h.id.as_str()))
            .collect();
        let missing: Vec<&str> = hs.iter().map(|h| h.def.id).filter(|id| !bound.contains(id)).collect();
        assert_eq!(missing, Vec::<&str>::new(), "harnesses with no column: {missing:?}");
    }

    #[tokio::test]
    async fn splits_the_fleet_into_the_two_jobs_an_org_runs_different_models_behind() {
        // A personal assistant reads one owner's inbox and drafts in their
        // voice; a workspace agent works tickets and drives the toolkit against
        // a shared board. One column would average a model's fitness for both
        // and be right about neither.
        let bindings = bind_slots(builtin_activity_harnesses()).await;
        let assistant = harness_ids(find(&bindings, "fleet:assistant"));
        let agent = harness_ids(find(&bindings, "fleet:agent"));
        for id in ["inbox-brief", "inbox-command", "inbox-reply", "briefer:daily-open", "briefer:daily-delta"] {
            assert!(assistant.contains(&id), "fleet:assistant missing {id}: {assistant:?}");
        }
        for id in ["work-session", "hermes:knowledge", "channel-plan"] {
            assert!(agent.contains(&id), "fleet:agent missing {id}: {agent:?}");
        }
        // The two are disjoint — a harness in both would be scored twice against
        // one model and read as corroboration.
        let in_both: Vec<&str> = assistant.iter().copied().filter(|id| agent.contains(id)).collect();
        assert_eq!(in_both, Vec::<&str>::new());
    }

    #[tokio::test]
    async fn locks_every_fleet_binding_against_the_real_registry() {
        // Same guarantee the declared-edge table gets, for the same reason:
        // these bindings are BY NAME, so a renamed harness silently empties a
        // column rather than failing anywhere.
        let ids: HashSet<&str> = builtin_activity_harnesses().iter().map(|h| h.def.id).collect();
        let bindings = bind_slots(builtin_activity_harnesses()).await;
        for b in bindings.iter().filter(|b| b.slot.kind == SlotKind::Fleet) {
            let key = slot_key(b.slot.kind, &b.slot.id);
            assert!(!b.harnesses.is_empty(), "{key} has no harnesses");
            for h in &b.harnesses {
                assert!(ids.contains(h.id.as_str()), "{} is not a registered harness", h.id);
            }
        }
    }

    #[tokio::test]
    async fn locks_every_declared_edge_against_the_real_registry() {
        let ids: HashSet<&str> = builtin_activity_harnesses().iter().map(|h| h.def.id).collect();
        for edge in declared_edges() {
            assert!(ids.contains(edge.harness), "{} is not a registered harness", edge.harness);
        }
    }

    #[tokio::test]
    async fn keeps_a_slot_nothing_reaches_with_an_empty_list_rather_than_dropping_the_column() {
        let bindings = bind_slots(builtin_activity_harnesses()).await;
        assert_eq!(find(&bindings, "role:embedding").harnesses, Vec::new());
    }

    // ── The three bands ──────────────────────────────────────────────────────

    #[test]
    fn ready_when_every_requirement_is_measured_true_contract_at_ceiling_task_at_floor() {
        let h = harness_over("h", &["json"], 1);
        let mut s = sweep();
        s.harnesses = vec![score_of("h")];
        s.cases = vec![kase("h")];
        let report = run(&s, &[h], HashMap::from([("json".to_string(), fact(true))]), one_slot(&h));
        assert_eq!(report.slots[0].band, FitnessBand::Ready);
        assert_eq!(report.slots[0].reasons, Vec::new());
    }

    #[test]
    fn workable_exactly_below_the_ready_contract_boundary_and_ready_at_it() {
        let h = harness("h");
        let rate = CONTRACT_READY - 0.01;

        let mut s_at = sweep();
        s_at.harnesses = vec![score_of("h")];
        s_at.cases = vec![kase("h")];
        let at = run(&s_at, &[h], no_caps(), one_slot(&h));

        // repairRate pinned to contractRate so the weakness being named is the
        // contract itself and not the repair turn.
        let mut s_below = sweep();
        let mut sc = score_of("h");
        sc.contract_rate = rate;
        sc.repair_rate = rate;
        s_below.harnesses = vec![sc];
        s_below.cases = vec![kase("h")];
        let below = run(&s_below, &[h], no_caps(), one_slot(&h));

        assert_eq!(at.slots[0].band, FitnessBand::Ready);
        assert_eq!(below.slots[0].band, FitnessBand::Workable);
        assert_eq!(below.slots[0].reasons[0].kind, ReasonKind::Contract);
    }

    #[test]
    fn names_the_repair_turn_not_the_contract_when_a_sub_ceiling_first_pass_is_fully_rescued() {
        let h = harness("h");
        let mut s = sweep();
        let mut sc = score_of("h");
        sc.contract_rate = CONTRACT_READY - 0.01;
        sc.repair_rate = 1.0;
        s.harnesses = vec![sc];
        s.cases = vec![kase("h")];
        let report = run(&s, &[h], no_caps(), one_slot(&h));
        assert_eq!(report.slots[0].reasons[0].kind, ReasonKind::RepairCarried);
    }

    #[test]
    fn unfit_below_the_contract_floor_and_names_the_harness_never_a_bare_score() {
        let h = harness("h");
        let mut s = sweep();
        let mut sc = score_of("h");
        sc.contract_rate = CONTRACT_UNFIT - 0.01;
        sc.repair_rate = 0.5;
        s.harnesses = vec![sc];
        let mut c = kase("h");
        c.contract_held = false;
        c.first_pass = false;
        c.task = TaskVerdict::Unscored;
        c.error = Some("the reply never closed its JSON value".to_string());
        s.cases = vec![c];
        let report = run(&s, &[h], no_caps(), one_slot(&h));
        assert_eq!(report.slots[0].band, FitnessBand::Unfit);
        let reason = &report.slots[0].reasons[0];
        assert_eq!(reason.kind, ReasonKind::Contract);
        assert_eq!(reason.harness.as_deref(), Some("h"));
        assert_eq!(reason.assertion.as_deref(), Some("the reply never closed its JSON value"));
    }

    #[test]
    fn forty_ninetyfive_is_workable_and_says_the_repair_path_carries_it() {
        let h = harness("h");
        let mut s = sweep();
        let mut sc = score_of("h");
        sc.contract_rate = 0.4;
        sc.repair_rate = REPAIR_WORKABLE;
        s.harnesses = vec![sc];
        s.cases = vec![kase("h")];
        let carried = run(&s, &[h], no_caps(), one_slot(&h));
        assert_eq!(carried.slots[0].band, FitnessBand::Workable);
        assert!(carried.slots[0].reasons.iter().any(|r| r.kind == ReasonKind::RepairCarried));

        let mut s2 = sweep();
        let mut sc2 = score_of("h");
        sc2.contract_rate = 0.4;
        sc2.repair_rate = 0.45;
        s2.harnesses = vec![sc2];
        s2.cases = vec![kase("h")];
        let stranded = run(&s2, &[h], no_caps(), one_slot(&h));
        assert_eq!(stranded.slots[0].band, FitnessBand::Unfit);
    }

    #[test]
    fn never_claims_the_repair_path_carried_a_text_harness_where_no_repair_is_sent() {
        let h = harness("h");
        let mut s = sweep();
        let mut sc = score_of("h");
        // repairable: false mirrors the runner allowing no repairs on text
        // output.
        sc.meta.repairable = false;
        sc.meta.output_kind = "text".into();
        sc.contract_rate = 0.4;
        sc.repair_rate = 0.4;
        s.harnesses = vec![sc];
        s.cases = vec![kase("h")];
        let report = run(&s, &[h], no_caps(), one_slot(&h));
        assert_eq!(report.slots[0].band, FitnessBand::Unfit);
        assert!(!report.slots[0].reasons.iter().any(|r| r.kind == ReasonKind::RepairCarried));
    }

    #[test]
    fn workable_within_10pct_of_the_task_floor_unfit_beyond_it_with_the_assertion_verbatim() {
        let h = harness("h");
        // A slot with no floor policy, so the boundary being driven is the
        // default.
        let slot = slot_over(&h, "code-standard", "Workbench · Standard effort", &[]);
        let floor = DEFAULT_TASK_FLOOR;

        let mut s_near = sweep();
        let mut near = score_of("h");
        near.task_score = Some(floor - 0.05);
        s_near.harnesses = vec![near];
        let mut c = kase("h");
        c.task = TaskVerdict::Fail;
        c.task_error = Some("the title must be 3-7 words".to_string());
        s_near.cases = vec![c.clone()];
        let near_report = run(&s_near, &[h], no_caps(), slot.clone());
        assert_eq!(near_report.slots[0].band, FitnessBand::Workable);

        let mut s_far = sweep();
        let mut far = score_of("h");
        far.task_score = Some(floor * (1.0 - TASK_TOLERANCE) - 0.01);
        s_far.harnesses = vec![far];
        s_far.cases = vec![c];
        let far_report = run(&s_far, &[h], no_caps(), slot);
        assert_eq!(far_report.slots[0].band, FitnessBand::Unfit);
        assert_eq!(
            far_report.slots[0].reasons[0].assertion.as_deref(),
            Some("the title must be 3-7 words")
        );
    }

    #[test]
    fn unfit_on_a_capability_recorded_false_the_assignment_audit_1_6_is_about() {
        let h = harness_over("h", &["search"], 1);
        let mut s = sweep();
        s.harnesses = vec![score_of("h")];
        s.cases = vec![kase("h")];
        let report = run(&s, &[h], HashMap::from([("search".to_string(), fact(false))]), one_slot(&h));
        assert_eq!(report.slots[0].band, FitnessBand::Unfit);
        assert_eq!(report.slots[0].reasons[0].kind, ReasonKind::MissingCapability);
    }

    #[test]
    fn unfit_when_a_role_requires_a_capability_the_model_lacks_even_with_every_harness_green() {
        let h = harness("h");
        let mut s = sweep();
        s.harnesses = vec![score_of("h")];
        s.cases = vec![kase("h")];
        let report = run(
            &s,
            &[h],
            HashMap::from([("search".to_string(), fact(false))]),
            slot_over(&h, "research-recon", "Research · Recon", &["search"]),
        );
        assert_eq!(report.slots[0].band, FitnessBand::Unfit);
        assert_eq!(report.slots[0].reasons[0].harness, None);
    }

    #[test]
    fn caps_at_workable_when_a_required_capability_was_never_measured_unknown_is_never_unfit() {
        let h = harness_over("h", &["json"], 1);
        let mut s = sweep();
        s.harnesses = vec![score_of("h")];
        s.cases = vec![kase("h")];
        let report = run(&s, &[h], no_caps(), one_slot(&h));
        assert_eq!(report.slots[0].band, FitnessBand::Workable);
        assert!(report.slots[0].reasons.iter().any(|r| r.kind == ReasonKind::UnmeasuredCapability));
    }

    #[test]
    fn unfit_on_a_safety_regression_above_the_production_baseline_clean_at_or_below_it() {
        let h = harness("h");
        let mut s = sweep();
        let mut sc = score_of("h");
        sc.guard_rate = 0.2;
        s.harnesses = vec![sc];
        let mut c = kase("h");
        c.findings = 2;
        s.cases = vec![c];

        let regressed = run_with(&s, &[h], no_caps(), None, Some(HashMap::from([("h".to_string(), 0.1)])), one_slot(&h));
        assert_eq!(regressed.slots[0].band, FitnessBand::Unfit);
        assert_eq!(regressed.slots[0].reasons[0].kind, ReasonKind::Safety);

        let tolerated = run_with(&s, &[h], no_caps(), None, Some(HashMap::from([("h".to_string(), 0.2)])), one_slot(&h));
        assert_eq!(tolerated.slots[0].band, FitnessBand::Ready);
    }

    #[test]
    fn treats_a_missing_baseline_as_zero_and_says_so_rather_than_pretending_it_measured_zero() {
        let h = harness("h");
        let mut s = sweep();
        let mut sc = score_of("h");
        sc.guard_rate = 0.34;
        s.harnesses = vec![sc];
        let mut c = kase("h");
        c.findings = 1;
        s.cases = vec![c];
        let report = run(&s, &[h], no_caps(), one_slot(&h));
        assert_eq!(report.slots[0].band, FitnessBand::Unfit);
        assert!(report.slots[0].reasons[0].detail.contains("nothing filed for this harness yet"));
    }

    #[test]
    fn does_not_read_a_guard_rate_of_zero_as_clean_when_the_guard_was_off() {
        let h = harness("h");
        let mut s = sweep();
        s.guarded = false;
        s.harnesses = vec![score_of("h")];
        s.cases = vec![kase("h")];
        let report = run(&s, &[h], no_caps(), one_slot(&h));
        assert_eq!(report.slots[0].band, FitnessBand::Workable);
        assert!(report.slots[0].reasons.iter().any(|r| r.kind == ReasonKind::GuardOff));
        assert!(!report.guarded);
    }

    #[test]
    fn does_not_blame_the_model_when_nothing_answered_a_refused_floor_or_dead_gateway_is_untested() {
        let h = harness("h");
        let mut s = sweep();
        let mut sc = score_of("h");
        sc.answered_rate = 0.0;
        sc.contract_rate = 0.0;
        sc.repair_rate = 0.0;
        sc.task_score = None;
        s.harnesses = vec![sc];
        let mut c = kase("h");
        c.answered = false;
        c.contract_held = false;
        c.first_pass = false;
        c.task = TaskVerdict::Unscored;
        c.error = Some("refused below the capability floor: search".to_string());
        s.cases = vec![c];
        let report = run(&s, &[h], no_caps(), one_slot(&h));
        assert_eq!(report.slots[0].band, FitnessBand::Untested);
        assert_eq!(
            report.slots[0].reasons[0].assertion.as_deref(),
            Some("refused below the capability floor: search")
        );
    }

    #[test]
    fn not_unfit_when_a_registered_tool_reaches_the_capability_the_model_lacks() {
        // THE CORRECTION THIS WHOLE PASS IS ABOUT. deepseek-v4-flash: `search`
        // measured false, `tools` measured true, and a web-search server in the
        // registry. It was reported Not-a-fit for all three Research slots — a
        // true statement about the weights and a false one about the slot, which
        // is a model running inside Talaria with the tools this org registered.
        let h = harness_over("h", &["search"], 1);
        let mut s = sweep();
        s.harnesses = vec![score_of("h")];
        s.cases = vec![kase("h")];
        let report = run_with(
            &s,
            &[h],
            HashMap::from([("search".to_string(), fact(false))]),
            Some(HashMap::from([("search".to_string(), tool_reach("search", "exa", "web_search"))])),
            None,
            slot_over(&h, "research-recon", "Research · Recon", &["search"]),
        );
        assert_eq!(report.slots[0].band, FitnessBand::Ready);
        // Reported, not silent: if that server is removed the cell changes, and
        // this is the sentence that explains why.
        let supplied: Vec<&FitnessReason> = report.slots[0]
            .reasons
            .iter()
            .filter(|r| r.kind == ReasonKind::SuppliedCapability)
            .collect();
        assert!(!supplied.is_empty());
        assert!(supplied[0].detail.contains("exa.web_search"), "detail: {}", supplied[0].detail);
    }

    #[test]
    fn still_unfit_when_nothing_reaches_the_capability() {
        // Reach widens the question; it does not soften it. An org with no
        // search server and a memory-only model gets the same verdict it always
        // got.
        let h = harness_over("h", &["search"], 1);
        let mut s = sweep();
        s.harnesses = vec![score_of("h")];
        s.cases = vec![kase("h")];
        let report = run_with(
            &s,
            &[h],
            HashMap::from([("search".to_string(), fact(false))]),
            Some(HashMap::from([(
                "search".to_string(),
                no_reach("search", "no enabled MCP server offers a tool for it"),
            )])),
            None,
            slot_over(&h, "research-recon", "Research · Recon", &["search"]),
        );
        assert_eq!(report.slots[0].band, FitnessBand::Unfit);
        // And the sentence names the org's next move rather than blaming the
        // model.
        assert!(report.slots[0].reasons[0].detail.contains("no enabled MCP server"));
    }

    #[test]
    fn falls_back_to_the_raw_capability_fact_when_nothing_asked_about_reach() {
        // Every caller that has no registry to ask gets the pre-reach verdict,
        // which is narrower and never wrong in the unsafe direction.
        let h = harness_over("h", &["search"], 1);
        let mut s = sweep();
        s.harnesses = vec![score_of("h")];
        s.cases = vec![kase("h")];
        let report = run(&s, &[h], HashMap::from([("search".to_string(), fact(false))]), one_slot(&h));
        assert_eq!(report.slots[0].band, FitnessBand::Unfit);
    }

    #[test]
    fn reports_a_harness_the_sweep_could_not_run_as_not_runnable_not_a_contract_failure() {
        // The tool-loop harnesses on a gateway candidate. `cases: 0` with a
        // `skipReason` is how the sweep says "nothing ran"; a band of `unfit`
        // here would blame a model that was never called.
        let h = harness("h");
        let why = "X runs the model\u{2019}s own tool loop, and \"gw/model\" is served by the org gateway, which has no tool loop.";
        let mut s = sweep();
        let mut sc = score_of("h");
        sc.cases = 0;
        sc.skipped = 2;
        sc.skip_reason = Some(why.to_string());
        sc.contract_rate = 0.0;
        sc.repair_rate = 0.0;
        sc.task_score = None;
        sc.answered_rate = 0.0;
        s.harnesses = vec![sc];
        s.cases = vec![];
        let report = run(&s, &[h], no_caps(), one_slot(&h));
        assert_eq!(report.slots[0].band, FitnessBand::Untested);
        assert_eq!(report.slots[0].reasons[0].kind, ReasonKind::NotRunnable);
        // The sweep's own sentence, verbatim — not "this sweep did not run it",
        // which reads as "press Test again" and would be wrong.
        assert_eq!(report.slots[0].reasons[0].detail, why);
    }

    #[test]
    fn states_a_missing_capability_once_not_once_per_harness_that_also_needs_it() {
        // THE NOISE THIS KILLS. A research slot declares `requires: ['search']`
        // and binds a harness that requires it too, so the same fact was
        // reported twice — and the harness telling dragged the runner's whole
        // refusal paragraph with it, once per slot, burying every other reason
        // on the page.
        let h = harness_over("h", &["search"], 1);
        let mut s = sweep();
        let mut sc = score_of("h");
        sc.answered_rate = 0.0;
        sc.contract_rate = 0.0;
        sc.repair_rate = 0.0;
        sc.task_score = None;
        s.harnesses = vec![sc];
        let mut c = kase("h");
        c.answered = false;
        c.contract_held = false;
        c.first_pass = false;
        c.task = TaskVerdict::Unscored;
        c.error = Some("a very long refusal paragraph".to_string());
        s.cases = vec![c];
        let report = run(
            &s,
            &[h],
            HashMap::from([("search".to_string(), fact(false))]),
            slot_over(&h, "research-recon", "Research · Recon", &["search"]),
        );
        assert_eq!(report.slots[0].band, FitnessBand::Unfit);
        // One line, about the slot the admin is choosing for.
        assert_eq!(report.slots[0].reasons.len(), 1, "reasons: {:?}", report.slots[0].reasons.iter().map(|r| r.kind.as_str()).collect::<Vec<_>>());
        assert_eq!(report.slots[0].reasons[0].harness, None);
        assert_eq!(report.slots[0].reasons[0].capability.as_deref(), Some("search"));
        // And no `no-answer` restating the same refusal a third time.
        assert!(!report.slots[0].reasons.iter().any(|r| r.kind == ReasonKind::NoAnswer));
    }

    #[test]
    fn keeps_a_harness_level_capability_line_the_slot_does_not_itself_declare() {
        // The dedupe is per capability, not per reason kind: a harness needing
        // something its slot never asked for is news, and must survive.
        let h = harness_over("h", &["vision"], 1);
        let mut s = sweep();
        s.harnesses = vec![score_of("h")];
        s.cases = vec![kase("h")];
        let report = run(
            &s,
            &[h],
            HashMap::from([
                ("search".to_string(), fact(false)),
                ("vision".to_string(), fact(false)),
            ]),
            slot_over(&h, "utility", "Utility", &["search"]),
        );
        let caps: Vec<Option<&str>> = report.slots[0].reasons.iter().map(|r| r.capability.as_deref()).collect();
        assert_eq!(caps, vec![Some("search"), Some("vision")]);
    }

    // ── The empty column ─────────────────────────────────────────────────────

    #[test]
    fn a_slot_with_no_harness_bound_reads_as_unbound_not_as_an_empty_pass() {
        let s = sweep();
        let report = run(
            &s,
            &[],
            no_caps(),
            vec![binding_of(
                FitnessSlot {
                    kind: SlotKind::Role,
                    id: "embedding".to_string(),
                    label: "Embeddings".to_string(),
                    hint: String::new(),
                    requires: Vec::new(),
                    live: false,
                },
                &[],
            )],
        );
        assert_eq!(report.slots[0].band, FitnessBand::Unbound);
        assert_eq!(report.slots[0].reasons[0].kind, ReasonKind::NoHarness);
        assert!(report.slots[0].reasons[0].detail.contains("This is not a pass"));
        assert!(report.slots[0].contract.is_none());
    }

    #[test]
    fn an_unbound_column_still_reports_a_capability_the_role_is_recorded_as_lacking() {
        // So an unbound column is not a hiding place.
        let s = sweep();
        let report = run(
            &s,
            &[],
            HashMap::from([("search".to_string(), fact(false))]),
            vec![binding_of(
                FitnessSlot {
                    kind: SlotKind::Role,
                    id: "research-recon".to_string(),
                    label: "Research · Recon".to_string(),
                    hint: String::new(),
                    requires: vec!["search".into()],
                    live: true,
                },
                &[],
            )],
        );
        assert_eq!(report.slots[0].band, FitnessBand::Unfit);
    }

    #[test]
    fn cannot_be_ready_when_one_of_several_bound_harnesses_has_no_verdict() {
        let good = harness("good");
        let silent = harness("silent");
        let mut s = sweep();
        s.harnesses = vec![score_of("good")];
        s.cases = vec![kase("good")];
        let report = run(
            &s,
            &[good, silent],
            no_caps(),
            vec![binding_of(utility_slot(), &["good", "silent"])],
        );
        assert_eq!(report.slots[0].band, FitnessBand::Workable);
        assert!(report.slots[0].reasons.iter().any(|r| r.kind == ReasonKind::PartialCoverage));
    }

    #[test]
    fn says_a_harness_declares_no_fixtures_rather_than_scoring_it() {
        let bare = harness_over("bare", &[], 0);
        let s = sweep();
        let report = run(&s, &[bare], no_caps(), one_slot(&bare));
        assert_eq!(report.slots[0].band, FitnessBand::Untested);
        assert_eq!(report.slots[0].reasons[0].kind, ReasonKind::NoFixtures);
    }

    // ── Aggregates ───────────────────────────────────────────────────────────

    #[test]
    fn weights_by_case_and_carries_a_label_saying_so() {
        // schema_valid is comparable per harness only.
        let a = harness("a");
        let b = harness("b");
        let mut s = sweep();
        let mut sa = score_of("a");
        sa.cases = 1;
        sa.contract_rate = 0.0;
        sa.repair_rate = 0.0;
        sa.task_score = None;
        let mut sb = score_of("b");
        sb.cases = 3;
        s.harnesses = vec![sa, sb];
        let mut c1 = kase("a");
        c1.case = "a1".to_string();
        c1.first_pass = false;
        c1.contract_held = false;
        c1.task = TaskVerdict::Unscored;
        let mut b1 = kase("b");
        b1.case = "b1".to_string();
        let mut b2 = kase("b");
        b2.case = "b2".to_string();
        let mut b3 = kase("b");
        b3.case = "b3".to_string();
        s.cases = vec![c1, b1, b2, b3];
        let report = run(&s, &[a, b], no_caps(), vec![binding_of(utility_slot(), &["a", "b"])]);
        // 3 of 4 cases, not the mean of 0 and 1.
        let contract = report.slots[0].contract.as_ref().expect("contract rate");
        assert!((contract.rate - 0.75).abs() < 1e-9, "rate: {}", contract.rate);
        assert!(contract.label.contains("comparable within a harness only"));
        // The band still comes from the WORST harness, not from the aggregate.
        assert_eq!(report.slots[0].band, FitnessBand::Unfit);
    }

    #[test]
    fn scores_the_fixture_check_only_over_cases_that_were_gradable() {
        let h = harness("h");
        let mut s = sweep();
        let mut sc = score_of("h");
        sc.cases = 2;
        sc.task_score = Some(0.5);
        s.harnesses = vec![sc];
        let mut c1 = kase("h");
        c1.case = "1".to_string();
        let mut c2 = kase("h");
        c2.case = "2".to_string();
        c2.task = TaskVerdict::Unscored;
        s.cases = vec![c1, c2];
        let report = run(&s, &[h], no_caps(), one_slot(&h));
        let task = report.slots[0].task.as_ref().expect("task rate");
        assert_eq!(task.numerator, 1);
        assert_eq!(task.denominator, 1);
    }

    // ── Floors ───────────────────────────────────────────────────────────────

    #[test]
    fn task_floors_are_per_slot_so_one_harness_can_be_ready_for_utility_and_unfit_for_judge() {
        let h = harness("h");
        let mut s = sweep();
        let mut sc = score_of("h");
        sc.task_score = Some(0.75);
        s.harnesses = vec![sc];
        s.cases = vec![kase("h")];

        let utility = run(&s, &[h], no_caps(), one_slot(&h));
        let judge = run(
            &s,
            &[h],
            no_caps(),
            vec![binding_of(
                FitnessSlot { kind: SlotKind::Agent, id: "judge".to_string(), label: "Judge".to_string(), ..utility_slot() },
                &[h.def.id],
            )],
        );
        assert_eq!(utility.slots[0].band, FitnessBand::Ready);
        assert_eq!(judge.slots[0].band, FitnessBand::Unfit);
        assert_eq!(utility.slots[0].task_floor, task_floor_of("role:utility").unwrap());
        assert_eq!(judge.slots[0].task_floor, task_floor_of("agent:judge").unwrap());
    }

    #[test]
    fn takes_an_install_override_ahead_of_the_shipped_policy() {
        let slot = utility_slot();
        assert_eq!(task_floor_for(&slot, None), task_floor_of("role:utility").unwrap());
        assert_eq!(
            task_floor_for(&slot, Some(&HashMap::from([("role:utility".to_string(), 0.99)]))),
            0.99
        );
    }

    #[test]
    fn falls_back_to_the_default_for_a_slot_with_no_policy() {
        let slot = FitnessSlot { id: "code-standard".to_string(), ..utility_slot() };
        assert_eq!(task_floor_for(&slot, None), DEFAULT_TASK_FLOOR);
    }

    // ── Unbound harnesses ────────────────────────────────────────────────────

    #[test]
    fn unbound_harnesses_are_scored_on_their_own() {
        // "Can this model work a ticket" still has an answer.
        let h = harness("work-session");
        let mut s = sweep();
        s.harnesses = vec![score_of("work-session")];
        s.cases = vec![kase("work-session")];
        let report = run(&s, &[h], no_caps(), vec![]);
        assert_eq!(report.unbound.len(), 1);
        assert_eq!(report.unbound[0].harness, "work-session");
        assert_eq!(report.unbound[0].band, FitnessBand::Ready);
    }

    #[test]
    fn unbound_excludes_anything_a_slot_already_claims() {
        let h = harness("h");
        let mut s = sweep();
        s.harnesses = vec![score_of("h")];
        s.cases = vec![kase("h")];
        let report = run(&s, &[h], no_caps(), one_slot(&h));
        assert_eq!(report.unbound, Vec::new());
    }
}
