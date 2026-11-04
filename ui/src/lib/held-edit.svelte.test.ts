// @vitest-environment runes
//
// A refused buffer must survive the editor going away: it is kept on the
// device while the save is refused, offered back on the next open, and
// forgotten once the person decides. These run the real wiring on Svelte's
// client runtime against an in-memory localStorage.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { heldKey, holdRefusedEdits, parseHeld, readHeld, writeHeld } from './held-edit.svelte'

// sv-router (the navigation guard's home) reads `location.search` when it
// loads; this runtime has no document. Hoisted above the imports.
vi.hoisted(() => {
  ;(globalThis as { location?: unknown }).location ??= new URL('http://localhost/')
})

class MemoryStorage {
  map = new Map<string, string>()
  getItem(k: string) {
    return this.map.has(k) ? this.map.get(k)! : null
  }
  setItem(k: string, v: string) {
    this.map.set(k, v)
  }
  removeItem(k: string) {
    this.map.delete(k)
  }
}

const KEY = heldKey('kb-doc', 'doc-1', 'user-1')
/** Let the client runtime run its queued effects. */
const settle = () => new Promise((resolve) => setTimeout(resolve, 0))
let store: MemoryStorage

beforeEach(() => {
  store = new MemoryStorage()
  ;(globalThis as { window?: unknown }).window = { localStorage: store, addEventListener() {}, removeEventListener() {} }
})
afterEach(() => {
  delete (globalThis as { window?: unknown }).window
})

describe('the stored shape', () => {
  it('keys by kind, document and viewer', () => {
    expect(heldKey('artifact', 'a1', 'u9')).toBe('talaria:refused:artifact:a1:u9')
  })
  it('reads back what was written, and nothing malformed', () => {
    writeHeld(KEY, { body: 'mine', at: '2026-11-03T10:00:00.000Z' })
    expect(readHeld(KEY)).toEqual({ body: 'mine', at: '2026-11-03T10:00:00.000Z' })
    expect(parseHeld({ body: 3, at: 'x' })).toBeNull()
    expect(parseHeld({ body: 'b', at: 'x', title: 4 })).toBeNull()
    store.setItem(KEY, '{not json')
    expect(readHeld(KEY)).toBeNull()
  })
  it('degrades to nothing when storage refuses access', () => {
    ;(globalThis as { window?: unknown }).window = {
      get localStorage(): Storage {
        throw new Error('blocked')
      },
    }
    expect(() => writeHeld(KEY, { body: 'mine', at: 'now' })).not.toThrow()
    expect(readHeld(KEY)).toBeNull()
  })
})

describe('holdRefusedEdits', () => {
  async function mount(initial: { doc: { body: string } | null; editing?: boolean }) {
    const s = $state({
      conflict: false,
      held: null as { title?: string; body?: string } | null,
      busy: false,
      editing: initial.editing ?? true,
      doc: initial.doc,
    })
    // The editor's text lives in tiptap, not in Svelte state: a plain read.
    const editor = { text: undefined as string | undefined }
    let hold!: ReturnType<typeof holdRefusedEdits>
    const stop = $effect.root(() => {
      hold = holdRefusedEdits({
        key: () => KEY,
        saver: {
          get conflict() {
            return s.conflict
          },
          get held() {
            return s.held
          },
          get busy() {
            return s.busy
          },
        },
        editing: () => s.editing,
        live: () => (editor.text === undefined ? undefined : { body: editor.text }),
        current: () => s.doc,
        leave: async () => true,
      })
    })
    await settle()
    return { s, editor, hold: () => hold, stop }
  }

  it('opening a document with nothing kept offers nothing', async () => {
    const m = await mount({ doc: { body: 'server' } })
    expect(m.hold().offered).toBeNull()
    m.stop()
  })

  it('keeps every refused attempt while editing, and forgets it once resolved', async () => {
    const m = await mount({ doc: { body: 'server' } })
    m.s.conflict = true
    m.s.held = { body: 'my words' }
    await settle()
    expect(readHeld(KEY)?.body).toBe('my words')
    m.s.held = { body: 'my words, more' }
    await settle()
    expect(readHeld(KEY)?.body).toBe('my words, more')
    // Keep mine landed (or a restore, or discard): the refusal is over.
    m.s.conflict = false
    m.s.held = null
    await settle()
    expect(readHeld(KEY)).toBeNull()
    m.stop()
  })

  it('takes the live buffer as the editor goes away, then offers it on the next open', async () => {
    const first = await mount({ doc: { body: 'agent text' } })
    first.s.conflict = true
    first.s.held = { body: 'typed before the last pause' }
    await settle()
    first.editor.text = 'typed before the last pause, and after it'
    first.stop() // a tree click, a backlink, Back: the editor unmounts
    expect(readHeld(KEY)?.body).toBe('typed before the last pause, and after it')

    const second = await mount({ doc: { body: 'agent text' }, editing: false })
    expect(second.hold().offered?.body).toBe('typed before the last pause, and after it')
    second.hold().settle() // Discard (or Keep mine, once it landed)
    expect(second.hold().offered).toBeNull()
    expect(readHeld(KEY)).toBeNull()
    second.stop()
  })

  it('drops a kept edit the document already holds, without asking', async () => {
    writeHeld(KEY, { body: 'same text', at: '2026-11-03T10:00:00.000Z' })
    const m = await mount({ doc: { body: 'same text' } })
    expect(m.hold().offered).toBeNull()
    expect(readHeld(KEY)).toBeNull()
    m.stop()
  })

  it('keeps nothing for a refusal outside the editor (a read-mode Muse accept says it itself)', async () => {
    const m = await mount({ doc: { body: 'server' }, editing: false })
    m.s.conflict = true
    m.s.held = { body: 'a draft' }
    await settle()
    m.stop()
    expect(readHeld(KEY)).toBeNull()
  })
})
