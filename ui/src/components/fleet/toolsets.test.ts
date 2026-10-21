import { describe, expect, it } from 'vitest'
import { MANDATORY_DISABLED, normalizeToolsets, sameToolsets, toggleToolset, toolsetRows } from './toolsets'

// The governed list as the server publishes it, trimmed to the names these
// cases need. Alphabetical, because that is the contract the grid relies on
// for its reading order.
const GOVERNED = ['browser', 'clarify', 'coding', 'memory', 'todo', 'web']
const FLOOR = ['todo', 'clarify']

describe('toolsetRows: the checkbox is the toolset, the list is its absence', () => {
  it('reads an empty disabled list as every governed toolset on, bar the floor', () => {
    const rows = toolsetRows(GOVERNED, FLOOR, [])
    expect(rows.map((r) => r.name)).toEqual(GOVERNED)
    expect(rows.filter((r) => r.on).map((r) => r.name)).toEqual(['browser', 'coding', 'memory', 'web'])
    expect(rows.filter((r) => r.forced).map((r) => r.name)).toEqual(['clarify', 'todo'])
    // Forced rows are off, which is the only honest rendering: the renderer
    // closes them whatever the def says.
    expect(rows.filter((r) => r.forced).every((r) => !r.on)).toBe(true)
  })

  it('turns a disabled name off without touching its neighbours', () => {
    const rows = toolsetRows(GOVERNED, FLOOR, ['browser'])
    expect(rows.find((r) => r.name === 'browser')).toEqual({ name: 'browser', on: false, forced: false })
    expect(rows.find((r) => r.name === 'web')?.on).toBe(true)
  })

  it('renders a legacy def that stored a forced name identically to one that did not', () => {
    // Same rendered config, so the same grid — otherwise an operator reads a
    // difference that does not exist downstream.
    expect(toolsetRows(GOVERNED, FLOOR, ['todo', 'browser'])).toEqual(toolsetRows(GOVERNED, FLOOR, ['browser']))
  })

  it('dedupes and trims the governed list, and drops a blank name', () => {
    const rows = toolsetRows([' web ', 'web', '', 'browser'], FLOOR, [])
    expect(rows.map((r) => r.name)).toEqual(['web', 'browser'])
  })

  it('matches a disabled name that arrived with whitespace', () => {
    expect(toolsetRows(GOVERNED, FLOOR, [' browser '])?.find((r) => r.name === 'browser')?.on).toBe(false)
  })

  it('forces nothing when the server publishes no floor', () => {
    const rows = toolsetRows(GOVERNED, [], [])
    expect(rows.some((r) => r.forced)).toBe(false)
    expect(rows.every((r) => r.on)).toBe(true)
  })
})

describe('toggleToolset: the whole list, sorted and deduped', () => {
  it('turning a toolset OFF adds it to the disabled list', () => {
    expect(toggleToolset([], 'browser', false, FLOOR)).toEqual(['browser'])
  })

  it('turning a toolset ON removes it', () => {
    expect(toggleToolset(['browser', 'web'], 'browser', true, FLOOR)).toEqual(['web'])
  })

  it('sorts and dedupes, so two routes to one set produce one list', () => {
    const a = toggleToolset(toggleToolset([], 'web', false, FLOOR), 'browser', false, FLOOR)
    const b = toggleToolset(toggleToolset([], 'browser', false, FLOOR), 'web', false, FLOOR)
    expect(a).toEqual(['browser', 'web'])
    expect(a).toEqual(b)
    expect(toggleToolset(['web', 'web', ' web '], 'browser', false, FLOOR)).toEqual(['browser', 'web'])
  })

  it('never stores a forced name, whichever way it is flipped', () => {
    expect(toggleToolset([], 'todo', false, FLOOR)).toEqual([])
    expect(toggleToolset(['todo', 'clarify', 'web'], 'browser', false, FLOOR)).toEqual(['browser', 'web'])
    expect(toggleToolset(['todo'], 'todo', true, FLOOR)).toEqual([])
  })

  it('still refuses the floor when the caller passes no forced list', () => {
    // The three-argument call is the contract other code may reach for; it
    // must not be the door a mandatory name walks through.
    expect(toggleToolset([], 'todo', false)).toEqual([])
    expect(toggleToolset([], 'clarify', false)).toEqual([])
    // The server constant's own order (talaria-agent-defs::MANDATORY_DISABLED).
    expect(MANDATORY_DISABLED).toEqual(['todo', 'clarify'])
  })

  it('leaves the caller’s list alone', () => {
    const before = ['web']
    toggleToolset(before, 'browser', false, FLOOR)
    expect(before).toEqual(['web'])
  })
})

describe('sameToolsets: order is not a change', () => {
  it('compares as sets, so the server order and the sorted order agree', () => {
    expect(sameToolsets(['web', 'browser'], ['browser', 'web'])).toBe(true)
    expect(sameToolsets([], [])).toBe(true)
  })

  it('sees a real difference', () => {
    expect(sameToolsets(['web'], ['web', 'browser'])).toBe(false)
    expect(sameToolsets(['web'], ['browser'])).toBe(false)
  })

  it('does not call a duplicate a difference', () => {
    expect(sameToolsets(['web', 'web'], ['web'])).toBe(true)
  })
})

describe('normalizeToolsets: the storable spelling', () => {
  it('sorts, trims, dedupes and drops the floor', () => {
    expect(normalizeToolsets([' web ', 'browser', 'web', 'todo', '', 'clarify'], FLOOR)).toEqual(['browser', 'web'])
  })

  it('reads a legacy def carrying a forced name as the same list without it', () => {
    // Which is what makes the Apply button honest: the rendered config for
    // these two is identical, so the grid must not offer a roll between them.
    expect(normalizeToolsets(['todo', 'browser'], FLOOR)).toEqual(normalizeToolsets(['browser'], FLOOR))
  })

  it('drops the floor with no forced list passed', () => {
    expect(normalizeToolsets(['todo', 'clarify', 'web'])).toEqual(['web'])
  })

  it('leaves the caller\u2019s list alone', () => {
    const before = ['web', 'browser']
    normalizeToolsets(before, FLOOR)
    expect(before).toEqual(['web', 'browser'])
  })
})
