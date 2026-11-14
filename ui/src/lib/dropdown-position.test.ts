import { describe, expect, it } from 'vitest'
import { anchorMoved, dropdownHorizStyle } from './dropdown-position'

describe('dropdownHorizStyle', () => {
  const panel = 288
  const view = 1280

  it('left-aligns a left-edge trigger so the panel grows into the window', () => {
    expect(dropdownHorizStyle('left', { left: 12, right: 80 }, view, panel)).toBe('left: 12px')
  })

  it('does not let a right-aligned left-edge trigger paint off-screen', () => {
    // Switcher in the nav rail: align=right would be `right: 1200px` and the
    // 288px panel would overflow the left edge of the window.
    const style = dropdownHorizStyle('right', { left: 12, right: 80 }, view, panel)
    expect(style.startsWith('left:')).toBe(true)
    const left = Number(style.slice('left: '.length, -2))
    expect(left).toBeGreaterThanOrEqual(8)
    expect(left + panel).toBeLessThanOrEqual(view - 8)
  })

  it('right-aligns a right-edge trigger', () => {
    expect(dropdownHorizStyle('right', { left: 1200, right: 1272 }, view, panel)).toBe('right: 8px')
  })

  it('flips a left-aligned right-edge trigger so it stays on screen', () => {
    const style = dropdownHorizStyle('left', { left: 1200, right: 1272 }, view, panel)
    expect(style.startsWith('right:')).toBe(true)
  })

  it('clamps a panel wider than the viewport to the left margin', () => {
    const style = dropdownHorizStyle('left', { left: 4, right: 40 }, 200, 288)
    expect(style).toBe('left: 8px')
  })
})

describe('anchorMoved', () => {
  const atOpen = { top: 410, left: 939 }

  it('is false when the trigger sits where it sat at open', () => {
    expect(anchorMoved({ top: 410, left: 939 }, atOpen)).toBe(false)
    expect(anchorMoved({ top: 410.4, left: 938.7 }, atOpen)).toBe(false)
  })

  it('is true once the page under the panel has scrolled the trigger away', () => {
    expect(anchorMoved({ top: 380, left: 939 }, atOpen)).toBe(true)
    expect(anchorMoved({ top: 410, left: 900 }, atOpen)).toBe(true)
  })

  it('is true when either spot is unknown, so the panel still closes', () => {
    expect(anchorMoved(undefined, atOpen)).toBe(true)
    expect(anchorMoved({ top: 410, left: 939 }, null)).toBe(true)
  })
})
