import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { edgeFade } from './tab-strip'

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
