// THE harness contract. A harness DECLARES what it needs; it never chooses a
// transport, a model, a parser, or a failure policy. `runHarness` (run.ts)
// honors the declaration, and it is the only code that talks to a model.
//
// WHY THIS FILE EXISTS
//   `PLATFORM_AGENTS` in server/platform-agents.ts already names nine harnesses
//   and describes each one's job — and carries none of the things that make a
//   harness a harness. The prompt, the output shape, the fallback chain, the
//   failure behavior and the guard pass live in eight other files, hand-written
//   nine times over, and `PLATFORM_AGENTS[].auto` is a PROSE DESCRIPTION of a
//   chain implemented elsewhere and free to drift from it (it already had:
//   'pl-main when judging is enabled without a pick' was spelled out in the
//   judge definition — api/src/harness/defs/judge.rs since the port — and in
//   six other files besides). This interface is the other half of that
//   registry — the executable half.
//
//   The cost of the nine copies is not aesthetic. Each one answers "the model
//   returned something I could not use" DIFFERENTLY and SILENTLY: the judge
//   escalates to a human (so a weak judge model is a notification storm), the
//   blurb writer returns 0 and re-burns the same batch every ten minutes
//   forever, Muse returns null so the button just does nothing. `onFailure`
//   below is that decision, stated once, per harness, in public.
//
// PURE BY CONSTRUCTION. This module is types and one identity function. It
// imports no database, no gateway and no settings, so an app author can build a
// harness definition — and the eval suite can enumerate every one of them —
// without booting Talaria.
import type { z } from 'zod'
import type { Capability } from './capability'
import type { ModelSpec } from './model'
import type { ToolCall, ToolDefinition, ToolPolicy } from './transport'

/** One chat turn as every transport in Talaria spells it. Deliberately
 *  text-only: a harness that needs image parts is a different contract, and
 *  inventing the slot before something needs it is how the union rots.
 *
 *  RE-DECIDED, NOT INHERITED, when the tool probes were armed. The `vision`
 *  probe wants image content and is the first caller ever to want any, so the
 *  question was live: widen `content` to the OpenAI content-parts union, or
 *  leave vision unmeasurable. It stays a string, because a HALF-WIDENED union is
 *  worse than none and half is all that is reachable from here —
 *  `completeViaGateway`'s signature takes `content: string`, and every consumer
 *  of a message list in the tree reads `.content` as one: `groundingTextOf` and
 *  `extractToolRecord` for the guard pass, `lastUserMessage` and `anchorJson` in
 *  the runner, `estimateTokens(m.content.length)` on both metering paths, and 23
 *  harness `render`s. A union that only the persona payload honored would report
 *  `[object Object]` into the ledger and ground the guard against nothing.
 *
 *  So `content` stays a string, and `vision` is measured through a seam that
 *  does not need it (`gatewayImageTurn` / `personaProbeTurn` build their own
 *  multimodal body). What DID land here is the TOOL channel below, which is a
 *  different question: two optional fields, no change to `content`, and every
 *  reader listed above keeps working untouched. */
export interface Message {
  role: 'system' | 'user' | 'assistant' | 'tool'
  content: string
  /** THE TOOL CHANNEL, and it is additive on purpose.
   *
   *  `content` stays a string — the content-parts union this file warns about
   *  elsewhere is a genuinely tree-wide change and this is not it. These two
   *  optional fields carry what an assistant turn and a tool result ARE on the
   *  wire, so a loop that replays a tool conversation can send the shape every
   *  provider already speaks.
   *
   *  WHY IT HAD TO EXIST. The dry-run sandbox had nowhere to put a tool call, so
   *  it wrote one into the assistant's TEXT: first `[tool] write_file({...})`,
   *  then `(called write_file)`. Models imitated whichever string they were
   *  shown — 34 replies in one sweep contained our own narration verbatim, then
   *  the arguments as prose — so `reply.toolCalls` came back empty, the loop
   *  broke, and fixtures reported "read the repository and never wrote a file"
   *  about models that had written it. Changing the wording moved the imitation;
   *  only giving the calls their own channel ends it.
   *
   *  Renderers never set either: a harness `render` produces system and user
   *  turns. Transports that cannot express them fall back to `content`. */
  toolCalls?: ToolCall[]
  /** Set with `role: 'tool'` — which call this message is the result of. */
  toolCallId?: string
}

/** What `render` is told about the call it is rendering for.
 *
 *  `widened` is the capability-gated superpower switch, and it is how "decent
 *  on a 14B local model, excellent on a frontier one" is EXPRESSED rather than
 *  hoped for. A model that has earned the widen capabilities gets the richer
 *  prompt or the fuller action list; one that has not gets the deterministic
 *  surface. Both branches must be real answers — `widened: false` is the
 *  product working, not a degraded mode with an apology in it.
 *
 *  `model` is there so a render can name the model in its own prompt (some
 *  small models follow instructions noticeably better when addressed) and so a
 *  harness can size its context to what it is talking to. It is NOT an
 *  invitation to branch on model ids: that is what capabilities are for. */
export interface RenderContext {
  widened: boolean
  model: string
}

/** The per-ROLE minimum a harness declares.
 *
 *  DESIGN DECISION, LOCKED: the floor is per role, not global. Talaria must be
 *  decent on a 7-14B self-hosted model and excellent on a large one, and those
 *  two sentences are only compatible if each job says for ITSELF what it cannot
 *  do without. The titler, the summarizer and the librarian declare almost
 *  nothing — they have to work on whatever the self-host has, and a titler that
 *  refuses to name a chat is worse than a mediocre title. The judge, research
 *  and code harnesses declare real capabilities and REFUSE below them, because
 *  a judge that silently degrades is a judge whose verdicts are noise, and
 *  noise is worse than an honest "this model cannot do this job".
 *
 *  `capabilities` is the non-negotiable SUBSET of `requires`: the things whose
 *  absence changes the answer from "worse" to "wrong". */
export interface RoleFloor {
  /** THE REFUSAL LIST, and it is only ever read when `refuseBelow` is true —
   *  `runHarness` intersects the known-missing capabilities with this array and
   *  then does nothing with the result unless the harness refuses. So a floor
   *  that declares capabilities WITHOUT refusing is an inert declaration that
   *  reads to the next author as a hard requirement, which is how the eight
   *  ports arrived with two spellings of "needs JSON, runs anyway": one wrote
   *  `capabilities: []` and five wrote `capabilities: ['json']`, and both ran
   *  identically. Keep this EMPTY unless `refuseBelow` is true, and say what the
   *  job leans on in `requires` — which is what the fitness matrix scores, and
   *  which never blocks. */
  capabilities: Capability[]
  /** True: refuse the run and say which capability is missing. False: run
   *  anyway and let the result carry the fact. Never silently half-work. */
  refuseBelow: boolean
  /** CAPABILITIES THE PLATFORM MAY SUPPLY, so the floor asks whether the RUN can
   *  reach them rather than whether the MODEL has them.
   *
   *  THE DISTINCTION IS THE WHOLE POINT. A slot an admin assigns is not a bare
   *  model — it is a model running inside Talaria, with the tools this org has
   *  registered and a gateway that can hand it definitions. `research-search`
   *  refusing every model without native browsing was correct about the weights
   *  and wrong about the deployment: a model measured at 100% tool calling and
   *  100% tool selection, with a web-search server registered, does the job.
   *
   *  Listing a capability here does NOT weaken the floor. It redirects it:
   *  `capability-reach.ts` still has to find a registered, enabled tool AND a
   *  model that can call it, and an org with neither gets the same refusal with
   *  a better sentence. What it stops is refusing on a fact that was true about
   *  the model and irrelevant to the run.
   *
   *  A harness that lists a capability here MUST have a code path that actually
   *  uses the tool — see `researchSearchHarness`, which picks its transport on
   *  the answer. Declaring it without one would turn a refusal into a silently
   *  worse run, which is the failure this floor was built to prevent. */
  suppliable?: Capability[]
  /** One sentence, shown next to the model picker in Admin. Written for the
   *  admin choosing a model, not for the developer reading this file. */
  note: string
}

/** A fixture the model-fitness suite replays through `runHarness` with a
 *  candidate model pinned (audit Part 3, tier 2).
 *
 *  `check` is deliberately a plain assertion over the parsed value rather than
 *  an expected output: most harness assertions are string facts ("3-7 words",
 *  "no invented status", "never an actionId outside the allowlist"), and a
 *  deterministic check keeps the suite fast, cheap and free of the
 *  who-judges-the-judge regress. Return null to pass, or one line saying what
 *  was wrong — that line is what the admin reads in the drill-down. */
export interface EvalCase<I, O> {
  name: string
  input: I
  /** `ctx` carries WHAT THE MODEL DID, for the harnesses that are dry-run
   *  against a sandbox Talaria (the Rust fitness toolbox, api/src/fitness/toolbox/).
   *  Optional to receive and
   *  empty for every single-shot harness, so the hundred existing fixtures that
   *  ignore it stay correct — a fixture only reaches for it when the question it
   *  is asking is behavioural.
   *
   *  THE POINT OF IT: "did it SAY it triaged the ticket" is answerable from the
   *  value and is the wrong question; "did it call triage_ticket before claiming
   *  to have started" is answerable only from here, and is the failure that
   *  costs an org a week. */
  /** See `CheckResult`: null passes, a string fails the MODEL, and `{ gap }`
   *  reports that this fixture could not fairly ask the question. */
  check: (value: O, ctx: EvalContext) => CheckResult
  /** Which difficulty band this fixture belongs to. Bands are reported
   *  separately, so "solid on standard, fails the hard band" is sayable instead
   *  of collapsing into one rate that hides which half a model can do.
   *
   *  Defaults to 'standard' — the band a fixture belongs to unless its author
   *  says otherwise, so adding a band to the type did not silently re-label
   *  every existing fixture as easy. */
  band?: EvalBand
}

/** EASY is the floor a model must clear to be usable at all; STANDARD is the job
 *  as it actually arrives; HARD is where a frontier model should pull ahead —
 *  ambiguity, a trap, a rule that has to be applied against the grain. A model
 *  failing only the hard band is a real and useful answer, and the old flat rate
 *  could not express it. */
export type EvalBand = 'easy' | 'standard' | 'hard'

/** What a fixture can see about a DRY RUN — a harness turn the fitness suite ran
 *  with a real tool loop against an isolated, in-memory Talaria.
 *
 *  Typed structurally rather than importing the suite that fills it: that suite
 *  is the Rust api's (api/src/fitness/toolbox/) and no TS import crosses
 *  languages, but the deeper reason survives the port — a harness definition
 *  must stay importable without any benchmark's module graph attached, which is
 *  exactly the kind of coupling the harness layer exists to avoid. */
export interface EvalContext {
  /** Every tool call the model made, in order, with what it got back. Empty for
   *  a harness that was not dry-run. */
  calls: ReadonlyArray<{ tool: string; args: Record<string, unknown>; result: unknown; error: string | null }>
  /** Did the model call `a` at any point before it called `b`? False when either
   *  never happened, which is the reading a fixture wants: "read the ticket
   *  before commenting" is not satisfied by doing neither. */
  calledBefore: (a: string, b: string) => boolean
  /** The sandbox world AFTER the run — the state a fixture asserts the model
   *  left behind. Null when the harness was not dry-run. */
  world: unknown
  /** The loop hit its turn bound with the model still calling tools. */
  exhausted: boolean
}

/** The context a single-shot harness's fixture receives: nothing happened,
 *  because nothing could. Exported so the fitness suite and every test that
 *  calls a `check` by hand agree on what "no tools ran" looks like. */
export const NO_TOOLS: EvalContext = { calls: [], calledBefore: () => false, world: null, exhausted: false }

/** THE FLOOR EVERY TEXT FIXTURE NEEDS, and the bug it closes.
 *
 *  A text harness's `clean` is usually `raw.trim() || null`, so any non-empty
 *  string is a legitimate value and `schema_valid` is honestly true. That is
 *  correct — the CONTRACT is not lying. What was lying was the task score: six
 *  fixtures across the summarizer, the distiller, the briefer and outreach
 *  asserted only that the answer was not too long, not markdown, not a question
 *  and not a repeat of the input. Every one of those is a real failure mode and
 *  every one of them is satisfied by saying almost nothing, so replaying the
 *  literal string `{"nope": true}` through the whole registry scored six PASSES.
 *  `evals.test.ts` keeps that census, and it is a `<=` so tightening a fixture
 *  never fails it.
 *
 *  So a one-sided fixture states its floor here: how short is too short to be an
 *  answer at all, and — where the input has an unmistakable subject — one of a
 *  few words the answer has to have engaged with. `mentions` is deliberately a
 *  SET of alternatives and not a phrase: it must reject a non-answer without
 *  scoring the model's word choice, and a fixture that only one wording can pass
 *  measures our prompt rather than the model.
 *
 *  Returns the admin-facing sentence, or null when the answer clears the floor. */
/** A COUNT LIMIT, WITH THE MARGIN A HUMAN WOULD GIVE IT.
 *
 *  A prompt that asks for "3-7 words" is an instruction, not a schema. Scored as
 *  a hard boundary it failed three capable models for an EIGHT-word title — one
 *  word over, on a title the product then clamps to 90 characters anyway, so
 *  nothing anywhere was harmed by the extra word. That is not a measurement of
 *  the model; it is a measurement of how literally it read a range.
 *
 *  So a count fails when the overshoot is MATERIAL. The default margin is a
 *  quarter, floored at one unit, which lets 3-7 accept 2-8 and rejects the
 *  one-word title and the paragraph alike — the two answers that are actually a
 *  different kind of thing from the one asked for.
 *
 *  USE IT FOR A STATED PREFERENCE, NOT FOR A HARD EDGE. Where exceeding the
 *  number breaks something — a card that clips, an action taken one time too
 *  many, a batch that creates an eleventh ticket — assert the real limit
 *  directly and say what breaks. `tolerance: 0` is available for the cases in
 *  between, and reads as the deliberate choice it is. */
/** WHAT A FIXTURE'S `check` MAY CONCLUDE.
 *
 *  `null`      the model did the job.
 *  `string`    the model did not, and this sentence says how. A FAILURE.
 *  `{ gap }`   THE HARNESS DID NOT GIVE THE MODEL WHAT THE JOB NEEDED, so the
 *              answer cannot be scored. NOT a failure, and never attributed to
 *              the model.
 *
 *  THE THIRD CASE IS THE ONE WORTH EXPLAINING. A fixture asserts that a coding
 *  run ran the tests, or that a session filed a capability gap, or that a brief
 *  named the one blocked item — and every one of those is only a fair question
 *  if the run was actually given a test runner, a gap tool, and a briefing that
 *  contained the item. When it was not, the model can do everything right and
 *  still miss the assertion, and scoring that as a model failure is measuring
 *  our own fixture and calling it a capability.
 *
 *  This is the same distinction `EvalCaseScore.skipped` already draws one level
 *  up — "we never asked" is not "it answered badly" — pushed down to where the
 *  fixture can see what it actually handed over. A gap is reported to US: it
 *  lands in the run's own list of things to fix, not in the model's score.
 *
 *  Return one when the ASSERTION IS UNANSWERABLE as posed. Do not return one for
 *  a hard task — difficulty is what a band is for. */
export type CheckResult = string | null | { gap: string }

/** Narrowing helper, so consumers never test the shape by hand. */
export const isGap = (r: CheckResult): r is { gap: string } => typeof r === 'object' && r !== null && 'gap' in r

export function countProblem(
  actual: number,
  limit: { min?: number; max?: number; unit: string; asked: string; tolerance?: number },
): string | null {
  const slack = (n: number): number => Math.max(1, Math.round(n * (limit.tolerance ?? 0.25)))
  const plural = (n: number): string => `${n} ${limit.unit}${n === 1 ? '' : 's'}`
  if (limit.min !== undefined && actual < limit.min - slack(limit.min)) return `${plural(actual)} — the prompt asks for ${limit.asked}`
  if (limit.max !== undefined && actual > limit.max + slack(limit.max)) return `${plural(actual)} — the prompt asks for ${limit.asked}`
  return null
}

export function belowAnswerFloor(value: string, floor: { minChars: number; mentions?: readonly string[] }): string | null {
  const text = value.trim()
  if (text.length < floor.minChars) {
    return `the answer is ${text.length} characters, which is too short to be an answer to this at all (${floor.minChars} is the floor)`
  }
  if (!floor.mentions || floor.mentions.length === 0) return null
  const lower = text.toLowerCase()
  if (floor.mentions.some((term) => lower.includes(term.toLowerCase()))) return null
  return `the answer never engages with what it was given - it mentions none of ${JSON.stringify([...floor.mentions])}`
}

/** THE RELATION BETWEEN THE INPUT AND THE OUTPUT — the half of a harness
 *  contract a schema is structurally incapable of stating.
 *
 *  WHY THIS EXISTS. A schema is a module constant. It is built once, at import
 *  time, and it cannot see the run's input, so every harness whose correctness
 *  is a RELATION between what was asked and what came back had no way to say so
 *  — and `runHarness` recorded `schemaValid: true` for a value the caller then
 *  threw away. Four shipped bugs were that one defect in different clothes:
 *
 *    blurb-writer  `z.record(z.string(), z.string())` cannot constrain the KEYS.
 *                  A model that tidied `qwen3-14b` into `Qwen3 14B` passed the
 *                  schema, wrote zero blurbs, and reported a 100% contract rate
 *                  — then came back around on the identical batch every ten
 *                  minutes forever.
 *    channel-plan  the elements must be TICKETS from the transcript, not titles
 *                  the model invented and not a bracketed citation marker.
 *    muse:ticket   a date must be one the WRITE PATH accepts (`z.string()` here
 *                  against `z.string().datetime()` on the route), so the repair
 *                  turn could never fire on the likeliest small-model mistake.
 *    redaction     a value that still parses after being cut in half.
 *
 *  THE OFFLINE SUITE ALREADY KNEW. `EvalCase.check` is this same assertion, and
 *  blurb-writer's own fixture rejects invented ids — so the eval fixtures and
 *  the `harness_runs.schema_valid` column DISAGREED, and the production one was
 *  the optimistic liar. `schema_valid` is the OBSERVED half of the model-fitness
 *  matrix; a column that says a model held a contract it did not hold makes the
 *  whole matrix worth less than nothing.
 *
 *  WHAT IT IS. Runs AFTER schema validation, only ever on a parsed value.
 *  Returns null when the value is usable, or ONE PLAIN SENTENCE naming the
 *  problem. That sentence is fed straight back to the model as a repair
 *  instruction, exactly like `parseJson`'s error, so write it as an instruction
 *  to the model and not as a note to a developer: "the keys must be the model
 *  ids exactly as given - 'Qwen3 14B' is not one of them" repairs; "invariant
 *  violated in blurbWriter" does not.
 *
 *  A VERIFY FAILURE IS A CONTRACT FAILURE, in every sense the runner has: it
 *  repairs on the same loop against the same counter, it sets
 *  `schemaValid: false`, and it lands on the `harness_runs` row honestly.
 *
 *  It may THROW without escaping the runner — this is harness-author code
 *  meeting model output, the same as `render`, `clean` and `ground`, and a throw
 *  out of any of them is a failed contract rather than the one exception that
 *  escapes a runner whose whole promise is that a bad model produces a RESULT.
 *
 *  IT IS TOLD WHAT `render` WAS TOLD. The third argument is the SAME
 *  `RenderContext` the prompt was built from, and it exists because the widened
 *  surface changes the contract rather than only the wording: `inbox-command`
 *  offers a probed model the item's whole action list and a regex-bound one a
 *  single id, so "did it stay inside what it was offered" is unanswerable from
 *  `(value, input)` alone. Without it that harness — the one carrying the
 *  product's safety assertion — had to leave its own eval's check to a caller,
 *  and recorded `schemaValid: true` for a proposal that caller dropped.
 *
 *  KEEP IT CHEAP AND PURE. It runs on every attempt of every run, including the
 *  redaction re-check, and this module imports no database by construction. A
 *  verify that needs to ask the database whether an id exists is a check for the
 *  caller, not for the contract. */
export type Verify<I, O> = (value: O, input: I, ctx: RenderContext) => string | null

/** THE GROUNDING MATERIAL for one turn — the tool record a harness can honestly
 *  supply from its OWN input, which no transport is in a position to derive.
 *
 *  WHY THIS EXISTS: `ungrounded_ref` ("cites link(s)/id(s) that did not appear in
 *  any tool result this turn — may be fabricated") is the single highest-value
 *  rule in `guardrails.ts` and it COULD NOT FIRE FROM ANY HARNESS, by
 *  construction. The rule returns null when `backingTools` is empty or the
 *  results overflowed, and `runHarness` derives its record from the messages IT
 *  sent — which for a harness turn contain no tool messages at all — or, on the
 *  fleet path, sets `overflowed: true` because a persona's tool loop ran inside
 *  the agent. So the rule self-skipped on all 23 harnesses, and the one path in
 *  the product whose defining failure mode is a fabricated citation had to run
 *  it OUTSIDE the runner over a record it built by hand.
 *
 *  A harness that HAS the material is a harness whose input already contains it:
 *  research's synthesis stage is handed the search hits and the numbered source
 *  registry, which ARE the tool results for that turn. This hook is how it says
 *  so, and the runner then supplies an honest `Available` instead of a cautious
 *  one.
 *
 *  HONESTY IS EXPRESSIBLE HERE; OPTIMISM IS NOT THE DEFAULT. A harness that
 *  declares no `ground`, or whose `ground` returns null or an empty `tools`
 *  list, keeps exactly the `Available` its transport earned — the rules SKIP
 *  rather than run on material nobody has. Claiming a grounded turn with no
 *  backing tool would turn "we cannot check this" into "we checked and it is
 *  fine", which is the one direction a guard must never move. */
export interface Grounding {
  /** The backing tools that GENUINELY ran for this turn, named as the tool
   *  registry names them (research's search stages are `research_search`). An
   *  EMPTY list is not grounding: `runHarness` treats it as an absent hook and
   *  falls back to the transport's own record. */
  tools: string[]
  /** Everything those tools returned, concatenated. Supply MORE than the prompt
   *  carried where you have it — grounding a citation against more than the
   *  model saw can only remove false positives, never add one. */
  results: string
  /** Did any of those tools return a transport/availability error?
   *
   *  `null` means the harness genuinely cannot say, which SKIPS
   *  `fabricated_outage` rather than asserting "nothing errored" — the
   *  difference between a rule that is quiet and a rule that is wrong. */
  errored: boolean | null
}

export interface HarnessDefinition<I, O> {
  id: string
  label: string
  /** One line, shown in Admin. Today's `PLATFORM_AGENTS[].job`. */
  job: string

  /** What the model must be able to DO. The fitness suite scores against this,
   *  and `runHarness` consults it before the call instead of discovering the
   *  answer from a 400 halfway through. Unknown is not missing — an untested
   *  model still runs (see capability.ts). */
  requires: Capability[]

  /** The floor for THIS role, and what happens below it. */
  floor: RoleFloor

  /** Model resolution, declared not written — the chain that was hand-copied
   *  into seven files (audit 1.10). See harness/model.ts. */
  model: ModelSpec

  /** Input -> messages. THE ONLY THING A HARNESS AUTHOR WRITES BY HAND. */
  render: (input: I, ctx: RenderContext) => Promise<Message[]> | Message[]

  /** The output contract. `kind: 'json'` puts the runner in structured mode:
   *  it asks for JSON at the protocol level when the model can honor that,
   *  parses with the one balanced-brace extractor, and repairs once on a
   *  malformed reply (audit 1.4 — nothing in the tree retries today).
   *
   *  For `kind: 'text'` with no `clean`, O is string by construction. `clean`
   *  is where a text harness narrows: it receives the raw reply and returns the
   *  value, or null to fail the contract (which is exactly what the Rust titler
   *  def's quote-and-fence stripping, api/src/harness/defs/titler.rs, already
   *  does by hand).
   *
   *  `verify` is the OTHER half of the contract and the half neither a schema
   *  nor a `clean` can express, because both are written before the input
   *  exists — see `Verify` above for the four bugs that were all this one gap.
   *  Both output kinds carry it: a text harness's "did it answer the question I
   *  asked" is the same question as a JSON harness's "are these the ids I sent". */
  output:
    | { kind: 'text'; clean?: (raw: string) => O | null; verify?: Verify<I, O> }
    | { kind: 'json'; schema: z.ZodType<O>; repair?: number; verify?: Verify<I, O> }

  /** What a failure MEANS here. Stated per harness because before this existed
   *  each site answered it differently and in silence:
   *    'null'            the caller keeps what it had (titler, summarizer)
   *    'throw'           the caller must handle it (a request-path harness)
   *    { fallback }      a declared safe value (a default verdict, an empty list)
   *    { escalate: true } a human decides — the runner sets `escalate` on the
   *                       result and the caller raises it, because only the
   *                       caller knows who to tell (the judge def's
   *                       `tellHumansTheGateStopped`, api/src/harness/defs/judge.rs).
   *                       A FLAG, not a phrase in
   *                       the error string: a caller that has to string-match to
   *                       find out is a caller that stops escalating the day
   *                       somebody rewords the message.
   *
   *  'throw' MEANS ANY FAILURE TO PRODUCE A VALUE, which is what a caller reads
   *  it to mean and what it did not do. `runHarness` RETURNS for everything that
   *  happens before or during the call — nothing in the chain routes, the floor
   *  refuses, `render` throws, the transport dies — so 'throw' used to cover the
   *  contract failure and nothing else, and the policy had to be restated by
   *  hand at every call site. Five callers restated it; the two that did not
   *  BOTH shipped a bug: research synthesis saved an empty report, marked the
   *  run `done`, indexed it and notified the requester after a 502, and the
   *  channel planner answered "nothing to plan yet" on a channel full of work
   *  because its agent container was restarting. It now throws on all of them.
   *
   *  THE OTHER THREE STAY CONTRACT-SCOPED, and that asymmetry is deliberate
   *  rather than left over. They describe what a caller GETS when the model
   *  answered and the answer was unusable — a question that does not arise when
   *  no model was reached — and widening them would break both callers that use
   *  them. `{ fallback }` on a pre-call failure would hand outreach its
   *  "nothing to surface" token during a gateway outage, so a dead provider
   *  would read as a normal quiet pass on every sweep. `{ escalate: true }` on a
   *  pre-call failure would have the judge notify every board editor about every
   *  ticket for as long as the gateway is down — the notification storm this
   *  whole audit is about. A caller that wants either of those on an
   *  unreachable model has `HarnessResult.answered` to say so explicitly, which
   *  is a sentence somebody wrote on purpose rather than a policy that widened
   *  under them. */
  onFailure: 'null' | 'throw' | { fallback: O } | { escalate: true }

  /** The capability-gated widening. Set it and `render` is called with
   *  `widened: true` only when EVERY capability here is `value: true` with
   *  `source: 'probe'` for the resolved model — Talaria's own measurement, not a
   *  vendor's claim. Unknown does not widen, and neither does `declared` or
   *  `learned`: widening is the direction that hands a model more authority, so
   *  it is the direction that demands evidence. See the comment on that check in
   *  run.ts, which explains why the floor is deliberately laxer about
   *  provenance and why the asymmetry is not an inconsistency. */
  widen?: { requires: Capability[]; note: string }

  /** Guardrails. `rules` narrows the registry to the ids that make sense for
   *  this output (a titler cannot make a zero-tool claim); omitted means every
   *  enabled rule. `redact` makes the runner strip credentials and PII out of
   *  the VALUE it returns, for harnesses whose output is persisted. */
  guard?: { rules?: string[]; redact?: boolean }

  /** The turn's real tool record, from the harness's own input. Declare it and
   *  the runner guards with `{ results: true }` and a genuine `backingTools`, so
   *  `ungrounded_ref` and `fabricated_outage` can actually fire. Omit it — which
   *  every harness that has no tool results should — and nothing changes. See
   *  `Grounding` above for why the empty case is deliberately inert. */
  ground?: (input: I) => Grounding | null

  temperature?: number

  /** May the model use ITS OWN tools on this turn?
   *
   *  Omitted means 'none', and 'none' is right for every single-shot structured
   *  harness: the runner asks one question and parses one answer, so a tool call
   *  can only be the model wandering off. It is enforced at the protocol level
   *  (`tools: []` / `tool_choice: 'none'`), not left to the prompt.
   *
   *  'own' is the three turns whose whole feature IS the tool loop — an agent
   *  working a ticket, an outreach check-in acting through `message_user`, a
   *  briefing follow-up the owner expects to answer from live data. On those,
   *  'none' does not weaken the answer, it disarms the agent and then trips
   *  `zero_tool_claim` for having called no tool. A model served by the ORG
   *  GATEWAY cannot honor 'own' at all, and says so by failing the call rather
   *  than by quietly running a tool-loop harness as a single completion. */
  tools?: ToolPolicy

  /** HOW TO DRY-RUN THIS HARNESS in the fitness suite, when the platform has to
   *  supply the tool loop the model cannot run for itself.
   *
   *  Only meaningful alongside `tools: 'own'`. Production hands those turns to a
   *  persona whose loop lives inside the agent container, where the platform can
   *  see tool NAMES and nothing else — so the suite could never ask the question
   *  that matters, which is not "can this model emit a tool call" but "given
   *  these tools and this situation, what did it actually do". The suite offers
   *  the tools named here, backed by an isolated in-memory Talaria, and the
   *  fixtures assert over the call log (`EvalContext.calls`).
   *
   *  NAME THE TOOLS THE JOB NEEDS AND NO MORE. A tool surface is a prompt:
   *  handing a briefing chat the ticket-triage tools measures a model's
   *  tolerance for irrelevant options rather than its judgement, and a candidate
   *  that looks worse for it has been measured against a deployment nobody has.
   *
   *  Omitted means every sandboxed tool, which is right only for a harness whose
   *  agent genuinely carries the whole toolkit. */
  dryRun?: {
    /** Which Talaria toolkit tools to offer. Omit when `workspace` is set —
     *  those two are different surfaces and a harness has one. */
    tools?: string[]
    /** MODEL TURNS THIS HARNESS'S LOOP MAY TAKE, when the default six is not
     *  what production gives it. Declare it to match the real job: benching a
     *  twelve-turn work session at six measures a shorter job than the one the
     *  harness does, and then judges the model on work it was cut off in the
     *  middle of. Capped by `MAX_TURN_CEILING`.
     *
     *  IT ALSO WIDENS THE CASE CLOCK: `turnsPerCase` reads it, so a longer loop
     *  gets proportionally longer to run in rather than timing out at a budget
     *  sized for a shorter one. */
    maxTurns?: number
    /** Overrides on the sandbox's standard world — a ticket in a particular
     *  state, a gap already filed, a DM already sent. Typed loosely here so this
     *  module stays free of the fitness suite's imports; the suite narrows it.
     *
     *  A FUNCTION OF THE INPUT, like `workspace` and `credentials` below, and
     *  for the same reason they are. A flat record is read once per DEFINITION,
     *  so a harness can only ever pose questions about ONE world — and the most
     *  valuable fixture in a group is routinely the one that changes it. "Google
     *  is not connected: do you say so, or invent a link" cannot share a harness
     *  with "read the calendar" unless this takes the input, and splitting a
     *  coherent surface into two harnesses to vary one boolean is the tail
     *  wagging the dog.
     *
     *  A plain record still works and still means "the same world for every
     *  fixture", which is what most harnesses want. */
    world?: Record<string, unknown> | ((input: I) => Record<string, unknown>)
    /** THE OTHER SURFACE: a FILE workspace with a test runner, for the coding
     *  harnesses. Built per fixture from that fixture's own input, because a
     *  repository and the oracle that decides whether its tests pass are
     *  properties of the case rather than of the harness.
     *
     *  Typed structurally for the same reason `EvalContext` is: a harness
     *  definition must stay importable without the fitness suite, since
     *  `registry.ts` enumerates every definition in production. */
    workspace?: (input: I) => {
      files: Array<{ path: string; content: string }>
      passes: (files: ReadonlyArray<{ path: string; content: string }>) => string | null
    }

    /** THE CREDENTIAL SURFACE — a shell and outbound HTTP, where a granted
     *  handle is actually spent. Declared by a harness whose subject is what the
     *  model does with a credential it cannot read.
     *
     *  Structurally typed for the same reason `workspace` above is: a definition
     *  must stay importable without the fitness suite, because `registry.ts`
     *  enumerates every one of them in production. */
    credentials?: (input: I) => {
      granted: Array<{ handle: string; value: string; accepts: string }>
    }
  }

  /** Tools this harness OFFERS the model on the turn — a different question from
   *  `tools` above, which is about the model's own loop (see `ToolPolicy` in
   *  transport.ts). Declared here rather than assembled by a caller for the same
   *  reason the prompt is: the runner puts it on the request, the transport that
   *  cannot serve it refuses the call, and `harness_runs` records a turn that
   *  says what it really was.
   *
   *  Today its only declarers are the `tools` and `tool-select` probes, which is
   *  the point:
   *  offering four disjoint tools and reading back which one the model called is
   *  the only honest way to measure the capability that widens the Inbox command
   *  harness (audit 1.8). A harness that offers tools must be pinned to a
   *  GATEWAY model — a fleet persona's tool loop belongs to the agent, and the
   *  fleet transport refuses rather than pretending otherwise. */
  toolDefs?: ToolDefinition[]

  /** How long a persona transport may HOLD for an agent that is not answering
   *  yet, in ms. `proxyChat` waits two minutes by default; an agent restarting
   *  under a config propagation refuses connections for tens of seconds, and a
   *  work session must survive a fleet re-render mid-session — so `work-session`
   *  asks for ten minutes and the briefing panel, where a person is watching a
   *  spinner, asks for thirty seconds. Meaningless on the gateway path. */
  holdMs?: number

  /** Fixtures the model-fitness suite replays. An app-shipped harness that
   *  declares these gets its own column in the org's fitness matrix for the
   *  cost of an array. */
  evals?: EvalCase<I, O>[]
}

/** A JSON HARNESS REQUIRES JSON, DERIVED RATHER THAN DECLARED.
 *
 *  A harness with `output.kind === 'json'` needs structured output by
 *  construction — the platform sends its schema at the protocol level and parses
 *  the reply against it. That makes it a FLOOR: a model measured unable to
 *  produce structured output is unfit for the task, and asking it anyway means
 *  handing prose to a parser and recording the wreckage as the model's failure.
 *
 *  DERIVED because the rule is mechanical and the alternative is nine copies of
 *  it that drift. Restating a floor per harness is how one of them comes to omit
 *  it and quietly go back to the old behaviour. `registry.test.ts` asserts every
 *  JSON harness carries it, so the derivation cannot be silently bypassed.
 *
 *  WHAT "UNFIT" MEANS HERE IS NARROW, and deliberately so. `run.ts` refuses only
 *  on a capability MEASURED false by a probe or declared false by a human — never
 *  on an unknown, never on a single upstream 400, never on a catalog spec sheet.
 *  A fresh self-host that has probed nothing still runs every JSON harness. The
 *  floor bites exactly when Talaria has real evidence the model cannot do the
 *  thing the task is made of.
 *
 *  `refuseBelow` is forced true for the same reason: a floor that names the
 *  capability but declines to act on it is a comment, not a floor. An author can
 *  still add capabilities and their own note; neither is overwritten. */
const withJsonFloor = <I, O>(h: HarnessDefinition<I, O>): HarnessDefinition<I, O> =>
  h.output.kind !== 'json' || h.floor.capabilities.includes('json')
    ? h
    : { ...h, floor: { ...h.floor, capabilities: [...h.floor.capabilities, 'json'], refuseBelow: true } }

/** Identity, for the inference — plus the one derived floor above. Written as a
 *  function rather than a bare `satisfies` so that `render`'s input and
 *  `schema`'s output are checked against each other at the definition site — the
 *  one place a harness author can get the pair wrong and the last place anyone
 *  would look for it. */
export const defineHarness = <I, O>(h: HarnessDefinition<I, O>): HarnessDefinition<I, O> => withJsonFloor(h)
