import { describe, expect, it } from 'vitest'
import { lastAssistantIndex, pendingCount } from './chat-queue'
import type { DisplayMessage } from './chat-view'

// The queue anchoring rules behind ChatView's live indicator and stream-event
// writes. The regression this pins: BOTH used to anchor to messages.length - 1,
// so the moment a queued user message became the last row, the still-streaming
// assistant turn went dark (indicator unmounted, "· saved (was in progress)")
// and content/tool events were dropped on the floor.

const user = (content = 'hi'): DisplayMessage => ({ role: 'user', content })
const assistant = (over: Partial<DisplayMessage> = {}): DisplayMessage => ({
  role: 'assistant',
  content: '',
  status: 'streaming',
  ...over,
})

describe('lastAssistantIndex', () => {
  it('finds no assistant row in an empty thread', () => {
    expect(lastAssistantIndex([])).toBe(-1)
  })

  it('finds the last assistant row', () => {
    const msgs = [user('q1'), assistant(), user('q2'), assistant({ status: 'complete' })]
    expect(lastAssistantIndex(msgs)).toBe(3)
  })

  it('keeps anchoring on the assistant turn when a queued user message lands after it', () => {
    const msgs = [user('q1'), assistant(), user('queued while streaming')]
    expect(lastAssistantIndex(msgs)).toBe(1)
  })

  it('returns -1 for a thread of only user rows', () => {
    expect(lastAssistantIndex([user('a'), user('b')])).toBe(-1)
  })
})

describe('pendingCount', () => {
  it('is zero on an empty thread', () => {
    expect(pendingCount([])).toBe(0)
  })

  it('is zero when a turn is actively streaming', () => {
    const msgs = [user('q1'), assistant({ status: 'streaming' })]
    expect(pendingCount(msgs)).toBe(0)
  })

  it('counts user rows queued behind a streaming turn', () => {
    const msgs = [user('q1'), assistant({ status: 'streaming' }), user('q2'), user('q3')]
    expect(pendingCount(msgs)).toBe(2)
  })

  it('counts user rows queued behind a resumed (server-owned) streaming turn', () => {
    const msgs = [user('q1'), assistant({ status: 'streaming', seq: 7 }), user('q2')]
    expect(pendingCount(msgs)).toBe(1)
  })

  it('is zero once every turn has resolved', () => {
    const msgs = [
      user('q1'),
      assistant({ status: 'complete' }),
      user('q2'),
      assistant({ status: 'complete' }),
    ]
    expect(pendingCount(msgs)).toBe(0)
  })

  it('is zero when the last turn errored — nothing is waiting', () => {
    const msgs = [user('q1'), assistant({ status: 'error' })]
    expect(pendingCount(msgs)).toBe(0)
  })
})
