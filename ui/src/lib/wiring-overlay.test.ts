import { describe, expect, it } from 'vitest'
import {
  clientToCanvas,
  distanceToWire,
  fitTransform,
  hitTestNode,
  hitTestPort,
  portPoint,
  portRadius,
  wireMidpoint,
  zoomAt,
  TOUCH_PORT_RADIUS,
} from '@/lib/wiring-overlay'
import { NODE_H, NODE_W, wirePath } from '@/lib/workchain-rules'

// The wiring overlay's pure geometry (TALA-34): port points, hit testing,
// viewport math, wire distance. Same discipline as workchain-rules tests —
// pin what the interaction layer derives from.

describe('portPoint', () => {
  it('the in-port rides the left edge midpoint', () => {
    expect(portPoint({ x: 100, y: 50 }, 'in')).toEqual({ x: 100, y: 50 + NODE_H / 2 })
  })

  it('the out-port rides the right edge midpoint', () => {
    expect(portPoint({ x: 100, y: 50 }, 'out')).toEqual({ x: 100 + NODE_W, y: 50 + NODE_H / 2 })
  })
})

describe('portRadius', () => {
  it('touch pointers get the larger target', () => {
    expect(portRadius('touch')).toBe(TOUCH_PORT_RADIUS)
    expect(portRadius('mouse')).toBeLessThan(TOUCH_PORT_RADIUS)
    expect(portRadius('pen')).toBeLessThan(TOUCH_PORT_RADIUS)
    expect(portRadius(undefined)).toBeLessThan(TOUCH_PORT_RADIUS)
  })
})

describe('hitTestPort', () => {
  const at = (taskId: string, side: 'in' | 'out', pos = { x: 0, y: 0 }) => ({ taskId, side, pos })

  it('hits inside the radius', () => {
    expect(hitTestPort({ x: 4, y: NODE_H / 2 + 3 }, [at('a', 'in')], 10)).toEqual({ taskId: 'a', side: 'in' })
  })

  it('misses outside the radius', () => {
    expect(hitTestPort({ x: 40, y: NODE_H / 2 + 40 }, [at('a', 'in')], 10)).toBeNull()
  })

  it('picks the NEAREST candidate', () => {
    const hits = hitTestPort(
      { x: NODE_W - 4, y: NODE_H / 2 },
      [at('a', 'in'), at('a', 'out', { x: 0, y: 0 })],
      10,
    )
    expect(hits).toEqual({ taskId: 'a', side: 'out' })
  })
})

describe('hitTestNode', () => {
  const pos = new Map([
    ['a', { x: 0, y: 0 }],
    ['b', { x: 300, y: 200 }],
  ])

  it('a point inside a card names it', () => {
    expect(hitTestNode({ x: 50, y: 60 }, pos)).toBe('a')
    expect(hitTestNode({ x: 310, y: 300 }, pos)).toBe('b')
  })

  it('a point between cards misses', () => {
    expect(hitTestNode({ x: 250, y: 200 }, pos)).toBeNull()
  })
})

describe('clientToCanvas', () => {
  it('inverts the viewport transform', () => {
    const view = { x: 40, y: 20, k: 1.5 }
    const canvas = clientToCanvas({ x: 170, y: 65 }, { left: 10, top: 10 }, view)
    // screen p = k * canvas p + t  →  canvas p = (screen p - t) / k
    expect(canvas).toEqual({ x: (170 - 10 - 40) / 1.5, y: (65 - 10 - 20) / 1.5 })
  })
})

describe('zoomAt', () => {
  it('keeps the focus point fixed on screen', () => {
    const view = { x: 30, y: 10, k: 1 }
    const focus = { x: 100, y: 60 } // canvas space
    const screen = { x: view.x + focus.x * view.k, y: view.y + focus.y * view.k }
    const next = zoomAt(view, 1.5, focus)
    expect(next.x + focus.x * next.k).toBeCloseTo(screen.x, 5) // screen p unchanged
    expect(next.y + focus.y * next.k).toBeCloseTo(screen.y, 5)
  })

  it('clamps to the min and max', () => {
    expect(zoomAt({ x: 0, y: 0, k: 2.5 }, 10, { x: 0, y: 0 }).k).toBe(2.5)
    expect(zoomAt({ x: 0, y: 0, k: 0.4 }, 0.01, { x: 0, y: 0 }).k).toBe(0.4)
    // no zoom in bounds: unchanged
    expect(zoomAt({ x: 5, y: 5, k: 1 }, 1, { x: 0, y: 0 })).toEqual({ x: 5, y: 5, k: 1 })
  })
})

describe('fitTransform', () => {
  it('fits the bounds inside the viewport with padding', () => {
    const t = fitTransform({ w: 600, h: 400 }, { w: 800, h: 500 }, 24)
    expect(t.k).toBeLessThanOrEqual((800 - 48) / 600)
    expect(t.k).toBeGreaterThan(0)
    // centered-ish: the content's screen span fits with margin on both sides
    expect(t.x).toBeGreaterThanOrEqual(0)
    expect(t.y).toBeGreaterThanOrEqual(0)
    expect(t.x + 600 * t.k).toBeLessThanOrEqual(800)
  })

  it('degenerate bounds fall back to identity', () => {
    expect(fitTransform({ w: 0, h: 0 }, { w: 800, h: 500 })).toEqual({ x: 0, y: 0, k: 1 })
    expect(fitTransform({ w: 600, h: 400 }, { w: 0, h: 0 })).toEqual({ x: 0, y: 0, k: 1 })
  })

  it('tiny viewports clamp the zoom, never below the floor', () => {
    const t = fitTransform({ w: 2000, h: 1200 }, { w: 100, h: 80 })
    expect(t.k).toBeGreaterThanOrEqual(0.2)
  })
})

describe('distanceToWire', () => {
  it('zero on the path itself, large off it', () => {
    const from = { x: 0, y: 0 }
    const to = { x: 300, y: 0 }
    // midpoint of the straight wire
    expect(distanceToWire({ x: (NODE_W + 300) / 2, y: NODE_H / 2 }, from, to, wirePath)).toBeLessThan(1)
    expect(distanceToWire({ x: 260, y: NODE_H / 2 + 200 }, from, to, wirePath)).toBeGreaterThan(50)
  })

  it('rejects a malformed path as unreachable (Infinity)', () => {
    expect(distanceToWire({ x: 0, y: 0 }, { x: 0, y: 0 }, { x: 300, y: 0 }, () => 'nonsense')).toBe(Infinity)
  })
})

describe('wireMidpoint', () => {
  it('lands between the two endpoints', () => {
    const m = wireMidpoint({ x: 0, y: 0 }, { x: 300, y: 0 })
    expect(m.x).toBeGreaterThan(NODE_W)
    expect(m.x).toBeLessThan(300)
    expect(m.y).toBeCloseTo(NODE_H / 2, 5)
  })
})
