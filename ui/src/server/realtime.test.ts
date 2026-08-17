// The mirroring half of runs: topics, payload shape, and who may attach.
//
// Redis is faked at the `ioredis` module boundary rather than mocked per call,
// because the thing under test IS the routing — "a run event reaches the
// subscriber on that run's topic and no other" is a statement about channel
// names, and a mock that records `publish(channel, payload)` without delivering
// anything cannot make it. The fake is a real (in-process) pub/sub: subscribers
// register channels, `publish` fans out to the ones that match.
import { describe, expect, it, vi } from 'vitest'

const { FakeRedis, hub } = vi.hoisted(() => {
  type MessageHandler = (channel: string, message: string) => void
  const hub: FakeRedisImpl[] = []

  class FakeRedisImpl {
    readonly channels = new Set<string>()
    readonly handlers: MessageHandler[] = []
    disconnected = false

    constructor(..._args: unknown[]) {
      hub.push(this)
    }

    on(event: string, cb: (...args: never[]) => void): this {
      if (event === 'message') this.handlers.push(cb as unknown as MessageHandler)
      return this
    }

    async subscribe(channel: string): Promise<number> {
      this.channels.add(channel)
      return this.channels.size
    }

    async publish(channel: string, message: string): Promise<number> {
      let delivered = 0
      for (const sub of hub) {
        if (!sub.channels.has(channel)) continue
        delivered++
        for (const h of sub.handlers) h(channel, message)
      }
      return delivered
    }

    disconnect(): void {
      this.disconnected = true
      this.channels.clear()
    }
  }

  return { FakeRedis: FakeRedisImpl, hub }
})

vi.mock('ioredis', () => ({ default: FakeRedis }))

process.env.REDIS_URL = 'redis://fake'

import { mayWatchRun, publishRun, publishUser, runEventStream, userEventStream, type RunEvent, type RunWatchDeps, type UserEvent } from './realtime'

// ── Stream helpers ───────────────────────────────────────────────────────────

interface Attached {
  next: () => Promise<string>
  abort: () => void
}

/** Attach to a stream and consume the `: connected` preamble, so every later
 *  read in a test is an actual event. */
async function attach(stream: ReadableStream<Uint8Array>, ac: AbortController): Promise<Attached> {
  const reader = stream.getReader()
  const dec = new TextDecoder()
  const next = async (): Promise<string> => {
    const { value } = await reader.read()
    return value ? dec.decode(value) : ''
  }
  const preamble = await next()
  expect(preamble).toBe(': connected\n\n')
  return { next, abort: () => ac.abort() }
}

/** The JSON body of one `data:` frame. */
function frame(chunk: string): Record<string, unknown> {
  expect(chunk.startsWith('data: ')).toBe(true)
  return JSON.parse(chunk.slice('data: '.length)) as Record<string, unknown>
}

const runEvent = (over: Partial<RunEvent> = {}): RunEvent => ({
  type: 'run',
  runId: 'run-a',
  kind: 'demo',
  state: 'running',
  phase: 'working',
  ...over,
})

// ── Topics ───────────────────────────────────────────────────────────────────

describe('run:<id>', () => {
  it('delivers a published run event to a subscriber on that run and to nobody else', async () => {
    const acA = new AbortController()
    const acB = new AbortController()
    const a = await attach(runEventStream('run-a', acA.signal), acA)
    const b = await attach(runEventStream('run-b', acB.signal), acB)

    publishRun('run-a', runEvent({ phase: 'for a' }))
    // Then a DIFFERENT event on b's own topic. If the first publish had leaked
    // across topics, b's first frame would be that one — reading b and finding
    // only its own event is what proves the isolation, without racing a
    // timeout to assert an absence.
    publishRun('run-b', runEvent({ runId: 'run-b', phase: 'for b' }))

    expect(frame(await a.next())).toMatchObject({ runId: 'run-a', phase: 'for a' })
    const onB = frame(await b.next())
    expect(onB).toMatchObject({ runId: 'run-b', phase: 'for b' })

    a.abort()
    b.abort()
  })

  it('never carries the decision question, even when the caller is holding one', async () => {
    const ac = new AbortController()
    const s = await attach(runEventStream('run-q', ac.signal), ac)

    // A caller with a WIDER object than the parameter type: TypeScript's
    // excess-property check does not fire for a variable, which is exactly the
    // case `publishRun`'s field-by-field serialization exists to survive.
    const wide = {
      ...runEvent({ runId: 'run-q', state: 'awaiting' }),
      question: { key: 'k', question: 'Archive the Contoso thread?', detail: 'secret detail', options: [] },
      internalNote: 'must not ship',
    }
    publishRun('run-q', wide as RunEvent)

    const got = frame(await s.next())
    expect(Object.keys(got).sort()).toEqual(['kind', 'phase', 'runId', 'state', 'type'])
    expect(got.question).toBeUndefined()
    expect(JSON.stringify(got)).not.toContain('secret detail')
    ac.abort()
  })

  it('carries the terminal error only when there is one', async () => {
    const ac = new AbortController()
    const s = await attach(runEventStream('run-e', ac.signal), ac)
    publishRun('run-e', runEvent({ runId: 'run-e', state: 'error', error: 'upstream refused' }))
    expect(frame(await s.next())).toMatchObject({ state: 'error', error: 'upstream refused' })
    ac.abort()
  })
})

describe('user:<id>', () => {
  it('carries id-shaped events only', async () => {
    const ac = new AbortController()
    const s = await attach(userEventStream('user-1', ac.signal), ac)

    const wide = {
      type: 'run',
      runId: 'run-a',
      state: 'awaiting',
      phase: 'about to archive #ACME-14',
      error: 'nope',
      question: { key: 'k', question: 'private', options: [] },
    }
    publishUser('user-1', wide as UserEvent)

    const got = frame(await s.next())
    expect(Object.keys(got).sort()).toEqual(['runId', 'state', 'type'])
    expect(JSON.stringify(got)).not.toContain('ACME-14')
    ac.abort()
  })

  it('keeps one person’s firehose off another person’s', async () => {
    const ac1 = new AbortController()
    const ac2 = new AbortController()
    const one = await attach(userEventStream('user-1', ac1.signal), ac1)
    const two = await attach(userEventStream('user-2', ac2.signal), ac2)

    publishUser('user-1', { type: 'run', runId: 'mine', state: 'awaiting' })
    publishUser('user-2', { type: 'run', runId: 'theirs', state: 'queued' })

    expect(frame(await one.next())).toMatchObject({ runId: 'mine' })
    expect(frame(await two.next())).toMatchObject({ runId: 'theirs' })

    ac1.abort()
    ac2.abort()
  })

  it('publishes a notification event id-shaped too', async () => {
    const ac = new AbortController()
    const s = await attach(userEventStream('user-3', ac.signal), ac)
    publishUser('user-3', { type: 'notification', notificationId: 'n-1' })
    expect(frame(await s.next())).toEqual({ type: 'notification', notificationId: 'n-1' })
    ac.abort()
  })

  it('disconnects its dedicated subscriber when the request aborts', async () => {
    const ac = new AbortController()
    const before = hub.length
    const s = await attach(userEventStream('user-4', ac.signal), ac)
    const mine = hub[before]
    expect(mine).toBeDefined()
    expect(mine!.disconnected).toBe(false)
    s.abort()
    expect(mine!.disconnected).toBe(true)
  })
})

// ── The gate ─────────────────────────────────────────────────────────────────

type Row = { ownerUserId: string | null; subjectType: string | null; subjectId: string | null }

function watchDeps(row: Row | null, over: Partial<RunWatchDeps> = {}): RunWatchDeps {
  return {
    getRun: async () => row,
    boardRole: async () => null,
    channelRole: async () => null,
    taskBoardId: async () => null,
    conversationAccess: async () => null,
    // Default NO, so every case below that does not opt in is asserting the
    // ordinary-member answer — including the org-wide refusal.
    isAdmin: async () => false,
    ...over,
  }
}

const ownerless = (subjectType: string | null, subjectId: string | null): Row => ({
  ownerUserId: null,
  subjectType,
  subjectId,
})

describe('mayWatchRun', () => {
  it('allows the owner', async () => {
    const v = await mayWatchRun('u1', 'r1', watchDeps({ ownerUserId: 'u1', subjectType: null, subjectId: null }))
    expect(v).toEqual({ ok: true })
  })

  it('refuses a run that is not there, with the same answer as a refusal', async () => {
    expect(await mayWatchRun('u1', 'r1', watchDeps(null))).toEqual({ ok: false, reason: 'missing' })
  })

  it('refuses a stranger on somebody else’s subjectless run', async () => {
    const v = await mayWatchRun('u2', 'r1', watchDeps({ ownerUserId: 'u1', subjectType: null, subjectId: null }))
    expect(v).toEqual({ ok: false, reason: 'not-audience' })
  })

  // ── Org-wide runs: the widening, both directions ───────────────────────────
  //
  // A fitness sweep and a retrieval migration are ownerless AND subjectless, so
  // the original rule refused everybody — the run had a live stream and no
  // audience at all. The pair below is the whole rule: the shape decides who is
  // asked, and admin-ness decides the answer.

  it('lets an ADMIN watch an org-wide run (no owner, no subject)', async () => {
    const asked: string[] = []
    const v = await mayWatchRun(
      'u9',
      'r1',
      watchDeps(ownerless(null, null), {
        isAdmin: async (userId) => {
          asked.push(userId)
          return true
        },
      }),
    )
    expect(v).toEqual({ ok: true })
    // Asked about the CALLER, not about the run's absent owner.
    expect(asked).toEqual(['u9'])
  })

  it('refuses a non-admin on an org-wide run', async () => {
    expect(await mayWatchRun('u2', 'r1', watchDeps(ownerless(null, null)))).toEqual({ ok: false, reason: 'not-audience' })
  })

  it('does NOT let an admin watch somebody else’s owned run', async () => {
    // The widening is about a SHAPE OF ROW, not about being an admin. A run with
    // an owner is that person's, and an admin gets the same refusal as anyone.
    const admin = watchDeps({ ownerUserId: 'u1', subjectType: null, subjectId: null }, { isAdmin: async () => true })
    expect(await mayWatchRun('u2', 'r1', admin)).toEqual({ ok: false, reason: 'not-audience' })
  })

  it('does NOT let an admin watch a run whose SUBJECT refuses them', async () => {
    // A run about a board the admin is not a member of still resolves through
    // `boardRole`. Being an admin is not a read grant on somebody's board.
    const admin = watchDeps(ownerless('board', 'b1'), { isAdmin: async () => true, boardRole: async () => null })
    expect(await mayWatchRun('u2', 'r1', admin)).toEqual({ ok: false, reason: 'not-audience' })
  })

  it('does NOT let an admin watch a subject type it has no predicate for', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    const admin = watchDeps(ownerless('research', 'x1'), { isAdmin: async () => true })
    expect(await mayWatchRun('u2', 'r1', admin)).toEqual({ ok: false, reason: 'unknown-subject' })
    warn.mockRestore()
  })

  it('defers to boardRole for a board subject', async () => {
    const seen: string[] = []
    const allow = watchDeps(ownerless('board', 'b1'), {
      boardRole: async (userId, boardId) => {
        seen.push(`${userId}/${boardId}`)
        return 'editor'
      },
    })
    expect(await mayWatchRun('u2', 'r1', allow)).toEqual({ ok: true })
    expect(seen).toEqual(['u2/b1'])
    expect(await mayWatchRun('u2', 'r1', watchDeps(ownerless('board', 'b1')))).toEqual({
      ok: false,
      reason: 'not-audience',
    })
  })

  it('resolves a task subject through its board', async () => {
    const allow = watchDeps(ownerless('task', 't1'), {
      taskBoardId: async (taskId) => (taskId === 't1' ? 'b9' : null),
      boardRole: async (_userId, boardId) => (boardId === 'b9' ? 'viewer' : null),
    })
    expect(await mayWatchRun('u2', 'r1', allow)).toEqual({ ok: true })

    // A task whose board cannot be resolved is a refusal, not a fall-through to
    // some other predicate.
    const orphan = watchDeps(ownerless('task', 't1'), { boardRole: async () => 'owner' })
    expect(await mayWatchRun('u2', 'r1', orphan)).toEqual({ ok: false, reason: 'not-audience' })
  })

  it('defers to channelRole for a channel subject', async () => {
    const allow = watchDeps(ownerless('channel', 'c1'), { channelRole: async () => 'member' })
    expect(await mayWatchRun('u2', 'r1', allow)).toEqual({ ok: true })
    expect(await mayWatchRun('u2', 'r1', watchDeps(ownerless('channel', 'c1')))).toEqual({
      ok: false,
      reason: 'not-audience',
    })
  })

  it('defers to accessibleConversation for a conversation subject', async () => {
    const allow = watchDeps(ownerless('conversation', 'k1'), { conversationAccess: async () => ({ role: 'collaborator' }) })
    expect(await mayWatchRun('u2', 'r1', allow)).toEqual({ ok: true })
    expect(await mayWatchRun('u2', 'r1', watchDeps(ownerless('conversation', 'k1')))).toEqual({
      ok: false,
      reason: 'not-audience',
    })
  })

  it('REFUSES a subject type it has no predicate for, rather than allowing it', async () => {
    const warn = vi.spyOn(console, 'warn').mockImplementation(() => {})
    // Every predicate says yes; the verdict is still no, because the question
    // "who may read a `research` subject" has no answer in this file and a
    // default-allow would make every future subject type a silent widening.
    const permissive = watchDeps(ownerless('research', 'x1'), {
      boardRole: async () => 'owner',
      channelRole: async () => 'owner',
      conversationAccess: async () => ({}),
      taskBoardId: async () => 'b1',
    })
    expect(await mayWatchRun('u2', 'r1', permissive)).toEqual({ ok: false, reason: 'unknown-subject' })
    expect(warn).toHaveBeenCalled()
    warn.mockRestore()
  })

  it('refuses a subject type it knows but whose id is missing', async () => {
    expect(await mayWatchRun('u2', 'r1', watchDeps(ownerless('board', null)))).toEqual({
      ok: false,
      reason: 'not-audience',
    })
  })

  it('opens no Redis subscriber while deciding — a refusal must cost nothing', async () => {
    const before = hub.length
    await mayWatchRun('u2', 'r1', watchDeps(ownerless('research', 'x1'), {}))
    await mayWatchRun('u2', 'r1', watchDeps(null))
    expect(hub.length).toBe(before)
  })
})
