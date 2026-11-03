import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { edgeFade, revealCurrent } from './tab-strip'

class FakeStrip {
  scrollWidth = 0
  clientWidth = 0
  scrollLeft = 0
  dataset: Record<string, string> = {}
  children: object[] = []
  private listeners = new Map<string, () => void>()
  addEventListener(type: string, fn: () => void) {
    this.listeners.set(type, fn)
  }
  removeEventListener(type: string) {
    this.listeners.delete(type)
  }
  scrollTo(left: number) {
    this.scrollLeft = left
    this.listeners.get('scroll')?.()
  }
}

let resized: () => void = () => {}
let mutated: () => void = () => {}

beforeEach(() => {
  vi.stubGlobal(
    'ResizeObserver',
    class {
      constructor(cb: () => void) {
        resized = cb
      }
      observe() {}
      disconnect() {}
    },
  )
  vi.stubGlobal(
    'MutationObserver',
    class {
      constructor(cb: () => void) {
        mutated = cb
      }
      observe() {}
      disconnect() {}
    },
  )
})

afterEach(() => {
  vi.unstubAllGlobals()
})

function mount(scrollWidth: number, clientWidth: number) {
  const strip = new FakeStrip()
  strip.scrollWidth = scrollWidth
  strip.clientWidth = clientWidth
  const action = edgeFade(strip as unknown as HTMLElement)
  return { strip, action }
}

describe('edgeFade', () => {
  it('sets data-more while tabs remain to the right', () => {
    const { strip } = mount(900, 400)
    expect(strip.dataset.more).toBe('')
    strip.scrollTo(200)
    expect(strip.dataset.more).toBe('')
  })

  it('clears data-more at the end of the scroll', () => {
    const { strip } = mount(900, 400)
    strip.scrollTo(500)
    expect(strip.dataset.more).toBeUndefined()
    strip.scrollTo(100)
    expect(strip.dataset.more).toBe('')
  })

  it('leaves a strip that fits unmarked', () => {
    const { strip } = mount(400, 400)
    expect(strip.dataset.more).toBeUndefined()
  })

  it('re-reads the strip when its content resizes', () => {
    const { strip } = mount(400, 400)
    strip.scrollWidth = 700
    resized()
    expect(strip.dataset.more).toBe('')
  })

  it('stops listening once destroyed', () => {
    const { strip, action } = mount(900, 400)
    action.destroy()
    strip.scrollTo(500)
    expect(strip.dataset.more).toBe('')
  })
})

class FakeTabStrip {
  scrollLeft = 0
  open: { offset: number; width: number } | null = null
  constructor(readonly left: number, readonly width: number) {}
  getBoundingClientRect() {
    return { left: this.left, right: this.left + this.width }
  }
  querySelector() {
    if (!this.open) return null
    const left = this.left + this.open.offset - this.scrollLeft
    return { getBoundingClientRect: () => ({ left, right: left + this.open!.width }) }
  }
}

describe('revealCurrent', () => {
  it('scrolls an open tab past the right edge into view, clear of the fade', () => {
    const strip = new FakeTabStrip(48, 150)
    strip.open = { offset: 220, width: 90 }
    revealCurrent(strip as unknown as HTMLElement)
    const tab = strip.querySelector()!.getBoundingClientRect()
    expect(tab.right).toBeLessThanOrEqual(48 + 150 - 32)
    expect(tab.left).toBeGreaterThanOrEqual(48)
  })

  it('scrolls back when the newly opened tab is left of the view', () => {
    const strip = new FakeTabStrip(48, 150)
    strip.scrollLeft = 200
    strip.open = { offset: 20, width: 32 }
    revealCurrent(strip as unknown as HTMLElement)
    expect(strip.querySelector()!.getBoundingClientRect().left).toBe(48 + 12)
  })

  it('leaves the strip alone when the open tab is already in view', () => {
    const strip = new FakeTabStrip(48, 150)
    strip.open = { offset: 40, width: 60 }
    revealCurrent(strip as unknown as HTMLElement)
    expect(strip.scrollLeft).toBe(0)
  })

  it('reveals again when the open tab changes', () => {
    const strip = new FakeTabStrip(48, 150)
    strip.open = { offset: 40, width: 60 }
    revealCurrent(strip as unknown as HTMLElement)
    strip.open = { offset: 300, width: 90 }
    mutated()
    expect(strip.querySelector()!.getBoundingClientRect().right).toBe(48 + 150 - 32)
  })
})
