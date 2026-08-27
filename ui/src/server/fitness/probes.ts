// TIER-1 CAPABILITY PROBES — ~10 cheap calls that establish model-level FACTS.
//
// WHY THIS FILE IS THE POINT OF THE WHOLE HARNESS REFACTOR
//   Talaria has to be decent on a 7-14B self-hosted model and excellent on a
//   frontier one, and an admin has to be able to tell WHICH, per role, from the
//   UI. `capability.ts` is the type that answers that; `run.ts` reads it for the
//   floor and for widening. Nothing has ever WRITTEN it in the affirmative.
//
//   THIS IS THE FIRST PRODUCTION WRITER OF `value: true`. Until now
//   `recordCapability` had exactly one caller — `llm-gateway.ts`, which writes
//   `value: false` when an upstream 400s on a contract parameter — so every
//   capability-gated behavior in the product (floor refusals, widening) has
//   never fired in anger. That cuts both ways and both ways are dangerous:
//
//     a false `true`   silently widens a model's surface across the app. The
//                      Inbox hands a 7B model the item's whole action list
//                      because `tool-select` says it earned it.
//     a false `false`  refuses a working model. Probe facts DO NOT EXPIRE (see
//                      capability.ts) — a learned fact ages out in 30 days, a
//                      probe fact is a deliberate measurement and stands until
//                      someone re-measures. A wrong one is forever.
//
//   So every verdict below is asymmetric on purpose. Proof of PRESENCE is
//   allowed to be a single verified observation; proof of ABSENCE has to come
//   from a check that cannot fail for an unrelated reason. Where neither is
//   available the probe writes NOTHING, and an absent fact means unknown, which
//   `missingCapabilities` already treats as safe.
//
// WHAT RUNS AND WHAT DOES NOT, as of the tool-definition slot landing
//   `tools` and `tool-select` are ARMED: `TransportRequest.toolDefs` carries
//   real definitions to the model and `TransportReply.toolCalls` reports what it
//   called, so the fact that widens the Inbox command harness (audit 1.8) can
//   finally be recorded. They still SKIP on a fleet persona, whose tool loop
//   runs inside the agent container where we can neither place our tools nor see
//   the call — a skip, never a `false`, because nothing about the model was
//   measured. `vision` still skips everywhere: `Message.content` is a string
//   across the whole tree and a half-widened content-parts union would be worse
//   than none (the argument is written out on `ProbeDeps.askWithImages`).
//
// THE THREE RULES
//   1. DETERMINISTIC SCORING ONLY. No LLM-as-judge anywhere in tier 1. If it
//      cannot be checked with code — a parse, a string equality, a clock, an
//      HTTP GET, a `vm` run of the assertions — it is not a probe. That keeps
//      the suite fast, cheap, and free of the who-judges-the-judge regress.
//   2. A PROBE THAT ERRORS WRITES NOTHING. Transport down, 401, gateway
//      restarting: those are facts about the deployment, not about the model.
//      `runProbes` distinguishes "the transport threw" from "the model
//      answered badly" by watching the transport itself, not by string-matching
//      an error message.
//   3. THE ESTIMATE IS DATA. `estimateProbes` returns calls and tokens (and a
//      price when one is known) so the admin UI can show a number before
//      anyone spends money. Nothing in here prints.
//
// HOW A PROBE REACHES THE MODEL
//   Through `runHarness` with `ctx.model` pinned — the same runner, the same
//   `pickTransport` selection, so a fleet persona is probed exactly the way a
//   harness turn on it would run. Four dependencies are injected on every probe
//   call and each one closes a specific way a probe could lie:
//
//     missingCapabilities -> []   A PROBE MEASURES THE MODEL, NOT THE RECORD.
//                                 The runner suppresses `response_format` when
//                                 a `json: false` fact exists, so without this a
//                                 re-probe after one bad 400 could never observe
//                                 the model honoring JSON mode again — the
//                                 ratchet the TTL exists to release, reinstated
//                                 in the one tool built to release it.
//     capabilities -> {}          Same reason, for the widening gate.
//     recordRun -> no-op          `harness_runs` is the OBSERVED half of the
//                                 fitness matrix. Synthetic probe traffic in it
//                                 would corrupt the production contract rate the
//                                 page reads beside these facts.
//     recordFindings -> no-op     `guard_findings.model` is the live
//                                 confabulation rate per model. Probe prompts
//                                 are adversarial-ish by construction and would
//                                 inflate it.
import { runInNewContext } from 'node:vm'
import { z } from 'zod'
import { capabilityKey, getCapabilities, recordCapability, type Capability, type CapabilityFact, type CapabilityKey } from '../harness/capability'
import type { HarnessDefinition, Message } from '../harness/define'
import { personaCapabilityKeys } from '../harness/persona'
import type { GuardConfig } from '../guardrails'
import {
  defaultTransport,
  gatewayImageTurn,
  offersToolDefinitions,
  personaProbeTurn,
  runHarness,
  type HarnessDeps,
  type ToolCall,
  type ToolDefinition,
  type Transport,
} from '../harness/run'
import { gatewayPulse, routingFor, type GatewayPulse } from '../llm-gateway'
import { safeFetch } from '../safe-fetch'
import { advertisedWindow } from '../model-catalog'
import { estimateTokens } from '../usage'
import { noteLive } from './live-feed'
import type { EvalLogLine } from './surface'

// ── What a probe is ──────────────────────────────────────────────────────────

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

/** A native tool call the model made — the transport's own type, aliased rather
 *  than re-declared. Two structurally identical shapes across the seam is how a
 *  scorer starts reading a field the transport stopped filling. */
export type ProbeToolCall = ToolCall

const RAW_CAP = 1_200
const bounded = (s: string): string | null => (s ? s.slice(0, RAW_CAP) : null)

// ── The fixtures ─────────────────────────────────────────────────────────────
//
// Static, so `estimateProbes` can size a run exactly rather than guessing, and
// so a scorer test can drive the same prompt the production probe sends.

const sys = (content: string): Message => ({ role: 'system', content })
const usr = (content: string): Message => ({ role: 'user', content })

/** Every probe opens with this. Small models follow a terse system line better
 *  than a chatty one, and a probe that lost a trial to a preamble would be
 *  measuring our prompt rather than the model. */
const TERSE = sys('You answer exactly what is asked, with nothing else. No preamble, no explanation, no markdown fence.')

const JSON_TRIVIAL_SCHEMA = z.object({ name: z.string(), count: z.number(), ok: z.boolean() })

/** Three phrasings of the same trivial object. Three rather than one because a
 *  single lucky parse is not a capability, and rather than five because this
 *  probe's real question is the PROTOCOL one (did `response_format` survive),
 *  which one call already answers. */
const JSON_TRIALS: ReadonlyArray<{ name: string; messages: Message[] }> = [
  { name: 'trivial object', messages: [TERSE, usr('Return a JSON object with the keys name (the string "talaria"), count (the number 3) and ok (the boolean true).')] },
  { name: 'trivial object, reordered', messages: [TERSE, usr('Return JSON. It must have ok set to true, count set to 3, and name set to "talaria".')] },
  { name: 'trivial object, restated', messages: [TERSE, usr('Give me one JSON object describing a record whose name is "talaria", whose count is 3, and which is ok.')] },
]

const JSON_STRICT_SCHEMA = z.object({
  id: z.string().min(1),
  tags: z.array(z.string().min(1)).min(2),
  items: z.array(z.object({ label: z.string().min(1), weight: z.number() })).min(2),
  // The long string field is where small models break structured output: they
  // start the prose, forget they are inside JSON, and close the object early or
  // emit an unescaped newline. `min(200)` is the point of the field.
  summary: z.string().min(200),
})

const JSON_STRICT_INSTRUCTION =
  'Return exactly one JSON object with these keys and nothing else:\n' +
  '  id       a short string identifier\n' +
  '  tags     an array of at least 2 short lowercase strings\n' +
  '  items    an array of at least 2 objects, each { "label": string, "weight": number }\n' +
  '  summary  a single string of AT LEAST 200 characters'

const JSON_STRICT_TRIALS: ReadonlyArray<{ name: string; messages: Message[] }> = [
  'a small coastal town', 'a distributed build cache', 'a rescued greyhound', 'a municipal water tender', 'a used bookshop',
].map((subject) => ({
  name: `nested object: ${subject}`,
  messages: [TERSE, usr(`${JSON_STRICT_INSTRUCTION}\n\nThe object describes: ${subject}.`)],
}))

/** The classic small-model tell, and the one the titler and the summarizer lean
 *  on hardest: both end their prompt with "reply with ONLY the title/summary",
 *  and both are `output.kind: 'text'`, so a model that adds "Sure! Here you go:"
 *  fails their contract on every single run. */
const INSTRUCTION_TRIALS: ReadonlyArray<{ name: string; messages: Message[]; expect: string }> = [
  { name: 'exactly OK', messages: [TERSE, usr('Reply with exactly the word OK. No punctuation, no explanation.')], expect: 'OK' },
  { name: 'exactly three words', messages: [TERSE, usr('Reply with exactly these three words in this order and nothing else: red green blue')], expect: 'red green blue' },
  { name: 'exactly one digit', messages: [TERSE, usr('Reply with the single digit 7 and nothing else.')], expect: '7' },
]

const SEARCH_SCHEMA = z.object({ date: z.string(), url: z.string(), quote: z.string() })

const SEARCH_INSTRUCTION =
  'Search the web right now, open one page, and reply with exactly one JSON object:\n' +
  '  {"date": "<today\'s date in UTC as YYYY-MM-DD>", "url": "<the full https URL of the page you opened>", ' +
  '"quote": "<a verbatim sentence of at least 40 characters copied from that page>"}'

const SEARCH_TRIALS: ReadonlyArray<{ name: string; messages: Message[] }> = [
  { name: 'fresh page, verbatim quote', messages: [TERSE, usr(`${SEARCH_INSTRUCTION}\n\nThe page must be a news story published in the last two days.`)] },
  { name: 'fresh page, verbatim quote (second topic)', messages: [TERSE, usr(`${SEARCH_INSTRUCTION}\n\nThe page must be documentation for a software project, opened today.`)] },
]

/** One code task: a precise contract plus the assertions it must satisfy. The
 *  assertions ARE the grade — no second model reads the code and forms an
 *  opinion about it, which is the whole difference between a probe and a vibe. */
export interface CodeTask {
  name: string
  /** The function the model must define at top level. */
  fn: string
  prompt: string
  /** Each is `[arguments, expected]`. Compared structurally. */
  cases: ReadonlyArray<{ args: unknown[]; expect: unknown }>
}

export const CODE_TASKS: readonly CodeTask[] = [
  {
    name: 'slugify',
    fn: 'slugify',
    prompt:
      'Write a JavaScript function `slugify(input)` and nothing else. It lowercases the input, replaces every run of ' +
      'characters that are not a-z or 0-9 with a single "-", and removes any leading or trailing "-". ' +
      'Reply with the function source only.',
    cases: [
      { args: ['Hello, World!'], expect: 'hello-world' },
      { args: ['  A  B  '], expect: 'a-b' },
      { args: ['Talaria Harness 2.0'], expect: 'talaria-harness-2-0' },
      { args: [''], expect: '' },
      { args: ['---already---slugged---'], expect: 'already-slugged' },
    ],
  },
  {
    name: 'mergeRanges',
    fn: 'mergeRanges',
    prompt:
      'Write a JavaScript function `mergeRanges(ranges)` and nothing else. `ranges` is an array of [start, end] number ' +
      'pairs. It returns a new array of merged, non-overlapping pairs sorted by start; ranges that touch at an endpoint ' +
      '(for example [1,2] and [2,3]) merge into one. It must not modify the input. Reply with the function source only.',
    cases: [
      { args: [[[1, 3], [2, 6], [8, 10]]], expect: [[1, 6], [8, 10]] },
      { args: [[[8, 10], [1, 3]]], expect: [[1, 3], [8, 10]] },
      { args: [[[1, 2], [2, 3]]], expect: [[1, 3]] },
      { args: [[]], expect: [] },
    ],
  },
]

// ── Scorers: pure, deterministic, and the entire test surface ────────────────

/** Pass rate over the CONCLUSIVE trials. Null when nothing was conclusive —
 *  which is the signal to write no fact at all. */
export function rateOf(trials: readonly Trial[]): number | null {
  const graded = trials.filter((t) => t.ok !== null)
  if (graded.length === 0) return null
  return graded.filter((t) => t.ok === true).length / graded.length
}

const pct = (n: number): string => `${Math.round(n * 100)}%`

/** The reason the first failing trial gives, for the one-line detail. */
const firstFailure = (trials: readonly Trial[]): string => trials.find((t) => t.ok === false)?.note ?? 'no failure recorded'

/** `json` — a trivial object requested WITH `response_format`.
 *
 *  THE SILENT-STRIP CASE (audit 1.2) IS THE POINT. `llm-gateway.ts` learns
 *  unsupported parameters from upstream 400s and pre-strips them forever after;
 *  `response_format` was as strippable as `top_p`, so a model that refuses JSON
 *  mode had the constraint deleted, the retry SUCCEEDED, and the caller — which
 *  had asked for JSON precisely because it was about to run a JSON parser — got
 *  free prose. A model whose reply happens to parse in that state does NOT have
 *  this capability: the next caller with a harder schema gets prose, and the
 *  runner's `jsonMode` will keep asking for something the endpoint throws away.
 *  So a reported drop is `false` regardless of how well the replies parsed. */
export function scoreJson(trials: readonly Trial[], protocol: { requested: boolean; dropped: boolean }): ProbeVerdict | null {
  const rate = rateOf(trials)
  if (rate === null) return null
  if (!protocol.requested) {
    // Cannot happen through `runProbes` — it injects an empty capability record
    // precisely so the runner always asks. Kept because a caller supplying its
    // own `ask` could reach it, and "we never tested the thing we are about to
    // record" must not become a recorded fact.
    return null
  }
  // THIS FACT IS ABOUT THE MODEL, NOT THE ENDPOINT, and it used to be about
  // both. A dropped `response_format` returned `value: false` even when every
  // reply parsed — the detail said so in as many words ("replies still parsed
  // 100% of the time, but the JSON constraint is not honored here"). One word
  // therefore carried two unrelated claims: "this model cannot produce JSON"
  // and "this server does not implement response_format".
  //
  // THAT CONFLATION BECAME LOAD-BEARING the moment a JSON harness put `json` in
  // its floor: a self-hosted llama.cpp or Ollama box whose models emit perfect
  // JSON would have had all nine structured harnesses declared unfit, for a
  // property of the SERVER. The deployment half is already tracked where it
  // belongs — `contractDropped` on the reply, and the gateway's learned-param
  // ratchet, which is what suppresses the parameter on later calls.
  //
  // So the verdict is the parse rate either way, and the drop only changes the
  // sentence: on a dropped parameter this measured whether the model returns
  // JSON when ASKED IN PROSE, which is the harder question and the one a floor
  // should turn on.
  if (rate < 1) {
    return { value: false, score: rate, detail: `only ${pct(rate)} of ${trials.length} JSON-mode calls returned a usable object - ${firstFailure(trials)}` }
  }
  return protocol.dropped
    ? {
        value: true,
        score: 1,
        detail: `returned a valid object on all ${trials.length} calls, though this endpoint dropped response_format - the model produces JSON from the prompt alone`,
      }
    : { value: true, score: 1, detail: `honored response_format and returned a valid object on all ${trials.length} calls` }
}

/** `json-strict` — nested arrays plus a 200-character string field, scored as a
 *  conformance RATE rather than pass/fail. A model at 4/5 is genuinely usable
 *  behind the runner's repair turn; one at 1/5 is not, and the number is what
 *  tells them apart. */
const JSON_STRICT_FLOOR = 0.8

export function scoreJsonStrict(trials: readonly Trial[]): ProbeVerdict | null {
  const rate = rateOf(trials)
  if (rate === null) return null
  return rate >= JSON_STRICT_FLOOR
    ? { value: true, score: rate, detail: `${pct(rate)} of ${trials.length} nested-schema objects conformed on the first attempt` }
    : { value: false, score: rate, detail: `only ${pct(rate)} of ${trials.length} nested-schema objects conformed - ${firstFailure(trials)}` }
}

/** `tools` — one tool definition, one prompt that cannot be answered without
 *  calling it. Any correct call is proof; nothing else is. */
export function scoreTools(trials: readonly Trial[]): ProbeVerdict | null {
  const rate = rateOf(trials)
  if (rate === null) return null
  return rate === 1
    ? { value: true, score: 1, detail: 'called the offered tool with well-formed arguments' }
    : { value: false, score: rate, detail: `did not call the offered tool - ${firstFailure(trials)}` }
}

/** `tool-select` — 4 tools, 4 prompts, one correct tool each.
 *
 *  STRICT ON PURPOSE: this is the capability that WIDENS the Inbox command
 *  harness from a regex-chosen single action to the item's whole action list
 *  (audit 1.8). A model that picks right 3 times out of 4 has not earned that —
 *  the fourth pick is an action taken on somebody's ticket. Anything below 4/4
 *  is `false`, and the score says how close it got. */
export function scoreToolSelect(trials: readonly Trial[]): ProbeVerdict | null {
  const rate = rateOf(trials)
  if (rate === null) return null
  const graded = trials.filter((t) => t.ok !== null).length
  return rate === 1
    ? { value: true, score: 1, detail: `picked the correct tool on all ${graded} prompts` }
    : { value: false, score: rate, detail: `picked the correct tool on ${pct(rate)} of ${graded} prompts - widening needs all of them` }
}

/** `instruction-following` — "reply with exactly the word OK" and two siblings.
 *
 *  Compared after `trim()` and nothing else. A model that answers "OK." or
 *  "Sure — OK" did not do what it was told, and every text harness in the
 *  product (titler, summarizer, librarian) ends its prompt with exactly this
 *  kind of instruction. Being generous here would score a model as passing a
 *  test the product then fails it on. */
export function scoreInstruction(trials: readonly Trial[]): ProbeVerdict | null {
  const rate = rateOf(trials)
  if (rate === null) return null
  return rate === 1
    ? { value: true, score: 1, detail: `reproduced all ${trials.length} exact-output instructions verbatim` }
    : { value: false, score: rate, detail: `followed ${pct(rate)} of ${trials.length} exact-output instructions - ${firstFailure(trials)}` }
}

/** `search` — ASYMMETRIC, and this is the most carefully-hedged verdict here.
 *
 *  A pass means ONE ATTEMPT did the whole thing: named today's date, cited a
 *  URL, and quoted a sentence WE THEN FETCHED AND FOUND on that page.
 *
 *  A `false` is much harder to earn, because probe facts never expire and this
 *  one gates `research-recon`: it requires a trial that failed the DATE check,
 *  which needs no network of ours and no cooperation from the cited host. A
 *  model that got the date right and then failed only on the quote lands
 *  INCONCLUSIVE — plenty of search models cite pages that answer a bare GET with
 *  403, and refusing research to one of them for good would be worse than
 *  knowing nothing. */
export const SEARCH_DATE_TRIAL = 'date'
export const SEARCH_CITATION_TRIAL = 'citation'

/** The attempt a trial belongs to. `searchTrials` names both of one reply's
 *  observations `${name} / date` and `${name} / citation`, so the part before
 *  the separator is the reply they were read off. */
const attemptOf = (t: Trial): string => t.name.split(' / ')[0] ?? t.name

export function scoreSearch(trials: readonly Trial[]): ProbeVerdict | null {
  const rate = rateOf(trials)
  if (rate === null) return null
  // A VERIFIED QUOTE IS NOT PROOF ON ITS OWN, which this used to assume — the
  // quote check asks whether the sentence is on the page, and a model with a
  // large memorized corpus answers that from training data. `deepseek-v4-pro`
  // did exactly that: it passed the citation check on one attempt out of three
  // and was written `search: true` FOREVER, on an endpoint that returns no
  // citations at all. Research then ran its search stages natively on a model
  // that never searched, and every run died with an empty source registry.
  //
  // SO THE TWO OBSERVATIONS HAVE TO CORROBORATE EACH OTHER, and from the SAME
  // REPLY. A model that really searched knows what day it is; one quoting a page
  // it remembers is answering a question about the past. Neither check is
  // sufficient alone — the date is stamped into plenty of system prompts, and
  // the quote is memorizable — but a reply that lands both did the work. Trials
  // from different attempts are not evidence about each other, which is what
  // pairing by attempt enforces.
  const searchedForReal = trials.some(
    (t) =>
      t.ok === true &&
      t.name.includes(SEARCH_CITATION_TRIAL) &&
      trials.some((d) => d.ok === true && d.name.includes(SEARCH_DATE_TRIAL) && attemptOf(d) === attemptOf(t)),
  )
  if (searchedForReal) {
    return { value: true, score: rate, detail: 'named today’s date and quoted a sentence that is actually on the page it cited' }
  }
  const staleDate = trials.filter((t) => t.ok === false && t.name.includes(SEARCH_DATE_TRIAL))
  if (staleDate.length >= 2) {
    return { value: false, score: rate, detail: `no live data: ${staleDate[0]?.note ?? 'the model could not name today’s date'}` }
  }
  // INCONCLUSIVE, and this is where a verified-quote-but-stale-date model now
  // lands. It is the right answer for it: nothing here can tell a search model
  // having a bad day from a model with a good memory, and `capability-reach.ts`
  // sends the run through a real search tool either way.
  return null
}

/** `long-context` — a needle at 50% and 90% of the window actually tested. Both
 *  depths have to land: a model that finds the needle halfway in and loses it at
 *  90% is exactly the model that will drop the last of a long transcript. */
export function scoreLongContext(trials: readonly Trial[], tested: number, assumed = false): ProbeVerdict | null {
  const rate = rateOf(trials)
  if (rate === null) return null
  const window = tested.toLocaleString('en-US')
  // SAY WHEN THE WINDOW WAS ASSUMED. The measurement is exactly as real either
  // way — a needle either came back or it did not — but "we tested 32,000
  // tokens because that is our ceiling" and "we tested 32,000 tokens because
  // that is what this model advertises" support different conclusions, and an
  // admin reading a green tag deserves to know which they have.
  const how = assumed ? `${window}-token prompt (this model advertises no window, so the probe used its own ceiling)` : `${window}-token prompt`
  return rate === 1
    ? { value: true, score: 1, detail: `found the needle at 50% and 90% depth in a ${how}` }
    : { value: false, score: rate, detail: `found the needle in ${pct(rate)} of a ${how} - ${firstFailure(trials)}` }
}

/** `code` — graded by RUNNING the assertions, never by another model's opinion. */
export function scoreCode(trials: readonly Trial[]): ProbeVerdict | null {
  const rate = rateOf(trials)
  if (rate === null) return null
  return rate === 1
    ? { value: true, score: 1, detail: 'every code task passed every assertion when run' }
    : { value: false, score: rate, detail: `${pct(rate)} of the code tasks passed their assertions - ${firstFailure(trials)}` }
}

/** `vision` — only ever reached when the endpoint advertises it. */
export function scoreVision(trials: readonly Trial[]): ProbeVerdict | null {
  const rate = rateOf(trials)
  if (rate === null) return null
  return rate === 1
    ? { value: true, score: 1, detail: 'read every probe image correctly' }
    : { value: false, score: rate, detail: `read ${pct(rate)} of the probe images correctly - ${firstFailure(trials)}` }
}

// ── Deterministic checks the scorers are built from ──────────────────────────

/** Hosts a model reaches for when it is inventing a citation. A URL here is a
 *  fabricated source however well-formed it looks. */
const PLACEHOLDER_HOSTS = new Set(['example.com', 'www.example.com', 'example.org', 'example.net', 'localhost', 'test.com'])

export function citationProblem(url: string): string | null {
  let parsed: URL
  try {
    parsed = new URL(url)
  } catch {
    return 'the citation is not an absolute URL'
  }
  if (parsed.protocol !== 'http:' && parsed.protocol !== 'https:') return `the citation is not http(s): ${parsed.protocol}`
  if (!parsed.hostname.includes('.')) return `the citation has no real host: ${parsed.hostname}`
  if (PLACEHOLDER_HOSTS.has(parsed.hostname.toLowerCase())) return `the citation is a placeholder host: ${parsed.hostname}`
  return null
}

/** Days between an ISO date and now, or null when the string is not a date.
 *  Tolerance is a full day in each direction because "today in UTC" is a
 *  genuinely ambiguous question for a model whose provider stamps a local date
 *  into its system prompt, and we are not measuring timezone arithmetic. */
export function dateDriftDays(date: string, now: number): number | null {
  if (!/^\d{4}-\d{2}-\d{2}$/.test(date)) return null
  const at = Date.parse(`${date}T00:00:00Z`)
  if (!Number.isFinite(at)) return null
  const today = Date.parse(`${new Date(now).toISOString().slice(0, 10)}T00:00:00Z`)
  return Math.abs(at - today) / 86_400_000
}

/** Whitespace- and case-insensitive containment, with HTML tags removed from the
 *  haystack. A search model quotes rendered text; the page we fetch is markup,
 *  and the difference between them is tags and line wrapping, not content. */
export function quoteAppears(quote: string, page: string): boolean {
  const flat = (s: string): string => s.replace(/<[^>]*>/g, ' ').replace(/&nbsp;/g, ' ').replace(/\s+/g, ' ').trim().toLowerCase()
  const needle = flat(quote)
  if (needle.length < 40) return false
  return flat(page).includes(needle)
}

/** The code the model wrote, with the two wrappers it habitually adds removed.
 *
 *  Fences and a leading `export` are formatting, not contract: a model that
 *  produced a correct `slugify` and typed `export` in front of it solved the
 *  problem, and failing it for that would be scoring our extractor. Everything
 *  else — a class, a default export, an async function — is left alone and fails
 *  honestly in the `vm` below. */
export function extractCode(raw: string): string {
  const fenced = /```(?:[a-zA-Z]*)\n([\s\S]*?)```/.exec(raw)
  const body = fenced?.[1] ?? raw
  return body.replace(/^[ \t]*export\s+(?=(?:async\s+)?function\b|const\b|let\b|var\b)/gm, '')
}

/** Structural equality over the JSON-shaped values these assertions produce.
 *  `JSON.stringify` is not enough (key order) and a deep-equal library is not
 *  worth a dependency for numbers, strings and arrays. */
function sameValue(a: unknown, b: unknown): boolean {
  if (Object.is(a, b)) return true
  if (Array.isArray(a) && Array.isArray(b)) return a.length === b.length && a.every((x, i) => sameValue(x, b[i]))
  if (a && b && typeof a === 'object' && typeof b === 'object') {
    const ak = Object.keys(a as Record<string, unknown>).sort()
    const bk = Object.keys(b as Record<string, unknown>).sort()
    return ak.length === bk.length && ak.every((k, i) => k === bk[i]) && ak.every((k) => sameValue((a as Record<string, unknown>)[k], (b as Record<string, unknown>)[k]))
  }
  return false
}

/** How long the WHOLE task — definition plus every assertion — may run.
 *
 *  A wrong regex loop is an ordinary small-model failure and it has to cost
 *  250ms, not a wedged admin request. That is only true if the calls happen
 *  INSIDE the timed script: `vm`'s timeout covers the execution of the script it
 *  is given and nothing else, so pulling the function out and calling it
 *  afterwards puts an unbounded `while (true) {}` on the host's stack with no
 *  timeout anywhere near it. Written down because the obvious shape of this
 *  function is the broken one. */
export const CODE_TIMEOUT_MS = 250

const CODE_RESULT = z.object({
  nofn: z.boolean().optional(),
  out: z.array(z.object({ ok: z.boolean(), value: z.string().optional(), error: z.string().optional() })).optional(),
})

/** The script's own JSON coming back, validated. Defensive because the string is
 *  produced by a program the model wrote: a candidate that shadows
 *  `JSON.stringify` can return anything at all, and that has to fail the task,
 *  never the probe. */
function readCodeResult(reply: unknown): z.infer<typeof CODE_RESULT> | null {
  if (typeof reply !== 'string') return null
  try {
    const parsed = CODE_RESULT.safeParse(JSON.parse(reply))
    return parsed.success ? parsed.data : null
  } catch {
    return null
  }
}

/** Run one task's assertions against the model's source.
 *
 *  THIS IS NOT A SECURITY SANDBOX and must never be described as one. `node:vm`
 *  isolates globals, not the process. It is here because the alternative —
 *  grading code by asking another model whether it looks right — is not a
 *  measurement at all, and the input is a function this same admin just asked
 *  their own configured model to write.
 *
 *  What it does do is keep the boundary NARROW: the arguments cross as a JSON
 *  string and the results come back as a JSON string, so no host object is ever
 *  handed to candidate code, and the context holds nothing but a stubbed
 *  `console` (no `require`, no `process`, no `fetch`, no timers).
 *
 *  Returns null when every assertion passed, or the one line the admin reads. */
export function runCodeTask(task: CodeTask, raw: string): string | null {
  const src = extractCode(raw)
  if (!src.trim()) return `${task.name}: the model returned no code`
  const argsJson = JSON.stringify(JSON.stringify(task.cases.map((c) => c.args)))
  const script = `${src}
;(function () {
  if (typeof ${task.fn} !== 'function') return JSON.stringify({ nofn: true })
  var cases = JSON.parse(${argsJson})
  var out = []
  for (var i = 0; i < cases.length; i++) {
    try { out.push({ ok: true, value: JSON.stringify(${task.fn}.apply(null, cases[i])) }) }
    catch (e) { out.push({ ok: false, error: String((e && e.message) || e) }) }
  }
  return JSON.stringify({ out: out })
})()`
  let reply: unknown
  try {
    // `console` is stubbed rather than omitted: a model that left a debug log in
    // an otherwise correct function solved the problem, and a ReferenceError for
    // it would score our context instead of the model.
    const sandbox: Record<string, unknown> = { console: { log: () => {}, warn: () => {}, error: () => {} } }
    reply = runInNewContext(script, sandbox, { timeout: CODE_TIMEOUT_MS })
  } catch (err) {
    return `${task.name}: the code did not run (${messageOf(err)})`
  }
  const parsed = readCodeResult(reply)
  if (!parsed) return `${task.name}: the code did not produce a result`
  if (parsed.nofn) return `${task.name}: no function named ${task.fn} was defined`
  const out = parsed.out ?? []
  for (const [i, c] of task.cases.entries()) {
    const call = `${task.name}(${JSON.stringify(c.args).slice(1, -1)})`
    const got = out[i]
    if (!got) return `${call} was never reached`
    if (!got.ok) return `${call} threw: ${got.error ?? 'unknown error'}`
    // An absent `value` means the function returned undefined (or something
    // JSON cannot carry), which is a real answer and almost never the right one.
    const value: unknown = got.value === undefined ? undefined : JSON.parse(got.value)
    if (!sameValue(value, c.expect)) return `${call} returned ${JSON.stringify(value)}, expected ${JSON.stringify(c.expect)}`
  }
  return null
}

const messageOf = (err: unknown): string => (err instanceof Error ? err.message : String(err))

// ── The needle haystack ──────────────────────────────────────────────────────

/** Filler that is boring, uniform and countable, so the needle is the only thing
 *  in the prompt worth retrieving and depth is a real position rather than an
 *  artifact of where the interesting text happened to be. */
export function haystack(tokens: number, needle: string, depth: number): string {
  const lines: string[] = []
  // ~11 tokens a line at the 4-chars-per-token estimate the ledger uses.
  const target = Math.max(2, Math.floor(tokens / 11))
  for (let i = 0; i < target; i++) lines.push(`Line ${i + 1}: routine maintenance log entry, nothing of note recorded today.`)
  const at = Math.min(lines.length, Math.max(1, Math.round(lines.length * depth)))
  lines.splice(at, 0, needle)
  return lines.join('\n')
}

export const NEEDLE_QUESTION = 'What is the passphrase for record 4817? Reply with the passphrase and nothing else.'
export const needleLine = (token: string): string => `Line 0: IMPORTANT - the passphrase for record 4817 is ${token}.`

/** Smallest prompt worth calling a long-context test. Below this the probe is
 *  measuring ordinary recall and a `true` would overstate what was checked. */
export const MIN_LONG_CONTEXT_TOKENS = 8_000

/** The default ceiling on how much context a probe will pay for.
 *
 *  A 200k-window model probed at 90% depth is ~180k prompt tokens per trial —
 *  dollars, not cents, and this whole tier exists to reject a bad model in
 *  seconds for pennies. The probe therefore tests a CAPPED window and says so in
 *  the detail, so nobody reads `long-context: true` as a claim about the full
 *  advertised window. Raise it per run when the answer matters more than the
 *  bill. */
export const DEFAULT_MAX_CONTEXT_TOKENS = 32_000

/** THE CLOCK ONE PROBE RACES. Generous, because `long-context` legitimately
 *  sends two ~25k-token prompts and a reasoning model is slow per call — this is
 *  a backstop against a hung transport, not a performance budget. */
export const PROBE_TIMEOUT_MS = 180_000

// ── Injected edges ───────────────────────────────────────────────────────────

/** One probe call. `ask` is the plain text/JSON turn every probe but two takes;
 *  the tool probes go through `ToolAskSpec` below, and the image channel is the
 *  one that is still shut (see `ProbeDeps.askWithImages`). */
export interface AskSpec {
  id: string
  messages: Message[]
  /** Present: request JSON at the protocol level and validate against it. */
  schema?: z.ZodType<unknown>
}

/** A tool the probe OFFERS, which is exactly `TransportRequest.toolDefs`'s
 *  element type. Aliased for the same reason as `ProbeToolCall`: the fixture the
 *  test drives and the definition the model is sent must be one type. */
export type ToolSpec = ToolDefinition

export interface ToolAskSpec {
  id: string
  messages: Message[]
  tools: ToolSpec[]
}

export interface ToolAttempt {
  toolCalls: ProbeToolCall[]
  transportError: string | null
}

export interface ImageAskSpec {
  id: string
  messages: Message[]
  /** `data:` URLs. Never a remote fetch — a probe must not depend on a host. */
  images: string[]
}

export interface ProbeDeps {
  /** The pinned-candidate call. Default: `runHarness` with `ctx.model` set. */
  ask: (spec: AskSpec) => Promise<Attempt>
  /** THE SIXTH ASK, ARMED. `TransportRequest` now carries tool DEFINITIONS
   *  alongside the tool POLICY it always had (harness/transport.ts), and
   *  `TransportReply.toolCalls` reports what the model called, so both tool
   *  probes make real calls and score real answers.
   *
   *  What is still NOT allowed, and the reason this took a transport slot rather
   *  than an afternoon: a prompt-level imitation. "Reply with the name of the
   *  tool you would call" measures instruction following, and recording its
   *  result as `tools: true` would be exactly the false `true` this file exists
   *  to avoid — permanently, since probe facts do not expire. */
  askWithTools: (spec: ToolAskSpec) => Promise<ToolAttempt>
  /** CAN this candidate be offered tool definitions at all? False for a FLEET
   *  PERSONA: its tool loop runs inside the agent container, so tools we offer
   *  are neither guaranteed to reach the model nor observable when called, and
   *  the fleet transport refuses the call outright.
   *
   *  Asked BEFORE the call so the probes SKIP instead of erroring. A refusal
   *  thrown from the transport would land as `kind: 'errored'`, which by rule 2
   *  means "the deployment failed" — and a perfectly healthy persona is not a
   *  broken deployment. `estimateProbes` reads the same edge, so the priced call
   *  count matches the calls a run actually makes. */
  offersToolDefinitions: () => Promise<boolean>
  /** THE CHANNEL THAT IS STILL SHUT, and deliberately so. `Message.content` is a
   *  string by construction (see the note on `Message` in harness/define.ts):
   *  widening it to the OpenAI content-parts union is a tree-wide change —
   *  `completeViaGateway`'s signature, the grounding text and tool record the
   *  guard pass is built from, both token estimates, 23 harness renders — and a
   *  union only some transports honored would meter `[object Object]` and ground
   *  the guard against nothing. A half-widened content type is worse than none,
   *  so `vision` skips and says which wall it hit.
   *
   *  Note that the probe is gated behind an endpoint that DECLARES vision, and
   *  nothing in Talaria writes that fact yet — so opening this channel alone
   *  would still run zero image calls. */
  askWithImages: ((spec: ImageAskSpec) => Promise<Attempt>) | null
  /** The advertised context window of the endpoint serving this model, or null
   *  when nothing advertises one. */
  contextWindow: () => Promise<number | null>
  /** Does anything DECLARE this capability for the model? Only the vision probe
   *  asks, and only so it can skip cleanly on an endpoint that never claimed it.
   *  A `declared` fact is the advertisement; the probe is the verification. */
  advertises: (cap: Capability) => Promise<boolean>
  /** Fetch a cited page as text for the search probe's quote check. Null (a
   *  failed fetch, a 403, a timeout) makes the trial inconclusive, never failed. */
  fetchText: (url: string) => Promise<string | null>
  record: (key: CapabilityKey, cap: Capability, fact: CapabilityFact) => Promise<void>
  /** THE FACT WE ALREADY MEASURED for this capability, or null.
   *
   *  Only a fact whose `source` is `probe` counts. A `declared`, `catalog` or
   *  `learned` fact is a CLAIM or an inference, and the whole job of tier 1 is to
   *  verify those — treating one as "already measured" would mean a model
   *  catalog's marketing copy could permanently prevent us from checking it.
   *
   *  Null for an ambiguous or unroutable id, because nothing was ever written
   *  under a key for it (`runProbes`'s ambiguity rule), so there is nothing to
   *  reuse and every probe runs. */
  measured: (cap: Capability) => Promise<CapabilityFact | null>
  now: () => number
  maxContextTokens: number
  /** The needle. An argument so a test is not at the mercy of a random value. */
  needleToken: string
  /** Prices for the estimate, $/MTok. Null when nothing prices this model. */
  price: () => Promise<{ in: number; out: number } | null>
}

// ── The default `ask`: runHarness with the candidate pinned ──────────────────

function probeDef<O>(
  id: string,
  messages: Message[],
  output: HarnessDefinition<null, O>['output'],
  tools: ToolSpec[] = [],
): HarnessDefinition<null, O> {
  return {
    id: `fitness:probe:${id}`,
    label: `capability probe (${id})`,
    job: 'measure one model capability against a fixed prompt',
    requires: [],
    // A PROBE NEVER REFUSES THE MODEL IT IS MEASURING. An empty floor with
    // `refuseBelow: false` is the only correct declaration here — the whole
    // purpose of the run is to produce the fact a floor would consult.
    floor: { capabilities: [], refuseBelow: false, note: 'A probe measures; it does not refuse.' },
    // Never consulted: `runProbes` always pins `ctx.model`. `[]` is the runner's
    // declared way of saying "the model comes from the caller" and fails loudly
    // rather than silently probing the org's utility model.
    model: { chain: [] },
    render: () => messages,
    output,
    onFailure: 'null',
    // THE PROBE'S OWN PATIENCE, ON THE SOCKET. `runProbes` races every probe
    // against `PROBE_TIMEOUT_MS` and moves on; without this the abandoned HTTP
    // request kept running to the gateway's ten-minute default, holding a
    // connection nobody was waiting for. Eight candidate sweeps doing that at
    // once is how a healthy provider starts queueing and every later call blows
    // its budget too — see `UpstreamCall.signal`.
    holdMs: PROBE_TIMEOUT_MS,
    // No rule in the registry is meaningful about a probe reply, and a probe
    // must not add rows to the guard statistics the fitness page reads as a
    // per-model confabulation rate.
    guard: { rules: [] },
    temperature: 0,
    ...(tools.length ? { toolDefs: tools } : {}),
  }
}

/** The four injected edges every probe run shares, so `runnerAsk` and
 *  `runnerToolAsk` cannot drift on any of them — each one is load-bearing and
 *  the header says why. */
function probeRunDeps(transport: Transport): Partial<HarnessDeps> {
  return {
    transport,
    // See the header: a probe measures the model, not the record.
    missingCapabilities: async (): Promise<Capability[]> => [],
    capabilities: async (): Promise<Record<string, never>> => ({}),
    routing: async (m: string): Promise<{ endpoints: string[]; upstreamModel: string }> => ({ endpoints: [], upstreamModel: m }),
    personaKeys: async (): Promise<string[]> => [],
    // NO GUARD PASS ON A PROBE, declared at both ends: `guard: { rules: [] }`
    // narrows the registry to nothing, and this makes the run independent of
    // the org's guard settings entirely. A probe measures a capability; it is
    // not evidence about how the model behaves on real work, and the org's
    // confabulation statistics must not move because an admin benchmarked a
    // model. It also keeps a probe run free of a settings read per call.
    guardConfig: async (): Promise<GuardConfig> => ({ mode: 'off', checks: {}, minConfidence: 1, policedHosts: [], coach: false }),
    guardText: async (): Promise<never[]> => [],
    recordFindings: async (): Promise<void> => {},
    recordRun: async (): Promise<void> => {},
  }
}

/** THE IMAGE CHANNEL, OPENED — without widening `Message`.
 *
 *  The argument for leaving `vision` unmeasured was sound about the thing it was
 *  arguing against: turning `Message.content` into a content-parts union is a
 *  tree-wide change (26 harness renders, both token estimates, the grounding
 *  text the guard pass is built from), and a union only some transports honored
 *  would meter `[object Object]` and ground the guard against nothing.
 *
 *  BUT MEASURING A MODEL DOES NOT REQUIRE THE HARNESS TREE TO CARRY IMAGES. This
 *  builds the multimodal body itself and hands it to the gateway plumbing
 *  `completeViaGateway` uses — `buildUpstream` for the endpoint's key, defaults
 *  and learned-parameter stripping, `fetchUpstream` for the call. No shared type
 *  changes, so there is no half-widened union to go wrong.
 *
 *  WHAT THE TAG THEN MEANS, precisely: this model reads images. It does NOT mean
 *  a harness can send one yet — that still needs the widening above. A capability
 *  is a fact about the model, and refusing to record one Talaria cannot yet spend
 *  is how `vision` stayed blank on models that have had it for years.
 *
 *  GATEWAY-SERVED CANDIDATES ONLY. A fleet persona takes a rendered turn through
 *  its own container and has no raw-body seam, exactly as with the tool probes —
 *  so it skips there, and a skip is never a `false`. */
/** The image ask, delegated to the transport layer.
 *
 *  BOTH BRANCHES LIVE IN `transport.ts` (`gatewayImageTurn`, `personaProbeTurn`)
 *  because both build a raw upstream body, and raw-body construction is the
 *  transport's job — `gatewayToolTurn` is the precedent right beside them. Doing
 *  it here tripped `hand-written-harness`, and the rule was right: a model call
 *  assembled outside the transport layer is exactly the thing that grew six JSON
 *  extractors and three unguarded paths the last time.
 *
 *  A PERSONA HAS A SEAM AFTER ALL, and the old skip was written before anyone
 *  looked for it: `proxyChat` forwards its payload to the agent's own
 *  `/v1/chat/completions`, and `ChatPayload.content` has always accepted OpenAI
 *  content parts "passed through untouched". What was missing was never the
 *  fleet path — it was `Message.content` being a string, so a `TransportRequest`
 *  had no way to say "attach this image". */
export function runnerImageAsk(model: string): (spec: ImageAskSpec) => Promise<Attempt> {
  return async (spec) => {
    const blank: Attempt = { raw: '', transportError: null, jsonRequested: false, contractDropped: false, contractHeld: false }
    try {
      const caller = `fitness:probe:${spec.id}`
      const persona = !(await offersToolDefinitions(model).catch(() => false))
      const text = persona
        ? (await personaProbeTurn(model, spec.messages, { images: spec.images, caller })).text
        : await gatewayImageTurn(model, spec.messages, spec.images, caller, { timeoutMs: PROBE_TIMEOUT_MS })
      return { ...blank, raw: text }
    } catch (err) {
      return { ...blank, transportError: messageOf(err) }
    }
  }
}

export function runnerAsk(model: string, base: Transport = defaultTransport): (spec: AskSpec) => Promise<Attempt> {
  return async (spec) => {
    const seen = { jsonRequested: false, contractDropped: false, threw: null as string | null }
    const transport: Transport = async (req) => {
      seen.jsonRequested = seen.jsonRequested || req.jsonMode
      try {
        const reply = await base(req)
        if (reply.contractDropped) seen.contractDropped = true
        return reply
      } catch (err) {
        // WATCHED HERE, NOT PARSED OUT OF AN ERROR STRING. `answered: false`
        // covers both "the transport threw" and "the model returned an empty
        // reply", and only the first of those must void the probe (rule 2).
        seen.threw = messageOf(err)
        throw err
      }
    }
    const ctx = { caller: `fitness:probe:${spec.id}`, model, deps: probeRunDeps(transport) }
    const result = spec.schema
      ? await runHarness(probeDef(spec.id, spec.messages, { kind: 'json' as const, schema: spec.schema, repair: 0 }), null, ctx)
      : await runHarness(probeDef<string>(spec.id, spec.messages, { kind: 'text' as const }), null, ctx)
    return {
      raw: result.raw ?? '',
      transportError: seen.threw,
      jsonRequested: seen.jsonRequested,
      contractDropped: seen.contractDropped,
      contractHeld: result.schemaValid,
    }
  }
}

/** The tool-offering call, through the same runner and the same transport rule.
 *
 *  There is no contract to hold here and that is deliberate: the whole
 *  observation lives in `TransportReply.toolCalls`, and a model that calls the
 *  right tool typically returns EMPTY content, which every text contract in the
 *  tree reads as a failure. So the probe def is a plain text harness with
 *  `onFailure: 'null'`, the run's value is ignored, and the trial is graded by
 *  `toolCallProblem` over what the transport reported. */
export function runnerToolAsk(model: string, base: Transport = defaultTransport): (spec: ToolAskSpec) => Promise<ToolAttempt> {
  return async (spec) => {
    const seen = { calls: [] as ProbeToolCall[], threw: null as string | null }
    const transport: Transport = async (req) => {
      try {
        const reply = await base(req)
        // ABSENT IS NOT EMPTY (`TransportReply.toolCalls`). A transport that
        // never ran the loop reports undefined, and reading that as "called
        // nothing" would write `tools: false` — forever — about a model that was
        // never offered a tool. `pickTransport` cannot produce it (the fleet
        // path refuses a request carrying `toolDefs` rather than answering it),
        // so this is the belt on a bespoke `base` handed in by a caller.
        if (!reply.toolCalls) {
          throw new Error(`the transport for "${model}" answered a tool-definition request without reporting any tool calls`)
        }
        seen.calls = reply.toolCalls
        return reply
      } catch (err) {
        seen.threw = messageOf(err)
        throw err
      }
    }
    const ctx = { caller: `fitness:probe:${spec.id}`, model, deps: probeRunDeps(transport) }
    await runHarness(probeDef<string>(spec.id, spec.messages, { kind: 'text' as const }, spec.tools), null, ctx)
    return { toolCalls: seen.calls, transportError: seen.threw }
  }
}

// ── Default edges that read the deployment ───────────────────────────────────

/** Every endpoint that could serve this model, for the window and the price.
 *  Deliberately the same derivation `run.ts` uses for capability keys. */
async function endpointsFor(model: string): Promise<Array<{ name: string; contextLength: number | null; price: { in: number; out: number } | null }>> {
  const route = await routingFor(model).catch(() => null)
  if (!route || route.endpoints.length === 0) return []
  return route.endpoints.map((ep) => {
    const over = ep.modelPrices?.[route.upstreamModel]
    const auto = ep.autoPrices?.[route.upstreamModel]
    const inTok = over?.in ?? auto?.in ?? ep.priceInPerMtok
    const outTok = over?.out ?? auto?.out ?? ep.priceOutPerMtok
    return {
      name: ep.name,
      contextLength: ep.contextLength,
      price: typeof inTok === 'number' && typeof outTok === 'number' ? { in: inTok, out: outTok } : null,
    }
  })
}

/** THE SMALLEST advertised window in the pool, not the largest. A bare model id
 *  can land on any member, so a claim has to hold for the worst of them.
 *
 *  THE MODEL'S OWN NUMBER FIRST, and this is the fix for a probe that used to
 *  skip on models the provider describes in full. `llm_endpoints.context_length`
 *  is ONE integer per endpoint — a single number for an OpenRouter row serving
 *  four hundred models with windows from 4k to 1M. It is written only by
 *  `fleet-federate.ts`, and `ensureEndpoint`'s `on conflict do update` does not
 *  refresh it, so on a normal install it is null and the long-context probe
 *  skipped with "nothing advertises a context window for this model" about
 *  models whose catalog entry says 1,048,576.
 *
 *  The endpoint row stays as the FALLBACK rather than being deleted: a federated
 *  fleet writes it and publishes no catalog, so for those deployments it is the
 *  only number there is. */
async function smallestWindow(model: string): Promise<number | null> {
  const advertised = await advertisedWindow(model).catch(() => null)
  if (advertised !== null) return advertised
  const eps = await endpointsFor(model)
  const windows = eps.map((e) => e.contextLength).filter((n): n is number => typeof n === 'number' && n > 0)
  if (windows.length === 0) {
    // Not a gateway model, and no catalog entry: a fleet persona records its
    // window on the agent's config, not on an endpoint row, and nothing here
    // can read it honestly.
    return null
  }
  return Math.min(...windows)
}

async function priceFor(model: string): Promise<{ in: number; out: number } | null> {
  const eps = await endpointsFor(model)
  const priced = eps.map((e) => e.price).filter((p): p is { in: number; out: number } => p !== null)
  if (priced.length === 0) return null
  // The DEAREST member, for the same reason as the window: an estimate that
  // could be exceeded by the endpoint the round-robin happens to pick is not an
  // estimate an admin can act on.
  return priced.reduce((a, b) => (a.in + a.out >= b.in + b.out ? a : b))
}

/** A cited page, as text. Never throws, never blocks for long, and answers null
 *  for anything that is not a plainly readable 2xx — every one of which makes
 *  the trial inconclusive rather than failed. The URL is MODEL-SUPPLIED (it
 *  rides in a probe reply), so this goes through safeFetch like every other
 *  agent-influenced fetch — a citation pointing at the metadata service or an
 *  internal host is refused, not fetched. A refusal is indistinguishable from
 *  an unreachable page: the trial reads inconclusive, which is the honest
 *  verdict for a citation we would not follow in production either. */
async function fetchCitedPage(url: string): Promise<string | null> {
  try {
    const res = await safeFetch(url, { timeoutMs: 10_000, maxBytes: 400_000 })
    if (!res.ok) return null
    const body = await res.text()
    return body.length > 0 ? body : null
  } catch {
    return null
  }
}

export function defaultDeps(model: string, over: Partial<ProbeDeps> = {}): ProbeDeps {
  return {
    ask: runnerAsk(model),
    askWithTools: runnerToolAsk(model),
    // Asked of the TRANSPORT RULE rather than answered here, so this cannot
    // disagree with the transport that would refuse the call.
    offersToolDefinitions: () => offersToolDefinitions(model),
    // Gateway-served candidates only — a persona has no raw-body seam, the same
    // wall the tool probes hit.
    askWithImages: runnerImageAsk(model),
    contextWindow: () => smallestWindow(model),
    advertises: async (cap) => {
      // A `declared` fact IS the advertisement — capability.ts's third source is
      // "an admin (or a model catalog) writes declared", and nothing else in
      // Talaria records which modalities an endpoint serves. The probe then
      // VERIFIES the advertisement, and a probe fact overrides a declared one on
      // the same key, which is the correct direction: a measurement beats a
      // claim. Nothing declared means nothing to verify, so the probe skips.
      const keys = await probeKeys(model)
      const none: Partial<Record<Capability, CapabilityFact>> = {}
      const facts = await Promise.all(keys.map((k) => getCapabilities(k).catch(() => none)))
      return facts.some((f) => f[cap]?.value === true && f[cap]?.source === 'declared')
    },
    fetchText: fetchCitedPage,
    record: recordCapability,
    measured: async (cap) => {
      // ONE key or none. `runProbes` refuses to write facts for a pooled id, so
      // a pooled id has none to reuse and every probe runs — which is the right
      // answer twice over: nothing was recorded, and what one pool member can do
      // is not what another can.
      const keys = await probeKeys(model)
      if (keys.length !== 1) return null
      const facts = await getCapabilities(keys[0]!).catch(() => ({}) as Partial<Record<Capability, CapabilityFact>>)
      const fact = facts[cap]
      return fact?.source === 'probe' ? fact : null
    },
    now: () => Date.now(),
    maxContextTokens: DEFAULT_MAX_CONTEXT_TOKENS,
    needleToken: 'GRANITE-FOX-7731',
    price: () => priceFor(model),
    ...over,
  }
}

// ── The tool fixtures, and the one image fixture still waiting ───────────────
//
// The tool fixtures are LIVE: `TransportRequest.toolDefs` carries them to the
// model and `TransportReply.toolCalls` brings back what was called, so both tool
// probes now make real calls on any gateway-served candidate. The vision fixture
// below is still parked behind `Message.content` being a string — see
// `ProbeDeps.askWithImages` for why widening it is a change of its own.

const WEATHER_TOOL: ToolSpec = {
  name: 'get_weather',
  description: 'Look up the current weather for a city. The only way to know current weather.',
  parameters: { type: 'object', properties: { city: { type: 'string', description: 'City name' } }, required: ['city'] },
}

const TOOL_TRIAL: { messages: Message[] } = { messages: [TERSE, usr('What is the weather in Lisbon right now?')] }

/** Four tools with four clearly disjoint jobs. Disjoint on purpose: a
 *  tool-selection score is only meaningful when a wrong pick is unambiguously
 *  wrong, and two plausibly-overlapping tools would measure our fixture design
 *  rather than the model. */
const SELECT_TOOLS: ToolSpec[] = [
  { name: 'get_weather', description: 'Current weather for a city.', parameters: { type: 'object', properties: { city: { type: 'string' } }, required: ['city'] } },
  { name: 'send_email', description: 'Send an email to a recipient.', parameters: { type: 'object', properties: { to: { type: 'string' }, body: { type: 'string' } }, required: ['to', 'body'] } },
  { name: 'convert_currency', description: 'Convert an amount from one currency to another.', parameters: { type: 'object', properties: { amount: { type: 'number' }, from: { type: 'string' }, to: { type: 'string' } }, required: ['amount', 'from', 'to'] } },
  { name: 'create_ticket', description: 'Open a work ticket on a board.', parameters: { type: 'object', properties: { title: { type: 'string' } }, required: ['title'] } },
]

const TOOL_SELECT_TRIALS: ReadonlyArray<{ name: string; messages: Message[]; expect: string }> = [
  { name: 'weather', messages: [TERSE, usr('Is it raining in Porto at the moment?')], expect: 'get_weather' },
  { name: 'email', messages: [TERSE, usr('Let ana@example.org know the deploy is finished.')], expect: 'send_email' },
  { name: 'currency', messages: [TERSE, usr('How much is 240 euros in Japanese yen?')], expect: 'convert_currency' },
  { name: 'ticket', messages: [TERSE, usr('Open a task called "rotate the staging certificate".')], expect: 'create_ticket' },
]

/** A 1x1 solid red PNG, inline. A `data:` URL rather than a hosted image
 *  because a probe that depended on a host being up would fail as a network
 *  problem and be scored as a model that cannot see. */
/** An upstream saying the MODEL takes no images, as opposed to an upstream that
 *  is down. OpenRouter answers a text-only model with `404 No endpoints found
 *  that support image input`; that is the deployment telling us plainly what the
 *  model can be sent, not a failure to reach it.
 *
 *  Deliberately narrow. Anything this does not recognize stays an `errored`,
 *  which writes nothing — a wrong `vision: false` never expires. */
export const noImageInput = (err: string): boolean =>
  /no endpoints found that support image input|does not support image|image input is not supported|vision is not supported/i.test(err)

/** THREE OPAQUE 128x128 SOLID FIELDS, and the previous fixture is why the size
 *  and the opacity are both stated.
 *
 *  It was a SINGLE PIXEL at RGBA(255, 0, 0, 127) - 1x1, and half transparent.
 *  Nothing can be concluded from it: a one-pixel image carries less than one
 *  patch of any vision encoder, and a half-alpha red renders pink on a white
 *  matte and maroon on a black one, so the "right" answer depended on whichever
 *  background the provider happened to composite against. claude-haiku answered
 *  BLUE and would have been recorded `vision: false` - a false negative on a
 *  model that has read images for years, written into a record that does not
 *  expire. That is exactly the wrong fact rule 3 of this file exists to prevent,
 *  arriving from the fixture rather than from the scorer.
 *
 *  Three colours rather than one because a single trial cannot separate "reads
 *  images" from "guessed, and there was only one thing to guess". */
const VISION_TRIALS: ReadonlyArray<{ name: string; messages: Message[]; image: string; expect: string }> = [
  {
    name: 'solid red',
    messages: [TERSE, usr('What single color fills this image? Reply with one word: RED, GREEN or BLUE.')],
    image: 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAIAAAACACAIAAABMXPacAAAAyElEQVR42u3RQREAAAjDsCmZf1GIQQY8clcFTabVYbEAAAABACAAAAQAgAAAEAAAAgBAAAAIAAABACAAAAQAgAAAEAAAAgBAAAAIAAABACAAAAQAgAAAEAAAAgBAAAAIAAABACAAAAQAgAAAEAAAAgBAAAAAcAEAAAEAIAAABACAAAAQAAACAEAAAAgAAAEAIAAABACAAAAQAAACAEAAAAgAAAEAIAAABACAAAAQAAACAEAAAAgAAAEAIAAABACAAAAQAAAC8KEFIPUEG5PrRbsAAAAASUVORK5CYII=',
    expect: 'RED',
  },
  {
    name: 'solid green',
    messages: [TERSE, usr('What single color fills this image? Reply with one word: RED, GREEN or BLUE.')],
    image: 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAIAAAACACAIAAABMXPacAAAAyElEQVR42u3RQQ0AAAjEsFOCEhSjEhnwaDIFa2pah8UCAAAEAIAAABAAAAIAQAAACAAAAQAgAAAEAIAAABAAAAIAQAAACAAAAQAgAAAEAIAAABAAAAIAQAAACAAAAQAgAAAEAIAAABAAAAIAQAAACAAAAQAAwAUAAAQAgAAAEAAAAgBAAAAIAAABACAAAAQAgAAAEAAAAgBAAAAIAAABACAAAAQAgAAAEAAAAgBAAAAIAAABACAAAAQAgAAAEAAAAgBAAAAIwIcW6UkD0KHeGfUAAAAASUVORK5CYII=',
    expect: 'GREEN',
  },
  {
    name: 'solid blue',
    messages: [TERSE, usr('What single color fills this image? Reply with one word: RED, GREEN or BLUE.')],
    image: 'data:image/png;base64,iVBORw0KGgoAAAANSUhEUgAAAIAAAACACAIAAABMXPacAAAAyElEQVR42u3RQQ0AAAjEsJODEvwHRciAR5MpWFM9OiwWAAAgAAAEAIAAABAAAAIAQAAACAAAAQAgAAAEAIAAABAAAAIAQAAACAAAAQAgAAAEAIAAABAAAAIAQAAACAAAAQAgAAAEAIAAABAAAAIAQAAACAAAAC4AACAAAAQAgAAAEAAAAgBAAAAIAAABACAAAAQAgAAAEAAAAgBAAAAIAAABACAAAAQAgAAAEAAAAgBAAAAIAAABACAAAAQAgAAAEAAAAgBAAD60hHcEsZKvPusAAAAASUVORK5CYII=',
    expect: 'BLUE',
  },
]

// ── The probes ───────────────────────────────────────────────────────────────

export interface ProbeDefinition {
  id: ProbeId
  label: string
  /** One line an admin reads: what a pass here means for their install. */
  claim: string
  /** Calls this probe makes when it is not skipped, for the estimate. */
  calls: number
  /** Prompt tokens per call, from the fixtures. `long-context` overrides it at
   *  estimate time because its prompt is sized from the model's window. */
  promptTokens: number
  /** The most a probe reply is worth paying for. */
  completionTokens: number
  run: (deps: ProbeDeps) => Promise<ProbeOutcome>
}

const promptTokensOf = (messages: readonly Message[]): number => estimateTokens(messages.reduce((n, m) => n + m.content.length, 0))

/** A transport failure anywhere in a probe voids the whole probe (rule 2). */
const transportFailure = (attempts: readonly Attempt[]): string | null => attempts.find((a) => a.transportError)?.transportError ?? null

/** The one reason a tool probe still skips, in one place because both probes
 *  say it and `estimateProbes` has to predict it. A persona is not a broken
 *  deployment, so this is a skip rather than an error — and not a `false`,
 *  because nothing about the model was measured. */
const FLEET_TOOL_SKIP =
  'this candidate is a fleet persona: its tool loop runs inside the agent container, so Talaria cannot offer it tool ' +
  'definitions or see which one it called. Probe the gateway model behind the agent instead.'

const errored = (reason: string, trials: Trial[] = []): ProbeOutcome => ({ kind: 'errored', reason, trials })
const skipped = (reason: string, trials: Trial[] = []): ProbeOutcome => ({ kind: 'skipped', reason, trials })
const settle = (verdict: ProbeVerdict | null, trials: Trial[], why: string): ProbeOutcome =>
  verdict ? { kind: 'scored', verdict, trials } : skipped(why, trials)

export const PROBES: readonly ProbeDefinition[] = [
  {
    id: 'json',
    label: 'JSON mode',
    claim: 'The endpoint honors response_format and the model returns a parseable object.',
    calls: JSON_TRIALS.length,
    promptTokens: Math.max(...JSON_TRIALS.map((t) => promptTokensOf(t.messages))),
    completionTokens: 120,
    run: async (deps) => {
      const attempts: Attempt[] = []
      const trials: Trial[] = []
      for (const t of JSON_TRIALS) {
        const a = await deps.ask({ id: `json:${t.name}`, messages: t.messages, schema: JSON_TRIVIAL_SCHEMA })
        attempts.push(a)
        if (a.transportError) break
        trials.push({ name: t.name, ok: a.contractHeld, note: a.contractHeld ? 'returned a valid object' : 'the reply was not a valid object', raw: bounded(a.raw) })
      }
      const down = transportFailure(attempts)
      if (down) return errored(down, trials)
      const protocol = { requested: attempts.every((a) => a.jsonRequested), dropped: attempts.some((a) => a.contractDropped) }
      return settle(scoreJson(trials, protocol), trials, 'no JSON-mode call completed')
    },
  },
  {
    id: 'json-strict',
    label: 'Schema conformance',
    claim: 'Nested arrays and a long string field survive the model intact, first attempt.',
    calls: JSON_STRICT_TRIALS.length,
    promptTokens: Math.max(...JSON_STRICT_TRIALS.map((t) => promptTokensOf(t.messages))),
    completionTokens: 400,
    run: async (deps) => {
      const attempts: Attempt[] = []
      const trials: Trial[] = []
      for (const t of JSON_STRICT_TRIALS) {
        const a = await deps.ask({ id: `json-strict:${t.name}`, messages: t.messages, schema: JSON_STRICT_SCHEMA })
        attempts.push(a)
        if (a.transportError) break
        trials.push({ name: t.name, ok: a.contractHeld, note: a.contractHeld ? 'conformed' : 'the object did not match the schema', raw: bounded(a.raw) })
      }
      const down = transportFailure(attempts)
      if (down) return errored(down, trials)
      return settle(scoreJsonStrict(trials), trials, 'no schema call completed')
    },
  },
  {
    id: 'tools',
    label: 'Tool calling',
    claim: 'The model emits a well-formed tool call when the answer requires one.',
    calls: 1,
    promptTokens: promptTokensOf(TOOL_TRIAL.messages),
    completionTokens: 120,
    run: async (deps) => {
      const offers = await deps.offersToolDefinitions()
      if (!offers) return skipped(FLEET_TOOL_SKIP)
      const a = await deps.askWithTools({ id: 'tools', messages: TOOL_TRIAL.messages, tools: [WEATHER_TOOL] })
      if (a.transportError) return errored(a.transportError)
      const call = a.toolCalls[0]
      const problem = toolCallProblem(a.toolCalls, WEATHER_TOOL.name, ['city'])
      const trials: Trial[] = [{ name: 'calls the offered tool', ok: problem === null, note: problem ?? `called ${call?.name}`, raw: call ? bounded(`${call.name}(${call.args})`) : null }]
      return settle(scoreTools(trials), trials, 'the tool call never completed')
    },
  },
  {
    id: 'tool-select',
    label: 'Tool selection',
    claim: 'Given four tools, the model picks the right one every time. This is what widens the Inbox.',
    calls: TOOL_SELECT_TRIALS.length,
    promptTokens: Math.max(...TOOL_SELECT_TRIALS.map((t) => promptTokensOf(t.messages))),
    completionTokens: 120,
    run: async (deps) => {
      const offers = await deps.offersToolDefinitions()
      if (!offers) return skipped(FLEET_TOOL_SKIP)
      const trials: Trial[] = []
      for (const t of TOOL_SELECT_TRIALS) {
        const a = await deps.askWithTools({ id: `tool-select:${t.name}`, messages: t.messages, tools: SELECT_TOOLS })
        if (a.transportError) return errored(a.transportError, trials)
        const problem = toolCallProblem(a.toolCalls, t.expect, [])
        const call = a.toolCalls[0]
        trials.push({ name: t.name, ok: problem === null, note: problem ?? `picked ${t.expect}`, raw: call ? bounded(`${call.name}(${call.args})`) : null })
      }
      return settle(scoreToolSelect(trials), trials, 'no tool-selection call completed')
    },
  },
  {
    id: 'instruction-following',
    label: 'Exact instructions',
    claim: '"Reply with exactly the word OK" produces exactly OK. Every text harness depends on this.',
    calls: INSTRUCTION_TRIALS.length,
    promptTokens: Math.max(...INSTRUCTION_TRIALS.map((t) => promptTokensOf(t.messages))),
    completionTokens: 40,
    run: async (deps) => {
      const attempts: Attempt[] = []
      const trials: Trial[] = []
      for (const t of INSTRUCTION_TRIALS) {
        const a = await deps.ask({ id: `instruction:${t.name}`, messages: t.messages })
        attempts.push(a)
        if (a.transportError) break
        const ok = a.raw.trim() === t.expect
        trials.push({ name: t.name, ok, note: ok ? 'exact' : `answered ${JSON.stringify(a.raw.trim().slice(0, 60))} instead of ${JSON.stringify(t.expect)}`, raw: bounded(a.raw) })
      }
      const down = transportFailure(attempts)
      if (down) return errored(down, trials)
      return settle(scoreInstruction(trials), trials, 'no instruction call completed')
    },
  },
  {
    id: 'search',
    label: 'Live web search',
    claim: 'The model can open a page today and quote it verbatim. Without this, research invents citations.',
    calls: SEARCH_TRIALS.length,
    promptTokens: Math.max(...SEARCH_TRIALS.map((t) => promptTokensOf(t.messages))),
    completionTokens: 250,
    run: async (deps) => {
      const attempts: Attempt[] = []
      const trials: Trial[] = []
      for (const t of SEARCH_TRIALS) {
        const a = await deps.ask({ id: `search:${t.name}`, messages: t.messages, schema: SEARCH_SCHEMA })
        attempts.push(a)
        if (a.transportError) break
        trials.push(...(await searchTrials(t.name, a, deps)))
      }
      const down = transportFailure(attempts)
      if (down) return errored(down, trials)
      return settle(scoreSearch(trials), trials, 'the cited pages could not be verified - nothing was learned either way')
    },
  },
  {
    id: 'long-context',
    label: 'Long context',
    claim: 'A fact planted at 50% and 90% of the window is still there when asked for.',
    calls: 2,
    promptTokens: DEFAULT_MAX_CONTEXT_TOKENS,
    completionTokens: 40,
    run: async (deps) => {
      // A WINDOW NOBODY ADVERTISES IS NOT A REASON NOT TO LOOK.
      //
      // This used to skip outright, and on the Anthropic endpoint it skipped
      // every time: Anthropic's /v1/models returns an id and a display name and
      // nothing else, so `advertisedWindow` is null for every Claude model. The
      // result was a permanent "nothing advertises a context window" on models
      // with some of the largest windows in the industry — a gap in the capability
      // matrix caused entirely by a provider's terse catalog.
      //
      // Nothing here is allowed to know Claude's window (see the note on
      // provider lists: catalogs are fetched, never hardcoded), so the honest
      // move is to MEASURE instead of guess. Absent an advertisement the probe
      // tests its own default ceiling, and the verdict says the window was
      // assumed. A model that cannot hold it fails the needle, or the upstream
      // rejects the request and that is recorded as an error — both are findings.
      // Skipping produced neither.
      const advertised = await deps.contextWindow().catch(() => null)
      const tested = advertised ? Math.min(advertised, deps.maxContextTokens) : deps.maxContextTokens
      if (tested < MIN_LONG_CONTEXT_TOKENS) {
        return skipped(`the tested window would be ${tested.toLocaleString('en-US')} tokens, below the ${MIN_LONG_CONTEXT_TOKENS.toLocaleString('en-US')} this probe considers long`)
      }
      // 80% of the window for the prompt: a needle "at 90% depth" means 90% of
      // the way through the text we sent, and sending a prompt that fills the
      // window leaves the model no room to answer in.
      const budget = Math.floor(tested * 0.8)
      const attempts: Attempt[] = []
      const trials: Trial[] = []
      for (const depth of [0.5, 0.9]) {
        const text = haystack(budget, needleLine(deps.needleToken), depth)
        const a = await deps.ask({ id: `long-context:${depth}`, messages: [TERSE, usr(`${text}\n\n${NEEDLE_QUESTION}`)] })
        attempts.push(a)
        if (a.transportError) break
        const ok = a.raw.toLowerCase().includes(deps.needleToken.toLowerCase())
        trials.push({ name: `needle at ${Math.round(depth * 100)}%`, ok, note: ok ? 'found' : 'the passphrase was not in the reply', raw: bounded(a.raw) })
      }
      const down = transportFailure(attempts)
      if (down) return errored(down, trials)
      return settle(scoreLongContext(trials, budget, advertised === null), trials, 'no long-context call completed')
    },
  },
  {
    id: 'code',
    label: 'Code',
    claim: 'A small function with a precise contract passes its assertions when run.',
    calls: CODE_TASKS.length,
    promptTokens: Math.max(...CODE_TASKS.map((t) => promptTokensOf([TERSE, usr(t.prompt)]))),
    completionTokens: 400,
    run: async (deps) => {
      const attempts: Attempt[] = []
      const trials: Trial[] = []
      for (const task of CODE_TASKS) {
        const a = await deps.ask({ id: `code:${task.name}`, messages: [TERSE, usr(task.prompt)] })
        attempts.push(a)
        if (a.transportError) break
        const problem = runCodeTask(task, a.raw)
        trials.push({ name: task.name, ok: problem === null, note: problem ?? 'passed every assertion', raw: bounded(a.raw) })
      }
      const down = transportFailure(attempts)
      if (down) return errored(down, trials)
      return settle(scoreCode(trials), trials, 'no code call completed')
    },
  },
  {
    id: 'vision',
    label: 'Vision',
    claim: 'The model reads an image it was given.',
    calls: VISION_TRIALS.length,
    promptTokens: 400,
    completionTokens: 40,
    run: async (deps) => {
      // THE STRUCTURAL BLOCKER IS CHECKED FIRST, and the catalog gate is gone.
      //
      // "This endpoint does not advertise vision" was the reason shown for every
      // Claude model — which read as a fact about Claude and is not one; it is a
      // fact about a catalog that publishes no modalities. Worse, it hid the
      // REAL blocker behind it, so the one thing an admin could act on was
      // invisible. A catalog that does advertise vision is a reason to believe
      // the probe will pass, never a precondition for running it.
      if (!deps.askWithImages) {
        return skipped(
          'Talaria cannot put image parts in a harness turn: Message.content is a string across the whole tree, and widening it ' +
            'to OpenAI content parts is a change every transport, both token estimates and the guard pass have to make together. ' +
            'Vision is unmeasured here - not absent. See ProbeDeps.askWithImages.',
        )
      }
      const trials: Trial[] = []
      for (const t of VISION_TRIALS) {
        const a = await deps.askWithImages({ id: `vision:${t.name}`, messages: t.messages, images: [t.image] })
        // A REFUSAL OF THE IMAGE ITSELF IS AN ANSWER. OpenRouter replies to a
        // text-only model with `404 No endpoints found that support image
        // input` — which is not a broken deployment, it is the deployment
        // telling us plainly that this model cannot be sent an image. Rule 2
        // sends errors to `errored` (writes nothing) because they are facts
        // about the gateway; this one is a fact about the model on this
        // endpoint, which is exactly what a capability key addresses.
        if (a.transportError && noImageInput(a.transportError)) {
          return settle({ value: false, score: 0, detail: `this endpoint serves no provider that accepts image input for this model` }, trials, '')
        }
        if (a.transportError) return errored(a.transportError, trials)
        const ok = a.raw.trim().toUpperCase().includes(t.expect)
        trials.push({ name: t.name, ok, note: ok ? 'read correctly' : `answered ${JSON.stringify(a.raw.trim().slice(0, 60))}`, raw: bounded(a.raw) })
      }
      return settle(scoreVision(trials), trials, 'no image call completed')
    },
  },
]

/** What is wrong with the model's tool calls, or null. Exported for the scorer
 *  tests, which drive it with replies recorded from real providers. */
export function toolCallProblem(calls: readonly ProbeToolCall[], expected: string, requiredArgs: readonly string[]): string | null {
  if (calls.length === 0) return 'the model answered in prose instead of calling a tool'
  const call = calls[0]
  if (!call) return 'the model answered in prose instead of calling a tool'
  if (calls.length > 1) return `called ${calls.length} tools when one was needed (${calls.map((c) => c.name).join(', ')})`
  if (call.name !== expected) return `called ${call.name} instead of ${expected}`
  if (requiredArgs.length === 0) return null
  let args: unknown
  try {
    args = JSON.parse(call.args || '{}')
  } catch {
    return `called ${call.name} with arguments that are not JSON`
  }
  if (!args || typeof args !== 'object' || Array.isArray(args)) return `called ${call.name} with arguments that are not an object`
  const missing = requiredArgs.filter((k) => !(k in (args as Record<string, unknown>)))
  return missing.length ? `called ${call.name} without ${missing.join(', ')}` : null
}

/** The two graded observations one search reply produces: the date (checkable
 *  against our own clock, and the ONLY thing allowed to write `search: false`)
 *  and the citation (checkable only if the host lets us read the page). */
async function searchTrials(name: string, a: Attempt, deps: ProbeDeps): Promise<Trial[]> {
  const raw = bounded(a.raw)
  const malformed: Trial[] = [{ name: `${name} / citation`, ok: false, note: 'the reply was not the requested JSON object', raw }]
  if (!a.contractHeld) return malformed
  const parsed = readSearchReply(a.raw)
  if (!parsed) return malformed
  const { date, url, quote } = parsed
  const drift = dateDriftDays(date, deps.now())
  const dateOk = drift !== null && drift <= 1
  const trials: Trial[] = [
    { name: `${name} / ${SEARCH_DATE_TRIAL}`, ok: dateOk, note: dateOk ? "named today's date" : `said today is ${date}`, raw },
  ]
  const citation = citationProblem(url)
  if (citation) {
    trials.push({ name: `${name} / citation`, ok: false, note: citation, raw })
    return trials
  }
  const page = await deps.fetchText(url).catch(() => null)
  if (page === null) {
    // INCONCLUSIVE, NOT FAILED. A live news site answering our bare GET with a
    // 403 says nothing about the model, and writing `search: false` from it
    // would refuse research to a working search model permanently.
    trials.push({ name: `${name} / citation`, ok: null, note: `could not read ${url} to check the quote`, raw })
    return trials
  }
  const found = quoteAppears(quote, page)
  trials.push({ name: `${name} / citation`, ok: found, note: found ? `quote verified on ${url}` : `the quoted sentence is not on ${url}`, raw })
  return trials
}

/** The object out of a search reply.
 *
 *  A second read of text the runner already parsed and validated — `Attempt` is
 *  deliberately a flat record of what was OBSERVED rather than a typed value, so
 *  that every scorer here can be driven from a recorded string in a test with no
 *  runner anywhere. The cost is this one re-parse, on one probe. */
export function readSearchReply(text: string): { date: string; url: string; quote: string } | null {
  const start = text.indexOf('{')
  const end = text.lastIndexOf('}')
  if (start < 0 || end <= start) return null
  try {
    const parsed = SEARCH_SCHEMA.safeParse(JSON.parse(text.slice(start, end + 1)))
    return parsed.success ? parsed.data : null
  } catch {
    return null
  }
}

// ── Capability keys: where a fact is allowed to land ─────────────────────────

/** The keys this candidate's facts belong under — the same derivation `run.ts`
 *  uses, gateway routing first and a fleet persona's backing model second. */
export async function probeKeys(model: string): Promise<CapabilityKey[]> {
  const route = await routingFor(model).catch(() => null)
  if (route && route.endpoints.length > 0) return route.endpoints.map((ep) => capabilityKey(ep.name, route.upstreamModel))
  return personaCapabilityKeys(model).catch(() => [])
}

// ── The estimate, as data ────────────────────────────────────────────────────

export interface ProbeEstimateRow {
  id: ProbeId
  calls: number
  promptTokens: number
  completionTokens: number
  /** Zero calls because we ALREADY MEASURED IT, not because it cannot run. The
   *  two are both free and mean opposite things to an admin. */
  known: boolean
}

export interface ProbeEstimate {
  model: string
  rows: ProbeEstimateRow[]
  calls: number
  promptTokens: number
  completionTokens: number
  /** Null when nothing prices this model — the UI shows tokens and call count,
   *  which are the numbers that do not depend on a catalog being reachable. */
  usd: number | null
  /** Probes this run will REUSE rather than re-measure. Reported so the price
   *  line can say why a probes tier costs less than the last one did. */
  known: number
}

/** What a run will cost BEFORE it starts. Returned as data — nothing here
 *  prints, and the admin UI is what turns it into a sentence. */
export async function estimateProbes(model: string, opts: { ids?: ProbeId[]; deps?: Partial<ProbeDeps>; reprobe?: boolean } = {}): Promise<ProbeEstimate> {
  const deps = defaultDeps(model, opts.deps)
  const chosen = PROBES.filter((p) => !opts.ids || opts.ids.includes(p.id))
  const window = await deps.contextWindow().catch(() => null)
  // A PROBE THAT WILL SKIP COSTS NOTHING, and the estimate has to say so.
  // Charging for the six calls of three probes that cannot run made the one
  // number an admin decides on before spending money overstate a probes-only run
  // by a fifth. Read off the same deps the run itself will read, so the estimate
  // cannot claim a probe will happen that `runProbes` then skips — and, now that
  // the tool probes are armed, cannot claim they will skip when they will
  // actually make five calls.
  const offersTools = await deps.offersToolDefinitions().catch(() => false)
  const willSkip = async (id: ProbeId): Promise<boolean> => {
    // THE SAME EDGE THE PROBE ASKS, not a copy of its reasoning: the tool probes
    // skip on a fleet persona and run on a gateway model, and this is billed off
    // that answer rather than off a guess about the deployment.
    if (id === 'tools' || id === 'tool-select') return !offersTools
    if (id === 'vision') return deps.askWithImages === null
    // Sized from the model's own window when one is advertised, and from the
    // probe's own ceiling when none is — it runs either way now, so the only
    // skip left is a window too small to call long.
    if (id === 'long-context') return Math.min(window ?? deps.maxContextTokens, deps.maxContextTokens) < MIN_LONG_CONTEXT_TOKENS
    return false
  }
  const rows: ProbeEstimateRow[] = []
  for (const p of chosen) {
    // ALREADY MEASURED COSTS NOTHING EITHER, and it is the commonest reason a
    // probes tier is cheap: a model tested last month re-tested this month pays
    // for the harnesses and nothing else.
    const known = !opts.reprobe && (await deps.measured(p.id).catch(() => null)) !== null
    const skip = known || (await willSkip(p.id))
    // The one probe whose prompt is not a fixture: it is sized from the model's
    // own window, so estimating it from the fixture would understate the run by
    // whatever the window happens to be.
    const promptTokens =
      p.id === 'long-context' ? Math.floor(Math.min(window ?? deps.maxContextTokens, deps.maxContextTokens) * 0.8) : p.promptTokens
    rows.push({ id: p.id, calls: skip ? 0 : p.calls, promptTokens, completionTokens: p.completionTokens, known })
  }
  const price = await deps.price().catch(() => null)
  const promptTokens = rows.reduce((n, r) => n + r.calls * r.promptTokens, 0)
  const completionTokens = rows.reduce((n, r) => n + r.calls * r.completionTokens, 0)
  return {
    model,
    rows,
    calls: rows.reduce((n, r) => n + r.calls, 0),
    promptTokens,
    completionTokens,
    usd: price ? (promptTokens * price.in + completionTokens * price.out) / 1e6 : null,
    known: rows.filter((r) => r.known).length,
  }
}

// ── Latency and cost: the EXISTING ring, not a second clock ──────────────────

export interface LatencyReading extends GatewayPulse {
  /** What the probe run itself is expected to cost, so latency and price sit on
   *  one object in the admin UI. Null when nothing prices the model. */
  usd: number | null
}

/** Read straight off `gatewayPulse()` — the 500-entry TTFB ring `llm-gateway.ts`
 *  already keeps. A probe suite that timed its own calls would report a
 *  different p50 than /observability for the same model, and the one nobody
 *  could reconcile would be this one. */
export const latencyReading = (usd: number | null): LatencyReading => ({ ...gatewayPulse(), usd })

// ── The driver ───────────────────────────────────────────────────────────────

export interface ProbeResult {
  id: ProbeId
  label: string
  outcome: ProbeOutcome
}

/** ONE PROBE, AS A CONSOLE LINE. The vocabulary is the terminal's, so a probe
 *  and a fixture colour the same way and a watcher does not have to learn two.
 *
 *  `known` is a SKIP rather than a pass: no call was made, so nothing was
 *  measured on this run, and painting it green would tell a watcher the model
 *  just demonstrated something it did not. */
export function probeLine(r: ProbeResult, ms: number): EvalLogLine {
  const o = r.outcome
  const verdict: EvalLogLine['verdict'] =
    o.kind === 'skipped' ? 'skip' : o.kind === 'known' ? 'skip' : o.kind === 'errored' ? 'error' : o.verdict.value ? 'pass' : 'fail'
  const note =
    o.kind === 'skipped'
      ? o.reason
      : o.kind === 'known'
        ? `already measured (${o.verdict.value ? 'yes' : 'no'}); no call made`
        : o.kind === 'errored'
          ? o.reason
          : o.verdict.detail
  return { harness: 'probes', case: r.label, verdict, ms, tokens: 0, calls: 0, up: null, note: note?.slice(0, 200) ?? null }
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

/** Run tier 1 against a pinned candidate and record what it establishes.
 *
 *  THE AMBIGUITY RULE IS THE MOST IMPORTANT LINE IN THIS FUNCTION. Capability is
 *  a property of the ENDPOINT serving a model, not of the model name — a
 *  quantized local build and the vendor's own API genuinely differ in what they
 *  can hold. A bare model id served by a POOL lands on one member per call
 *  (round-robin), so a run against it measured one endpoint; writing the result
 *  under all of them would credit a vendor API's tool calling to a llama.cpp
 *  build, which is a false `true` on every capability at once. When the
 *  candidate resolves to more than one key the run still happens and the results
 *  are still returned for a human to read — but nothing is recorded, and the
 *  report names the keys so the admin can re-run against the endpoint-qualified
 *  ids ("<endpoint>/<model>"), each of which resolves to exactly one. */
export async function runProbes(
  model: string,
  /** `timeoutMs` overrides the wall clock — a test drives it at milliseconds. */
  /** `reprobe` re-measures capabilities we already have a probe fact for. Off
   *  by default: a probe fact is a property of an `endpoint:model` and does not
   *  go stale on its own, so paying for it again on every sweep is spend with no
   *  new information behind it. */
  opts: { ids?: ProbeId[]; deps?: Partial<ProbeDeps>; timeoutMs?: number; reprobe?: boolean } = {},
): Promise<ProbeReport> {
  const deps = defaultDeps(model, opts.deps)
  const keys = await probeKeys(model)
  const chosen = PROBES.filter((p) => !opts.ids || opts.ids.includes(p.id))

  const results: ProbeResult[] = []
  const started = Date.now()
  let mark = started
  for (const probe of chosen) {
    // ALREADY MEASURED — report the standing fact and make no call. This is the
    // single biggest saving available on a re-test: nine probes on a model
    // tested before is nine calls buying an answer we already wrote down.
    //
    // It is reported as `known` rather than omitted, because a capability
    // missing from the report reads as unmeasured, and the whole point is that
    // it is not.
    const had = opts.reprobe ? null : await deps.measured(probe.id).catch(() => null)
    if (had) {
      const known: ProbeResult = {
        id: probe.id,
        label: probe.label,
        outcome: { kind: 'known', at: had.at, trials: [], verdict: { value: had.value, score: had.score ?? (had.value ? 1 : 0), detail: had.detail ?? 'measured by an earlier run' } },
      }
      results.push(known)
      noteLive(model, probeLine(known, 0))
      continue
    }
    // A probe that THROWS is a probe that errored, which by rule 2 writes
    // nothing. Author code meeting model output has the same standing here as it
    // does in `runHarness`: a throw is a failed measurement, never an exception
    // that escapes the suite and voids the eight probes that already ran.
    // A WALL CLOCK, because tier 2 has one and tier 1 did not. A provider that
    // accepts the connection and goes away left `runProbes` awaiting a promise
    // that never settles — holding a run slot forever, unreachable by Stop
    // (which is only honored between tiers). With eight candidates able to run
    // at once that is eight slots a few hung calls can take permanently.
    //
    // A timeout is an ERROR, never a `false`: nothing about the model was
    // measured, so by rule 2 it writes nothing.
    const budget = opts.timeoutMs ?? PROBE_TIMEOUT_MS
    const outcome = await Promise.race([
      probe.run(deps).catch((err: unknown) => errored(messageOf(err))),
      new Promise<ProbeOutcome>((resolve) => setTimeout(() => resolve(errored(`the probe did not finish inside ${budget}ms`)), budget)),
    ])
    const one: ProbeResult = { id: probe.id, label: probe.label, outcome }
    results.push(one)
    const now = Date.now()
    noteLive(model, probeLine(one, now - mark))
    mark = now
  }

  const estimate = await estimateProbes(model, opts).catch(() => null)
  const latency = latencyReading(estimate?.usd ?? null)

  const base: Omit<ProbeReport, 'wrote' | 'ambiguous'> = { model, keys, results, latency }
  // Nothing routes and nothing backs it: the results are still worth reading,
  // but there is no endpoint:model to file them under and inventing one would
  // pool this model's facts with whatever else lacked a key.
  if (keys.length === 0) return { ...base, wrote: 0, ambiguous: null }
  if (keys.length > 1) return { ...base, wrote: 0, ambiguous: keys }

  const key = keys[0]!
  const at = new Date(deps.now()).toISOString()
  let wrote = 0
  for (const r of results) {
    if (r.outcome.kind !== 'scored') continue
    const { verdict } = r.outcome
    const fact: CapabilityFact = { value: verdict.value, source: 'probe', at, detail: verdict.detail, score: verdict.score }
    // One await at a time: `recordCapability` is a read-modify-write of one
    // `app_settings` row and it serializes in process, but sequencing here also
    // means a failed write stops at the fact that failed instead of leaving the
    // count wrong.
    await deps.record(key, r.id, fact)
    wrote++
  }
  return { ...base, wrote, ambiguous: null }
}
