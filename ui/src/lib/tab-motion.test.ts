import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { LIFT, liftPlan, tabLift } from './tab-motion'

describe('liftPlan', () => {
  it('reduced motion is an opacity-only crossfade of 150ms', () => {
    const plan = liftPlan(true)
    for (const track of [plan.tab, plan.view, plan.exit]) {
      for (const frame of track.frames) expect(Object.keys(frame)).toEqual(['opacity'])
      expect(track.options.duration).toBe(150)
      expect(track.options.delay ?? 0).toBe(0)
    }
  })

  it('full motion lifts the tab and the view and sinks the old view', () => {
    const plan = liftPlan(false)
    expect(plan.tab.frames[0]).toMatchObject({ transform: 'translateY(8px) scale(0.92)', opacity: 0.4 })
    expect(plan.view.frames[0]).toMatchObject({ transform: 'translateY(12px)', filter: 'blur(3px)', opacity: 0 })
    expect(plan.view.options).toMatchObject({ duration: 280, delay: 40, easing: LIFT.ease })
    expect(plan.exit.frames[1]).toMatchObject({ transform: 'translateY(-8px)', opacity: 0 })
    expect(plan.exit.options.duration).toBe(160)
  })
})

function fakeTab(href: string) {
  return {
    getAttribute: (name: string) => (name === 'href' ? href : null),
    hasAttribute: () => false,
    animate: vi.fn(() => ({ cancel() {}, finished: Promise.resolve() })),
  }
}

describe('tabLift', () => {
  let swapped: () => void = () => {}
  let open: ReturnType<typeof fakeTab>
  let click: (event: object) => void = () => {}
  const strip = {
    querySelector: () => open,
    addEventListener: (_: string, fn: (event: object) => void) => (click = fn),
    removeEventListener: () => {},
  }
  const clickTile = (tile: ReturnType<typeof fakeTab>, mods: object = {}) =>
    click({ button: 0, target: { closest: () => tile }, ...mods })

  beforeEach(() => {
    open = fakeTab('/work')
    vi.stubGlobal(
      'MutationObserver',
      class {
        constructor(cb: () => void) {
          swapped = cb
        }
        observe() {}
        disconnect() {}
      },
    )
    vi.stubGlobal('document', { querySelector: () => null })
    tabLift(strip as unknown as HTMLElement)
  })
  afterEach(() => vi.unstubAllGlobals())

  it('lifts the clicked tile once it opens', () => {
    clickTile(fakeTab('/boards'))
    swapped()
    expect(open.animate).not.toHaveBeenCalled()
    open = fakeTab('/boards')
    swapped()
    expect(open.animate).toHaveBeenCalledTimes(1)
  })

  it('a route change nobody clicked, such as back or a deep link, does not lift', () => {
    open = fakeTab('/boards')
    swapped()
    expect(open.animate).not.toHaveBeenCalled()
  })

  it('a modified click opens elsewhere and does not arm', () => {
    clickTile(fakeTab('/boards'), { metaKey: true })
    open = fakeTab('/boards')
    swapped()
    expect(open.animate).not.toHaveBeenCalled()
  })

  it('a click is spent by the next tab change, even one that lands elsewhere', () => {
    clickTile(fakeTab('/boards'))
    open = fakeTab('/comms')
    swapped()
    open = fakeTab('/boards')
    swapped()
    expect(open.animate).not.toHaveBeenCalled()
  })
})
