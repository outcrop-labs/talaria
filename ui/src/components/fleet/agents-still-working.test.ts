import { describe, expect, it, vi } from 'vitest'
import type { AgentContainers, AgentDef, FleetAction, FleetRollRow } from '@/lib/fleet-defs'

// The note a control leaves when the route answers 202 (the verb outlived its
// 210 s answer and goes on). The module's hooks pull svelte-query and the
// session; mocked away so the import resolves (see agents-roster-health.test.ts
// for why this lives here and not beside routes/app/agents.svelte.ts).
vi.mock('@tanstack/svelte-query', () => ({ useQueryClient: vi.fn() }))
vi.mock('@/lib/session', () => ({ useSession: vi.fn() }))

import {
  controlAnswerToast,
  noteStillApplying,
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

describe('a saved change whose apply is still going on', () => {
  const row = (state: FleetRollRow['state'], since: number | null, detail: string | null = null): FleetRollRow => ({
    slug: 'ada',
    department: 'ops',
    displayName: 'Ada',
    state,
    since,
    detail,
    batch: null,
  })

  it('names it in the roster’s words', () => {
    expect(STILL_WORKING_LABEL.apply).toBe('still applying')
  })

  it('holds until the roll picture has its roll’s verdict', () => {
    const at = 1_000_000_000
    noteStillWorking('p1', 'apply', undefined, at)
    const d = def({ id: 'p1' })
    const w = stillWorkingOf('p1')!
    // Rolling (and still starting) is not a verdict.
    expect(stillWorkingArrived(w, d, box('running', 'starting'), at + 1_000, row('rolling', at - 200_000))).toBe(false)
    expect(stillWorkingArrived(w, d, box('running', 'starting'), at + 1_000, row('rolling', at - 200_000, 'still starting'))).toBe(false)
    // Done or failed, for a roll that began in this apply's window: arrived.
    expect(stillWorkingArrived(w, d, box('running', 'healthy'), at + 1_000, row('done', at - 200_000))).toBe(true)
    expect(stillWorkingArrived(w, d, box('exited'), at + 1_000, row('failed', at - 200_000))).toBe(true)
    // An older roll's row says nothing of this apply: the bounce rule decides.
    expect(stillWorkingArrived(w, d, box('running', 'healthy'), at + 1_000, row('done', at - 900_000))).toBe(false)
  })

  // A roll that ended just before the Save & apply is not this apply's:
  // its row's `since` is before the request went out. Only a roll that
  // began after the send (with the server's clock a little behind the
  // browser's) is read as the apply's verdict.
  it('reads only a roll that began after its request went out', () => {
    const sentAt = 2_000_000_000
    const at = sentAt + 212_000 // the 202, at the route's bound
    noteStillApplying('p3', sentAt, at)
    const d = def({ id: 'p3' })
    const w = stillWorkingOf('p3')!
    expect(w.sentAt).toBe(sentAt)
    // Done 30 s before the save: an older roll, whatever the 300 s window
    // would have said.
    expect(stillWorkingArrived(w, d, box('running', 'healthy'), at + 1_000, row('done', sentAt - 90_000))).toBe(false)
    expect(stillWorkingArrived(w, d, box('running', 'healthy'), at + 1_000, row('failed', sentAt - 30_000))).toBe(false)
    // Began after the send: this apply's roll.
    expect(stillWorkingArrived(w, d, box('running', 'healthy'), at + 1_000, row('done', sentAt + 40_000))).toBe(true)
    expect(stillWorkingArrived(w, d, box('running', 'starting'), at + 1_000, row('rolling', sentAt + 40_000))).toBe(false)
    // A server clock a few seconds behind the browser's still reads as it.
    expect(stillWorkingArrived(w, d, box('running', 'healthy'), at + 1_000, row('done', sentAt - 5_000))).toBe(true)
  })

  it('without the roll picture, holds through the bounce until the agent is healthy again', () => {
    noteStillWorking('p2', 'apply')
    const d = def({ id: 'p2' })
    // Still the old container, healthy: not arrived.
    settleStillWorking(d, box('running', 'healthy'))
    expect(stillWorkingOf('p2')).not.toBeNull()
    // The roll's recreate: down, then warming — still applying.
    settleStillWorking(d, box('exited'))
    settleStillWorking(d, box('running', 'starting'))
    expect(stillWorkingOf('p2')?.bounced).toBe(true)
    // Healthy on the new container: arrived.
    settleStillWorking(d, box('running', 'healthy'))
    expect(stillWorkingOf('p2')).toBeNull()
  })
})

// A refused or failed control used to resolve `{ error }` into nothing: the
// roster stayed as it was with no word of why. Every verb's answer is said.
describe('a control the route refused or failed', () => {
  const verbs: Array<[FleetAction, string]> = [
    ['up', 'start'],
    ['stop', 'stop'],
    ['restart', 'restart'],
    ['roll', 'roll'],
    ['retire', 'retire'],
    ['unretire', 're-hire'],
    ['delete', 'delete'],
  ]

  it('says every verb’s refusal and failure, in the route’s own sentence', () => {
    for (const [action, verb] of verbs) {
      const refused = controlAnswerToast(action, 'Ada', { error: 'this agent is already rolling — wait for it to finish', status: 409 })
      expect(refused).toEqual({ title: `Could not ${verb} Ada`, body: 'this agent is already rolling — wait for it to finish', tone: 'danger' })
      const failed = controlAnswerToast(action, 'Ada', { error: `could not ${verb} the agent — see server logs`, status: 500 })
      expect(failed?.title).toBe(`Could not ${verb} Ada`)
      expect(failed?.body).toBe(`could not ${verb} the agent — see server logs`)
      // No answer at all (the network, the proxy): said the same way.
      expect(controlAnswerToast(action, 'Ada', { error: 'Could not reach the server.' })?.title).toBe(`Could not ${verb} Ada`)
    }
  })

  it('does not call an unconfirmed verb a failure', () => {
    const t = controlAnswerToast('stop', 'Ada', {
      error: 'could not confirm the agent’s stop — docker is not answering; the roster shows its state once it does',
      status: 503,
    })
    expect(t?.title).toBe('Ada: the stop is not confirmed yet')
    expect(t?.title).not.toMatch(/could not/i)
  })

  it('says nothing for a control that landed or is still landing', () => {
    expect(controlAnswerToast('up', 'Ada', { ok: true, warming: true })).toBeNull()
    expect(controlAnswerToast('stop', 'Ada', { ok: true, pending: true, state: 'running' })).toBeNull()
    expect(controlAnswerToast('roll', 'Ada', { ok: true, rolling: true })).toBeNull()
  })
})
