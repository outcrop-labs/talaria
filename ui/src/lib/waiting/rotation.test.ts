import assert from 'node:assert/strict'
import { test } from 'vitest'
import { deal, pickInline, resolveSite } from './rotation'
import { WAITING_SITES, WAITING_SITE_KEYS, type WaitingSiteKey } from './sites'
import {
  ROLE_RANGE,
  WAITING_STATES,
  statesForSlot,
  speedFor,
  waitingState,
  type WaitingRole,
} from './registry'

// The dealer is the whole randomisation, and it is the kind of code that is
// wrong silently: a bad shuffle still returns a full assignment, just a worse
// one. These assert the four properties that make it worth having over a
// one-line per-site hash — determinism, coverage, fit, and spread.

const seeds = [0, 1, 7, 42, 1337, 0xc0ffee, 0xffffffff]

test('the same seed deals the same hand', () => {
  for (const seed of seeds) assert.deepEqual(deal(seed), deal(seed))
})

test('different seeds deal different hands', () => {
  const hands = new Set(seeds.map((s) => JSON.stringify(deal(s))))
  // Not "all distinct" — two seeds colliding is possible and harmless. The
  // property that matters is that the seed is actually an input.
  assert.ok(hands.size > 1, 'every seed produced the same assignment')
})

test('every site gets a real state', () => {
  const hand = deal(42)
  assert.equal(Object.keys(hand).length, WAITING_SITE_KEYS.length)
  for (const key of WAITING_SITE_KEYS) {
    assert.ok(waitingState(hand[key]), `${key} → ${hand[key]} is not a registered slug`)
  }
})

test('a dealt state physically fits its slot', () => {
  for (const seed of seeds) {
    const hand = deal(seed)
    for (const key of WAITING_SITE_KEYS) {
      const state = waitingState(hand[key])!
      const slot = WAITING_SITES[key].slot
      assert.ok(
        state.slots.includes(slot),
        `seed ${seed}: ${key} (${slot}) got ${state.slug}, which fits ${state.slots.join('/')}`,
      )
    }
  }
})

test('button sites never get the grid family or a wide field', () => {
  // The constraint with teeth: a 5x5 lattice or a six-cell Heartbeat inside a
  // 28px button either overflows it or shrinks until the dots merge.
  for (const seed of seeds) {
    const hand = deal(seed)
    for (const key of WAITING_SITE_KEYS) {
      if (WAITING_SITES[key].slot !== 'button') continue
      const state = waitingState(hand[key])!
      assert.equal(state.family, 'braille', `${key} got grid state ${state.slug}`)
      assert.ok(state.cols <= 3, `${key} got ${state.slug} at ${state.cols} cells`)
    }
  }
})

test('the deck is spent before anything repeats', () => {
  // The reason to deal rather than hash per site. Sites outnumber states, so
  // SOME reuse is forced — but no state may be used twice while another that
  // fits the same slot is still untouched.
  for (const seed of seeds) {
    const hand = deal(seed)
    const uses = new Map<string, number>()
    for (const key of WAITING_SITE_KEYS) uses.set(hand[key], (uses.get(hand[key]) ?? 0) + 1)

    for (const key of WAITING_SITE_KEYS) {
      const slot = WAITING_SITES[key].slot
      const mine = uses.get(hand[key])!
      if (mine < 2) continue
      for (const alt of statesForSlot(slot)) {
        assert.ok(
          (uses.get(alt.slug) ?? 0) >= mine - 1,
          `seed ${seed}: ${hand[key]} used ${mine}x while ${alt.slug} sat at ${uses.get(alt.slug) ?? 0}`,
        )
      }
    }
  }
})

test('a hand covers most of the catalogue', () => {
  // 36 sites over 30 states, with the button sites confined to 10 of them —
  // so perfect coverage is not reachable, but a hand that only shows half the
  // set means the dealer is clumping and the point is lost.
  for (const seed of seeds) {
    const distinct = new Set(Object.values(deal(seed)))
    assert.ok(
      distinct.size >= WAITING_STATES.length - 6,
      `seed ${seed} showed only ${distinct.size} of ${WAITING_STATES.length}`,
    )
  }
})

test('adding a site does not re-deal the sites before it', () => {
  // Sorted-key iteration means a new row only disturbs sites that sort after
  // it AND compete for the same cards. Asserted by simulating the two keys
  // that bracket the table.
  const hand = deal(42)
  const first = WAITING_SITE_KEYS[0]!
  assert.equal(deal(42)[first], hand[first])
})

test('a pin overrides every site, and a bad pin is ignored', () => {
  const hand = deal(42)
  const key = WAITING_SITE_KEYS[0]!
  assert.equal(resolveSite(hand, key, 'heartbeat').slug, 'heartbeat')
  assert.equal(resolveSite(hand, key, 'no-such-state').slug, hand[key])
  assert.equal(resolveSite(hand, key, null).slug, hand[key])
})

test('slugs are unique across both families', () => {
  // The registry is one flat map, so a collision between a braille id and a
  // grid id would silently drop one of them.
  assert.equal(new Set(WAITING_STATES.map((s) => s.slug)).size, WAITING_STATES.length)
})

test('every state lands inside its role range, as near 1x as that allows', () => {
  // This is what lets any field serve any role instead of bucketing the
  // catalogue by declared period (which would give `submitting` a pool of one).
  for (const state of WAITING_STATES) {
    for (const role of Object.keys(ROLE_RANGE) as WaitingRole[]) {
      const [lo, hi] = ROLE_RANGE[role]
      const speed = speedFor(state, role)
      const effective = state.period / speed
      assert.ok(
        effective >= lo - 1e-9 && effective <= hi + 1e-9,
        `${state.slug} @ ${role} → ${effective}ms, outside ${lo}-${hi}ms`,
      )
      // Nearest-to-1: a state already in range must not be re-timed at all.
      if (state.period >= lo && state.period <= hi) {
        assert.equal(speed, 1, `${state.slug} is already ${role}-paced but was re-timed`)
      }
    }
  }
})

test('no state is ever pushed past 2.7x or below 0.4x', () => {
  // The set was reviewed between 0.5x and 2x. Range-snapping keeps the worst
  // case at 2400/900 = 2.67x; a regression that widened the ranges or added a
  // longer-period field would show up here rather than as a blurred mark.
  for (const state of WAITING_STATES) {
    for (const role of Object.keys(ROLE_RANGE) as WaitingRole[]) {
      const speed = speedFor(state, role)
      assert.ok(speed <= 2.7 && speed >= 0.4, `${state.slug} @ ${role} runs at ${speed.toFixed(2)}x`)
    }
  }
})

test('every site key in the table is reachable and sorted', () => {
  const keys = Object.keys(WAITING_SITES) as WaitingSiteKey[]
  assert.deepEqual(WAITING_SITE_KEYS.slice(), keys.slice().sort())
  assert.equal(new Set(keys).size, keys.length)
})

// ── Inline sites: the SDK path, where the key set is open ───────────────────

test('an inline site is stable for a seed and moves with it', () => {
  const site = { key: 'my-app/summarise', role: 'reasoning' } as const
  assert.equal(pickInline(42, site).slug, pickInline(42, site).slug)
  const across = new Set([0, 1, 7, 42, 1337].map((s) => pickInline(s, site).slug))
  assert.ok(across.size > 1, 'inline pick ignores the seed')
})

test('inline sites respect their slot, and default to inline', () => {
  for (const seed of seeds) {
    for (const key of ['a', 'bb', 'ccc', 'app/x', 'app/y', 'zzzzzzzz']) {
      const btn = pickInline(seed, { key, role: 'submitting', slot: 'button' })
      assert.ok(btn.slots.includes('button'), `${key} → ${btn.slug} does not fit a button`)
      assert.equal(btn.family, 'braille')
      const dflt = pickInline(seed, { key, role: 'reasoning' })
      assert.ok(dflt.slots.includes('inline'), `${key} → ${dflt.slug} does not fit inline`)
    }
  }
})

test('inline keys spread across the pool rather than clumping', () => {
  // A weak hash would land most keys on one state and the SDK marks would all
  // look identical, which is the failure this path is most likely to have.
  const picks = new Set(
    Array.from({ length: 200 }, (_, i) => pickInline(42, { key: `app/site-${i}`, role: 'tool' }).slug),
  )
  assert.ok(picks.size >= 20, `200 keys hit only ${picks.size} distinct states`)
})

test('a pin overrides inline sites too', () => {
  assert.equal(pickInline(42, { key: 'x', role: 'tool' }, 'heartbeat').slug, 'heartbeat')
  assert.equal(pickInline(42, { key: 'x', role: 'tool' }, 'nope').slug, pickInline(42, { key: 'x', role: 'tool' }).slug)
})
