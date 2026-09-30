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

/** The viewport transform that fits `bounds` into `viewport` with padding.
 *  `bounds.x`/`bounds.y` are the graph's own top-left in CANVAS space and
 *  default to the origin — a chain whose cards were dragged left of it (the
 *  api stores negative coordinates on purpose) fits from its real corner,
 *  not from 0,0. */
export function fitTransform(
  bounds: { x?: number; y?: number; w: number; h: number },
  viewport: { w: number; h: number },
  pad = 24,
): { x: number; y: number; k: number } {
  if (bounds.w <= 0 || bounds.h <= 0 || viewport.w <= 0 || viewport.h <= 0) return { x: 0, y: 0, k: 1 }
  const k = Math.min(
    1.5,
    Math.max(0.2, Math.min((viewport.w - pad * 2) / bounds.w, (viewport.h - pad * 2) / bounds.h)),
  )
  // Centre the scaled bounds in the viewport, then subtract the graph's own
  // origin: the screen position of a canvas point p is `t + k*p`, so landing
  // bounds.x at the left margin means t = margin − k*bounds.x.
  return {
    k,
    x: (viewport.w - bounds.w * k) / 2 - (bounds.x ?? 0) * k,
    y: (viewport.h - bounds.h * k) / 2 - (bounds.y ?? 0) * k,
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

/** A dragged card's resting place: the nearest grid intersection, so
 *  hand-placed cards line up with each other and with the dot field the canvas
 *  draws. `free` (Alt held) turns it off for the one placement that wants to
 *  sit between the dots.
 *
 *  Snapping the CARD's own corner, not the pointer: the offset the drag picked
 *  up when it grabbed the card is preserved by the caller, so a card grabbed
 *  by its edge still lands on the grid rather than a pitch away from it. */
export function snapToGrid(
  p: { x: number; y: number },
  pitch: number,
  free = false,
): { x: number; y: number } {
  if (free || pitch <= 0) return p
  return { x: Math.round(p.x / pitch) * pitch, y: Math.round(p.y / pitch) * pitch }
}

/** The wire nearest a canvas point, or null past `radius`.
 *
 *  ONE walk, two callers: selecting a wire by clicking near it, and deciding
 *  whether a ticket dropped on the canvas landed ON a wire (which splices it
 *  into that connection instead of dropping it loose). They agreed by accident
 *  while the loop lived inside the component; here they cannot drift.
 *
 *  `radius` is in CANVAS units — the caller divides its screen tolerance by the
 *  zoom, so the target stays the same size under the pointer at every scale. */
export function nearestWire<E extends { fromTaskId: string; toTaskId: string }>(
  point: { x: number; y: number },
  edges: readonly E[],
  positions: Map<string, { x: number; y: number }>,
  pathOf: (from: { x: number; y: number }, to: { x: number; y: number }) => string,
  radius: number,
): E | null {
  let best: E | null = null
  let bestDist = radius
  for (const edge of edges) {
    const f = positions.get(edge.fromTaskId)
    const t = positions.get(edge.toTaskId)
    if (!f || !t) continue
    const d = distanceToWire(point, f, t, pathOf)
    if (d <= bestDist) {
      bestDist = d
      best = edge
    }
  }
  return best
}
