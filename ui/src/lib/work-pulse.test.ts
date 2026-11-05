import { describe, expect, it } from 'vitest'
import { agentName, behindLine, elapsed, since, spendLine, stateChip, stepLine, toolWord } from './work-pulse'

describe('work pulse labels', () => {
  it('names the agent the way the board does', () => {
    expect(agentName('doug-engineering')).toBe('Doug')
    expect(agentName(null)).toBe('The agent')
  })

  it('ticks a clock at the scale a step runs', () => {
    expect(elapsed(42_000)).toBe('42s')
    expect(elapsed(185_000)).toBe('3m 05s')
    expect(elapsed(3_840_000)).toBe('1h 04m')
    expect(elapsed(-5)).toBe('0s')
  })

  it('reads a server instant on the client clock', () => {
    // The server is 2s ahead of this browser.
    expect(since(1_000, 10_000, 2_000)).toBe(11_000)
    expect(since(null, 10_000, 0)).toBeNull()
  })

  it('spells a step as the tool and its argument', () => {
    expect(toolWord('terminal')).toBe('shell')
    expect(toolWord('mcp__talaria__get_ticket')).toBe('get_ticket')
    expect(stepLine({ tool: 'terminal', detail: 'cargo  test\n -p talaria-notify' })).toBe('shell · cargo test -p talaria-notify')
    expect(stepLine({ tool: 'memory', detail: '' })).toBe('memory')
    expect(stepLine(null)).toBe('')
  })

  it('tells a session waiting for its agent from one that is working', () => {
    expect(stateChip('sending', 'Doug', null)).toMatchObject({ label: 'waiting for Doug', tone: 'warn' })
    expect(stateChip('running', 'Doug', null)).toMatchObject({ label: 'running', tone: 'accent' })
    expect(stateChip('quiet', 'Doug', 400_000).label).toBe('quiet 6m 40s')
    expect(stateChip('queued', 'Doug', null).label).toBe('queued')
    expect(stateChip('awaiting', 'Doug', null).label).toBe('waiting for you')
    expect(stateChip('unknown', 'Doug', null).label).toBe('status unknown')
  })

  it('names only the tickets the reader may see', () => {
    expect(behindLine(['PLAT-1', 'PLAT-2'], 0)).toBe('behind PLAT-1, PLAT-2')
    expect(behindLine(['PLAT-1'], 2)).toBe('behind PLAT-1 and 2 other tickets')
    expect(behindLine([], 1)).toBe('behind 1 other ticket')
    expect(behindLine(undefined, undefined)).toBe('')
  })

  it('shows tokens and cost, or says why there are none', () => {
    expect(spendLine({ calls: 3, tokens: 48_200, cost: 0.312, unpriced: 0, missing: null }).text).toBe('48k tok · $0.31')
    expect(spendLine({ calls: 2, tokens: 900, cost: null, unpriced: 2, missing: null }).text).toBe('900 tok · unpriced')
    // Partly priced: the total says what it leaves out.
    expect(spendLine({ calls: 10, tokens: 48_200, cost: 1.2, unpriced: 3, missing: null }).text).toBe('48k tok · $1.20 + 3 unpriced')
    const shared = spendLine({ calls: 0, tokens: 0, cost: null, unpriced: 0, missing: { calls: 523, reason: 'shared-key' } })
    expect(shared.text).toBe('not metered to this ticket')
    expect(shared.title).toContain('523 came in')
    expect(shared.title).toContain('Roll the agent')
    expect(spendLine({ calls: 0, tokens: 0, cost: null, unpriced: 0, missing: { calls: 1, reason: 'untagged' } }).title).toContain('Hermes before v0.22')
    expect(spendLine({ calls: 0, tokens: 0, cost: null, unpriced: 0, missing: null }).text).toBe('no model calls yet')
  })
})
