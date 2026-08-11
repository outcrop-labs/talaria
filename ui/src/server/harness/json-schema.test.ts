import { describe, expect, it } from 'vitest'
import { z } from 'zod'
import { promptShape, strictEligible, wireSchemaOf } from './json-schema'
import { builtinActivityHarnesses } from './registry'

// What goes ON THE WIRE for a structured harness. The bug this closes: Talaria
// declared a zod schema per JSON harness and used it only to reject bad answers
// afterwards, sending `response_format: { type: 'json_object' }` — which
// Anthropic 400s outright, and which under-constrains everywhere else.

describe('wireSchemaOf', () => {
  it('renders the shape the model must produce', () => {
    const wire = wireSchemaOf('judge', z.object({ verdict: z.enum(['pass', 'fail']), summary: z.string() }))

    expect(wire?.schema).toMatchObject({
      type: 'object',
      properties: { verdict: { type: 'string', enum: ['pass', 'fail'] }, summary: { type: 'string' } },
      required: ['verdict', 'summary'],
      additionalProperties: false,
    })
    expect(wire?.strict).toBe(true)
  })

  it('drops $schema, which providers have no use for and some reject', () => {
    const wire = wireSchemaOf('t', z.object({ a: z.string() }))

    expect(wire?.schema).not.toHaveProperty('$schema')
  })

  it('makes a name the wire format accepts out of a harness id', () => {
    // Ids carry ':' (`muse:ticket`); the field is `^[a-zA-Z0-9_-]+$`.
    expect(wireSchemaOf('muse:ticket', z.object({ a: z.string() }))?.name).toBe('muse_ticket')
  })

  it('sends an open-keyed map NON-strict rather than closing it', () => {
    // blurb-writer: `z.record(z.string(), z.string())`, open by design because
    // the keys are the caller's vendor ids. Strict would 400; closing it would
    // rewrite the contract the harness declared.
    const wire = wireSchemaOf('blurb-writer', z.record(z.string(), z.string()))

    expect(wire?.strict).toBe(false)
    expect(wire?.schema.additionalProperties).toEqual({ type: 'string' })
  })

  it('sends a schema with OPTIONAL fields non-strict, and never forces them required', () => {
    // THE HAZARD AN EARLIER DRAFT WALKED INTO. Strict wants every property in
    // `required`, so adding the missing ones is tempting — but a key is absent
    // from `required` because the harness declared it optional, and the
    // conventional strict workaround (a union with null) is rejected by
    // `z.optional()`. We would have rewritten the contract into one its own
    // parser fails.
    const wire = wireSchemaOf('t', z.object({ a: z.string(), b: z.string().optional() }))

    expect(wire?.schema.required).toEqual(['a'])
    expect(wire?.strict).toBe(false)
  })

  it('never throws on a schema it cannot render', () => {
    // It runs on the hot path of every structured call. A schema zod cannot
    // express is a reason to send a weaker request, never to fail a run.
    expect(() => wireSchemaOf('t', z.custom(() => true))).not.toThrow()
  })
})

describe('strictEligible', () => {
  it('refuses a nested object that is left open', () => {
    const open = { type: 'object', properties: { inner: { type: 'object' } }, required: ['inner'], additionalProperties: false }

    expect(strictEligible(open)).toBe(false)
  })

  it('walks arrays and unions rather than judging only the root', () => {
    const wire = wireSchemaOf('t', z.object({ rows: z.array(z.object({ id: z.string() })) }))

    expect(wire?.strict).toBe(true)
    expect(strictEligible({ anyOf: [{ type: 'string' }, { type: 'object' }] })).toBe(false)
  })
})

describe('every JSON harness Talaria ships', () => {
  const jsonHarnesses = builtinActivityHarnesses().filter((h) => h.outputKind === 'json')

  it('renders a schema for the wire — no JSON harness falls back to json_object', () => {
    // A harness that cannot render one would go back to "some JSON, shape
    // unspecified", which is the state this whole module exists to leave.
    expect(jsonHarnesses.length).toBeGreaterThan(0)
    for (const h of jsonHarnesses) {
      const wire = h.use((def) => (def.output.kind === 'json' ? wireSchemaOf(def.id, def.output.schema) : null))
      expect(wire, `${h.id} renders no wire schema`).not.toBeNull()
      expect(wire?.name, h.id).toMatch(/^[a-zA-Z0-9_-]+$/)
    }
  })

  it('records WHICH harnesses can be sent strict, so a schema change is visible here', () => {
    // Not an aspiration — a census. A harness moving between these lists is a
    // real change in how strongly the provider constrains its replies, and it
    // should fail here rather than be discovered from a 400 or a bad reply.
    const strict: string[] = []
    const loose: string[] = []
    for (const h of jsonHarnesses) {
      const wire = h.use((def) => (def.output.kind === 'json' ? wireSchemaOf(def.id, def.output.schema) : null))
      ;(wire?.strict ? strict : loose).push(h.id)
    }
    expect({ strict: strict.sort(), loose: loose.sort() }).toMatchSnapshot()
  })
})

describe('the provider subset', () => {
  it('drops validation keywords, keeping only structure', () => {
    // ANTHROPIC FOUND THIS ONE LIVE:
    //   response_format.json_schema.schema: For 'array' type, 'minItems' values
    //   other than 0 or 1 are not supported (got: [2, 5])
    // from a `z.array(...).min(2).max(5)` in the json-strict probe. Providers
    // implement different subsets and reject what they do not know, so a
    // faithfully rendered schema is a 400 waiting to happen.
    const wire = wireSchemaOf('t', z.object({ queries: z.array(z.string().min(4)).min(2).max(5) }))
    const queries = (wire?.schema.properties as Record<string, Record<string, unknown>>).queries!

    expect(queries).toMatchObject({ type: 'array', items: { type: 'string' } })
    expect(queries).not.toHaveProperty('minItems')
    expect(queries).not.toHaveProperty('maxItems')
    expect(queries.items).not.toHaveProperty('minLength')
  })

  it('keeps the constraint enforced where it belongs — on the way back', () => {
    // Nothing is lost by dropping them: the same zod object still parses the
    // reply, so a two-item minimum is enforced on `safeParse`. The wire schema
    // shapes decoding; it was never the validator.
    const schema = z.object({ queries: z.array(z.string()).min(2) })
    expect(schema.safeParse({ queries: ['one'] }).success).toBe(false)
    expect(schema.safeParse({ queries: ['one', 'two'] }).success).toBe(true)
  })

  it('keeps description and title, which are instruction rather than validation', () => {
    const wire = wireSchemaOf('t', z.object({ verdict: z.string().describe('pass or fail') }))

    expect((wire?.schema.properties as Record<string, Record<string, unknown>>).verdict?.description).toBe('pass or fail')
  })
})

describe('the shape a small model is told to produce', () => {
  it('writes the schema out as something a person would sketch', () => {
    // THE LAZINESS THIS ENDS. Every structured call carried one sentence —
    // "Reply with exactly one JSON value and nothing else" — and never said WHAT
    // SHAPE, while the harness one line away held the schema. A frontier model
    // infers it from the surrounding prose and looks fine, which is why nobody
    // noticed; a 7-14B model does not, and this layer exists so that difference
    // is engineered away rather than left to the model.
    const wire = wireSchemaOf('judge', z.object({ verdict: z.enum(['pass', 'revise']), summary: z.string(), issues: z.array(z.string()).optional() }))

    expect(promptShape(wire!.schema)).toBe('{"verdict": "pass" | "revise", "summary": string, "issues"?: [string, …]}')
  })

  it('marks optional keys, so a small model does not invent one to fill a slot', () => {
    const wire = wireSchemaOf('t', z.object({ a: z.string(), b: z.number().optional() }))
    expect(promptShape(wire!.schema)).toBe('{"a": string, "b"?: number}')
  })

  it('says an open map is a map rather than printing an empty object', () => {
    // `{}` reads as "an empty object", which is the opposite of what a
    // `z.record` means — and blurb-writer's whole contract is an open map.
    const wire = wireSchemaOf('blurb-writer', z.record(z.string(), z.string()))
    expect(promptShape(wire!.schema)).toBe('{"<key>": string, …}')
  })

  it('omits a shape too large to be prompt rather than truncating it', () => {
    // A truncated shape is worse than none: it reads as the whole contract and
    // is not one.
    const wide = z.object(Object.fromEntries(Array.from({ length: 80 }, (_, i) => [`field_${i}`, z.string()])))
    expect(promptShape(wireSchemaOf('t', wide)!.schema)).toBeNull()
  })
})
