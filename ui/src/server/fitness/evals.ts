// TIER 2 OF THE MODEL FITNESS SUITE — harness conformance.
//
// WHAT IT ANSWERS, and it is not "is this a good model": for each harness in
// THIS install, can the candidate model hold that harness's contract, and where
// exactly does it break? A new model release should be a fifteen-minute sweep
// and a swap, not a week of production surprises.
//
// THIS IS A DRIVER, NOT A SUBSYSTEM. Everything it needs already exists:
//   registry.ts   enumerates the harnesses and hands each definition back with
//                 its I and O still paired (`RegisteredHarness.use`)
//   define.ts     `EvalCase` — a fixture input plus a deterministic `check`
//   run.ts        `runHarness`, which already takes `ctx.model` (the pin) and
//                 `ctx.deps.transport` (the seam `run.test.ts` drives it
//                 through). Replaying a fixture against a candidate is those
//                 two fields and nothing else.
// So the whole of tier 2 is: for every fixture, pin the candidate, run it, and
// read the numbers off the row the runner already writes.
//
// THE FIVE NUMBERS, and why `repairRate` is the one that matters:
//   contractRate  the contract held on the FIRST attempt
//   repairRate    the contract held at all, after the repair turn (CUMULATIVE —
//                 see the field comment). A model at 40% first-pass and 95%
//                 after one repair is USABLE; one at 40/45 is not, and until
//                 this file existed nothing in Talaria could tell those two
//                 apart. That distinction is the entire argument for audit 1.4.
//   taskScore     the fixture's own deterministic `check`. No model judging a
//                 model — see `EvalCase` in define.ts for why, and judge.ts for
//                 the one harness that cannot be a string assertion and is
//                 scored by AGREEMENT with a labeled set instead.
//   guardRate     guard findings per run, from the pass the runner already does
//   latency/cost  what the sweep actually spent
//
// THE PREDICATE IS THE RUNNER'S OWN, READ OFF THE ROW IT WRITES.
//   `harness_runs.schema_valid` and the offline fixtures disagreed once, and the
//   production column was the optimistic liar: blurb-writer's schema
//   (`z.record(z.string(), z.string())`) cannot constrain the KEYS, so a reply
//   keyed by tidied-up display names wrote zero blurbs and recorded a perfect
//   contract, while the fixture had rejected that exact reply the whole time.
//   `output.verify` closed that gap by giving the input-relational half of the
//   contract somewhere to live.
//
//   A benchmark that re-derived "did the contract hold" would reopen it: two
//   spellings of the predicate is how the two halves came to disagree in the
//   first place. So this file NEVER decides that question. It captures the
//   `HarnessRunRow` the runner hands to `recordRun` — the literal row production
//   reads — and scores `row.schemaValid`, `row.repairs`, `row.findings` and
//   `row.latencyMs`. If the benched number and the observed number ever diverge
//   again, it is because the model changed, not because the ruler did.
//
//   The fixture's `check` is scored SEPARATELY, as `taskScore`, and the two are
//   allowed to disagree — that is what `optimistic` counts. See its comment for
//   which readings are expected and which one is a bug.
//
// WHAT THE SWEEP DELIBERATELY DOES NOT WRITE: `harness_runs` rows and
// `guard_findings` rows. Both tables are the OBSERVED half of the fitness page,
// shown next to the benched half, and a sweep that filed into them would move
// the number it is being compared against — seventy benched judge runs would
// swamp a week of real ones and make a model's live confabulation rate a
// property of how often an admin pressed Test. The guard pass still RUNS (that
// is where `guardRate` comes from); only the filing is suppressed. Token spend
// is real spend and still reaches `usage_events` through the transports, which
// is why the sweep's caller names itself.
import { getSetting, setSetting } from '../audit'
import { getGuardConfig } from '../guardrails'
import { listActivityHarnesses, type HarnessSource, type RegisteredHarness } from '../harness/registry'
import { runHarness, type HarnessDeps, type HarnessResult, type HarnessRunRow } from '../harness/run'
import { defaultTransport, offersToolDefinitions, runsOwnToolLoop, type Transport, type TransportRequest } from '../harness/transport'
import { makeSandbox } from './toolbox/sandbox'
import { makeWorkbench } from './toolbox/hermes-tools'
import { sandboxTransport, turnBudget, type DryRunResult } from './toolbox/dry-run'
import { toolSearchTransport, type SearchSource } from '../harness/defs/research'
import { supplierFor, PROVIDERS_KEY } from '../capability-reach'
import { platformSupply } from '../capability-platform'
import { listMcpServers } from '../mcp-registry'

/** WHAT A DRY RUN NEEDS OF ITS SANDBOX, and no more: the two surfaces
 *  (`toolbox/sandbox.ts` for Talaria's toolkit, `toolbox/hermes-tools.ts` for a
 *  file workspace) differ in everything except this. `world` is optional
 *  because only one of them has one, and a fixture that reaches for it knows
 *  which surface it is written against. */
interface DrySandbox {
  calls: EvalContext['calls']
  calledBefore: EvalContext['calledBefore']
  tools: Array<{ name: string; description: string; parameters: Record<string, unknown> }>
  dispatch: (call: { name: string; args: string }) => Promise<{ text: string; isError: boolean }>
  world?: unknown
}
import { estimateTokens } from '../usage'
import type { Capability } from '../harness/capability'
import type { ToolPolicy } from '../harness/transport'
import { NO_TOOLS, isGap, type EvalBand, type EvalCase, type EvalContext, type HarnessDefinition, type Message } from '../harness/define'

// ── The scoring surface ──────────────────────────────────────────────────────

/** One fixture, replayed once against the candidate. */
export interface EvalCaseScore {
  harness: string
  /** `EvalCase.name`. Unique within a harness; `caseKey` joins the two. */
  case: string
  /** `EvalCase.band`, defaulted. Carried on the CASE rather than looked up from
   *  the registry later, because a report read back from the archive has to keep
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
   *  THE NUMBER THIS REPLACES WAS A LIE, and an expensive one. Three harnesses
   *  declare `tools: 'own'` because the tool loop IS their feature
   *  (`work-session`, `outreach:check-in`, `briefer:chat`). Replayed against an
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
   *  rate-limited on every attempt (see `rateLimitedCase`). The reading is the
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
   *  second copy of the rule `run.ts` owns. */
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
   *  people who own the harness. See `CheckResult` in harness/define.ts. */
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
   *  harness must never strand the sweep — see `runEvalSweep`. */
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
   *    `output.verify` (define.ts), not a change here.
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
  /** The model's-own-tool-loop policy. Read by `harnessSkipReason`, which needs
   *  it before the harness runs. */
  tools: ToolPolicy
  requires: Capability[]
  /** Does this harness declare the input-relational half of its contract? See
   *  `EvalCaseScore.optimistic`. */
  verifies: boolean
  /** CAN a repair turn happen here at all? `run.ts` sets `maxRepairs` to 0 for
   *  every text harness — the one repair wording lives in json.ts and ends
   *  "send the corrected JSON value only", which is nonsense to a titler — and
   *  thirteen of the registry's harnesses are text.
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

export type EvalSweepState = 'idle' | 'running' | 'stopped' | 'done' | 'error'

/** THE RESUMABLE, BOUNDED RUN, in `app_settings` — the same shape and the same
 *  lifecycle as `ReindexStatus` in retrieval/migrate.ts, deliberately. Talaria
 *  has one long-run mechanism and this is it; a second one would be a second
 *  set of stuck-state bugs to learn about.
 *
 *  It carries the scored cases as well as the progress, and that is what makes
 *  the run RESUMABLE rather than merely observable: a sweep stopped by an admin
 *  (or interrupted by a deploy) can be restarted and will skip what it already
 *  paid for. Seventy cases of bounded text is a few KB. */
export interface EvalSweepStatus {
  state: EvalSweepState
  /** The candidate this sweep is about. Resuming only ever continues a sweep of
   *  the SAME model — scores from two models in one set would be a matrix cell
   *  that means nothing. */
  model: string | null
  startedAt: string | null
  finishedAt: string | null
  /** Cases finished / cases planned. */
  done: number
  total: number
  /** The harness currently running, for the progress line. */
  harness: string | null
  error: string | null
  cases: EvalCaseScore[]
}

/** The finished (or stopped) sweep, scored. */
export interface EvalSweep {
  model: string
  state: EvalSweepState
  startedAt: string | null
  finishedAt: string | null
  done: number
  total: number
  error: string | null
  harnesses: HarnessScore[]
  cases: EvalCaseScore[]
  /** Registered harnesses that declare NO fixtures. They are invisible to tier
   *  2 — not passing, not failing — and an admin reading a full green matrix
   *  deserves to know which columns were never tested. */
  unfixtured: string[]
  /** Was the guard pass actually on? With `mode: 'off'` every `guardRate` is
   *  zero, and zero-because-off must not read as zero-because-clean. */
  guarded: boolean
  /** HOW WIDE THE SWEEP RAN. Reported because it changes what `latencyP50`
   *  MEANS: at four wide the number includes queueing at the provider, so it is
   *  "what a call costs under this load" rather than "what a call costs". A page
   *  that showed the number without the width would be quietly comparing two
   *  different measurements across two runs. */
  concurrency: SweepConcurrency
  /** THE CASES THIS PASS ACTUALLY RAN, as opposed to the ones it inherited from
   *  an earlier one.
   *
   *  Speed is measured over these and never over `cases`. A supplemental pass
   *  that ran seven fixtures must not report a latency computed from two hundred
   *  and forty inherited ones measured last week at a different width — the
   *  number would not be about this pass, this model's current deployment, or
   *  anything an admin could act on. It also means a small deliberate pass is a
   *  legitimate way to refresh the speed reading without re-buying the battery. */
  measured: EvalCaseScore[]
}

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

// ── Injected edges ───────────────────────────────────────────────────────────

export interface EvalDeps {
  harnesses: () => Promise<RegisteredHarness[]>
  /** THE TOOL THIS INSTALL SUPPLIES FOR A CAPABILITY, or null. Injected so the
   *  sweep stays runnable with no database and no MCP registry anywhere near it
   *  — and so a test can drive the supplemented path without one. */
  supplier?: (capability: Capability) => Promise<{ server: string; tool: string } | null>
  /** HOW A SUPPLIED TOOL IS ACTUALLY CALLED. Defaults to the same dispatcher
   *  production uses (registry or platform, see `toolSearchTransport`).
   *
   *  Injected so a sweep test can decide what search RETURNS. Without this seam
   *  the only way to exercise the empty-search gap was to let a unit test reach
   *  a live SearXNG over the network and hope it found nothing — a test whose
   *  verdict depends on the internet is not a test. */
  searchTool?: (server: string, tool: string, args: Record<string, unknown>) => Promise<{ text: string; structured: unknown }>
  /** Passed through to `runHarness` as `ctx.deps`. The sweep adds its own
   *  transport wrapper, `recordRun` and `recordFindings` on top (see the file
   *  header for why the last two are suppressed); everything else — model
   *  resolution, capability facts, the guard config — stays REAL, because the
   *  capability floor refusing a weak model IS a tier-2 result and a fake that
   *  never refuses would be testing the fake. */
  harnessDeps: Partial<HarnessDeps>
  /** THIS CANDIDATE'S checkpoint. Per model since sweeps run concurrently — a
   *  single row would have three candidates' cases overwrite each other, and
   *  the resume that reads it back would restart the wrong one. */
  readStatus: (model: string) => Promise<EvalSweepStatus>
  writeStatus: (status: EvalSweepStatus) => Promise<void>
  /** The named candidates' checkpoints, for the panel that draws the running
   *  sweeps. Takes the list rather than discovering it — see the note on the
   *  real implementation. */
  readAllStatus: (models: readonly string[]) => Promise<Record<string, EvalSweepStatus>>
  /** Dollars for one call's tokens, or null when this install cannot say.
   *
   *  DEFAULTS TO NULL ON PURPOSE. Talaria prices spend in exactly one place —
   *  the `PRICED` view in usage.ts, over `usage_events`, with a documented
   *  coalesce order across the admin's per-model override, the auto-fetched
   *  public rate and the endpoint default. The sweep's turns land in that table
   *  through the real transports, so the fitness page can join the ledger and
   *  get the same dollars everything else in the product quotes. Re-deriving
   *  that formula here would be a second price that drifts from the first, and
   *  a cost estimate nobody can reconcile with the invoice is worse than none.
   *  The hook is here so a caller that HAS an oracle can supply it. */
  price: (model: string, promptTokens: number, completionTokens: number) => Promise<number | null>
  /** CAN this candidate run a harness that wants the model's OWN tool loop?
   *  Asked once per sweep, before any harness runs — see `harnessSkipReason`.
   *  Defaults to the transport's own answer, which is derived from
   *  `pickTransport` and therefore cannot disagree with the call that would
   *  otherwise refuse. */
  servesOwnTools: (model: string) => Promise<boolean>
  /** CAN this candidate be handed tool DEFINITIONS — the other half of the
   *  question above. A gateway model can; a fleet persona cannot (its loop runs
   *  inside the agent container and reports only names). Together they decide
   *  whether a tool-loop harness runs as production runs it, is dry-run against
   *  the sandbox, or is honestly skipped. */
  acceptsToolDefinitions: (model: string) => Promise<boolean>
  now: () => number
}

/** THE CHECKPOINT — ONE ROW PER CANDIDATE, and the row is the reason a wide
 *  sweep is affordable at all.
 *
 *  WHAT THE SHARED MAP COST. The checkpoint is written once per CASE, so a
 *  store holding every running candidate's cases in one row made each write a
 *  read-modify-write of every other run's work as well. Write traffic over a
 *  sweep went as O(N² × cases²) — about 400 MB at three candidates, ~4.6 GB at
 *  ten — and, worse, each write is a synchronous `JSON.parse`/`stringify` of a
 *  multi-megabyte blob ON THE EVENT LOOP THAT SERVES THE UI. The ceiling that
 *  produced was five or six concurrent runs, and the thing an org actually wants
 *  to do here is sweep a shortlist of a dozen models in one sitting.
 *
 *  Per row, a write is proportional to ONE candidate's own cases and nothing
 *  else. N drops out of the cost entirely: ten concurrent sweeps write exactly
 *  what ten sequential ones would, and no two of them touch the same row, so
 *  the serializing write queue this used to need is gone.
 *
 *  TWO OLDER SHAPES ARE STILL READ, never written. `harness_eval_runs` (the
 *  shared map) and `harness_eval_status` (the single row before that) each hold
 *  the resume point of whatever sweep was in flight when its shape changed —
 *  code already loaded in the process keeps writing the old one, and the loser
 *  of ignoring it is hours of paid-for cases. They age out on their own. */
const runKey = (model: string): string => `harness_eval_run:${model}`
const RUNS_KEY = 'harness_eval_runs'
const LEGACY_STATUS_KEY = 'harness_eval_status'

export const IDLE_STATUS: EvalSweepStatus = {
  state: 'idle',
  model: null,
  startedAt: null,
  finishedAt: null,
  done: 0,
  total: 0,
  harness: null,
  error: null,
  cases: [],
}

/** One candidate's checkpoint: its own row, then the two older shapes. The
 *  fallbacks are read-only and exist so a sweep that was in flight when the
 *  storage changed still resumes rather than re-buying its cases. */
async function readRun(model: string): Promise<EvalSweepStatus> {
  const own = await getSetting<EvalSweepStatus | null>(runKey(model), null)
  if (own) return own
  const shared = (await getSetting<Record<string, EvalSweepStatus>>(RUNS_KEY, {}))[model]
  if (shared) return shared
  const legacy = await getSetting<EvalSweepStatus | null>(LEGACY_STATUS_KEY, null)
  return legacy?.model === model ? legacy : IDLE_STATUS
}

const REAL_DEPS: EvalDeps = {
  harnesses: listActivityHarnesses,
  // The same resolver `capability-reach.ts` uses, so the sweep supplements
  // exactly what production would supply — never a second answer to "what does
  // this install have".
  supplier: async (capability) =>
    supplierFor(
      capability,
      await listMcpServers().catch(() => []),
      await getSetting(PROVIDERS_KEY, {}).catch(() => ({})),
      // TALARIA'S OWN TOOLS COUNT. Without this the sweep asked an empty MCP
      // registry — the default on every fresh install — got null, and ran
      // `research-search` with NO SEARCH TOOL while SearXNG answered queries on
      // the same box. The harness that exists to measure tool-driven research
      // was measuring a model answering from memory.
      await platformSupply().catch(() => []),
    ),
  harnessDeps: {},
  readStatus: readRun,
  // ONE ROW, ONE WRITER, NO READ FIRST. Nothing else writes this key, so there
  // is nothing to merge and nothing to serialize against — which is the whole
  // point of the per-candidate row.
  writeStatus: async (status) => {
    if (status.model) await setSetting(runKey(status.model), status)
  },
  // ASKED FOR BY NAME rather than enumerated: the caller (`fitnessRuns`) already
  // knows which candidates have runs, and a prefix scan of `app_settings` to
  // rediscover that would be a second, weaker source of the same list.
  readAllStatus: async (models) =>
    Object.fromEntries(await Promise.all(models.map(async (m) => [m, await readRun(m)] as const))),
  price: async () => null,
  // A THROW HERE MUST NOT SKIP EVERYTHING. If the fleet listing is unreachable
  // the honest fallback is "assume it can run", which spends one refusal per
  // fixture and records the refusal — the pre-port behaviour — rather than
  // silently marking three harnesses untestable on a transient outage.
  servesOwnTools: (model) => runsOwnToolLoop(model).catch(() => true),
  acceptsToolDefinitions: (model) => offersToolDefinitions(model).catch(() => false),
  now: () => Date.now(),
}

/** One candidate's live status, for a polling admin panel. */
export const evalSweepStatus = (model: string): Promise<EvalSweepStatus> => REAL_DEPS.readStatus(model)

/** Several candidates', for the panel that draws all the running sweeps. */
export const evalSweepStatuses = (models: readonly string[]): Promise<Record<string, EvalSweepStatus>> => REAL_DEPS.readAllStatus(models)

/** THROW AWAY THE RESUME LEDGER for one candidate.
 *
 *  A sweep's persisted status is BOTH its progress bar and the list of cases it
 *  already paid for, so clearing the archived report without clearing this
 *  leaves a model that looks untested and then resumes into a run that is
 *  already finished — a Start that returns instantly having bought nothing. The
 *  two always go together, which is why `clearFitnessResults` owns both. */
export const clearEvalStatus = (model: string): Promise<void> => REAL_DEPS.writeStatus({ ...IDLE_STATUS, model })

export interface EvalOptions {
  /** THE BOUND ON ONE CASE. A harness that hangs — a persona container that
   *  accepts the connection and never answers, an upstream that holds a stream
   *  open forever — must cost the sweep one case, not the whole run. The case
   *  is recorded `timedOut` and the sweep moves on.
   *
   *  It is a race rather than only an `AbortSignal` because the signal is a
   *  REQUEST: a transport that ignores it, or a `render` stuck in author code
   *  before any transport is reached, would leave the sweep waiting on a promise
   *  that never settles. The signal is fired too, so a transport that does honor
   *  it stops burning tokens. */
  caseTimeoutMs?: number
  /** ASKED TO STOP FROM ANYWHERE, not just from this process. The in-process
   *  set below only reaches a sweep whose closure lives in the instance the
   *  request hit — one HMR reload or one restart away from being a different
   *  one, which is why the Stop button did nothing. `surface.ts` supplies a
   *  reader over the persisted request; absent, only the local set applies. */
  shouldStop?: (model: string) => Promise<boolean>
  /** ARCHIVE EVERY CASE, PASSING INCLUDED. The settings-row report keeps a
   *  transcript only for cases that failed something — right for a drill-down,
   *  useless for verification, because "did our fixture accept something weak"
   *  can only be answered from a PASSING transcript. Injected rather than called
   *  directly so the sweep stays testable with no database anywhere near it. */
  archiveCase?: (model: string, runStartedAt: string, score: EvalCaseScore) => Promise<void>
  /** Called once when a run ends, whatever way it ended. */
  archivePrune?: (model: string) => Promise<void>
  /** Only these harness ids. Empty/omitted means every registered harness. */
  only?: string[]
  /** Ignore a resumable status and start clean. */
  restart?: boolean
  /** KEEP THE PASSES, RE-ASK EVERYTHING ELSE.
   *
   *  The middle setting between resume and restart, and the one an admin
   *  actually wants after a bad run: a sweep that timed out on five cases
   *  because the provider was busy does not need the other two hundred and
   *  forty-two bought again. Resume cannot do it — every case is already
   *  RECORDED, so there is nothing pending — and restart re-buys the lot.
   *
   *  See `worthRetrying` for what counts as "everything else"; the short version
   *  is anything that did not reach a clean verdict, including the cases this
   *  run had to mark unmeasured. */
  retryFailed?: boolean
  /** RUN WHAT HAS NEVER BEEN RUN, and nothing else.
   *
   *  The mode that matters once a suite is being actively developed: this month
   *  the registry gained fixtures on nine harnesses, and a model tested before
   *  them had no verdict on any of the new ones. Resume cannot help — the run is
   *  `done`, so nothing is pending — and restart re-buys two hundred and forty
   *  cases to ask seven questions.
   *
   *  It also PRUNES: a recorded case whose fixture no longer exists is a verdict
   *  about an assertion nobody can read, and leaving it in the ledger means the
   *  matrix is scored partly on questions the suite has stopped asking. */
  supplement?: boolean
  /** HOW MANY OF A HARNESS'S FIXTURES RUN AT ONCE. See `DEFAULT_CONCURRENCY`.
   *  Clamped to `MAX_CONCURRENCY`; 1 restores the old strictly-sequential sweep. */
  concurrency?: number
  /** The gaps between retries of a rate-limited case. Injected so a test can
   *  drive the retry path in milliseconds instead of half a minute — the
   *  production values are `PRESSURE_BACKOFF_MS`. */
  pressureBackoffMs?: readonly number[]
  deps?: Partial<EvalDeps>
}

/** HOW MANY CASES RUN AT ONCE, and what the old sequential rule was protecting.
 *
 *  THIS SWEEP WAS SEQUENTIAL ON PURPOSE, for two reasons that are both still
 *  true and neither of which required going one at a time:
 *
 *    LATENCY STOPS MEANING WHAT THE PAGE SAYS. `latencyP50` under N-way
 *    concurrency includes queueing at the provider, so it is no longer "what one
 *    call costs". The fix is not to refuse concurrency; it is to SAY SO —
 *    `EvalSweep.concurrency` is recorded and the fitness page labels the number
 *    with it, so nobody reads a 4-wide p50 as a single-request latency.
 *
 *    A SELF-HOSTED 14B BEHIND ONE GPU rate-limits, and those 429s would score as
 *    contract failures — a fact about the deployment recorded as a fact about
 *    the model. The fix is `narrowOnPressure` below: the sweep drops its width
 *    when it sees rate-limit pressure and says it did, rather than an admin
 *    having to know in advance what their hardware will take.
 *
 *  FOUR, because a 247-fixture sweep one-at-a-time is most of an hour and the
 *  first duplicate an admin runs is the one they stop watching. Four against a
 *  hosted gateway is unremarkable; against a single GPU the pressure valve finds
 *  1 within a case or two.
 *
 *  IT MULTIPLIES WITH `MAX_CONCURRENT_RUNS`: eight candidates at four wide is
 *  thirty-two calls in flight. That is the shape a shortlist comparison has, and
 *  it is why the valve is per sweep rather than global — each sweep discovers its
 *  own ceiling against the provider it is actually talking to. */
export const DEFAULT_CONCURRENCY = 4
export const MAX_CONCURRENCY = 8

/** A reply that means "you are asking too fast", not "the model is bad". Both
 *  halves matter: a 429 is unambiguous, and the 5xx family covers the overloaded
 *  gateways that answer 502/503 under the same pressure. */
const PRESSURE = /\b(429|too many requests|rate.?limit|502|503|504|overloaded|capacity)\b/i

/** HOW MANY TIMES A PRESSURED CASE IS RE-RUN before the sweep gives up on it.
 *
 *  A 429 IS NOT A RESULT ABOUT THE MODEL. It is the provider saying "slower",
 *  and scoring it as a contract failure is the same category error as scoring a
 *  401 as a model that cannot hold JSON — a fact about the deployment recorded
 *  as a fact about the weights, permanently, in a matrix an admin makes a
 *  purchasing decision from. The sweep narrows itself when it sees one; it
 *  should also RE-ASK the question it did not get an answer to.
 *
 *  Three attempts with a widening gap, because rate limits clear on a timescale
 *  of seconds and the sweep has already halved its own width by the second one.
 *  A case that is still pressured after that is not going to succeed by being
 *  asked a fourth time in the same minute. */
const PRESSURE_RETRIES = 3
const PRESSURE_BACKOFF_MS = [2_000, 8_000, 20_000]

/** A LOST REQUEST GETS ONE MORE CHANCE, NOT THREE, and the asymmetry is about
 *  cost rather than about principle. A rate limit comes back in milliseconds, so
 *  three retries are nearly free. A request that is never answered costs the
 *  WHOLE CASE BUDGET to discover — sixty seconds, or a hundred and twenty on a
 *  harness with a repair turn — so three retries would turn one lost request
 *  into four minutes, and five of them into twenty. One retry doubles the cost
 *  and gives a genuine second chance; a request that vanishes twice is telling
 *  you about the deployment, not about the model. */
const TIMEOUT_RETRIES = 1

/** HOW MANY CLEAN CASES IN A ROW REOPEN THE VALVE BY ONE LANE.
 *
 *  THE BUG THIS EXISTS TO FIX. The pressure valve was one-way. `narrow()` halved
 *  the width and nothing ever put it back, so a sweep that requested 4 and met a
 *  single lost request in its first minute — one vanished HTTP call, the failure
 *  mode `lostRequest` was written for precisely because it says NOTHING about
 *  the deployment's capacity — spent its remaining two hundred and forty cases
 *  strictly sequential. The archived reading said it plainly and nobody read it:
 *  `requested: 4, ended: 1`. An admin who picked 4 in the modal got 1, was never
 *  told, and waited four times as long for it.
 *
 *  A VALVE THAT ONLY CLOSES IS A RATCHET, and a ratchet is the wrong shape for a
 *  signal that is usually transient. Rate limits clear in seconds; a lost request
 *  is frequently a single bad connection. Both deserve an immediate retreat and
 *  neither is evidence about the next two hundred cases.
 *
 *  FIVE, AND WHY IT IS NOT ONE. Reopening after a single success would oscillate
 *  — widen, hit the same limit, halve, repeat — turning a quiet ceiling into a
 *  sawtooth that pays a 429 every few cases. Five clean cases at the current
 *  width is evidence the deployment is actually serving that width. And it steps
 *  up by ONE rather than doubling back: coming down fast and going up slow is
 *  what keeps a real ceiling found instead of rediscovered. */
const RECOVER_AFTER = 5

/** THE DEPLOYMENT CANNOT REACH THIS MODEL AT ALL — which is not a fact about the
 *  model, and must never be scored as one.
 *
 *  THE RUN THAT FOUND THIS. A sweep of qwen3.8-max recorded 247 failures, every
 *  one of them `gateway completion 404: No allowed providers are available for
 *  the selected model`, in 58 milliseconds each. The cause was the org's own
 *  no-train policy: `data_collection: 'deny'` pins `provider.only` to the US
 *  pool, and that model is served only by alibaba. A real and useful finding —
 *  and the matrix reported it as a model that fails every harness in Talaria.
 *
 *  These are the answers that mean "we never asked": no provider under this
 *  routing policy, no such model, or a credential the endpoint rejected. */
const UNREACHABLE =
  /no allowed providers|no endpoints found|not a valid model|model_not_found|does not exist or you do not have access|\b40[13]\b|invalid api key|unauthorized/i

const unreachable = (score: EvalCaseScore): boolean => score.error !== null && UNREACHABLE.test(score.error)

/** How many consecutive unreachable cases before the sweep stops asking.
 *
 *  A STRUCTURAL REFUSAL DOES NOT GET BETTER ON THE NEXT FIXTURE. Three in a row
 *  with nothing in between is a routing or credential fact about the whole run,
 *  and spending the remaining two hundred and forty cases rediscovering it costs
 *  an admin an hour and tells them nothing they did not know by case three. */
const UNREACHABLE_STREAK = 3

/** Was this case's failure the DEPLOYMENT rather than the model?
 *
 *  TWO SHAPES, and the second is the one the traces found. The first is an
 *  explicit 429 or an overloaded-gateway 5xx. The second is a request that WENT
 *  OUT AND NEVER CAME BACK: every timeout in the first traced sweep read
 *  `1 upstream call, still no reply after 60006ms` — one request, no response,
 *  ever. That is not a slow model against a tight budget, which would show a
 *  settled call just over the line; it is a lost request, and it says exactly as
 *  much about the model as a 429 does, which is nothing.
 *
 *  So it retries on the same path. A case that measured nothing must not be
 *  scored as if it had. */
const lostRequest = (score: EvalCaseScore): boolean => score.timedOut && (score.upstream ?? []).some((u) => !u.settled)
const rateLimited = (score: EvalCaseScore): boolean => score.error !== null && PRESSURE.test(score.error)
const pressured = (score: EvalCaseScore): boolean => rateLimited(score) || lostRequest(score)
/** How many more times to ask, given which shape of nothing came back. */
const retriesFor = (score: EvalCaseScore): number => (rateLimited(score) ? PRESSURE_RETRIES : TIMEOUT_RETRIES)

/** A wait a Stop can interrupt. A sweep that ignored the button for twenty
 *  seconds of backoff would be the Stop bug again in a smaller costume. */
const backoff = async (ms: number, stopped: () => boolean): Promise<void> => {
  const until = Date.now() + ms
  while (Date.now() < until) {
    if (stopped()) return
    await new Promise((r) => setTimeout(r, Math.min(250, until - Date.now())))
  }
}

/** THE SWEEP'S WIDTH, and the two things that move it. Passed as one object
 *  because the three are meaningless apart: a `narrow` with no `ceiling` to
 *  recover toward is the ratchet `RECOVER_AFTER` exists to undo. */
interface Valve {
  /** How many cases may be in flight RIGHT NOW. Re-read constantly. */
  width: () => number
  /** The most lanes this sweep will ever want — what the admin asked for. Lanes
   *  are spawned to this and park below it, so reopening costs nothing. */
  ceiling: number
  /** The provider pushed back. Halves the width. */
  narrow: (why: string) => void
  /** A case came back. Reopens a lane after `RECOVER_AFTER` in a row. */
  settled: () => void
}

/** How long a parked lane waits before re-checking the width. Short enough that
 *  a reopened lane is working again within a case, long enough that six parked
 *  lanes are not a spin loop. */
const PARK_MS = 250

/** Run `items` through `worker`, `valve.width()` at a time, stopping early when
 *  `stop` says so. Order of COMPLETION is not order of submission, which is
 *  fine: the resume ledger is a set of case keys and every rate is computed over
 *  the whole list.
 *
 *  WIDTH IS LIVE, AND IT DID NOT USED TO BE. This function read `width()` once —
 *  to size the lane array — and then ran those lanes to exhaustion. Two comments
 *  inside the loop claimed otherwise (that width was re-read per item, that a
 *  lane above it parked itself) and neither was true of the code under them. The
 *  visible consequence: narrowing did nothing until the NEXT harness, because
 *  the next harness built a new pool; and widening did nothing ever, because
 *  lanes were only ever spawned, never woken.
 *
 *  So lanes are spawned to the CEILING and park below the current width. A
 *  sweep that narrows sheds lanes inside the harness that hit the pressure, and
 *  one that recovers picks them back up without waiting for a pool rebuild. */
async function pool<T>(items: readonly T[], valve: Valve, stop: () => boolean, worker: (item: T) => Promise<void>): Promise<void> {
  let next = 0
  const lanes = Array.from({ length: Math.max(1, Math.min(valve.ceiling, items.length)) }, async (_slot, lane) => {
    for (;;) {
      if (stop()) return
      // Checked BEFORE parking, so the last lanes to finish do not sit waiting
      // on a width that will never rise again for a list that is already empty.
      if (next >= items.length) return
      if (lane >= valve.width()) {
        await new Promise((r) => setTimeout(r, PARK_MS))
        continue
      }
      const i = next++
      if (i >= items.length) return
      await worker(items[i]!)
    }
  })
  await Promise.all(lanes)
}

/** THE CLOCK ONE CASE RACES — sized per harness, not flat.
 *
 *  A FLAT 60s WAS A SINGLE-CALL BUDGET APPLIED TO MULTI-CALL CASES, and it read
 *  as the model's failure. A case is not one model call: `research-search` runs
 *  up to three tool rounds, a dry run up to its own `dryRun.maxTurns`, and a JSON harness can
 *  add a repair turn on top. The harnesses that timed out most were exactly the
 *  ones that call the most — research-search 22%, workbench:standard 15%,
 *  muse:draft 13% — and every one of those timeouts was then charged against the
 *  contract rate, capping research-search at 0.78 for ANY model however good.
 *
 *  So the budget is per-turn, multiplied by the turns the harness may actually
 *  take. Reasoning models are slow per call and this scales with them; the bound
 *  still exists so one hung transport cannot strand a sweep. */
const PER_TURN_TIMEOUT_MS = 60_000

/** Turns a harness may take in one case, for the clock above. Read off the same
 *  constants the code paths use, so a raised turn budget cannot silently leave
 *  the timeout behind. */
export function turnsPerCase(def: HarnessDefinition<unknown, unknown>, dryRun: boolean, supplied = false): number {
  // A dry run drives the loop itself; every other case is one model turn.
  //
  // EXCEPT A SUPPLEMENTED ONE, which is the case this missed. When the platform
  // supplies a capability the model lacks, the harness runs inside
  // `toolSearchTransport` — up to `MAX_TOOL_ROUNDS` search turns plus a closing
  // turn to answer — and this function handed that whole loop the budget for ONE
  // model turn. glm-5.2 filed three research-search cases as
  // `did not finish inside 60000ms after 1 upstream call(s)`, which reads as a
  // hung request and was really a four-turn job on a one-turn clock.
  const loop = dryRun ? turnBudget(def.dryRun?.maxTurns) : supplied ? SUPPLIED_TURNS : 1
  const repair = def.output.kind === 'json' ? Math.max(0, def.output.repair ?? 1) : 0
  return loop + repair
}

/** Turns the supplement transport may take: `MAX_TOOL_ROUNDS` searching plus one
 *  to write the answer. Stated here rather than imported so this file does not
 *  depend on a harness definition for its clock — and if that loop grows, this
 *  is the number to grow with it. */
const SUPPLIED_TURNS = 4

const DEFAULT_CASE_TIMEOUT_MS = PER_TURN_TIMEOUT_MS
/** Bounded for the same reason `HarnessResult.raw` is: a drill-down, not an
 *  archive, and a model that answers with 200KB of prose must not be able to
 *  turn one failed case into a settings row nothing can read. */
const DRILLDOWN_CAP = 4_000

export const caseKey = (harness: string, name: string): string => `${harness}::${name}`

// ── Skipping ─────────────────────────────────────────────────────────────────

/** WHY THIS CANDIDATE CANNOT BE TESTED ON THIS HARNESS AT ALL, or null when it
 *  can. Asked once per harness, before any fixture runs.
 *
 *  A SKIP IS NOT A KINDNESS AND NOT A PASS. The rule it encodes is narrow on
 *  purpose: the harness declares a REQUEST this candidate's transport is
 *  documented to refuse, so the call cannot happen and no reply can exist. That
 *  is a fact about the pairing, and it is knowable without spending anything.
 *
 *  EVERY OTHER FAILURE STILL RUNS. In particular a capability floor refusing a
 *  model (`research-search` on a model with no web search) is NOT skipped: the
 *  floor refusing IS the tier-2 result, the harness genuinely cannot be trusted
 *  on that model, and `score.ts` already turns the recorded fact into an `unfit`
 *  band naming the capability. Skipping it would replace a correct red cell with
 *  a shrug.
 *
 *  ONE RULE TODAY. Kept as a function returning a sentence rather than a boolean
 *  so the next one — a streaming-only harness against a transport that cannot
 *  stream, say — adds a branch and an explanation together. */
export function harnessSkipReason(
  harness: { label: string; tools: ToolPolicy },
  model: string,
  reach: { ownToolLoop: boolean; toolDefinitions: boolean },
): string | null {
  if (harness.tools !== 'own') return null
  // THE ORDER MATTERS. A fleet persona runs its own loop, so the harness runs as
  // production runs it. A gateway model has no loop of its own but CAN be handed
  // definitions — so the platform supplies the loop and the sandbox
  // (`toolbox/dry-run.ts`), which measures the thing that actually matters:
  // not "can it emit a tool call" but "given these tools and this situation,
  // what did it do". Only a model that can do neither is untestable here.
  if (reach.ownToolLoop || reach.toolDefinitions) return null
  return (
    `${harness.label} runs a tool loop, and "${model}" can neither run its own nor be handed tool definitions. ` +
    'The sweep did not call the model: nothing here is a measurement of it.'
  )
}

/** The record of a fixture that was never sent. Every measured field is a zero
 *  that `scoreHarness` excludes rather than averages — see `EvalCaseScore.skipped`. */
const skippedCase = (harness: string, name: string, band: EvalBand, reason: string): EvalCaseScore => ({
  harness,
  case: name,
  band,
  skipped: reason,
  contractHeld: false,
  firstPass: false,
  repairs: 0,
  answered: false,
  task: 'unscored',
  taskError: null,
  gap: null,
  findings: 0,
  latencyMs: 0,
  startedAt: new Date(0).toISOString(),
  wallMs: 0,
  promptTokens: 0,
  completionTokens: 0,
  costUsd: null,
  estimated: false,
  timedOut: false,
  optimistic: false,
  error: null,
  prompt: null,
  raw: null,
  turns: null,
  calls: null,
  upstream: null,
})

// ── Scoring (pure over recorded cases) ───────────────────────────────────────

/** Nearest-rank percentile. Exact on the small samples a sweep produces — a
 *  harness declares two to five fixtures, and an interpolating percentile over
 *  three numbers invents a latency nothing measured. */
function percentile(sorted: number[], q: number): number {
  if (sorted.length === 0) return 0
  const rank = Math.min(sorted.length - 1, Math.max(0, Math.ceil(q * sorted.length) - 1))
  return sorted[rank] ?? 0
}

const rate = (n: number, of: number): number => (of === 0 ? 0 : n / of)

/** A band's pass rate over the cases that were scorable in it, or null when it
 *  had none. NULL, never zero: a band with no fixtures has not been failed. */
function bandScore(taskable: EvalCaseScore[], band: EvalBand): number | null {
  const mine = taskable.filter((c) => c.band === band)
  return mine.length === 0 ? null : rate(mine.filter((c) => c.task === 'pass').length, mine.length)
}

/** Score one harness's cases. Pure, and it takes the METADATA rather than the
 *  registry so that a test can score recorded cases without a registry at all. */
export function scoreHarness(meta: HarnessMeta, all: EvalCaseScore[]): HarnessScore {
  // THE PARTITION IS THE WHOLE FUNCTION. A skipped case is an absence, not a
  // zero, and every line below counts over `cases` — the ones that ran. Mixing
  // the two would divide by a denominator that includes fixtures nothing asked.
  const skips = all.filter((c) => c.skipped !== null)
  const cases = all.filter((c) => c.skipped === null)
  // A TIMEOUT IS NOT A CONTRACT FAILURE, and counting it as one was the same
  // mistake `skipped` was introduced to fix, wearing different clothes. The
  // model never finished answering, so nothing about its contract was observed
  // — the clock ran out, which is a fact about our budget, the provider's
  // latency and our own tool loop. It used to sit in the denominator with
  // `contractHeld: false`, which capped `research-search` at 0.78 for any model
  // however good, purely because a fifth of its cases ran out of room.
  //
  // `scored` — cases that reached a verdict — already existed and was used only
  // for the latency percentile. Every rate now divides by it.
  const scored = cases.filter((c) => !c.timedOut)
  const total = scored.length
  const first = scored.filter((c) => c.firstPass).length
  const held = scored.filter((c) => c.contractHeld).length
  const failedFirst = scored.filter((c) => !c.firstPass).length
  const recovered = scored.filter((c) => c.contractHeld && !c.firstPass).length
  // A GAP IS NOT TASKABLE. `task` is already 'unscored' for one, so this falls
  // out — the count and the reasons are carried separately so they reach the
  // people who can fix them instead of vanishing into an absence.
  const gapped = scored.filter((c) => c.gap !== null)
  const taskable = scored.filter((c) => c.task !== 'unscored')
  const priced = cases.filter((c) => c.costUsd !== null)
  const latencies = [...scored.map((c) => c.latencyMs)].sort((a, b) => a - b)
  return {
    ...meta,
    // `cases` stays every case that RAN, timeouts included, so the count an
    // admin reads still matches the fixtures spent. `scored` is the denominator
    // of the rates, and the gap between them is the timeout count.
    cases: cases.length,
    skipped: skips.length,
    skipReason: skips[0]?.skipped ?? null,
    gaps: gapped.length,
    gapReasons: [...new Set(gapped.map((c) => c.gap ?? ''))].filter(Boolean),
    scored: scored.length,
    contractRate: rate(first, total),
    repairRate: rate(held, total),
    repairYield: failedFirst === 0 || !meta.repairable ? null : rate(recovered, failedFirst),
    taskScore: taskable.length === 0 ? null : rate(taskable.filter((c) => c.task === 'pass').length, taskable.length),
    bandScores: {
      easy: bandScore(taskable, 'easy'),
      standard: bandScore(taskable, 'standard'),
      hard: bandScore(taskable, 'hard'),
    },
    // OVER `scored` LIKE THE REST. A timed-out case produced no reply, so the
    // guard pass never ran on it and it can contribute no findings — leaving it
    // in the denominator would dilute the rate by the share of cases that ran
    // out of clock. Same for `answeredRate`: "did the model answer" is not a
    // question about a case that never got to.
    guardRate: rate(
      scored.reduce((n, c) => n + c.findings, 0),
      total,
    ),
    answeredRate: rate(scored.filter((c) => c.answered).length, total),
    latencyP50: percentile(latencies, 0.5),
    latencyP95: percentile(latencies, 0.95),
    promptTokens: cases.reduce((n, c) => n + c.promptTokens, 0),
    completionTokens: cases.reduce((n, c) => n + c.completionTokens, 0),
    costUsd: priced.length === 0 ? null : priced.reduce((n, c) => n + (c.costUsd ?? 0), 0),
    estimated: cases.some((c) => c.estimated),
    timeouts: cases.filter((c) => c.timedOut).length,
    optimistic: cases.filter((c) => c.optimistic).length,
  }
}

/** Group recorded cases by harness and score each. Order follows `metas`, which
 *  is the registry's order — the order Admin shows the harnesses in. */
export function scoreHarnesses(metas: HarnessMeta[], cases: EvalCaseScore[]): HarnessScore[] {
  const out: HarnessScore[] = []
  for (const meta of metas) {
    const mine = cases.filter((c) => c.harness === meta.id)
    if (mine.length === 0) continue
    out.push(scoreHarness(meta, mine))
  }
  return out
}

export const metaOf = (h: RegisteredHarness): HarnessMeta => ({
  id: h.id,
  label: h.label,
  source: h.source,
  outputKind: h.outputKind,
  tools: h.tools,
  requires: h.requires,
  verifies: h.use((def) => def.output.verify !== undefined),
  // Mirrors `maxRepairs` in run.ts, which is the only thing that decides
  // whether a repair turn happens: JSON output, and a `repair` count the
  // harness did not zero out.
  repairable: h.use((def) => def.output.kind === 'json' && (def.output.repair ?? 1) > 0),
})

// ── Stopping ─────────────────────────────────────────────────────────────────

/** In-process, exactly like `reindexRunning` in retrieval/migrate.ts: one node
 *  runs the sweep and the Stop button reaches the same node. It is a REQUEST,
 *  honored between cases and by aborting the case in flight, so a stop always
 *  lands on a case boundary and the persisted status is never half a case. */
/** The candidates sweeping in this process, and the ones asked to stop.
 *
 *  SETS RATHER THAN BOOLEANS because sweeps now run concurrently. Everything a
 *  sweep touches was already per-candidate — its checkpoint, its cases, its
 *  scores — so the only thing that had to stop being global was the flag. What
 *  stays global is the CAP, which lives in surface.ts with the other run
 *  admission rules; a second sweep of the SAME model is still refused here,
 *  because two sweeps interleaving one candidate's cases is the thing the
 *  original boolean was actually protecting against. */
const sweeping = new Set<string>()
const stopRequested = new Set<string>()

/** THE CASE THAT IS RUNNING RIGHT NOW, per candidate.
 *
 *  WHY IT IS IN MEMORY AND NOT IN THE STATUS ROW. The persisted status is
 *  written after every CASE — it is the progress bar and the resume ledger, and
 *  that cadence is already the right one for both. A turn-by-turn view needs an
 *  update per MODEL TURN, which on a 250-fixture sweep with a six-turn tool loop
 *  is nearly two thousand writes to one `app_settings` row. So the in-flight
 *  view is process-local and free.
 *
 *  WHAT THAT COSTS, said plainly: an instance that did not start the run shows
 *  nothing here. That degrades to an empty panel, never to a wrong one, and the
 *  completed-case feed beside it is persisted and unaffected. */
const inFlight = new Map<string, Map<string, InFlightCase>>()

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

/** What is this candidate doing right now? EMPTY when nothing is, or when the
 *  sweep belongs to another instance.
 *
 *  A LIST, because a sweep runs several cases at once (see `DEFAULT_CONCURRENCY`)
 *  and a panel showing one of four would make three quarters of a working sweep
 *  invisible. Oldest first, so the one most likely to be stuck reads first. */
export const inFlightFor = (model: string): InFlightCase[] =>
  [...(inFlight.get(model)?.values() ?? [])].sort((a, b) => a.startedAt - b.startedAt)

/** Ask a running sweep to stop. Returns whether one was running to ask.
 *  Without a model, asks every sweep in flight to stop. */
export function stopEvalSweep(model?: string): boolean {
  if (model === undefined) {
    if (sweeping.size === 0) return false
    for (const m of sweeping) stopRequested.add(m)
    return true
  }
  if (!sweeping.has(model)) return false
  stopRequested.add(model)
  return true
}

/** Is a sweep running IN THIS PROCESS? A persisted `state: 'running'` with this
 *  false is a sweep a restart interrupted, which is resumable rather than
 *  stuck — see `runEvalSweep`. */
export const evalSweepRunning = (model?: string): boolean => (model === undefined ? sweeping.size > 0 : sweeping.has(model))

// ── The driver ───────────────────────────────────────────────────────────────

interface CaseRun {
  row: HarnessRunRow | null
  prompt: string
  promptTokens: number
  completionTokens: number
  estimated: boolean
  threw: string | null
  timedOut: boolean
  /** EVERY UPSTREAM CALL THIS CASE MADE, in order. See `UpstreamAttempt`. */
  upstream: UpstreamAttempt[]
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

/** A call still open when the case was killed has no duration of its own yet.
 *  Give it the time it had actually been waiting, so the sentence below can say
 *  "still had no reply after 59.4s" rather than "0ms". */
const settleOpen = (calls: readonly UpstreamAttempt[], startedAt: number): UpstreamAttempt[] =>
  calls.map((c) => (c.settled ? c : { ...c, ms: Date.now() - startedAt }))

/** THE SENTENCE A TIMEOUT SHOULD HAVE BEEN. Names what the budget was actually
 *  spent on, which is the whole question an admin has when a model they know to
 *  be fast times out. */
function timeoutDetail(caseMs: number, calls: readonly UpstreamAttempt[]): string {
  if (calls.length === 0) {
    // The case never got a request out. That is not the model: it is route
    // resolution, the capability floor, key resolution or the provider catalog
    // — all of which happen before a token is spent and all of which can block.
    return `the case did not finish inside ${caseMs}ms and never made an upstream call at all — the time went somewhere before the request (route resolution, the endpoint key, or the provider catalog), not to the model`
  }
  const open = calls.filter((c) => !c.settled)
  const done = calls.filter((c) => c.settled)
  const parts = [`the case did not finish inside ${caseMs}ms after ${calls.length} upstream call(s)`]
  if (done.length) parts.push(`${done.length} came back (${done.map((c) => `${c.ms}ms${c.error ? ' error' : ''}`).join(', ')})`)
  if (open.length) parts.push(`${open.length} still had no reply after ${Math.max(...open.map((c) => c.ms))}ms`)
  const errored = calls.filter((c) => c.error)
  if (errored.length) parts.push(`last error: ${errored.at(-1)!.error}`)
  return parts.join('; ')
}

/** The transport the sweep wraps around the real one: it changes nothing about
 *  the call and records the prompt and the token counts, which no other seam
 *  can see. `HarnessResult` carries neither by design — the drill-down needs
 *  the prompt and the cost line needs the tokens, and widening the runner's
 *  result type for a benchmark would put benchmark concerns in the hot path. */
function recordingTransport(base: Transport, into: CaseRun, live: InFlightCase | null): Transport {
  return async (req: TransportRequest) => {
    into.prompt = req.messages.map((m) => `${m.role}: ${m.content}`).join('\n\n')
    if (live) {
      live.turn++
      // THE REQUEST IS THE CONVERSATION. A tool loop hands the whole transcript
      // back down on every turn, so publishing `req.messages` gives a live view
      // that grows a turn at a time without the loop having to report anything.
      live.turns = recordTurns(req.messages) ?? req.messages.map((m) => ({ role: m.role, content: m.content.slice(0, LIVE_TURN_CAP) }))
      live.calls++
      live.open++
    }
    // PUSHED BEFORE THE AWAIT, so a call that never comes back is still in the
    // list when the case is killed. Recording on completion would make the one
    // attempt that matters — the one that hung — the only one invisible.
    const attempt: UpstreamAttempt = { ms: 0, settled: false, error: null }
    const at = Date.now()
    into.upstream.push(attempt)
    let reply: Awaited<ReturnType<Transport>>
    try {
      reply = await base(req)
    } catch (err) {
      attempt.ms = Date.now() - at
      attempt.settled = true
      attempt.error = (err instanceof Error ? err.message : String(err)).slice(0, 300)
      if (live) live.open--
      throw err
    }
    attempt.ms = Date.now() - at
    attempt.settled = true
    if (live) {
      live.open--
      // The reply, appended, so the panel shows what came back rather than only
      // what went out.
      if (reply.text.trim()) live.turns = [...live.turns, { role: 'assistant', content: reply.text.slice(0, LIVE_TURN_CAP) }]
    }
    if (reply.usage) {
      into.promptTokens += reply.usage.promptTokens
      into.completionTokens += reply.usage.completionTokens
    } else {
      // The same chars/4 fallback the token ledger uses, from the same helper —
      // a second estimator would give the fitness page and the invoice two
      // different token counts for one call.
      into.promptTokens += estimateTokens(into.prompt.length)
      into.completionTokens += estimateTokens(reply.text.length)
      into.estimated = true
    }
    return reply
  }
}

/** Race a promise against a wall clock. The loser is not cancelled — it cannot
 *  be, which is the whole point (see `EvalOptions.caseTimeoutMs`) — so the run
 *  promise is given a `.catch` BEFORE the race: a transport that rejects five
 *  minutes after the sweep moved on must not surface as an unhandled rejection
 *  that takes the process with it. */
async function bounded<T>(work: Promise<T>, ms: number, onTimeout: () => void): Promise<{ done: true; value: T } | { done: false }> {
  let timer: ReturnType<typeof setTimeout> | null = null
  const settled = work.then((value) => ({ done: true, value }) as const)
  const expiry = new Promise<{ done: false }>((resolve) => {
    timer = setTimeout(() => {
      onTimeout()
      resolve({ done: false })
    }, ms)
  })
  try {
    return await Promise.race([settled, expiry])
  } finally {
    if (timer) clearTimeout(timer)
  }
}

const cap = (text: string | null): string | null => (text ? text.slice(0, DRILLDOWN_CAP) : null)

/** One turn in the LIVE view, bounded harder than the archived one: it is
 *  re-sent on every poll while a case runs, so a work-session prompt at full
 *  length would be shipped a dozen times per case for no reading anyone does. */
const LIVE_TURN_CAP = 600

/** A tool result the model saw, bounded harder than the prose is. The loop
 *  itself truncates at 8 000 characters before handing one back; this is the
 *  archive's share of that, and a `get_ticket` payload is the reason it exists. */
const TOOL_CAP = 1_200
/** A tool RESULT, kept shorter still — see `recordCalls`. */
const RESULT_CAP = 600

/** THE TRANSCRIPT, FLATTENED FOR THE ARCHIVE.
 *
 *  A single-turn case returns null: `prompt` and `raw` already carry the whole
 *  exchange, and writing it twice per case would double the size of a settings
 *  row for no reading anyone would do. */
function recordTurns(messages: readonly Message[] | undefined): EvalTurn[] | null {
  if (!messages || messages.length <= 2) return null
  return messages.map((m) => ({
    role: m.role,
    content: (m.content ?? '').slice(0, TOOL_CAP),
    ...(m.toolCalls?.length ? { toolCalls: m.toolCalls.map((c) => c.name) } : {}),
  }))
}

/** WHAT THE MODEL DID, kept for every dry-run case — but not at full weight.
 *
 *  `withResults` is false for a case that passed cleanly, and that is a size
 *  decision with a reading behind it. The NAMES and the ARGUMENTS are what every
 *  behavioural fixture asserts over and what an admin compares between two
 *  models, and they are a few hundred bytes; the RESULTS are a `get_ticket`
 *  payload each and are the whole weight. On a case that failed, the result is
 *  often the explanation (the tool refused, and the model carried on anyway), so
 *  it stays. On a case that passed there is nothing to explain.
 *
 *  Without this split, twenty-four archived reports carrying every tool result
 *  of every passing dry run would put megabytes into a settings row — the same
 *  mistake `drilldown` exists to prevent for transcripts. */
function recordCalls(sandbox: DrySandbox | null, withResults: boolean): EvalToolCall[] | null {
  if (!sandbox) return null
  return sandbox.calls.map((c) => ({
    tool: c.tool,
    args: JSON.stringify(c.args).slice(0, TOOL_CAP),
    result: !withResults || c.result === null || c.result === undefined ? null : JSON.stringify(c.result).slice(0, RESULT_CAP),
    error: c.error,
  }))
}

/** How often a case in flight asks whether it has been stopped.
 *
 *  STOP USED TO BE HONORED ONLY BETWEEN CASES, and that is why the button read
 *  as broken. A dry run is budgeted `PER_TURN_TIMEOUT_MS × turnsPerCase` — seven
 *  minutes for a work-session fixture — so an admin who pressed Stop watched
 *  nothing happen for minutes and pressed it again. The request had landed; the
 *  sweep was politely finishing a case nobody wanted.
 *
 *  Half a second is nothing against a model call and is instant to a person. */
const STOP_POLL_MS = 500

/** How often the sweep re-reads the PERSISTED stop request. One second, so a
 *  Stop pressed against another worker reaches the running case about as fast as
 *  one pressed against this one. */
const STOP_WATCH_MS = 1_000

async function runOneCase<I, O>(
  def: HarnessDefinition<I, O>,
  fixture: EvalCase<I, O>,
  model: string,
  deps: EvalDeps,
  timeoutMs: number,
  dryRun: boolean,
  /** Asked twice a second while the case runs. When it turns true the case is
   *  ABORTED and DISCARDED — see the null return. */
  stopped: () => boolean = () => false,
): Promise<EvalCaseScore | null> {
  const startedAt = Date.now()
  const capture: CaseRun = { row: null, prompt: '', promptTokens: 0, completionTokens: 0, estimated: false, threw: null, timedOut: false, upstream: [] }
  // PUBLISHED FOR THE LIVE PANEL, and cleared in every exit path below — a stale
  // "running now" left behind by a stopped case is worse than none, because it
  // makes a finished sweep look like a wedged one.
  const live: InFlightCase = {
    harness: def.id,
    case: fixture.name,
    band: fixture.band ?? 'standard',
    startedAt: Date.now(),
    turn: 0,
    maxTurns: turnsPerCase(def as HarnessDefinition<unknown, unknown>, dryRun),
    calls: 0,
    open: 0,
    turns: [],
  }
  const slot = inFlight.get(model) ?? new Map<string, InFlightCase>()
  inFlight.set(model, slot)
  slot.set(caseKey(def.id, fixture.name), live)
  const controller = new AbortController()
  // CANCELLED IS NOT FAILED, and conflating them would be the worse bug of the
  // two. A case aborted by Stop is recorded NOWHERE: the persisted status is the
  // resume ledger, so writing a cancelled case into it would mark the fixture
  // done, skip it on resume, and leave the model carrying a phantom failure it
  // was never actually given a chance at.
  let cancelled = false
  // RESOLVED THE MOMENT STOP IS SEEN, and raced against the case below.
  //
  // Aborting the controller is necessary and not sufficient: it releases the
  // socket, but the case only finishes once the transport notices and rejects,
  // and a transport that ignores its signal (a hung provider, a fake one in a
  // test) would leave Stop waiting out the full case budget again. Racing the
  // cancellation makes the sweep's response to Stop independent of how well the
  // thing underneath it behaves — which is the property a Stop button needs.
  let sawStop: () => void = () => {}
  const stopSeen = new Promise<void>((resolve) => {
    sawStop = resolve
  })
  const poll = setInterval(() => {
    if (!stopped()) return
    cancelled = true
    controller.abort()
    sawStop()
  }, STOP_POLL_MS)
  // Never hold the process open for a poller.
  ;(poll as unknown as { unref?: () => void }).unref?.()

  // AN ISOLATED TALARIA, ONE PER CASE. Built only for a harness whose feature is
  // the tool loop; every other harness gets `null` and its fixtures see
  // `NO_TOOLS`. Per case rather than per harness because two fixtures sharing a
  // mutable board would make the second one's assertions depend on the first
  // one's model.
  // WHICH SURFACE. A harness declaring `workspace` is a CODING harness and gets
  // files and a test runner (`toolbox/hermes-tools.ts`); everything else gets
  // Talaria's own toolkit. They are different jobs with different tools, and a
  // harness has exactly one of them.
  // ── SUPPLEMENT WHAT THE MODEL LACKS AND THE DEPLOYMENT HAS ────────────────
  //
  // THE HALF THAT WAS MISSING. `RoleFloor.suppliable` already lets the run
  // PROCEED when a capability is reachable through a registered tool — that is
  // how `research-search` avoids refusing a model that cannot browse. But the
  // sweep then handed that model the ordinary transport, with no tool on it, so
  // it answered from memory and the fixture failed it for having no sources.
  // Production does not do that: `research.ts` picks `toolSearchTransport` for
  // exactly this case.
  //
  // So the benchmark measures what an admin actually assigns — a model running
  // inside Talaria with the tools this org registered — rather than the bare
  // weights. A model with no native search and a search tool available is a
  // model that can do research here, and the matrix should say so.
  /** WHAT THE SUPPLIED SEARCH TOOL ACTUALLY RETURNED, across every call this
   *  case made. Empty after a run that called the tool means our search found
   *  nothing — our gap, not the model's failure. */
  const sources: SearchSource[] = []
  const suppliable = def.floor.suppliable ?? []
  const supplier = suppliable.length > 0 && deps.supplier ? await deps.supplier(suppliable[0]!).catch(() => null) : null

  const workspace = dryRun ? def.dryRun?.workspace?.(fixture.input) : undefined
  // The two sandboxes share exactly the surface `EvalContext` needs — a call
  // log, an ordering question, and (only Talaria's) a world. Narrowed to that
  // rather than to a union, so a third surface adds a branch above and nothing
  // else here.
  const sandbox: DrySandbox | null = !dryRun
    ? null
    : workspace
      ? makeWorkbench(workspace)
      : makeSandbox({ tools: def.dryRun?.tools, ...(def.dryRun?.world ? { world: def.dryRun.world } : {}) })
  const dry: { result?: DryRunResult } = {}
  const base = deps.harnessDeps.transport ?? defaultTransport

  const harnessDeps: Partial<HarnessDeps> = {
    ...deps.harnessDeps,
    transport: recordingTransport(
      sandbox
        ? sandboxTransport(sandbox, base, dry, turnBudget(def.dryRun?.maxTurns))
        : supplier
          ? // The same transport production uses, driving the same tool. THE SINK
            // IS KEPT, not thrown away: it is the only evidence of whether the
            // deployment's search actually FOUND anything, and that is the
            // difference between a model that answered badly and a search
            // backend that returned nothing for it to answer from. See the gap
            // rule below.
            toolSearchTransport(`fitness:${def.id}`, sources, supplier, { base, ...(deps.searchTool ? { callTool: deps.searchTool } : {}) })
          : base,
      capture,
      live,
    ),
    // The row the runner writes IS the predicate this file scores. Captured
    // here and NOT forwarded to the real `recordRun`: see the file header on why
    // a sweep must not file into the table it is being compared against.
    recordRun: async (row) => {
      capture.row = row
    },
    // Same argument, and stronger — `guard_findings.model` is the live
    // per-model confabulation rate, and seventy benched runs would rewrite it.
    // The guard pass still runs; only the filing is suppressed.
    recordFindings: async () => {},
  }

  const work = runHarness(def, fixture.input, {
    caller: `fitness:${def.id}`,
    model,
    signal: controller.signal,
    deps: harnessDeps,
  })
    .catch((err: unknown): HarnessResult<O> | null => {
      // `onFailure: 'throw'` harnesses (research synthesis, channel-plan) throw
      // out of the runner by declaration. A benchmark that let that escape would
      // let one harness's failure policy end the sweep, which is the opposite of
      // what a sweep is for. The row is already captured — `run.ts` writes it
      // before it throws, precisely so a throwing harness stays visible.
      capture.threw = err instanceof Error ? err.message : String(err)
      return null
    })

  // SIZED TO WHAT THIS CASE MAY DO, not to a flat single-call figure — see
  // `turnsPerCase`. The caller's budget is the PER-TURN allowance.
  const caseMs = timeoutMs * turnsPerCase(def as HarnessDefinition<unknown, unknown>, dryRun, supplier !== null)
  const outcome = await Promise.race([
    bounded(work, caseMs, () => {
      capture.timedOut = true
      controller.abort()
    }),
    stopSeen.then(() => ({ done: false }) as const),
  ]).finally(() => {
    clearInterval(poll)
    slot.delete(caseKey(def.id, fixture.name))
  })

  // THE ABORT ACTUALLY CANCELS NOW. `ctx.signal` reaches the transport and, as
  // of the same round, `UpstreamCall.signal` reaches the socket — so a stopped
  // case releases its upstream connection instead of running to completion with
  // nobody waiting for it.
  if (cancelled) return null

  const row = capture.row
  // The typed result, straight off the race — never read back out of the
  // capture, so the fixture's `check` sees an O rather than an `unknown` this
  // file had to assert its way back to.
  const result: HarnessResult<O> | null = outcome.done ? outcome.value : null
  const contractHeld = row ? row.schemaValid : (result?.schemaValid ?? false)
  const repairs = row ? row.repairs : (result?.repairs ?? 0)
  const firstPass = contractHeld && repairs === 0

  // The fixture grades a MODEL VALUE or nothing. A contract failure has no value
  // to grade, and `onFailure: { fallback }` hands back the harness author's
  // declared constant with `schemaValid: false` over it — grading that would
  // award the model marks for a value it never produced.
  // WHAT THE MODEL DID, for a fixture that grades behaviour rather than prose.
  // `NO_TOOLS` for everything else, so a single-shot fixture reaching for
  // `ctx.calls` sees an honest empty list rather than undefined.
  const evalContext: EvalContext = sandbox
    ? { calls: sandbox.calls, calledBefore: sandbox.calledBefore, world: sandbox.world ?? null, exhausted: dry.result?.exhausted ?? false }
    : NO_TOOLS

  let task: EvalCaseScore['task'] = 'unscored'
  let taskError: string | null = null
  /** THE HARNESS'S OWN GAP, kept apart from the model's score — see
   *  `CheckResult` in define.ts. A fixture that could not fairly ask its
   *  question reports it here and the case is scored `unscored`, exactly as a
   *  skip is: "we did not give it what the job needed" is not "it answered
   *  badly", and attributing one to the other measures our fixture and calls it
   *  a capability. */
  let gap: string | null = null
  if (contractHeld && result && result.value !== null) {
    try {
      const verdict = fixture.check(result.value, evalContext)
      if (isGap(verdict)) gap = verdict.gap
      else taskError = verdict
    } catch (err) {
      // A fixture check is author code meeting model output, the same as
      // `clean` and `verify`, and `run.ts` holds those to "a throw is a
      // failure, never an escaped exception". A sweep that died on one badly
      // written assertion would take 22 other harnesses with it.
      taskError = `the fixture check threw on the value: ${err instanceof Error ? err.message : String(err)}`
    }
    // WE CUT IT OFF, THEN JUDGED THE RESULT.
    //
    // An exhausted dry run is a model still working when the loop's budget ran
    // out. Grading the half-finished state against an assertion that asks
    // whether the job was DONE charges our budget to the model — the same
    // category error as scoring a 429 as a contract failure, and the one this
    // whole round keeps finding in different clothes.
    //
    // IT IS SAFE AS A BLANKET RULE, which is not obvious and is worth stating. A
    // fixture that measures RESTRAINT (did it refrain from writing, did it not
    // manufacture activity) is satisfied by a model that called almost nothing —
    // and a model that called almost nothing cannot have exhausted a six-turn
    // loop. So the fixtures this would wrongly excuse are the ones it cannot
    // reach. The failure it does excuse is real every time: a work session
    // stopped at turn six and asked why the ticket is not finished.
    //
    // The gap is reported to US, which is the point: it says either the budget
    // is too small for the job or the harness is asking for more than the job
    // needs, and both are ours to fix.
    // OUR SEARCH FOUND NOTHING, so the fixture could not fairly ask its question.
    //
    // WHAT THIS COSTS WHEN IT IS MISSING. Asked what NIST 800-53 AC-2 requires,
    // a model called the search tool four times and got back NIST's homepage,
    // the NIST Chemistry WebBook and a Wikipedia article. It then did exactly
    // what the harness asks — "I cannot state from these results what AC-2
    // requires, and I will not supply the content from memory" — and the sweep
    // recorded a task failure against the model for it. The better the model
    // behaves here, the worse it scores, which inverts the whole benchmark.
    //
    // THE SIGNAL IS THE SINK, not the prose. `toolSearchTransport` throws when
    // the model never called the tool at all (that IS a model failure, and a
    // serious one), so reaching here with an empty sink means the tool was
    // called and returned nothing citable. Reported to US: it says the search
    // backend cannot answer this fixture, which is either an engine to fix or a
    // fixture to rewrite, and both are ours.
    if (gap === null && taskError !== null && supplier !== null && sources.length === 0) {
      gap = `the model called "${supplier.tool}" and this deployment's search returned nothing citable, so the fixture judged an answer written from no sources ("${taskError}"). Fix the search backend or ask this fixture something the installed engines can find.`
      taskError = null
    }
    if (gap === null && taskError !== null && evalContext.exhausted) {
      gap = `the model was still working when the loop's ${turnsPerCase(def as HarnessDefinition<unknown, unknown>, dryRun, supplier !== null)}-turn budget ran out, and the assertion then judged unfinished work ("${taskError}"). Raise this harness's dryRun.maxTurns or ask the fixture something a bounded loop can answer.`
      taskError = null
    }
    task = gap !== null ? 'unscored' : taskError === null ? 'pass' : 'fail'
  }

  // THE FLOOR DECLINED TO ASK — no question reached the model, so nothing here
  // is a fact about it. Recorded as a SKIP, exactly like a harness this
  // candidate's transport cannot drive.
  //
  // IT USED TO BE A FAILURE, and it was charged to the candidate. The health
  // view showed glm-5.2 failing five `research-search` fixtures on
  // `"glm-5.2" cannot run harness "research-search"` — a refusal it never saw,
  // recorded as five wrong answers. That is the exact category error the
  // capability floor exists to make visible, committed by the code that reads
  // the floor's own verdict.
  if (result?.refused) {
    return skippedCase(def.id, fixture.name, fixture.band ?? 'standard', result.error ?? 'the capability floor refused this model, so the fixture was never asked')
  }

  // A GAP IS NOT CLEAN. `clean` decides whether the drill-down (the prompt, the
  // raw reply, the transcript) is kept or dropped, and a gap is exactly the case
  // where it matters most: the fixture is telling US it could not fairly ask its
  // question, and the first thing whoever owns that fixture needs is what was
  // actually sent and what came back. Dropping it because `task !== 'fail'` made
  // our own bugs the only failures in the suite with no evidence attached.
  const clean = outcome.done && contractHeld && task !== 'fail' && gap === null
  const costUsd =
    capture.promptTokens + capture.completionTokens > 0 ? await deps.price(model, capture.promptTokens, capture.completionTokens).catch(() => null) : null

  return {
    harness: def.id,
    case: fixture.name,
    band: fixture.band ?? 'standard',
    skipped: null,
    contractHeld,
    firstPass,
    repairs,
    answered: result?.answered ?? false,
    task,
    taskError,
    gap,
    findings: row?.findings ?? 0,
    latencyMs: row?.latencyMs ?? 0,
    startedAt: new Date(startedAt).toISOString(),
    wallMs: Date.now() - startedAt,
    promptTokens: capture.promptTokens,
    completionTokens: capture.completionTokens,
    costUsd,
    estimated: capture.estimated,
    timedOut: capture.timedOut,
    optimistic: contractHeld && task === 'fail',
    error: capture.timedOut ? timeoutDetail(caseMs, settleOpen(capture.upstream, startedAt)) : (capture.threw ?? row?.error ?? result?.error ?? null),
    prompt: clean ? null : cap(capture.prompt),
    raw: clean ? null : cap(result?.raw ?? null),
    // THE TRANSCRIPT FOLLOWS THE SAME RULE AS THE PROMPT — kept when something
    // went wrong, dropped when nothing did. THE CALL LOG DOES NOT: it is small,
    // it is what every behavioural fixture actually asserts over, and comparing
    // two models on one fixture means comparing the two lists. Available only on
    // failure would mean the comparison worth making is the one you cannot see.
    turns: clean ? null : recordTurns(dry.result?.messages),
    calls: recordCalls(sandbox, !clean),
    upstream: clean ? null : settleOpen(capture.upstream, startedAt),
  }
}

/** THE SWEEP: every fixture in the registry, replayed against one candidate.
 *
 *  RESUMABLE. The persisted status carries the scored cases, so a sweep an admin
 *  stopped — or a deploy interrupted — restarts where it left off rather than
 *  re-buying seventy calls. Resume is only ever within ONE candidate: a status
 *  for a different model is discarded, because a matrix cell assembled from two
 *  models is a number with no referent.
 *
 *  BOUNDED. Every case races a wall clock (`EvalOptions.caseTimeoutMs`) and a
 *  case that loses is recorded `timedOut` and left behind. No single harness can
 *  strand the sweep, whatever its transport does.
 *
 *  SEQUENTIAL, on purpose. A parallel sweep measures the gateway's queue rather
 *  than the model, so `latencyP50` would stop meaning what the fitness page
 *  says it means; and a self-hosted 14B behind one GPU answers a parallel sweep
 *  with rate-limit errors that would score as contract failures. */
export async function runEvalSweep(model: string, opts: EvalOptions = {}): Promise<EvalSweep> {
  const deps: EvalDeps = { ...REAL_DEPS, ...opts.deps }
  const timeoutMs = opts.caseTimeoutMs ?? DEFAULT_CASE_TIMEOUT_MS

  // One sweep at a time PER CANDIDATE. A second concurrent sweep of the same
  // model would interleave two runs' cases into one checkpoint; different
  // candidates keep separate ones and run side by side. The caller gets the
  // RUNNING sweep's progress back rather than an error — the second press of a
  // Test button means "show me the run", not "start a second one" — with no
  // harness scores on it, because scoring a half-finished sweep would print a
  // contract rate over the cases that happened to be done.
  if (sweeping.has(model)) return sweepOf(await deps.readStatus(model).catch(() => IDLE_STATUS), [], [], false)
  sweeping.add(model)
  stopRequested.delete(model)

  const iso = (at: number): string => new Date(at).toISOString()
  // THE PERSISTED REQUEST, POLLED — not read at harness boundaries.
  //
  // `stopRequested` is in-process and instant, and it is empty in any instance
  // that did not start the run: a Stop pressed against a different worker only
  // ever arrives through `shouldStop`, which reads the persisted flag. That used
  // to be asked once per harness, so a cross-process Stop waited for the current
  // harness to finish — eleven work-session fixtures at up to seven minutes each.
  //
  // The watcher below keeps `externallyStopped` fresh so the SYNCHRONOUS
  // predicate handed to `runHarnessCases` (and, through it, to the poller inside
  // each case) is never more than a second stale. It cannot be an await on the
  // hot path: a case checks twice a second and a settings read per check would
  // put the sweep's cadence on the database.
  let externallyStopped = false
  const asked = async (): Promise<boolean> => {
    if (stopRequested.has(model)) return true
    externallyStopped = externallyStopped || (await opts.shouldStop?.(model).catch(() => false)) === true
    return externallyStopped
  }
  /** True the instant either half of the request lands. Sync, so a case can ask
   *  it from a timer. */
  const stoppedNow = (): boolean => stopRequested.has(model) || externallyStopped

  // ── Width, and the valve that moves it ─────────────────────────────────────
  //
  // IT USED TO BE ONE WAY ONLY, on the theory that a provider which rate-limited
  // once under this load will do it again, and that an oscillating width would
  // spend the run rediscovering the same ceiling and scoring the 429s it found
  // on the way as model failures. The first half is right about rate limits and
  // wrong about everything else the valve fires on: `pressured` also covers a
  // LOST REQUEST — a single HTTP call that went out and never came back — which
  // is not a ceiling and carries no information about the next case. A 247-case
  // sweep of deepseek-v4-flash requested width 4, met one of those in its first
  // minute, and ran the remaining two hundred and forty cases sequentially.
  //
  // So it reopens, and the shape of the reopening is what answers the original
  // objection: down by HALVES on any pressure, up by ONE after `RECOVER_AFTER`
  // clean cases in a row. A real ceiling is found in two or three narrowings and
  // then held, because every attempt to climb past it costs one case and is
  // immediately halved back; a transient loss costs five clean cases of recovery
  // and nothing else.
  // ── Unreachable, and when to stop asking ───────────────────────────────────
  //
  // A structural refusal does not get better on the next fixture. Counted in a
  // ROW rather than in total: one 404 among two hundred passes is a blip worth
  // recording and ignoring; three with nothing in between is a fact about the
  // whole run.
  let unreachableRun = 0
  let unreachableWhy: string | null = null
  const noteUnreachable = (why: string): void => {
    unreachableRun++
    unreachableWhy ??= why.slice(0, 300)
  }

  let width = Math.max(1, Math.min(opts.concurrency ?? DEFAULT_CONCURRENCY, MAX_CONCURRENCY))
  const startedWidth = width
  let lowWidth = width
  let narrowedBecause: string | null = null
  /** Clean cases since the last time the provider pushed back. */
  let calm = 0
  const narrow = (why: string): void => {
    // THE REASON IS RECORDED EVEN WHEN THERE IS NOTHING LEFT TO NARROW. A sweep
    // already at width 1 that is still being rate-limited has learned the most
    // important thing on this page — the deployment cannot serve this run at all
    // right now — and dropping the sentence because the arithmetic had nowhere
    // to go would leave an admin reading unmeasured cases with no reason beside
    // them.
    narrowedBecause ??= why.slice(0, 200)
    calm = 0
    if (width <= 1) return
    width = Math.max(1, Math.floor(width / 2))
    lowWidth = Math.min(lowWidth, width)
  }
  /** ONE CLEAN CASE. Reopens a lane once `RECOVER_AFTER` of them have landed in
   *  a row at the current width — see that constant for why the sweep comes down
   *  fast and goes back up slowly. */
  const settled = (): void => {
    if (width >= startedWidth) return
    if (++calm < RECOVER_AFTER) return
    calm = 0
    width = Math.min(startedWidth, width + 1)
  }
  const valve: Valve = { width: () => width, ceiling: startedWidth, narrow, settled }
  const watcher = setInterval(() => void asked(), STOP_WATCH_MS)
  ;(watcher as unknown as { unref?: () => void }).unref?.()
  try {
    const all = await deps.harnesses()
    const wanted = opts.only?.length ? all.filter((h) => opts.only?.includes(h.id)) : all
    const metas = wanted.map(metaOf)
    const unfixtured = wanted.filter((h) => h.evalNames.length === 0).map((h) => h.id)

    // A persisted status for THIS model is a resume point. A persisted
    // 'running' with `sweeping` false is a sweep a restart interrupted — also
    // resumable, and treating it as stuck instead would leave the feature
    // permanently unusable after one unlucky deploy.
    const prior = await deps.readStatus(model).catch(() => IDLE_STATUS)
    // A FINISHED RUN IS RESUMABLE WHEN YOU ARE RETRYING IT, and that is the
    // whole point of the button: the run an admin wants to re-ask five cases of
    // is one that RAN TO COMPLETION with five holes in it. Without this clause
    // `retryFailed` silently did nothing — a done run was not resumable, so
    // there was no prior ledger to keep the passes from, and it re-bought
    // everything exactly like `restart`. Caught by its own test, not by reading.
    const resumable =
      !opts.restart && prior.model === model && prior.state !== 'idle' && (opts.retryFailed === true || opts.supplement === true || prior.state !== 'done')
    // A CASE THAT REACHED A CLEAN VERDICT IS EVIDENCE; anything else is a hole.
    // `retryFailed` keeps the first and re-opens the second, so the sweep's
    // ordinary resume machinery does the rest — the dropped cases are simply
    // pending again.
    // EVERY FIXTURE THE REGISTRY DECLARES RIGHT NOW. A supplemental sweep is
    // exactly "the declared set minus what the ledger already answers", so it
    // needs the declared set — and computing it here also gives the prune
    // something to prune against.
    const declared = new Set(wanted.flatMap((h) => h.evalNames.map((name) => caseKey(h.id, name))))
    const priorCases = resumable ? prior.cases : []
    const kept = opts.retryFailed
      ? priorCases.filter((c) => !worthRetrying(c))
      : // A VERDICT ABOUT A FIXTURE THAT NO LONGER EXISTS is a verdict on an
        // assertion nobody can read, and it keeps scoring the matrix. Dropped on
        // a supplemental pass, which is the pass whose whole subject is the
        // difference between the ledger and the registry.
        opts.supplement
        ? priorCases.filter((c) => declared.has(caseKey(c.harness, c.case)))
        : [...priorCases]
    const cases: EvalCaseScore[] = kept
    const already = new Set(cases.map((c) => caseKey(c.harness, c.case)))
    /** Cases THIS PASS produced, as opposed to ones it inherited. Speed is
     *  measured over these — see `EvalSweep.measured`. */
    const measured: EvalCaseScore[] = []

    const total = wanted.reduce((n, h) => n + h.evalNames.length, 0)
    const startedAt = resumable ? prior.startedAt : iso(deps.now())

    const write = (state: EvalSweepState, harness: string | null, error: string | null): Promise<void> =>
      deps
        .writeStatus({
          state,
          model,
          startedAt,
          finishedAt: state === 'running' ? null : iso(deps.now()),
          done: cases.length,
          total,
          harness,
          error,
          cases,
        })
        .catch(() => {})

    await write('running', null, null)

    // Read once, and report it: with the guard off every `guardRate` is zero,
    // and zero-because-off must never be read as zero-because-clean.
    const guardConfig = deps.harnessDeps.guardConfig ?? getGuardConfig
    const guarded = await guardConfig()
      .then((c) => c.mode !== 'off')
      .catch(() => false)

    // ASKED ONCE, NOT PER HARNESS. The answer is a property of the candidate's
    // transport and cannot change inside one sweep; asking per harness would put
    // a fleet listing on the hot path three times for no new information.
    const ownTools = await deps.servesOwnTools(model).catch(() => true)
    // CAN THE PLATFORM SUPPLY THE LOOP INSTEAD. A gateway model has no loop of
    // its own and can be handed definitions, which is what lets the three
    // tool-loop harnesses be measured on it at all — see `harnessSkipReason`.
    const toolDefs = await deps.acceptsToolDefinitions(model).catch(() => false)

    for (const harness of wanted) {
      if (await asked()) break
      // STOP ASKING. Two hundred and forty more cases would each buy the same
      // 404 and tell an admin nothing they did not know by case three; the
      // sweep ends with the reason instead of an hour of identical failures.
      if (unreachableRun >= UNREACHABLE_STREAK) break
      const pending = harness.evalNames.filter((name) => !already.has(caseKey(harness.id, name)))
      if (pending.length === 0) continue

      // THE SKIP, BEFORE A TOKEN IS SPENT. A harness this candidate's transport
      // is documented to refuse produces no reply on any fixture, so the sweep
      // records the absence and its reason instead of buying `pending.length`
      // refusals and scoring them as contract failures. The cases are still
      // WRITTEN — they are the resume ledger and the progress denominator, and a
      // sweep that merely `continue`d would restart into the same skip and show
      // a progress bar that never reaches its total.
      const skip = harnessSkipReason(harness, model, { ownToolLoop: ownTools, toolDefinitions: toolDefs })
      if (skip) {
        for (const name of pending) {
          cases.push(skippedCase(harness.id, name, harness.bandOf[name] ?? 'standard', skip))
        }
        await write('running', harness.id, null)
        continue
      }

      // Persisted AFTER EVERY CASE, not after every harness. The status is both
      // the progress bar and the resume ledger, and a sweep that checkpointed
      // per harness would re-buy a whole harness's fixtures after a restart —
      // and, on the slowest harnesses, show a progress bar that does not move
      // for minutes.
      // A DRY RUN is for a tool-loop harness the PLATFORM has to drive: the model
      // cannot run its own loop here, but it can be handed definitions, so the
      // sweep supplies the loop and an isolated Talaria to run it against. A
      // fleet persona runs its own loop and needs none of this.
      const dryRun = harness.tools === 'own' && !ownTools && toolDefs
      await harness.use(<I, O>(def: HarnessDefinition<I, O>) =>
        runHarnessCases(
          def,
          pending,
          model,
          deps,
          timeoutMs,
          stoppedNow,
          dryRun,
          async (score) => {
            // THE STREAK RESETS ON ANYTHING THAT REACHED THE MODEL. Only an
            // unbroken run of refusals means the deployment cannot serve this
            // candidate at all.
            if (score.skipped === null || !/could not reach this model/.test(score.skipped)) unreachableRun = 0
            cases.push(score)
            measured.push(score)
            // FILED AS IT LANDS, so a sweep an admin stops still keeps every
            // transcript it paid for. Never blocks the sweep: the archive is
            // valuable and the run is more valuable.
            // `startedAt` is the run's identity in the archive — a resumed sweep
            // keeps the original, so its cases file under one run rather than
            // splitting into two half-runs nobody can audit.
            await opts.archiveCase?.(model, startedAt ?? iso(deps.now()), score).catch(() => {})
            await write('running', harness.id, null)
          },
          valve,
          opts.pressureBackoffMs ?? PRESSURE_BACKOFF_MS,
          noteUnreachable,
        ),
      )
    }

    const state: EvalSweepState = stoppedNow() ? 'stopped' : 'done'
    // The run's own headline when it gave up: a routing or credential fact, said
    // ONCE — in the status row AND on the returned sweep, because the archive
    // reads one and the caller reads the other.
    const gaveUp = unreachableRun >= UNREACHABLE_STREAK ? `the deployment could not reach this model — ${unreachableWhy}` : null
    await write(state, null, gaveUp)
    return sweepOf(
      { state, model, startedAt, finishedAt: iso(deps.now()), done: cases.length, total, harness: null, error: gaveUp, cases },
      metas,
      unfixtured,
      guarded,
      { requested: startedWidth, ended: width, low: lowWidth, narrowedBecause },
      measured,
    )
  } catch (err) {
    // Same shape as `reindexAll`: the failure lands in the status rather than
    // escaping to a route handler, because the admin who pressed the button is
    // watching this row and not a stack trace.
    const message = err instanceof Error ? err.message : String(err)
    const prior = await deps.readStatus(model).catch(() => IDLE_STATUS)
    const failed: EvalSweepStatus = { ...prior, state: 'error', model, finishedAt: iso(deps.now()), harness: null, error: message }
    await deps.writeStatus(failed).catch(() => {})
    return sweepOf(failed, [], [], false)
  } finally {
    clearInterval(watcher)
    await opts.archivePrune?.(model).catch(() => {})
    sweeping.delete(model)
    stopRequested.delete(model)
    // The case-level `finally` clears this on every normal path; this is for the
    // ones that never reach it (a throw in setup, a harness that blew up before
    // its race started). A "running now" that outlives its sweep makes a
    // finished run look wedged, which is exactly the confusion this panel exists
    // to remove.
    inFlight.delete(model)
  }
}

/** One harness's pending fixtures, in declaration order, stopping between cases
 *  when a stop was asked for. Split out so that `def`'s I and O stay paired
 *  under `use` — the closure remembers the types the registry erased. */
async function runHarnessCases<I, O>(
  def: HarnessDefinition<I, O>,
  pending: string[],
  model: string,
  deps: EvalDeps,
  timeoutMs: number,
  stopped: () => boolean,
  dryRun: boolean,
  onCase: (score: EvalCaseScore) => Promise<void>,
  valve: Valve,
  backoffMs: readonly number[],
  onUnreachable: (why: string) => void,
): Promise<void> {
  const fixtures = (def.evals ?? []).filter((f) => pending.includes(f.name))
  await pool(fixtures, valve, stopped, async (fixture) => {
    // RE-ASKED, NOT FAILED. Each attempt is a WHOLE fresh case — a new sandbox,
    // a new world, a clean capture — because a retry that reused the previous
    // attempt's mutated world would be grading the model on a board another run
    // had already written to.
    //
    // WHICH IS ALSO WHY THE CLOCK IS OUT HERE. Each attempt starts its own, so a
    // case that was re-asked would report only the surviving attempt's wall
    // time — a case whose first two requests vanished would claim to have cost
    // four seconds when it cost the sweep two minutes. `wallMs` is documented as
    // what the case cost the sweep, so it is measured across every attempt and
    // the backoff between them.
    const openedAt = Date.now()
    let score = await runOneCase(def, fixture, model, deps, timeoutMs, dryRun, stopped)
    for (let attempt = 0; attempt < PRESSURE_RETRIES; attempt++) {
      if (score !== null && pressured(score) && attempt >= retriesFor(score)) break
      // Null means the case was cancelled mid-flight. Nothing is recorded: the
      // fixture stays pending, so a resume picks it up rather than inheriting a
      // failure that never happened.
      if (score === null || !pressured(score) || stopped()) break
      // THE PRESSURE VALVE, on the way past: the width comes down as well as the
      // question being re-asked, so the retry is issued into a quieter sweep.
      valve.narrow(score.error ?? 'the request was never answered')
      await backoff(backoffMs[attempt] ?? backoffMs.at(-1) ?? 0, stopped)
      if (stopped()) return
      score = await runOneCase(def, fixture, model, deps, timeoutMs, dryRun, stopped)
    }
    if (score === null) return
    const whole = { ...score, startedAt: new Date(openedAt).toISOString(), wallMs: Date.now() - openedAt }
    if (unreachable(whole)) {
      onUnreachable(whole.error ?? 'the deployment could not reach this model')
      await onCase(unreachableCase(whole))
      return
    }
    // THE OTHER HALF OF THE VALVE. A case that came back — pass or fail, the
    // grade is not this counter's business — is evidence the deployment served
    // the width it was asked at. An `unreachable` case is NOT: it never left the
    // building, so it says nothing either way and returns above without voting.
    if (pressured(whole)) {
      await onCase(rateLimitedCase(whole))
      return
    }
    valve.settled()
    await onCase(whole)
  })
}

/** DID THIS CASE LEAVE A HOLE? Everything that is not a clean pass.
 *
 *  Deliberately generous. A skip costs nothing to re-attempt — a harness this
 *  candidate's transport cannot drive skips again in microseconds, before a
 *  token is spent — while a skip that was really a rate limit re-runs properly.
 *  A `gap` is re-asked because the usual reason to press this button is that
 *  somebody has just fixed the harness that reported it. Being generous costs a
 *  few free re-skips; being narrow costs a full sweep. */
export const worthRetrying = (c: EvalCaseScore): boolean => !(c.skipped === null && c.gap === null && c.contractHeld && c.task === 'pass')

/** A case that never reached the model at all — see `UNREACHABLE`.
 *
 *  UNMEASURED, LIKE A RATE LIMIT, and for the same reason: the sweep learned
 *  something about the deployment and nothing about the candidate. Recording it
 *  as a contract failure is what made one routing policy look like a model that
 *  fails every harness in the product. */
function unreachableCase(score: EvalCaseScore): EvalCaseScore {
  return {
    ...score,
    skipped: `the deployment could not reach this model — ${(score.error ?? '').slice(0, 200)}. Nothing here was measured about the model itself.`,
    contractHeld: false,
    firstPass: false,
    answered: false,
    task: 'unscored',
    optimistic: false,
  }
}

/** A case the provider never let us ask, after every retry.
 *
 *  RECORDED AS UNMEASURED, NOT AS FAILED. `skipped` excludes a case from every
 *  rate — which is the honest arithmetic here, because we did not learn anything
 *  about the model. The alternative, leaving it as a contract failure, prints a
 *  red cell that means "your provider was busy" and reads as "this model cannot
 *  hold a contract". */
function rateLimitedCase(score: EvalCaseScore): EvalCaseScore {
  return {
    ...score,
    skipped: score.timedOut
      ? `the request was issued and never answered, on ${TIMEOUT_RETRIES + 1} attempts, each abandoned after the case budget — this case measured nothing about the model. The provider dropped the call; re-run it when the deployment is healthier.`
      : `the provider answered with rate limits on every attempt (${PRESSURE_RETRIES + 1} tries) — this case measured nothing about the model. Re-run it, narrower, when the deployment is quieter.`,
    contractHeld: false,
    firstPass: false,
    answered: false,
    task: 'unscored',
    optimistic: false,
  }
}

function sweepOf(
  status: EvalSweepStatus,
  metas: HarnessMeta[],
  unfixtured: string[],
  guarded: boolean,
  concurrency: SweepConcurrency = { requested: 1, ended: 1, low: 1, narrowedBecause: null },
  measured: EvalCaseScore[] = [],
): EvalSweep {
  return {
    concurrency,
    measured,
    model: status.model ?? '',
    state: status.state,
    startedAt: status.startedAt,
    finishedAt: status.finishedAt,
    done: status.done,
    total: status.total,
    error: status.error,
    harnesses: scoreHarnesses(metas, status.cases),
    cases: status.cases,
    unfixtured,
    guarded,
  }
}
