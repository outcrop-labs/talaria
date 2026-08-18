import { ANIMATIONS, AMBIENT, IMMEDIATE, STANDARD, type WaitingAnimation } from './animations'
import { GRID_ANIMATIONS, type GridAnimation } from './grid-animations'

/**
 * The thirty waiting states as ONE addressable set.
 *
 * Two renderers, one catalogue. `animations.ts` holds the 21 braille fields and
 * `grid-animations.ts` the 9 dot grids; neither knows about the other, and
 * neither should — they are engines. This is the layer that knows both exist,
 * and it is the only thing the rotation and the components talk to.
 *
 * The slug is the stable handle. It is what `?waiting=` pins, what the dealer
 * shuffles, and what a bug report names — so slugs are frozen once shipped even
 * if the display name changes. Ids are already unique ACROSS the two families
 * (that is a property `animations.ts` and `grid-animations.ts` maintain between
 * them), which is what lets one flat registry work at all.
 */

export type WaitingFamily = 'braille' | 'grid'

/**
 * Where a mark is being asked to sit. This is a PHYSICAL constraint, not a
 * semantic one — it answers "does this fit", not "does this mean the right
 * thing". Semantics are `WaitingRole` below.
 */
export type WaitingSlot = 'button' | 'inline' | 'status'

/**
 * What the wait MEANS, mapped to Mercury's state table (spec §9).
 *
 * Deliberately NOT a pool filter. The obvious reading of the timing ladder is
 * to bucket the catalogue by declared period and let each role draw from its
 * own bucket — but the periods are 750/1200/2100 and exactly ONE animation
 * (`classic`) is natively 750, so "submitting" would be a pool of one and every
 * submit button in the app would show the same mark forever.
 *
 * Both renderers already take a `speed` multiplier, so the role instead sets
 * the TEMPO: any field can play at any rung. That is what the ladder is
 * actually for — it governs how fast a loader reads, not which loader you own.
 */
export type WaitingRole = 'submitting' | 'reasoning' | 'tool' | 'background'

/**
 * Effective loop length per role — the rungs from spec §9, as the RANGES the
 * spec actually gives rather than the single value `animations.ts` picked out
 * of each one.
 *
 * The range is what makes this cheap. Snapping to a rung's midpoint forces a
 * speed on every state whether it needs one or not: a 1200ms field asked to
 * read as `reasoning` would play at exactly 1.0 either way, but a 2400ms one
 * asked to submit would run at 3.2× — well past anything the set was reviewed
 * at, and enough to turn Composing's cursor into a blur. Aiming at the nearest
 * EDGE of the range instead caps the stretch at 2.67× and leaves most states
 * playing exactly as authored.
 */
export const ROLE_RANGE: Record<WaitingRole, readonly [number, number]> = {
  submitting: [600, 900],
  reasoning: [1000, 1400],
  tool: [1000, 1400],
  background: [1800, 2400],
}

/** The rung each declared period sits on — kept for readers of `animations.ts`. */
export const ROLE_NOMINAL: Record<WaitingRole, number> = {
  submitting: IMMEDIATE,
  reasoning: STANDARD,
  tool: STANDARD,
  background: AMBIENT,
}

export interface WaitingState {
  /** Stable handle. Frozen once shipped. */
  slug: string
  name: string
  family: WaitingFamily
  blurb: string
  /** Declared loop length, ms. Divided into the role's rung to get `speed`. */
  period: number
  /**
   * Horizontal cost in monospace columns, so the dealer can keep a six-cell
   * Heartbeat out of a 24px icon button. A grid is square and costs its side.
   */
  cols: number
  /** Slots this state physically fits. Derived, never hand-maintained. */
  slots: readonly WaitingSlot[]
}

/**
 * Which slots a state fits, from its width alone.
 *
 * Buttons hold an icon's worth of room, so three columns is the ceiling and the
 * grid family is out entirely — a 5×5 square is a block, and a block inside a
 * 28px button either overflows it or shrinks until the dots merge. Everything
 * fits `inline` and `status`; the difference between those two is the size the
 * call site renders at, not the set it draws from.
 */
function slotsFor(family: WaitingFamily, cols: number): readonly WaitingSlot[] {
  if (family === 'grid') return ['inline', 'status']
  return cols <= 3 ? ['button', 'inline', 'status'] : ['inline', 'status']
}

const fromBraille = (a: WaitingAnimation): WaitingState => ({
  slug: a.id,
  name: a.name,
  family: 'braille',
  blurb: a.blurb,
  period: a.period,
  cols: a.cols,
  slots: slotsFor('braille', a.cols),
})

const fromGrid = (a: GridAnimation): WaitingState => ({
  slug: a.id,
  name: a.name,
  family: 'grid',
  blurb: a.blurb,
  period: a.period,
  cols: a.n,
  slots: slotsFor('grid', a.n),
})

/**
 * Sorted by slug, not by declaration order.
 *
 * The dealer shuffles this, and a shuffle is only reproducible if what goes in
 * is in a fixed order. Declaration order is fixed too — until someone moves an
 * animation up a block to group it with a neighbour, and every seed in every
 * open session silently deals a different hand. Sorting by the frozen handle
 * makes the input immune to edits that were meant to be cosmetic.
 */
export const WAITING_STATES: readonly WaitingState[] = [
  ...ANIMATIONS.map(fromBraille),
  ...GRID_ANIMATIONS.map(fromGrid),
].sort((a, b) => (a.slug < b.slug ? -1 : a.slug > b.slug ? 1 : 0))

const BY_SLUG = new Map(WAITING_STATES.map((s) => [s.slug, s]))

export const waitingState = (slug: string): WaitingState | undefined => BY_SLUG.get(slug)

export const statesForSlot = (slot: WaitingSlot): readonly WaitingState[] =>
  WAITING_STATES.filter((s) => s.slots.includes(slot))

/**
 * Playback multiplier that lands `state` inside `role`'s range.
 *
 * The renderers compute phase as `elapsed * speed / period`, so the effective
 * loop is `period / speed`. Landing in `[lo, hi]` therefore means a speed in
 * `[period / hi, period / lo]` — and within that window the best choice is the
 * one nearest 1, because 1 is the tempo the field was drawn at. A state whose
 * declared period already sits in the range comes back untouched.
 */
export function speedFor(state: WaitingState, role: WaitingRole): number {
  const [lo, hi] = ROLE_RANGE[role]
  const min = state.period / hi
  const max = state.period / lo
  return Math.min(Math.max(1, min), max)
}
