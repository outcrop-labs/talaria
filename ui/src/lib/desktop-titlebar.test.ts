import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

const invoke = vi.fn()
vi.mock('@tauri-apps/api/core', () => ({ invoke: (...args: unknown[]) => invoke(...args) }))

const desktopWindow = vi.fn()
vi.mock('./desktop-settings.svelte', async (original) => ({
  ...(await original<typeof import('./desktop-settings.svelte')>()),
  desktopWindow: (action: string) => desktopWindow(action),
}))

interface Box {
  top: number
  bottom: number
}

interface FakeEl {
  tag: string
  parentElement: FakeEl | null
  box: Box
  matches(selector: string): boolean
  getBoundingClientRect(): Box
}

const BAR: Box = { top: 0, bottom: 56 }

const el = (tag: string, parentElement: FakeEl | null = null, box: Box = BAR): FakeEl => ({
  tag,
  parentElement,
  box,
  matches(selector) {
    const tags = selector.split(',').map((s) => s.trim())
    if (!tags.every((t) => /^[a-z]+$/.test(t))) throw new Error(`fake matches takes tag names only: ${selector}`)
    return tags.includes(this.tag)
  },
  getBoundingClientRect() {
    return this.box
  },
})

class FakeBar {
  private listeners = new Map<string, (e: unknown) => void>()
  constructor(private self: FakeEl) {}
  addEventListener(type: string, fn: (e: unknown) => void) {
    this.listeners.set(type, fn)
  }
  removeEventListener(type: string) {
    this.listeners.delete(type)
  }
  getBoundingClientRect() {
    return this.self.box
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
const markPath = el('path', el('svg', el('div', el('div', bar))))
const tileIcon = el('svg', el('a', el('div', bar)))
const closeIcon = el('svg', el('button', bar))
const searchPanel = el('div', el('section', el('div', bar)), { top: 46, bottom: 320 })
const panelText = el('div', searchPanel, { top: 54, bottom: 74 })

const mount = (windowDrag: (node: HTMLElement) => unknown) => {
  const node = new FakeBar(bar)
  windowDrag(node as unknown as HTMLElement)
  return node
}

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
    const node = mount(windowDrag)
    node.fire('mousedown', space)
    node.fire('dblclick', space)
    node.fire('mousedown', markPath)
    expect(windowActions()).toEqual(['startDragging', 'toggleMaximize', 'startDragging'])
  })

  it('ignores presses inside a link or a button', async () => {
    const { windowDrag } = await import('./desktop-titlebar')
    const node = mount(windowDrag)
    for (const target of [tileIcon, closeIcon]) {
      node.fire('mousedown', target)
      node.fire('dblclick', target)
    }
    expect(windowActions()).toEqual([])
  })

  it('ignores presses on a panel that hangs out of the bar', async () => {
    const { windowDrag } = await import('./desktop-titlebar')
    const node = mount(windowDrag)
    for (const target of [searchPanel, panelText]) {
      node.fire('mousedown', target)
      node.fire('dblclick', target)
    }
    expect(windowActions()).toEqual([])
  })

  it('ignores a secondary button', async () => {
    const { windowDrag } = await import('./desktop-titlebar')
    const node = mount(windowDrag)
    node.fire('mousedown', space, 2)
    expect(windowActions()).toEqual([])
  })

  it('stops dragging once the OS titlebar is picked', async () => {
    const { windowDrag } = await import('./desktop-titlebar')
    const { setTitlebarMode } = await import('./desktop-settings.svelte')
    invoke.mockResolvedValueOnce({ titlebar: 'os' })
    await setTitlebarMode('os')
    const node = mount(windowDrag)
    node.fire('mousedown', space)
    expect(windowActions()).toEqual([])
  })

  it('listens to nothing in a browser', async () => {
    vi.stubGlobal('window', {})
    const { windowDrag } = await import('./desktop-titlebar')
    const node = mount(windowDrag)
    expect(node.listening).toBe(0)
  })
})
