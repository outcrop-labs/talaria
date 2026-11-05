// The modal's save wiring, pinned at the source: the pure helpers are tested
// in lib/agent-design.test.ts, but a modal that stopped calling them stayed
// green there (that is how a close-path keepalive and a restore-on-409
// shipped once). No DOM in this suite, so this reads what the component
// calls.
import { readFileSync } from 'node:fs'
import { describe, expect, it } from 'vitest'

const src = readFileSync(new URL('./CreateAgentModal.svelte', import.meta.url), 'utf8')
const code = src
  .split('\n')
  .filter((l) => !l.trim().startsWith('//'))
  .join('\n')

describe('CreateAgentModal save wiring', () => {
  it('closing saves as an ordinary save; only pagehide rides keepalive', () => {
    expect(code).toContain("window.addEventListener('pagehide', flushAway)")
    const cleanup = code.slice(code.indexOf("window.removeEventListener('pagehide', flushAway)"))
    expect(cleanup.slice(0, 200)).toContain('void flush()')
    expect(code).not.toMatch(/flush\(true\)/)
    const away = code.slice(code.indexOf('const flushAway'), code.indexOf('function restoreUnsaved'))
    expect(away).toContain('{ keepalive: true }')
    expect(away).toContain('stashUnsaved(')
    const save = code.slice(code.indexOf('const save = '), code.indexOf('const flush = '))
    expect(save).not.toContain('keepalive')
  })

  it('a kept edit is offered once, at open, never from a reseed', () => {
    const calls = code.match(/restoreUnsaved\(uid,/g) ?? []
    expect(calls).toHaveLength(1)
    const at = code.indexOf('restoreUnsaved(uid,')
    const guard = code.lastIndexOf('stashChecked = true', at)
    expect(guard).toBeGreaterThan(-1)
    expect(at - guard).toBeLessThan(120)
    // The 409 reseed path never reaches it.
    const reseed = code.slice(code.indexOf('if (seededFrom === null) {'), code.indexOf('if (!d || key === seededFrom) return'))
    expect(reseed).not.toContain('restoreUnsaved')
  })

  it('Create names its design before the flush and stops when the flush fails', () => {
    const create = code.slice(code.indexOf('const create = async'))
    const named = create.indexOf('const designId = design?.id')
    const flushed = create.indexOf('if (!(await flush()))')
    expect(named).toBeGreaterThan(-1)
    expect(flushed).toBeGreaterThan(named)
    expect(create).toContain('...(designId ? { designId } : {})')
  })
})
