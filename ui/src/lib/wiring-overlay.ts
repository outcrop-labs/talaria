// The wiring overlay's pure geometry (TALA-34). Kept out of the component so
// hit testing and viewport math stay testable in plain TS — the same shape
// workchain-rules.ts gives the canvas's layout and paths.
import { NODE_H, NODE_W } from '@/lib/workchain-rules'

/** A port's position in CANVAS coordinates. In-port sits at a card's left
 *  edge midpoint, out-port at its right edge midpoint — the same points the
 *  wires terminate on. */
export function portPoint(
  pos: { x: number; y: number },
  side: 'in' | 'out',
): { x: number; y: number } {
  return side === 'in'
    ? { x: pos.x, y: pos.y + NODE_H / 2 }
    : { x: pos.x + NODE_W, y: pos.y + NODE_H / 2 }
}

/** The hit radius around a port. Pointer-type aware: touch pointers get the
 *  larger target (triage answer — fingers are fatter than cursors). */
export const MOUSE_PORT_RADIUS = 10
export const TOUCH_PORT_RADIUS = 24

export function portRadius(pointerType: string | undefined): number {
  return pointerType === 'touch' ? TOUCH_PORT_RADIUS : MOUSE_PORT_RADIUS
}

/** Nearest port hit for a canvas-space point, or null. Only IN-ports are
 *  drop targets on a wiring drag (the gesture pulls FROM an out-port), so
 *  the candidates are the caller's — the caller pre-filters to valid steps.
 *  Returns the owning taskId and its side so a drop can name the wire. */
export function hitTestPort(
  point: { x: number; y: number },
  candidates: Array<{ taskId: string; side: 'in' | 'out'; pos: { x: number; y: number } }>,
  radius: number,
): { taskId: string; side: 'in' | 'out' } | null {
  let best: { taskId: string; side: 'in' | 'out' } | null = null
  let bestDist = radius
  for (const c of candidates) {
    const p = portPoint(c.pos, c.side)
    const d = Math.hypot(p.x - point.x, p.y - point.y)
    if (d <= bestDist) {
      bestDist = d
      best = { taskId: c.taskId, side: c.side }
    }
  }
  return best
}

/** The nearest node a point lands on (a drop on a CARD, not its port).
 *  `radius` is ignored here — anywhere inside the card's box counts. */
export function hitTestNode(
  point: { x: number; y: number },
  positions: Map<string, { x: number; y: number }>,
): string | null {
  for (const [taskId, p] of positions) {
    if (point.x >= p.x && point.x <= p.x + NODE_W && point.y >= p.y && point.y <= p.y + NODE_H)
      return taskId
  }
  return null
}

// ── The viewport: pan + zoom ────────────────────────────────────────────────

/** The canvas takes a wheel only when it is a zoom gesture: ctrl or cmd held
 *  (a trackpad pinch synthesizes ctrl). A bare wheel is the page's scroll. */
export function wheelZoomGesture(e: { ctrlKey: boolean; metaKey: boolean }): boolean {
  return e.ctrlKey || e.metaKey
}

/** A canvas-space point from a client-space one, through the viewport. */
export function clientToCanvas(
  client: { x: number; y: number },
  rect: { left: number; top: number },
  view: { x: number; y: number; k: number },
): { x: number; y: number } {
  return {
    x: (client.x - rect.left - view.x) / view.k,
    y: (client.y - rect.top - view.y) / view.k,
  }
}

/** Zoom keeping `focus` (canvas space) fixed under the pointer. */
export function zoomAt(
  view: { x: number; y: number; k: number },
  factor: number,
  focus: { x: number; y: number },
  min = 0.4,
  max = 2.5,
): { x: number; y: number; k: number } {
  const k = Math.min(max, Math.max(min, view.k * factor))
  if (k === view.k) return view
  // The pointer's screen position is `t + k*p`; holding it steady across the
  // zoom means `t' = t + p*(k − k')` — the canvas point stays under the cursor.
  return {
    x: view.x + focus.x * (view.k - k),
    y: view.y + focus.y * (view.k - k),
    k,
  }
}

/** The viewport transform that fits `bounds` into `viewport` with padding. */
export function fitTransform(
  bounds: { w: number; h: number },
  viewport: { w: number; h: number },
  pad = 24,
): { x: number; y: number; k: number } {
  if (bounds.w <= 0 || bounds.h <= 0 || viewport.w <= 0 || viewport.h <= 0) return { x: 0, y: 0, k: 1 }
  const k = Math.min(
    1.5,
    Math.max(0.2, Math.min((viewport.w - pad * 2) / bounds.w, (viewport.h - pad * 2) / bounds.h)),
  )
  return {
    k,
    x: (viewport.w - pad * 2 - bounds.w * k) / 2 + pad,
    y: (viewport.h - pad - bounds.h * k) / 2 + pad,
  }
}

/** Wire hit testing: distance from a point to a cubic bezier (the wire path).
 *  Sampled — 24 segments is visually indistinguishable from exact for 1.5px
 *  strokes and stays allocation-free on the move path. */
export function distanceToWire(
  point: { x: number; y: number },
  from: { x: number; y: number },
  to: { x: number; y: number },
  pathOf: (from: { x: number; y: number }, to: { x: number; y: number }) => string,
  samples = 24,
): number {
  const d = pathOf(from, to)
  // Parse the cubic bezier the rules module builds: M x y C .., .., .. ..
  const nums = d.match(/-?\d+(?:\.\d+)?/g)?.map(Number) ?? []
  if (nums.length < 8) return Infinity
  const [x1, y1, c1x, c1y, c2x, c2y, x2, y2] = nums as [number, number, number, number, number, number, number, number]
  let best = Infinity
  for (let i = 1; i <= samples; i++) {
    const t = i / samples
    const mt = 1 - t
    const x = mt * mt * mt * x1 + 3 * mt * mt * t * c1x + 3 * mt * t * t * c2x + t * t * t * x2
    const y = mt * mt * mt * y1 + 3 * mt * mt * t * c1y + 3 * mt * t * t * c2y + t * t * t * y2
    best = Math.min(best, Math.hypot(x - point.x, y - point.y))
  }
  return best
}

/** The midpoint of a cubic bezier at t=0.5 — where a wire's selection state
 *  (and any delete affordance) reads best. */
export function wireMidpoint(
  from: { x: number; y: number },
  to: { x: number; y: number },
): { x: number; y: number } {
  const x1 = from.x + NODE_W
  const y1 = from.y + NODE_H / 2
  const x2 = to.x
  const y2 = to.y + NODE_H / 2
  const dx = Math.max(40, Math.abs(x2 - x1) / 2)
  const c1x = x1 + dx
  const c1y = y1
  const c2x = x2 - dx
  const c2y = y2
  // B(0.5) = (P0 + 3P1 + 3P2 + P3) / 8
  return { x: (x1 + 3 * c1x + 3 * c2x + x2) / 8, y: (y1 + 3 * c1y + 3 * c2y + y2) / 8 }
}
