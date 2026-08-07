import { describe, expect, it } from 'vitest'
import { readFileSync } from 'node:fs'
import { dirname, resolve } from 'node:path'
import { fileURLToPath } from 'node:url'
import { TALARIA_TOOLS } from './talaria-tools'

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

describe('the sandbox toolkit is a faithful copy of the real one', () => {
  const source = readFileSync(MCP_SOURCE, 'utf8')
  const real = realDescriptions(source)

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
})
