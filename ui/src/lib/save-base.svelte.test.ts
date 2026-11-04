// @vitest-environment runes
//
// No silent overwrite: an agent saves while a person's editor holds a buffer
// from the version before, and the person's next save must not wipe the
// agent's edit. The route half (`overwrites_unseen`) has its own Rust tests;
// these pin the editor half — what each answer of a guarded PUT does to the
// buffer's base and to the one conflict line, and that saves run in order.
import { describe, expect, it } from 'vitest'
import { baseAfterSave, conflictVerdict, guardedSaver, type SaveAnswer } from './save-base.svelte'

interface Row {
  updatedAt: string
  body: string
  title?: string
}
type Patch = { title?: string; body?: string; icon?: string }
type Sent = Patch & { baseUpdatedAt?: string }

/** A fake document route with the real rule: a body save naming a base the
 *  row has moved past, over a body that differs, is refused with the row. */
function fakeRoute(initial: Row) {
  let row = { ...initial }
  let tick = 0
  const sent: Sent[] = []
  const put = async (p: Sent): Promise<SaveAnswer<Row>> => {
    sent.push(p)
    await Promise.resolve()
    if (p.baseUpdatedAt !== undefined && p.body !== undefined && p.baseUpdatedAt !== row.updatedAt && p.body !== row.body) {
      return { conflict: { ...row } }
    }
    row = { ...row, ...(p.body !== undefined ? { body: p.body } : {}), ...(p.title !== undefined ? { title: p.title } : {}), updatedAt: `t${++tick}` }
    return { saved: { ...row } }
  }
  return {
    put,
    sent,
    get row() {
      return row
    },
    /** Someone else's save — an agent's edit_kb_doc, or an icon change. */
    external(patch: Partial<Row>) {
      row = { ...row, ...patch, updatedAt: `x${++tick}` }
    },
  }
}

const opened = { updatedAt: 't0', body: 'v1' }

describe('conflictVerdict', () => {
  it('retries when only metadata moved — the body is still the buffer’s base', () => {
    expect(conflictVerdict(opened, { updatedAt: 'x1', body: 'v1' })).toBe('retry')
  })
  it('is a conflict when someone else’s text is in the document', () => {
    expect(conflictVerdict(opened, { updatedAt: 'x1', body: 'agent text' })).toBe('conflict')
  })
})

describe('baseAfterSave', () => {
  it('a body save makes the saved row the base', () => {
    expect(baseAfterSave(opened, { updatedAt: 't1', body: 'mine' }, true)).toEqual({ updatedAt: 't1', body: 'mine' })
  })
  it('a save without a body advances the version only while the body is unchanged', () => {
    expect(baseAfterSave(opened, { updatedAt: 't1', body: 'v1' }, false)).toEqual({ updatedAt: 't1', body: 'v1' })
    // An agent's body landed before this icon save: the buffer is still on
    // v1, so the base must stay there and the next body save stay checked.
    expect(baseAfterSave(opened, { updatedAt: 't1', body: 'agent text' }, false)).toEqual(opened)
  })
})

describe('guardedSaver', () => {
  const setup = (initial: Row = opened) => {
    const route = fakeRoute(initial)
    let saver!: ReturnType<typeof guardedSaver<Row, Patch>>
    const stop = $effect.root(() => {
      saver = guardedSaver<Row, Patch>(route.put)
      saver.rebase(initial)
    })
    return { route, saver, stop }
  }

  it('opening a document an agent just created shows nothing', () => {
    const { saver, stop } = setup({ updatedAt: 't0', body: 'written by the agent' })
    expect(saver.conflict).toBe(false)
    stop()
  })

  it('a guarded save names its base, lands, and moves the base on', async () => {
    const { route, saver, stop } = setup()
    expect(await saver.save({ body: 'mine' }, { guard: true })).toMatchObject({ body: 'mine' })
    expect(route.sent[0]).toEqual({ body: 'mine', baseUpdatedAt: 't0' })
    expect(saver.base).toEqual({ updatedAt: 't1', body: 'mine' })
    await saver.save({ body: 'mine, more' }, { guard: true })
    expect(route.sent[1]!.baseUpdatedAt).toBe('t1')
    expect(saver.conflict).toBe(false)
    stop()
  })

  it('an agent edit under the buffer refuses the next save and keeps the agent’s text', async () => {
    const { route, saver, stop } = setup()
    route.external({ body: 'agent text' })
    expect(await saver.save({ body: 'my stale buffer' }, { guard: true })).toBeNull()
    expect(saver.conflict).toBe(true)
    expect(route.row.body).toBe('agent text')
    // Typing on: refused without another round trip.
    expect(await saver.save({ body: 'my stale buffer, more' }, { guard: true })).toBeNull()
    expect(route.sent).toHaveLength(1)
    expect(route.row.body).toBe('agent text')
    stop()
  })

  it('"keep mine" overwrites deliberately and clears the line', async () => {
    const { route, saver, stop } = setup()
    route.external({ body: 'agent text' })
    await saver.save({ body: 'mine' }, { guard: true })
    expect(saver.conflict).toBe(true)
    expect(await saver.save({ body: 'mine' })).toMatchObject({ body: 'mine' })
    expect(route.sent.at(-1)).toEqual({ body: 'mine' })
    expect(saver.conflict).toBe(false)
    expect(saver.base?.body).toBe('mine')
    stop()
  })

  it('a move that left the body alone is retried on the fresh version, silently', async () => {
    const { route, saver, stop } = setup()
    route.external({ title: 'renamed elsewhere' }) // icon, re-file, promote: the body is still v1
    expect(await saver.save({ body: 'mine' }, { guard: true })).toMatchObject({ body: 'mine' })
    expect(route.sent.map((p) => p.baseUpdatedAt)).toEqual(['t0', 'x1'])
    expect(saver.conflict).toBe(false)
    stop()
  })

  it('saves run one at a time, each on the base the one before it left', async () => {
    const { route, saver, stop } = setup()
    const a = saver.save({ body: 'one' }, { guard: true })
    const b = saver.save({ body: 'one two' }, { guard: true })
    expect(await a).toMatchObject({ body: 'one' })
    expect(await b).toMatchObject({ body: 'one two' })
    expect(route.sent.map((p) => p.baseUpdatedAt)).toEqual(['t0', 't1'])
    expect(saver.conflict).toBe(false)
    expect(route.row.body).toBe('one two')
    stop()
  })

  it('an icon save over an agent edit does not launder the stale base', async () => {
    const { route, saver, stop } = setup()
    route.external({ body: 'agent text' })
    await saver.save({ icon: '📄' }, { guard: true })
    expect(await saver.save({ body: 'my stale buffer' }, { guard: true })).toBeNull()
    expect(route.row.body).toBe('agent text')
    stop()
  })

  it('reseeding the buffer (a restore, a fresh edit session) clears the line', async () => {
    const { route, saver, stop } = setup()
    route.external({ body: 'agent text' })
    await saver.save({ body: 'mine' }, { guard: true })
    saver.rebase(route.row)
    expect(saver.conflict).toBe(false)
    expect(await saver.save({ body: 'agent text, then mine' }, { guard: true })).toMatchObject({ body: 'agent text, then mine' })
    stop()
  })
})
