import { describe, expect, it, vi } from 'vitest'
import type { AgentContainers, AgentDef } from '@/lib/fleet-defs'

// The note a control leaves when the route answers 202 (the verb outlived its
// 210 s answer and goes on). The module's hooks pull svelte-query and the
// session; mocked away so the import resolves (see agents-roster-health.test.ts
// for why this lives here and not beside routes/app/agents.svelte.ts).
vi.mock('@tanstack/svelte-query', () => ({ useQueryClient: vi.fn() }))
vi.mock('@/lib/session', () => ({ useSession: vi.fn() }))

import {
  noteStillWorking,
  settleStillWorking,
  STILL_WORKING_LABEL,
  stillWorkingArrived,
  stillWorkingOf,
} from '@/routes/app/agents.svelte'

const def = (over: Partial<AgentDef> = {}): AgentDef =>
  ({
    id: 'a1',
    slug: 'ada',
    department: 'ops',
    displayName: 'Ada',
    enabled: true,
    managed: true,
    source: 'created',
    currentVersion: 1,
    ...over,
  }) as AgentDef

const box = (state: string | null, health: 'starting' | 'healthy' | 'unhealthy' | null = null): AgentContainers => ({
  department: 'ops',
  managed: state ? { name: 'talaria-fleet-agent-ops-1', state, status: state, health } : null,
})

describe('a control still being carried out', () => {
  it('names what it is still doing, in the roster’s own words', () => {
    expect(STILL_WORKING_LABEL.stop).toBe('still stopping')
    expect(STILL_WORKING_LABEL.up).toBe('still starting')
    expect(STILL_WORKING_LABEL.unretire).toBe('still re-hiring')
  })

  it('holds a stop until the roster shows the agent stopped', () => {
    noteStillWorking('s1', 'stop', 'running')
    const d = def({ id: 's1' })
    settleStillWorking(d, box('running', 'healthy'))
    expect(stillWorkingOf('s1')?.action).toBe('stop')
    settleStillWorking(d, box('exited'))
    expect(stillWorkingOf('s1')).toBeNull()
  })

  it('holds a start until it runs, warming counts', () => {
    noteStillWorking('u1', 'up', 'stopped')
    const d = def({ id: 'u1' })
    settleStillWorking(d, box('exited'))
    expect(stillWorkingOf('u1')).not.toBeNull()
    settleStillWorking(d, box('running', 'starting'))
    expect(stillWorkingOf('u1')).toBeNull()
  })

  it('holds a restart through the running reading before its bounce', () => {
    noteStillWorking('r1', 'restart', 'running')
    const d = def({ id: 'r1' })
    // Still the old container: running, healthy — not the restart landing.
    settleStillWorking(d, box('running', 'healthy'))
    expect(stillWorkingOf('r1')).not.toBeNull()
    // The bounce: down, then up again.
    settleStillWorking(d, box('exited'))
    expect(stillWorkingOf('r1')?.bounced).toBe(true)
    settleStillWorking(d, box('running', 'starting'))
    expect(stillWorkingOf('r1')).toBeNull()
  })

  it('holds a retire until the def is retired and nothing runs', () => {
    noteStillWorking('t1', 'retire', 'running')
    settleStillWorking(def({ id: 't1', enabled: false }), box('running', 'healthy'))
    expect(stillWorkingOf('t1')).not.toBeNull()
    settleStillWorking(def({ id: 't1', enabled: false }), box(null))
    expect(stillWorkingOf('t1')).toBeNull()
  })

  it('stops claiming anything once a verb could no longer be running', () => {
    const w = { action: 'stop' as const, since: 0, bounced: false }
    expect(stillWorkingArrived(w, def(), box('running', 'healthy'), 19 * 60_000)).toBe(false)
    expect(stillWorkingArrived(w, def(), box('running', 'healthy'), 21 * 60_000)).toBe(true)
  })
})
