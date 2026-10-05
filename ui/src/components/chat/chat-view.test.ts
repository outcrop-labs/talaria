// Agent-DM transcript parity (U6): the pure halves of ChatView's message
// shape — the stored→display mapping, the reaction-chip mapping, the
// optimistic toggle, and the stamp merge that brings server ids/timestamps
// onto rows the client drew itself.
import { describe, expect, it } from 'vitest'
import type { StoredMessage } from '@/lib/conversations.svelte'
import { reactionChips, stampFromServer, toDisplay, toggleReactionLocal, turnTime, type DisplayMessage } from './chat-view'

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
