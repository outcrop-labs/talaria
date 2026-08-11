import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { TALARIA_TOOLS } from './talaria-tools'
import { backedToolNames } from './sandbox'

// THE LOCK ON THE COPY.
//
// `talaria-tools.ts` is a hand-copy of registrations that live in
// `mcp/src/index.ts`, because that module's body starts an MCP server and every
// handler reaches for Talaria's HTTP API — importing it from a benchmark would
// boot a server. A copy is the right call and a rotting copy is not, so this
// reads the real source and fails when the two disagree.
//
// WHAT IS COMPARED, and why it is the prefix rather than the whole string.
// Several real descriptions interpolate a shared clause (`${LIVE_ONLY}`,
// `${COMMENT_EXEMPTION}`, `${OFF_THE_TABLE}`) that only exists inside that
// module. The sandbox spells the meaning out in its own words after that point.
// So the assertion is: the sandbox description STARTS WITH the real literal text
// up to the first interpolation. That is the part a copy can be held to exactly,
// and it is where every hard rule lives ("you cannot move it back", "use ONLY
// when you genuinely cannot", "Not for status updates").
//
// IF THIS FAILS, the fix is to update `talaria-tools.ts` to match the toolkit —
// never to loosen the comparison. A benchmark measuring a model against tools
// Talaria does not have is a benchmark that flatters every candidate.

/** Resolved from THIS FILE, not from `process.cwd()`: vitest can be invoked
 *  from the repo root or from `ui/`, and a cwd-relative path silently points at
 *  a different tree depending on which. */
const MCP_SOURCE = resolve(dirname(fileURLToPath(import.meta.url)), '../../../../../mcp/src/index.ts')

/** Every `server.registerTool('name', { description: <string expr> …` in the
 *  source, with the description's leading literal segments concatenated and its
 *  first interpolation treated as the end of what a copy can promise. */
function realDescriptions(source: string): Map<string, string> {
  const out = new Map<string, string>()
  const re = /server\.registerTool\(\s*'([a-z_]+)',\s*\{\s*description:\s*/g
  let m: RegExpExecArray | null
  while ((m = re.exec(source))) {
    const literal = readStringExpression(source, re.lastIndex)
    if (literal !== null) out.set(m[1]!, literal)
  }
  return out
}

/** Read a JS string expression — `'a'`, `"a"`, a template, or several joined by
 *  `+` — starting at `from`. Stops at the first `${`, because that is where a
 *  static copy stops being able to promise anything. */
function readStringExpression(source: string, from: number): string | null {
  let i = from
  let out = ''
  for (;;) {
    while (i < source.length && /\s/.test(source[i]!)) i++
    const quote = source[i]
    if (quote !== "'" && quote !== '"' && quote !== '`') break
    i++
    let done = false
    while (i < source.length) {
      const ch = source[i]!
      if (ch === '\\') {
        // Only the escapes these descriptions actually use.
        const next = source[i + 1]!
        out += next === 'n' ? '\n' : next === 't' ? '\t' : next
        i += 2
        continue
      }
      if (ch === quote) {
        i++
        done = true
        break
      }
      if (quote === '`' && ch === '$' && source[i + 1] === '{') return out
      out += ch
      i++
    }
    if (!done) return null
    // A `+` continues the same expression; anything else ends it.
    let j = i
    while (j < source.length && /\s/.test(source[j]!)) j++
    if (source[j] !== '+') break
    i = j + 1
  }
  return out.length > 0 ? out : null
}

/** Every `server.registerTool('name', { … inputSchema: { a: …, b: … } … })`'s
 *  ARGUMENT KEYS, and which of them are required.
 *
 *  WHY THE NAMES AND NOT THE TYPES. A zod schema in another package cannot be
 *  evaluated here (that module boots an MCP server on import), and the parts of
 *  it a static reader can get exactly right are the keys and whether
 *  `.optional()` appears before the next key. That is also the part that was
 *  wrong: eight of sixteen tools carried invented argument names, so a model
 *  graded here learned a call production answers with a 400.
 *
 *  The scan is deliberately shallow — top-level keys of the `inputSchema` object
 *  literal, at brace depth 1 — because that is what an MCP input schema is. */
function realParameters(source: string): Map<string, { keys: string[]; required: string[] }> {
  const out = new Map<string, { keys: string[]; required: string[] }>()
  const re = /server\.registerTool\(\s*'([a-z_]+)',\s*\{/g
  let m: RegExpExecArray | null
  while ((m = re.exec(source))) {
    const at = source.indexOf('inputSchema:', m.index)
    // `inputSchema` must belong to THIS registration, not the next one's.
    const nextReg = source.indexOf('server.registerTool(', m.index + 1)
    if (at === -1 || (nextReg !== -1 && at > nextReg)) continue
    const open = source.indexOf('{', at)
    if (open === -1) continue
    const keys: string[] = []
    const required: string[] = []
    let depth = 0
    let i = open
    for (; i < source.length; i++) {
      const ch = source[i]!
      if (ch === '{' || ch === '[' || ch === '(') depth++
      else if (ch === '}' || ch === ']' || ch === ')') {
        depth--
        if (depth === 0) break
      } else if (depth === 1) {
        // A key at the top level of the schema object: `name: z.…`
        const key = /^([A-Za-z_$][\w$]*)\s*:/.exec(source.slice(i, i + 40))
        const prev = source.slice(0, i).trimEnd().slice(-1)
        if (key && (prev === '{' || prev === ',')) {
          keys.push(key[1]!)
          i += key[0].length - 1
          // Everything up to the next top-level comma is this key's expression.
          let j = i + 1
          let d = 0
          for (; j < source.length; j++) {
            const c = source[j]!
            if (c === '{' || c === '[' || c === '(') d++
            else if (c === '}' || c === ']' || c === ')') {
              if (d === 0) break
              d--
            } else if (c === ',' && d === 0) break
          }
          if (!source.slice(i, j).includes('.optional()')) required.push(key[1]!)
          i = j - 1
        }
      }
    }
    out.set(m[1]!, { keys, required })
  }
  return out
}

const paramsOf = (tool: (typeof TALARIA_TOOLS)[number]): { keys: string[]; required: string[] } => {
  const p = tool.parameters as { properties?: Record<string, unknown>; required?: string[] }
  return { keys: Object.keys(p.properties ?? {}), required: [...(p.required ?? [])] }
}

describe('the sandbox toolkit is a faithful copy of the real one', () => {
  const source = readFileSync(MCP_SOURCE, 'utf8')
  const real = realDescriptions(source)
  const realArgs = realParameters(source)

  it('reads the real registrations at all — a parser that matched nothing would pass everything', () => {
    // The guard on the guard. If `registerTool` is ever spelled differently this
    // file must fail loudly rather than quietly assert over an empty map.
    expect(real.size).toBeGreaterThan(30)
    expect(real.get('get_ticket')).toContain('Get a ticket in full')
  })

  it('offers only tools the toolkit actually registers', () => {
    // An invented tool is the worst failure available here: the model looks
    // capable in the benchmark and calls something that does not exist in
    // production.
    const missing = TALARIA_TOOLS.map((t) => t.name).filter((n) => !real.has(n))
    expect(missing).toEqual([])
  })

  it('carries each description verbatim, up to the first interpolation', () => {
    const drifted: string[] = []
    for (const tool of TALARIA_TOOLS) {
      const want = real.get(tool.name)
      if (!want) continue
      if (!tool.description.startsWith(want)) drifted.push(tool.name)
    }
    expect(drifted).toEqual([])
  })

  it('reads the real input schemas at all — the same guard, on the parameter scan', () => {
    expect(realArgs.size).toBeGreaterThan(30)
    expect(realArgs.get('add_time')?.keys).toEqual(['taskId', 'seconds'])
    expect(realArgs.get('message_user')?.keys).toEqual(['to', 'message'])
    // A tool with no arguments must read as none rather than as unparsed.
    expect(realArgs.get('list_boards')?.keys).toEqual([])
  })

  it('names every argument exactly as the toolkit does', () => {
    // THE FLATTERY THIS CLOSES. `comment(body)`, `add_time(minutes)`,
    // `read_channel(channel)`, `message_user(user, body)` — four of the eight
    // invented names the sandbox used to carry. A model that called them
    // "correctly" in the benchmark got a 400 in production, and the benchmark
    // called it competent. Never loosen this to make a tool pass: the fix is to
    // rename the sandbox's argument.
    const drifted: string[] = []
    for (const tool of TALARIA_TOOLS) {
      const want = realArgs.get(tool.name)
      if (!want) continue
      const got = paramsOf(tool)
      if (got.keys.join(',') !== want.keys.join(',')) drifted.push(`${tool.name}: sandbox has [${got.keys.join(', ')}], toolkit has [${want.keys.join(', ')}]`)
    }
    expect(drifted).toEqual([])
  })

  it('requires exactly what the toolkit requires', () => {
    // A sandbox that demands MORE fails a model for a call production accepts;
    // one that demands FEWER lets a model omit an argument the API rejects.
    const drifted: string[] = []
    for (const tool of TALARIA_TOOLS) {
      const want = realArgs.get(tool.name)
      if (!want) continue
      const got = paramsOf(tool)
      if ([...got.required].sort().join(',') !== [...want.required].sort().join(',')) {
        drifted.push(`${tool.name}: sandbox requires [${got.required.join(', ')}], toolkit requires [${want.required.join(', ')}]`)
      }
    }
    expect(drifted).toEqual([])
  })

  it('models EVERY tool the toolkit registers, and backs every one it models', () => {
    // COVERAGE IS THE POINT, and it used to be sixteen of forty-four. Twenty-
    // eight tools an org's agents use every day had no simulated backend, no
    // fixture, and no way for anyone to notice. `scripts/check-invariants.mjs`
    // enforces the same thing in CI; this is the unit-level statement of it, so
    // a new registration fails in the suite a developer runs first.
    const unmodelled = [...real.keys()].filter((n) => !TALARIA_TOOLS.some((t) => t.name === n))
    expect(unmodelled).toEqual([])
    const unbacked = TALARIA_TOOLS.map((t) => t.name).filter((n) => !backedToolNames().includes(n))
    expect(unbacked).toEqual([])
  })
})
