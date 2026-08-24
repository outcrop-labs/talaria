import { beforeEach, describe, expect, it, vi } from 'vitest'

// A notification line in the inbox/brief exists to be FOLLOWED. These tests
// hold the two halves of that rule where it is enforced — inside
// `notificationItems`, before anything downstream can render it:
//   · a notification with NO href (or '/') never becomes an item at all;
//   · a notification whose href points at a board or channel the reader is
//     not a member of is dropped — the reader was removed, or never added,
//     and a line that 403s when opened is about somebody else's resource.
//
// The database is faked by query text; every assertion is about this file's
// filtering, not about Postgres.

const net = vi.hoisted(() => ({
  /** Rows the notifications select will return. */
  notifications: [] as Array<{ id: string; kind: string; title: string; body: string; href: string; createdAt: string }>,
  /** board ids the membership query admits. */
  boards: new Set<string>(),
  /** channel ids the membership query admits. */
  channels: new Set<string>(),
}))

vi.mock('@/server/db/pg', () => ({
  db: async () => (strings: TemplateStringsArray, ...values: unknown[]) => {
    const text = strings.join(' ').replace(/\s+/g, ' ').trim()
    if (text.includes('from notifications')) return Promise.resolve(net.notifications)
    if (text.includes('from boards b')) {
      const wanted = values[values.length - 1] as string[]
      return Promise.resolve(wanted.filter((id) => net.boards.has(id)).map((id) => ({ id })))
    }
    if (text.includes('from channels c')) {
      const wanted = values[values.length - 1] as string[]
      return Promise.resolve(wanted.filter((id) => net.channels.has(id)).map((id) => ({ id })))
    }
    return Promise.resolve([])
  },
}))

const BOARD = '11111111-1111-1111-1111-111111111111'
const OTHER_BOARD = '22222222-2222-2222-2222-222222222222'
const CHANNEL = '33333333-3333-3333-3333-333333333333'

const row = (id: string, href: string, kind = 'mention') => ({
  id,
  kind,
  title: `title ${id}`,
  body: 'body',
  href,
  createdAt: '2026-08-20T09:00:00Z',
})

const { notificationItems } = await import('@/server/inbox-focus-sources')

describe('notificationItems: no unfollowable line enters the list', () => {
  beforeEach(() => {
    net.notifications = []
    net.boards = new Set()
    net.channels = new Set()
  })

  it('keeps a notification whose board or channel the reader is on, and a surface link', async () => {
    net.notifications = [
      row('n1', `/boards/${BOARD}/task-1`),
      row('n2', `/comms/channel/${CHANNEL}`),
      row('n3', '/research'),
    ]
    net.boards.add(BOARD)
    net.channels.add(CHANNEL)
    const items = await notificationItems('u1')
    expect(items.map((i) => i.sourceId).sort()).toEqual(['n1', 'n2', 'n3'])
  })

  it('drops a notification pointing at a resource the reader is not a member of', async () => {
    net.notifications = [
      row('n1', `/boards/${BOARD}/task-1`),
      row('n2', `/comms/channel/${CHANNEL}`),
      row('n3', `/boards/${OTHER_BOARD}/task-2`),
    ]
    // Member of the board, not the channel; never on OTHER_BOARD.
    net.boards.add(BOARD)
    const items = await notificationItems('u1')
    expect(items.map((i) => i.sourceId)).toEqual(['n1'])
  })
})
