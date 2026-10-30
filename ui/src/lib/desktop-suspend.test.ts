// The desktop shell suspends an off-screen instance only when its page says
// a reload loses nothing the person entered. Every kind of field the rule
// names, and the unsaved mark, checked against what a reload would and would
// not keep.
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { fieldOf, TypedInput, type Field } from './desktop-suspend'

type Listener = (e: { target: unknown }) => void

type Fired = 'focusin' | 'pointerdown' | 'beforeinput' | 'input' | 'change'

/** A document that routes the events the tracker listens to, and answers
 *  `querySelector` for the unsaved mark from a set the test controls. */
class FakeDoc {
  listeners = new Map<string, Listener[]>()
  marked = new Set<string>()
  addEventListener(type: string, fn: Listener) {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), fn])
  }
  querySelector(selector: string) {
    return this.marked.has(selector) ? {} : null
  }
  fire(type: Fired, target: unknown) {
    for (const fn of this.listeners.get(type) ?? []) fn({ target })
  }
}

/** A minimal element: a tag, a value or text, a parent chain, and the two
 *  attributes `closest` is asked about. */
function el(
  tagName: string,
  o: {
    type?: string
    value?: string
    checked?: boolean
    text?: string
    parent?: Field | null
    editable?: boolean
    draft?: boolean
    disabled?: boolean
    readOnly?: boolean
  } = {},
): Field & { value?: string; checked?: boolean; textContent: string | null; isConnected: boolean } {
  const self: Field & { editable: boolean; draft: boolean } = {
    tagName,
    type: o.type,
    value: o.value,
    checked: o.checked,
    textContent: o.text ?? null,
    isConnected: true,
    disabled: o.disabled,
    readOnly: o.readOnly,
    parentElement: o.parent ?? null,
    editable: o.editable ?? false,
    draft: o.draft ?? false,
    closest(selector: string) {
      const want = selector.startsWith('[contenteditable]') ? 'editable' : 'draft'
      for (let n: (Field & { editable?: boolean; draft?: boolean }) | null = this; n; n = n.parentElement) {
        if (n[want as 'editable' | 'draft']) return n
      }
      return null
    },
  }
  return self
}

/** Type into a field the way the browser does: beforeinput sees the old
 *  value, then the value changes, then input fires. */
function type(doc: FakeDoc, field: Field & { value?: string; textContent: string | null }, next: string) {
  doc.fire('beforeinput', field)
  if (field.tagName === 'INPUT' || field.tagName === 'TEXTAREA') field.value = next
  else field.textContent = next
  doc.fire('input', field)
}

/** Pick in a select or toggle a box the way the browser does: the person
 *  reaches for it (focus or press), its state changes, then input and change
 *  fire. No beforeinput. */
function pick(doc: FakeDoc, field: Field & { value?: string; checked?: boolean }, next: string | boolean, reach = true) {
  if (reach) doc.fire('focusin', field)
  if (typeof next === 'boolean') field.checked = next
  else field.value = next
  doc.fire('input', field)
  doc.fire('change', field)
}

describe('which fields count as entered input', () => {
  it('takes every input a person sets, textareas and selects, and skips buttons, hidden and files', () => {
    for (const t of ['text', 'search', 'email', 'url', 'tel', 'password', 'number', 'TEXT']) {
      expect(fieldOf(el('INPUT', { type: t }) as unknown as EventTarget), t).not.toBeNull()
    }
    for (const t of ['checkbox', 'radio', 'range', 'color', 'date', 'time']) {
      expect(fieldOf(el('INPUT', { type: t }) as unknown as EventTarget), t).not.toBeNull()
    }
    // A picked file is handed off on pick in ui; what it becomes (a marked
    // chip, or a saved upload) is what counts.
    for (const t of ['file', 'submit', 'button', 'reset', 'image', 'hidden']) {
      expect(fieldOf(el('INPUT', { type: t }) as unknown as EventTarget), t).toBeNull()
    }
    expect(fieldOf(el('TEXTAREA') as unknown as EventTarget)).not.toBeNull()
    expect(fieldOf(el('SELECT') as unknown as EventTarget)).not.toBeNull()
    expect(fieldOf(el('DIV') as unknown as EventTarget)).toBeNull()
    expect(fieldOf(null)).toBeNull()
    expect(fieldOf({} as EventTarget)).toBeNull()
  })

  it('takes the outermost editable host of a rich editor', () => {
    const outer = el('DIV', { editable: true, text: '' })
    const inner = el('DIV', { editable: true, parent: outer })
    const p = el('P', { parent: inner })
    expect(fieldOf(p as unknown as EventTarget)).toBe(outer)
  })
})

describe('what a reload would lose', () => {
  let doc: FakeDoc
  let typed: TypedInput

  beforeEach(() => {
    doc = new FakeDoc()
    typed = new TypedInput(doc as unknown as Document)
  })

  it('is nothing on a page nobody typed on', () => {
    expect(typed.holds()).toBe(false)
  })

  it('is a typed field, input or textarea or rich editor', () => {
    const input = el('INPUT', { type: 'text', value: '' })
    type(doc, input, 'half a ticket title')
    expect(typed.holds()).toBe(true)

    const t2 = new TypedInput((doc = new FakeDoc()) as unknown as Document)
    const area = el('TEXTAREA', { value: '' })
    type(doc, area, 'a prompt mid-edit')
    expect(t2.holds()).toBe(true)

    const t3 = new TypedInput((doc = new FakeDoc()) as unknown as Document)
    const editor = el('DIV', { editable: true, text: '' })
    type(doc, editor, 'a description')
    expect(t3.holds()).toBe(true)
  })

  it('is an edit to a field that already held text, including clearing it', () => {
    const name = el('INPUT', { type: 'text', value: 'Ops agent' })
    type(doc, name, '')
    expect(typed.holds()).toBe(true)
  })

  it('is nothing once the field is typed back to what it held', () => {
    const name = el('INPUT', { type: 'text', value: 'Ops agent' })
    type(doc, name, 'Ops agen')
    expect(typed.holds()).toBe(true)
    type(doc, name, 'Ops agent')
    expect(typed.holds()).toBe(false)
  })

  it('is nothing in a composer whose draft ui keeps across a reload', () => {
    const composer = el('DIV', { draft: true })
    const editor = el('DIV', { editable: true, text: '', parent: composer })
    type(doc, editor, 'an unsent message')
    expect(typed.holds()).toBe(false)
  })

  it('is nothing once a field has left the page, or is disabled or read-only', () => {
    const gone = el('INPUT', { type: 'text', value: '' })
    type(doc, gone, 'typed on a page since left')
    gone.isConnected = false
    expect(typed.holds()).toBe(false)

    const off = el('INPUT', { type: 'text', value: '', disabled: true })
    type(doc, off, 'x')
    const ro = el('TEXTAREA', { value: '', readOnly: true })
    type(doc, ro, 'x')
    expect(typed.holds()).toBe(false)
  })

  it('counts a value that arrived with no beforeinput (autofill) from empty', () => {
    const email = el('INPUT', { type: 'email', value: '' })
    email.value = 'jon@example.com'
    doc.fire('input', email)
    expect(typed.holds()).toBe(true)
  })

  it('is a select picked in an unsaved form, until it is picked back', () => {
    const status = el('SELECT', { value: 'todo' })
    pick(doc, status, 'doing')
    expect(typed.holds()).toBe(true)
    pick(doc, status, 'todo')
    expect(typed.holds()).toBe(false)
  })

  it('is a select changed with no focus first, counted from nothing known', () => {
    const status = el('SELECT', { value: 'todo' })
    pick(doc, status, 'doing', false)
    expect(typed.holds()).toBe(true)
  })

  it('is a checkbox toggled, until it is toggled back, with or without a press first', () => {
    const box = el('INPUT', { type: 'checkbox', checked: false })
    pick(doc, box, true)
    expect(typed.holds()).toBe(true)
    pick(doc, box, false)
    expect(typed.holds()).toBe(false)

    // Its label clicked: the box itself was never reached for.
    const t2 = new TypedInput((doc = new FakeDoc()) as unknown as Document)
    const viaLabel = el('INPUT', { type: 'checkbox', checked: true })
    pick(doc, viaLabel, false, false)
    expect(t2.holds()).toBe(true)
    pick(doc, viaLabel, true, false)
    expect(t2.holds()).toBe(false)
  })

  it('is a radio picked', () => {
    const radio = el('INPUT', { type: 'radio', checked: false })
    pick(doc, radio, true, false)
    expect(typed.holds()).toBe(true)
  })

  it('is a pending attachment over a composer, whatever the fields hold', () => {
    // The chips are uploaded and unsent: the draft keeps the text beside them
    // (so the composer's own editor does not count), not the chips.
    const composer = el('DIV', { draft: true })
    const editor = el('DIV', { editable: true, text: '', parent: composer })
    type(doc, editor, 'see this')
    expect(typed.holds()).toBe(false)
    doc.marked.add('[data-desktop-unsaved]')
    expect(typed.holds()).toBe(true)
    // Sent or removed: the strip and its mark are gone.
    doc.marked.delete('[data-desktop-unsaved]')
    expect(typed.holds()).toBe(false)
  })

  it('ignores a file picker, a button and a hidden field', () => {
    for (const t of ['file', 'button', 'hidden']) {
      const f = el('INPUT', { type: t, value: '' })
      type(doc, f, 'C:\\fakepath\\shot.png')
    }
    expect(typed.holds()).toBe(false)
  })
})

describe('the hook the shell reads', () => {
  afterEach(() => {
    vi.unstubAllGlobals()
    vi.resetModules()
  })

  it('is installed only inside the shell, once, and answers the contract', async () => {
    const doc = new FakeDoc()
    vi.stubGlobal('document', doc)
    vi.stubGlobal('window', { __TAURI_INTERNALS__: {}, localStorage: { getItem: () => null } })
    const mod = await import('./desktop-suspend')
    mod.installDesktopSuspend()
    const hook = (window as unknown as { __TALARIA_DESKTOP__: { version: number; suspendState: () => unknown } })
      .__TALARIA_DESKTOP__
    expect(hook.version).toBe(1)
    expect(hook.suspendState()).toEqual({ holdsInput: false, notify: true })
    const first = hook
    mod.installDesktopSuspend()
    expect((window as unknown as { __TALARIA_DESKTOP__: unknown }).__TALARIA_DESKTOP__).toBe(first)
    // A pending attachment shows up in the answer, read from the document.
    doc.marked.add('[data-desktop-unsaved]')
    expect(hook.suspendState()).toEqual({ holdsInput: true, notify: true })
    doc.marked.clear()
    // Typed text shows up in the answer.
    const input = el('INPUT', { type: 'text', value: '' })
    type(doc, input, 'x')
    expect(hook.suspendState()).toEqual({ holdsInput: true, notify: true })
  })

  it('reports notify off when the person switched OS notifications off', async () => {
    vi.stubGlobal('document', new FakeDoc())
    vi.stubGlobal('window', {
      __TAURI_INTERNALS__: {},
      localStorage: { getItem: (k: string) => (k === 'talaria.browserNotify' ? 'off' : null) },
    })
    const mod = await import('./desktop-suspend')
    mod.installDesktopSuspend()
    const hook = (window as unknown as { __TALARIA_DESKTOP__: { suspendState: () => unknown } }).__TALARIA_DESKTOP__
    expect(hook.suspendState()).toEqual({ holdsInput: false, notify: false })
  })

  it('is absent in a browser', async () => {
    vi.stubGlobal('document', new FakeDoc())
    vi.stubGlobal('window', {})
    const mod = await import('./desktop-suspend')
    mod.installDesktopSuspend()
    expect((window as unknown as { __TALARIA_DESKTOP__?: unknown }).__TALARIA_DESKTOP__).toBeUndefined()
  })
})
