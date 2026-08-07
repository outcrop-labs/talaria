// The Catalog writer: one-line, plain-language descriptions for every model the
// gateway serves, written in the org's own voice.
//
// THE BATCH IS THE WHOLE DIFFICULTY. This is the only harness in the tree that
// asks for one object with MANY keys, and that is the shape a small model is
// worst at: it answers with six of the ten ids, or nests them under a "models"
// wrapper, or mirrors the array it was handed back as an array of records.
//
// WHY THE SCHEMA IS SHAPED THE WAY IT IS (the choice, stated once):
//   TOLERANT ON CARDINALITY, STRICT ON TYPE.
//   - A PARTIAL batch is a success. Every model this pass skips simply keeps its
//     catalog blurb and comes back around on the next sweep, so six good lines
//     are six good lines. `rewritePendingBlurbs` already reads the record key by
//     key and ignores what isn't there; the schema tolerates in the same
//     direction rather than demanding all ten and throwing six away.
//   - A WRONG TYPE is a failed contract, not something to salvage. The tempting
//     alternative — accept `unknown` values and quietly keep the strings — would
//     make a model that answered entirely in nested objects return `{}` with
//     `schemaValid: true`, which is precisely the silent success the audit is
//     about (1.1: `return 0`, forever, with nothing logged). Failing instead
//     buys the repair turn, and "expected object, got array" / "field 'x' should
//     be string, got object" is about as actionable as a repair instruction
//     gets. One repair round trip fixes the array and the nesting cases; nothing
//     fixes a contract rate that was never measured.
//
// Length and per-line emptiness are NOT in the schema or on `verify`, on
// purpose. Clamping to 200 chars is the caller's write-time concern (it always
// was), and putting `.max(200)` on the contract would let one over-long line
// fail a batch of ten. The eval fixtures below do assert length and
// non-emptiness, because there the point is to MEASURE the model rather than to
// keep the sweep moving.
//
// THE KEYS ARE THE EXCEPTION, and they are on `verify` rather than in the
// fixtures alone — see `keyIssue` below for why a tolerant schema plus a
// tolerant caller added up to a harness that reported a perfect contract for a
// reply that wrote nothing.
import { z } from 'zod'
import { defineHarness } from '../define'

/** One catalog entry on its way to being rewritten. `description` is the raw
 *  public-catalog line; `name` is the vendor's pretty label. */
export interface BlurbCandidate {
  id: string
  name: string
  description: string
}

/** The org name travels in the INPUT rather than being read from settings
 *  inside `render`, so the definition stays pure and an eval fixture fully
 *  determines the prompt it replays. */
export interface BlurbBatch {
  orgName: string
  models: BlurbCandidate[]
}

/** model id -> one-line description. The keys are the ids the batch asked for
 *  and nothing else — `keyIssue` is what makes that true. */
export type BlurbMap = Record<string, string>

const SCHEMA: z.ZodType<BlurbMap> = z.record(z.string(), z.string())

/** THE INPUT-RELATIVE HALF OF THE CONTRACT, in ONE function, used by
 *  `output.verify` and by every eval fixture below. Two spellings of this rule
 *  is how the offline suite and the production telemetry came to disagree, so
 *  there is one.
 *
 *  `SCHEMA` cannot enforce the keys — it is a module constant and the batch's
 *  ids are runtime input — so `z.record(z.string(), z.string())` accepts any
 *  flat string map, including one keyed by the tidied-up DISPLAY NAMES the
 *  prompt hands the model right next to the ids it asks it to use ("Qwen3 14B"
 *  instead of "qwen3-14b"). Every per-id lookup then missed,
 *  `rewritePendingBlurbs` wrote nothing, the same batch came back around every
 *  ten minutes forever, and the run was recorded as a PERFECT CONTRACT — audit
 *  1.1's exact symptom with green telemetry over it. The eval fixture rejected
 *  that reply the whole time; `harness_runs.schema_valid` did not, and between
 *  the two the production column was the optimistic liar.
 *
 *  AN EMPTY OBJECT IS A FAILURE HERE AND A PARTIAL ONE IS NOT. Six lines out of
 *  ten is six good lines — the four this sweep skipped keep their catalog blurb
 *  and come back around in ten minutes. Zero lines is the sweep achieving
 *  nothing and re-burning the identical batch forever, which is the bug, and
 *  unlike the ticket drafter there is no honest "nothing to write here": every
 *  id in the batch is a registered model with a catalog description attached.
 *
 *  WRITTEN AS AN INSTRUCTION TO THE MODEL, because `runHarness` hands this
 *  sentence straight back on the repair turn. It names the offending key, quotes
 *  it, and re-states the ids — a 14B model can act on that; "keys must be a
 *  subset of the requested ids" is a note to a developer. */
function keyIssue(ids: string[], value: BlurbMap): string | null {
  const keys = Object.keys(value)
  if (keys.length === 0) return `you returned an empty object - write one line for each of these model ids: ${ids.join(', ')}`
  const unasked = keys.filter((k) => !ids.includes(k))
  if (!unasked.length) return null
  const quoted = unasked.map((k) => `"${k}"`).join(', ')
  return `the keys must be the model ids exactly as they were given - ${quoted} ${unasked.length === 1 ? 'is' : 'are'} not one of them. Use these ids as the object's keys, spelled exactly like this: ${ids.join(', ')}.`
}

/** The fixture assertion, shared by every eval case. Deterministic on purpose —
 *  no second model is needed to tell whether a batch came back usable.
 *
 *  It opens with the contract itself (`keyIssue`) so the fixtures and the
 *  harness can never disagree about the keys, and then adds what the fixtures
 *  MEASURE but the contract deliberately tolerates: an empty or over-long line.
 *  Failing a batch of ten over one long sentence would cost nine good ones,
 *  while scoring it tells an admin something true about the model. */
function checkBatch(ids: string[], value: BlurbMap): string | null {
  const keys = keyIssue(ids, value)
  if (keys) return keys
  for (const [id, line] of Object.entries(value)) {
    if (!line.trim()) return `the description for '${id}' is empty`
    if (line.length > 200) return `the description for '${id}' is ${line.length} chars - the picker shows one line`
  }
  return null
}

export const blurbWriterHarness = defineHarness<BlurbBatch, BlurbMap>({
  id: 'blurb-writer',
  label: 'Catalog writer',
  job: 'Keeps the model catalog human: one-line plain-language blurbs for every registered model.',

  // `json-strict` is what a many-keyed object actually exercises, and saying so
  // is how the fitness matrix learns to distinguish this harness from the
  // titler. Neither is in the floor: see below.
  requires: ['json', 'json-strict'],

  floor: {
    capabilities: [],
    refuseBelow: false,
    note: 'Runs on any model; a weaker one describes only part of each batch, and the models it skipped keep their catalog line until the next sweep picks them up.',
  },

  // Pin, then the default chain (Utility role, env default, first routable).
  // No `role: 'utility'`: the default chain already has that step, and declaring
  // both resolves the same model under a different `chain_step` label — see the
  // note in titler.ts.
  model: { pin: 'blurb-writer' },

  render: (input) => {
    const ids = input.models.map((m) => m.id)
    return [
      {
        role: 'system',
        content:
          `You write one-line model descriptions for ${input.orgName || 'a team'}'s workspace pickers. ` +
          'Each line tells a non-technical teammate what the model is good at and when to pick it — plain, confident, concrete. ' +
          'No parameter counts, no version trivia, no vendor marketing. 110 characters max each. ' +
          'Reply with ONLY a JSON object mapping each model id to its one-line description.',
      },
      {
        role: 'user',
        // The id list is repeated after the payload because a model that
        // helpfully tidies an id ("qwen3-14b" -> "Qwen3 14B") produces keys
        // nothing matches, and the whole batch then writes nothing. Naming the
        // keys verbatim, last, is the cheapest defense there is; `verify` below
        // is what happens when it does not take.
        content: `${JSON.stringify(input.models)}\n\nUse exactly these ${ids.length} ids as the object's keys, spelled exactly as written: ${ids.join(', ')}`,
      },
    ]
  },

  // `verify` is the KEYS — the half of this contract a schema is structurally
  // unable to state, because the ids only exist at run time. It gets the batch
  // straight from the input, so the harness and its fixtures assert the same
  // rule from the same function (`keyIssue`), and a reply keyed by display name
  // is now a repairable contract failure that lands on the run row instead of a
  // silent zero-write pass with `schema_valid: true` over it.
  output: { kind: 'json', schema: SCHEMA, verify: (value, input) => keyIssue(input.models.map((m) => m.id), value) },

  // Fire and forget, as it always was: a failed pass writes nothing and the same
  // models come back pending on the next sweep. What is new is that the failure
  // is now a harness_runs row instead of a bare `return 0` — the audit's
  // clearest silent-failure case was that this batch could re-burn every ten
  // minutes forever with nothing anywhere saying so.
  //
  // The one repair turn is what replaced the caller's old salvage pass. That
  // pass re-keyed a display-named reply by normalizing ids and names, which
  // rescued the batch AND hid the fact that the model had not answered the
  // question — so the model was never told, never corrected, and the fitness
  // matrix never saw it. Re-asking once with the ids quoted fixes the same
  // replies and is honest about the ones it cannot.
  onFailure: 'null',

  // NO WIDENING, deliberately. A stronger model writes a better sentence, which
  // is quality the prompt already asks for — it does not do anything MORE here.
  // Batch size is the caller's argument and widening must never reach it.

  // Blurbs are persisted and shown in a picker, so the output is redacted rather
  // than merely flagged. The rules are narrowed to the two that can fire on a
  // sentence about a model: a catalog line cannot claim a tool ran or invent an
  // outage, and running those rules would only add noise to `guard_findings`,
  // which the fitness page reads as a per-model confabulation rate.
  guard: { rules: ['secret_leak', 'pii_leak'], redact: true },

  temperature: 0.4,

  evals: [
    {
      name: 'a full batch of three',
      input: {
        orgName: 'Outcrop Labs',
        models: [
          { id: 'pl-main', name: 'Qwen: Qwen3 14B', description: 'A general-purpose model with strong reasoning for its size.' },
          { id: 'pl-fast', name: 'Meta: Llama 3.1 8B Instruct', description: 'A small, fast instruction-tuned model.' },
          { id: 'pl-code', name: 'Qwen: Qwen2.5 Coder 32B', description: 'A code-specialized model for completion and review.' },
        ],
      },
      check: (value) => checkBatch(['pl-main', 'pl-fast', 'pl-code'], value),
    },
    {
      // A one-entry batch is where a small model most often abandons the shape
      // entirely and answers with a bare sentence, or with
      // `{"description": "..."}` — the key it was told to use is the assertion.
      name: 'a batch of one still comes back keyed by id',
      input: {
        orgName: 'Outcrop Labs',
        models: [{ id: 'pl-vision', name: 'Qwen: Qwen2.5 VL 7B', description: 'A vision-language model that reads images and documents.' }],
      },
      check: (value) => checkBatch(['pl-vision'], value) ?? (value['pl-vision'] ? null : "the only id in the batch, 'pl-vision', has no description"),
    },
    {
      // Ids carry dots, digits and colons. A model that normalizes them writes
      // keys the caller will never look up, and the failure is invisible without
      // this case.
      name: 'ids with punctuation survive verbatim',
      input: {
        orgName: 'Outcrop Labs',
        models: [
          { id: 'llama-3.1-8b', name: 'Meta: Llama 3.1 8B', description: 'A small instruction-tuned model.' },
          { id: 'mixtral-8x7b-v0.1', name: 'Mistral: Mixtral 8x7B', description: 'A sparse mixture-of-experts model.' },
        ],
      },
      check: (value) => checkBatch(['llama-3.1-8b', 'mixtral-8x7b-v0.1'], value),
    },
  ],
})
