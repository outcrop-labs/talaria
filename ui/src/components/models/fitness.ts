// Client side of Admin → Models → Fitness: the payload contract, the queries,
// and the pure helpers the panels render through.
//
// THE VOCABULARY LIVES HERE, ONCE. Four surfaces show a band (the matrix cell,
// the model header, the slot list, the role-assignment warning) and three show
// a capability tag (roles, platform agents, member access). One table each, so
// a colour or a word cannot mean two things on two panels.
//
// Nothing here scores. Bands come from `server/fitness/score.ts`, capability
// facts from `server/harness/capability.ts`, thresholds off the wire — the
// route sends them precisely so that "Ready needs 95%" and the arithmetic that
// decides it cannot drift apart.
//
// RUNTIME-DEPENDENCY-FREE ON PURPOSE. The queries live next door in
// `fitness-queries.ts` so that this module — where the two rules that could
// make the page lie are written down — is importable by `vitest.config.ts`'s
// plain node environment, which has no Svelte plugin and cannot load
// `@tanstack/svelte-query`.
import type { ChipTone } from '@/components/ui/chip'
import type { Capability } from '@/server/harness/capability'
import type { FitnessBand, FitnessReport } from '@/server/fitness/score'
import type { EvalCaseScore, HarnessScore } from '@/server/fitness/evals'
import type { AdversarialReport, ProvocationScore } from '@/server/fitness/adversarial'
import type { ProbeReport } from '@/server/fitness/probes'
import type { Divergence, ObservedHarness, ObservedModel } from '@/server/fitness/observed'
import type {
  CapabilityState,
  CapabilityView,
  FitnessIndexEntry,
  FitnessRunStatus,
  ModelRow,
  RunEstimate,
  TierEstimate,
  TierId,
} from '@/routes/api/admin.model-fitness'

export type {
  AdversarialReport,
  Capability,
  CapabilityState,
  CapabilityView,
  Divergence,
  EvalCaseScore,
  FitnessBand,
  FitnessIndexEntry,
  FitnessReport,
  FitnessRunStatus,
  HarnessScore,
  ModelRow,
  ObservedHarness,
  ObservedModel,
  ProbeReport,
  ProvocationScore,
  RunEstimate,
  TierEstimate,
  TierId,
}

export interface Thresholds {
  contractReady: number
  contractUnfit: number
  repairWorkable: number
  observedWindowDays: number
  minObservedRuns: number
}

export interface SlotView {
  kind: 'role' | 'agent'
  id: string
  key: string
  label: string
  hint: string
  requires: Capability[]
  live: boolean
  taskFloor: number
}

export interface MatrixPayload {
  slots: SlotView[]
  models: ModelRow[]
  index: Record<string, FitnessIndexEntry>
  status: FitnessRunStatus & { done: number; total: number; harness: string | null; sweepState: string }
  thresholds: Thresholds
  registry: { harnesses: number; fixtures: number; unfixtured: string[] }
}

export interface DetailPayload {
  model: string
  record: {
    model: string
    at: string
    tiers: TierId[]
    report: FitnessReport
    harnesses: HarnessScore[]
    cases: EvalCaseScore[]
    droppedCases: number
    probes: ProbeReport | null
    adversarial: AdversarialReport | null
    sweep: { state: string; done: number; total: number; error: string | null; unfixtured: string[] }
  } | null
  observed: ObservedHarness[]
  observedModel: ObservedModel | null
  divergences: Divergence[]
  thresholds: Thresholds
}

// ── The band vocabulary ──────────────────────────────────────────────────────

/** THE MOST IMPORTANT TABLE ON THE PAGE.
 *
 *  `untested` is neutral, never a tint that reads as a pass. The single most
 *  dangerous thing this surface could do is imply a model was checked when it
 *  was not — an admin swapping in a 14B on the strength of a green cell nobody
 *  filled would find out in production, which is the exact week-of-surprises
 *  this feature exists to delete. `unbound` is the same neutral for the same
 *  reason: no harness reaches that slot, so the sweep said nothing about it. */
export const BAND_META: Record<FitnessBand, { label: string; tone: ChipTone; glyph: string; blurb: string }> = {
  ready: {
    label: 'Ready',
    tone: 'success',
    glyph: '●',
    blurb: 'Every capability this slot needs is present, the contract held, and the fixtures passed its floor.',
  },
  workable: {
    label: 'Workable',
    tone: 'warn',
    glyph: '◐',
    blurb: 'Usable, with a named weakness — often a contract that only holds after the repair turn, or a capability nothing has measured.',
  },
  unfit: {
    label: 'Not a fit',
    tone: 'danger',
    glyph: '○',
    blurb: 'A missing capability, a contract below the floor, or a safety regression. The reason names the harness and the assertion.',
  },
  untested: {
    label: 'Untested',
    tone: 'neutral',
    glyph: '·',
    blurb: 'Nothing has measured this slot on this model. Not a pass.',
  },
  unbound: {
    label: 'No harness',
    tone: 'neutral',
    glyph: '–',
    blurb: 'No harness in this install is bound to this slot, so a run can say nothing about a model for it. Not a pass.',
  },
}

/** Text colour per band. The second half of the vocabulary, and it lives beside
 *  `BAND_META` for the same reason: the matrix draws a glyph, the detail panel
 *  draws a reason sentence and a slot label, and all three were separate copies
 *  of this table until a reconcile pass found them. A chip tone and a text
 *  colour are different Tailwind tokens, which is why this is not folded into
 *  `BAND_META.tone`. */
export const BAND_TEXT: Record<FitnessBand, string> = {
  ready: 'text-success',
  workable: 'text-warning',
  unfit: 'text-danger',
  untested: 'text-ink-dim',
  unbound: 'text-ink-dim',
}

/** The band for a cell, treating an absent record as `untested` rather than as
 *  anything else. A model nobody has run has no cells at all, and every one of
 *  them must read grey. */
export const bandOf = (entry: FitnessIndexEntry | undefined, slotKey: string): FitnessBand => entry?.cells[slotKey]?.band ?? 'untested'

export const reasonOf = (entry: FitnessIndexEntry | undefined, slotKey: string): string | null =>
  entry?.cells[slotKey]?.reason ?? null

/** Band severity, worst first — for the row summary and for the detail panel's
 *  "worst first" slot ordering. `untested` sits BELOW `workable`: a row nobody
 *  has measured must not summarize as better than a row with a known weakness.
 *
 *  This is the client's copy of `score.ts`'s private `BAND_ORDER`, and it is a
 *  copy on purpose — importing a runtime value out of `server/fitness/score.ts`
 *  would pull the registry, the resolver and `model-roles.ts` into the browser
 *  bundle for five integers. `fitness.test.ts` asserts the two agree, so the
 *  copy cannot rot silently. */
export const BAND_SEVERITY: Record<FitnessBand, number> = { unfit: 0, untested: 1, unbound: 2, workable: 3, ready: 4 }

export function rowSummary(entry: FitnessIndexEntry | undefined, slots: SlotView[]): { band: FitnessBand; counts: Record<FitnessBand, number> } {
  const counts: Record<FitnessBand, number> = { ready: 0, workable: 0, unfit: 0, untested: 0, unbound: 0 }
  // Seeded at the BEST band and walked down. Seeding at `untested` looks safer
  // and is the bug: it is not the worst band, so a row of nothing but Workable
  // cells could never climb out of it and every measured row would summarize as
  // unmeasured. With no slots at all there is nothing to summarize, and
  // `untested` is the honest answer.
  let worst: FitnessBand = slots.length === 0 ? 'untested' : 'ready'
  for (const slot of slots) {
    const band = bandOf(entry, slot.key)
    counts[band] += 1
    if (BAND_SEVERITY[band] < BAND_SEVERITY[worst]) worst = band
  }
  return { band: worst, counts }
}

// ── Capability tags ──────────────────────────────────────────────────────────

/** Plain words for a capability, for the sentence an admin reads when their
 *  assignment is a bad fit. "needs 'search'" is a field name; "cannot search
 *  the web" is a fact they can act on. */
export const CAPABILITY_WORDS: Record<Capability, { short: string; plain: string }> = {
  json: { short: 'json', plain: 'return JSON on request' },
  'json-strict': { short: 'json+', plain: 'hold a nested JSON schema reliably' },
  tools: { short: 'tools', plain: 'call tools' },
  'tool-select': { short: 'tool pick', plain: 'pick the right tool from several' },
  search: { short: 'search', plain: 'search the web' },
  vision: { short: 'vision', plain: 'read images' },
  'long-context': { short: 'long ctx', plain: 'hold a long context' },
  code: { short: 'code', plain: 'write working code' },
  'instruction-following': { short: 'instr', plain: 'follow an exact instruction' },
}

export const TAG_TONE: Record<CapabilityState, ChipTone> = { yes: 'success', no: 'danger', unknown: 'neutral' }

/** THE TAG SENTENCE, and the distinction the whole capability model rests on:
 *  a tag must say whether a fact is KNOWN-TRUE, KNOWN-FALSE or NEVER-MEASURED.
 *  Unknown is not false — Talaria has to keep working on a model nobody has
 *  benchmarked, so an unmeasured tag is an invitation to probe, not a warning. */
export function tagTitle(view: CapabilityView): string {
  const word = CAPABILITY_WORDS[view.cap].plain
  if (view.state === 'unknown') {
    return view.detail ?? `Nobody has measured whether this model can ${word}. Unknown is not a no — run the probes to find out.`
  }
  const verdict = view.state === 'yes' ? `Measured: this model can ${word}.` : `Measured: this model cannot ${word}.`
  const how =
    view.source === 'probe'
      ? 'From a probe run'
      : view.source === 'learned'
        ? 'Learned from what the provider rejected'
        : 'Declared by an admin or a model catalog'
  const score = view.score === null ? '' : ` (${Math.round(view.score * 100)}% of trials)`
  return `${verdict} ${how}${score}${view.detail ? ` — ${view.detail}` : ''}.`
}

/** Tags worth showing next to a model chip. An all-unknown model would print
 *  nine grey chips and say nothing, so a row with NO measured facts collapses
 *  to a single "untested" tag the caller renders instead. */
export function visibleTags(row: ModelRow | undefined): { measured: CapabilityView[]; anyMeasured: boolean } {
  const measured = (row?.capabilities ?? []).filter((c) => c.state !== 'unknown')
  return { measured, anyMeasured: measured.length > 0 }
}

// ── The assignment warning (audit 1.6, delivered as a sentence) ──────────────

export interface AssignmentNotice {
  band: FitnessBand
  /** One sentence, plain words, naming what failed. Never a validation error:
   *  the admin may know something the probe does not, and the assignment
   *  stands either way. */
  text: string
}

/** What to say under a slot whose assigned model tested badly for it.
 *
 *  Two independent sources, and they are kept apart on purpose. `capabilityNote`
 *  is `roleAssignmentIssues` on the server — a capability recorded FALSE, which
 *  is a fact about the MODEL. The band is a fact about the RUN. A model can be
 *  unfit for either reason and the sentence should say which; when both fire,
 *  the capability is the more actionable and goes first. */
export function assignmentNotice(args: {
  entry: FitnessIndexEntry | undefined
  slotKey: string
  capabilityNote?: string | null
}): AssignmentNotice | null {
  if (args.capabilityNote) return { band: 'unfit', text: args.capabilityNote }
  const band = bandOf(args.entry, args.slotKey)
  if (band !== 'unfit') return null
  const reason = reasonOf(args.entry, args.slotKey)
  return {
    band,
    text: reason
      ? `${reason} You can still assign it — this is what the last test found, not a rule.`
      : 'This model tested Not a fit for this slot. You can still assign it — this is what the last test found, not a rule.',
  }
}

// ── Formatting ───────────────────────────────────────────────────────────────

export const pct = (n: number): string => `${Math.round(n * 100)}%`

/** Dollars at a scale a fitness run actually costs. A run is often cents, and
 *  "$0.00" for a real four-cent spend is the kind of rounding that makes an
 *  admin distrust every other number on the page. */
export function usd(n: number | null): string {
  if (n === null) return 'unpriced'
  if (n === 0) return '$0'
  if (n < 0.01) return '<$0.01'
  return `$${n.toFixed(2)}`
}

export const TIER_META: Record<TierId, { label: string; blurb: string }> = {
  probes: {
    label: 'Probes',
    blurb: 'Model-level facts: JSON mode, tools, search, long context, instruction following. Seconds, cents. This is what fills the capability tags.',
  },
  evals: {
    label: 'Harness conformance',
    blurb: 'Every harness’s own fixtures, replayed against this model. Contract rate, repair rate and the fixture assertions. The slow one.',
  },
  adversarial: {
    label: 'Adversarial',
    blurb: 'Safety provocations scored with the production guard rules. Opt-in, and the one tier where a strong adversary model is a requirement.',
  },
}

/** The estimate, as one line an admin can decide on. Exported and pure because
 *  the sentence in front of a Start button that spends real money is worth a
 *  test. */
export function estimateSentence(est: RunEstimate | null): string {
  if (!est) return 'Pricing this run'
  const calls = `${est.calls} call${est.calls === 1 ? '' : 's'}`
  if (!est.priced) return `${calls}. Nothing on this install prices ${est.model}, so there is no dollar figure — the call count is exact.`
  if (est.usd === null) return `${calls}. Part of this run could not be priced, so no total is shown.`
  const floor = est.unmeasuredHarnesses > 0 ? ' at least' : ' about'
  return `${calls}, costing${floor} ${usd(est.usd)}.`
}
