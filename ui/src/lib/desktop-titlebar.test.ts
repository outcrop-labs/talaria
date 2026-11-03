import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const invoke = vi.fn()
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => invoke(...args) }))

const desktopWindow = vi.fn()
vi.mock('./desktop-settings.svelte', async (original) => ({
  ...(await original<typeof import('./desktop-settings.svelte')>()),
  desktopWindow: (action: string) => desktopWindow(action),
}))

interface FakeEl {
  tag: string
  parent: FakeEl | null
  closest(selector: string): FakeEl | null
}

const el = (tag: string, parent: FakeEl | null = null): FakeEl => ({
  tag,
  parent,
  closest(selector) {
    const tags = selector.split(',').map((s) => s.trim())
    if (!tags.every((t) => /^[a-z]+$/.test(t))) throw new Error(`fake closest takes tag names only: ${selector}`)
    for (let n: FakeEl | null = this; n; n = n.parent) if (tags.includes(n.tag)) return n
    return null
  },
})

class FakeBar {
  private listeners = new Map<string, (e: unknown) => void>()
  addEventListener(type: string, fn: (e: unknown) => void) {
    this.listeners.set(type, fn)
  }
  removeEventListener(type: string) {
    this.listeners.delete(type)
  }
  fire(type: string, target: FakeEl, button = 0) {
    this.listeners.get(type)?.({ button, target, preventDefault() {} })
  }
  get listening() {
    return this.listeners.size
  }
}

const bar = el('nav')
const space = el('div', bar)
const tileIcon = el('svg', el('a', el('div', bar)))
const closeIcon = el('svg', el('button', bar))

const windowActions = () => desktopWindow.mock.calls.map(([action]) => action)

describe('the themed titlebar drag surface', () => {
  beforeEach(() => {
    vi.resetModules()
    invoke.mockReset()
    desktopWindow.mockReset()
    invoke.mockResolvedValue(undefined)
    vi.stubGlobal('window', { __TAURI_INTERNALS__: {} })
  })

  afterEach(() => {
    vi.unstubAllGlobals()
  })

  it('drags and maximizes from empty bar space', async () => {
    const { windowDrag } = await import('./desktop-titlebar')
    const node = new FakeBar()
    windowDrag(node as unknown as HTMLElement)
    node.fire('mousedown', space)
    node.fire('dblclick', space)
    expect(windowActions()).toEqual(['startDragging', 'toggleMaximize'])
  })

  it('ignores presses inside a link or a button', async () => {
    const { windowDrag } = await import('./desktop-titlebar')
    const node = new FakeBar()
    windowDrag(node as unknown as HTMLElement)
    for (const target of [tileIcon, closeIcon]) {
      node.fire('mousedown', target)
      node.fire('dblclick', target)
    }
    expect(windowActions()).toEqual([])
  })

  it('ignores a secondary button', async () => {
    const { windowDrag } = await import('./desktop-titlebar')
    const node = new FakeBar()
    windowDrag(node as unknown as HTMLElement)
    node.fire('mousedown', space, 2)
    expect(windowActions()).toEqual([])
  })

  it('stops dragging once the OS titlebar is picked', async () => {
    const { windowDrag } = await import('./desktop-titlebar')
    const { setTitlebarMode } = await import('./desktop-settings.svelte')
    invoke.mockResolvedValueOnce({ titlebar: 'os' })
    await setTitlebarMode('os')
    const node = new FakeBar()
    windowDrag(node as unknown as HTMLElement)
    node.fire('mousedown', space)
    expect(windowActions()).toEqual([])
  })

  it('listens to nothing in a browser', async () => {
    vi.stubGlobal('window', {})
    const { windowDrag } = await import('./desktop-titlebar')
    const node = new FakeBar()
    windowDrag(node as unknown as HTMLElement)
    expect(node.listening).toBe(0)
  })
})
