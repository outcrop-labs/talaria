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
    const reseed = code.slice(code.indexOf('if (seededFrom === null) {'), code.indexOf('if (!d) {'))
    expect(reseed).not.toContain('restoreUnsaved')
  })

  it('Create names its design before the flush and stops when the flush fails', () => {
    const create = code.slice(code.indexOf('const create = async'))
    const named = create.indexOf('const designId = design?.id')
    const flushed = create.indexOf('if (!(await flush()))')
    expect(named).toBeGreaterThan(-1)
    expect(flushed).toBeGreaterThan(named)
    // The id that rides out is the one named before the flush, or the record
    // this form's own nameless save created during it — never one a flush
    // refetched from elsewhere.
    expect(create).toContain('const spend = designId ?? made')
    expect(create.indexOf('made = null')).toBeLessThan(flushed)
    expect(create).toContain('...(spend ? { designId: spend } : {})')
    const save = code.slice(code.indexOf('const save = '), code.indexOf('const flush = '))
    expect(save).toContain('if (!id) made = d.id')
  })

  it('a flush waits for a save already on the wire, not only a pending one', () => {
    // Review of #720, N3: past the debounce, a Create read no design id
    // while the manual start's save was still out, and left a copy behind.
    const save = code.slice(code.indexOf('const save = '), code.indexOf('const flush = '))
    expect(save).toContain('inFlight = sent')
    const flush = code.slice(code.indexOf('const flush = '), code.indexOf('const flushAway'))
    expect(flush).toContain('if (!saveTimer) return inFlight ?? Promise.resolve(true)')
  })

  it('a hand-back arriving over typing is offered, never taken in over it (NN12)', () => {
    // The reseed reads the SLOT; every verb acts on `design`, the form's own.
    expect(code).toContain('const design = $derived(slot && slot.id !== offered ? slot : null)')
    expect(code).toContain('const key = landingKey(slot)')
    // The branch for a design arriving in the slot after the first read.
    const arriving = code.slice(code.indexOf('if (d.id === offered || key === seededFrom) return'), code.indexOf('// A design that left for a hire'))
    const guard = arriving.indexOf('if (keepsTypedInput(d, seededFrom,')
    expect(guard).toBeGreaterThan(-1)
    // Before the form is reseeded from the arriving design, or marked seeded.
    expect(guard).toBeLessThan(arriving.indexOf('seededFrom = key'))
    expect(guard).toBeLessThan(arriving.indexOf('form = { ...EMPTY_FIELDS, ...(d.draft'))
    expect(arriving.slice(guard, guard + 160)).toContain('offered = d.id')
    // Taken in only by the person's choice, and offered where both steps show it.
    const open = code.slice(code.indexOf('const openOffered'), code.indexOf('const discardOffered'))
    expect(open).toContain('offered = null')
    expect(open).toContain('form = { ...EMPTY_FIELDS, ...(d.draft ?? {}) }')
    expect(code).toContain('data-testid="design-arrived"')
    expect(code.match(/\{@render returnedNotes\(\)\}/g) ?? []).toHaveLength(2)
  })

  it('a design that leaves the slot resets the form, never leaves it to be saved as a copy', () => {
    const gone = code.slice(code.indexOf('if (!d) {'), code.indexOf('if (key === seededFrom) return'))
    expect(gone).toContain('leftTheSlot(')
    expect(gone).toContain('resetForm(')
    const reset = code.slice(code.indexOf('function resetForm'), code.indexOf('function resetForm') + 400)
    expect(reset).toContain('clearTimeout(saveTimer)')
    expect(reset).toContain('form = { ...EMPTY_FIELDS }')
  })
})
