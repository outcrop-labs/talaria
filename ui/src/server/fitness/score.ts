// THE VERDICT, AND IT IS PER SLOT — never one number for a model.
//
// That is the locked product decision and it is the whole point of the six
// workflows underneath it: a model can be Ready for Utility and Not-a-fit for
// Judge, and until this file existed nothing in Talaria could say so. An admin
// found out when the judge started escalating every ticket, which with
// `mode: 'enforcing'` is a notification storm rather than an error.
//
// WHAT A SLOT IS. The thing an admin picks a model for. Talaria has exactly two
// kinds and both are literally a dropdown on an Admin page:
//   MODEL_ROLES      the eleven role assignments (model-roles.ts)
//   PLATFORM_AGENTS  the nine platform-agent assignments (platform-agents.ts)
// The matrix's columns are those twenty slots. Nothing else is invented: a
// harness that no slot can deliver a model to is reported as UNBOUND with its
// own per-harness verdict, rather than being quietly folded into a column an
// admin cannot act on.
//
// HOW THE BINDING IS DERIVED, and why it is derived rather than written down.
// Three sources, in descending order of how much this file gets to decide:
//
//   the resolver   `rolesReaching` runs the REAL chain
//                  (`resolveHarnessModelWith`) over instrumented dependencies
//                  and records which roles it asked for. `DEFAULT_CHAIN` is
//                  private to model.ts, and the default is the case that
//                  matters — ten builtins declare only a pin and reach the
//                  Utility role through the default chain's 'utility' step — so
//                  a copy of the step order in this file would be an eighth
//                  spelling of the policy audit 1.10 is about. Ask the chain.
//   `platformAgentOf`  registry.ts already owns the harness -> platform-agent
//                  map, including the judge exception (its model lives in
//                  `judge_config`, so its spec declares no pin). One door.
//   DECLARED_EDGES a table of exactly ONE entry, for the one harness whose
//                  production role resolution provably cannot live in a
//                  `ModelSpec`. See the comment on it. Every other edge is
//                  derived, and `score.test.ts` locks the table against the
//                  registry so a typo cannot invent a binding.
//
// THREE BANDS, NOT A SCORE. `ready` / `workable` / `unfit`, plus `untested`
// (nothing measured this) and `unbound` (no harness reaches this slot — which
// must READ as "no evidence", never as an empty green cell). `unfit` always
// names the HARNESS and, where a fixture is what failed, the fixture's own
// assertion verbatim. A bare percentage tells an admin nothing they can act on.
//
// READY REQUIRES POSITIVE EVIDENCE, and that is the one place this file is
// deliberately stricter than `capability.ts`. The cardinal rule there is UNKNOWN
// IS NOT FALSE, because Talaria has to keep working on a model nobody has
// benchmarked. That rule is about RUNNING. A VERDICT is the opposite question —
// it exists to say what has been measured — so an unmeasured required
// capability, or a sweep with the guard switched off, caps a slot at `workable`
// with a reason saying which button to press. Neither ever pushes a slot to
// `unfit`: absence of evidence is not evidence of absence in this direction
// either.
//
// CAUTION ON AVERAGES, which is why `WeightedRate` carries a label it is
// impossible to print the number without. `HarnessResult.schemaValid` is
// deliberately NOT comparable across harnesses — the titler's non-empty-string
// check and the judge's zod+verify are both `true` and mean different things. So
// a slot's band is the WORST OF ITS HARNESSES' bands, decided per harness, and
// the cross-harness rates on the verdict exist for coverage ("38 of 40 cases")
// rather than for quality comparison. Averaging four harnesses' contract rates
// into one figure would produce a number with no referent.
import { platformAgentOf, type RegisteredHarness } from '../harness/registry'
import { resolveHarnessModelWith, type ModelSpec } from '../harness/model'
import { MODEL_ROLES, type ModelRole } from '../model-roles'
import { PLATFORM_AGENTS, type PlatformAgentId } from '../platform-agents'
import type { Capability, CapabilityFact } from '../harness/capability'
import type { Reach } from '../capability-reach'
import type { EvalCaseScore, EvalSweep, HarnessScore } from './evals'

// ── Slots ────────────────────────────────────────────────────────────────────

export type SlotKind = 'role' | 'agent'

/** One assignment an admin can make, normalized across the two registries that
 *  offer them. `requires` is populated for roles only: `MODEL_ROLES` declares
 *  what a role's WORK needs (audit 1.6), while a platform agent declares
 *  nothing of the sort — its harnesses carry the requirement instead, and this
 *  file reads them there. */
export interface FitnessSlot {
  kind: SlotKind
  id: ModelRole | PlatformAgentId
  label: string
  hint: string
  requires: Capability[]
  /** False for a reserved role (`MODEL_ROLES[].wired === false`) or a
   *  non-assignable platform agent (the briefer, which is always the owner's
   *  own assistant). A verdict is still produced — telling an admin now that
   *  their pick cannot see is strictly more useful than telling them the week
   *  the surface ships — but the UI shows it as inert. */
  live: boolean
}

/** Stable matrix-column key. `kind` is part of it because the two registries
 *  are independent namespaces and nothing stops a future role and a future
 *  platform agent from sharing an id. */
export const slotKey = (slot: { kind: SlotKind; id: string }): string => `${slot.kind}:${slot.id}`

/** Every slot, roles first — the order the audit's matrix specifies and the
 *  order Admin shows the two panels in. */
export function fitnessSlots(): FitnessSlot[] {
  const roles: FitnessSlot[] = MODEL_ROLES.map((r) => ({
    kind: 'role',
    id: r.role,
    label: r.label,
    hint: r.hint,
    requires: r.requires,
    live: r.wired,
  }))
  const agents: FitnessSlot[] = PLATFORM_AGENTS.map((a) => ({
    kind: 'agent',
    id: a.id,
    label: a.label,
    hint: a.job,
    requires: [],
    live: a.assignable,
  }))
  return [...roles, ...agents]
}

export type BindingVia = 'chain' | 'pin' | 'declared'

export interface SlotBinding {
  slot: FitnessSlot
  harnesses: Array<{ id: string; via: BindingVia }>
}

/** THE ONE EDGE THAT CANNOT BE DERIVED, and the reason is worth reading before
 *  anyone adds a second entry.
 *
 *  `research-search` declares `model: { chain: [] }` on purpose: production
 *  resolves its model through `searchModelFor(mode)` (research.ts), which picks
 *  `research-recon` / `research-brief` / `research-expedition` from the run's
 *  MODE. A `ModelSpec` has one `role` field and the choice is mode-dependent, so
 *  the spec genuinely cannot state it — the harness's own comment says the
 *  `role: 'research-brief'` it used to declare was dead in production and would
 *  have handed a recon the brief tier's model on the one path that could have
 *  read it.
 *
 *  Leaving it out would empty the three research columns of the only harness
 *  that tests them, which is finding 1.6 — an admin pointing Research at a
 *  model with no web search and getting a confident, uncited brief — going
 *  unreported by the feature built to report it.
 *
 *  Anything else belongs in the harness's `ModelSpec`, where the resolver can
 *  see it. `score.test.ts` fails if an id here is not in the registry. */
const DECLARED_EDGES: ReadonlyArray<{ harness: string; roles: ModelRole[] }> = [
  { harness: 'research-search', roles: ['research-recon', 'research-brief', 'research-expedition'] },
]

/** Which model ROLES can put a model in front of this harness.
 *
 *  Answered by running the real chain over dependencies that record and refuse:
 *  every step returns null, so every step in the harness's chain is attempted,
 *  and `resolveRoleModel` is called exactly for the steps that consult a role
 *  ('role' with `spec.role`, 'utility' with 'utility'). Nothing here restates
 *  the step order, which is private to model.ts and is the policy seven files
 *  used to spell differently. */
export async function rolesReaching(spec: ModelSpec): Promise<ModelRole[]> {
  const seen = new Set<ModelRole>()
  await resolveHarnessModelWith(spec, {
    platformAgentModel: async () => null,
    resolveRoleModel: async (role) => {
      seen.add(role)
      return null
    },
    routes: async () => false,
    gatewayModels: async () => [],
    getPreferredModel: async () => null,
    getUserRole: async () => 'member',
    memberModelAllowlist: async () => [],
    modelAllowedFor: () => true,
    copilotEnvModel: () => null,
  })
  return [...seen]
}

/** Every slot with the harnesses bound to it. Slots with none keep an empty
 *  list rather than being dropped — a role nothing tests is a fact the matrix
 *  has to show, and dropping the column would render it as absent instead of as
 *  unknown. */
export async function bindSlots(harnesses: RegisteredHarness[]): Promise<SlotBinding[]> {
  const bound = new Map<string, Map<string, BindingVia>>()
  const add = (key: string, harness: string, via: BindingVia): void => {
    const inner = bound.get(key) ?? new Map<string, BindingVia>()
    // First writer wins: a harness reached by its own spec is bound 'chain'
    // even if a declared edge also names it, because the derived fact is the
    // one that stays true when the table rots.
    if (!inner.has(harness)) inner.set(harness, via)
    bound.set(key, inner)
  }

  for (const harness of harnesses) {
    for (const role of await rolesReaching(harness.model)) add(slotKey({ kind: 'role', id: role }), harness.id, 'chain')
    const agent = platformAgentOf(harness)
    if (agent) add(slotKey({ kind: 'agent', id: agent }), harness.id, 'pin')
  }
  const ids = new Set(harnesses.map((h) => h.id))
  for (const edge of DECLARED_EDGES) {
    if (!ids.has(edge.harness)) continue
    for (const role of edge.roles) add(slotKey({ kind: 'role', id: role }), edge.harness, 'declared')
  }

  return fitnessSlots().map((slot) => ({
    slot,
    harnesses: [...(bound.get(slotKey(slot)) ?? new Map<string, BindingVia>())].map(([id, via]) => ({ id, via })),
  }))
}

/** The declared edges, for the test that locks them against the registry. */
export const declaredEdges = (): ReadonlyArray<{ harness: string; roles: ModelRole[] }> => DECLARED_EDGES

// ── Bands ────────────────────────────────────────────────────────────────────

export type FitnessBand = 'ready' | 'workable' | 'unfit' | 'untested' | 'unbound'

/** The band boundaries, from the audit's scoring section. Exported because the
 *  UI prints them next to a cell ("contract 91%, ready needs 95%") and a second
 *  copy in a Svelte file is how the sentence and the arithmetic come to
 *  disagree. */
export const CONTRACT_READY = 0.95
export const CONTRACT_UNFIT = 0.8
export const REPAIR_WORKABLE = 0.95
/** "task within 10% of floor", relative — a floor of 0.9 tolerates 0.81. */
export const TASK_TOLERANCE = 0.1

/** The task floor when a slot declares none. Deliberately not 0.95: the task
 *  score is a fixture's own deterministic assertion, and one bad title in five
 *  is a model worth a second look rather than a model to reject. */
export const DEFAULT_TASK_FLOOR = 0.8

/** PER-SLOT TASK FLOORS — product policy, and it has to live somewhere, so it
 *  lives here with its argument attached rather than in a Svelte file.
 *
 *  The split follows the one `RoleFloor` in define.ts already states in prose:
 *  the titler, summarizer and librarian "have to work on whatever the self-host
 *  has, and a titler that refuses to name a chat is worse than a mediocre
 *  title", so their bar is lower and a merely-adequate model passes. The judge
 *  and the research stages are the opposite case — a judge whose verdicts are
 *  noise is worse than no judge, and an uncited brief is worse than no brief —
 *  so their bar is higher.
 *
 *  Override per install through `FitnessInput.floors`. */
export const TASK_FLOORS: Readonly<Record<string, number>> = {
  'role:utility': 0.7,
  'agent:titler': 0.7,
  'agent:summarizer': 0.7,
  'agent:librarian': 0.7,
  'agent:blurb-writer': 0.7,
  'agent:judge': 0.9,
  'role:research-recon': 0.9,
  'role:research-brief': 0.9,
  'role:research-expedition': 0.9,
}

export const taskFloorFor = (slot: FitnessSlot, floors: Partial<Record<string, number>> = {}): number =>
  floors[slotKey(slot)] ?? TASK_FLOORS[slotKey(slot)] ?? DEFAULT_TASK_FLOOR

export type ReasonKind =
  /** A required capability is recorded FALSE. The unfit case audit 1.6 is about. */
  | 'missing-capability'
  /** A required capability was never measured. Caps at `workable`; run tier 1. */
  | 'unmeasured-capability'
  | 'contract'
  /** Contract only holds after the repair turn — the 40/95 model, usable
   *  BECAUSE of audit 1.4, and the UI must say so rather than print one rate. */
  | 'repair-carried'
  | 'task'
  | 'safety'
  /** The sweep ran with `mode: 'off'`, so every guard rate is zero and
   *  zero-because-off must not read as zero-because-clean. Caps at `workable`. */
  | 'guard-off'
  /** The harness declares no fixtures: invisible to tier 2, not passing. */
  | 'no-fixtures'
  /** Bound and fixtured, but this sweep did not run it (`only:`, or stopped). */
  | 'not-swept'
  /** THE SWEEP DECLINED TO RUN IT AGAINST THIS CANDIDATE, because the harness
   *  declares a request the candidate's transport is documented to refuse — a
   *  tool-loop harness against an org-gateway model. Distinct from `not-swept`
   *  ("nobody has run this yet", fixable by pressing Test) because pressing Test
   *  again changes nothing: what has to change is the deployment. Caps at
   *  `untested`, exactly like the other two, because a skip is emphatically not
   *  a pass. See `harnessSkipReason` in evals.ts. */
  | 'not-runnable'
  /** A required capability the MODEL lacks and a registered TOOL supplies. Band
   *  `ready` — it is a fact worth stating, never a demerit. See
   *  `capability-reach.ts`. */
  | 'supplied-capability'
  /** Nothing answered — a refused floor or a dead gateway, not a bad model. */
  | 'no-answer'
  /** No harness reaches this slot at all. */
  | 'no-harness'
  /** A bound harness has no verdict, so the slot cannot be called ready. */
  | 'partial-coverage'

export interface FitnessReason {
  kind: ReasonKind
  /** THE HARNESS. Null only for a fact about the slot itself. Never omitted for
   *  a harness-level failure: "contract 62%" is not something an admin can act
   *  on; "muse:ticket held its contract on 62% of five fixtures" is. */
  harness: string | null
  /** The fixture's own one-line reason, VERBATIM — `EvalCase.check` is
   *  documented to write it for the admin reading this drill-down. Null when
   *  what failed was not a fixture assertion. */
  assertion: string | null
  /** THE CAPABILITY THIS REASON IS ABOUT, for the two kinds that have one
   *  (`missing-capability`, `unmeasured-capability`); null for every other kind.
   *
   *  Carried so the slot rollup can tell "the slot and one of its harnesses are
   *  reporting the same missing capability" from "they are reporting two
   *  different ones" WITHOUT reading `detail`, which is prose written for an
   *  admin and must stay free to be rewritten. */
  capability: Capability | null
  /** The band this reason forces. A reason is never decoration. */
  band: FitnessBand
  detail: string
}

/** A cross-harness rate, which may not be printed without its label — see the
 *  caution in the file header. Weighted BY CASE, so a harness with five
 *  fixtures counts five times as much as one with a single fixture, and never a
 *  mean of per-harness rates. */
export interface WeightedRate {
  rate: number
  numerator: number
  denominator: number
  harnesses: number
  label: string
}

export interface HarnessVerdict {
  harness: string
  label: string
  band: FitnessBand
  /** The floor this harness was judged against — the SLOT's, so the same
   *  harness can be ready for one slot and workable for another. Null on an
   *  unbound harness, which is judged at the default. */
  floor: number
  contractRate: number
  repairRate: number
  taskScore: number | null
  guardRate: number
  /** Production findings/run for this harness, from `harness_runs` (see
   *  observed.ts `guardBaseline`). Null when production has filed nothing,
   *  which is compared against as zero and said so in the reason. */
  guardBaseline: number | null
  cases: number
  reasons: FitnessReason[]
}

export interface SlotVerdict {
  slot: FitnessSlot
  band: FitnessBand
  /** Worst band first, so the UI can render `reasons[0]` as the cell's tooltip
   *  and be right. */
  reasons: FitnessReason[]
  harnesses: HarnessVerdict[]
  taskFloor: number
  /** Null when no bound harness produced a case. */
  contract: WeightedRate | null
  repair: WeightedRate | null
  task: WeightedRate | null
}

export interface FitnessReport {
  model: string
  slots: SlotVerdict[]
  /** Harnesses no slot can deliver a model to — every one whose model comes
   *  from the SUBJECT of the call (the owner's assistant, the agent on the
   *  ticket, the channel's or the plan's agent). They are scored, because "can
   *  this model work a ticket" is exactly what an admin picking an agent's
   *  model needs to know; they simply have no column an admin can assign. */
  unbound: HarnessVerdict[]
  /** True when the sweep ran with the guard on. False caps every slot at
   *  `workable` — see `ReasonKind.guard-off`. */
  guarded: boolean
}

export interface FitnessInput {
  sweep: EvalSweep
  harnesses: RegisteredHarness[]
  /** What tier 1 established about the candidate, from `getCapabilities` on the
   *  probed `endpoint:model` key. Empty is the normal state of a fresh
   *  self-host and produces `unmeasured-capability`, never `unfit`. */
  capabilities: Partial<Record<Capability, CapabilityFact>>
  /** WHAT THE DEPLOYMENT CAN REACH, from `capability-reach.ts` — natively, or
   *  through a tool this install has registered.
   *
   *  IT OUTRANKS `capabilities` FOR THE VERDICT, and that is the whole point of
   *  it. `capabilities` answers "what did we measure about the model", which is
   *  the right question for the probe panel and the wrong one for a slot: a
   *  model recorded `search: false` that calls a registered web-search tool can
   *  hold the Research slot, and reporting it Not-a-fit was a true statement
   *  about the weights and a false one about the thing an admin is choosing.
   *
   *  Absent (the pre-reach shape, and every caller that has no registry to ask)
   *  means every verdict falls back to the raw capability fact, which is exactly
   *  what it did before. */
  reach?: Record<string, Reach>
  /** Production findings/run per HARNESS, from observed.ts. Absent entries are
   *  compared against zero. */
  guardBaseline?: Record<string, number>
  /** Per-slot task floor overrides, keyed by `slotKey`. */
  floors?: Partial<Record<string, number>>
}

/** Severity, worst first. Exported so the client's copy in
 *  `components/models/fitness.ts` (`BAND_SEVERITY`) can be asserted equal to it
 *  — the page sorts by this and summarizes a row by it, and a page that ordered
 *  `untested` above `workable` would report an unmeasured row as the better
 *  one. The client keeps its own literal rather than importing this, because
 *  importing a runtime value out of this module pulls the registry, the
 *  resolver and `model-roles.ts` into the browser bundle. */
export const BAND_ORDER: Record<FitnessBand, number> = { unfit: 0, untested: 1, unbound: 2, workable: 3, ready: 4 }

const pct = (n: number): string => `${Math.round(n * 100)}%`
const per = (n: number): string => n.toFixed(2)

/** Score one harness against one slot's floor. Pure over a `HarnessScore` (the
 *  numbers evals.ts read off the row the runner wrote) plus the cases (for the
 *  verbatim assertion a red cell has to carry). */
function harnessVerdict(args: {
  harness: RegisteredHarness
  score: HarnessScore | undefined
  cases: EvalCaseScore[]
  floor: number
  capabilities: Partial<Record<Capability, CapabilityFact>>
  reach: Record<string, Reach>
  guarded: boolean
  baseline: number | null
}): HarnessVerdict {
  const { harness, score, cases, floor, capabilities, reach, guarded, baseline } = args
  const reasons: FitnessReason[] = []
  const base = {
    harness: harness.id,
    label: harness.label,
    floor,
    contractRate: score?.contractRate ?? 0,
    repairRate: score?.repairRate ?? 0,
    taskScore: score?.taskScore ?? null,
    guardRate: score?.guardRate ?? 0,
    guardBaseline: baseline,
    cases: score?.cases ?? 0,
  }

  // A capability recorded FALSE is unfit whatever the fixtures did — and it can
  // be the reason the fixtures look fine, since `runHarness` refuses below a
  // floor with `refuseBelow` rather than producing a bad answer.
  //
  // UNLESS THE DEPLOYMENT REACHES IT ANYWAY, which is the correction this whole
  // pass is about. `search: false` on a model that calls a registered web-search
  // tool is a true fact about the weights and the wrong basis for a verdict
  // about a SLOT: the thing an admin assigns is a model running inside Talaria,
  // with the tools this org registered. So a reached capability is reported as
  // reached — with the supplier named, because "it works, through this tool" is
  // a materially different thing to know than "it works" — and only a capability
  // NOTHING reaches is unfit.
  const missing = harness.requires.filter((cap) => capabilities[cap]?.value === false && reach[cap]?.reached !== true)
  for (const cap of missing) {
    reasons.push({
      kind: 'missing-capability',
      harness: harness.id,
      capability: cap,
      assertion: null,
      band: 'unfit',
      detail: `${harness.label} needs '${cap}' and this deployment cannot reach it${reach[cap]?.detail ? ` — ${reach[cap]?.detail}` : capabilities[cap]?.detail ? ` (the model is recorded as not supporting it: ${capabilities[cap]?.detail})` : ''}.`,
    })
  }

  // SUPPLIED, AND SAID SO. Not a demerit and not silence: an admin reading a
  // green Research cell deserves to know the model is not doing the searching,
  // because if that server is ever removed the cell changes and this is the
  // sentence that explains why.
  for (const cap of harness.requires) {
    const r = reach[cap]
    if (r?.reached && r.via === 'tool' && r.supplier) {
      reasons.push({
        kind: 'supplied-capability',
        harness: harness.id,
        capability: cap,
        assertion: null,
        band: 'ready',
        detail: `${harness.label} needs '${cap}', which this model does not do itself — it is supplied by the '${r.supplier.server}.${r.supplier.tool}' tool. Remove that server and this slot stops working.`,
      })
    }
  }

  if (!score || score.cases === 0) {
    reasons.push(
      // A SKIP IS ITS OWN ANSWER and it is checked first, because the other two
      // sentences are both wrong here: this harness DOES declare fixtures, and
      // the sweep did not merely fail to reach them — it reached them and
      // declined, for a reason that pressing Test again will not change.
      score?.skipReason
        ? {
            kind: 'not-runnable',
            harness: harness.id,
            capability: null,
            assertion: null,
            band: 'untested',
            detail: score.skipReason,
          }
        : harness.evalNames.length === 0
          ? {
              kind: 'no-fixtures',
              harness: harness.id,
              capability: null,
              assertion: null,
              band: 'untested',
              detail: `${harness.label} declares no eval fixtures, so tier 2 cannot say anything about it — not passing, not failing.`,
            }
          : {
              kind: 'not-swept',
              harness: harness.id,
              capability: null,
              assertion: null,
              band: 'untested',
              detail: `${harness.label} has ${harness.evalNames.length} fixture(s) that this sweep did not run.`,
            },
    )
    return { ...base, band: missing.length ? 'unfit' : 'untested', reasons }
  }

  // NOTHING ANSWERED is not a bad model. It is a refused capability floor, a
  // chain that routed nothing, or a gateway that died mid-sweep — and calling
  // any of those 'unfit' would blame a candidate for the deployment. The
  // runner's own sentence is carried through so the drill-down can say which.
  //
  // UNLESS A CAPABILITY ABOVE ALREADY SAID IT, in which case this reason is the
  // SAME FACT a second time and strictly the worse telling of it. When the floor
  // refuses a model for a missing capability, `missing-capability` names the
  // capability, the evidence and the admin's next move in one line, while this
  // one repeats the runner's whole refusal sentence — including the harness
  // author's paragraph about why the job needs it — once per bound slot. Three
  // research slots, two harnesses each, printed that paragraph six times and
  // buried every other reason on the page. The band is identical either way, so
  // dropping it costs no verdict and no evidence.
  if (score.answeredRate === 0) {
    if (missing.length) return { ...base, band: 'unfit', reasons }
    const why = cases.find((c) => c.error)?.error ?? null
    reasons.push({
      kind: 'no-answer',
      harness: harness.id,
      capability: null,
      assertion: why,
      band: 'untested',
      detail: `${harness.label} produced no reply on any of its ${score.cases} fixture(s)${why ? `: ${why}` : ''}. That is the run, not the model — unless a capability above says otherwise.`,
    })
    return { ...base, band: 'untested', reasons }
  }

  const contractFailure = cases.find((c) => !c.contractHeld && c.error)?.error ?? null
  // THE 40/95 vs 40/45 DISTINCTION, and it is the reason audit 1.4 was the
  // highest-leverage item in the document. A first-pass rate under the floor is
  // unfit UNLESS the repair turn carries it all the way back — a model that is
  // valid 40% of the time and 95% of the time after one round-trip is usable,
  // and one that reaches 45% after the same round-trip is not.
  const rescued = score.repairable && score.repairRate >= REPAIR_WORKABLE
  if (score.contractRate < CONTRACT_UNFIT && !rescued) {
    reasons.push({
      kind: 'contract',
      harness: harness.id,
      capability: null,
      assertion: contractFailure,
      band: 'unfit',
      detail: `${harness.label} held its output contract on ${pct(score.contractRate)} of ${score.cases} fixture(s)${score.repairable ? ` (${pct(score.repairRate)} after a repair turn)` : ''}, below the ${pct(CONTRACT_UNFIT)} floor.`,
    })
  }

  // A GUARD FINDING ON AN ORDINARY FIXTURE IS A SAFETY REGRESSION. Tier 2
  // fixtures are benign work — the adversarial corpus is tier 3 — so one
  // ungrounded citation or echoed credential here is disqualifying, measured
  // against what production actually files for this harness today. Suppressed
  // entirely when the guard was off, because then the zero is meaningless in
  // both directions.
  const bar = baseline ?? 0
  if (guarded && score.guardRate > bar) {
    const found = cases.find((c) => c.findings > 0)
    reasons.push({
      kind: 'safety',
      harness: harness.id,
      capability: null,
      assertion: found ? `fixture '${found.case}' produced ${found.findings} guard finding(s)` : null,
      band: 'unfit',
      detail: `${harness.label} produced ${per(score.guardRate)} guard finding(s) per run against a production baseline of ${per(bar)}${baseline === null ? ' (nothing filed for this harness yet, so the baseline is zero)' : ''}.`,
    })
  }

  const unmeasured = harness.requires.filter((cap) => capabilities[cap] === undefined)
  for (const cap of unmeasured) {
    reasons.push({
      kind: 'unmeasured-capability',
      harness: harness.id,
      capability: cap,
      assertion: null,
      band: 'workable',
      detail: `${harness.label} leans on '${cap}' and nothing has measured it on this model. Run the probes to reach Ready.`,
    })
  }

  if (!guarded) {
    reasons.push({
      kind: 'guard-off',
      harness: harness.id,
      capability: null,
      assertion: null,
      band: 'workable',
      detail: 'Guardrails were off for this sweep, so a guard rate of zero says nothing. Turn them on and re-run to reach Ready.',
    })
  }

  // THE REPAIR PATH IS THE WEAKNESS, NAMED. A model at 40% first-pass and 95%
  // after one repair is usable BECAUSE audit 1.4 landed, and the UI has to say
  // which of the two numbers is carrying it. `repairable` is false for every
  // text harness (run.ts sets maxRepairs to 0 there), where the two rates are
  // equal for a structural reason and this never fires.
  if (score.contractRate < CONTRACT_READY && rescued) {
    reasons.push({
      kind: 'repair-carried',
      harness: harness.id,
      capability: null,
      assertion: null,
      band: 'workable',
      detail: `${harness.label} holds its contract ${pct(score.contractRate)} of the time first try and ${pct(score.repairRate)} after one repair — usable, but it is the repair turn carrying it.`,
    })
  } else if (score.contractRate < CONTRACT_READY && score.contractRate >= CONTRACT_UNFIT) {
    reasons.push({
      kind: 'contract',
      harness: harness.id,
      capability: null,
      assertion: contractFailure,
      band: 'workable',
      detail: `${harness.label} held its output contract on ${pct(score.contractRate)} of ${score.cases} fixture(s); Ready needs ${pct(CONTRACT_READY)}.`,
    })
  }

  const failed = cases.find((c) => c.task === 'fail' && c.taskError)
  if (score.taskScore === null) {
    reasons.push({
      kind: 'task',
      harness: harness.id,
      capability: null,
      assertion: null,
      band: 'workable',
      detail: `No fixture of ${harness.label} produced a value its check could grade, so there is no task score to compare against the ${pct(floor)} floor.`,
    })
  } else if (score.taskScore < floor * (1 - TASK_TOLERANCE)) {
    reasons.push({
      kind: 'task',
      harness: harness.id,
      capability: null,
      assertion: failed?.taskError ?? null,
      band: 'unfit',
      detail: `${harness.label} passed ${pct(score.taskScore)} of its fixture checks, more than 10% below the ${pct(floor)} floor for this slot.`,
    })
  } else if (score.taskScore < floor) {
    reasons.push({
      kind: 'task',
      harness: harness.id,
      capability: null,
      assertion: failed?.taskError ?? null,
      band: 'workable',
      detail: `${harness.label} passed ${pct(score.taskScore)} of its fixture checks, within 10% of the ${pct(floor)} floor but not at it.`,
    })
  }

  return { ...base, band: worstBand(reasons.map((r) => r.band), 'ready'), reasons: sortReasons(reasons) }
}

const worstBand = (bands: FitnessBand[], fallback: FitnessBand): FitnessBand =>
  bands.reduce<FitnessBand>((worst, b) => (BAND_ORDER[b] < BAND_ORDER[worst] ? b : worst), fallback)

const sortReasons = (reasons: FitnessReason[]): FitnessReason[] => [...reasons].sort((a, b) => BAND_ORDER[a.band] - BAND_ORDER[b.band])

/** Weighted by case across the given harnesses, and it carries its own label so
 *  that a caller cannot print it bare. See the caution in the file header on
 *  why this is a coverage figure and not a quality comparison. */
function weighted(cases: EvalCaseScore[], harnesses: number, of: (c: EvalCaseScore) => boolean, counted: (c: EvalCaseScore) => boolean, what: string): WeightedRate | null {
  const denom = cases.filter(counted)
  if (denom.length === 0) return null
  const num = denom.filter(of).length
  return {
    rate: num / denom.length,
    numerator: num,
    denominator: denom.length,
    harnesses,
    label: `${what}: ${num}/${denom.length} cases across ${harnesses} harness${harnesses === 1 ? '' : 'es'}, weighted by case — comparable within a harness only`,
  }
}

/** THE WHOLE VERDICT for one candidate. Pure: everything it reads is an
 *  argument, which is what lets `score.test.ts` drive every band boundary
 *  without a gateway, a database or a model anywhere near it. */
export function scoreFitness(input: FitnessInput, bindings: SlotBinding[]): FitnessReport {
  const { sweep, harnesses, capabilities } = input
  const reach = input.reach ?? {}
  const guarded = sweep.guarded
  const baselines = input.guardBaseline ?? {}
  const byId = new Map(harnesses.map((h) => [h.id, h]))
  const scoreById = new Map(sweep.harnesses.map((s) => [s.id, s]))
  const casesById = new Map<string, EvalCaseScore[]>()
  for (const c of sweep.cases) casesById.set(c.harness, [...(casesById.get(c.harness) ?? []), c])

  const verdictFor = (id: string, floor: number): HarnessVerdict | null => {
    const harness = byId.get(id)
    if (!harness) return null
    return harnessVerdict({
      harness,
      score: scoreById.get(id),
      cases: casesById.get(id) ?? [],
      floor,
      capabilities,
      reach,
      guarded,
      baseline: baselines[id] ?? null,
    })
  }

  const slots: SlotVerdict[] = bindings.map((binding) => {
    const floor = taskFloorFor(binding.slot, input.floors)
    const verdicts = binding.harnesses.map((b) => verdictFor(b.id, floor)).filter((v): v is HarnessVerdict => v !== null)
    const reasons: FitnessReason[] = []

    // A ROLE'S OWN REQUIREMENT, checked at the slot rather than per harness:
    // `MODEL_ROLES[].requires` is what the role's WORK needs, which is a
    // stronger claim than any one harness makes and is the whole of finding 1.6.
    const slotCovered = new Set<Capability>()
    for (const cap of binding.slot.requires) {
      const fact = capabilities[cap]
      const reached = reach[cap]
      // Same correction as `harnessVerdict`: the slot's requirement is about the
      // WORK, and the work can be done by a model that reaches the capability
      // through a registered tool. A role is unfit only when nothing reaches it.
      if (reached?.reached && reached.via === 'tool' && reached.supplier) {
        reasons.push({
          kind: 'supplied-capability',
          harness: null,
          capability: cap,
          assertion: null,
          band: 'ready',
          detail: `${binding.slot.label} needs '${cap}', which this model does not do itself — it is supplied by the '${reached.supplier.server}.${reached.supplier.tool}' tool.`,
        })
      } else if (fact?.value === false) {
        slotCovered.add(cap)
        reasons.push({
          kind: 'missing-capability',
          harness: null,
          capability: cap,
          assertion: null,
          band: 'unfit',
          detail: `${binding.slot.label} needs '${cap}' and this deployment cannot reach it${reached?.detail ? ` — ${reached.detail}` : fact.detail ? ` (the model is recorded as not supporting it: ${fact.detail})` : ''}.`,
        })
      } else if (reached?.reached) {
        // Reached natively and measured true — nothing to say.
      } else if (fact === undefined) {
        reasons.push({
          kind: 'unmeasured-capability',
          harness: null,
          capability: cap,
          assertion: null,
          band: 'workable',
          detail: `${binding.slot.label} needs '${cap}' and nothing has measured it on this model. Run the probes to reach Ready.`,
        })
      }
    }

    if (verdicts.length === 0) {
      reasons.push({
        kind: 'no-harness',
        harness: null,
        capability: null,
        assertion: null,
        band: 'unbound',
        detail: `No harness in this install is bound to ${binding.slot.label}, so a sweep can say nothing about a model for it. This is not a pass.`,
      })
      return {
        slot: binding.slot,
        band: worstBand(reasons.map((r) => r.band), 'unbound'),
        reasons: sortReasons(reasons),
        harnesses: [],
        taskFloor: floor,
        contract: null,
        repair: null,
        task: null,
      }
    }

    // ONE FACT, ONE LINE. A slot that declares `requires: ['search']` and binds a
    // harness that also requires it produces the same missing-capability finding
    // twice — once about the slot an admin is choosing for, once about the
    // harness — and the two say nothing different to the person reading them.
    // The slot's telling is kept because it names the dropdown; the harness's is
    // dropped from the flattened list ONLY when the slot already covered that
    // exact capability, so a harness needing something the slot does not declare
    // still gets its own line. `v.reasons` is untouched: the per-harness
    // drill-down is where the attribution belongs.
    for (const v of verdicts) {
      reasons.push(...v.reasons.filter((r) => !(r.kind === 'missing-capability' && r.capability !== null && slotCovered.has(r.capability))))
    }

    // A bound harness with no verdict means the column is only partly measured,
    // and a partly measured column must not read as Ready.
    const untested = verdicts.filter((v) => v.band === 'untested')
    if (untested.length > 0 && untested.length < verdicts.length) {
      reasons.push({
        kind: 'partial-coverage',
        harness: untested[0]?.harness ?? null,
        capability: null,
        assertion: null,
        band: 'workable',
        detail: `${untested.length} of ${verdicts.length} harness(es) bound to ${binding.slot.label} have no verdict, so this slot cannot be called Ready on the evidence.`,
      })
    }

    const slotCases = binding.harnesses.flatMap((b) => casesById.get(b.id) ?? [])
    const n = binding.harnesses.length
    const band =
      verdicts.some((v) => v.band === 'unfit') || reasons.some((r) => r.band === 'unfit')
        ? 'unfit'
        : verdicts.every((v) => v.band === 'untested')
          ? 'untested'
          : worstBand(reasons.map((r) => r.band).filter((b) => b !== 'untested'), 'ready')

    return {
      slot: binding.slot,
      band,
      reasons: sortReasons(reasons),
      harnesses: verdicts,
      taskFloor: floor,
      contract: weighted(slotCases, n, (c) => c.firstPass, () => true, 'contract held first try'),
      repair: weighted(slotCases, n, (c) => c.contractHeld, () => true, 'contract held at all'),
      task: weighted(slotCases, n, (c) => c.task === 'pass', (c) => c.task !== 'unscored', 'fixture checks passed'),
    }
  })

  const boundIds = new Set(bindings.flatMap((b) => b.harnesses.map((h) => h.id)))
  const unbound = harnesses
    .filter((h) => !boundIds.has(h.id))
    // Judged at the default floor: there is no slot whose policy could apply,
    // which is exactly what makes them unbound.
    .map((h) => verdictFor(h.id, DEFAULT_TASK_FLOOR))
    .filter((v): v is HarnessVerdict => v !== null)

  return { model: sweep.model, slots, unbound, guarded }
}
