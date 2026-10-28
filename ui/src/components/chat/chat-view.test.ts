// Agent-DM transcript parity (U6): the pure halves of ChatView's message
// shape — the stored→display mapping, the reaction-chip mapping, the
// optimistic toggle, and the stamp merge that brings server ids/timestamps
// onto rows the client drew itself.
import { describe, expect, it } from 'vitest'
import type { StoredMessage } from '@/lib/conversations.svelte'
import {
  cronJobFromMarker,
  foldToolVerdicts,
  reactionChips,
  sendRoute,
  stampFromServer,
  toDisplay,
  toggleReactionLocal,
  turnTime,
  type DisplayMessage,
} from './chat-view'

const stored = (over: Partial<StoredMessage> = {}): StoredMessage => ({
  role: 'assistant',
  content: 'hi',
  reasoning: '',
  tools: [],
  status: 'complete',
  seq: 3,
  ...over,
})

describe('toDisplay', () => {
  it('maps id, createdAt and reactions from a stored message', () => {
    const d = toDisplay(
      stored({
        id: 'm-1',
        createdAt: '2026-10-05T12:00:00.000Z',
        reactions: [{ emoji: '🙌', actors: ['a@x.co'], actorTypes: ['user'] }],
      }),
    )
    expect(d.id).toBe('m-1')
    expect(d.createdAt).toBe('2026-10-05T12:00:00.000Z')
    expect(d.reactions).toEqual([{ emoji: '🙌', actors: ['a@x.co'], actorTypes: ['user'] }])
  })

  it('leaves the fields absent on an older payload', () => {
    const d = toDisplay(stored())
    expect(d.id).toBeUndefined()
    expect(d.createdAt).toBeUndefined()
    expect(d.reactions).toBeUndefined()
  })
})

describe('reactionChips', () => {
  const me = { id: 'u-1', email: 'me@x.co' }

  it('counts actors and marks mine by email', () => {
    const chips = reactionChips([{ emoji: '🙌', actors: ['me@x.co', 'b@x.co'], actorTypes: ['user', 'user'] }], me)
    expect(chips).toEqual([{ emoji: '🙌', count: 2, mine: true, title: 'You, b' }])
  })

  it('marks mine by user id too', () => {
    const chips = reactionChips([{ emoji: '👀', actors: ['u-1'], actorTypes: ['user'] }], me)
    expect(chips[0]?.mine).toBe(true)
  })

  it('an agent actor with the same string is never mine', () => {
    const chips = reactionChips([{ emoji: '✅', actors: ['u-1'], actorTypes: ['agent'] }], me, (a) => `Agent ${a}`)
    expect(chips[0]).toEqual({ emoji: '✅', count: 1, mine: false, title: 'Agent u-1' })
  })

  it('no reactions → no chips', () => {
    expect(reactionChips(undefined, me)).toEqual([])
  })
})

describe('toggleReactionLocal', () => {
  const me = { id: 'u-1', email: 'me@x.co' }

  it('adds a new emoji as the viewer', () => {
    expect(toggleReactionLocal([], '🙌', me)).toEqual([{ emoji: '🙌', actors: ['me@x.co'], actorTypes: ['user'] }])
  })

  it('joins an existing emoji', () => {
    const next = toggleReactionLocal([{ emoji: '🙌', actors: ['b@x.co'], actorTypes: ['user'] }], '🙌', me)
    expect(next).toEqual([{ emoji: '🙌', actors: ['b@x.co', 'me@x.co'], actorTypes: ['user', 'user'] }])
  })

  it('removes the viewer, dropping an emptied emoji', () => {
    expect(toggleReactionLocal([{ emoji: '🙌', actors: ['u-1'], actorTypes: ['user'] }], '🙌', me)).toEqual([])
  })
})

describe('stampFromServer', () => {
  it('copies id/createdAt/seq/reactions without touching the local content', () => {
    const local: DisplayMessage[] = [
      { role: 'user', content: 'q', createdAt: 'local-1' },
      { role: 'assistant', content: 'partial', status: 'complete' },
    ]
    const remote = [
      stored({ role: 'user', content: 'q', id: 'u', seq: 1, createdAt: '2026-10-05T10:00:00Z' }),
      stored({ role: 'assistant', content: 'partial and more', status: 'streaming', id: 'a', seq: 2, createdAt: '2026-10-05T10:00:01Z', reactions: [] }),
    ]
    const out = stampFromServer(local, remote)
    expect(out[0]).toMatchObject({ content: 'q', id: 'u', seq: 1, createdAt: '2026-10-05T10:00:00Z' })
    expect(out[1]).toMatchObject({ content: 'partial', status: 'complete', id: 'a', seq: 2, reactions: [] })
  })

  it('leaves a row alone when the roles disagree', () => {
    const local: DisplayMessage[] = [{ role: 'user', content: 'q' }]
    const out = stampFromServer(local, [stored({ role: 'assistant', id: 'x' })])
    expect(out[0]?.id).toBeUndefined()
  })
})

describe('turnTime', () => {
  it('formats in the given zone', () => {
    expect(turnTime('2026-10-05T12:05:00Z', 'UTC')).toMatch(/12:05|12\.05/)
  })

  it('a bad zone falls back to the browser zone instead of throwing', () => {
    expect(turnTime('2026-10-05T12:05:00Z', 'Not/AZone')).not.toBe('')
  })

  it('no or unparseable time → empty', () => {
    expect(turnTime(undefined)).toBe('')
    expect(turnTime('nope')).toBe('')
  })
})

describe('toDisplay — the row stamps', () => {
  // `stopped` and `steer` ride in the same `metadata` bag as `resumed`,
  // which is the only one the conversations query types today; the cast is
  // this test standing in for the wire until that DTO names them.
  const stamped = (metadata: Record<string, boolean>): StoredMessage =>
    ({ ...stored(), metadata }) as unknown as StoredMessage

  it('reads stopped and steer beside resumed', () => {
    const d = toDisplay(stamped({ resumed: true, stopped: true, steer: true }))
    expect(d.resumed).toBe(true)
    expect(d.stopped).toBe(true)
    expect(d.steer).toBe(true)
  })

  it('a row with no stamps is neither stopped nor steered', () => {
    const d = toDisplay(stored())
    expect(d.stopped).toBe(false)
    expect(d.steer).toBe(false)
  })
})

describe('cronJobFromMarker', () => {
  // The server's shape is `cron:<job>:<execution>:<turn>` — the marker names
  // the EXECUTION, so only the second field identifies the schedule.
  it('reads the job out of the second field', () => {
    expect(cronJobFromMarker('cron:abc123:ff01:t7')).toBe('abc123')
  })

  it('a marker with an empty turn field still names the job', () => {
    expect(cronJobFromMarker('cron:abc123:ff01:')).toBe('abc123')
  })

  it('every other marker shape is not a scheduled run', () => {
    for (const raw of ['cron:', 'cron::ff01:t7', 'research:r-1', 'cronjob:abc', '', undefined, null]) {
      expect(cronJobFromMarker(raw)).toBeUndefined()
    }
  })
})

describe('toDisplay — the cron stamp', () => {
  // `marker` rides the same metadata bag as the three stamps above — one
  // jsonb row stamp, declared on the wire type beside them.
  const marked = (marker: string): StoredMessage => ({ ...stored(), metadata: { marker } })

  it('stamps the job a scheduled turn came from', () => {
    expect(toDisplay(marked('cron:job-9:ff01:t2')).cronJobId).toBe('job-9')
  })

  it('a turn somebody asked for carries no job', () => {
    expect(toDisplay(stored()).cronJobId).toBeUndefined()
    // A marker from another engine is not a schedule either: the research
    // driver stamps its own posts through the same idempotency key.
    expect(toDisplay(marked('research:r-1:report')).cronJobId).toBeUndefined()
  })
})

describe('sendRoute', () => {
  const ctx = (over: Partial<Parameters<typeof sendRoute>[0]> = {}) =>
    sendRoute({ streaming: true, intent: 'enter', hasConversation: true, ...over })

  it('nothing streaming → a fresh turn, whichever way it was sent', () => {
    expect(ctx({ streaming: false })).toBe('fresh')
    expect(ctx({ streaming: false, intent: 'send-now' })).toBe('fresh')
    // Even with no conversation yet: that turn is what creates one.
    expect(ctx({ streaming: false, hasConversation: false })).toBe('fresh')
  })

  it('Enter during a reply queues — a send never interrupts', () => {
    expect(ctx()).toBe('queue')
  })

  it('send-now during a reply steers it into the running turn', () => {
    expect(ctx({ intent: 'send-now' })).toBe('steer')
  })

  it('no conversation id yet → held on the client, never a steer', () => {
    // The first turn's response headers are the only place an id comes from,
    // and a steer has no address until it lands.
    expect(ctx({ hasConversation: false })).toBe('hold')
    expect(ctx({ hasConversation: false, intent: 'send-now' })).toBe('hold')
  })

  it('attachments go the queue: the steer wire carries text only', () => {
    expect(ctx({ intent: 'send-now', hasAttachments: true })).toBe('queue')
    expect(ctx({ hasAttachments: true })).toBe('queue')
  })
})

// The agent's verdict on a call, brought onto a row the stream drew. The live
// shape: Hermes' completions stream ended the refused `rm` `completed`; the
// server's row says `error` (the tool-events door marked it).
describe('foldToolVerdicts', () => {
  const rm = { id: 'call_8b732cd27d', name: 'terminal', label: '💻 rm -rf /tmp/fu-canary', status: 'completed' as const }
  const echo = { id: 'call_547dfba246', name: 'terminal', label: '💻 echo fu-safe-ok', status: 'completed' as const }
  const failed = { ...rm, status: 'error' as const }
  const user = (id: string): DisplayMessage => ({ role: 'user', content: 'go', id })

  it('marks a landed row by id, only the failed call, and nothing else on it', () => {
    const local: DisplayMessage[] = [user('u1'), { role: 'assistant', content: 'done', id: 'a1', tools: [rm, echo], chips: [] }]
    const remote = [stored({ id: 'u1', role: 'user' }), stored({ id: 'a1', content: 'server prose', tools: [failed, echo] })]
    const next = foldToolVerdicts(local, remote)
    expect(next[1]!.tools!.map((t) => t.status)).toEqual(['error', 'completed'])
    // Prose and every other field stay what is on screen.
    expect(next[1]!.content).toBe('done')
    expect(next[0]).toBe(local[0])
  })

  it('pairs the live row (no id yet) with the newest assistant row', () => {
    const local: DisplayMessage[] = [user('u1'), { role: 'assistant', content: 'stre', tools: [rm], status: 'streaming' }]
    const remote = [stored({ id: 'u1', role: 'user' }), stored({ id: 'a-live', status: 'streaming', tools: [failed] })]
    expect(foldToolVerdicts(local, remote)[1]!.tools![0]!.status).toBe('error')
  })

  it('never pairs the live row with a row already on screen, nor a call by name', () => {
    // The server's newest row is the earlier turn, still on screen by id: the
    // live row (the same command, a call id Hermes minted again) is not it.
    const earlier: DisplayMessage = { role: 'assistant', content: 'one', id: 'a1', tools: [failed] }
    const live: DisplayMessage = { role: 'assistant', content: 'two', tools: [rm], status: 'streaming' }
    const local = [user('u1'), earlier, user('u2'), live]
    const remote = [stored({ id: 'u1', role: 'user' }), stored({ id: 'a1', tools: [failed] }), stored({ id: 'u2', role: 'user' })]
    expect(foldToolVerdicts(local, remote)).toBe(local)
    // A call with no id is never matched.
    const bare: DisplayMessage[] = [{ role: 'assistant', content: 'x', id: 'a1', tools: [{ ...rm, id: undefined }] }]
    expect(foldToolVerdicts(bare, [stored({ id: 'a1', tools: [{ ...failed, id: undefined }] })])).toBe(bare)
  })

  it('only ever moves toward error, and answers the same array when nothing changed', () => {
    const local: DisplayMessage[] = [{ role: 'assistant', content: 'x', id: 'a1', tools: [failed, echo] }]
    // The server saying `completed` for a call shown failed changes nothing.
    expect(foldToolVerdicts(local, [stored({ id: 'a1', tools: [rm, echo] })])).toBe(local)
    expect(foldToolVerdicts(local, [stored({ id: 'a1', tools: [failed, echo] })])).toBe(local)
    expect(foldToolVerdicts(local, [])).toBe(local)
  })
})
