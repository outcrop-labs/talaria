import {
  deal,
  pickInline,
  resolveSite,
  type InlineWaitingSite,
  type WaitingAssignment,
} from './rotation'
import { speedFor, type WaitingState } from './registry'
import { WAITING_SITES, type WaitingSiteKey } from './sites'

/**
 * The seed, and the hand it deals — one per browsing session.
 *
 * WHY sessionStorage RATHER THAN localStorage. "Per session" has to mean
 * something a user experiences. localStorage would fix the cast at first visit
 * and hold it for months, which is `deal(constant)` with extra steps — the
 * randomisation would exist in the code and never be observed. sessionStorage
 * re-rolls on each new tab while staying stable across every navigation inside
 * one, which is exactly the promise: coherent now, different next time.
 *
 * WHY A SEED RATHER THAN PICKING PER MOUNT. Two marks alive at once (the chat
 * turn and a tool row three lines below it) must not disagree, and a mark must
 * not change identity when its component remounts mid-wait. Storing 4 bytes and
 * dealing from them gives both for free, and makes the whole thing replayable —
 * `?waiting-seed=42` reproduces a colleague's exact cockpit.
 *
 * This is safe to import from a node test: every window access is guarded, and
 * an unseeded module still deals a valid hand.
 */

const SEED_KEY = 'talaria.waiting.seed'

/** `?waiting=<slug>` — pin every site to one state, for review. */
const PIN_PARAM = 'waiting'
/** `?waiting-seed=<uint32>` — replay someone else's hand. */
const SEED_PARAM = 'waiting-seed'

function readParam(name: string): string | null {
  if (typeof window === 'undefined') return null
  try {
    return new URLSearchParams(window.location.search).get(name)
  } catch {
    return null
  }
}

function initialSeed(): number {
  const forced = Number(readParam(SEED_PARAM))
  if (Number.isFinite(forced) && forced >= 0) return forced >>> 0

  if (typeof window === 'undefined') return 0
  try {
    const stored = window.sessionStorage.getItem(SEED_KEY)
    if (stored !== null) {
      const n = Number(stored)
      if (Number.isFinite(n)) return n >>> 0
    }
    // crypto over Math.random: the seed is 32 bits and is the ONLY entropy in
    // the system, so a weak generator here is the whole rotation's weakness.
    const fresh = window.crypto.getRandomValues(new Uint32Array(1))[0]!
    window.sessionStorage.setItem(SEED_KEY, String(fresh))
    return fresh
  } catch {
    // Private mode, storage disabled, or a locked-down embed. A per-load seed
    // is still better than a constant — it just stops surviving navigation.
    return Date.now() >>> 0
  }
}

let seed = $state(initialSeed())
let pin = $state<string | null>(readParam(PIN_PARAM))

/**
 * The hand, memoised on the seed.
 *
 * A plain cache rather than a module-level `$derived`: reactivity comes from
 * the `seed` read below, which happens inside whatever `$derived` the calling
 * component is in, so tracking is already correct — and a rune here would only
 * add a second, subtler mechanism doing the same job.
 */
let dealtSeed = -1
let dealtHand: WaitingAssignment | null = null

function assignment(): WaitingAssignment {
  const current = seed
  if (dealtHand === null || dealtSeed !== current) {
    dealtSeed = current
    dealtHand = deal(current)
  }
  return dealtHand
}

/**
 * There is deliberately no reseed() or pin() function here.
 *
 * Both would be API with no caller: the two things a reviewer actually needs
 * are `?waiting=<slug>` and `?waiting-seed=<n>`, which are read at boot above,
 * and the current seed is already inspectable without a helper —
 * `sessionStorage['talaria.waiting.seed']`, or clear it and reload to re-roll.
 * Shipping mutators for a control surface that does not exist is how a kit
 * grows affordances nobody wired.
 */

export interface ResolvedWaiting {
  state: WaitingState
  /** Playback multiplier that lands the state on the site's rung. */
  speed: number
}

/**
 * Anything that can name a waiting site: a key from the table, or — for code
 * outside this app, which cannot add a row to the table — a descriptor that
 * carries its own role. See `InlineWaitingSite` in rotation.ts for why the two
 * cases use different mechanisms.
 */
export type WaitingSiteRef = WaitingSiteKey | InlineWaitingSite

/**
 * What a call site actually needs: which state, and how fast.
 *
 * A function rather than a store-per-site because the answer only changes when
 * the seed or the pin does, and both are module-level `$state` — so a component
 * calling this inside a `$derived` re-runs exactly then and never otherwise.
 */
export function waitingFor(ref: WaitingSiteRef): ResolvedWaiting {
  if (typeof ref === 'string') {
    const state = resolveSite(assignment(), ref, pin)
    return { state, speed: speedFor(state, WAITING_SITES[ref].role) }
  }
  const state = pickInline(seed, ref, pin)
  return { state, speed: speedFor(state, ref.role) }
}
