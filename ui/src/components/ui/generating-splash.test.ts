import { describe, expect, it } from 'vitest'

import { SPLASH_ELAPSED_AFTER, splashLabel, splashSources } from './generating-splash'

// The two contracts the splash adds beyond what `Generating` already
// proved: the elapsed-count rule (honesty: say the wait only once it is
// genuinely long, then as a bare number — never a percentage, never a
// promise of how far along) and the field's shape (same material as the
// block: an edge lull plus two crossing waves, tuned — not copied — so the
// splash reads as its own moment).

describe('splashLabel: the elapsed count', () => {
  it('shows only the label while the wait is still unremarkable', () => {
    expect(splashLabel('Designing the agent', 0)).toBe('Designing the agent')
    expect(splashLabel('Designing the agent', SPLASH_ELAPSED_AFTER)).toBe('Designing the agent')
  })

  it('appends a bare elapsed count once the wait is genuinely long', () => {
    expect(splashLabel('Designing the agent', SPLASH_ELAPSED_AFTER + 1)).toBe(
      `Designing the agent — ${SPLASH_ELAPSED_AFTER + 1}s`,
    )
    expect(splashLabel('Designing the agent', 63)).toBe('Designing the agent — 63s')
  })

  it('never reports progress as a percentage or fraction', () => {
    for (const seconds of [0, 5, 11, 42, 300]) {
      const out = splashLabel('Designing the agent', seconds)
      expect(out).not.toMatch(/%|\d+\/\d+/)
      // The count, when present, is the ELAPSED count and nothing derived
      // from it — the surface cannot know the length, so it cannot promise
      // how far along it is.
      if (seconds > SPLASH_ELAPSED_AFTER) expect(out).toContain(`${seconds}s`)
    }
  })
})

describe('splashSources: the ambient field', () => {
  it('is the house material: one edge lull plus two travelling crests', () => {
    expect(splashSources).toHaveLength(3)
    expect(splashSources.filter((s) => s.kind === 'edge')).toHaveLength(1)
    expect(splashSources.filter((s) => s.kind === 'wave')).toHaveLength(2)
  })

  it('crosses the two crests on different axes, so the splash is not an enlarged copy of the block', () => {
    // Generating.svelte runs both waves along x; the splash's second crest
    // rides y. Same material, its own moment — the tuning the ticket asks
    // for, stated as a shape a regression can actually fail.
    const waves = splashSources.filter((s) => s.kind === 'wave') as Array<{ axis: string }>
    expect(waves[0]?.axis).toBe('x')
    expect(waves[1]?.axis).toBe('y')
  })

  it('gives every source an id and a strength the engine can tween', () => {
    for (const s of splashSources) {
      expect(s.id).toBeTruthy()
      expect(s.strength).toBeGreaterThan(0)
      expect(s.strength).toBeLessThanOrEqual(1)
    }
  })
})
