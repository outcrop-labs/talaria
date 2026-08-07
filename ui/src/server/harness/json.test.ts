import { describe, expect, it } from 'vitest'
import { z } from 'zod'
import { extractJson, parseJson, relaxJson, repairPrompt } from '@/server/harness/json'

// Everything here is pure: no gateway, no DB, no settings. That is the point of
// the module and it is the reason this file can carry the whole small-model
// corpus without a fixture harness.
//
// The three shapes at the top are not invented. They are the shapes the audit
// reproduced BY EXECUTION against the extractors that shipped, and the reason
// this module exists at all. LEGACY below re-runs those extractors on them so
// the regression is documented in code rather than in a document nobody opens.

/** Fenced object, then prose that mentions a brace. The single most common
 *  small-model response shape there is. */
const FENCED_THEN_PROSE = [
  'Here is the verdict:',
  '',
  '```json',
  '{"verdict": "pass", "summary": "The fix matches the ticket."}',
  '```',
  '',
  'Note: the {summary} field is deliberately short.',
].join('\n')

/** A preamble, then the model shows its work and answers twice. */
const PREAMBLE_TWO_OBJECTS = [
  'Let me think about this. My first read:',
  '{"verdict": "pass", "summary": "Looks complete."}',
  'On reflection I would also accept:',
  '{"verdict": "revise", "summary": "Missing a test."}',
].join('\n')

/** The answer, then a bulleted explanation containing a brace. */
const OBJECT_THEN_BULLETS = [
  '{"verdict": "revise", "summary": "No test covers the new branch."}',
  '',
  '- I checked the diff',
  '- The {issues} array is empty because nothing else stood out',
].join('\n')

// The two strategies this module replaces, transcribed from judge.ts:107 /
// model-info.ts:139 (greedy regex) and inbox-focus-assistant.ts:5
// (indexOf/lastIndexOf), so "these used to fail" is an assertion, not a claim.
const legacyGreedy = (text: string): unknown => {
  const m = text.match(/\{[\s\S]*\}/)
  if (!m) return null
  try {
    return JSON.parse(m[0]) as unknown
  } catch {
    return null
  }
}
const legacyIndexOf = (text: string): unknown => {
  const start = text.indexOf('{')
  const end = text.lastIndexOf('}')
  if (start === -1 || end <= start) return null
  try {
    return JSON.parse(text.slice(start, end + 1)) as unknown
  } catch {
    return null
  }
}

const verdictSchema = z.object({
  verdict: z.enum(['pass', 'revise', 'escalate']),
  summary: z.string(),
  issues: z.array(z.string()).default([]),
})

// ── extractJson ─────────────────────────────────────────────────────────────

describe('extractJson', () => {
  it('reads a fenced object followed by prose containing a brace', () => {
    expect(extractJson(FENCED_THEN_PROSE)).toBe('{"verdict": "pass", "summary": "The fix matches the ticket."}')
  })

  it('takes the FIRST object when a preamble is followed by two of them', () => {
    expect(extractJson(PREAMBLE_TWO_OBJECTS)).toBe('{"verdict": "pass", "summary": "Looks complete."}')
  })

  it('reads an object followed by bulleted prose containing a brace', () => {
    expect(extractJson(OBJECT_THEN_BULLETS)).toBe('{"verdict": "revise", "summary": "No test covers the new branch."}')
  })

  it('does not let a brace inside a string literal close the value', () => {
    const text = 'Sure: {"note": "use {curly} braces here", "ok": true} — hope that helps.'
    expect(extractJson(text)).toBe('{"note": "use {curly} braces here", "ok": true}')
  })

  it('does not let an escaped quote inside a string literal end it', () => {
    const text = '{"q": "he said \\"go}\\" and left", "n": 2}'
    expect(JSON.parse(extractJson(text) ?? 'null')).toEqual({ q: 'he said "go}" and left', n: 2 })
  })

  it('handles an array root', () => {
    expect(extractJson('Queries:\n["one", "two"]\nThat is all.')).toBe('["one", "two"]')
  })

  it('handles nesting several levels deep', () => {
    const value = { plan: { steps: [{ title: 'a', tags: ['x', 'y'] }] } }
    expect(extractJson(`prefix ${JSON.stringify(value)} suffix`)).toBe(JSON.stringify(value))
  })

  it('walks past a prose brace group that is not JSON', () => {
    const text = 'Replace {placeholder} with the value, like so: {"name": "talaria"}'
    expect(extractJson(text)).toBe('{"name": "talaria"}')
  })

  it('prefers a fenced block over an earlier citation marker that happens to be valid JSON', () => {
    const text = 'According to [1] the answer is:\n```json\n["alpha", "beta"]\n```'
    expect(extractJson(text)).toBe('["alpha", "beta"]')
  })

  it('handles a fenced block whose closing fence the model forgot', () => {
    expect(extractJson('```json\n{"a": 1}')).toBe('{"a": 1}')
  })

  it('handles tilde fences', () => {
    expect(extractJson('~~~\n{"a": 1}\n~~~')).toBe('{"a": 1}')
  })

  it('returns the span verbatim, not the relaxed rewrite', () => {
    expect(extractJson('{"a": 1,}')).toBe('{"a": 1,}')
  })

  it('returns null on prose with no JSON in it at all', () => {
    expect(extractJson('I am not sure I can answer that, sorry.')).toBeNull()
  })

  it('returns null on a truncated object rather than guessing at the tail', () => {
    expect(extractJson('{"verdict": "pass", "summary": "the fix looks')).toBeNull()
  })

  it('returns null on a truncated array', () => {
    expect(extractJson('["one", "two", "thr')).toBeNull()
  })

  it('does not mine a complete fragment out of a truncated value', () => {
    // The inner object is complete and parseable. Returning it would be a
    // fragment presented as the answer, which is exactly the failure mode.
    expect(extractJson('{"outer": 1, "inner": {"c": 2}')).toBeNull()
  })
})

// ── The regression this module exists to prevent ────────────────────────────

describe('the extractors this module replaces', () => {
  const shapes: Array<[string, string]> = [
    ['fenced + trailing prose with a brace', FENCED_THEN_PROSE],
    ['preamble + two objects', PREAMBLE_TWO_OBJECTS],
    ['object then bulleted prose with a brace', OBJECT_THEN_BULLETS],
  ]

  it.each(shapes)('greedy regex fails on %s where extractJson succeeds', (_label, text) => {
    expect(legacyGreedy(text)).toBeNull()
    expect(extractJson(text)).not.toBeNull()
  })

  it.each(shapes)('indexOf/lastIndexOf fails on %s where extractJson succeeds', (_label, text) => {
    expect(legacyIndexOf(text)).toBeNull()
    expect(extractJson(text)).not.toBeNull()
  })

  it('both legacy strategies did work on a clean object — which is why they shipped', () => {
    expect(legacyGreedy('{"a": 1}')).toEqual({ a: 1 })
    expect(legacyIndexOf('{"a": 1}')).toEqual({ a: 1 })
  })
})

// ── relaxJson ───────────────────────────────────────────────────────────────

const relaxed = (raw: string): unknown => JSON.parse(relaxJson(raw)) as unknown

describe('relaxJson', () => {
  it('drops a trailing comma in an object', () => {
    expect(relaxed('{"a": 1, "b": 2,}')).toEqual({ a: 1, b: 2 })
  })

  it('drops trailing commas in nested arrays and objects', () => {
    expect(relaxed('{"tags": ["x", "y",], "meta": {"n": 1,},}')).toEqual({ tags: ['x', 'y'], meta: { n: 1 } })
  })

  it('straightens curly quotes used as delimiters around keys and values', () => {
    expect(relaxed('{“verdict”: “pass”}')).toEqual({ verdict: 'pass' })
  })

  it('leaves a curly apostrophe inside a string value alone', () => {
    expect(relaxed('{"a": "the model’s answer", "b": 1,}')).toEqual({ a: 'the model’s answer', b: 1 })
  })

  it('leaves a trailing-comma lookalike inside a string value alone', () => {
    expect(relaxed('{"a": "x,]", "b": 1,}')).toEqual({ a: 'x,]', b: 1 })
  })

  it('maps bare NaN, Infinity, -Infinity and undefined to null', () => {
    expect(relaxed('{"a": NaN, "b": Infinity, "c": -Infinity, "d": undefined}')).toEqual({
      a: null,
      b: null,
      c: null,
      d: null,
    })
  })

  it('leaves the word Infinity inside a string value alone', () => {
    expect(relaxed('{"a": "to Infinity and beyond", "b": NaN}')).toEqual({ a: 'to Infinity and beyond', b: null })
  })

  it('escapes a raw newline inside a string value', () => {
    expect(relaxed('{"body": "line one\nline two"}')).toEqual({ body: 'line one\nline two' })
  })

  it('leaves already-valid JSON byte-identical', () => {
    const valid = '{"a": [1, 2], "b": {"c": "d"}}'
    expect(relaxJson(valid)).toBe(valid)
  })

  it('does not invent a closing brace for a truncated value', () => {
    expect(() => JSON.parse(relaxJson('{"a": 1'))).toThrow()
  })
})

// ── parseJson ───────────────────────────────────────────────────────────────

describe('parseJson', () => {
  it('validates the extracted value against the schema', () => {
    const result = parseJson(FENCED_THEN_PROSE, verdictSchema)
    expect(result).toEqual({ ok: true, value: { verdict: 'pass', summary: 'The fix matches the ticket.', issues: [] } })
  })

  it('recovers a trailing comma through the relax retry', () => {
    const result = parseJson('{"verdict": "pass", "summary": "ok", "issues": [],}', verdictSchema)
    expect(result.ok).toBe(true)
  })

  it('names the missing field', () => {
    const result = parseJson('{"summary": "ok"}', verdictSchema)
    expect(result.ok).toBe(false)
    if (result.ok) return
    expect(result.error).toContain("missing required field 'verdict'")
    expect(result.raw).toBe('{"summary": "ok"}')
  })

  it('names a nested missing field by path', () => {
    const schema = z.object({ plan: z.object({ title: z.string() }) })
    const result = parseJson('{"plan": {}}', schema)
    expect(result.ok).toBe(false)
    if (result.ok) return
    expect(result.error).toContain("missing required field 'plan.title'")
  })

  it('names the field AND both types on a type mismatch', () => {
    const result = parseJson('{"verdict": "pass", "summary": 42}', verdictSchema)
    expect(result.ok).toBe(false)
    if (result.ok) return
    expect(result.error).toContain("field 'summary' should be string, got number")
  })

  it('names the offending array element by index', () => {
    const result = parseJson('{"verdict": "pass", "summary": "ok", "issues": ["a", 7]}', verdictSchema)
    expect(result.ok).toBe(false)
    if (result.ok) return
    expect(result.error).toContain("field 'issues[1]' should be string, got number")
  })

  it('says "expected object, got array" when the root shape is wrong', () => {
    const result = parseJson('[{"verdict": "pass", "summary": "ok"}]', verdictSchema)
    expect(result.ok).toBe(false)
    if (result.ok) return
    expect(result.error).toContain('expected object, got array')
  })

  it('lists the allowed options when an enum value is wrong', () => {
    const result = parseJson('{"verdict": "maybe", "summary": "ok"}', verdictSchema)
    expect(result.ok).toBe(false)
    if (result.ok) return
    expect(result.error).toContain("field 'verdict' must be one of")
    expect(result.error).toContain('"revise"')
  })

  it('skips a parseable-but-wrong-shaped candidate for a later one that validates', () => {
    // "[1]" is a valid JSON array and a citation marker. The schema is what
    // tells them apart, which is why validation drives the candidate walk.
    const text = 'According to [1], the queries are:\n["alpha", "beta"]'
    const result = parseJson(text, z.array(z.string()))
    expect(result).toEqual({ ok: true, value: ['alpha', 'beta'] })
  })

  it('reports truncation as truncation, with no extracted raw', () => {
    const result = parseJson('{"verdict": "pass", "summary": "the fix looks', verdictSchema)
    expect(result).toEqual({
      ok: false,
      error: 'the JSON value was opened but never closed - the response looks truncated',
      raw: null,
    })
  })

  it('reports prose with no JSON at all, with no extracted raw', () => {
    const result = parseJson('I could not safely determine that.', verdictSchema)
    expect(result).toEqual({
      ok: false,
      error: 'no JSON object or array was found in the response',
      raw: null,
    })
  })

  it('caps the error at three problems so a small model does not rewrite everything', () => {
    const schema = z.object({ a: z.string(), b: z.string(), c: z.string(), d: z.string(), e: z.string() })
    const result = parseJson('{}', schema)
    expect(result.ok).toBe(false)
    if (result.ok) return
    expect(result.error.split(';')).toHaveLength(3)
  })

  it('never leaks a stack trace or an internal type name into the error', () => {
    const result = parseJson('{"verdict": "pass", "summary": 42}', verdictSchema)
    expect(result.ok).toBe(false)
    if (result.ok) return
    expect(result.error).not.toMatch(/ZodError|\bat .+:\d+|Invalid input/)
  })
})

// ── repairPrompt ────────────────────────────────────────────────────────────

describe('repairPrompt', () => {
  it('carries the concrete error and forbids everything but the value', () => {
    const prompt = repairPrompt("missing required field 'verdict'")
    expect(prompt).toContain("missing required field 'verdict'")
    expect(prompt).toMatch(/JSON value only/)
    expect(prompt).toMatch(/no markdown code fence/)
  })
})
