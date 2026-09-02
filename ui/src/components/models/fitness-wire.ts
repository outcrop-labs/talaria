// THE FITNESS WIRE SHAPES, in the browser tree — the contract Admin → Models
// reads, served by the Rust twin in `api/src/fitness/`. These definitions
// lived in the TS fitness engines; the engines are
// gone with the Rust cutover and only their SHAPES cross the wire, so the
// shapes live where their only consumer lives. The Rust side serializes
// camelCase field-for-field — a field renamed here is a field the fitness
// pages render as blank, which is the failure this file exists to prevent.
//
// TYPES ONLY. Nothing here may import a module with runtime behavior: this
// file is pulled into `vitest.config.ts`'s plain node environment, which has
// no Svelte plugin (see the header note in `./fitness`).
import type { Capability, CapabilityFact, CapabilityKey } from '@/server/harness/capability'
import type { EvalBand } from '@/server/harness/define'
import type { ToolPolicy } from '@/server/harness/transport'
import type { Finding } from '@/server/guardrails'
import type { GatewayPulse } from '@/server/llm-gateway'
import type { ModelRole } from '@/server/model-roles'
import type { PlatformAgentId } from '@/server/platform-agents'

/** Where a registered harness came from. Lifted from the TS harness registry,
 *  which the cutover deleted; the Rust twin spells the same union
 *  (`api/src/harness/registry.rs`). */
export type HarnessSource = 'builtin' | `app:${string}` | 'custom'

/** THE THREE KINDS OF ASSIGNMENT AN ADMIN MAKES, and the third was missing.
 *
 *  'role' and 'agent' are the two registries an admin picks a model from on the
 *  Models page. 'fleet' is the one nobody had modelled: the model behind a
 *  HERMES PERSONA — the containerized agent that works tickets, answers a
 *  channel, briefs an owner and drives the workspace toolkit.
 *
 *  ITS ABSENCE MADE TWELVE HARNESSES INVISIBLE. `work-session`, `channel-plan`,
 *  `plan-doc`, `outreach:check-in`, all three Inbox harnesses, both briefers,
 *  research-queries and research-synthesis all declare `model: { chain: [] }`
 *  because production pins the SUBJECT of the call — the agent on the ticket, in
 *  the channel, on the plan. So they bound to no slot, and the fitness matrix —
 *  whose columns ARE the slots — had no column for any of them. They were
 *  measured, scored and archived into a report with nowhere to appear.
 *
 *  That is the largest single consumer of models in the product, and the page an
 *  admin uses to choose one said nothing about it. */
export type SlotKind = 'role' | 'agent' | 'fleet'

/** The fleet slots. TWO, NOT ONE, because they are genuinely different jobs and
 *  an org routinely runs different models behind them: a personal assistant
 *  reads one owner's inbox and drafts in their voice, while a workspace agent
 *  works tickets and drives the toolkit against a shared board. A single column
 *  would average a model's fitness for both and be right about neither. */
export type FleetSlotId = 'assistant' | 'agent'

/** One assignment an admin can make, normalized across the two registries that
 *  offer them. `requires` is populated for roles only: `MODEL_ROLES` declares
 *  what a role's WORK needs (audit 1.6), while a platform agent declares
 *  nothing of the sort — its harnesses carry the requirement instead, and this
 *  file reads them there. */
export interface FitnessSlot {
  kind: SlotKind
  id: ModelRole | PlatformAgentId | FleetSlotId
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

export type BindingVia = 'chain' | 'pin' | 'declared'

export type FitnessBand = 'ready' | 'workable' | 'unfit' | 'untested' | 'unbound'

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
   *  a pass. The Rust twin spells the rule in `api/src/fitness/evals.rs`. */
  | 'not-runnable'
  /** A required capability the MODEL lacks and a registered TOOL supplies. Band
   *  `ready` — it is a fact worth stating, never a demerit. See
   *  the Rust reach engine. */
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
   *  the Rust observed engine). Null when production has filed nothing,
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

/** One fixture, replayed once against the candidate. */
export interface EvalCaseScore {
  harness: string
  /** `EvalCase.name`. Unique within a harness; `caseKey` joins the two. */
  case: string
  /** `EvalCase.band`, defaulted. Carried on the CASE rather than looked up from
   *  the archive later, because a report read back has to keep
   *  meaning what it meant when it was written — a fixture re-banded next
   *  quarter must not silently re-band last quarter's run. */
  band: EvalBand

  /** THE SWEEP NEVER CALLED THE MODEL, and this is not a result about it.
   *
   *  Non-null carries the sentence saying why, written for the admin reading the
   *  drill-down. Every field below it is a zero that means "not measured", and
   *  `scoreHarness` excludes the case from every rate rather than averaging the
   *  zeros in — which is the whole reason the field exists.
   *
   *  THE NUMBER THIS REPLACES WAS A LIE, and an expensive one. Harnesses
   *  whose whole feature is the tool loop (`work-session`,
   *  `outreach:check-in`) declare `tools: 'own'` because the
   *  loop IS the feature. Replayed against an
   *  ORG GATEWAY candidate, `gatewayToolsRefusal` refuses each one in about four
   *  milliseconds, before a token is spent — and the sweep recorded that as
   *  `contractHeld: false`, so the matrix printed "0% first pass" for a model
   *  nothing had asked a question. An admin reading three red rows concludes the
   *  candidate cannot hold a contract; the truth is that this install has no way
   *  to test it, which is a fact about the install.
   *
   *  Tier 1 already had this distinction — `ProbeOutcome` has a `skipped` kind
   *  and the tool probes use it the moment `offersToolDefinitions` says no. Tier
   *  2 did not, and the two tiers disagreed about the same model on the same
   *  page.
   *
   *  IT ALSO COVERS "WE ASKED AND NEVER GOT AN ANSWER": a case the provider
   *  rate-limited on every attempt. The reading is the
   *  same one — nothing was measured — and the alternative is a red cell that
   *  means "your provider was busy" and is read as "this model cannot hold a
   *  contract". */
  skipped: string | null

  /** THE CONTRACT HELD — `harness_runs.schema_valid`, taken from the row the
   *  runner wrote rather than recomputed here. See the file header. */
  contractHeld: boolean
  /** It held WITHOUT a repair turn. `repairs === 0 && contractHeld`, which is
   *  exact: the runner's attempt loop breaks the moment the contract holds, so
   *  a zero repair count on a valid run means the first reply was valid. */
  firstPass: boolean
  repairs: number
  /** The model produced a reply the contract could be applied to. False for
   *  every way a run ends without one — the capability floor refused it, the
   *  chain routed nothing, the transport died — and in each of those `error`
   *  carries the runner's own sentence naming which. That sentence is the
   *  drill-down; this flag alone cannot tell a refused floor from a dead
   *  gateway, and re-deriving the floor predicate here to find out would be a
   *  second copy of the rule the harness runner owns. */
  answered: boolean

  /** THE FIXTURE'S OWN DETERMINISTIC CHECK.
   *  'unscored' when the contract failed: there is no model value to grade, and
   *  counting a contract failure as a task failure would double-charge one
   *  fault. Also 'unscored' for a harness whose `onFailure` is `{ fallback }` —
   *  the runner hands back the DECLARED SAFE VALUE with `schemaValid: false`,
   *  and grading that would award the model task points for a constant its
   *  author wrote. */
  task: 'pass' | 'fail' | 'unscored'
  /** The fixture's one-line reason, verbatim. This is what an admin reads in
   *  the drill-down, which is why `EvalCase.check` is documented to write it
   *  for a human rather than for a developer. */
  taskError: string | null
  /** OUR GAP, not the model's failure. Non-null means this fixture could not
   *  fairly ask its question — the run was never given what the assertion
   *  demanded — so the case is `unscored` and this sentence is reported to the
   *  people who own the harness. `CheckResult` still exists in `harness/define`. */
  gap: string | null

  /** Guard findings this run is EVIDENCE for — `harness_runs.findings`, which
   *  excludes grounded hits exactly as `recordFindings` does. */
  findings: number
  /** THE RUNNER'S OWN MEASURE of the final attempt — `harness_runs.latency_ms`,
   *  covering render, the model turns, the repair round-trip and the guard pass.
   *  It is what the observed-vs-tested comparison is computed from, so it must
   *  stay the same number production records. */
  latencyMs: number
  /** WHEN THE SWEEP STARTED THIS CASE, absolute. Needed to reconstruct a
   *  timeline: under concurrency, `latencyMs` alone cannot tell a slow model
   *  from four fast cases queued behind each other, and "which cases were in
   *  flight together" is the first question a speed comparison asks. */
  startedAt: string
  /** WHAT THE CASE COST THE SWEEP, wall clock, INCLUDING everything `latencyMs`
   *  excludes: sandbox construction, the closing turn of a tool loop, and — the
   *  big one — every retry of a rate-limited or lost request. A case whose first
   *  two attempts vanished and whose third took 4s has `latencyMs: 4000` and
   *  `wallMs: 124000`, and only the second number explains where the sweep's
   *  afternoon went. */
  wallMs: number
  promptTokens: number
  completionTokens: number
  /** Null when nothing priced the tokens — see `EvalDeps.price`. */
  costUsd: number | null
  /** True when the token counts are a chars/4 estimate because the transport
   *  reported no usage. A cost built on estimated tokens is an estimate. */
  estimated: boolean

  /** The case did not settle inside the bound and the sweep moved on. A hanging
   *  harness must never strand the sweep. */
  timedOut: boolean

  /** THE CONTRACT HELD AND THE FIXTURE REJECTED THE VALUE ANYWAY.
   *
   *  Not automatically a bug, and the two readings are worth keeping apart:
   *
   *    EXPECTED where the fixture grades QUALITY the contract deliberately does
   *    not police. The judge agreeing with a label, a title being 3-7 words, a
   *    blurb fitting on one line: blurb-writer's header says in so many words
   *    that length is measured and not enforced, because failing a batch of ten
   *    over one long sentence would cost nine good ones. That is `taskScore`
   *    doing its job.
   *
   *    A BUG where the fixture is asserting something the CALLER depends on —
   *    the keys are the ids that were sent, the elements are tickets from the
   *    transcript, the date is one the write path accepts. Then production is
   *    recording `schema_valid: true` for a value the caller will throw away,
   *    which is audit finding 1.1 with green telemetry over it. The fix is
   *    `output.verify` (harness/define), not a change here.
   *
   *  `HarnessScore.verifies` is the tell: a JSON harness with fixtures and no
   *  `verify` has no way to express the second kind at all. */
  optimistic: boolean

  /** The runner's failure sentence, redacted and bounded by `run.ts`. Null on a
   *  clean run. */
  error: string | null
  /** DRILL-DOWN, kept only for cases that failed something — the actual prompt
   *  and the actual response, which is what makes a red cell trustworthy
   *  instead of merely alarming. A clean case carries neither: seventy passing
   *  transcripts in a settings row is an archive, not telemetry. */
  prompt: string | null
  raw: string | null

  /** THE WHOLE CONVERSATION, for a case that had one.
   *
   *  `prompt`/`raw` above are the FIRST request and the LAST reply, which is the
   *  entire story of a single-shot structured harness and almost none of the
   *  story of a tool loop. A dry run is six model turns, up to eighteen tool
   *  calls and their results; reading its verdict without them means reading
   *  "never called get_ticket" and having to take it on faith.
   *
   *  Null for a case that took one turn (there is nothing here `prompt`/`raw` do
   *  not already say) and for a clean one (see `prompt`). */
  turns: EvalTurn[] | null

  /** EVERY UPSTREAM CALL THIS CASE MADE, kept whenever the case did not finish
   *  cleanly. The evidence behind a timeout: how many requests went out, how
   *  long each took, and which one never came back. */
  upstream: UpstreamAttempt[] | null

  /** WHAT THE MODEL ACTUALLY DID — `Sandbox.calls`, verbatim and in order.
   *
   *  Kept for EVERY dry-run case including the passing ones, unlike the
   *  transcript, because it is small and because it is the primary artifact
   *  every behavioural fixture asserts over. An admin comparing two models on
   *  the same fixture is comparing these lists; making them available only on
   *  failure would mean the interesting comparison is the one you cannot see. */
  calls: EvalToolCall[] | null
}

/** One turn of a recorded conversation. Mirrors `Message` in harness/define.ts
 *  rather than reusing it: this shape is PERSISTED into a settings row and read
 *  back by a UI months later, so it is flat, bounded, and free of anything the
 *  runner might redefine. */
export interface EvalTurn {
  role: 'system' | 'user' | 'assistant' | 'tool'
  content: string
  /** Names only. The arguments are on `calls`, where they are not duplicated
   *  once per turn. */
  toolCalls?: string[]
}

/** One tool call as the sandbox saw it. */
export interface EvalToolCall {
  tool: string
  /** JSON, bounded. Rendered rather than parsed by the UI. */
  args: string
  /** What came back, bounded — the model saw this, so an admin should too. */
  result: string | null
  /** The tool refused. A refusal is a real event and reads very differently
   *  from a call that never happened. */
  error: string | null
}

/** Everything the sweep needs to know about a harness that is not a score.
 *  Split out so `scoreHarnesses` stays pure over recorded cases. */
export interface HarnessMeta {
  id: string
  label: string
  source: HarnessSource
  outputKind: 'text' | 'json'
  /** The model's-own-tool-loop policy. Read by the skip rule in the Rust twin, which needs
   *  it before the harness runs. */
  tools: ToolPolicy
  requires: Capability[]
  /** Does this harness declare the input-relational half of its contract? See
   *  `EvalCaseScore.optimistic`. */
  verifies: boolean
  /** CAN a repair turn happen here at all? `run.ts` sets `maxRepairs` to 0 for
   *  every text harness — the one repair wording is shared, and ends
   *  "send the corrected JSON value only", which is nonsense to a titler — and
   *  most harnesses are text.
   *
   *  Without this flag `repairRate` and `contractRate` are equal on those
   *  thirteen for a structural reason, and `repairYield` would read 0 ("the
   *  repair turn rescued nothing") where the honest answer is "no repair turn
   *  was ever sent". A benchmark that prints a rescue rate for a round-trip
   *  that cannot occur is exactly the kind of confidently wrong number this
   *  whole audit is about. */
  repairable: boolean
}

/** The per-harness column of the fitness matrix. */
export interface HarnessScore extends HarnessMeta {
  /** Fixtures this sweep RAN — skipped ones are not cases, they are absences.
   *
   *  Every rate below is over this denominator, so a harness the candidate
   *  cannot be tested on reports zero of everything with `cases: 0`, which every
   *  consumer already reads as "no evidence". A denominator that counted the
   *  skips would print 0% instead, and 0% is a verdict. */
  cases: number
  /** Fixtures the sweep declined to run against this candidate, with the reason
   *  on `skipReason`. Reported so a full-green matrix can still say what it did
   *  not look at. */
  skipped: number
  /** FIXTURES THAT COULD NOT FAIRLY ASK THEIR QUESTION — our gap, not the
   *  model's failure. Excluded from `taskScore` for the same reason a skip is
   *  excluded from `contractRate`: a denominator that counts them turns a hole
   *  in the test environment into a number about the model. `gapReasons` is what
   *  the run reports back to whoever owns the harness. */
  gaps: number
  gapReasons: string[]
  /** Why, verbatim from the first skipped case. Null when nothing was skipped. */
  skipReason: string | null
  /** Cases that reached a verdict — everything except a timeout. */
  scored: number

  /** THE CONTRACT HELD ON THE FIRST ATTEMPT, over all cases. */
  contractRate: number
  /** THE CONTRACT HELD AT ALL, over all cases — CUMULATIVE, so it is always
   *  >= `contractRate` and the pair reads the way the audit states it: 40/95 is
   *  a usable model with a repair path, 40/45 is not. A conditional rate would
   *  print 92 and 8 for those two and bury the decision. */
  repairRate: number
  /** Of the cases that failed first, the share the repair turn RECOVERED. The
   *  conditional number, kept because it is the one that says whether spending
   *  a second round-trip on this model is worth it.
   *
   *  Null when the question does not arise: nothing failed first, or the
   *  harness cannot repair at all (`repairable`). Zero means the repair turn
   *  ran and rescued nothing, which is a different and much worse fact. */
  repairYield: number | null

  /** Fixture checks passed, over the cases that were task-scorable. Null when
   *  none were — a model that never held the contract has no task score, and
   *  printing 0 would blame it twice. */
  taskScore: number | null
  /** THE SAME NUMBER, SPLIT BY DIFFICULTY. Null per band when that band had
   *  nothing scorable.
   *
   *  WHY IT IS WORTH THE FIELD: one flat rate cannot tell "competent, loses the
   *  hard edge cases" from "unreliable on the basics", and those are different
   *  purchasing decisions. A 70% that is easy 100 / standard 100 / hard 20 is a
   *  fine Utility model; a 70% that is easy 70 / standard 70 / hard 70 is a
   *  model that fails one job in three at random, which is worse in every way
   *  that matters. */
  bandScores: Record<EvalBand, number | null>
  /** Findings per run. */
  guardRate: number
  answeredRate: number

  latencyP50: number
  latencyP95: number
  promptTokens: number
  completionTokens: number
  /** Sum of the priced cases; null when nothing was priced. */
  costUsd: number | null
  estimated: boolean

  timeouts: number
  optimistic: number
}

/** The finished (or stopped) sweep, scored. */
export interface SweepConcurrency {
  /** What the run asked for. */
  requested: number
  /** What it ended at. Below `requested` means the valve was still closed when
   *  the sweep finished; back AT `requested` means it closed and reopened, which
   *  is the ordinary shape of a run that hit one bad minute. */
  ended: number
  /** THE NARROWEST IT EVER RAN, which is the number that explains the timings.
   *  `ended` alone cannot: a sweep that spent two hundred cases at width 1 and
   *  recovered on the last ten ends at 4 and looks like it never struggled. */
  low: number
  /** THE PROVIDER PUSHING BACK, verbatim, whether or not there was width left to
   *  give up. Non-null is a fact about the DEPLOYMENT and must never be read as
   *  a fact about the model — it is the reason beside every case this run had to
   *  mark unmeasured. */
  narrowedBecause: string | null
}

/** The case a sweep is on, as it happens. */
export interface InFlightCase {
  harness: string
  case: string
  band: EvalBand
  /** Epoch ms. The UI turns this into "running for 12s", which is the number
   *  that tells a watcher whether a sweep is working or wedged. */
  startedAt: number
  /** Model turns started, and the most this case may take (`turnsPerCase`). */
  turn: number
  maxTurns: number
  /** Upstream calls issued, and how many have no reply yet. */
  calls: number
  open: number
  /** THE CONVERSATION SO FAR, trimmed — what "show the turns currently testing"
   *  actually means. Rebuilt from the request on every turn, because the request
   *  IS the conversation up to that point. */
  turns: EvalTurn[]
}

export type TierId = 'probes' | 'evals' | 'adversarial'

/** THE MATRIX ROW, and the only thing the matrix read touches. Kept apart from
 *  the full report so drawing 30 models x 20 slots is ONE settings read rather
 *  than thirty multi-hundred-kilobyte ones. */
/** The speed half of a run, computed once where the sweep is scored. */
export interface SpeedReading {
  /** THE HEADLINE: output tokens per second, median over the cases this pass
   *  measured.
   *
   *  WHY NOT TIME-PER-CASE, which this used to lead with. A fixture that asks
   *  for a SKILL.md takes longer than one that asks for a chat title on any
   *  model, so a per-case latency is mostly a fact about which fixtures ran —
   *  and it moves whenever the corpus does. Tokens per second divides that out:
   *  it is the rate the model generates at, comparable between two models that
   *  ran different fixtures and between two runs of a corpus that changed.
   *
   *  Null when nothing measured both a duration and a completion, which is the
   *  honest answer for a sweep of contract failures. */
  tokensPerSecond: number | null
  /** Median per-case latency, ms — the runner's own measure, so it is the same
   *  number `harness_runs` records and observed-vs-tested compares against.
   *  Kept beside the rate because "how fast does it generate" and "how long do I
   *  wait for a fixture" are both real questions. */
  p50: number
  p95: number
  /** Wall clock of the whole sweep, from the first case starting to the last
   *  one finishing. NOT the sum of the latencies: under concurrency the run is
   *  shorter than its parts, and with retries a case costs more than it says. */
  elapsedMs: number
  /** Fixtures per minute — the figure that answers "how long will testing the
   *  next candidate take me". */
  perMinute: number
  /** Cases in flight while this was measured. See `FitnessIndexEntry.speed`. */
  concurrency: number
  /** HOW MANY CASES THIS READING IS OVER. A supplemental pass of seven fixtures
   *  gives a real but small sample, and a median over seven is a different claim
   *  from a median over two hundred and forty — the panel says which. */
  sample: number
}

export interface FitnessIndexEntry {
  model: string
  /** When the run that produced these bands finished. The matrix prints it —
   *  a verdict with no date is a verdict an admin cannot judge the age of. */
  at: string
  tiers: TierId[]
  guarded: boolean
  /** slotKey → the cell. `reason` is `SlotVerdict.reasons[0].detail`, which
   *  worst-band-first ordering (`api/src/fitness/score.rs`) makes this the right one. */
  cells: Record<string, { band: FitnessBand; reason: string | null }>
  /** Tier 3, which is a fact about the MODEL rather than about a slot, so it
   *  never colors a cell. See the note on `safety` in the payload below. */
  safety: { band: AdversarialBand; resistance: number | null } | null
  probesWrote: number
  /** HOW FAST THIS MODEL IS IN THIS INSTALL, measured over the same 247
   *  fixtures every candidate runs — which is what makes the number comparable
   *  down a column at all. Null when tier 2 did not run.
   *
   *  IT CARRIES THE WIDTH IT WAS MEASURED AT, and that is not decoration: at
   *  four in flight a per-case latency includes queueing at the provider, so a
   *  p50 from a 4-wide sweep and a p50 from a sequential one are two different
   *  measurements wearing one name. The column says so rather than letting an
   *  admin compare them silently. */
  speed: SpeedReading | null
  costUsd: number | null
  calls: number
  /** The sweep did not finish (stopped, or interrupted by a deploy). Every
   *  unrun harness is already `untested` in the cells; this says the RUN was
   *  partial so the page can say so once, at the top, instead of the admin
   *  inferring it from a scatter of grey. */
  partial: boolean
  /** PER HARNESS, the small half of the report — what the value view needs to
   *  weigh a verdict against how much of a real day that harness is, without
   *  reading two dozen multi-hundred-kilobyte records to draw one table.
   *
   *  `band` is `harnessBands`' worst-across-slots collapse, and covers the
   *  UNBOUND harnesses the cells cannot speak for. `prompt`/`completion` are
   *  per-case tokens THIS model spent, which is the only basis on which a terse
   *  model and a chatty one price differently.
   *
   *  OPTIONAL, and stays that way: entries archived before this field existed
   *  are still valid rows. The Rust value view backfills one from the full report on
   *  read, and degrades to the cells and the shared token budget if even that
   *  is gone — never a demand to re-test a model an admin already paid for. */
  harnesses?: Record<string, HarnessSummary>
}

export type FitnessIndex = Record<string, FitnessIndexEntry>

/** Per-case token counts MEASURED by the last sweep that ran each harness.
 *  This is what makes the tier-2 estimate an estimate rather than a guess: the
 *  fixtures are fixed, so the tokens one costs are close to the tokens it cost
 *  last time. A harness nobody has run yet contributes nothing and is COUNTED,
 *  so the UI can say the figure is a floor instead of quietly understating. */
export type CapabilityState = 'yes' | 'no' | 'unknown' | 'supplied'

/** One capability as the UI shows it. `state` is three-valued and that is the
 *  whole point: KNOWN-TRUE, KNOWN-FALSE and NEVER-MEASURED are three different
 *  facts, and `missingCapabilities` only ever treats the middle one as a lack.
 *  A two-valued tag would turn every unprobed model on a fresh self-host into a
 *  wall of red. */
export interface CapabilityView {
  cap: Capability
  state: CapabilityState
  source: CapabilityFact['source'] | null
  detail: string | null
  score: number | null
  at: string | null
  /** What supplies it, when `state` is 'supplied'. Named so the tag can say
   *  WHICH tool — "supplied" with no attribution is a claim an admin cannot
   *  check, and the supplier is the thing that might be switched off tomorrow. */
  via: { server: string; tool: string } | null
}

export interface ModelRow {
  id: string
  /** `endpoint/model` — one endpoint, one capability key, probeable. */
  qualified: boolean
  endpoints: string[]
  /** A bare id served by MORE THAN ONE endpoint. Capability is a property of
   *  the endpoint, so a pooled id's facts can disagree — and `runProbes`
   *  refuses to write under a pooled key for exactly that reason. The row is
   *  shown, its facts are shown where every member agrees, and the UI points at
   *  the endpoint-qualified ids for a run. */
  pooled: boolean
  capabilities: CapabilityView[]
}

export interface ModelPrice {
  in: number
  out: number
}

export interface TierEstimate {
  tier: TierId
  calls: number
  promptTokens: number
  completionTokens: number
  usd: number | null
  /** Says what the tokens ARE, because they are not the same kind of number in
   *  every tier and an admin comparing them deserves to know which. */
  basis: 'fixture' | 'measured' | 'ceiling'
  note: string
}

export interface RunEstimate {
  model: string
  adversaryModel: string | null
  tiers: TierEstimate[]
  calls: number
  usd: number | null
  /** False when nothing prices this model. The call count is still exact, and
   *  it is the number that does not depend on a catalog being reachable. */
  priced: boolean
  /** Harnesses whose per-case tokens nothing has measured yet. Non-zero means
   *  the tier-2 dollar figure is a FLOOR, and the UI says so. */
  unmeasuredHarnesses: number
  fixtures: number
}

/** ONE LINE PER CASE, FOR THE LIVE FEED.
 *
 *  `liveCases` above sends the failures plus a handful of recent ones, because a
 *  full `EvalCaseScore` carries a prompt, a reply and a tool log and a poll every
 *  three seconds cannot ship 250 of those. But an admin watching a sweep wants to
 *  SEE IT MOVING — which fixture is running, what each one decided, how long it
 *  took — and a list that only shows failures looks identical whether the sweep
 *  is flying or wedged.
 *
 *  So this is the other half: every case that has landed, reduced to the fields a
 *  terminal line needs. ~90 bytes each, so the whole 250-fixture sweep is about
 *  20KB — an order of magnitude under what one failed case's transcript costs.
 *
 *  NEWEST LAST, because it is a log and a log reads downward. */
export interface EvalLogLine {
  harness: string
  case: string
  /** What happened, in the vocabulary the terminal colours by. `gap` is OURS —
   *  the fixture could not fairly ask its question — and is deliberately its own
   *  verdict rather than folded into `fail`. */
  verdict: 'pass' | 'fail' | 'gap' | 'skip' | 'timeout' | 'error'
  ms: number
  tokens: number
  /** Tool calls the case made, for a dry run. Zero elsewhere. */
  calls: number
  /** UPSTREAM CALLS the case made, and how many never came back. The two
   *  numbers that turn a timeout from a symptom into a diagnosis: `1/1 open`
   *  is a request that hung, `4/0 open` is a case that spent its budget on
   *  retries, and `0` is time that went somewhere before the provider. */
  up: { calls: number; open: number } | null
  /** The fixture's own sentence, when there is one to show. */
  note: string | null
}

export interface MatrixView {
  slots: SlotView[]
  models: ModelRow[]
  index: FitnessIndex
  /** EVERY RUN, not one. Up to `max` candidates are tested at once, and a
   *  single `status` field could only ever draw one of them — which is how an
   *  admin who started three would watch two of them vanish. */
  runs: FitnessStatusView[]
  max: number
  full: boolean
  /** The five band-threshold numbers, sent rather than restated — a cell
   *  tooltip prints "contract 91%, Ready needs 95%", and a second copy of the
   *  constants in the client is how that sentence and the scorer come to
   *  disagree. */
  thresholds: {
    contractReady: number
    contractUnfit: number
    repairWorkable: number
    observedWindowDays: number
    minObservedRuns: number
  }
  registry: { harnesses: number; fixtures: number; provocations: number; unfixtured: string[] }
}

/** A RUN IN FLIGHT, as the drill-down can show it.
 *
 *  The sweep checkpoints every case as it lands, so the audit trail an admin
 *  wants while a run is working already exists — it was simply never sent. The
 *  drill-down read only the ARCHIVED record, so opening a model mid-run said
 *  "no run on record" about a sweep that was at that moment 140 fixtures in.
 *
 *  Bounded by the same `drilldown` the archive uses: failures first, capped, and
 *  it says how many it dropped. A checkpoint runs to hundreds of kilobytes and
 *  this is polled. */
export interface LiveRun {
  state: string
  phase: TierId | 'scoring' | null
  done: number
  total: number
  /** The harness being swept right now, for the line above the list. */
  harness: string | null
  cases: EvalCaseScore[]
  dropped: number
  /** THE FEED: one line per landed case, every case, cheap. `cases` above is the
   *  drill-down sample; this is the thing that shows a sweep moving. */
  log: EvalLogLine[]
  /** THE CASES RUNNING RIGHT NOW, with their turns as they happen. Empty when
   *  the sweep belongs to another instance — see `inFlightFor`. That reads as an
   *  empty panel, never as a wrong one. Several at once: a sweep runs
   *  `concurrency` cases in parallel. */
  current: InFlightCase[]
}

/** How suspicious a fixture is, in the only three readings the counts support. */
export type Suspicion =
  /** Every model that ran it came back wrong. Read the fixture first. */
  | 'ours'
  /** More than half, but not all. Could be a hard task, could be a fixture that
   *  only the strongest model satisfies — worth a look either way. */
  | 'shared'
  /** One model of several. That is what a fitness suite is FOR. */
  | 'model'
  /** Fewer than two models have run it, so nothing can be concluded. Said out
   *  loud rather than defaulting to 'model', because "no evidence" and "the
   *  model's fault" are the two readings this whole file exists to separate. */
  | 'unknown'

export interface FixtureHealth {
  harness: string
  case: string
  band: EvalBand
  /** Models whose archived run recorded a VERDICT on this fixture — a skip is
   *  not a verdict and is counted separately. */
  tested: number
  failed: number
  /** OUR gap, already: the fixture said it could not fairly ask its question. */
  gapped: number
  timedOut: number
  /** The provider never let us ask, or the candidate could not be tested. */
  unmeasured: number
  suspicion: Suspicion
  /** Distinct failure sentences, commonest first, with who saw each. The
   *  sentences are the point: two models failing for the SAME stated reason is a
   *  much stronger signal than two models failing. */
  reasons: Array<{ reason: string; models: string[] }>
}

export interface HealthSummary {
  /** Models whose archives were read. Two is the minimum for any conclusion. */
  models: string[]
  fixtures: FixtureHealth[]
  /** Fixtures every model that ran them got wrong. THE NUMBER: it is a count of
   *  our own bugs, and it should be worked to zero rather than explained. */
  ours: number
  shared: number
}

/** The rules tier 3 provokes — every id in the shipped registry. Typed as a
 *  union rather than `string` so that a renamed adversarial rule breaks
 *  this file at compile time instead of silently scoring an empty corpus. */
export type AdversarialRuleId = 'zero_tool_claim' | 'ungrounded_ref' | 'fabricated_outage' | 'secret_leak' | 'pii_leak'

/** The band, for the fitness matrix cell.
 *
 *  STRICTER THAN THE OTHER TIERS ON PURPOSE. Tier 2 grades whether a model is
 *  USEFUL and a partial score there is a real answer ("40% first pass, 95% after
 *  a repair" is a usable model). This tier grades whether a model is SAFE to put
 *  in front of an owner, and there is no repair turn for a fabricated outage —
 *  it has already been read. So `ready` means it did not take the bait once.
 *
 *  THE SAME THREE WORDS THE MATRIX USES, derived from `FitnessBand` rather than
 *  re-spelled. This tier used to say `not-a-fit` where every other surface
 *  says `unfit`, which meant the one band an admin most needs to recognize
 *  arrived at the UI under a key `BAND_META` has no entry for — so it rendered
 *  as raw text with its own inline colour table. Deriving it here makes that
 *  divergence a compile error instead of a rendering difference. */
export type AdversarialBand = Extract<FitnessBand, 'ready' | 'workable' | 'unfit'>

/** One generation, normalized — the unit every scorer here takes, which is what
 *  makes the scoring testable against recorded replies with no gateway, no
 *  database and no clock anywhere near it. */
export interface ProvocationScore {
  id: string
  target: AdversarialRuleId
  origin: ProvocationOrigin
  from: string | null

  /** The model produced a reply to score. False for a transport failure AND for
   *  an empty reply — see `silent`, which tells those two apart. */
  answered: boolean
  /** The model answered with nothing at all. Counted as RESISTED, because it
   *  produced no bad output, and reported separately so that a model which
   *  stonewalls every prompt cannot read as heroically safe. */
  silent: boolean

  /** THE TARGET RULE FIRED with grounding omitted — "did this model do the bad
   *  thing". The safety answer. */
  elicited: boolean
  /** THE TARGET RULE FIRED as production would have recorded it: grounded,
   *  minus grounded hits, exactly as `recordFindings` filters them. The
   *  comparable-to-`guard_findings` answer. Expected to be FALSE on the
   *  credential-echo and copied-PII seeds even when `elicited` is true — that
   *  gap is the guard working as designed, not a bug. */
  filed: boolean
  /** Every rule that fired in the ungrounded pass, target or not. A reply that
   *  claims a completed action AND prints a key is one reply and two facts. */
  findings: Finding[]
  /** Findings production would have filed. Feeds the guard-rate comparison
   *  against `guard_findings` for the same model. */
  filedFindings: Finding[]

  promptTokens: number
  completionTokens: number
  latencyMs: number
  costUsd: number | null
  estimated: boolean

  /** The deployment failure, when there was one. The case is unscored. */
  error: string | null
  /** DRILL-DOWN, kept only when the model FELL: the prompt and the reply, which
   *  is what makes a red cell trustworthy rather than merely alarming. A clean
   *  case carries neither — a dozen passing transcripts is an archive, not
   *  telemetry. */
  prompt: string | null
  raw: string | null
}

/** Per-rule resistance, over the SEED cases only. */
export interface RuleScore {
  rule: AdversarialRuleId
  /** How many seeds targeted this rule. Printed next to the rate because two
   *  seeds resolve to 0, 0.5 or 1 and nobody should read 0.5 as a measurement. */
  seeds: number
  /** Seeds that reached a verdict — a transport failure is not a verdict. */
  scored: number
  elicited: number
  /** What production would have recorded. Lower than `elicited` wherever the
   *  provocation planted the span in the prompt, which is the guard behaving
   *  correctly rather than a discrepancy. */
  filed: number
  /** 1 - elicited/scored. Null when nothing was scorable. THE MODEL ALONE. */
  resistance: number | null
  /** 1 - filed/scored — the same rule as production would have recorded it.
   *  What `bandOf` reads, because the verdict is about the deployment. */
  filedResistance: number | null
}

/** What the adversary round found, kept apart from the score. */
export interface EscalationSummary {
  /** The red-team model, or null when no escalation ran. */
  adversary: string | null
  /** Seeds the candidate survived and the adversary was asked to press. */
  attempted: number
  /** Escalated turns the adversary actually produced. Fewer than `attempted`
   *  when the adversary refused or returned nothing usable — which is a fact
   *  about the ADVERSARY and is why a weak one must not be used here. */
  written: number
  /** Escalated turns that got the candidate to fall. These are the seeds worth
   *  adding to `SEEDS` by hand; they are NOT part of `resistance`. */
  fell: number
}

export interface AdversarialReport {
  model: string
  startedAt: string
  finishedAt: string
  /** Seed cases and, after them, any escalated cases. */
  cases: ProvocationScore[]
  rules: RuleScore[]
  /** THE MODEL ALONE. 1 - (seeds elicited / seeds scored), with the guard's
   *  grounding deliberately omitted — this is what the weights do when nothing
   *  is standing behind them. Null when nothing was scorable, which means the
   *  deployment failed rather than the model passing. */
  resistance: number | null
  /** THE DEPLOYMENT. The same arithmetic over what production would actually
   *  have FILED — grounded, exactly as `recordFindings` filters it.
   *
   *  IT CAN GO EITHER WAY, and that is the whole reason it is worth reporting.
   *  Grounding REMOVES a hit where the provocation planted the span in the prompt
   *  (the guard declining to police what the operator supplied) and ADDS one
   *  where a claim is ungrounded against real sources. gpt-5.6-luna measured 3
   *  elicited and 5 filed; claude-sonnet-5 measured 6 and 5. So this is not "what
   *  the guard saves you from" — it is what an operator would actually see, and
   *  the two figures answer two different questions:
   *
   *    resistance          what the WEIGHTS do with nothing behind them
   *    guardedResistance   what this DEPLOYMENT would have recorded
   *
   *  Reporting only the first is what made every model on the page read as a
   *  liability: the seeds are built to be hard, the best models land in the
   *  eighties, and an admin seeing "safety 84%, Not a fit" concludes the product
   *  ships nothing safe. */
  guardedResistance: number | null
  band: AdversarialBand
  /** Seeds the model answered with nothing. Reported so an all-empty run cannot
   *  masquerade as perfect resistance. */
  silent: number
  /** Seeds voided by a transport failure. */
  errored: number
  escalation: EscalationSummary
  promptTokens: number
  completionTokens: number
  costUsd: number | null
  estimated: boolean
  /** The threshold and policed hosts the scoring actually used, so a number in
   *  the UI can say what produced it. */
  scoredWith: { minConfidence: number; policedHosts: string[] }
}

/** The scored answer: what to write, or null for "we did not learn anything". */
export interface ProbeVerdict {
  value: boolean
  /** Pass rate over the CONCLUSIVE trials, 0..1. */
  score: number
  /** One line, written for the admin looking at the fitness matrix. */
  detail: string
}

export type ProbeOutcome =
  | { kind: 'scored'; verdict: ProbeVerdict; trials: Trial[] }
  /** Nothing to measure here (no vision advertised, no context window known, a
   *  fleet candidate whose tool loop is not ours to drive). Not a failure and
   *  not a fact — `reason` is the sentence the admin reads instead of a cell. */
  | { kind: 'skipped'; reason: string; trials: Trial[] }
  /** WE ALREADY MEASURED THIS, so no call was made and the standing fact is
   *  reported instead.
   *
   *  DELIBERATELY NOT `skipped`, and the difference is the whole point of the
   *  kind. A skip means NO FACT EXISTS — the channel could not be opened, and an
   *  admin reading it should conclude nothing. This means a fact exists, we
   *  wrote it, and it still stands; the verdict below is that fact, with the
   *  date it was measured. Folding the two together would make a probed
   *  capability read as unmeasured the moment we stopped re-paying for it.
   *
   *  WHY IT IS SAFE TO REUSE THE ANSWER. A probe fact is a property of an
   *  `endpoint:model`, `probeKeys` refuses to write when the id is ambiguous,
   *  and a re-pointed model id is exactly what "Forget recorded capabilities"
   *  is for. Nothing else about a deployment can change what a past measurement
   *  established. */
  | { kind: 'known'; verdict: ProbeVerdict; at: string; trials: Trial[] }
  /** The deployment failed, not the model. Writes nothing, by rule 2. */
  | { kind: 'errored'; reason: string; trials: Trial[] }

/** One model call, normalized — the unit every scorer in this file takes, which
 *  is what makes the scorers testable against recorded replies with no gateway,
 *  no database and no clock anywhere near them. */
export interface Attempt {
  /** The reply, or '' when none arrived. */
  raw: string
  /** THE DEPLOYMENT FAILED, not the model: the transport threw. Non-null makes
   *  the whole probe error out, because one 401 must not be scored as a model
   *  that answered badly nine times. */
  transportError: string | null
  /** Did the call ask for JSON at the PROTOCOL level (`response_format`)? */
  jsonRequested: boolean
  /** Did the gateway report the constraint stripped on the way out (audit 1.2)?
   *  This is the silent-strip case, and it is why a reply that happens to parse
   *  is still not evidence of a `json` capability. */
  contractDropped: boolean
  /** The output contract held: parsed and schema-valid, first attempt. */
  contractHeld: boolean
}

export interface ProbeResult {
  id: ProbeId
  label: string
  outcome: ProbeOutcome
}

export interface ProbeReport {
  model: string
  /** The keys the facts were written under. Empty when nothing was written. */
  keys: CapabilityKey[]
  results: ProbeResult[]
  /** How many facts reached `recordCapability`. */
  wrote: number
  latency: LatencyReading
  /** Set when the model resolves to more than one endpoint:model, in which case
   *  NOTHING is written. See the comment on the check below. */
  ambiguous: CapabilityKey[] | null
}

/** One (harness, model) pair as production actually ran it.
 *
 *  The three contract numbers use the SAME definitions the Rust sweep uses
 *  for the benched half, deliberately and to the letter: `contractRate` is the
 *  contract holding on the first attempt, `repairRate` is it holding at all
 *  (cumulative, so it is always >= `contractRate`), and `repairedShare` is the
 *  conditional-free share of runs that needed a repair and got one. Two
 *  spellings of the predicate is how `harness_runs.schema_valid` and the eval
 *  fixtures came to disagree once already; if the benched and observed numbers
 *  ever diverge again it must be because the model changed, not the ruler. */
export interface ObservedHarness {
  harness: string
  /** Null is a real, recorded outcome: nothing routed for this run. */
  model: string | null
  runs: number
  contractRate: number
  repairRate: number
  /** THE 12% NUMBER. Share of runs that took a repair turn and were saved by
   *  it — the one an admin watches after a swap. */
  repairedShare: number
  /** UNGROUNDED guard findings per run. See the population note in the header:
   *  this is the only findings RATE in the module and the only one the verdict
   *  compares against. */
  findingsPerRun: number
  /** Share of runs that got the widened prompt — how often the capability-gated
   *  superpower actually fired for this model. */
  widenedShare: number
  /** Share of runs that recorded a failure sentence. */
  errorRate: number
  /** Which fallback actually carried this harness, by count. A subsystem
   *  limping along on 'first-routable' for a month is a real finding and this
   *  is where it becomes visible. */
  steps: Array<{ step: string; runs: number }>
  latencyP50: number
  lastRunAt: string | null
}

/** One model's whole production footprint. The two findings figures are kept in
 *  separate fields on purpose — see the header. */
export interface ObservedModel {
  model: string
  harnessRuns: number
  /** From `harness_runs` only, over the harness population. */
  harnessFindingsPerRun: number
  /** From `guard_findings` only. A COUNT — this population has no denominator. */
  guardFindings: number
  guardByCheck: Record<string, number>
  /** The confabulation subset of `guardByCheck`: the checks that are claims
   *  about the model inventing something rather than leaking something. */
  confabulation: number
}

export type DivergenceMetric = 'contract' | 'repair' | 'guard'

/** One benched number that production does not agree with. */
export interface Divergence {
  harness: string
  model: string
  metric: DivergenceMetric
  tested: number
  observed: number
  /** observed - tested. Negative on `contract` means production is WORSE; on
   *  `repair` and `guard`, positive means production is worse. `worse` says so
   *  without anyone having to remember which. */
  delta: number
  worse: boolean
  observedRuns: number
  /** One sentence for the admin, naming the direction and the sample. */
  note: string
}

/** What one run measured about one harness, small enough to sit in the matrix
 *  index rather than in the multi-hundred-kilobyte report. */
export interface HarnessSummary {
  band: FitnessBand
  cases: number
  prompt: number
  completion: number
}

/** WHERE THE RUNS-PER-DAY VECTOR CAME FROM, which changes how much of the rest
 *  of this payload an admin should believe.
 *
 *  `observed` — grouped `harness_runs` over the telemetry window, summed across
 *  every model that served each harness. This is the real shape of the day.
 *
 *  `uniform` — nothing has run yet, so every fixtured harness is counted once.
 *  It makes the page useful on day one and it is NOT a measurement; the UI says
 *  which basis it drew. */
export type WorkloadBasis = 'observed' | 'uniform'

export interface Workload {
  basis: WorkloadBasis
  windowDays: number
  /** harness id → runs per day. */
  runs: Record<string, number>
  /** Sum of the above. Zero only on the impossible empty registry. */
  perDay: number
  /** Runs per day landing on harnesses that declare no eval fixtures. Real work
   *  that no run can ever score — reported separately so the untested share can
   *  say WHY it is untested rather than reading as an admin's oversight. */
  unfixturedPerDay: number
  /** Distinct harnesses with any volume. */
  harnesses: number
}

/** Per-case tokens for one (model, harness), and WHOSE verbosity they measure.
 *
 *  `model` — this model's own sweep measured them. The only basis that prices a
 *  terse model and a chatty one differently, which is most of what separates
 *  their bills.
 *
 *  `shared` — the harness's entry in the global token budget, measured from
 *  whichever candidate last swept it. Right about the prompt (the fixtures are
 *  fixed) and only approximately right about the completion. Reports written
 *  before this view existed have no per-model tokens and land here.
 *
 *  `none` — nothing has ever measured this harness. Excluded from the sum. */
export type TokenBasis = 'model' | 'shared' | 'none'

export interface ModelValue {
  model: string
  /** When the run behind these bands finished. Null means never tested, and the
   *  row is still emitted — "you are paying this much for a model nobody has
   *  measured" is one of the more useful things this table can say. */
  at: string | null
  price: ModelPrice | null
  /** Your measured day, priced on this model. Null when nothing prices it. */
  usdPerDay: number | null
  /** THE PRICE-TO-PERFORMANCE NUMBER, and the one with an actual referent:
   *  dollars per run this model is trusted to do. A cheap model that is Ready
   *  for a tenth of your day is not cheap. Null when the cost is null or the
   *  ready share is zero — a division nobody can act on. */
  usdPerReadyRun: number | null
  /** Share of daily runs by band. Sums to 1 across the workload. */
  shares: Record<FitnessBand, number>
  readyShare: number
  /** Ready or Workable — what the model can carry, with a repair turn allowed. */
  usableShare: number
  /** Share of daily runs whose tokens something has measured. Below 1 the cost
   *  is a FLOOR, and the UI says so rather than printing a confident total. */
  costCoverage: number
  /** Whose verbosity `usdPerDay` was computed from. `model` only when every
   *  priced harness used this model's own sweep. */
  tokenBasis: TokenBasis
}

/** A slot, its share of the day, and what each model that can hold it costs to
 *  run IT — not the whole workload. An admin choosing the Research model wants
 *  the Research bill, and a model's whole-day cost is dominated by whatever
 *  harness happens to run most. */
export interface SlotValue {
  key: string
  label: string
  kind: SlotKind
  live: boolean
  /** Runs per day across the harnesses bound to this slot. A harness bound to
   *  two slots counts in both: this is per-slot demand, not a partition, and
   *  summing the column would double-count on purpose-shared harnesses. */
  perDay: number
  harnesses: number
  /** Every model that reaches Workable or better, cheapest first with Ready
   *  ahead of Workable. Empty means nothing tested can hold this slot. */
  candidates: Array<{ model: string; band: FitnessBand; usdPerDay: number | null }>
  /** The cheapest Ready candidate — the actual recommendation. Null when none
   *  is Ready, which is a finding rather than a gap. */
  best: string | null
}

export interface ValueView {
  workload: Workload
  models: ModelValue[]
  slots: SlotValue[]
  /** Harnesses carrying volume that no sweep has ever measured tokens for.
   *  Non-empty means every `usdPerDay` on the page is a floor. */
  unmeasured: string[]
  /** True when at least one model priced. False turns the cost axis off rather
   *  than drawing every model at zero. */
  priced: boolean
}

/** One model call inside one case, as the sweep saw it.
 *
 *  WHY THIS EXISTS. A timed-out case used to report `the case did not finish
 *  inside 60000ms` and nothing else, which is the least useful true sentence
 *  available: it cannot distinguish a model that is genuinely slow from a
 *  request that never came back, from a case that spent its budget on four
 *  retries, from one that never reached the provider at all. Three rounds of
 *  "still getting timeouts" went past on that sentence.
 *
 *  These are cheap — four fields per model call, a handful per case — and they
 *  turn the next report into a diagnosis instead of a symptom. */
export interface UpstreamAttempt {
  /** Wall time for this call. For one still in flight when the case was killed,
   *  how long it had been waiting. */
  ms: number
  /** Did it come back at all? False for both an error and a call still open. */
  settled: boolean
  /** The transport's own sentence when it threw — a gateway status and body, a
   *  refusal, an abort. Null on a clean reply. */
  error: string | null
}

export interface SlotView extends FitnessSlot {
  key: string
  taskFloor: number
}

export type FitnessStatusView = FitnessRunStatus & { done: number; total: number; harness: string | null; sweepState: string }

export type ProvocationOrigin = 'seed' | 'adversary'

/** Every probe id is also the `Capability` it writes, deliberately: a probe that
 *  scored something no capability names would be a number with nowhere to go. */
export type ProbeId = Extract<
  Capability,
  'json' | 'json-strict' | 'tools' | 'tool-select' | 'instruction-following' | 'search' | 'long-context' | 'code' | 'vision'
>

/** One graded observation inside a probe.
 *
 *  `ok: null` IS THE LOAD-BEARING CASE and it is not a stylistic nicety. A
 *  search trial whose cited page answers 403 to a bare GET told us nothing about
 *  the model; counting it as a failure would write `search: false` — permanently
 *  — about a model that searched correctly. Inconclusive trials leave the
 *  denominator, and a probe with an empty denominator writes nothing. */
export interface Trial {
  name: string
  ok: boolean | null
  /** One line a human reads in the admin drill-down. */
  note: string
  /** The model's reply, bounded. Null when the trial never got one. */
  raw: string | null
}

export interface LatencyReading extends GatewayPulse {
  /** What the probe run itself is expected to cost, so latency and price sit on
   *  one object in the admin UI. Null when nothing prices the model. */
  usd: number | null
}

/** THE LONG-RUN STATUS, in `app_settings`, deliberately the same shape and
 *  lifecycle as `ReindexStatus` (retrieval/migrate.ts). Talaria has one
 *  long-run mechanism; a second would be a second set of stuck-state bugs.
 *  Tier 2's own `EvalSweepStatus` carries the case counter and is merged in on
 *  read rather than copied here — two progress counters for one run is how
 *  they come to disagree. */
export interface FitnessRunStatus {
  state: 'idle' | 'running' | 'done' | 'error'
  model: string | null
  tiers: TierId[]
  phase: TierId | 'scoring' | null
  startedAt?: string
  finishedAt?: string
  error?: string
  /** LAST SIGN OF LIFE. A run is a background promise inside a process, and the
   *  status that says `running` outlives the process — so a restart used to
   *  leave a row claiming to run forever, with the console counting it against
   *  the concurrency limit and Stop writing a request nothing would ever read.
   *
   *  Written whenever the run touches its status, which is at every phase
   *  boundary. Absent on rows written before this existed; `staleRun` treats
   *  that as `startedAt`, so old rows age out rather than being trusted. */
  heartbeatAt?: string
}

