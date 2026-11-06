// @vitest-environment runes
//
// The context menu's controller and its target action (context-menu.svelte.ts):
// where a menu opens and whether it opens for the keyboard, where focus goes
// back to, and the long-press that stands in for a right-click on touch. The
// keys inside an open menu are context-menu-keys.test.ts; the rendering is
// exercised in the running app (the PR's screenshots).
import assert from 'node:assert/strict'
import { afterEach, beforeEach, test, vi } from 'vitest'
import { LONG_PRESS_MS, contextMenuTarget, useContextMenu, type ContextMenuController, type ContextMenuEntry } from './context-menu.svelte'

class FakeElement {
  listeners = new Map<string, ((e: unknown) => void)[]>()
  style = { setProperty: vi.fn() }
  isContentEditable = false
  inMenu = false
  constructor(public rect = { top: 100, bottom: 124, left: 20, right: 200 }) {}
  getBoundingClientRect() {
    return this.rect
  }
  closest(sel: string) {
    return sel === '[role="menu"]' && this.inMenu ? this : null
  }
  addEventListener(type: string, f: (e: unknown) => void) {
    this.listeners.set(type, [...(this.listeners.get(type) ?? []), f])
  }
  removeEventListener(type: string, f: (e: unknown) => void) {
    this.listeners.set(type, (this.listeners.get(type) ?? []).filter((g) => g !== f))
  }
  fire(type: string, e: Record<string, unknown>) {
    const ev = { type, target: this, currentTarget: this, preventDefault: vi.fn(), stopPropagation: vi.fn(), ...e }
    for (const f of this.listeners.get(type) ?? []) f(ev)
    return ev
  }
}
class FakeKeyboardEvent {
  preventDefault = vi.fn()
  stopPropagation = vi.fn()
  constructor(
    public type: string,
    public key: string,
    public target: unknown,
    public shiftKey = false,
  ) {}
  get currentTarget() {
    return this.target
  }
}

const doc = { activeElement: null as unknown, body: {} }
beforeEach(() => {
  vi.stubGlobal('Element', FakeElement)
  vi.stubGlobal('HTMLElement', FakeElement)
  vi.stubGlobal('KeyboardEvent', FakeKeyboardEvent)
  vi.stubGlobal('document', doc)
  doc.activeElement = null
})
afterEach(() => {
  vi.unstubAllGlobals()
  vi.useRealTimers()
})

/** The open state, read fresh — an `assert.equal(menu.state, null)` would
 *  otherwise narrow every later read of the getter to `never`. */
const stateOf = (m: ContextMenuController) => m.state

const items: ContextMenuEntry[] = [{ label: 'Open' }, 'sep', { label: 'Delete', danger: true }]
const pointer = (x: number, y: number, target: unknown) =>
  ({ type: 'contextmenu', clientX: x, clientY: y, target, currentTarget: target, detail: 1, preventDefault: vi.fn(), stopPropagation: vi.fn() }) as unknown as MouseEvent

test('a right-click opens at the pointer, for the pointer', () => {
  const menu = useContextMenu()
  const row = new FakeElement()
  const e = pointer(40, 60, row)
  menu.openMenu(e, items)
  assert.equal(menu.state?.x, 40)
  assert.equal(menu.state?.y, 60)
  assert.equal(menu.state?.keyboard, false)
  assert.equal(menu.state?.anchor, undefined)
  // The browser's own menu never shows.
  assert.equal(vi.mocked(e.preventDefault).mock.calls.length, 1)
})

test('the Menu key opens against the focused element, with the first item to take focus', () => {
  const menu = useContextMenu()
  const row = new FakeElement({ top: 10, bottom: 34, left: 5, right: 280 })
  doc.activeElement = row
  menu.openMenu(new FakeKeyboardEvent('keydown', 'ContextMenu', row) as unknown as KeyboardEvent, items)
  assert.equal(menu.state?.keyboard, true)
  assert.deepEqual(menu.state?.anchor, { top: 10, bottom: 34, left: 5, right: 280 })
  // Focus comes back here when the menu closes from inside.
  assert.equal(menu.state?.returnFocus, row)
})

test('Enter on a "…" button (a click with detail 0) opens for the keyboard; a mouse click does not', () => {
  const menu = useContextMenu()
  const more = new FakeElement()
  menu.openMenu({ ...(pointer(0, 0, more) as object), type: 'click', detail: 0 } as unknown as MouseEvent, items)
  assert.equal(menu.state?.keyboard, true)
  assert.ok(menu.state?.anchor)
  menu.openAt(more as unknown as Element, items, { keyboard: false })
  assert.equal(menu.state?.keyboard, false)
  assert.deepEqual(menu.state?.anchor, more.rect)
})

test('a menu with nothing in it does not open, and focus inside a menu is never the place to return to', () => {
  const menu = useContextMenu()
  menu.openMenu(pointer(1, 1, new FakeElement()), ['sep'])
  assert.equal(stateOf(menu), null)
  const row = new FakeElement()
  doc.activeElement = row
  menu.openMenu(pointer(1, 1, row), items)
  // Re-opened while focus sits on one of the open menu's items: keep the row.
  const menuItem = new FakeElement()
  menuItem.inMenu = true
  doc.activeElement = menuItem
  menu.openMenu(pointer(2, 2, row), items)
  assert.equal(menu.state?.returnFocus, row)
  menu.closeMenu()
  assert.equal(stateOf(menu), null)
})

test('a long touch press opens the menu at the finger and swallows the tap that follows', () => {
  vi.useFakeTimers()
  const menu = useContextMenu()
  const row = new FakeElement()
  const action = contextMenuTarget(row as unknown as HTMLElement, { menu, items: () => items })
  row.fire('pointerdown', { pointerType: 'touch', clientX: 30, clientY: 40 })
  vi.advanceTimersByTime(LONG_PRESS_MS - 1)
  assert.equal(stateOf(menu), null)
  vi.advanceTimersByTime(1)
  assert.equal(menu.state?.x, 30)
  assert.equal(menu.state?.y, 40)
  const click = row.fire('click', {})
  assert.equal(vi.mocked(click.preventDefault).mock.calls.length, 1)
  // Only that one tap: the next is a tap again.
  const next = row.fire('click', {})
  assert.equal(vi.mocked(next.preventDefault).mock.calls.length, 0)
  action.destroy()
})

test("the release's compatibility mousedown is swallowed too, so the open menu does not read it as an outside click", () => {
  vi.useFakeTimers()
  const now = vi.spyOn(performance, 'now').mockReturnValue(1_000)
  const menu = useContextMenu()
  const row = new FakeElement()
  contextMenuTarget(row as unknown as HTMLElement, { menu, items: () => items })
  row.fire('pointerdown', { pointerType: 'touch', clientX: 30, clientY: 40 })
  vi.advanceTimersByTime(LONG_PRESS_MS)
  row.fire('pointerup', {})
  const down = row.fire('mousedown', {})
  assert.equal(vi.mocked(down.stopPropagation).mock.calls.length, 1)
  // A device that sends no click after a long-press: the swallow lapses, and
  // the next real tap is a tap.
  now.mockReturnValue(2_000)
  const later = row.fire('click', {})
  assert.equal(vi.mocked(later.preventDefault).mock.calls.length, 0)
  now.mockRestore()
})

test('a press that drifts is a scroll, a short one is a tap, and a mouse never long-presses', () => {
  vi.useFakeTimers()
  const menu = useContextMenu()
  const row = new FakeElement()
  contextMenuTarget(row as unknown as HTMLElement, { menu, items: () => items })
  row.fire('pointerdown', { pointerType: 'touch', clientX: 30, clientY: 40 })
  row.fire('pointermove', { clientX: 30, clientY: 80 })
  vi.advanceTimersByTime(LONG_PRESS_MS * 2)
  assert.equal(stateOf(menu), null)
  row.fire('pointerdown', { pointerType: 'touch', clientX: 30, clientY: 40 })
  row.fire('pointerup', {})
  vi.advanceTimersByTime(LONG_PRESS_MS * 2)
  assert.equal(stateOf(menu), null)
  row.fire('pointerdown', { pointerType: 'mouse', clientX: 30, clientY: 40 })
  vi.advanceTimersByTime(LONG_PRESS_MS * 2)
  assert.equal(stateOf(menu), null)
})

test('the target opens on right-click and on Shift+F10, reading its items when it opens', () => {
  const menu = useContextMenu()
  const row = new FakeElement()
  let label = 'Pin'
  contextMenuTarget(row as unknown as HTMLElement, { menu, items: () => [{ label }] })
  row.fire('contextmenu', { clientX: 5, clientY: 6 })
  assert.equal((menu.state?.items[0] as { label: string }).label, 'Pin')
  menu.closeMenu()
  label = 'Unpin'
  for (const f of row.listeners.get('keydown') ?? []) f(new FakeKeyboardEvent('keydown', 'F10', row, true))
  assert.equal(menu.state?.keyboard, true)
  assert.equal((menu.state?.items[0] as { label: string }).label, 'Unpin')
})
