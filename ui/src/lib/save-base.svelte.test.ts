// @vitest-environment runes
//
// No silent overwrite: an agent saves while a person's editor holds a buffer
// from the version before, and the person's next save must not wipe the
// agent's edit. The route half (the conditional content update behind both
// PUTs) has its own live route tests (api/tests/it/doc_save_base_live.rs);
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
 *  row has moved past is refused with the row, unless it would change
 *  nothing there (same body, same title if it sends one). */
function fakeRoute(initial: Row) {
  let row = { ...initial }
  let tick = 0
  const sent: Sent[] = []
  /** Set to hold the NEXT put until `release()` — a save still in flight. */
  let gate: Promise<void> | null = null
  let release = () => {}
  const put = async (p: Sent): Promise<SaveAnswer<Row>> => {
    sent.push(p)
    if (gate) {
      const g = gate
      gate = null
      await g
    }
    await Promise.resolve()
    // The route's rule: a stale base is refused unless the save would
    // change nothing that moved — the same body, and the same title if any.
    const nothingToOverwrite = p.body === row.body && (p.title === undefined || p.title === row.title)
    if (p.baseUpdatedAt !== undefined && p.body !== undefined && p.baseUpdatedAt !== row.updatedAt && !nothingToOverwrite) {
      return { conflict: { ...row } }
    }
    row = { ...row, ...(p.body !== undefined ? { body: p.body } : {}), ...(p.title !== undefined ? { title: p.title } : {}), updatedAt: `t${++tick}` }
    return { saved: { ...row } }
  }
  return {
    put,
    sent,
    hold() {
      gate = new Promise<void>((r) => (release = r))
      return () => release()
    },
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

  it('a reseed while a save is in flight is not undone by that save landing (Edit → Read → Edit)', async () => {
    const { route, saver, stop } = setup()
    // Leaving Edit: the exit save of the last words is in flight…
    const letGo = route.hold()
    const exit = saver.save({ body: 'v1 + last words' }, { guard: true })
    await Promise.resolve()
    // …and back in Edit before it answers: the editor reseeds from the doc
    // it still has, which does not hold those words.
    saver.rebase(opened)
    letGo()
    expect(await exit).toMatchObject({ body: 'v1 + last words' })
    // The old save landed, but it does not move the reseeded buffer's base:
    // the next autosave would replace the words it never saw, so it refuses.
    expect(saver.base).toEqual(opened)
    expect(await saver.save({ body: 'v1 + new typing' }, { guard: true })).toBeNull()
    expect(saver.conflict).toBe(true)
    expect(route.row.body).toBe('v1 + last words')
    stop()
  })

  it('keeps what was typed after a refusal as the held buffer, until a reseed', async () => {
    const { route, saver, stop } = setup()
    route.external({ body: 'agent text' })
    await saver.save({ body: 'mine' }, { guard: true })
    expect(saver.held).toEqual({ body: 'mine' })
    await saver.save({ body: 'mine, more' }, { guard: true })
    expect(saver.held).toEqual({ body: 'mine, more' })
    saver.rebase(route.row)
    expect(saver.held).toBeNull()
    stop()
  })

  it('is busy while a save is in flight, and idle() waits for it', async () => {
    const { route, saver, stop } = setup()
    const letGo = route.hold()
    const p = saver.save({ body: 'mine' }, { guard: true })
    expect(saver.busy).toBe(true)
    let idle = false
    void saver.idle().then(() => (idle = true))
    await Promise.resolve()
    expect(idle).toBe(false)
    letGo()
    await p
    await saver.idle()
    expect(saver.busy).toBe(false)
    stop()
  })
})

describe('guardedSaver and titles', () => {
  const titled = { updatedAt: 't0', body: 'v1', title: 'Plan' }
  const setup = () => {
    const route = fakeRoute(titled)
    let saver!: ReturnType<typeof guardedSaver<Row, Patch>>
    const stop = $effect.root(() => {
      saver = guardedSaver<Row, Patch>(route.put)
      saver.rebase(titled)
    })
    return { route, saver, stop }
  }

  it('a guarded save does not send a title the person did not change', async () => {
    const { route, saver, stop } = setup()
    await saver.save({ title: 'Plan', body: 'mine' }, { guard: true })
    expect(route.sent[0]).toEqual({ body: 'mine', baseUpdatedAt: 't0' })
    stop()
  })

  it('another person’s rename survives the silent retry', async () => {
    const { route, saver, stop } = setup()
    route.external({ title: 'Plan (renamed by Docbot)' }) // body still v1
    expect(await saver.save({ title: 'Plan', body: 'mine' }, { guard: true })).toMatchObject({ body: 'mine' })
    expect(route.row.title).toBe('Plan (renamed by Docbot)')
    expect(saver.conflict).toBe(false)
    stop()
  })

  it('two renames are a conflict, not a retry', async () => {
    const { route, saver, stop } = setup()
    route.external({ title: 'Plan (renamed by Docbot)' })
    expect(await saver.save({ title: 'Plan v2', body: 'v1' }, { guard: true })).toBeNull()
    expect(saver.conflict).toBe(true)
    expect(route.row.title).toBe('Plan (renamed by Docbot)')
    expect(conflictVerdict(titled, { updatedAt: 'x1', body: 'v1', title: 'Other' }, { title: 'Plan v2' })).toBe('conflict')
    expect(conflictVerdict(titled, { updatedAt: 'x1', body: 'v1', title: 'Other' })).toBe('retry')
    stop()
  })
})
