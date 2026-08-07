// The ONE structured-output parser. Every place Talaria asks a model for JSON
// and reads it back comes through here.
//
// WHY THIS FILE EXISTS
//   Six different extractors grew up in this codebase, and three of them are the
//   same non-solution: take everything from the first `{` to the last `}` and
//   hand it to JSON.parse — `judge.ts` (greedy regex), `model-info.ts` (greedy
//   regex), `muse.svelte.ts` (greedy regex, three times, client-side), and
//   `inbox-focus-assistant.ts` (indexOf/lastIndexOf, same idea spelled by hand).
//   That is not a JSON scanner. It is a substring, and it was verified by
//   EXECUTION to fail on three shapes a 14B model emits constantly:
//
//     1. a fenced object followed by prose that mentions a `{placeholder}`
//     2. a preamble, then two objects (the model "shows its work", then answers)
//     3. an object, then a bulleted explanation containing a brace
//
//   In all three the greedy span swallows the trailing prose and the parse dies.
//   A brace-BALANCING scan that knows what a string literal is handles all three
//   and costs one pass. `research.ts:186` already half-learned this — a
//   non-greedy regex plus a line-based fallback — but there was nowhere to put
//   the lesson, so it never propagated. This is the somewhere.
//
//   The failure mattered more than it looks, because each of the six sites
//   answers a failed parse DIFFERENTLY and silently: the judge escalates to a
//   human (a weak judge model is therefore a notification storm), the blurb
//   writer returns 0 and re-burns the same batch every ten minutes forever,
//   Muse returns null so the button just does nothing. One extractor, one
//   parse result, and the caller declares what a failure means.
//
// WHAT THIS MODULE IS NOT
//   It is not a JSON5 parser and it does not guess at truncation. A value whose
//   braces never close is a FAILURE, reported as one, with a repair instruction
//   the caller can hand straight back to the model. Inventing the missing tail
//   is how a harness returns a confidently wrong answer instead of an error.
//
//   Pure by construction: zod and nothing else. No DB, no gateway, no settings.
//   It must stay that way — this is the module the eval suite leans on hardest,
//   and a test that needs setup is a test nobody runs.
import type { z } from 'zod'

// ── Scanning ─────────────────────────────────────────────────────────────────

/** Where the value opened at `start` closes, or -1 if it never does.
 *
 *  String-literal and escape aware, which is the entire point: `{"note": "}"}`
 *  and `{"q": "he said \"hi\""}` are one complete value each, and every
 *  "first brace to last brace" extractor in the tree gets both wrong.
 *
 *  Depth is counted across both bracket families rather than kept on a stack.
 *  A crossed pair (`{"a": 1]`) therefore reads as a complete-but-bogus
 *  candidate; JSON.parse adjudicates it a moment later and the caller moves on
 *  to the next candidate, which is the behaviour we want anyway. */
function balancedEnd(text: string, start: number): number {
  let depth = 0
  let inString = false
  let escaped = false
  for (let i = start; i < text.length; i++) {
    const ch = text.charAt(i)
    if (inString) {
      if (escaped) escaped = false
      else if (ch === '\\') escaped = true
      else if (ch === '"') inString = false
      continue
    }
    if (ch === '"') inString = true
    else if (ch === '{' || ch === '[') depth++
    else if (ch === '}' || ch === ']') {
      depth--
      if (depth <= 0) return i
    }
  }
  return -1
}

/** Every complete `{`/`[`-rooted span in `text`, outermost first, in order.
 *
 *  Stops dead at the first opener that never closes. That looks lossy — there
 *  could be a good object further down — but it is the truncation guarantee:
 *  once an opener is unterminated, everything after it is INSIDE an unfinished
 *  value, so anything we found there would be a fragment presented as an
 *  answer. A cut-off response has to fail. */
function* balancedSpans(text: string): Generator<string> {
  let i = 0
  while (i < text.length) {
    const ch = text.charAt(i)
    if (ch !== '{' && ch !== '[') {
      i++
      continue
    }
    const end = balancedEnd(text, i)
    if (end === -1) return
    yield text.slice(i, end + 1)
    i = end + 1
  }
}

/** Contents of every ``` / ~~~ fenced block, in order. An unclosed fence runs
 *  to the end of the text — models drop the closing fence constantly, usually
 *  on exactly the responses that were already near the token cap. */
function fencedBlocks(text: string): string[] {
  const blocks: string[] = []
  const re = /(?:^|\n)[ \t]*(?:`{3,}|~{3,})[^\n]*\n([\s\S]*?)(?:\n[ \t]*(?:`{3,}|~{3,})|$)/g
  for (const m of text.matchAll(re)) {
    const body = m[1]
    if (body) blocks.push(body)
  }
  return blocks
}

/** Fence delimiter lines removed, so a fenced value scans as ordinary text. A
 *  line that is only a fence marker is never part of a JSON value — JSON has no
 *  raw newlines inside strings, so this cannot cut into one. */
function stripFences(text: string): string {
  return text.replace(/^[ \t]*(?:`{3,}|~{3,})[^\n]*$/gm, '')
}

/** Candidate JSON spans, best-guess first.
 *
 *  Fenced blocks are yielded BEFORE the surrounding prose because a fence is
 *  the model explicitly saying "this is the payload". Without that ordering,
 *  "According to [1], here is the list: ```[\"a\",\"b\"]```" extracts `[1]` —
 *  a perfectly valid JSON array that happens to be a citation marker. Order by
 *  intent first, position second. */
function* candidates(text: string): Generator<string> {
  const seen = new Set<string>()
  const fresh = function* (source: string): Generator<string> {
    for (const span of balancedSpans(source)) {
      if (seen.has(span)) continue
      seen.add(span)
      yield span
    }
  }
  for (const block of fencedBlocks(text)) yield* fresh(block)
  yield* fresh(stripFences(text))
}

/** First complete, balanced JSON value in the text (`{` or `[` rooted), or null.
 *
 *  String-literal and escape aware, so a brace inside a string never closes the
 *  value. Markdown fences are stripped first, and fenced content is preferred
 *  over prose. If a candidate does not parse — a `{placeholder}` in an
 *  explanation, say — the scan continues to LATER candidates rather than
 *  giving up, which is what turns "the model rambled first" from a failure into
 *  a non-event.
 *
 *  Viability is judged by JSON.parse of the span OR of `relaxJson(span)`, so a
 *  trailing comma in the real answer never causes the scanner to walk past it
 *  and return some decorative brace group from the prose instead.
 *
 *  Returns the span EXACTLY as the model wrote it, not the relaxed rewrite:
 *  callers that log a failure should log what actually came back. */
export function extractJson(text: string): string | null {
  for (const span of candidates(text)) {
    if (tryParse(span).ok) return span
  }
  return null
}

// ── Relaxation ───────────────────────────────────────────────────────────────

const CURLY_DOUBLE = new Set(['“', '”'])
const CURLY_SINGLE = new Set(['‘', '’'])
/** Non-space characters that can only precede an OPENING string delimiter. */
const BEFORE_OPEN = new Set(['{', '[', ':', ','])
/** Non-space characters that can only follow a CLOSING string delimiter. */
const AFTER_CLOSE = new Set(['}', ']', ':', ','])

const nextNonSpace = (s: string, from: number): string => {
  for (let i = from; i < s.length; i++) {
    const ch = s.charAt(i)
    if (!/\s/.test(ch)) return ch
  }
  return ''
}
const prevNonSpace = (s: string, from: number): string => {
  for (let i = from; i >= 0; i--) {
    const ch = s.charAt(i)
    if (!/\s/.test(ch)) return ch
  }
  return ''
}

/** Curly quotes sitting where JSON demands a delimiter become straight quotes.
 *
 *  Position, not identity, is the test — a model that types `{“verdict”: “pass”}`
 *  puts its curly quotes exactly where `"` belongs, while the apostrophe in
 *  `"the model’s answer"` has letters on both sides and is left alone. A
 *  blanket replace would corrupt prose content; this only touches quotes that
 *  are structurally quotes. */
function straightenQuotes(raw: string): string {
  let out = ''
  for (let i = 0; i < raw.length; i++) {
    const ch = raw.charAt(i)
    if (!CURLY_DOUBLE.has(ch) && !CURLY_SINGLE.has(ch)) {
      out += ch
      continue
    }
    const before = prevNonSpace(raw, i - 1)
    const after = nextNonSpace(raw, i + 1)
    const structural = before === '' || after === '' || BEFORE_OPEN.has(before) || AFTER_CLOSE.has(after)
    out += structural ? '"' : ch
  }
  return out
}

/** Bare literals JSON has no word for, mapped to null. `undefined` is in the
 *  list because a model imitating JS emits it about as often as NaN. */
const BARE_LITERALS = ['-Infinity', '+Infinity', 'Infinity', 'NaN', 'undefined']
const isWordChar = (ch: string): boolean => ch !== '' && /[\w$]/.test(ch)

/** Tolerant repair of the shapes small models actually emit:
 *    - a trailing comma before `}` or `]`
 *    - curly quotes used as string delimiters
 *    - bare NaN / Infinity / undefined, which JSON has no literal for
 *    - a raw newline or tab inside a string value
 *
 *  The last one is safe rather than optimistic: a control character inside a
 *  string is ALWAYS invalid JSON, and by the time this runs the extractor has
 *  already proved the value's strings are balanced, so escaping it can only be
 *  a repair. Everything here is string-literal aware, so a `,]` or the word
 *  Infinity inside a string value survives untouched.
 *
 *  Deliberately NOT attempted: truncation. A value the model never finished is
 *  a failure with a repair prompt attached, not a shape to guess at. Also not
 *  attempted: single-quoted strings and // comments — both need real
 *  disambiguation (apostrophes, URLs) and neither shows up often enough to
 *  justify the ambiguity. */
export function relaxJson(raw: string): string {
  const src = straightenQuotes(raw)
  let out = ''
  let inString = false
  let escaped = false
  for (let i = 0; i < src.length; i++) {
    const ch = src.charAt(i)
    if (inString) {
      if (escaped) {
        escaped = false
        out += ch
      } else if (ch === '\\') {
        escaped = true
        out += ch
      } else if (ch === '"') {
        inString = false
        out += ch
      } else if (ch === '\n') out += '\\n'
      else if (ch === '\r') out += '\\r'
      else if (ch === '\t') out += '\\t'
      else out += ch
      continue
    }
    if (ch === '"') {
      inString = true
      out += ch
      continue
    }
    if (ch === ',') {
      const next = nextNonSpace(src, i + 1)
      if (next === '}' || next === ']') continue
      out += ch
      continue
    }
    const literal = BARE_LITERALS.find(
      (lit) => src.startsWith(lit, i) && !isWordChar(src.charAt(i + lit.length)) && !isWordChar(src.charAt(i - 1)),
    )
    if (literal) {
      out += 'null'
      i += literal.length - 1
      continue
    }
    out += ch
  }
  return out
}

// ── Parsing ──────────────────────────────────────────────────────────────────

export type ParseResult<T> = { ok: true; value: T } | { ok: false; error: string; raw: string | null }

type Parsed = { ok: true; value: unknown } | { ok: false; reason: string }

/** JSON.parse, then one relaxed retry. Two attempts, never more: if the value
 *  survives neither, the model has to be asked again — that is what
 *  `repairPrompt` is for, and it works far better than a third heuristic. */
function tryParse(span: string): Parsed {
  try {
    return { ok: true, value: JSON.parse(span) as unknown }
  } catch (strict) {
    try {
      return { ok: true, value: JSON.parse(relaxJson(span)) as unknown }
    } catch {
      return { ok: false, reason: strict instanceof Error ? strict.message : 'invalid JSON' }
    }
  }
}

const typeName = (v: unknown): string =>
  v === null ? 'null' : Array.isArray(v) ? 'array' : v === undefined ? 'nothing' : typeof v

/** `issues[0]`, `plan.steps[2].title` — the path a human (or a model) can read
 *  back against its own output. */
function pathLabel(path: readonly PropertyKey[]): string {
  return path.reduce<string>((acc, key) => {
    if (typeof key === 'number') return `${acc}[${key}]`
    return acc ? `${acc}.${String(key)}` : String(key)
  }, '')
}

function valueAt(root: unknown, path: readonly PropertyKey[]): unknown {
  let cur: unknown = root
  for (const key of path) {
    if (cur === null || typeof cur !== 'object') return undefined
    cur = (cur as Record<PropertyKey, unknown>)[key]
  }
  return cur
}

/** Was the field simply not there? Zod reports a missing key and a wrong-typed
 *  key through the same issue codes, but they are completely different
 *  instructions to a model ("you forgot X" vs "X must be a number"), so we ask
 *  the input rather than the error. */
function isAbsent(root: unknown, path: readonly PropertyKey[]): boolean {
  if (path.length === 0) return root === undefined
  let cur: unknown = root
  for (let i = 0; i < path.length; i++) {
    const key = path[i]
    if (key === undefined) return false
    if (cur === null || typeof cur !== 'object') return false
    if (!(key in (cur as Record<PropertyKey, unknown>))) return true
    cur = (cur as Record<PropertyKey, unknown>)[key]
  }
  return cur === undefined
}

function describeIssue(issue: z.core.$ZodIssue, root: unknown): string {
  const label = pathLabel(issue.path)
  if (isAbsent(root, issue.path)) return `missing required field '${label}'`
  if (issue.code === 'invalid_type') {
    const got = typeName(valueAt(root, issue.path))
    return label ? `field '${label}' should be ${issue.expected}, got ${got}` : `expected ${issue.expected}, got ${got}`
  }
  if (issue.code === 'invalid_value') {
    const allowed = issue.values.map((v) => JSON.stringify(v)).join(' | ')
    const got = JSON.stringify(valueAt(root, issue.path))
    return label ? `field '${label}' must be one of ${allowed} (got ${got})` : `value must be one of ${allowed}`
  }
  if (issue.code === 'unrecognized_keys') return `unexpected field${issue.keys.length > 1 ? 's' : ''} ${issue.keys.map((k) => `'${k}'`).join(', ')}`
  return label ? `field '${label}': ${issue.message}` : issue.message
}

/** At most three problems. A small model handed a list of eleven fixes tends to
 *  rewrite the whole thing from scratch and reintroduce the first one. */
function describeIssues(issues: readonly z.core.$ZodIssue[], root: unknown): string {
  const named = issues.slice(0, 3).map((i) => describeIssue(i, root))
  return named.length ? named.join('; ') : 'the value did not match the required shape'
}

/**
 * Extract, parse (with one relax retry), then validate against `schema`.
 *
 * Candidates are tried in order and the FIRST one that both parses and
 * validates wins, so a model that narrates before answering, or drops a `[1]`
 * citation marker ahead of its real array, still gets read correctly. When
 * nothing validates, the error describes the first candidate that at least
 * parsed — the model's actual attempt, not some brace group in its preamble.
 *
 * `error` is written to be fed BACK TO THE MODEL verbatim (see `repairPrompt`):
 * it names the concrete problem — "expected object, got array", "missing
 * required field 'verdict'" — and never carries a stack trace, a schema dump,
 * or an internal type name. `raw` is what was extracted, for logs, and is null
 * when nothing complete was found at all.
 */
export function parseJson<T>(text: string, schema: z.ZodType<T>): ParseResult<T> {
  let firstValid: { raw: string; error: string } | null = null
  let firstBroken: { raw: string; reason: string } | null = null

  for (const span of candidates(text)) {
    const parsed = tryParse(span)
    if (!parsed.ok) {
      firstBroken ??= { raw: span, reason: parsed.reason }
      continue
    }
    const result = schema.safeParse(parsed.value)
    if (result.success) return { ok: true, value: result.data }
    firstValid ??= { raw: span, error: describeIssues(result.error.issues, parsed.value) }
  }

  if (firstValid) return { ok: false, error: firstValid.error, raw: firstValid.raw }
  if (firstBroken) return { ok: false, error: `the JSON could not be parsed (${firstBroken.reason})`, raw: firstBroken.raw }

  // Nothing complete was found. Distinguish "started a value and never finished
  // it" from "answered in prose", because those are opposite instructions: one
  // model needs to be told it was cut off, the other needs to be told to stop
  // talking. Guessing at the missing tail is not on the menu — see relaxJson.
  const truncated = /[{[]/.test(stripFences(text))
  return {
    ok: false,
    error: truncated
      ? 'the JSON value was opened but never closed - the response looks truncated'
      : 'no JSON object or array was found in the response',
    raw: null,
  }
}

/** The repair turn's user message. One place, so every harness repairs
 *  identically and a change to the wording is measurable across all of them at
 *  once. Short and imperative on purpose: this text is spent on the models
 *  least able to afford it, and "no prose" has to be the last thing read. */
export function repairPrompt(error: string): string {
  return [
    `That response could not be used: ${error}.`,
    '',
    'Send the corrected JSON value only - no explanation before or after it, no markdown code fence.',
  ].join('\n')
}
