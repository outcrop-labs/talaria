import {
  statesForSlot,
  waitingState,
  type WaitingRole,
  type WaitingSlot,
  type WaitingState,
} from './registry'
import { WAITING_SITES, WAITING_SITE_KEYS, type WaitingSiteKey } from './sites'

/**
 * The dealer: one seed in, an assignment for every site out.
 *
 * Pure and synchronous on purpose — the whole randomisation is `deal(seed)`,
 * which means it can be asserted on in a node test rather than eyeballed in a
 * browser, and two surfaces reading the same seed cannot disagree.
 *
 * WHY DEAL RATHER THAN HASH PER SITE. The obvious implementation is
 * `states[hash(seed, key) % states.length]` at each call site, which is one
 * line and needs no table. It also collides constantly: with 36 sites drawing
 * from 30 states, independent hashing puts a duplicate somewhere with
 * probability >99%, and the duplicate you actually notice is the one where the
 * chat turn and the tool row three lines below it run the same mark slightly
 * out of phase. Dealing from a shuffled deck spends the whole catalogue before
 * it repeats anything, so a session shows ~30 distinct states instead of ~20.
 */

/**
 * mulberry32. Small, fast, and — the part that matters here — a pure function
 * of its state, so the same seed replays the same sequence in a test, in a
 * browser, and in whatever runs this next.
 */
function rng(seed: number): () => number {
  let a = seed >>> 0
  return () => {
    a = (a + 0x6d2b79f5) >>> 0
    let t = Math.imul(a ^ (a >>> 15), 1 | a)
    t = (t + Math.imul(t ^ (t >>> 7), 61 | t)) ^ t
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296
  }
}

/** Fisher-Yates against `next`, copying rather than sorting in place. */
function shuffle<T>(items: readonly T[], next: () => number): T[] {
  const out = items.slice()
  for (let i = out.length - 1; i > 0; i--) {
    const j = Math.floor(next() * (i + 1))
    const tmp = out[i]!
    out[i] = out[j]!
    out[j] = tmp
  }
  return out
}

export type WaitingAssignment = Readonly<Record<WaitingSiteKey, string>>

/**
 * Assign a state slug to every site.
 *
 * One shuffled deck for the whole app, walked once. Each site takes the first
 * card it can physically use (a `button` site skips the wide fields and the
 * whole grid family) that has been used least so far — so the deck is spent
 * evenly before anything repeats, and the sites that CAN'T use most of the deck
 * don't starve the ones that can.
 *
 * Sites are visited in sorted-key order, not in "most constrained first". A
 * greedy most-constrained pass would pack better, but it also means adding one
 * unconstrained site to the table re-deals sites that have nothing to do with
 * it. Stability across edits is worth more here than an optimal packing of a
 * set that has slack in it anyway.
 */
export function deal(seed: number): WaitingAssignment {
  const next = rng(seed)
  const deck = shuffle(statesForSlot('status'), next) // 'status' admits everything
  const used = new Map<string, number>(deck.map((s) => [s.slug, 0]))
  const out = {} as Record<WaitingSiteKey, string>

  for (const key of WAITING_SITE_KEYS) {
    const slot: WaitingSlot = WAITING_SITES[key].slot
    let pick: WaitingState | undefined
    let pickUses = Infinity
    for (const state of deck) {
      if (!state.slots.includes(slot)) continue
      const uses = used.get(state.slug)!
      // Strictly less-than, so ties go to the earlier card in the shuffled
      // deck — that is what keeps the deck order meaningful rather than
      // letting the site iteration order silently become the real shuffle.
      if (uses < pickUses) {
        pick = state
        pickUses = uses
        if (uses === 0) break // nothing can beat an unused card; stop scanning
      }
    }
    // Every slot has states (see registry.slotsFor), so `pick` is always set;
    // the fallback is here so a future slot with an empty pool fails loudly at
    // its call site rather than writing `undefined` into the assignment.
    const slug = pick?.slug ?? deck[0]!.slug
    used.set(slug, (used.get(slug) ?? 0) + 1)
    out[key] = slug
  }
  return out as WaitingAssignment
}

/**
 * Resolve one site against an assignment, with the pin honoured first.
 *
 * `pin` is the `?waiting=<slug>` review override. An unknown slug is IGNORED
 * rather than throwing or blanking: a typo in a URL should show you the normal
 * app, not an app with no waiting indicators in it.
 */
export function resolveSite(
  assignment: WaitingAssignment,
  key: WaitingSiteKey,
  pin?: string | null,
): WaitingState {
  const pinned = pin ? waitingState(pin) : undefined
  if (pinned) return pinned
  return waitingState(assignment[key]) ?? statesForSlot(WAITING_SITES[key].slot)[0]!
}

/**
 * A site declared at its call site rather than in the table.
 *
 * For code OUTSIDE this app — the SDK surface that third-party apps under
 * `apps/` build against. Those call sites are an open set: a plugin author
 * cannot add a row to `sites.ts`, and the app cannot know their keys at build
 * time, so they cannot participate in the deal.
 *
 * They therefore get the mechanism the deal replaced — a per-key hash — and
 * that is the RIGHT tool here for the reason it was the wrong one in-app.
 * Dealing needs a closed set to spend; hashing needs nothing but a key, at the
 * cost of collisions nobody can see across app boundaries anyway.
 *
 * `role` is required because there is no table row to carry it: an external
 * mark still has to declare what kind of wait it is, or it cannot be paced.
 */
export interface InlineWaitingSite {
  key: string
  role: WaitingRole
  /** Defaults to `inline`, the slot that admits everything but the tightest. */
  slot?: WaitingSlot
}

/** FNV-1a. Any stable string→uint32 does; this one is four lines and has no deps. */
function hashKey(key: string, seed: number): number {
  let h = (0x811c9dc5 ^ seed) >>> 0
  for (let i = 0; i < key.length; i++) {
    h = (h ^ key.charCodeAt(i)) >>> 0
    h = Math.imul(h, 0x01000193) >>> 0
  }
  return h >>> 0
}

/**
 * Pick a state for an inline site. Same seed as the deal, so an external app's
 * mark re-rolls with the rest of the cockpit rather than sitting still while
 * everything around it changes.
 */
export function pickInline(seed: number, site: InlineWaitingSite, pin?: string | null): WaitingState {
  const pinned = pin ? waitingState(pin) : undefined
  if (pinned) return pinned
  const pool = statesForSlot(site.slot ?? 'inline')
  return pool[hashKey(site.key, seed) % pool.length]!
}
