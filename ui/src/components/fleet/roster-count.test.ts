import { describe, expect, it, vi } from 'vitest'
import type { AgentContainers, AgentDef } from '@/lib/fleet-defs'

// The same seam as agents-roster-health.test.ts: the health module's hooks
// pull svelte-query and the session, which a node test has no use for.
vi.mock('@tanstack/svelte-query', () => ({ useQueryClient: vi.fn() }))
vi.mock('@/lib/session', () => ({ useSession: vi.fn() }))

import { rosterCount, rosterLine } from './roster-count'

type Container = NonNullable<AgentContainers['managed']>

const def = (slug: string, over: Partial<AgentDef> = {}): AgentDef =>
  ({ id: slug, slug, department: slug, displayName: slug, enabled: true, managed: true, source: 'created', currentVersion: 1, ...over }) as AgentDef

const fleet = (states: Record<string, Partial<Container> | null>): Map<string, AgentContainers> =>
  new Map(
    Object.entries(states).map(([department, managed]) => [
      department,
      { department, managed: managed && ({ name: department, state: 'running', status: 'Up', health: 'healthy', ...managed } as Container) },
    ]),
  )

describe('rosterCount', () => {
  it('counts running out of the agents meant to be up, and names the rest', () => {
    const defs = [
      ...['a', 'b', 'c', 'd', 'e'].map((s) => def(s)),
      def('sick'),
      def('booting'),
      def('draft'),
      def('gone', { enabled: false }),
    ]
    const count = rosterCount(
      defs,
      fleet({ a: {}, b: {}, c: {}, d: {}, e: {}, sick: { health: 'unhealthy' }, booting: { health: 'starting' }, gone: {} }),
    )
    expect(count.running).toBe(5)
    expect(count.counted).toBe(7)
    expect(count.exceptions).toEqual(['1 unhealthy', '1 starting'])
    expect(count.uncounted).toEqual(['1 draft', '1 retired'])
    expect(rosterLine(count)).toEqual(['5 of 7 running', '1 unhealthy', '1 starting', '1 draft and 1 retired not counted'])
  })

  it('counts a stopped agent, because it is meant to be up', () => {
    const count = rosterCount([def('a'), def('b')], fleet({ a: {}, b: { state: 'exited', health: null } }))
    expect(rosterLine(count)).toEqual(['1 of 2 running', '1 stopped'])
  })

  it('says nothing more when every counted agent is running', () => {
    expect(rosterLine(rosterCount([def('a'), def('b')], fleet({ a: {}, b: {} })))).toEqual(['2 of 2 running'])
  })

  it('does not read a department another agent runs as its own container', () => {
    const shared = def('second', { department: 'a', source: 'imported', departmentShare: { runs: false, runner: null, others: [] } })
    expect(rosterLine(rosterCount([def('a'), shared], fleet({ a: {} })))).toEqual(['1 of 2 running', '1 stopped'])
  })

  it('names who is left out when nobody is counted, and is empty for an empty roster', () => {
    expect(rosterLine(rosterCount([def('draft'), def('gone', { enabled: false })], fleet({})))).toEqual([
      'None running',
      '1 draft and 1 retired not counted',
    ])
    expect(rosterLine(rosterCount([], fleet({})))).toEqual([])
  })
})
