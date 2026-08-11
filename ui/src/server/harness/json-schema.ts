// THE HARNESS'S OWN SCHEMA, PUT ON THE WIRE.
//
// WHAT WAS WRONG. Every JSON harness declares a zod schema, and Talaria used it
// in exactly one place: `safeParse`, AFTER the reply came back. What went OUT
// was `response_format: { type: 'json_object' }` — "some JSON, shape
// unspecified" — plus a sentence of prose describing the shape. So the contract
// was enforced by rejecting bad answers rather than by preventing them, and two
// separate failures followed from it:
//
//   ANTHROPIC 400s ON IT OUTRIGHT. Its OpenAI-compatible layer accepts only
//   `response_format.type: 'json_schema'`, so every structured call to a Claude
//   model failed at the protocol — and the fitness suite scored that as the
//   MODEL failing to hold a contract. Nine of twenty-six harnesses read 0% on
//   claude-haiku while every text harness read 100%.
//
//   EVERYWHERE ELSE IT UNDER-CONSTRAINS. `json_object` guarantees syntax and
//   says nothing about keys or types, which is precisely the blurb-writer bug
//   this repo already found the hard way: a reply keyed by tidied-up display
//   names parsed fine, wrote zero blurbs, and recorded a perfect contract.
//   A schema on the wire makes that reply unrepresentable rather than
//   detectable after the fact.
//
// ── STRICT IS PER-SCHEMA, NOT A GLOBAL SWITCH ────────────────────────────────
//
// `strict: true` is the mode that guarantees conformance, and providers only
// accept it for schemas whose objects have fixed keys and forbid extras. Some
// harnesses cannot satisfy that and are not wrong to exist: `blurb-writer` is
// `z.record(z.string(), z.string())` — an open-keyed map, by design, because the
// keys are the caller's vendor ids. Asking for strict there is a 400.
//
// So this module decides per schema, and says which it got. A non-strict
// `json_schema` is still enormously better than `json_object`: the provider
// sees the types, and `output.verify` remains the backstop for the relational
// half a schema cannot state.
import { z } from 'zod'

/** A schema ready for `response_format.json_schema`. */
export interface WireSchema {
  /** Required by the wire format. Derived from the harness id, which is stable
   *  and unique, with the characters providers reject removed. */
  name: string
  schema: Record<string, unknown>
  /** Whether the provider may be asked to GUARANTEE conformance. False means
   *  the schema is legal JSON Schema but not strict-eligible — see the header. */
  strict: boolean
}

/** Providers accept `^[a-zA-Z0-9_-]+$` for the schema name; harness ids carry
 *  `:` (`muse:ticket`) and `/` never appears but is cheap to cover. */
const wireName = (harnessId: string): string => harnessId.replace(/[^a-zA-Z0-9_-]/g, '_').slice(0, 64) || 'response'

const isObject = (v: unknown): v is Record<string, unknown> => typeof v === 'object' && v !== null && !Array.isArray(v)

/** THE KEYWORDS THAT GO ON THE WIRE — structure only, never validation.
 *
 *  Providers implement different subsets of JSON Schema and reject what they do
 *  not know, so a schema rendered faithfully from zod is a 400 waiting to
 *  happen. Anthropic found the first one for us:
 *
 *    response_format.json_schema.schema: For 'array' type, 'minItems' values
 *    other than 0 or 1 are not supported (got: [2, 5])
 *
 *  — from a `z.array(...).min(2).max(5)` in the `json-strict` probe. Every
 *  provider has a list like that and none of them agree on it.
 *
 *  THE DIVISION OF LABOUR SETTLES IT. The wire schema exists to SHAPE DECODING:
 *  which keys, of what types, nested how. `output.schema` — the same zod object,
 *  on the way back — remains the validator, and it enforces every constraint
 *  dropped here. So sending `minItems: 2` buys nothing (a reply with one item is
 *  rejected on parse either way) and risks losing the entire call to a 400.
 *
 *  An allowlist rather than a blocklist, because the failure directions are not
 *  symmetric: an unknown keyword we forgot to drop is a 400 on every call a
 *  harness makes, and an unknown keyword we drop by accident is a slightly
 *  looser hint to the decoder. */
const WIRE_KEYWORDS: ReadonlySet<string> = new Set([
  'type',
  'properties',
  'required',
  'additionalProperties',
  'items',
  'prefixItems',
  'enum',
  'const',
  'anyOf',
  'oneOf',
  'allOf',
  'not',
  '$defs',
  '$ref',
  // Kept because it is INSTRUCTION, not validation: a field description is one
  // of the more effective ways to get the right value into the right key.
  'description',
  'title',
])

/** WHAT MAKES A SCHEMA STRICT-ELIGIBLE, walked rather than guessed.
 *
 *  Every object node must forbid extra properties and require every property it
 *  declares. A node that allows open keys (`additionalProperties` absent, true,
 *  or a schema) is the map case and cannot be strict. Composition keywords are
 *  walked into, because an eligible union of ineligible members is not a thing.
 *
 *  Returns false rather than throwing on anything it does not recognize: the
 *  cost of a false negative is one non-strict request, and the cost of a false
 *  positive is a 400 on every call the harness makes. */
export function strictEligible(node: unknown, depth = 0): boolean {
  if (depth > 12 || !isObject(node)) return false

  for (const key of ['anyOf', 'oneOf', 'allOf'] as const) {
    const branch = node[key]
    if (Array.isArray(branch)) return branch.every((b) => strictEligible(b, depth + 1))
  }
  if (isObject(node.$defs) && !Object.values(node.$defs).every((d) => strictEligible(d, depth + 1))) return false

  const type = node.type
  if (type === 'array') return node.items === undefined || strictEligible(node.items, depth + 1)
  if (type !== 'object') return true

  if (node.additionalProperties !== false) return false
  const props = node.properties
  if (!isObject(props)) return false
  const required = Array.isArray(node.required) ? node.required : []
  if (Object.keys(props).some((p) => !required.includes(p))) return false
  return Object.values(props).every((p) => strictEligible(p, depth + 1))
}

/** Tidy the emitted schema for the wire. It does exactly two things, and the
 *  list of things it REFUSES to do is the important half.
 *
 *  It keeps only the structural keywords (`WIRE_KEYWORDS`) and closes a
 *  fixed-key object that came out open.
 *
 *  IT NEVER TOUCHES `required`, and an earlier draft of this file did. Strict
 *  mode wants every property listed there, so it is tempting to add the missing
 *  ones — but a property is missing from `required` because the harness declared
 *  it OPTIONAL, and forcing it would demand a value the model may not have.
 *  Worse, the conventional way to keep it optional under strict is a union with
 *  null, and `z.optional()` rejects null: we would have rewritten the contract
 *  into one its own parser fails. A schema with optional fields is simply not
 *  strict-eligible, and sending it non-strict is the correct, lossless answer.
 *
 *  It likewise never closes an open MAP (`z.record` emits
 *  `additionalProperties: { type: 'string' }` and means it) — same rule: the
 *  wire mode adapts to the declared contract, never the other way round. */
function forWire(node: unknown, depth = 0): unknown {
  if (depth > 12 || !isObject(node)) return node
  // Structure only — see `WIRE_KEYWORDS`. This is also what drops `$schema`, a
  // document annotation providers have no use for and some reject at the root.
  const out: Record<string, unknown> = Object.fromEntries(Object.entries(node).filter(([k]) => WIRE_KEYWORDS.has(k)))

  for (const key of ['anyOf', 'oneOf', 'allOf'] as const) {
    if (Array.isArray(out[key])) out[key] = (out[key] as unknown[]).map((b) => forWire(b, depth + 1))
  }
  if (isObject(out.$defs)) out.$defs = Object.fromEntries(Object.entries(out.$defs).map(([k, v]) => [k, forWire(v, depth + 1)]))
  if (out.type === 'array' && out.items !== undefined) out.items = forWire(out.items, depth + 1)

  if (out.type === 'object' && isObject(out.properties)) {
    out.properties = Object.fromEntries(Object.entries(out.properties).map(([k, v]) => [k, forWire(v, depth + 1)]))
    if (out.additionalProperties === undefined) out.additionalProperties = false
  }
  return out
}

/** THE SHAPE, WRITTEN OUT FOR THE MODEL TO READ.
 *
 *  WHY THIS EXISTS. Every structured call carried one sentence — "Reply with
 *  exactly one JSON value and nothing else" — and never said WHAT SHAPE. A
 *  frontier model infers it from the surrounding prose and looks fine, which is
 *  exactly why nobody noticed: the harness was leaning on the model to do work
 *  the harness already had the answer to. A 7-14B model does not infer it, and
 *  the whole premise of this layer is that it should not have to.
 *
 *  So the schema the harness already declares is rendered into the prompt as a
 *  compact shape:
 *
 *      {"verdict": "pass" | "fail" | "escalate", "summary": string}
 *
 *  Not raw JSON Schema — that is verbose, and a `$ref`-heavy document is worse
 *  prompt than no document. This is the shape a person would sketch, which is
 *  what a small model can actually follow.
 *
 *  IT IS BELT AND BRACES, NOT A FALLBACK. It goes out even when
 *  `response_format` constrains decoding, for the reason the runner already
 *  gives about the anchor: a provider can drop the parameter, and the prompt
 *  survives it. Bounded, because a prompt is not a schema document — a shape
 *  that will not fit is omitted rather than truncated into something misleading.
 *
 *  Returns null when there is nothing useful to say. */
export function promptShape(schema: Record<string, unknown>, budget = 600): string | null {
  const render = (node: unknown, depth: number): string => {
    if (depth > 4 || !isObject(node)) return 'value'
    if (Array.isArray(node.enum)) return node.enum.map((v) => JSON.stringify(v)).join(' | ')
    if ('const' in node) return JSON.stringify(node.const)
    for (const key of ['anyOf', 'oneOf'] as const) {
      const branch = node[key]
      if (Array.isArray(branch)) return [...new Set(branch.map((b) => render(b, depth + 1)))].join(' | ')
    }
    if (node.type === 'array') return `[${render(node.items, depth + 1)}, …]`
    if (node.type === 'object') {
      const props = isObject(node.properties) ? Object.entries(node.properties) : []
      if (props.length === 0) {
        // An open map — `z.record`. Say so rather than printing `{}`, which
        // reads as "an empty object" and is the opposite of what it means.
        const value = node.additionalProperties
        return isObject(value) ? `{"<key>": ${render(value, depth + 1)}, …}` : '{…}'
      }
      const required = Array.isArray(node.required) ? node.required : []
      const body = props.map(([k, v]) => `${JSON.stringify(k)}${required.includes(k) ? '' : '?'}: ${render(v, depth + 1)}`).join(', ')
      return `{${body}}`
    }
    return typeof node.type === 'string' ? node.type : 'value'
  }

  const shape = render(schema, 0)
  if (shape === 'value' || shape === '{…}' || shape.length > budget) return null
  return shape
}

/** The harness's schema, ready for the wire — or null when this build cannot
 *  express it, in which case the caller falls back to `json_object` and the
 *  prompt anchor rather than sending something a provider will reject.
 *
 *  NEVER THROWS. It runs on the hot path of every structured call, and a schema
 *  zod cannot render is a reason to send a weaker request, never a reason to
 *  fail a run that would otherwise have worked. */
export function wireSchemaOf(harnessId: string, schema: z.ZodType<unknown>): WireSchema | null {
  let raw: unknown
  try {
    // `io: 'input'`, and the first draft of this file said 'output' with a
    // confident comment explaining why. It was backwards, and it cost five of
    // the nine JSON harnesses.
    //
    // A `.transform()` runs AFTER parsing, so the shape the MODEL emits is the
    // INPUT side — the output side is what Talaria holds once the transform has
    // run, which the model never sees and cannot be asked for. zod knows this:
    // asked for the output side of a transformed schema it can say nothing, and
    // with `unrepresentable: 'any'` it renders the whole thing as `{}`. That is
    // a legal JSON Schema meaning "any value", and Anthropic rejects it outright
    // ("Empty schema ({}) that accepts any JSON value is not supported"), so
    // every harness with a transform 400'd on every call.
    //
    // Measured across the registry: `io: 'output'` renders 5 of 9 as empty,
    // `io: 'input'` renders all 9. `json-schema.test.ts` holds that.
    raw = z.toJSONSchema(schema, { io: 'input', unrepresentable: 'any' })
  } catch {
    return null
  }
  if (!isObject(raw)) return null
  const wire = forWire(raw)
  // AN EMPTY SCHEMA IS NOT A SCHEMA. It is legal JSON Schema meaning "any
  // value", it constrains nothing, and Anthropic refuses it. Returning null
  // sends the request without a `response_format` and lets the prompt anchor
  // carry the ask — weaker, and enormously better than a 400.
  if (!isObject(wire) || Object.keys(wire).length === 0) return null
  return { name: wireName(harnessId), schema: wire, strict: strictEligible(wire) }
}
